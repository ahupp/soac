from __future__ import annotations

import ast
import contextlib
import gc
import importlib.util
import inspect
import os
import shlex
import subprocess
import sys
import tempfile
import textwrap
import uuid
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any, Iterator

from . import import_hook

MAX_CODEX_CONTEXT_CHARS = 80_000


@dataclass
class ProfileRecord:
    name: str
    module_name: str
    source_path: Path
    work_dir: Path
    function_id: str | None = None
    last_result: Any | None = None

    @property
    def profile_dump(self) -> Path:
        return self.work_dir / "profile.bin"

    @property
    def vcode_path(self) -> Path:
        return self.work_dir / f"{self.name}.vcode"

    @property
    def annotated_clif_path(self) -> Path:
        return self.work_dir / f"{self.name}.annotated.clif"


class SoacOptimizationExplorer:
    def __init__(self, shell: Any, *, artifact_root: Path | None = None) -> None:
        self.shell = shell
        self.artifact_root = artifact_root or Path(
            tempfile.mkdtemp(prefix="soac-ipython-")
        )
        self.records: dict[str, ProfileRecord] = {}

    def profile(self, line: str) -> Any:
        target_name, args, kwargs = self._parse_profile_call(line)
        function = self._lookup_function(target_name)
        source = self._source_for_function(function)
        record = self._write_profile_module(target_name, source)

        with _patched_env(
            {
                "SOAC_WORK_DIR": str(record.work_dir),
                "SOAC_OPT_MODE": "profile",
                "SOAC_MODULE_ENABLED": f"path:{record.source_path.parent}",
            }
        ):
            module = _load_soac_module_from_path(record.module_name, record.source_path)
            try:
                profiled_function = getattr(module, target_name)
                result = profiled_function(*args, **kwargs)
                record.last_result = result
            finally:
                sys.modules.pop(record.module_name, None)
                del module
                gc.collect()

        if not record.profile_dump.exists():
            raise RuntimeError(
                f"SOAC did not write profile counters at {record.profile_dump}"
            )

        self.records[target_name] = record
        print(f"profiled {target_name}; counters: {record.profile_dump}")
        return record.last_result

    def vcode(self, line: str) -> str:
        target_name = _parse_render_target(line, "%soac-vcode")
        record = self.records.get(target_name)
        if record is None:
            raise ValueError(
                f"{target_name!r} has not been profiled; run %soac-profile {target_name}(...) first"
            )

        function_id = record.function_id or self._lookup_function_id(record, target_name)
        record.function_id = function_id
        self._render_vcode(record, function_id)
        output = record.vcode_path.read_text(encoding="utf-8")
        print(output, end="" if output.endswith("\n") else "\n")
        return output

    def clif_annotate(self, line: str) -> str:
        target_name = _parse_render_target(line, "%soac-clif-annotate")
        record = self.records.get(target_name)
        if record is None:
            raise ValueError(
                f"{target_name!r} has not been profiled; run %soac-profile {target_name}(...) first"
            )

        function_id = record.function_id or self._lookup_function_id(record, target_name)
        record.function_id = function_id
        clif = self._render_clif(record, function_id).stdout
        source = record.source_path.read_text(encoding="utf-8")
        counter_summary = self._counter_summary(record)
        prompt = _build_clif_annotation_prompt(
            record=record,
            function_id=function_id,
            source=source,
            clif=clif,
            counter_summary=counter_summary,
        )
        output = self._run_codex_annotator(prompt, record.annotated_clif_path)
        print(output, end="" if output.endswith("\n") else "\n")
        return output

    def clif(self, line: str) -> str:
        target_name = _parse_render_target(line, "%soac-clif")
        record = self.records.get(target_name)
        if record is None:
            raise ValueError(
                f"{target_name!r} has not been profiled; run %soac-profile {target_name}(...) first"
            )

        function_id = record.function_id or self._lookup_function_id(record, target_name)
        record.function_id = function_id
        result = self._render_clif(record, function_id)
        output = result.stdout
        print(output, end="" if output.endswith("\n") else "\n")
        return output

    def _parse_profile_call(self, line: str) -> tuple[str, tuple[Any, ...], dict[str, Any]]:
        stripped = line.strip()
        if not stripped:
            raise ValueError("usage: %soac-profile function_name(args...)")
        parsed = ast.parse(stripped, mode="eval")
        expr = parsed.body
        if isinstance(expr, ast.Name):
            return expr.id, (), {}
        if not isinstance(expr, ast.Call) or not isinstance(expr.func, ast.Name):
            raise ValueError("usage: %soac-profile function_name(args...)")
        args = tuple(self._eval_ast_expr(arg) for arg in expr.args)
        kwargs: dict[str, Any] = {}
        for keyword in expr.keywords:
            value = self._eval_ast_expr(keyword.value)
            if keyword.arg is None:
                kwargs.update(value)
            else:
                kwargs[keyword.arg] = value
        return expr.func.id, args, kwargs

    def _eval_ast_expr(self, node: ast.AST) -> Any:
        expr = ast.Expression(body=node)
        ast.fix_missing_locations(expr)
        user_ns = getattr(self.shell, "user_ns", {})
        user_global_ns = getattr(self.shell, "user_global_ns", user_ns)
        return eval(compile(expr, "<soac-profile>", "eval"), user_global_ns, user_ns)

    def _lookup_function(self, target_name: str) -> Any:
        user_ns = getattr(self.shell, "user_ns", {})
        function = user_ns.get(target_name)
        if function is None:
            raise NameError(f"{target_name!r} is not defined in the IPython namespace")
        if not inspect.isfunction(function):
            raise TypeError(f"%soac-profile currently supports Python functions, got {function!r}")
        if "." in function.__qualname__ or "<locals>" in function.__qualname__:
            raise TypeError("%soac-profile currently supports top-level functions only")
        return function

    def _source_for_function(self, function: Any) -> str:
        try:
            source = inspect.getsource(function)
        except OSError as err:
            raise ValueError(
                f"could not recover source for {function.__name__!r}; define it in an IPython cell"
            ) from err
        return textwrap.dedent(source).strip() + "\n"

    def _write_profile_module(self, target_name: str, source: str) -> ProfileRecord:
        unique = uuid.uuid4().hex
        module_name = f"_soac_ipython_{target_name}_{unique}"
        module_dir = self.artifact_root / module_name
        module_dir.mkdir(parents=True, exist_ok=True)
        source_path = module_dir / f"{module_name}.py"
        source_path.write_text(source, encoding="utf-8")
        return ProfileRecord(
            name=target_name,
            module_name=module_name,
            source_path=source_path,
            work_dir=module_dir / "counters",
        )

    def _lookup_function_id(self, record: ProfileRecord, target_name: str) -> str:
        result = self._run_inspector("list_jit_functions", str(record.source_path))
        matches: list[str] = []
        for raw_line in result.stdout.splitlines():
            function_id, separator, qualname = raw_line.partition("\t")
            if separator and qualname == target_name:
                matches.append(function_id)
        if not matches:
            raise RuntimeError(f"could not find JIT function id for {target_name!r}")
        if len(matches) > 1:
            raise RuntimeError(f"ambiguous JIT function id for {target_name!r}: {matches}")
        return matches[0]

    def _render_vcode(self, record: ProfileRecord, function_id: str) -> None:
        self._render_clif(
            record,
            function_id,
            extra_args=("--vcode-out", str(record.vcode_path)),
        )

    def _render_clif(
        self,
        record: ProfileRecord,
        function_id: str,
        *,
        extra_args: tuple[str, ...] = (),
    ) -> subprocess.CompletedProcess[str]:
        env = {
            **os.environ,
            "SOAC_WORK_DIR": str(record.work_dir),
            "SOAC_OPT_MODE": "apply",
        }
        return self._run_inspector(
            "render_jit_clif",
            "--specialized",
            "--module-name",
            record.module_name,
            *extra_args,
            str(record.source_path),
            function_id,
            env=env,
        )

    def _counter_summary(self, record: ProfileRecord) -> str:
        if not record.profile_dump.exists():
            return f"profile counter dump not found at {record.profile_dump}"
        try:
            result = self._run_inspector(
                "inspect_counters",
                "--specializations",
                str(record.profile_dump),
            )
        except subprocess.CalledProcessError as err:
            stderr = err.stderr or ""
            stdout = err.stdout or ""
            return f"inspect_counters failed:\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
        return result.stdout

    def _run_codex_annotator(self, prompt: str, output_path: Path) -> str:
        output_path.parent.mkdir(parents=True, exist_ok=True)
        command = [
            "codex",
            "exec",
            "--sandbox",
            "read-only",
            "--ephemeral",
            "--color",
            "never",
            "-C",
            str(import_hook.REPO_ROOT),
            "-o",
            str(output_path),
            "-",
        ]
        env = {**os.environ, "NO_COLOR": "1"}
        try:
            result = subprocess.run(
                command,
                cwd=import_hook.REPO_ROOT,
                env=env,
                input=prompt,
                check=True,
                capture_output=True,
                text=True,
            )
        except FileNotFoundError as err:
            raise RuntimeError(
                "codex executable not found; install Codex CLI or ensure `codex` is on PATH"
            ) from err
        except subprocess.CalledProcessError as err:
            stderr = err.stderr or ""
            stdout = err.stdout or ""
            raise RuntimeError(
                f"codex exec failed with exit code {err.returncode}\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}"
            ) from err

        if output_path.exists():
            return output_path.read_text(encoding="utf-8")
        if result.stdout.strip():
            return result.stdout
        raise RuntimeError("codex exec completed but did not write annotated CLIF")

    def _run_inspector(
        self, bin_name: str, *args: str, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        command = [
            "cargo",
            "run",
            "-q",
            "-p",
            "soac-inspector",
            "--bin",
            bin_name,
            "--",
            *args,
        ]
        return subprocess.run(
            command,
            cwd=import_hook.REPO_ROOT,
            env=env,
            check=True,
            capture_output=True,
            text=True,
        )


def _parse_render_target(line: str, magic_name: str) -> str:
    parts = shlex.split(line)
    if len(parts) != 1:
        raise ValueError(f"usage: {magic_name} function_name")
    return parts[0]


def _build_clif_annotation_prompt(
    *,
    record: ProfileRecord,
    function_id: str,
    source: str,
    clif: str,
    counter_summary: str,
) -> str:
    return f"""Annotate SOAC generated Cranelift IR for an interactive optimization session.

Return only annotated CLIF. Do not include markdown fences, an introduction, or a summary.
Preserve the original CLIF order and text as much as possible.
Use CLIF comments beginning with `;`.
Add a short comment immediately before each block describing what that block does.
Add inline comments for important guards, specialization fast paths, slow paths, refcount cleanup,
direct indexed dict access, calls, and exception edges when they are visible.
Tie comments back to the Python source and counter evidence. If a block purpose is inferred, say so.

Function metadata:
- function name: {record.name}
- module name: {record.module_name}
- function id: {function_id}
- source path: {record.source_path}
- counter dump: {record.profile_dump}

Python source:
{_bounded_context(source)}

Decoded counter/specialization context:
{_bounded_context(counter_summary)}

Specialized CLIF to annotate:
{_bounded_context(clif)}
"""


def _bounded_context(value: str, *, limit: int = MAX_CODEX_CONTEXT_CHARS) -> str:
    if len(value) <= limit:
        return value
    omitted = len(value) - limit
    return f"{value[:limit]}\n... truncated {omitted} chars ..."


def _load_soac_module_from_path(module_name: str, source_path: Path) -> ModuleType:
    spec = importlib.util.spec_from_file_location(module_name, source_path)
    if spec is None or spec.loader is None:
        raise ImportError(f"could not create module spec for {source_path}")
    spec.loader = import_hook.SoacLoader(module_name, str(source_path.resolve()))
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    try:
        spec.loader.exec_module(module)
    except Exception:
        sys.modules.pop(module_name, None)
        raise
    return module


@contextlib.contextmanager
def _patched_env(values: dict[str, str]) -> Iterator[None]:
    prior = {name: os.environ.get(name) for name in values}
    try:
        os.environ.update(values)
        yield
    finally:
        for name, value in prior.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def load_ipython_extension(ipython: Any) -> None:
    from IPython.core.magic import Magics, line_magic, magics_class

    explorer = SoacOptimizationExplorer(ipython)

    @magics_class
    class SoacMagics(Magics):
        @line_magic("soac-profile")
        def soac_profile(self, line: str) -> Any:
            return explorer.profile(line)

        @line_magic("soac-vcode")
        def soac_vcode(self, line: str) -> None:
            # The explorer prints the formatted text. Returning the string would
            # make IPython also display its repr as a second large blob.
            explorer.vcode(line)

        @line_magic("soac-clif")
        def soac_clif(self, line: str) -> None:
            # The explorer prints the formatted text. Returning the string would
            # make IPython also display its repr as a second large blob.
            explorer.clif(line)

        @line_magic("soac-clif-annotate")
        def soac_clif_annotate(self, line: str) -> None:
            # The explorer prints the formatted text. Returning the string would
            # make IPython also display its repr as a second large blob.
            explorer.clif_annotate(line)

    ipython.register_magics(SoacMagics)
    ipython.soac_explorer = explorer
