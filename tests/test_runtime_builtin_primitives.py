import json
import textwrap
from pathlib import Path

import pytest

from tests._integration import stock_module
from tests._strict_integration import (
    assert_strict_source_rejected,
    create_strict_project,
)

_REVIEWED_BUILTIN_CASES = json.loads(
    (Path(__file__).parent / "fixtures/strict_builtin_primitive_cases.json").read_text()
)


def _runtime_range_case():
    """Exercise bad dynamic inputs without hiding statically invalid calls."""
    original = _REVIEWED_BUILTIN_CASES["runtime_range_is_reusable_iterable"]
    source = original["source"]
    assert source.count("def no_args():\n    return range()") == 1
    assert source.count("def no_index():\n    return list(range(object()))") == 1
    source = source.replace(
        "def no_args():\n    return range()",
        "def no_args(*args):\n    return range(*args)",
    ).replace(
        "def no_index():\n    return list(range(object()))",
        "def no_index(value):\n    return list(range(value))",
    )
    return {
        **original,
        "source": source,
        "validation": original["validation"].replace(
            "module.no_index()", "module.no_index(object())"
        ),
    }


def test_original_range_bad_calls_are_strict_checker_errors(tmp_path):
    case = _REVIEWED_BUILTIN_CASES["runtime_range_is_reusable_iterable"]
    errors = assert_strict_source_rejected(
        tmp_path / "original-invalid-range-calls",
        "from __future__ import strict\n" + case["source"],
        module_name="builtin_model",
        diagnostic="CheckerError: no-matching-overload",
    )
    assert "CheckerError: invalid-argument-type" in errors


@pytest.mark.parametrize("name", _REVIEWED_BUILTIN_CASES)
def test_reviewed_builtin_primitives_keep_ordinary_behavior(tmp_path, name):
    from soac import _soac_ext

    case = _REVIEWED_BUILTIN_CASES[name]
    with stock_module(tmp_path, name, case["source"]) as module:
        assert _soac_ext.strict_module_diagnostics(module) is None
        exec(  # noqa: S102 - retained ordinary validator, not analyzed source
            compile(case["validation"], __file__, "exec", dont_inherit=True),
            {"module": module, "pytest": pytest},
        )


@pytest.fixture(scope="module", params=_REVIEWED_BUILTIN_CASES)
def strict_reviewed_builtin_project(tmp_path_factory, request):
    name = request.param
    case = (
        _runtime_range_case()
        if name == "runtime_range_is_reusable_iterable"
        else _REVIEWED_BUILTIN_CASES[name]
    )
    project = create_strict_project(
        tmp_path_factory.mktemp("strict-builtins-" + name),
        {"builtin_model.py": "from __future__ import strict\n" + case["source"]},
        modules={"builtin_model": "builtin_model.py"},
    )
    return case, project


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_reviewed_builtin_primitives_use_authenticated_entries(
    strict_reviewed_builtin_project, entry_interpreter
):
    case, project = strict_reviewed_builtin_project
    project.run_case(
        "builtin_model",
        "import pytest\ndef validate_module(module):\n"
        + textwrap.indent(case["validation"], "    "),
        Path(__file__),
        required_functions=tuple(case["required_functions"]),
        entry_interpreter=entry_interpreter,
    )
