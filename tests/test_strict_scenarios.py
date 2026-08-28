"""Scenario files plus focused format, expectation and runner regressions."""

import ast
import builtins
import sys
from pathlib import Path
from types import FunctionType, ModuleType

import pytest

from tests._strict_scenarios import (
    ScenarioBlock,
    _check_modules,
    _execute_block,
    _surviving_function_witnesses,
    parse_strict_scenario,
    run_strict_scenario,
)


SCENARIOS = Path(__file__).with_name("strict_scenarios")


@pytest.mark.integration
@pytest.mark.parametrize("mode", ["soac", "entry", "cpython"])
@pytest.mark.parametrize(
    "path", sorted(SCENARIOS.glob("*.py")), ids=lambda path: path.stem
)
def test_strict_scenario(path: Path, mode: str, tmp_path: Path) -> None:
    project = run_strict_scenario(path, tmp_path / path.stem, mode=mode)
    for module in parse_strict_scenario(path).modules:
        assert (
            project.project / project.modules[module.name]
        ).read_bytes() == module.source.encode("utf-8")
    assert not (project.project / "pyproject.toml").exists()


def write_scenario(tmp_path: Path, source: str) -> Path:
    path = tmp_path / "scenario.py"
    path.write_text(source)
    return path


def test_parser_retains_sections_locations_and_real_comment_boundaries(
    tmp_path: Path,
) -> None:
    path = write_scenario(
        tmp_path,
        '''# Description before the first module.
# module:first
"""Module documentation.
# ok
# module:not_a_section
"""
from __future__ import annotations
value = [
# raise:NotASection
    1,
]
def function():
    # ok
    return value
# module:second
# soac: module(checked_attr=true)
class Example:
    foo: int = 0
# ok
assert function() == [1]
# raise:TypeError
Example().foo = "bad"
''',
    )
    scenario = parse_strict_scenario(path)
    assert tuple(module.name for module in scenario.modules) == ("first", "second")
    assert scenario.modules[0].functions == (("function", 10),)
    first = ast.parse(scenario.modules[0].source)
    assert ast.get_docstring(first).startswith("Module documentation.")
    assert isinstance(first.body[1], ast.ImportFrom)
    assert first.body[1].names[0].name == "annotations"
    assert isinstance(first.body[2], ast.Assign)
    second = ast.parse(scenario.modules[1].source)
    assert (
        sum(
            isinstance(node, ast.ImportFrom)
            and any(alias.name == "strict" for alias in node.names)
            for node in second.body
        )
        == 0
    )
    assert "# soac: module(checked_attr=true)" in scenario.modules[1].source
    assert tuple(block.label for block in scenario.blocks) == ("ok", "raise:TypeError")
    assert scenario.blocks[0].line == 19
    assert ast.parse(scenario.blocks[1].source).body[0].lineno == 22


@pytest.mark.parametrize("newline", ["\n", "\r\n", "\r"], ids=["lf", "crlf", "cr"])
def test_parser_preserves_exact_bytes_and_physical_line_boundaries(
    tmp_path: Path,
    newline: str,
) -> None:
    # These are literal string contents, not Python physical line separators.
    payload = "a\x85b\u2028c\u2029d\fe"
    body = newline.join([f'value = "{payload}"', "def read():", "    return value", ""])
    source = (
        f"# module:example{newline}{body}# ok{newline}assert read() == value{newline}"
    )
    path = tmp_path / "scenario.py"
    path.write_bytes(source.encode("utf-8"))
    scenario = parse_strict_scenario(path)
    assert scenario.modules[0].source.encode("utf-8") == body.encode("utf-8")
    assert scenario.modules[0].functions == (("read", 2),)
    assert scenario.blocks[0].line == 5
    namespace = {}
    exec(
        compile(scenario.modules[0].source, str(path), "exec", dont_inherit=True),
        namespace,
    )
    assert namespace["value"] == payload


@pytest.mark.parametrize(
    ("source", "message"),
    [
        ("pass\n", "start with"),
        ("# ok\npass\n", "start with"),
        ("x = 1\n# module:a\n# ok\npass\n", "code before"),
        ("# module:a\n# module:a\n# ok\npass\n", "duplicate module"),
        ("# module:a\n# ok\npass\n# module:b\n", "must precede"),
        ("# module:a\n", "at least one"),
        ("# module:a\n# ok\n# comment\n", "empty test block"),
        ("# module:../escape\n# ok\npass\n", "invalid module name"),
        ("# module:class\n# ok\npass\n", "invalid module name"),
        ("# module:a.child\n# ok\npass\n", "declared parent"),
        ("# module:a.__init__\n# ok\npass\n", "name a package"),
        ("# module:a\n# raise:\npass\n", "malformed"),
        ("# module:a\n# raise:ValueError()\npass\n", "invalid raise name"),
        ("# module:a\n# ok:yes\npass\n", "malformed"),
    ],
)
def test_parser_rejects_ambiguous_or_incomplete_files(
    tmp_path: Path,
    source: str,
    message: str,
) -> None:
    with pytest.raises(ValueError, match=message):
        parse_strict_scenario(write_scenario(tmp_path, source))


def test_parser_allows_declared_packages_and_empty_modules(tmp_path: Path) -> None:
    scenario = parse_strict_scenario(
        write_scenario(
            tmp_path,
            """# module:package
# module:package.child
value = 1
# ok
import package.child
assert package.child.value == 1
""",
        )
    )
    assert tuple(module.name for module in scenario.modules) == (
        "package",
        "package.child",
    )
    assert ast.parse(scenario.modules[0].source).body == []


def test_function_witnesses_do_not_confuse_generators_and_nested_scopes(
    tmp_path: Path,
) -> None:
    scenario = parse_strict_scenario(
        write_scenario(
            tmp_path,
            """# module:example
def generator():
    yield 1
def delegating_generator():
    yield from generator()
def outer():
    def nested():
        yield 1
    return nested()
def header_generator():
    def nested(value=(yield 1)):
        return value
    return nested
async def coroutine():
    return 1
# ok
assert list(outer()) == [1]
""",
        )
    )
    assert scenario.modules[0].functions == (("outer", 5),)


def test_function_witnesses_follow_surviving_definitions_and_aliases(
    tmp_path: Path,
) -> None:
    scenario = parse_strict_scenario(
        write_scenario(
            tmp_path,
            """# module:example
def removed():
    return 1
del removed
def rebound():
    return 2
rebound = foreign
def retained():
    return 3
alias = retained
second_alias = retained
del retained
def survivor():
    return 4
# ok
pass
""",
        )
    )
    module = ModuleType("example")
    module.foreign = lambda: "ordinary imported function"
    source_path = str(tmp_path / "example.py")
    exec(
        compile(scenario.modules[0].source, source_path, "exec", dont_inherit=True),
        vars(module),
    )
    assert module.rebound is module.foreign
    assert _surviving_function_witnesses(
        module,
        source_path,
        scenario.modules[0].functions,
    ) == (module.alias, module.survivor)
    # Matching coordinates do not make foreign globals a local definition.
    module.foreign_copy = FunctionType(module.survivor.__code__, {})
    assert _surviving_function_witnesses(
        module,
        source_path,
        scenario.modules[0].functions,
    ) == (module.alias, module.survivor)


@pytest.mark.parametrize("strict_assign", [False, True])
def test_surviving_local_function_cannot_skip_its_native_owner_witness(
    tmp_path: Path,
    monkeypatch,
    strict_assign: bool,
) -> None:
    module = ModuleType("scenario_unowned_function_probe")
    source_path = str(tmp_path / "probe.py")
    exec(
        compile("def local(): return 1\n", source_path, "exec", dont_inherit=True),
        vars(module),
    )
    monkeypatch.setitem(sys.modules, module.__name__, module)
    # Unit-test the function-check dispatch, without inventing native authority.
    # The real native function witness must still reject this ordinary function.
    monkeypatch.setattr(
        "tests._strict_scenarios._assert_cpython_module_witness",
        lambda *args, **kwargs: {},
    )
    with pytest.raises(
        AssertionError, match="function has no matching native source/body owner"
    ):
        _check_modules(
            ((module.__name__, source_path, "unused", (("local", 1),)),),
            "unused",
            "cpython",
            {module.__name__: {"strict_assign": strict_assign}},
        )


def test_last_statement_expectation_preserves_prefix_effects_and_custom_exception() -> (
    None
):
    module = ModuleType("ordinary_expectation_unit")
    module.events = []
    _execute_block(
        module,
        ScenarioBlock(
            1,
            """from __future__ import annotations
events.append("prefix")
class Expected(ValueError):
    pass
raise Expected("final")
""",
            "Expected",
        ),
        Path("expectation.py"),
        mode="soac",
    )
    assert module.events == ["prefix"]
    assert "Expected" not in vars(module)


@pytest.mark.parametrize(
    "in_prefix", [False, True], ids=["primary-module", "block-prefix"]
)
def test_exception_name_uses_its_namespace_binding_before_builtins(
    in_prefix: bool,
) -> None:
    module = ModuleType("ordinary_expectation_unit")

    class LocalValueError(Exception):
        pass

    module.LocalValueError = LocalValueError
    prefix = "ValueError = LocalValueError\n" if in_prefix else ""
    if not in_prefix:
        module.ValueError = LocalValueError
    _execute_block(
        module,
        ScenarioBlock(1, prefix + 'raise ValueError("final")\n', "ValueError"),
        Path("expectation.py"),
        mode="soac",
    )
    with pytest.raises(builtins.ValueError, match="not the selected exception"):
        _execute_block(
            module,
            ScenarioBlock(
                1,
                prefix
                + 'import builtins\nraise builtins.ValueError("not the selected exception")\n',
                "ValueError",
            ),
            Path("expectation.py"),
            mode="soac",
        )


def test_final_statement_expectation_preserves_hoisted_annotation_setup() -> None:
    module = ModuleType("ordinary_expectation_unit")
    module.events = []
    _execute_block(
        module,
        ScenarioBlock(
            1,
            """from __future__ import annotations
events.append("__annotations__" in globals())
assert "__annotations__" in globals()
value: int = missing_value
""",
            "NameError",
        ),
        Path("expectation.py"),
        mode="soac",
    )
    assert module.events == [True]


def test_final_statement_helper_does_not_replace_source_or_primary_names() -> None:
    module = ModuleType("ordinary_expectation_unit")
    module._soac_scenario_expectation = object()
    module.events = []
    _execute_block(
        module,
        ScenarioBlock(
            1,
            """
assert _soac_scenario_expectation is module._soac_scenario_expectation
_soac_scenario_expectation_ = "source binding"
events.append(_soac_scenario_expectation_)
raise LookupError("final")
""",
            "LookupError",
        ),
        Path("expectation.py"),
        mode="soac",
    )
    assert module.events == ["source binding"]


@pytest.mark.parametrize(
    "source",
    [
        "from __future__ import annotations\n",
        '"module documentation"\nfrom __future__ import annotations\n',
        "from __future__ import annotations\nfrom __future__ import division\n",
    ],
)
def test_final_future_import_remains_legal_outside_the_wrapper(source: str) -> None:
    with pytest.raises(
        AssertionError, match="expected ValueError, but block completed"
    ):
        _execute_block(
            ModuleType("ordinary_expectation_unit"),
            ScenarioBlock(1, source, "ValueError"),
            Path("expectation.py"),
            mode="soac",
        )


@pytest.mark.parametrize(
    "source",
    [
        'raise TypeError("prefix")\nraise TypeError("final")\n',
        'raise TypeError("prefix"); raise TypeError("final")\n',
        'class A:\n    def __init__(self):\n        raise TypeError("prefix")\na = A()\na.foo = 2\n',
    ],
)
def test_matching_exception_from_prefix_is_a_failure(source: str) -> None:
    with pytest.raises(TypeError, match="prefix"):
        _execute_block(
            ModuleType("ordinary_expectation_unit"),
            ScenarioBlock(1, source, "TypeError"),
            Path("expectation.py"),
            mode="soac",
        )


@pytest.mark.parametrize(
    ("source", "expected", "error", "message"),
    [
        ("pass\n", "TypeError", AssertionError, "expected TypeError"),
        ('raise KeyError("wrong")\n', "TypeError", KeyError, "wrong"),
        (
            'raise ValueError("last")\n',
            "missing_error",
            ValueError,
            "not an exception type",
        ),
        ('raise TypeError("unexpected")\n', None, TypeError, "unexpected"),
        ("invalid syntax here\n", "SyntaxError", SyntaxError, "invalid syntax"),
    ],
)
def test_expectation_rejects_missing_wrong_and_setup_errors(
    source: str,
    expected: str | None,
    error: type[Exception],
    message: str,
) -> None:
    with pytest.raises(error, match=message):
        _execute_block(
            ModuleType("ordinary_expectation_unit"),
            ScenarioBlock(1, source, expected),
            Path("expectation.py"),
            mode="soac",
        )


def test_qualified_exception_accepts_subclasses() -> None:
    _execute_block(
        ModuleType("ordinary_expectation_unit"),
        ScenarioBlock(1, 'raise KeyError("last")\n', "builtins.LookupError"),
        Path("expectation.py"),
        mode="cpython",
    )


@pytest.mark.integration
def test_runner_records_failures_and_still_runs_later_blocks(tmp_path: Path) -> None:
    path = write_scenario(
        tmp_path,
        """# module:example
# soac: module(strict_assign=true, checked_attr=true)
events: list[int] = []
class A:
    foo: int = 0
# ok
events.append(1)
raise RuntimeError("unexpected block failure")
# ok
assert events == []
# raise:ValueError
A().foo = "wrong exception"
# raise:TypeError
A().foo = 1
# ok
import os
os._exit(0)
# raise:TypeError
raise TypeError("prefix must fail")
A().foo = "bad"
""",
    )
    root = tmp_path / "run"
    with pytest.raises(AssertionError) as captured:
        run_strict_scenario(path, root, mode="cpython")
    error = str(captured.value)
    assert "unexpected block failure" in error
    assert "[raise:ValueError]" in error
    assert "expected TypeError, but block completed" in error
    assert "without completing the block" in error
    assert "prefix must fail" in error
    assert len(tuple(root.glob("runtime-*/driver.py"))) == 6
    assert {file.name for file in root.glob("block-*.complete")} == {"block-2.complete"}
    assert (root / "authority/deployment.json").is_file()


@pytest.mark.integration
@pytest.mark.parametrize("failing_module", ["primary", "dependency"])
def test_import_failure_cannot_satisfy_expected_exception(
    tmp_path: Path,
    failing_module: str,
) -> None:
    path = write_scenario(
        tmp_path,
        (
            "# module:primary\n"
            + (
                'raise ValueError("module setup failed")\n'
                if failing_module == "primary"
                else "value = 1\n"
            )
            + "# module:dependency\n"
            + (
                'raise ValueError("module setup failed")\n'
                if failing_module == "dependency"
                else "value = 2\n"
            )
            + '# raise:ValueError\nraise ValueError("expected terminal error")\n'
        ),
    )
    with pytest.raises(AssertionError, match="module setup failed"):
        run_strict_scenario(path, tmp_path / "run", mode="cpython")
    assert not tuple((tmp_path / "run").glob("block-*.complete"))


@pytest.mark.integration
def test_checker_rejection_cannot_satisfy_expected_exception(tmp_path: Path) -> None:
    path = write_scenario(
        tmp_path,
        """# module:example
# soac: module(strict_assign=true)
def invalid() -> int:
    return "not an int"
# raise:AssertionError
raise AssertionError("terminal")
""",
    )
    root = tmp_path / "run"
    with pytest.raises(AssertionError, match="actual checker rejected fixture"):
        run_strict_scenario(path, root, mode="cpython")
    assert not (root / "authority/deployment.json").exists()
    assert not tuple(root.glob("runtime-*"))
