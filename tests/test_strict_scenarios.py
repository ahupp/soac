"""Scenario files plus focused format, expectation and runner regressions."""

import ast
from pathlib import Path
from types import ModuleType

import pytest

from tests._strict_scenarios import (
    ScenarioBlock,
    _execute_block,
    parse_strict_scenario,
    run_strict_scenario,
)


SCENARIOS = Path(__file__).with_name("strict_scenarios")


@pytest.mark.integration
@pytest.mark.parametrize("mode", ["soac", "entry", "cpython"])
@pytest.mark.parametrize("path", sorted(SCENARIOS.glob("*.py")), ids=lambda path: path.stem)
def test_strict_scenario(path: Path, mode: str, tmp_path: Path) -> None:
    run_strict_scenario(path, tmp_path / path.stem, mode=mode)


def write_scenario(tmp_path: Path, source: str) -> Path:
    path = tmp_path / "scenario.py"
    path.write_text(source)
    return path


def test_parser_retains_sections_locations_and_real_comment_boundaries(tmp_path: Path) -> None:
    path = write_scenario(tmp_path, '''# Description before the first module.
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
from __future__ import strict
class Example:
    foo: int = 0
# ok
assert function() == [1]
# raise:TypeError
Example().foo = "bad"
''')
    scenario = parse_strict_scenario(path)
    assert tuple(module.name for module in scenario.modules) == ("first", "second")
    assert scenario.modules[0].functions == ("function",)
    first = ast.parse(scenario.modules[0].source)
    assert ast.get_docstring(first).startswith("Module documentation.")
    assert isinstance(first.body[1], ast.ImportFrom)
    assert first.body[1].names[0].name == "annotations"
    assert first.body[2].names[0].name == "strict"
    second = ast.parse(scenario.modules[1].source)
    assert sum(
        isinstance(node, ast.ImportFrom) and any(alias.name == "strict" for alias in node.names)
        for node in second.body
    ) == 1
    assert tuple(block.label for block in scenario.blocks) == ("ok", "raise:TypeError")
    assert scenario.blocks[0].line == 19
    assert ast.parse(scenario.blocks[1].source).body[0].lineno == 22


@pytest.mark.parametrize(("source", "message"), [
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
])
def test_parser_rejects_ambiguous_or_incomplete_files(
    tmp_path: Path, source: str, message: str,
) -> None:
    with pytest.raises(ValueError, match=message):
        parse_strict_scenario(write_scenario(tmp_path, source))


def test_parser_allows_declared_packages_and_empty_modules(tmp_path: Path) -> None:
    scenario = parse_strict_scenario(write_scenario(tmp_path, """# module:package
# module:package.child
value = 1
# ok
import package.child
assert package.child.value == 1
"""))
    assert tuple(module.name for module in scenario.modules) == ("package", "package.child")
    assert ast.parse(scenario.modules[0].source).body[0].names[0].name == "strict"


def test_function_witnesses_do_not_confuse_generators_and_nested_scopes(tmp_path: Path) -> None:
    scenario = parse_strict_scenario(write_scenario(tmp_path, '''# module:example
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
'''))
    assert scenario.modules[0].functions == ("outer",)


def test_last_statement_expectation_preserves_prefix_effects_and_custom_exception() -> None:
    module = ModuleType("ordinary_expectation_unit")
    module.events = []
    _execute_block(module, ScenarioBlock(1, '''from __future__ import annotations
events.append("prefix")
class Expected(ValueError):
    pass
raise Expected("final")
''', "Expected"), Path("expectation.py"), mode="soac")
    assert module.events == ["prefix"]
    assert "Expected" not in vars(module)


@pytest.mark.parametrize("source", [
    'raise TypeError("prefix")\nraise TypeError("final")\n',
    'raise TypeError("prefix"); raise TypeError("final")\n',
    'class A:\n    def __init__(self):\n        raise TypeError("prefix")\na = A()\na.foo = 2\n',
])
def test_matching_exception_from_prefix_is_a_failure(source: str) -> None:
    with pytest.raises(TypeError, match="prefix"):
        _execute_block(
            ModuleType("ordinary_expectation_unit"), ScenarioBlock(1, source, "TypeError"),
            Path("expectation.py"), mode="soac",
        )


@pytest.mark.parametrize(("source", "expected", "error", "message"), [
    ("pass\n", "TypeError", AssertionError, "expected TypeError"),
    ('raise KeyError("wrong")\n', "TypeError", KeyError, "wrong"),
    ('raise ValueError("last")\n', "missing_error", ValueError, "not an exception type"),
    ('raise TypeError("unexpected")\n', None, TypeError, "unexpected"),
    ("invalid syntax here\n", "SyntaxError", SyntaxError, "invalid syntax"),
])
def test_expectation_rejects_missing_wrong_and_setup_errors(
    source: str, expected: str | None, error: type[Exception], message: str,
) -> None:
    with pytest.raises(error, match=message):
        _execute_block(
            ModuleType("ordinary_expectation_unit"), ScenarioBlock(1, source, expected),
            Path("expectation.py"), mode="soac",
        )


def test_qualified_exception_accepts_subclasses() -> None:
    _execute_block(
        ModuleType("ordinary_expectation_unit"),
        ScenarioBlock(1, 'raise KeyError("last")\n', "builtins.LookupError"),
        Path("expectation.py"), mode="cpython",
    )


@pytest.mark.integration
def test_runner_records_failures_and_still_runs_later_blocks(tmp_path: Path) -> None:
    path = write_scenario(tmp_path, '''# module:example
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
''')
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
    tmp_path: Path, failing_module: str,
) -> None:
    path = write_scenario(tmp_path, (
        "# module:primary\n"
        + ('raise ValueError("module setup failed")\n' if failing_module == "primary" else "value = 1\n")
        + "# module:dependency\n"
        + ('raise ValueError("module setup failed")\n' if failing_module == "dependency" else "value = 2\n")
        + '# raise:ValueError\nraise ValueError("expected terminal error")\n'
    ))
    with pytest.raises(AssertionError, match="module setup failed"):
        run_strict_scenario(path, tmp_path / "run", mode="cpython")
    assert not tuple((tmp_path / "run").glob("block-*.complete"))


@pytest.mark.integration
def test_checker_rejection_cannot_satisfy_expected_exception(tmp_path: Path) -> None:
    path = write_scenario(tmp_path, '''# module:example
def invalid() -> int:
    return "not an int"
# raise:AssertionError
raise AssertionError("terminal")
''')
    root = tmp_path / "run"
    with pytest.raises(AssertionError, match="actual checker rejected fixture"):
        run_strict_scenario(path, root, mode="cpython")
    assert not (root / "authority/deployment.json").exists()
    assert not tuple(root.glob("runtime-*"))
