"""Scenario files plus focused format, expectation and runner regressions."""

import ast
import builtins
import json
import subprocess
import sys
from pathlib import Path
from types import FunctionType, ModuleType, SimpleNamespace

import pytest

from tests._strict_scenarios import (
    ScenarioBlock,
    _check_modules,
    _execute_block,
    _surviving_function_witnesses,
    discover_strict_scenarios,
    parse_strict_scenario,
    run_strict_scenario,
    scenario_pytest_id,
)


SCENARIOS = Path(__file__).with_name("strict_scenarios").resolve()


@pytest.mark.integration
@pytest.mark.parametrize(
    ("path", "mode"),
    [
        pytest.param(
            scenario.path,
            mode,
            id=scenario_pytest_id(scenario, mode, SCENARIOS),
        )
        for scenario in discover_strict_scenarios(SCENARIOS)
        for mode in scenario.modes
    ],
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


def test_discovery_enrolls_nested_files_and_distinguishes_equal_basenames(
    tmp_path: Path,
) -> None:
    sources = {
        "fields/basic.py": "# modes:soac,entry\n",
        "modules/nested/basic.py": "# modes:cpython\n",
        "ordinary.py": "",
    }
    for relative, header in sources.items():
        path = tmp_path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(header + "# module:example\n# ok\npass\n# ok\npass\n")
    scenarios = discover_strict_scenarios(tmp_path)
    assert [item.path.relative_to(tmp_path).as_posix() for item in scenarios] == list(sources)
    assert [item.modes for item in scenarios] == [
        ("soac", "entry"), ("cpython",), ("soac", "entry", "cpython"),
    ]
    assert all(len(item.blocks) == 2 for item in scenarios)
    assert {
        (item.path.relative_to(tmp_path).as_posix(), mode)
        for item in scenarios for mode in item.modes
    } == {
        ("fields/basic.py", "soac"),
        ("fields/basic.py", "entry"),
        ("modules/nested/basic.py", "cpython"),
        ("ordinary.py", "soac"),
        ("ordinary.py", "entry"),
        ("ordinary.py", "cpython"),
    }


def test_discovery_rejects_empty_or_missing_trees(tmp_path: Path) -> None:
    for root in (tmp_path, tmp_path / "missing"):
        with pytest.raises(ValueError, match="no strict scenario files"):
            discover_strict_scenarios(root)


def test_runner_rejects_unenrolled_mode_before_publication(tmp_path: Path) -> None:
    path = write_scenario(tmp_path, "# modes:cpython\n# module:example\n# ok\npass\n")
    root = tmp_path / "run"
    with pytest.raises(ValueError, match="not enrolled for mode 'soac'"):
        run_strict_scenario(path, root, mode="soac")
    assert not root.exists()


def test_real_pytest_collection_enrolls_the_recursive_catalog_once(tmp_path: Path) -> None:
    """Exercise the actual dispatcher/config, not only the discovery helper."""
    root = Path(__file__).resolve().parents[1]
    tree = tmp_path / "strict_scenarios"
    for relative, header in {
        "fields/test_same.py": "# modes:soac,entry\n",
        "modules/nested/test_same.py": "# modes:cpython\n",
        "ordinary.py": "",
    }.items():
        path = tree / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(
            header + '# module:example\nraise AssertionError("scenario source was imported")\n'
            '# ok\npass\n# ok\npass\n'
        )
    (tmp_path / "test_dispatch.py").write_bytes(Path(__file__).read_bytes())
    journal = tmp_path / "collection.json"
    (tmp_path / "conftest.py").write_text(
        "import json\nimport sys\nfrom pathlib import Path\n"
        + f"sys.path.insert(0, {str(root)!r})\n"
        + "def pytest_collection_finish(session):\n"
        + "    rows = [{'id': item.nodeid, 'path': str(item.callspec.params['path']), "
        + "'mode': item.callspec.params['mode']} for item in session.items]\n"
        + f"    Path({str(journal)!r}).write_text(json.dumps(rows))\n"
    )
    result = subprocess.run(
        [sys.executable, "-I", "-B", "-m", "pytest", "-c", str(root / "pytest.ini"),
         str(tmp_path), "--collect-only", "-q", "-k", "test_strict_scenario"],
        cwd=tmp_path, capture_output=True, text=True, timeout=30,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    rows = json.loads(journal.read_text())
    assert len(rows) == len({row["id"] for row in rows}) == 6
    assert {(Path(row["path"]).relative_to(tree).as_posix(), row["mode"]) for row in rows} == {
        ("fields/test_same.py", "soac"), ("fields/test_same.py", "entry"),
        ("modules/nested/test_same.py", "cpython"),
        ("ordinary.py", "soac"), ("ordinary.py", "entry"), ("ordinary.py", "cpython"),
    }


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
        ("# modes:\n# module:a\n# ok\npass\n", "malformed modes"),
        ("# modes:python\n# module:a\n# ok\npass\n", "distinct names"),
        ("# modes:soac,soac\n# module:a\n# ok\npass\n", "distinct names"),
        ("# modes:soac,\n# module:a\n# ok\npass\n", "distinct names"),
        ("# modes:soac\n# modes:entry\n# module:a\n# ok\npass\n", "once before"),
        ("# module:a\n# modes:soac\n# ok\npass\n", "once before"),
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


def test_prose_comments_are_not_malformed_directives(tmp_path: Path) -> None:
    body = (
        "# module opt-in is separate from fixture enrollment.\n"
        "# modes may differ for native controls.\n"
        "# raise only on a protected write.\n"
        "value = 1\n"
    )
    scenario = parse_strict_scenario(write_scenario(
        tmp_path,
        "# module:example\n" + body + "# ok\n# ok means normal completion.\nassert value == 1\n",
    ))
    assert scenario.modules[0].source == body
    assert len(scenario.blocks) == 1


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
@pytest.mark.parametrize("first_failure", ["runtime", "timeout"])
def test_runner_records_failures_and_still_runs_later_blocks(
    tmp_path: Path, monkeypatch, first_failure: str,
) -> None:
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
    from tests._strict_integration import STRICT_RUNTIME_TIMEOUT

    root = tmp_path / "run"
    original_run = subprocess.run
    runtime_deadlines = []

    def run(command, *args, **kwargs):
        if str(command[-1]).endswith("/driver.py"):
            runtime_deadlines.append(kwargs["timeout"])
            if first_failure == "timeout" and len(runtime_deadlines) == 1:
                raise subprocess.TimeoutExpired(command, kwargs["timeout"])
        return original_run(command, *args, **kwargs)

    monkeypatch.setattr(subprocess, "run", run)
    with pytest.raises(AssertionError) as captured:
        run_strict_scenario(path, root, mode="cpython")
    error = str(captured.value)
    if first_failure == "timeout":
        assert f"timed out after {STRICT_RUNTIME_TIMEOUT} seconds" in error
    else:
        assert "unexpected block failure" in error
    assert runtime_deadlines == [STRICT_RUNTIME_TIMEOUT] * 6
    assert "[raise:ValueError]" in error
    assert "expected TypeError, but block completed" in error
    assert "without completing the block" in error
    assert "prefix must fail" in error
    assert len(tuple(root.glob("runtime-*/driver.py"))) == 6
    assert {file.name for file in root.glob("block-*.complete")} == {"block-2.complete"}
    assert (root / "authority/deployment.json").is_file()


@pytest.mark.integration
def test_cached_ordinary_module_cannot_replace_declared_setup(
    tmp_path: Path,
) -> None:
    path = write_scenario(
        tmp_path,
        '# module:sys\nraise AssertionError("declared setup must execute")\n# ok\npass\n',
    )
    root = tmp_path / "run"
    with pytest.raises(AssertionError, match="ordinary module .* did not execute its declared source"):
        run_strict_scenario(path, root, mode="cpython")
    assert not tuple(root.glob("block-*.complete"))


def test_ordinary_source_witness_rejects_a_foreign_cached_origin(
    tmp_path: Path, monkeypatch,
) -> None:
    name = "ordinary_cached_scenario_unit"
    module = ModuleType(name)
    module.__spec__ = SimpleNamespace(origin=str(tmp_path / "foreign.py"))
    monkeypatch.setitem(sys.modules, name, module)
    with pytest.raises(AssertionError, match="did not execute its declared source"):
        _check_modules(
            ((name, str(tmp_path / "declared.py"), "not-a-contract", ()),),
            "not-a-publication", "cpython", {},
        )


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
