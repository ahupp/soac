"""Single-file source scenarios over the real authenticated strict test runner."""

from __future__ import annotations

import ast
import builtins
import hashlib
import importlib
import io
import keyword
import re
import subprocess
import tokenize
from dataclasses import dataclass
from pathlib import Path
from types import FunctionType, ModuleType

from tests._strict_integration import (
    ROOT,
    STRICT_RUNTIME_TIMEOUT as STRICT_RUNTIME_TIMEOUT,
    StrictProject,
    _assert_cpython_function_witness,
    _assert_cpython_module_witness,
    create_strict_project,
)


SCENARIO_MODES = ("soac", "entry", "cpython")


@dataclass(frozen=True)
class ScenarioModule:
    name: str
    source: str
    functions: tuple[tuple[str, int], ...]


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
    modes: tuple[str, ...] = SCENARIO_MODES


def scenario_pytest_id(scenario: StrictScenario, mode: str, root: Path) -> str:
    """One shared spelling for collection and exact supervisor enrollment."""
    relative = scenario.path.relative_to(root).with_suffix("").as_posix()
    return f"{relative}-{mode}"


def _qualified_name(value: str) -> bool:
    return all(
        part.isidentifier() and not keyword.iskeyword(part) for part in value.split(".")
    )


class _FunctionBodyYields(ast.NodeVisitor):
    """Nested callable/class bodies do not make the containing def a generator."""

    found = False

    def visit_Yield(self, node: ast.Yield) -> None:
        self.found = True

    def visit_YieldFrom(self, node: ast.YieldFrom) -> None:
        self.found = True

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        for expression in (
            *node.decorator_list,
            *node.args.defaults,
            *node.args.kw_defaults,
        ):
            if expression is not None:
                self.visit(expression)

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        for expression in (
            *node.decorator_list,
            *node.bases,
            *(item.value for item in node.keywords),
        ):
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
    source = path.read_bytes().decode("utf-8")
    # Universal physical line boundaries, without rewriting the section bytes.
    # str.splitlines also splits legal string contents such as U+0085/U+2028.
    lines = io.StringIO(source, newline="").readlines()
    markers: list[tuple[int, str, str | None]] = []
    modes = None
    depth = 0
    try:
        for token in tokenize.generate_tokens(
            io.StringIO(source, newline=None).readline
        ):
            if token.type == tokenize.OP:
                if token.string in "([{":
                    depth += 1
                elif token.string in ")]}":
                    depth -= 1
            if token.type != tokenize.COMMENT or token.start[1] != 0 or depth:
                continue
            text = token.string
            if re.match(r"#\s*modes(?:\s*:|\s*$)", text):
                if markers or modes is not None:
                    raise ValueError(
                        f"{path}:{token.start[0]}: modes must appear once before modules"
                    )
                match = re.fullmatch(r"#\s*modes\s*:\s*(.+?)\s*", text)
                if match is None:
                    raise ValueError(
                        f"{path}:{token.start[0]}: malformed modes directive"
                    )
                modes = tuple(part.strip() for part in match[1].split(","))
                if len(set(modes)) != len(modes) or any(
                    mode not in SCENARIO_MODES for mode in modes
                ):
                    raise ValueError(
                        f"{path}:{token.start[0]}: modes must be distinct names from "
                        + ", ".join(SCENARIO_MODES)
                    )
                continue
            if not re.match(r"#\s*(?:module|ok|raise)(?:\s*:|\s*$)", text):
                continue
            match = re.fullmatch(r"#\s*(?:(module|raise)\s*:\s*(\S+)|ok)\s*", text)
            if match is None:
                raise ValueError(
                    f"{path}:{token.start[0]}: malformed scenario directive"
                )
            kind, value = match.groups()
            kind = kind or "ok"
            if value is not None and not _qualified_name(value):
                raise ValueError(
                    f"{path}:{token.start[0]}: invalid {kind} name {value!r}"
                )
            if kind == "module" and "__init__" in value.split("."):
                raise ValueError(
                    f"{path}:{token.start[0]}: name a package, not its __init__ module"
                )
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
                raise ValueError(
                    f"{path}:{line}: module sections must precede test blocks"
                )
            assert value is not None
            if value in names:
                raise ValueError(f"{path}:{line}: duplicate module {value!r}")
            names.add(value)
            # Plain source functions can provide an unambiguous backend
            # witness. Decorators, generated methods and dynamic classes need
            # their own behavioral assertions rather than guessed identities.
            functions = tuple(
                (node.name, node.lineno - line)
                for node in tree.body
                if _plain_function(node)
            )
            # Source policy is production syntax, not a scenario default.
            # Keep exact source bytes; the real checker resolves all scopes.
            modules.append(ScenarioModule(value, body, functions))
        else:
            if not tree.body:
                raise ValueError(f"{path}:{line}: empty test block")
            blocks.append(ScenarioBlock(line, padded, value))
    if not blocks:
        raise ValueError(
            f"{path}: expected at least one '# ok' or '# raise:Exception' block"
        )
    for name in names:
        parent = name.rpartition(".")[0]
        if parent and parent not in names:
            raise ValueError(
                f"{path}: module {name!r} needs a declared parent {parent!r}"
            )
    return StrictScenario(
        path.resolve(), tuple(modules), tuple(blocks), modes or SCENARIO_MODES
    )


def discover_strict_scenarios(root: Path) -> tuple[StrictScenario, ...]:
    """Enroll every source file in the tree, with stable relative-path ordering.

    These files are scenario input, not importable pytest modules. Fail on an
    empty/mistyped tree instead of silently collecting no behavioral coverage.
    """
    paths = sorted(
        root.rglob("*.py"), key=lambda path: path.relative_to(root).as_posix()
    )
    if not paths:
        raise ValueError(f"{root}: no strict scenario files found")
    return tuple(parse_strict_scenario(path) for path in paths)


@dataclass
class _ExceptionExpectation:
    namespace: dict
    block: ScenarioBlock
    path: Path
    expected: type[BaseException] | None = None

    def resolve(self) -> None:
        assert self.block.exception is not None
        components = self.block.exception.split(".")
        if len(components) == 1:
            name = components[0]
            expected = (
                self.namespace[name]
                if name in self.namespace
                else vars(builtins).get(name)
            )
        else:
            expected = getattr(
                importlib.import_module(".".join(components[:-1])), components[-1]
            )
        if not isinstance(expected, type) or not issubclass(expected, BaseException):
            raise ValueError(
                f"{self.path}:{self.block.line}: {self.block.exception!r} is not an exception type"
            )
        self.expected = expected

    def missing(self) -> None:
        raise AssertionError(
            f"{self.path}:{self.block.line}: expected {self.block.exception}, but block completed"
        )


def _expectation_binding(tree: ast.Module, namespace: dict) -> str:
    # Include identifiers stored as strings (defs, aliases, exception targets,
    # patterns and global statements), not just ast.Name nodes. String literals
    # are included too, so a literal globals()[name] use cannot collide either.
    # Hashing arbitrary non-string module keys would run unrelated callbacks.
    occupied = {str.__str__(name) for name in namespace if isinstance(name, str)}
    for node in ast.walk(tree):
        for _, value in ast.iter_fields(node):
            if isinstance(value, str):
                occupied.add(value)
            elif isinstance(value, list):
                occupied.update(item for item in value if isinstance(item, str))
    name = "_soac_scenario_expectation"
    while name in occupied:
        name += "_"
    return name


def _execute_block(
    module: ModuleType,
    block: ScenarioBlock,
    path: Path,
    *,
    mode: str,
    strict: bool = True,
) -> None:
    """Only the final top-level statement can satisfy an expected exception."""
    namespace = dict(vars(module))
    namespace.update(
        __builtins__=builtins.__dict__,
        module=module,
        __dp_integration_mode__=mode,
        __dp_integration_strict__=strict,
        __dp_integration_soac__=mode in {"soac", "entry"},
        __dp_integration_entry__=mode == "entry",
    )
    tree = ast.parse(block.source, filename=str(path))
    code = compile(tree, str(path), "exec", dont_inherit=True)
    if block.exception is None:
        exec(code, namespace)  # noqa: S102 - trusted ordinary test code
        return
    expectation = _ExceptionExpectation(namespace, block, path)
    last = tree.body[-1]
    if isinstance(last, ast.ImportFrom) and last.module == "__future__":
        # A legal future import cannot be moved inside a try statement. The
        # already-validated prefix contains only a docstring/future imports,
        # so splitting this one case cannot lose hoisted annotation setup.
        prefix = compile(
            ast.Module(body=tree.body[:-1], type_ignores=tree.type_ignores),
            str(path),
            "exec",
            flags=code.co_flags,
            dont_inherit=True,
        )
        final = compile(
            ast.Module(body=[last], type_ignores=tree.type_ignores),
            str(path),
            "exec",
            flags=code.co_flags,
            dont_inherit=True,
        )
        exec(prefix, namespace)  # noqa: S102 - setup is outside the expectation
        expectation.resolve()
        try:
            exec(final, namespace)  # noqa: S102 - only the last statement is checked
        except expectation.expected:
            return
        expectation.missing()
        return

    # Keep one compiler scope: splitting an annotated last statement into a
    # separate module changes SETUP_ANNOTATIONS and deferred annotation setup.
    # Resolution is after the prefix but outside the final statement's try.
    binding = _expectation_binding(tree, namespace)

    def attribute(name: str) -> ast.Attribute:
        return ast.Attribute(
            value=ast.Name(id=binding, ctx=ast.Load()), attr=name, ctx=ast.Load()
        )

    def call(name: str) -> ast.Expr:
        return ast.Expr(value=ast.Call(func=attribute(name), args=[], keywords=[]))

    wrapped = ast.Module(
        body=[
            *tree.body[:-1],
            ast.copy_location(call("resolve"), last),
            ast.copy_location(
                ast.Try(
                    body=[last],
                    handlers=[
                        ast.ExceptHandler(
                            type=attribute("expected"), name=None, body=[ast.Pass()]
                        )
                    ],
                    orelse=[call("missing")],
                    finalbody=[],
                ),
                last,
            ),
        ],
        type_ignores=tree.type_ignores,
    )
    wrapped_code = compile(
        ast.fix_missing_locations(wrapped),
        str(path),
        "exec",
        flags=code.co_flags,
        dont_inherit=True,
    )
    namespace[binding] = expectation
    try:
        exec(wrapped_code, namespace)  # noqa: S102 - ordinary validation, final statement only
    finally:
        namespace.pop(binding, None)


def _surviving_function_witnesses(
    module: ModuleType,
    source_path: str,
    definitions: tuple[tuple[str, int], ...],
) -> tuple[FunctionType, ...]:
    """Select actual surviving definitions, not syntactic bindings or authority.

    Initializers can delete/rebind a def's name and retain it through an alias,
    even when their final module bindings will be frozen. Source coordinates
    only select test subjects: every selected value still needs its real native
    ownership witness. In particular, missing native ownership is not a reason
    to omit a matching local function.
    """
    namespace = vars(module)
    locations = set(definitions)
    seen: set[int] = set()
    functions = []
    for value in namespace.values():
        if type(value) is not FunctionType or value.__globals__ is not namespace:
            continue
        code = value.__code__
        if (
            code.co_filename != source_path
            or (code.co_qualname, code.co_firstlineno) not in locations
        ):
            continue
        if id(value) not in seen:
            seen.add(id(value))
            functions.append(value)
    return tuple(functions)


def _check_modules(
    specifications: tuple[tuple[str, str, str, tuple[tuple[str, int], ...]], ...],
    generation: str,
    mode: str,
    policies: dict,
) -> dict[str, ModuleType]:
    """Authenticate observations for every declared module, outside expectations."""
    from soac import _soac_ext
    import ctypes

    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p

    modules = {}
    for name, source_path, source_sha256, definitions in specifications:
        module = importlib.import_module(name)
        modules[name] = module
        functions = _surviving_function_witnesses(module, source_path, definitions)
        policy = policies.get(name)
        if policy is None:
            assert type(module) is ModuleType, (
                f"ordinary module {name} changed representation"
            )
            origin = getattr(vars(module).get("__spec__"), "origin", None)
            assert (
                isinstance(origin, str)
                and Path(origin).resolve() == Path(source_path).resolve()
            ), (
                f"ordinary module {name} did not execute its declared source: "
                f"expected {source_path}, observed {origin!r}"
            )
            assert _soac_ext.strict_module_diagnostics(module) is None, (
                f"ordinary module {name} acquired strict ownership"
            )
            for function in functions:
                assert not owner(function) and not metadata(function), (
                    f"ordinary function {name}.{function.__qualname__} acquired strict ownership"
                )
            continue
        strict_assign = policy["strict_assign"]
        if mode == "cpython":
            diagnostic = _assert_cpython_module_witness(
                module,
                module_name=name,
                source_path=source_path,
                source_sha256=source_sha256,
                artifact_generation=generation,
                strict_assign=strict_assign,
            )
            for function in functions:
                _assert_cpython_function_witness(function, diagnostic)
        else:
            diagnostic = _soac_ext.strict_module_diagnostics(module)
            assert diagnostic is not None, f"{name} executed without strict admission"
            assert diagnostic["schema"] == 2
            assert diagnostic["ready"] is True
            assert diagnostic["strict_assign"] is strict_assign
            assert diagnostic["sealed"] is strict_assign
            assert diagnostic["module_name"] == name
            assert diagnostic["source_path"] == source_path
            assert diagnostic["artifact_generation"] == generation
            assert diagnostic["initializer_entry_kind"] == "entry_interpreter"
            for function in functions:
                actual = _soac_ext.strict_function_entry_kind(function)
                expected = "entry_interpreter" if mode == "entry" else "checked_native"
                assert actual == expected, (name, function.__qualname__, actual)
    return modules


def _declares_package(source: str) -> bool:
    """A package directive also identifies a lone package section's filename.

    Only tokenize to choose __init__.py versus .py; the checker owns grammar,
    placement, rule resolution and authentication, including malformed input.
    """
    return any(
        token.type == tokenize.COMMENT
        and token.start[1] == 0
        and re.match(r"#\s*soac\s*:\s*package\s*\(", token.string)
        for token in tokenize.generate_tokens(
            io.StringIO(source, newline=None).readline
        )
    )


def run_strict_scenario(path: Path, root: Path, *, mode: str = "soac") -> StrictProject:
    """Analyze once; run every block in a fresh process, retaining all failures.

    Parsing, checking, admission, imports and witnesses never satisfy # raise.
    A completion receipt also rejects a zero-exit process that skipped the
    expectation (for example os._exit(0)). The original helper still owns
    signing, native startup, execution-mode selection and subprocess logs.
    """
    if mode not in SCENARIO_MODES:
        raise ValueError(f"unsupported strict scenario mode {mode!r}")
    scenario = parse_strict_scenario(path)
    if mode not in scenario.modes:
        raise ValueError(f"{path}: scenario is not enrolled for mode {mode!r}")
    names = {module.name for module in scenario.modules}
    paths = {
        module.name: module.name.replace(".", "/")
        + (
            "/__init__.py"
            if _declares_package(module.source)
            or any(name.startswith(module.name + ".") for name in names)
            else ".py"
        )
        for module in scenario.modules
    }
    assert len(set(paths.values())) == len(paths), (
        "scenario module paths must be distinct"
    )
    project = create_strict_project(
        root,
        {paths[module.name]: module.source for module in scenario.modules},
        modules=paths,
        preserve_source=True,
        backend="cpython" if mode == "cpython" else "soac",
    )
    specifications = tuple(
        (
            module.name,
            str(project.project / paths[module.name]),
            hashlib.sha256(
                (project.project / paths[module.name]).read_bytes()
            ).hexdigest(),
            module.functions,
        )
        for module in scenario.modules
    )
    failures = []
    for index, block in enumerate(scenario.blocks, 1):
        receipt = project.root / f"block-{index}.complete"
        policies = {name: dict(policy) for name, policy in project.policies.items()}
        validator = f"""
sys.path.insert(0, {str(ROOT)!r})
from pathlib import Path
from tests._strict_scenarios import ScenarioBlock, _check_modules, _execute_block
specifications = {specifications!r}
policies = {policies!r}
before = _check_modules(specifications, {project.publication["generation"]!r}, {mode!r}, policies)
module = before[{scenario.modules[0].name!r}]
_execute_block(module, ScenarioBlock({block.line!r}, {block.source!r}, {block.exception!r}),
               Path({str(scenario.path)!r}), mode={mode!r},
               strict={scenario.modules[0].name in project.policies!r})
after = _check_modules(specifications, {project.publication["generation"]!r}, {mode!r}, policies)
assert all(after[name] is original for name, original in before.items())
Path({str(receipt)!r}).write_text('complete')
"""
        try:
            project.run(
                validator,
                entry_interpreter=mode == "entry",
            )
            assert receipt.is_file() and receipt.read_text() == "complete", (
                "runtime exited without completing the block and contract witnesses"
            )
        except (AssertionError, OSError, subprocess.TimeoutExpired) as error:
            failures.append(f"{scenario.path}:{block.line} [{block.label}]: {error}")
    assert not failures, "\n\n".join(failures)
    return project
