#!/usr/bin/env python3
"""Prepare disclosed strict benchmark sources and their offline contracts.

Source manifests are measurement provenance, not runtime authority. The real
offline checker authenticates the exact immutable project for the prepared
benchmark interpreter before any worker starts.
"""

from __future__ import annotations
import __future__

import argparse
import ast
import copy
import hashlib
import io
import json
import keyword
import os
import shutil
import stat
import subprocess
import symtable
import tempfile
import tokenize
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any

import tomllib

SCHEMA = 3
SELECTION_POLICY = "driver-local-static-imports-v1"
HARNESS_POLICY = "terminal-main-measurement-suffix-v1"
CONFIGURATION_POLICY = "preserve-upstream-append-strict-table-v1"
EXECUTION_SCHEMA = 1


def _digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def strict_opt_in(source: bytes, filename: str) -> tuple[bytes, int]:
    """Insert only the future feature, retaining encoding, comments and code."""
    encoding, _ = tokenize.detect_encoding(io.BytesIO(source).readline)
    text = source.decode(encoding)
    original = ast.parse(text, filename=filename)
    if any(
        isinstance(node, ast.ImportFrom)
        and node.module == "__future__"
        and any(alias.name == "strict" for alias in node.names)
        for node in original.body
    ):
        raise ValueError(f"stock benchmark already opts into strict: {filename}")

    insertion_line = 0
    body = iter(original.body)
    first = next(body, None)
    if (
        isinstance(first, ast.Expr)
        and isinstance(first.value, ast.Constant)
        and isinstance(first.value.value, str)
    ):
        insertion_line = first.end_lineno
        first = next(body, None)
    while isinstance(first, ast.ImportFrom) and first.module == "__future__":
        insertion_line = first.end_lineno
        first = next(body, None)

    lines = text.splitlines(keepends=True)
    # Keep a shebang/coding cookie within the physical first two lines.
    # With no docstring/future imports, leading comments may stay before the
    # added future without changing Python's first-statement semantics.
    if insertion_line == 0:
        while insertion_line < len(lines):
            stripped = lines[insertion_line].lstrip()
            if stripped.startswith("#") or not stripped.strip():
                insertion_line += 1
            else:
                break
    newline = "\r\n" if "\r\n" in text else "\n"
    if first is not None and first.lineno == insertion_line:
        # A docstring/future header can share its physical line with ordinary
        # statements. Insert before that first ordinary statement, preserving
        # all existing bytes and the future-import ordering rule. AST columns
        # use UTF-8 bytes even when the original source has another encoding.
        header_line = lines[insertion_line - 1].encode("utf-8")
        before = "".join(lines[:insertion_line - 1]) + header_line[:first.col_offset].decode("utf-8")
        after = header_line[first.col_offset:].decode("utf-8") + "".join(lines[insertion_line:])
        candidate = before + "from __future__ import strict; " + after
        added_line = insertion_line
    else:
        before = "".join(lines[:insertion_line])
        after = "".join(lines[insertion_line:])
        if before and not before.endswith(("\n", "\r")):
            before += newline
        candidate = before + "from __future__ import strict" + newline + after
        added_line = insertion_line + 1

    transformed = ast.parse(candidate, filename=filename)
    added = [
        (index, node)
        for index, node in enumerate(transformed.body)
        if isinstance(node, ast.ImportFrom)
        and node.module == "__future__"
        and len(node.names) == 1
        and node.names[0].name == "strict"
    ]
    if len(added) != 1:
        raise ValueError(f"strict overlay has no unique future insertion: {filename}")
    del transformed.body[added[0][0]]
    if ast.dump(original, include_attributes=False) != ast.dump(
        transformed, include_attributes=False
    ):
        raise ValueError(f"strict overlay changed benchmark syntax: {filename}")
    return candidate.encode(encoding), added_line


def _main_guard(node: ast.stmt) -> bool:
    return (
        isinstance(node, ast.If)
        and not node.orelse
        and isinstance(node.test, ast.Compare)
        and isinstance(node.test.left, ast.Name)
        and node.test.left.id == "__name__"
        and len(node.test.ops) == 1
        and isinstance(node.test.ops[0], ast.Eq)
        and len(node.test.comparators) == 1
        and isinstance(node.test.comparators[0], ast.Constant)
        and node.test.comparators[0].value == "__main__"
    )


def _callable_global_reads(table) -> set[str]:
    """Names read after initialization, excluding already-executed module code."""
    result = set()
    for child in table.get_children():
        result.update(
            symbol.get_name()
            for symbol in child.get_symbols()
            if symbol.is_global() and symbol.is_referenced()
        )
        result.update(_callable_global_reads(child))
    return result


def project_driver_harness(
    source: bytes, filename: str
) -> tuple[bytes, bytes, dict[str, Any]]:
    """Keep setup in module initialization, move measurement after its seal.

    This is a disclosed benchmark-source projection, never a compiler special
    case. Definitions, setup, and measurement statements retain their order and
    syntax. The ordinary measurement suffix has a copied globals dictionary;
    it cannot rebind globals read by workload callables. Unknown main shapes or
    reflective namespace access fail preparation instead of measuring an
    initializing module or silently rewriting a workload algorithm.
    """
    encoding, _ = tokenize.detect_encoding(io.BytesIO(source).readline)
    text = source.decode(encoding)
    original = ast.parse(text, filename=filename)
    if any(
        isinstance(node, ast.ImportFrom)
        and node.module == "__future__"
        and any(alias.name == "strict" for alias in node.names)
        for node in original.body
    ):
        raise ValueError(f"stock benchmark already opts into strict: {filename}")
    if not original.body or not _main_guard(original.body[-1]):
        raise ValueError(f"benchmark requires a terminal __main__ guard: {filename}")
    guard = original.body[-1]
    if guard.body[0].lineno == guard.lineno:
        raise ValueError(f"benchmark main guard must have a multiline body: {filename}")

    functions = {
        node.name: node
        for node in original.body
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    }
    measurement_functions: set[str] = set()

    def measures(node: ast.AST) -> bool:
        return any(
            isinstance(item, ast.Call)
            and (
                isinstance(item.func, ast.Attribute)
                and item.func.attr.startswith("bench_")
                or isinstance(item.func, ast.Name)
                and item.func.id in measurement_functions
            )
            for item in ast.walk(node)
        )

    while True:
        discovered = {name for name, node in functions.items() if measures(node)}
        if discovered <= measurement_functions:
            break
        measurement_functions.update(discovered)
    cut = next((index for index, node in enumerate(guard.body) if measures(node)), None)
    if cut is None:
        raise ValueError(
            f"benchmark has no statically selected measurement suffix: {filename}"
        )
    suffix = guard.body[cut:]
    for statement in suffix:
        parents = {
            child: parent
            for parent in ast.walk(statement)
            for child in ast.iter_child_nodes(parent)
        }
        for node in ast.walk(statement):
            if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Name):
                continue
            if node.func.id not in {"globals", "locals", "vars", "eval", "exec"}:
                continue
            parent = parents.get(node)
            # Read-only selection by name is common to generic benchmark
            # dispatchers. It observes the ordinary harness's own real dict;
            # do not hand a writable alias to unknown code or execute strings.
            if not (
                node.func.id in {"globals", "locals"}
                and not node.args
                and not node.keywords
                and isinstance(parent, ast.Subscript)
                and isinstance(parent.ctx, ast.Load)
            ):
                raise ValueError(
                    f"benchmark measurement suffix reflects on its namespace: {filename}"
                )

    lines = text.splitlines(keepends=True)
    first = suffix[0]
    cut_line = min(
        [first.lineno, *(node.lineno for node in getattr(first, "decorator_list", ()))]
    )
    if cut and guard.body[cut - 1].end_lineno >= cut_line:
        raise ValueError(
            f"benchmark measurement suffix shares a setup line: {filename}"
        )

    def blank(line: str) -> str:
        return "\r\n" if line.endswith("\r\n") else "\n" if line.endswith("\n") else ""

    prefix_lines = [
        line if index < cut_line - 1 else blank(line)
        for index, line in enumerate(lines)
    ]
    if not cut:
        indent = lines[cut_line - 1][
            : len(lines[cut_line - 1]) - len(lines[cut_line - 1].lstrip())
        ]
        prefix_lines[cut_line - 1] = (
            indent + "pass" + (blank(lines[cut_line - 1]) or "\n")
        )
    harness_lines = [blank(line) for line in lines]
    # Preserve coding cookies/shebangs, but no executable module prelude.
    for index, line in enumerate(lines[:2]):
        if line.lstrip().startswith("#"):
            harness_lines[index] = line
    for index in range(guard.lineno - 1, guard.body[0].lineno - 1):
        harness_lines[index] = lines[index]
    harness_lines[cut_line - 1 :] = lines[cut_line - 1 :]
    prefix = "".join(prefix_lines)
    harness = "".join(harness_lines)
    prefix_ast = ast.parse(prefix, filename=filename)
    harness_ast = ast.parse(harness, filename=filename)
    if len(harness_ast.body) != 1 or not _main_guard(harness_ast.body[0]):
        raise ValueError(
            f"benchmark harness projection changed control flow: {filename}"
        )
    # A structured syntax comparison covers every original statement, not a
    # renderer substring or a hand-maintained benchmark-specific assertion.
    prefix_ast.body[-1].body = [*guard.body[:cut], *harness_ast.body[0].body]
    if ast.dump(prefix_ast, include_attributes=False) != ast.dump(
        original, include_attributes=False
    ):
        raise ValueError(f"benchmark projection changed workload syntax: {filename}")

    suffix_table = symtable.symtable(harness, filename, "exec")
    writes = {
        symbol.get_name()
        for symbol in suffix_table.get_symbols()
        if symbol.is_assigned() or symbol.is_imported() or symbol.is_namespace()
    }
    reads = _callable_global_reads(symtable.symtable(text, filename, "exec"))
    if writes and reads & {"globals", "eval", "exec"}:
        raise ValueError(
            f"benchmark workload dynamically reads harness globals: {filename}"
        )
    if overlap := writes & reads:
        raise ValueError(
            f"benchmark measurement suffix rebinds workload globals: {', '.join(sorted(overlap))}"
        )
    future_mask = 0
    for name in __future__.all_feature_names:
        future_mask |= getattr(__future__, name).compiler_flag
    flags = compile(source, filename, "exec", dont_inherit=True).co_flags & future_mask
    harness_bytes = harness.encode(encoding)
    projection = {
        "policy": HARNESS_POLICY,
        "cut_line": cut_line,
        "suffix_statement_index": cut,
        "compiler_flags": flags,
        "harness_sha256": _digest(harness_bytes),
    }
    return prefix.encode(encoding), harness_bytes, projection


def _module_name(relative: Path, entry: Path) -> str | None:
    if relative == entry:
        return "__main__"
    if relative.suffix != ".py":
        return None
    parts = list(relative.with_suffix("").parts)
    if parts[-1] == "__init__":
        parts.pop()
    if not parts or any(
        not part.isidentifier() or keyword.iskeyword(part) for part in parts
    ):
        return None
    return ".".join(parts)


def _source_inventory(root: Path) -> list[Path]:
    result = []
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if any(part in {"__pycache__", ".git", ".jj"} for part in relative.parts):
            continue
        # Do not dereference an outside source or silently substitute linked
        # data. Such a driver needs an explicit source-selection policy.
        if path.is_symlink():
            raise ValueError(f"benchmark overlay does not support symlinks: {relative}")
        if path.is_file():
            result.append(relative)
    return result


def _selected_modules(root: Path, entry: Path, inventory: list[Path]) -> dict[str, str]:
    """Select the driver and statically imported local Python modules only.

    A .py file can be benchmark input (for example a parser/compiler fixture),
    not executable workload code. Merely copying a driver directory must not
    add a future statement to that data. Dynamic imports and dependencies
    outside the driver directory stay ordinary under this fixed policy.
    """
    candidates: dict[str, Path] = {}
    for relative in inventory:
        if (name := _module_name(relative, entry)) is not None:
            if name in candidates:
                raise ValueError(f"ambiguous importable benchmark module: {name}")
            candidates[name] = relative
    selected = {"__main__": entry.as_posix()}
    pending = ["__main__"]

    def include(name: str) -> None:
        parts = name.split(".")
        # Importing a submodule first executes every containing package.
        for length in range(1, len(parts) + 1):
            prefix = ".".join(parts[:length])
            if prefix in candidates and prefix not in selected:
                selected[prefix] = candidates[prefix].as_posix()
                pending.append(prefix)

    while pending:
        name = pending.pop()
        relative = candidates[name]
        tree = ast.parse((root / relative).read_bytes(), filename=str(root / relative))
        package = name.split(".")
        if relative.name != "__init__.py":
            package.pop()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for alias in node.names:
                    include(alias.name)
            elif isinstance(node, ast.ImportFrom):
                if node.level:
                    if node.level > len(package):
                        continue
                    base = package[: len(package) - node.level + 1]
                else:
                    base = []
                if node.module:
                    base.extend(node.module.split("."))
                include(".".join(base))
                for alias in node.names:
                    if alias.name != "*":
                        include(".".join([*base, alias.name]))
    return dict(sorted(selected.items()))


def _source_fingerprint(manifest: dict[str, Any]) -> str:
    comparable = {
        "schema": manifest["schema"],
        "selection_policy": manifest["selection_policy"],
        "modules": manifest["modules"],
        "files": manifest["files"],
        "policy_sha256": manifest["policy_sha256"],
        "policy_projection": manifest["policy_projection"],
        "harness_projection": manifest["harness_projection"],
    }
    return _digest(
        json.dumps(comparable, sort_keys=True, separators=(",", ":")).encode()
    )


def _stock_fingerprint(records: list[dict[str, Any]]) -> str:
    inputs = [(record["relative_path"], record["stock_sha256"]) for record in records]
    return _digest(json.dumps(inputs, separators=(",", ":")).encode())


def stock_source_fingerprint(script: Path) -> str:
    script = script.resolve()
    return _stock_fingerprint(
        [
            {
                "relative_path": relative.as_posix(),
                "stock_sha256": _digest((script.parent / relative).read_bytes()),
            }
            for relative in _source_inventory(script.parent)
        ]
    )


def _project_strict_policy(
    upstream: bytes | None, modules: Mapping[str, str]
) -> tuple[bytes, dict[str, Any]]:
    """Preserve upstream bytes and add only the explicitly declared policy."""
    requested = {
        "include": sorted(modules.values()),
        "default_class_policy": "automatic",
        "unsupported_class_policy": "dynamic",
        "checked_fields": "disabled",
    }
    original = upstream if upstream is not None else b""
    try:
        parsed = tomllib.loads(original.decode("utf-8"))
    except (UnicodeError, tomllib.TOMLDecodeError) as error:
        raise ValueError("upstream pyproject.toml must be valid UTF-8 TOML") from error
    tool = parsed.get("tool", {})
    if not isinstance(tool, dict):
        # This is an invalid TOML value, not a Python API argument type.
        raise ValueError("upstream tool namespace conflicts with SOAC strict policy")  # noqa: TRY004
    soac = tool.get("soac", {})
    if not isinstance(soac, dict):
        # Preserve a uniform configuration-error boundary for parsed TOML.
        raise ValueError(  # noqa: TRY004 - invalid configuration value
            "upstream tool.soac namespace conflicts with SOAC strict policy"
        )
    if "strict" in soac:
        if soac["strict"] != requested:
            raise ValueError(
                "existing tool.soac.strict conflicts with benchmark policy"
            )
        appended = b""
    else:
        newline = b"\r\n" if b"\r\n" in original else b"\n"
        separator = newline if original else b""
        if original and not original.endswith(b"\n"):
            separator = newline + separator
        declarations = [
            "[tool.soac.strict]",
            *(
                f"{key} = {json.dumps(value, ensure_ascii=False)}"
                for key, value in requested.items()
            ),
        ]
        appended = (
            separator
            + newline.join(line.encode("utf-8") for line in declarations)
            + newline
        )
    policy = original + appended
    expected = copy.deepcopy(parsed)
    expected.setdefault("tool", {}).setdefault("soac", {})["strict"] = requested
    try:
        actual = tomllib.loads(policy.decode("utf-8"))
    except tomllib.TOMLDecodeError as error:
        # Inline-table namespaces cannot be extended by appending declarations.
        # Do not reserialize or overwrite their upstream bytes to make room.
        raise ValueError(
            "upstream TOML namespace cannot accept SOAC policy without rewriting"
        ) from error
    if actual != expected:
        raise ValueError("SOAC policy projection changed upstream configuration")
    return policy, {
        "policy": CONFIGURATION_POLICY,
        "upstream_sha256": None if upstream is None else _digest(upstream),
        "appended_utf8": appended.decode("utf-8"),
    }


def prepare_source_overlay(script: Path, output: Path) -> dict[str, Any]:
    script = script.absolute()
    if script.is_symlink() or not script.is_file() or script.suffix != ".py":
        raise ValueError("benchmark entry must be a real Python source file")
    script = script.resolve()
    root = script.parent
    output = output.absolute()
    if output.exists():
        raise ValueError(f"refusing to overwrite an existing strict overlay: {output}")
    if output.is_relative_to(root):
        raise ValueError("strict overlay must be outside the stock benchmark directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=f".{output.name}-", dir=output.parent))
    try:
        project = temporary / "project"
        project.mkdir()
        entry = Path(script.name)
        inventory = _source_inventory(root)
        modules = _selected_modules(root, entry, inventory)
        driver_prefix, harness, projection = project_driver_harness(
            script.read_bytes(), str(script)
        )
        (temporary / "harness.py").write_bytes(harness)
        selected_paths = {relative: name for name, relative in modules.items()}
        upstream_policy = (
            (root / "pyproject.toml").read_bytes()
            if Path("pyproject.toml") in inventory
            else None
        )
        policy, policy_projection = _project_strict_policy(upstream_policy, modules)
        files = []
        for relative in inventory:
            original_path = root / relative
            source = original_path.read_bytes()
            name = selected_paths.get(relative.as_posix())
            if name is not None:
                candidate, insertion = strict_opt_in(
                    driver_prefix if relative == entry else source, str(original_path)
                )
            elif relative == Path("pyproject.toml"):
                candidate, insertion = policy, None
            else:
                candidate, insertion = source, None
            destination = project / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(candidate)
            files.append(
                {
                    "relative_path": relative.as_posix(),
                    "stock_sha256": _digest(source),
                    "strict_sha256": _digest(candidate),
                    "module_name": name,
                    "strict_future_line": insertion,
                }
            )
        (project / "pyproject.toml").write_bytes(policy)
        manifest = {
            "schema": SCHEMA,
            "selection_policy": SELECTION_POLICY,
            "stock_script": str(script),
            "strict_script": str(output / "project" / entry),
            "project": str(output / "project"),
            "modules": dict(sorted(modules.items())),
            "files": files,
            "policy_sha256": _digest(policy),
            "policy_projection": policy_projection,
            "harness_projection": projection,
            "harness_script": str(output / "harness.py"),
            "language_difference": "strict future opt-in; declared SOAC policy under a byte-preserving configuration projection; unchanged terminal measurement suffix runs through an ordinary harness after module sealing",
        }
        manifest["source_fingerprint"] = _source_fingerprint(manifest)
        manifest["stock_source_fingerprint"] = _stock_fingerprint(files)
        canonical = json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
        manifest["overlay_fingerprint"] = _digest(canonical)
        (temporary / "source-manifest.json").write_text(
            json.dumps(manifest, sort_keys=True, indent=2) + "\n"
        )
        os.rename(temporary, output)
        return manifest
    finally:
        if temporary.exists():
            shutil.rmtree(temporary)


def verify_source_overlay(output: Path) -> dict[str, Any]:
    """Reject stale original inputs, changed policy, added files, or altered code."""
    manifest_path = output / "source-manifest.json"
    if manifest_path.is_symlink():
        raise ValueError("strict source manifest must not be a symlink")
    manifest = json.loads(manifest_path.read_text())
    fingerprint = manifest.pop("overlay_fingerprint")
    canonical = json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
    if manifest.get("schema") != SCHEMA or fingerprint != _digest(canonical):
        raise ValueError("strict source manifest fingerprint/schema mismatch")
    if manifest.get("selection_policy") != SELECTION_POLICY:
        raise ValueError("strict source selection policy mismatch")
    if manifest.get("source_fingerprint") != _source_fingerprint(manifest):
        raise ValueError("strict comparable source fingerprint mismatch")
    if manifest.get("stock_source_fingerprint") != _stock_fingerprint(
        manifest["files"]
    ):
        raise ValueError("stock comparable source fingerprint mismatch")
    project = output.absolute() / "project"
    stock_root = Path(manifest["stock_script"]).parent
    if Path(manifest["project"]) != project:
        raise ValueError("strict source manifest belongs to another overlay")
    expected = {record["relative_path"] for record in manifest["files"]}
    entry = Path(manifest["stock_script"]).name
    prefix, harness, projection = project_driver_harness(
        Path(manifest["stock_script"]).read_bytes(), manifest["stock_script"]
    )
    harness_path = output.absolute() / "harness.py"
    if (
        manifest.get("harness_projection") != projection
        or Path(manifest["harness_script"]) != harness_path
        or harness_path.is_symlink()
        or harness_path.read_bytes() != harness
    ):
        raise ValueError("strict benchmark harness projection changed")
    expected_modules = _selected_modules(
        stock_root, Path(entry), _source_inventory(stock_root)
    )
    if manifest["modules"] != expected_modules:
        raise ValueError("strict module source catalogue changed")
    selected_paths = {relative: name for name, relative in expected_modules.items()}
    if {path.as_posix() for path in _source_inventory(stock_root)} != expected:
        raise ValueError("stock benchmark inventory changed")
    if {path.as_posix() for path in _source_inventory(project)} != expected | {
        "pyproject.toml"
    }:
        raise ValueError("strict benchmark inventory changed")
    upstream_policy = (
        (stock_root / "pyproject.toml").read_bytes()
        if "pyproject.toml" in expected
        else None
    )
    policy, policy_projection = _project_strict_policy(
        upstream_policy, expected_modules
    )
    for record in manifest["files"]:
        relative = Path(record["relative_path"])
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError("strict source manifest path escapes its project")
        if record["module_name"] != selected_paths.get(relative.as_posix()):
            raise ValueError("strict source record module identity changed")
        source = (stock_root / relative).read_bytes()
        candidate = (project / relative).read_bytes()
        if (
            _digest(source) != record["stock_sha256"]
            or _digest(candidate) != record["strict_sha256"]
        ):
            raise ValueError(f"benchmark source changed: {relative}")
        if record["module_name"] is not None:
            expected_source, insertion = strict_opt_in(
                prefix if relative == Path(entry) else source,
                str(stock_root / relative),
            )
            if (
                expected_source != candidate
                or insertion != record["strict_future_line"]
            ):
                raise ValueError(
                    f"strict benchmark has a non-opt-in source change: {relative}"
                )
        elif relative == Path("pyproject.toml"):
            if candidate != policy or record["strict_future_line"] is not None:
                raise ValueError("strict benchmark configuration projection changed")
        elif candidate != source:
            raise ValueError(f"benchmark data changed: {relative}")
    if (
        (project / "pyproject.toml").read_bytes() != policy
        or _digest(policy) != manifest["policy_sha256"]
        or manifest.get("policy_projection") != policy_projection
    ):
        raise ValueError("strict benchmark policy projection changed")
    manifest["overlay_fingerprint"] = fingerprint
    return manifest


def prepare_strict_benchmark(
    script: Path,
    python: Path,
    output: Path,
    checker: Path,
    environment: Mapping[str, str],
    *,
    run: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> dict[str, Any]:
    """Analyze before launching workers, using their actual prepared venv.

    The private signing key, checker output, and startup descriptor live outside
    the immutable analyzed project. A source manifest is only measurement
    provenance; the native startup loader still authenticates every shard.
    """
    output = output.absolute()
    source_directory = output / "source"
    if source_directory.exists():
        source = verify_source_overlay(source_directory)
        if Path(source["stock_script"]) != script.resolve():
            raise ValueError("strict benchmark bundle belongs to another driver")
    else:
        source = prepare_source_overlay(script, source_directory)
    authority = output / "authority"
    authority.mkdir(mode=0o700, exist_ok=True)
    key = authority / "signing.key"
    try:
        descriptor = os.open(key, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        info = key.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077 or info.st_size != 32:
            raise ValueError(
                "benchmark signing key must be a private regular 32-byte file"
            )
    else:
        with os.fdopen(descriptor, "wb") as destination:
            destination.write(os.urandom(32))
    deployment = authority / "deployment.json"
    # Do not resolve the venv symlink: its prefix/site-packages are analysis
    # inputs even when two virtual environments share the native executable.
    selected_python = str(python.absolute())
    command = [
        str(checker.absolute()),
        "check",
        "--project",
        source["project"],
        "--python",
        selected_python,
        "--signing-key",
        str(key),
        "--output",
        str(output / "artifacts"),
        "--deployment",
        str(deployment),
    ]
    for name, relative in source["modules"].items():
        command.extend(["--module", f"{name}={relative}"])
    result = run(
        command,
        cwd=output,
        env=dict(environment),
        text=True,
        capture_output=True,
        timeout=600,
        check=False,
    )
    (output / "checker.stdout.log").write_text(result.stdout)
    (output / "checker.stderr.log").write_text(result.stderr)
    if result.returncode:
        raise RuntimeError(
            f"strict benchmark analysis failed; see {output / 'checker.stderr.log'}"
        )
    publication = json.loads(result.stdout)
    if publication.get("modules") != len(source["modules"]) or not deployment.is_file():
        raise ValueError(
            "offline checker did not publish the complete benchmark module set"
        )
    # Ensure source/config inputs were not changed during the offline run.
    if verify_source_overlay(source_directory) != source:
        raise ValueError("strict source overlay changed during analysis")
    execution = {
        "schema": EXECUTION_SCHEMA,
        "language": "strict",
        "source_directory": str(source_directory),
        "source_fingerprint": source["source_fingerprint"],
        "selection_policy": source["selection_policy"],
        "python_selection": selected_python,
        "deployment": str(deployment),
        "deployment_sha256": _digest(deployment.read_bytes()),
        "publication": publication,
    }
    manifest = output / "execution.json"
    temporary = output / ".execution.json.tmp"
    temporary.write_text(json.dumps(execution, sort_keys=True, indent=2) + "\n")
    temporary.replace(manifest)
    return {**execution, "manifest_path": str(manifest), "source": source}


def verify_strict_benchmark(manifest_path: Path, python: Path) -> dict[str, Any]:
    """Validate immutable measurement inputs; never run analysis in a worker."""
    if manifest_path.is_symlink():
        raise ValueError("strict execution manifest must be a regular file")
    execution = json.loads(manifest_path.read_text())
    if (
        execution.get("schema") != EXECUTION_SCHEMA
        or execution.get("language") != "strict"
    ):
        raise ValueError("benchmark has no strict offline execution manifest")
    if execution.get("python_selection") != str(python.absolute()):
        raise ValueError("benchmark worker is using a different Python environment")
    source = verify_source_overlay(Path(execution["source_directory"]))
    if (
        execution.get("source_fingerprint") != source["source_fingerprint"]
        or execution.get("selection_policy") != source["selection_policy"]
    ):
        raise ValueError("strict benchmark source selection changed after analysis")
    deployment = Path(execution["deployment"])
    if (
        deployment.is_symlink()
        or _digest(deployment.read_bytes()) != execution["deployment_sha256"]
    ):
        raise ValueError("strict benchmark startup descriptor changed after analysis")
    return {**execution, "manifest_path": str(manifest_path), "source": source}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("script", type=Path)
    parser.add_argument("output", type=Path)
    arguments = parser.parse_args()
    print(
        json.dumps(
            prepare_source_overlay(arguments.script, arguments.output), sort_keys=True
        )
    )


if __name__ == "__main__":
    main()
