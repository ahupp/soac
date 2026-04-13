#!/usr/bin/env python3
from __future__ import annotations

import argparse
import ast
import contextlib
import gc
import importlib.util
import json
import os
import re
import subprocess
import sys
import uuid
from pathlib import Path
from types import ModuleType
from typing import Any, Iterator

from soac import import_hook


def main() -> int:
    args = parse_args()
    source = read_source(args)
    target_name = args.function or infer_workload_target(args.workload)

    artifact_dir = choose_artifact_dir(args, target_name)
    artifact_dir.mkdir(parents=True, exist_ok=True)
    source_path = artifact_dir / "source.py"
    source_path.write_text(source.rstrip() + "\n", encoding="utf-8")
    (artifact_dir / "workload.txt").write_text(args.workload + "\n", encoding="utf-8")

    module_name = f"_soac_skill_{sanitize_name(target_name)}_{uuid.uuid4().hex}"
    counters_dir = artifact_dir / "counters"
    counters_dir.mkdir(parents=True, exist_ok=True)

    result_repr = profile_workload(
        module_name=module_name,
        source_path=source_path,
        workload=args.workload,
        counters_dir=counters_dir,
    )
    (artifact_dir / "result_repr.txt").write_text(result_repr + "\n", encoding="utf-8")

    profile_dump = counters_dir / "profile.bin"
    if not profile_dump.exists():
        raise RuntimeError(f"SOAC did not write profile counters at {profile_dump}")

    function_id = lookup_function_id(source_path, target_name)
    specializations = inspect_specializations(profile_dump)
    clif = render_specialized_clif(
        module_name=module_name,
        source_path=source_path,
        function_id=function_id,
        counters_dir=counters_dir,
    )

    specializations_path = artifact_dir / "specializations.txt"
    clif_path = artifact_dir / "specialized.clif"
    context_path = artifact_dir / "annotation_context.md"
    metadata_path = artifact_dir / "metadata.json"

    specializations_path.write_text(specializations, encoding="utf-8")
    clif_path.write_text(clif, encoding="utf-8")
    metadata = {
        "artifact_dir": str(artifact_dir),
        "module_name": module_name,
        "function_name": target_name,
        "function_id": function_id,
        "source_path": str(source_path),
        "workload": args.workload,
        "result_repr": result_repr,
        "profile_dump": str(profile_dump),
        "specializations_path": str(specializations_path),
        "clif_path": str(clif_path),
        "annotation_context_path": str(context_path),
    }
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    context_path.write_text(
        annotation_context(
            metadata=metadata,
            source=source_path.read_text(encoding="utf-8"),
            specializations=specializations,
            clif=clif,
        ),
        encoding="utf-8",
    )

    print(f"artifact_dir: {artifact_dir}")
    print(f"annotation_context: {context_path}")
    print(f"specialized_clif: {clif_path}")
    print(f"specializations: {specializations_path}")
    print(f"metadata: {metadata_path}")
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Profile a Python snippet workload and render specialized SOAC CLIF."
    )
    parser.add_argument(
        "--workload",
        required=True,
        help="Expression to evaluate under profile mode, for example 'add(1, 1)'.",
    )
    parser.add_argument(
        "--function",
        help="Function qualname to render. Defaults to the direct call target in --workload.",
    )
    parser.add_argument(
        "--source-file",
        type=Path,
        help="Path to Python source. If omitted, source is read from stdin.",
    )
    parser.add_argument(
        "--artifact-dir",
        type=Path,
        help="Directory for generated artifacts. Defaults to logs/soac-clif-snippets/<name>-<id>.",
    )
    return parser.parse_args()


def read_source(args: argparse.Namespace) -> str:
    if args.source_file is not None:
        source = args.source_file.read_text(encoding="utf-8")
    else:
        source = sys.stdin.read()
    if not source.strip():
        raise ValueError("Python source is empty")
    return source


def choose_artifact_dir(args: argparse.Namespace, target_name: str) -> Path:
    if args.artifact_dir is not None:
        return args.artifact_dir
    unique = uuid.uuid4().hex[:12]
    return (
        Path(import_hook.REPO_ROOT)
        / "logs"
        / "soac-clif-snippets"
        / f"{sanitize_name(target_name)}-{unique}"
    )


def infer_workload_target(workload: str) -> str:
    parsed = ast.parse(workload, mode="eval")
    expr = parsed.body
    if isinstance(expr, ast.Call) and isinstance(expr.func, ast.Name):
        return expr.func.id
    raise ValueError(
        "Could not infer function from workload; pass --function <qualname>."
    )


def sanitize_name(value: str) -> str:
    sanitized = re.sub(r"[^0-9A-Za-z_]+", "_", value).strip("_")
    return sanitized or "snippet"


def profile_workload(
    *,
    module_name: str,
    source_path: Path,
    workload: str,
    counters_dir: Path,
) -> str:
    with patched_env(
        {
            "SOAC_WORK_DIR": str(counters_dir),
            "SOAC_OPT_MODE": "profile",
            "SOAC_MODULE_ENABLED": f"path:{source_path.parent}",
        }
    ):
        module = load_soac_module_from_path(module_name, source_path)
        try:
            result = eval(
                compile(workload, "<soac-snippet-workload>", "eval"),
                module.__dict__,
                module.__dict__,
            )
            return safe_repr(result)
        finally:
            sys.modules.pop(module_name, None)
            del module
            gc.collect()


def load_soac_module_from_path(module_name: str, source_path: Path) -> ModuleType:
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


def lookup_function_id(source_path: Path, target_name: str) -> str:
    result = run_inspector("list_jit_functions", str(source_path))
    matches: list[str] = []
    available: list[str] = []
    for raw_line in result.stdout.splitlines():
        function_id, separator, qualname = raw_line.partition("\t")
        if not separator:
            continue
        available.append(f"{function_id}\t{qualname}")
        if qualname == target_name:
            matches.append(function_id)
    if not matches:
        raise RuntimeError(
            f"could not find JIT function id for {target_name!r}; available:\n"
            + "\n".join(available)
        )
    if len(matches) > 1:
        raise RuntimeError(f"ambiguous JIT function id for {target_name!r}: {matches}")
    return matches[0]


def inspect_specializations(profile_dump: Path) -> str:
    try:
        return run_inspector(
            "inspect_counters",
            "--specializations",
            str(profile_dump),
        ).stdout
    except subprocess.CalledProcessError as err:
        return (
            "inspect_counters failed\n"
            f"exit_code: {err.returncode}\n"
            f"stdout:\n{err.stdout or ''}\n"
            f"stderr:\n{err.stderr or ''}\n"
        )


def render_specialized_clif(
    *,
    module_name: str,
    source_path: Path,
    function_id: str,
    counters_dir: Path,
) -> str:
    env = {
        **os.environ,
        "SOAC_WORK_DIR": str(counters_dir),
        "SOAC_OPT_MODE": "apply",
    }
    return run_inspector(
        "render_jit_clif",
        "--specialized",
        "--module-name",
        module_name,
        str(source_path),
        function_id,
        env=env,
    ).stdout


def run_inspector(
    bin_name: str,
    *args: str,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            "cargo",
            "run",
            "-q",
            "-p",
            "soac-inspector",
            "--bin",
            bin_name,
            "--",
            *args,
        ],
        cwd=import_hook.REPO_ROOT,
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )


def annotation_context(
    *,
    metadata: dict[str, Any],
    source: str,
    specializations: str,
    clif: str,
) -> str:
    return f"""# SOAC CLIF Annotation Context

## Instructions

Annotate the specialized CLIF below. Preserve CLIF order and add `;` comments
before each block and beside important instructions. Explain guards, fast paths,
slow paths, exception edges, helper calls, refcount cleanup, and direct indexed
access. Use the source and counter context as evidence; mark uncertain block
purposes as inferred.

## Metadata

```json
{json.dumps(metadata, indent=2)}
```

## Python Source

```python
{source.rstrip()}
```

## Specialization Counters

```text
{specializations.rstrip()}
```

## Specialized CLIF

```clif
{clif.rstrip()}
```
"""


def safe_repr(value: object) -> str:
    try:
        return repr(value)
    except Exception as err:
        return f"<repr failed: {err}>"


@contextlib.contextmanager
def patched_env(values: dict[str, str]) -> Iterator[None]:
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


if __name__ == "__main__":
    raise SystemExit(main())
