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
    optimization_decisions_v3 = decide_optimizations(profile_dump, counters_dir, mode="v3")
    optimization_plan_v3 = print_optimization_plans_v3(counters_dir)
    post_opt_v3 = print_post_opt_v3_definition(counters_dir)
    specializations = inspect_specializations(profile_dump)
    pre_inline_clif = render_specialized_clif(
        module_name=module_name,
        source_path=source_path,
        function_id=function_id,
        counters_dir=counters_dir,
        pre_inline=True,
    )
    vcode_path = artifact_dir / "specialized.vcode"
    clif = render_specialized_clif(
        module_name=module_name,
        source_path=source_path,
        function_id=function_id,
        counters_dir=counters_dir,
        pre_inline=False,
        vcode_out=vcode_path,
    )
    vcode = vcode_path.read_text(encoding="utf-8")
    instr_typed = render_specialized_instr_typed(
        module_name=module_name,
        source_path=source_path,
        function_id=function_id,
        counters_dir=counters_dir,
    )

    specializations_path = artifact_dir / "specializations.txt"
    optimization_decisions_v3_path = artifact_dir / "optimization_decisions_v3.txt"
    optimization_plan_v3_path = artifact_dir / "optimization_plan_v3.txt"
    post_opt_v3_path = artifact_dir / "post_opt_v3.blockpy.txt"
    pre_inline_clif_path = artifact_dir / "pre_inline.clif"
    clif_path = artifact_dir / "specialized.clif"
    instr_typed_path = artifact_dir / "instr_typed.txt"
    context_path = artifact_dir / "annotation_context.md"
    metadata_path = artifact_dir / "metadata.json"

    specializations_path.write_text(specializations, encoding="utf-8")
    optimization_decisions_v3_path.write_text(
        optimization_decisions_v3, encoding="utf-8"
    )
    optimization_plan_v3_path.write_text(optimization_plan_v3, encoding="utf-8")
    post_opt_v3_path.write_text(post_opt_v3, encoding="utf-8")
    pre_inline_clif_path.write_text(pre_inline_clif, encoding="utf-8")
    clif_path.write_text(clif, encoding="utf-8")
    instr_typed_path.write_text(instr_typed, encoding="utf-8")
    metadata = {
        "artifact_dir": str(artifact_dir),
        "preferred_view": args.view,
        "module_name": module_name,
        "function_name": target_name,
        "function_id": function_id,
        "source_path": str(source_path),
        "workload": args.workload,
        "result_repr": result_repr,
        "profile_dump": str(profile_dump),
        "specializations_path": str(specializations_path),
        "optimization_decisions_v3_path": str(optimization_decisions_v3_path),
        "optimization_plan_v3_path": str(optimization_plan_v3_path),
        "post_opt_v3_path": str(post_opt_v3_path),
        "pre_inline_clif_path": str(pre_inline_clif_path),
        "clif_path": str(clif_path),
        "vcode_path": str(vcode_path),
        "instr_typed_path": str(instr_typed_path),
        "annotation_context_path": str(context_path),
    }
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    context_path.write_text(
        annotation_context(
            metadata=metadata,
            source=source_path.read_text(encoding="utf-8"),
            optimization_decisions_v3=optimization_decisions_v3,
            optimization_plan_v3=optimization_plan_v3,
            post_opt_v3=post_opt_v3,
            specializations=specializations,
            instr_typed=instr_typed,
            pre_inline_clif=pre_inline_clif,
            clif=clif,
            vcode=vcode,
        ),
        encoding="utf-8",
    )

    print(f"artifact_dir: {artifact_dir}")
    print(f"annotation_context: {context_path}")
    print(f"optimization_decisions_v3: {optimization_decisions_v3_path}")
    print(f"optimization_plan_v3: {optimization_plan_v3_path}")
    print(f"post_opt_v3: {post_opt_v3_path}")
    print(f"pre_inline_clif: {pre_inline_clif_path}")
    print(f"specialized_clif: {clif_path}")
    print(f"specialized_vcode: {vcode_path}")
    print(f"instr_typed: {instr_typed_path}")
    print(f"specializations: {specializations_path}")
    print(f"metadata: {metadata_path}")
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Profile a Python snippet workload and collect annotated SOAC views."
        )
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
        help="Directory for generated artifacts. Defaults to work/logs/soac-annotations/<name>-<id>.",
    )
    parser.add_argument(
        "--view",
        choices=("post-opt", "clif", "vcode"),
        default="post-opt",
        help="Preferred view to annotate in the answer. Defaults to post-opt.",
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
        / "work"
        / "logs"
        / "soac-annotations"
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
    except RuntimeError as err:
        return f"inspect_counters failed\n{err}\n"


def decide_optimizations(profile_dump: Path, counters_dir: Path, *, mode: str) -> str:
    module_root = counters_dir / "modules"
    return run_inspector(
        "decide_optimizations",
        "--counters",
        str(profile_dump),
        "--mode",
        mode,
        "--module-root",
        str(module_root),
        "--out",
        str(module_root),
    ).stdout


def print_optimization_plans_v3(counters_dir: Path) -> str:
    module_root = counters_dir / "modules"
    plan_paths = sorted(module_root.glob("**/mod.optv3"))
    if not plan_paths:
        return f"no optimizer v3 plans found under {module_root}\n"

    rendered = []
    for plan_path in plan_paths:
        plan_text = run_inspector(
            "print_optimization_plan_v3",
            "--plan",
            str(plan_path),
            "--details",
        ).stdout
        rendered.append(f"# {plan_path}\n{plan_text.rstrip()}\n")
    return "\n".join(rendered)


def print_post_opt_v3_definition(counters_dir: Path) -> str:
    module_root = counters_dir / "modules"
    module_paths = sorted(module_root.glob("**/mod.optv3.blockpy"))
    if not module_paths:
        return f"no optimizer v3 BlockPy modules found under {module_root}\n"

    rendered = []
    for module_path in module_paths:
        module_text = run_inspector(
            "print_codegen_module_cache",
            str(module_path),
        ).stdout
        rendered.append(f"# {module_path}\n{module_text.rstrip()}\n")
    return "\n".join(rendered)


def render_specialized_clif(
    *,
    module_name: str,
    source_path: Path,
    function_id: str,
    counters_dir: Path,
    pre_inline: bool,
    vcode_out: Path | None = None,
) -> str:
    env = {
        **os.environ,
        "SOAC_WORK_DIR": str(counters_dir),
        "SOAC_OPT_MODE": "apply",
    }
    args = [
        "render_jit_clif",
        "--specialized",
        "--module-name",
        module_name,
        str(source_path),
        function_id,
    ]
    if pre_inline:
        args.insert(2, "--pre-inline")
    if vcode_out is not None:
        args[1:1] = ["--vcode-out", str(vcode_out)]
    return run_inspector(*args, env=env).stdout


def render_specialized_instr_typed(
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
        "render_instr_typed",
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
    package = "soac_opt" if bin_name == "decide_optimizations" else "soac_inspector"
    command = [
        "cargo",
        "run",
        "-q",
        "-p",
        package,
        "--bin",
        bin_name,
        "--",
        *args,
    ]
    result = subprocess.run(
        command,
        cwd=import_hook.REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(
            "inspector command failed\n"
            f"command: {command!r}\n"
            f"exit_code: {result.returncode}\n"
            f"stdout:\n{result.stdout}\n"
            f"stderr:\n{result.stderr}"
        )
    return result


def annotation_context(
    *,
    metadata: dict[str, Any],
    source: str,
    optimization_decisions_v3: str,
    optimization_plan_v3: str,
    post_opt_v3: str,
    specializations: str,
    instr_typed: str,
    pre_inline_clif: str,
    clif: str,
    vcode: str,
) -> str:
    return f"""# SOAC Annotation Context

## Instructions

Produce an annotated view for `preferred_view` from the metadata. Default to the
post-opt-v3 BlockPy definition unless the user asked for CLIF or VCode. Preserve
the original order and add concise comments beside important instructions or
blocks. Explain guards, fast paths, slow paths, exception edges, helper calls,
refcount cleanup, direct indexed access, and lowered v3 optimization shapes. Use
the source, plan, and counter context as evidence; mark uncertain block purposes
as inferred.

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

## Optimizer v3 Decision CLI Output

```text
{optimization_decisions_v3.rstrip()}
```

## Optimizer v3 Plan

```text
{optimization_plan_v3.rstrip()}
```

## Post-Opt-V3 BlockPy Definition

```text
{post_opt_v3.rstrip()}
```

## InstrTyped Input To Codegen

```text
{instr_typed.rstrip()}
```

## Pre-Inlining Specialized CLIF

```clif
{pre_inline_clif.rstrip()}
```

## Specialized CLIF

```clif
{clif.rstrip()}
```

## Specialized VCode

```text
{vcode.rstrip()}
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
