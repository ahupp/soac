from __future__ import annotations

import json
import textwrap

import pytest

from tests._strict_integration import (
    _VALIDATION_PRELUDE,
    StrictValidationCase,
    create_strict_project,
)

_FUNCTION_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
def outer(a):
    x = 10

    def inner(b):
        return a + b + x

    return inner


class Example:
    def method(self):
        return 42
"""

_GENERATOR_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
def generator(value):
    yield value
    yield value + 1


def generator_expression(offset):
    return (offset + value for value in range(2))


async def coroutine(value):
    return value


async def async_generator(value):
    yield value
"""

_FUNCTION_VALIDATION = """
inner = module.outer(3)
other_inner = module.outer(9)
assert native_owner(inner) and native_owner(other_inner)
assert _soac_ext.strict_function_entry_kind(inner) == expected_entry
assert _soac_ext.strict_function_entry_kind(other_inner) == expected_entry

assert module.outer.__code__.co_name == "outer"
assert module.outer.__code__.co_qualname == "outer"
assert module.outer.__code__.co_firstlineno == 2
assert module.outer.__code__.co_filename.endswith("original_code_object.py")

assert inner(4) == 17
assert inner.__code__.co_name == "inner"
assert inner.__code__.co_qualname == "outer.<locals>.inner"
assert inner.__code__.co_firstlineno == 5
assert inner.__code__.co_freevars == ("a", "x")
assert inner is not other_inner
assert inner.__code__ is other_inner.__code__
assert other_inner(4) == 23

assert module.Example().method() == 42
assert module.Example.method.__code__.co_name == "method"
assert module.Example.method.__code__.co_qualname == "Example.method"
assert module.Example.method.__code__.co_firstlineno == 12
assert _soac_ext.strict_function_entry_kind(inner) == expected_entry
assert _soac_ext.strict_function_entry_kind(other_inner) == expected_entry
"""

_GENERATOR_VALIDATION = """
first_generator = module.generator(3)
second_generator = module.generator(9)
assert type(first_generator) is types.GeneratorType
assert first_generator.gi_code is module.generator.__code__
assert second_generator.gi_code is first_generator.gi_code
assert (
    next(first_generator),
    next(second_generator),
    next(first_generator),
    next(second_generator),
) == (3, 9, 4, 10)

first_expression = module.generator_expression(3)
second_expression = module.generator_expression(9)
assert type(first_expression) is types.GeneratorType
expression_code = next(
    constant
    for constant in module.generator_expression.__code__.co_consts
    if getattr(constant, "co_name", None) == "<genexpr>"
)
assert first_expression.gi_code is expression_code
assert second_expression.gi_code is expression_code
assert (
    next(first_expression),
    next(second_expression),
    next(first_expression),
    next(second_expression),
) == (3, 9, 4, 10)

first_coroutine = module.coroutine(3)
second_coroutine = module.coroutine(9)
assert type(first_coroutine) is types.CoroutineType
assert first_coroutine.cr_code is module.coroutine.__code__
assert second_coroutine.cr_code is first_coroutine.cr_code
with pytest.raises(StopIteration) as first_result:
    first_coroutine.send(None)
with pytest.raises(StopIteration) as second_result:
    second_coroutine.send(None)
assert first_result.value.value == 3
assert second_result.value.value == 9

first_async_generator = module.async_generator(3)
second_async_generator = module.async_generator(9)
assert type(first_async_generator) is types.AsyncGeneratorType
assert first_async_generator.ag_code is module.async_generator.__code__
assert second_async_generator.ag_code is first_async_generator.ag_code
with pytest.raises(StopIteration) as first_async_result:
    first_async_generator.__anext__().send(None)
with pytest.raises(StopIteration) as second_async_result:
    second_async_generator.__anext__().send(None)
assert first_async_result.value.value == 3
assert second_async_result.value.value == 9
"""


_GENERATOR_EXPRESSION_SHAPES_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)

def same_line(values):
    return (value for value in values), (value + 1 for value in values)

def nested(values):
    return ((value for value in row) for row in values)

def multiline(values):
    return (
        value
        for value in values
    )

async def asynchronous(values):
    return (value async for value in values)
"""


_GENERATOR_CREATE_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)

def make_zero():
    return (item for item in (1,))

def make_captured(marker):
    return (marker for item in (1,))
"""


@pytest.fixture(scope="module")
def original_code_project(tmp_path_factory):
    class_defs = "\n".join(
        f"""
class C{index}:
    value = {index}

    def method(self):
        return self.value
"""
        for index in range(8)
    )
    assert class_defs.startswith("\n")
    return create_strict_project(
        tmp_path_factory.mktemp("strict-original-code-objects"),
        {
            "original_code_object.py": _FUNCTION_SOURCE,
            "generator_original_code_object.py": _GENERATOR_SOURCE,
            "generator_expression_code_shapes.py": _GENERATOR_EXPRESSION_SHAPES_SOURCE,
            "generator_create_boundary.py": _GENERATOR_CREATE_SOURCE,
            "ordinary_generator_create_boundary.py": _GENERATOR_CREATE_SOURCE.replace(
                "# soac: module(strict_assign=true, checked_attr=true)", "# ordinary creation control", 1
            ),
            "class_helper_import_storm.py": "# soac: module(strict_assign=true, checked_attr=true)\n"
            + class_defs[1:],
        },
        modules={
            "original_code_object": "original_code_object.py",
            "generator_original_code_object": "generator_original_code_object.py",
            "generator_expression_code_shapes": "generator_expression_code_shapes.py",
            "generator_create_boundary": "generator_create_boundary.py",
            "class_helper_import_storm": "class_helper_import_storm.py",
        },
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_soac_functions_expose_original_code_objects(
    original_code_project, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    imports = f"""
import ctypes
from soac import _soac_ext
native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
native_owner.argtypes = [ctypes.py_object]
native_owner.restype = ctypes.c_void_p
expected_entry = {expected_entry!r}
"""
    validation = "def validate(module):\n" + textwrap.indent(
        imports + _FUNCTION_VALIDATION, "    "
    )
    original_code_project.run_case(
        "original_code_object",
        validation,
        original_code_project.project / "original_code_object.py",
        entry_interpreter=entry_interpreter,
        required_functions=("outer", "Example.method"),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generator_instances_reuse_original_code_objects(
    original_code_project, entry_interpreter
):
    project = original_code_project
    imports = """
import ctypes
import pytest
from soac import _soac_ext
import types
native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
native_owner.argtypes = [ctypes.py_object]
native_owner.restype = ctypes.c_void_p
def assert_factory_owners():
    for name in ('generator', 'coroutine', 'async_generator'):
        function = vars(module)[name]
        assert native_owner(function), name
        assert _soac_ext.strict_function_entry_kind(function) == 'generator_factory', name
assert_factory_owners()
"""
    validation = "def validate(module):\n" + textwrap.indent(
        imports + _GENERATOR_VALIDATION + "\nassert_factory_owners()\n", "    "
    )
    program = _VALIDATION_PRELUDE + project._validation_program(
        "generator_original_code_object",
        StrictValidationCase(
            validation,
            project.project / "generator_original_code_object.py",
            ("generator_expression",),
        ),
        entry_interpreter=entry_interpreter,
    )
    project.run(
        program,
        entry_interpreter=entry_interpreter,
        opt_mode="profile",
        extra_env={
            "SOAC_WORK_DIR": str(
                project.root / f"generator-profile-{int(entry_interpreter)}"
            )
        },
    )


def test_interpreter_generator_instances_reuse_original_code_objects(tmp_path):
    project = create_strict_project(
        tmp_path,
        {"native_generator_code_identity.py": _GENERATOR_SOURCE},
        modules={"native_generator_code_identity": "native_generator_code_identity.py"},
        backend="cpython",
    )
    # Same source and full value/code-identity observer, with no SOAC lowering
    # or JIT entry. run_case proves each actual native source/body owner.
    validation = "def validate(module):\n" + textwrap.indent(
        "import types\nimport pytest\n" + _GENERATOR_VALIDATION, "    "
    )
    project.run_case(
        "native_generator_code_identity", validation,
        project.project / "native_generator_code_identity.py",
        required_functions=("generator", "generator_expression", "coroutine", "async_generator"),
        backend="cpython",
    )


def test_generated_class_helpers_do_not_lazy_jit_during_import(original_code_project):
    project = original_code_project
    log_path = project.root / "class-helper-events.jsonl"
    validation = """
def validate(module):
    assert [getattr(module, f'C{i}')().method() for i in range(8)] == list(range(8))
"""
    program = _VALIDATION_PRELUDE + project._validation_program(
        "class_helper_import_storm",
        StrictValidationCase(
            validation,
            project.project / "class_helper_import_storm.py",
            tuple(f"C{index}.method" for index in range(8)),
        ),
        entry_interpreter=False,
    )
    project.run(
        program,
        opt_mode="profile",
        extra_env={
            "SOAC_WORK_DIR": str(project.root / "class-helper-profile"),
            "SOAC_LOG": f"soac_jit_codegen=info;json={log_path}",
        },
    )
    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    module_codegen = [
        row
        for row in rows
        if row.get("event") in ("soac.jit_codegen", "soac.jit_batch_codegen")
        and row["module_name"] == "class_helper_import_storm"
    ]
    assert module_codegen and all(row["status"] == "ok" for row in module_codegen)
    # Eager preparation uses batch codegen, whose event names only the root.
    # The native code inventory proves every emitted member, not just that root.
    code_rows = [
        json.loads(line)
        for line in (project.root / "class-helper-profile/jit-code-summary.jsonl")
        .read_text(encoding="utf-8")
        .splitlines()
        if line.strip()
    ]
    native_methods = {
        row["function_qualname"]
        for row in code_rows
        if row["entry_kind"] == "direct_function_body" and row["code_size"] > 0
    }
    assert native_methods == {f"C{index}.method" for index in range(8)}
    class_helper_codegen = [
        row
        for row in code_rows
        if row["function_qualname"].startswith(("_dp_class_ns_", "_dp_define_class_"))
    ]
    assert class_helper_codegen == []


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generator_expression_codes_distinguish_exact_original_source_occurrences(
    original_code_project, entry_interpreter
):
    original_code_project.run_case(
        "generator_expression_code_shapes",
        textwrap.dedent("""
        def validate(module):
            import ctypes
            import types
            import pytest
            from soac import _soac_ext
            from soac.strict import StrictRuntimeUnavailableError

            def expression_codes(code):
                return tuple(
                    constant for constant in code.co_consts
                    if type(constant) is types.CodeType and constant.co_name == "<genexpr>"
                )

            first_codes = expression_codes(module.same_line.__code__)
            assert len(first_codes) == 2
            assert first_codes[0] is not first_codes[1]
            assert first_codes[0].co_firstlineno == first_codes[1].co_firstlineno
            assert first_codes[0].co_qualname == first_codes[1].co_qualname
            first = module.same_line([3, 4])
            second = module.same_line([8, 9])
            for pair in (first, second):
                assert all(type(value) is types.GeneratorType for value in pair)
                assert pair[0].gi_code is first_codes[0]
                assert pair[1].gi_code is first_codes[1]
            assert list(first[0]) == [3, 4]
            assert list(first[1]) == [4, 5]
            assert list(second[0]) == [8, 9]
            assert list(second[1]) == [9, 10]
            assert first[0].gi_code is first_codes[0], "closure keeps original code after completion"

            (outer_code,) = expression_codes(module.nested.__code__)
            (inner_code,) = expression_codes(outer_code)
            outer = module.nested([[1, 2], [5, 6]])
            assert outer.gi_code is outer_code
            left = next(outer)
            right = next(outer)
            assert left.gi_code is inner_code and right.gi_code is inner_code
            assert list(left) == [1, 2] and list(right) == [5, 6]
            outer.close()
            assert outer.gi_code is outer_code

            (multiline_code,) = expression_codes(module.multiline.__code__)
            multiline = module.multiline([11, 12])
            assert multiline.gi_code is multiline_code
            assert list(multiline) == [11, 12]

            async def native_stream():
                yield 17
                yield 18
            stream = native_stream()
            pending = module.asynchronous(stream)
            with pytest.raises(StopIteration) as made:
                pending.send(None)
            asynchronous = made.value.value
            (async_code,) = expression_codes(module.asynchronous.__code__)
            assert type(asynchronous) is types.AsyncGeneratorType
            assert asynchronous.ag_code is async_code
            with pytest.raises(StopIteration) as first_item:
                asynchronous.__anext__().send(None)
            assert first_item.value.value == 17
            with pytest.raises(StopIteration) as second_item:
                asynchronous.__anext__().send(None)
            assert second_item.value.value == 18
            with pytest.raises(StopIteration):
                asynchronous.aclose().send(None)
            with pytest.raises(StopIteration):
                stream.aclose().send(None)
            assert asynchronous.ag_code is async_code
            assert _soac_ext.strict_function_entry_kind(module.asynchronous) == "generator_factory"

            # Exposing native code is not function admission. A fresh ordinary
            # function made from the code has no authenticated creation owner.
            assert first_codes[0].co_freevars == ()
            copied = types.FunctionType(first_codes[0], vars(module))
            native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            native_owner.argtypes = [ctypes.py_object]
            native_owner.restype = ctypes.c_void_p
            assert native_owner(copied) is None
            with pytest.raises(StrictRuntimeUnavailableError):
                copied(iter([1, 2]))
        """),
        original_code_project.project / "generator_expression_code_shapes.py",
        entry_interpreter=entry_interpreter,
        required_functions=("same_line", "nested", "multiline"),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("captured", [False, True])
def test_generator_expression_helpers_reject_native_entry_during_creation(
    original_code_project, function_create_watch_extension, entry_interpreter, captured
):
    original_code_project.run_case(
        "generator_create_boundary",
        textwrap.dedent(f"""
        def validate(module):
            import __future__
            import ctypes
            import importlib.util
            import resource
            import types
            import ordinary_generator_create_boundary as ordinary
            from soac import _soac_ext
            from soac.strict import StrictRuntimeUnavailableError

            # A broken CREATE boundary must fail this subprocess, not leave a
            # large core file. The native watcher saves pending exception state.
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
            spec = importlib.util.spec_from_file_location(
                "_strict_function_create_watch", {str(function_create_watch_extension)!r}
            )
            watcher = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(watcher)
            captured = {captured!r}
            name = "make_captured" if captured else "make_zero"
            marker = object()
            arguments = (marker,) if captured else ()
            expected = [marker] if captured else [1]
            strict_flag = __future__.strict.compiler_flag
            native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            native_owner.argtypes = [ctypes.py_object]
            native_owner.restype = ctypes.c_void_p

            # Observing an ordinary captured function before closure installation
            # is valid. Invoking it then can crash stock CPython at COPY_FREE_VARS,
            # so only the ordinary zero-capture control is invoked early.
            events = watcher.watch(
                ordinary.__dict__, "<genexpr>", (iter(()),), invoke=not captured
            )
            try:
                iterator = getattr(ordinary, name)(*arguments)
            finally:
                watcher.stop()
            assert len(events) == 1
            event = events[0]
            assert not event["owner_present"] and event["source_id"] == 0
            assert not event["flags"] & strict_flag
            assert not event["closure_present"]
            assert bool(event["freevars"]) is captured
            assert not native_owner(event["function"])
            if not captured:
                assert event["success"] is True
                assert type(event["result"]) is types.GeneratorType
                event["result"].close()
            assert list(iterator) == expected
            events.clear()

            factory = getattr(module, name)
            (original_code,) = (
                constant for constant in factory.__code__.co_consts
                if type(constant) is types.CodeType and constant.co_name == "<genexpr>"
            )
            for invoke in (False, True):
                events = watcher.watch(
                    module.__dict__, "<genexpr>", (iter(()),), invoke=invoke
                )
                try:
                    iterator = factory(*arguments)
                finally:
                    watcher.stop()
                assert len(events) == 1
                event = events[0]
                # These are native birth observations, before owner/closure init.
                assert event["flags"] & strict_flag
                assert not event["closure_present"]
                assert bool(event["freevars"]) is captured
                assert not event["owner_present"]
                assert event["source_id"] == 0
                if invoke:
                    assert event["success"] is False
                    assert isinstance(event["result"], StrictRuntimeUnavailableError)
                else:
                    assert event["invoked"] is False
                helper = event["function"]
                assert native_owner(helper)
                assert _soac_ext.strict_function_entry_kind(helper) == "generator_factory"
                assert helper.__code__ is not original_code
                assert type(iterator) is types.GeneratorType
                assert iterator.gi_code is original_code
                assert list(iterator) == expected
                assert iterator.gi_code is original_code
                events.clear()
        """),
        original_code_project.project / "generator_create_boundary.py",
        entry_interpreter=entry_interpreter,
        required_functions=("make_zero", "make_captured"),
    )


_MANAGED_GENERATOR_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)

def values(events, marker):
    events.append(("entered", marker))
    try:
        yield marker
    finally:
        events.append(("released", marker))
    return marker + 1

def make(events, marker):
    return values(events, marker)
"""


@pytest.fixture(scope="module")
def managed_generator_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-managed-generator-materialization"),
        {"managed_generator_materialization.py": _MANAGED_GENERATOR_SOURCE},
        modules={
            "managed_generator_materialization": "managed_generator_materialization.py"
        },
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_escaping_generators_preserve_native_identity_and_lifecycle(
    managed_generator_project, entry_interpreter
):
    ordinary_source = _MANAGED_GENERATOR_SOURCE.replace(
        "# soac: module(strict_assign=true, checked_attr=true)", "# ordinary materialization control", 1
    )
    managed_generator_project.run_case(
        "managed_generator_materialization",
        textwrap.dedent(f"""
        def validate(module):
            import ctypes
            import types
            import pytest
            from soac import _soac_ext

            ordinary = types.ModuleType("ordinary_generator_materialization")
            exec(compile({ordinary_source!r}, "ordinary_generator_materialization.py", "exec"),
                 vars(ordinary))
            native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            native_owner.argtypes = [ctypes.py_object]
            native_owner.restype = ctypes.c_void_p
            assert native_owner(module.values)
            assert _soac_ext.strict_function_entry_kind(module.values) == "generator_factory"

            def exercise(target, optimized):
                events = []
                created = target.make(events, 11)
                assert type(created) is types.GeneratorType
                assert created.gi_code is target.values.__code__
                assert created.gi_state == "GEN_CREATED"
                assert not created.gi_running and not created.gi_suspended
                assert events == []
                # Frame inspection remains an ordinary CPython control only.
                if not optimized:
                    assert created.gi_frame.f_code is target.values.__code__
                assert created.close() is None
                assert events == []
                if not optimized:
                    assert created.gi_frame is None
                assert created.gi_state == "GEN_CLOSED"
                assert created.gi_code is target.values.__code__

                first = target.make(events, 31)
                second = target.make(events, 41)
                assert type(first) is type(second) is types.GeneratorType
                assert first is not second
                assert first.gi_code is second.gi_code is target.values.__code__
                assert next(first) == 31 and next(second) == 41
                assert first.gi_suspended and second.gi_suspended
                assert not first.gi_running and not second.gi_running
                assert first.gi_yieldfrom is None
                if not optimized:
                    assert first.gi_frame.f_locals["marker"] == 31
                with pytest.raises(StopIteration) as completed:
                    next(first)
                assert completed.value.value == 32
                if not optimized:
                    assert first.gi_frame is None
                assert first.gi_state == "GEN_CLOSED"
                assert first.gi_code is target.values.__code__
                assert second.close() is None
                if not optimized:
                    assert second.gi_frame is None
                assert second.gi_state == "GEN_CLOSED"
                return events

            expected = exercise(ordinary, False)
            assert expected == [
                ("entered", 31), ("entered", 41),
                ("released", 31), ("released", 41),
            ]
            assert exercise(module, True) == expected
        """),
        managed_generator_project.project / "managed_generator_materialization.py",
        entry_interpreter=entry_interpreter,
        required_functions=("make",),
    )
