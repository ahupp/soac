from __future__ import annotations

from pathlib import Path
import sys

import pytest

from tests._integration import (
    exec_integration_validation,
    integration_module,
    split_integration_case,
)
from tests._strict_integration import create_strict_project

MODULES_DIR = Path(__file__).resolve().parent / "simple"

# These five cases use only module initialization and ordinary function calls;
# none relies on post-import mutation of a strict namespace or function. Keep
# selection explicit so adding a stock case does not silently enroll it. Only
# their strict variants gain the required future import; the stock files and
# the rest of each body remain unchanged.
STRICT_CASES = {
    "simple_00_empty_module": (),
    "simple_10_globals": (),
    "simple_20_operators": (),
    "simple_30_conditionals": (),
    "simple_40_functions": ("add", "double"),
}


@pytest.fixture(scope="module")
def strict_simple_project(tmp_path_factory):
    modules = {name: f"{name}.py" for name in STRICT_CASES}
    sources = {
        path: "# soac: module(strict_assign=true, checked_attr=true)\n"
        + split_integration_case(MODULES_DIR / path)[0]
        for path in modules.values()
    }
    return create_strict_project(
        tmp_path_factory.mktemp("strict-simple-cases"), sources, modules=modules
    )


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
    ["stock", "soac", "entry"],
    ids=["stock", "soac", "entry"],
)
def test_simple_integration_case(
    tmp_path: Path, case_path: Path, mode: str, request: pytest.FixtureRequest
) -> None:
    source, validate_source = split_integration_case(case_path)
    module_name = case_path.stem

    if mode != "stock":
        assert module_name in STRICT_CASES, (
            "review strict compatibility before enrolling this case"
        )
        project = request.getfixturevalue("strict_simple_project")
        project.run_case(
            module_name,
            validate_source,
            case_path,
            entry_interpreter=mode == "entry",
            required_functions=STRICT_CASES[module_name],
        )
        return

    sys.path.insert(0, str(MODULES_DIR))
    try:
        with integration_module(tmp_path, module_name, source, mode=mode) as module:
            exec_integration_validation(validate_source, module, case_path, mode=mode)
    finally:
        if str(MODULES_DIR) in sys.path:
            sys.path.remove(str(MODULES_DIR))
