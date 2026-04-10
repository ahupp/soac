from __future__ import annotations

from pathlib import Path
import sys

import pytest

from tests._integration import (
    exec_integration_validation,
    integration_module,
    split_integration_case,
)

MODULES_DIR = Path(__file__).resolve().parent / "integration_modules"


def _case_paths() -> list[Path]:
    cases: list[Path] = []
    for path in sorted(MODULES_DIR.glob("*.py")):
        try:
            if "# diet-python: validate" in path.read_text(encoding="utf-8"):
                cases.append(path)
        except OSError:
            continue
    return cases


@pytest.mark.integration
@pytest.mark.parametrize("case_path", _case_paths(), ids=lambda path: path.stem)
@pytest.mark.parametrize(
    "mode",
    ["stock", "soac"],
    ids=["stock", "soac"],
)
def test_integration_case(tmp_path: Path, case_path: Path, mode: str) -> None:
    if case_path.stem == "yield_from_stack_names" and mode == "soac":
        # BB-lowered generators do not preserve CPython frame-name identity for
        # sys._getframe() observations yet.
        pytest.xfail("BB generator frame-name observability not yet CPython-compatible")
    if case_path.stem == "multiprocessing_barrier_abort_reset":
        # Spawn-mode multiprocessing pickling cannot currently rediscover the
        # helper target function under the generated integration module name.
        pytest.xfail("spawn-mode multiprocessing helper pickling is not yet stable")
    if mode == "soac" and case_path.stem in {
        "exception_refcycle_after_except",
        "exception_refcycle_args_tuple",
        "taskgroup_propagate_cancellation_refcycle",
        "asyncio_taskgroup_base_error_refcycle",
        "iter_refcount_behavior",
        "with_context_exception_leak",
    }:
        # Dict-backed frame locals are currently GC-visible, unlike CPython's
        # fast-locals representation. This changes refcycle/collection behavior
        # for exception-sensitive cases on the SOAC-loaded path.
        pytest.xfail("exception/refcycle behavior differs in SOAC-loaded path")
    if mode == "soac" and case_path.stem in {
        "enum_dynamic_members_vars_update",
        "enum_ignore_dynamic_names",
        "exception_cleanup_name",
        "locals_cell_contents",
        "named_expression_cases",
        "named_expression_locals_unbound",
        "scope_locals",
    }:
        pytest.xfail("scope-aware builtin rewriting has been removed")
    if mode == "soac" and case_path.stem in {
        "builtin_dynamic_global_shadow",
    }:
        pytest.xfail("runtime-builtin loads intentionally skip module-global shadowing")

    source, validate_source = split_integration_case(case_path)
    module_name = case_path.stem

    sys.path.insert(0, str(MODULES_DIR))
    try:
        if case_path.stem == "bad_syntax":
            with pytest.raises(SyntaxError):
                with integration_module(tmp_path, module_name, source, mode=mode):
                    pass
            return
        if case_path.stem == "class_annotations_mutation":
            with pytest.raises(NameError):
                with integration_module(tmp_path, module_name, source, mode=mode):
                    pass
            return
        with integration_module(tmp_path, module_name, source, mode=mode) as module:
            exec_integration_validation(validate_source, module, case_path, mode=mode)
    finally:
        if str(MODULES_DIR) in sys.path:
            sys.path.remove(str(MODULES_DIR))
