"""Single-file source scenarios over the real authenticated strict test runner."""

from __future__ import annotations

import ast
import builtins
import hashlib
import importlib
import io
import json
import keyword
import re
import subprocess
import tokenize
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import (
    StrictProject,
    _assert_cpython_function_witness,
    _assert_cpython_module_witness,
    _plain_function_witness,
    create_strict_project,
)


@dataclass(frozen=True)
class ScenarioModule:
    name: str
    source: str
    functions: tuple[str, ...]


@dataclass(frozen=True)
class ScenarioBlock:
    line: int
    source: str
    exception: str | None

    @property
    def label(self) -> str:
        return "ok" if self.exception is None else f"raise:{self.exception}"


@dataclass(frozen=True)
class StrictScenario:
    path: Path
    modules: tuple[ScenarioModule, ...]
    blocks: tuple[ScenarioBlock, ...]


def _qualified_name(value: str) -> bool:
    return all(part.isidentifier() and not keyword.iskeyword(part) for part in value.split("."))


class _FunctionBodyYields(ast.NodeVisitor):
    """Nested callable/class bodies do not make the containing def a generator."""

    found = False

    def visit_Yield(self, node: ast.Yield) -> None:
        self.found = True

    def visit_YieldFrom(self, node: ast.YieldFrom) -> None:
        self.found = True

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        for expression in (*node.decorator_list, *node.args.defaults, *node.args.kw_defaults):
            if expression is not None:
                self.visit(expression)

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        for expression in (*node.decorator_list, *node.bases, *(item.value for item in node.keywords)):
            self.visit(expression)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        for expression in (*node.args.defaults, *node.args.kw_defaults):
            if expression is not None:
                self.visit(expression)


def _plain_function(node: ast.stmt) -> bool:
    if not isinstance(node, ast.FunctionDef) or node.decorator_list:
        return False
    yields = _FunctionBodyYields()
    for statement in node.body:
        yields.visit(statement)
    return not yields.found


def parse_strict_scenario(path: Path) -> StrictScenario:
    """Parse column-zero comment directives, never lookalikes inside strings.

    Module declarations precede validation blocks. The first module supplies
    bare names to ordinary validation. Every declared package parent must have
    its own module section; no undeclared source files are synthesized.
    """
    source = path.read_text(encoding="utf-8")
    lines = source.splitlines(keepends=True)
    markers: list[tuple[int, str, str | None]] = []
    depth = 0
    try:
        for token in tokenize.generate_tokens(io.StringIO(source).readline):
            if token.type == tokenize.OP:
                if token.string in "([{":
                    depth += 1
                elif token.string in ")]}":
                    depth -= 1
            if token.type != tokenize.COMMENT or token.start[1] != 0 or depth:
                continue
            text = token.string
            if not re.match(r"#\s*(?:module|ok|raise)(?:\s|:|$)", text):
                continue
            match = re.fullmatch(r"#\s*(?:(module|raise)\s*:\s*(\S+)|ok)\s*", text)
            if match is None:
                raise ValueError(f"{path}:{token.start[0]}: malformed scenario directive")
            kind, value = match.groups()
            kind = kind or "ok"
            if value is not None and not _qualified_name(value):
                raise ValueError(f"{path}:{token.start[0]}: invalid {kind} name {value!r}")
            if kind == "module" and "__init__" in value.split("."):
                raise ValueError(f"{path}:{token.start[0]}: name a package, not its __init__ module")
            markers.append((token.start[0], kind, value))
    except (tokenize.TokenError, IndentationError) as error:
        raise ValueError(f"{path}: invalid scenario syntax: {error}") from error
    if not markers or markers[0][1] != "module":
        raise ValueError(f"{path}: start with a '# module:name' section")
    if ast.parse("".join(lines[: markers[0][0] - 1]), filename=str(path)).body:
        raise ValueError(f"{path}: code before the first module section")

    modules: list[ScenarioModule] = []
    blocks: list[ScenarioBlock] = []
    names: set[str] = set()
    for index, (line, kind, value) in enumerate(markers):
        end = markers[index + 1][0] - 1 if index + 1 < len(markers) else len(lines)
        body = "".join(lines[line:end])
        padded = "\n" * line + body
        tree = ast.parse(padded, filename=str(path))
        if kind == "module":
            if blocks:
                raise ValueError(f"{path}:{line}: module sections must precede test blocks")
            assert value is not None
            if value in names:
                raise ValueError(f"{path}:{line}: duplicate module {value!r}")
            names.add(value)
            explicit = any(
                isinstance(node, ast.ImportFrom) and node.module == "__future__"
                and any(alias.name == "strict" for alias in node.names)
                for node in tree.body
            )
            selected_source = body if explicit else strict_opt_in(
                body.encode("utf-8"), f"{path} (module {value})",
            )[0].decode("utf-8")
            # Plain source functions can provide an unambiguous backend
            # witness. Decorators, generated methods and dynamic classes need
            # their own behavioral assertions rather than guessed identities.
            functions = tuple(
                node.name for node in tree.body
                if _plain_function(node)
            )
            modules.append(ScenarioModule(value, selected_source, functions))
        else:
            if not tree.body:
                raise ValueError(f"{path}:{line}: empty test block")
            blocks.append(ScenarioBlock(line, padded, value))
    if not blocks:
        raise ValueError(f"{path}: expected at least one '# ok' or '# raise:Exception' block")
    for name in names:
        parent = name.rpartition(".")[0]
        if parent and parent not in names:
            raise ValueError(f"{path}: module {name!r} needs a declared parent {parent!r}")
    return StrictScenario(path.resolve(), tuple(modules), tuple(blocks))


def _execute_block(
    module: ModuleType, block: ScenarioBlock, path: Path, *, mode: str,
) -> None:
    """Only the final top-level statement can satisfy an expected exception."""
    namespace = dict(vars(module))
    namespace.update(
        __builtins__=builtins.__dict__, module=module,
        __dp_integration_mode__=mode,
        __dp_integration_strict__=True,
        __dp_integration_soac__=mode in {"soac", "entry"},
        __dp_integration_entry__=mode == "entry",
    )
    tree = ast.parse(block.source, filename=str(path))
    code = compile(tree, str(path), "exec", dont_inherit=True)
    if block.exception is None:
        exec(code, namespace)  # noqa: S102 - trusted ordinary test code
        return
    # Compile both pieces before running either. Preserve this block's future
    # flags and original locations, including multiple statements on one line.
    prefix = compile(
        ast.Module(body=tree.body[:-1], type_ignores=tree.type_ignores),
        str(path), "exec", flags=code.co_flags, dont_inherit=True,
    )
    final = compile(
        ast.Module(body=tree.body[-1:], type_ignores=tree.type_ignores),
        str(path), "exec", flags=code.co_flags, dont_inherit=True,
    )
    exec(prefix, namespace)  # noqa: S102 - setup is outside the expectation
    components = block.exception.split(".")
    if len(components) == 1:
        expected = getattr(builtins, components[0], namespace.get(components[0]))
    else:
        expected = getattr(importlib.import_module(".".join(components[:-1])), components[-1])
    if not isinstance(expected, type) or not issubclass(expected, BaseException):
        raise ValueError(f"{path}:{block.line}: {block.exception!r} is not an exception type")
    try:
        exec(final, namespace)  # noqa: S102 - only the last statement is checked
    except expected:
        return
    raise AssertionError(f"{path}:{block.line}: expected {block.exception}, but block completed")


def _check_modules(
    specifications: tuple[tuple[str, str, str, tuple[str, ...]], ...],
    generation: str,
    mode: str,
) -> dict[str, ModuleType]:
    """Authenticate observations for every declared module, outside expectations."""
    from soac import _soac_ext

    modules = {}
    for name, source_path, source_sha256, functions in specifications:
        module = importlib.import_module(name)
        modules[name] = module
        if mode == "cpython":
            diagnostic = _assert_cpython_module_witness(
                module, module_name=name, source_path=source_path,
                source_sha256=source_sha256, artifact_generation=generation,
            )
            for function in functions:
                _assert_cpython_function_witness(_plain_function_witness(module, function), diagnostic)
        else:
            diagnostic = _soac_ext.strict_module_diagnostics(module)
            assert diagnostic is not None, f"{name} executed without strict admission"
            assert diagnostic["sealed"] is True
            assert diagnostic["module_name"] == name
            assert diagnostic["source_path"] == source_path
            assert diagnostic["artifact_generation"] == generation
            assert diagnostic["initializer_entry_kind"] == "entry_interpreter"
            for function in functions:
                actual = _soac_ext.strict_function_entry_kind(_plain_function_witness(module, function))
                expected = "entry_interpreter" if mode == "entry" else "checked_native"
                assert actual == expected, (name, function, actual)
    return modules


def run_strict_scenario(path: Path, root: Path, *, mode: str = "soac") -> StrictProject:
    """Analyze once; run every block in a fresh process, retaining all failures.

    Parsing, checking, admission, imports and witnesses never satisfy # raise.
    A completion receipt also rejects a zero-exit process that skipped the
    expectation (for example os._exit(0)). The original helper still owns
    signing, native startup, execution-mode selection and subprocess logs.
    """
    if mode not in {"soac", "entry", "cpython"}:
        raise ValueError(f"unsupported strict scenario mode {mode!r}")
    scenario = parse_strict_scenario(path)
    names = {module.name for module in scenario.modules}
    paths = {
        module.name: module.name.replace(".", "/") + (
            "/__init__.py" if any(name.startswith(module.name + ".") for name in names) else ".py"
        )
        for module in scenario.modules
    }
    assert len(set(paths.values())) == len(paths), "scenario module paths must be distinct"
    project = create_strict_project(
        root, {paths[module.name]: module.source for module in scenario.modules},
        modules=paths,
        policy=(
            "[tool.soac.strict]\ninclude = " + json.dumps(list(paths.values()))
            + '\nchecked_fields = "supported_annotations"\n'
        ),
        backend="cpython" if mode == "cpython" else "soac",
    )
    specifications = tuple(
        (
            module.name, str(project.project / paths[module.name]),
            hashlib.sha256((project.project / paths[module.name]).read_bytes()).hexdigest(),
            module.functions,
        )
        for module in scenario.modules
    )
    failures = []
    for index, block in enumerate(scenario.blocks, 1):
        receipt = project.root / f"block-{index}.complete"
        validator = f"""
def validate_module(module):
    from pathlib import Path
    from tests._strict_scenarios import ScenarioBlock, _check_modules, _execute_block
    specifications = {specifications!r}
    before = _check_modules(specifications, {project.publication['generation']!r}, {mode!r})
    _execute_block(module, ScenarioBlock({block.line!r}, {block.source!r}, {block.exception!r}),
                   Path({str(scenario.path)!r}), mode={mode!r})
    after = _check_modules(specifications, {project.publication['generation']!r}, {mode!r})
    assert all(after[name] is original for name, original in before.items())
    Path({str(receipt)!r}).write_text('complete')
"""
        try:
            project.run_case(
                scenario.modules[0].name, validator, scenario.path,
                entry_interpreter=mode == "entry",
                required_functions=scenario.modules[0].functions,
            )
            assert receipt.is_file() and receipt.read_text() == "complete", (
                "runtime exited without completing the block and contract witnesses"
            )
        except (AssertionError, OSError, subprocess.TimeoutExpired) as error:
            failures.append(f"{scenario.path}:{block.line} [{block.label}]: {error}")
    assert not failures, "\n\n".join(failures)
    return project
