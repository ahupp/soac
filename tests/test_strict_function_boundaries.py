"""Authenticated function execution with ordinary calls and protected storage."""

import json
import textwrap
from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project

_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from typing import Any, cast, final
from support import events, marker, observe

def identity(value: int) -> int:
    return value

first_lambda, second_lambda = (lambda value: value), (lambda value: value + 1)

def widened(value: float) -> float:
    return value

def optional(value: int | str | None) -> int | str | None:
    return value

def shape(first: int, /, second: int = 2, *items: int,
          named: str | None = None, **extras: int) -> int:
    return first + second

def caller(value: Any) -> int:
    return identity(value)

def bad_return(value: Any) -> int:
    return cast(int, value)

def finish_with_result(factory, observer, result: Any) -> int:
    payload = factory()
    try:
        raise LookupError("source handler")
    except LookupError:
        observer("body")
        return cast(int, result)

def raises(value: int) -> int:
    raise LookupError("body wins")

def annotation_trap(format: int):
    events.append("annotation evaluated")
    raise AssertionError("annotation provider must never be called by a boundary")

identity.__annotate__ = annotation_trap

def active_default(value=marker("active-old")) -> None:
    active_default.__defaults__ = (marker("active-new"),)
    observe(value)

active_default()
events.append("after-active")

def idle_default(value=marker("idle-old")):
    return value

idle_default.__defaults__ = (marker("idle-new"),)
events.append("after-idle")

def make_cycle():
    captured = []
    def inner(value: int) -> int:
        return value + len(captured)
    captured.append(inner)
    return inner

class StoppingIterator:
    def __next__(self):
        raise StopIteration

def catch_stop(iterator, observer):
    try:
        return next(iterator)
    except StopIteration:
        return observer()

class ReturningIterator:
    def __next__(self):
        try:
            raise LookupError("callee handler")
        except LookupError:
            return 7

def replace_result(iterator, create):
    value = create()
    value = next(iterator)
    return value
"""

_SUPPORT = """
events = []

class Marker:
    def __init__(self, name):
        self.name = name
    def __del__(self):
        events.append("drop:" + self.name)

def marker(name):
    return Marker(name)

def observe(value):
    events.append("use:" + value.name)
"""


@pytest.fixture(scope="module")
def strict_functions(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-functions"),
        {"checked.py": _SOURCE, "support.py": _SUPPORT},
        modules={"checked": "checked.py"},
    )


def test_profiled_stop_iteration_handler_preserves_implicit_exception_observers(
    strict_functions, tmp_path
):
    program = """
import sys
import checked
from soac import _soac_ext

assert _soac_ext.strict_function_entry_kind(checked.catch_stop) == 'checked_native'
iterator = checked.StoppingIterator()
marker = ValueError('outside handler')
for _ in range(200):
    try:
        raise marker
    except ValueError:
        caught = checked.catch_stop(iterator, sys.exception)
        assert type(caught) is StopIteration
        assert caught.__context__ is marker
        assert sys.exception() is marker
    assert sys.exception() is None
"""
    work = tmp_path / "stop-iteration-observation"
    for mode in ("profile", "apply", "verify"):
        strict_functions.run(
            program, opt_mode=mode, extra_env={"SOAC_WORK_DIR": str(work)}
        )
    assert (work / "profile.bin").is_file()
    assert (work / "verify.bin").is_file()


def test_profiled_result_replacement_preserves_caller_handler_and_cleanup(
    strict_functions, tmp_path
):
    program = """
import gc
import sys
import checked
from soac import _soac_ext

assert _soac_ext.strict_function_entry_kind(checked.replace_result) == 'checked_native'
seen = []
class Previous:
    def __del__(self):
        seen.append("drop")

iterator = checked.ReturningIterator()
marker = ValueError('caller handler')
for _ in range(200):
    try:
        raise marker
    except ValueError:
        assert checked.replace_result(iterator, Previous) == 7
        assert sys.exception() is marker
    assert sys.exception() is None
gc.collect()
assert seen == ["drop"] * 200
"""
    work = tmp_path / "inline-result-replacement"
    for mode in ("profile", "apply", "verify"):
        strict_functions.run(
            program, opt_mode=mode, extra_env={"SOAC_WORK_DIR": str(work)}
        )
    assert (work / "profile.bin").is_file()
    assert (work / "verify.bin").is_file()


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_actual_public_binder_preserves_annotation_only_values_and_errors(
    strict_functions, entry_interpreter
):
    strict_functions.run(
        """
        import checked
        from support import events

        def rejected(call, contains=None):
            try:
                call()
            except TypeError as error:
                if contains is not None:
                    assert contains in str(error), str(error)
            else:
                raise AssertionError("ordinary argument binding accepted an invalid call")

        assert checked.identity(True) is True
        value = int("12345678901234567890")
        assert checked.identity(value) is value
        assert checked.widened(value) is value
        assert checked.optional(None) is None
        assert checked.optional("ok") == "ok"
        assert checked.shape(3, 4, 5, 6, named=None, extra=7) == 7
        assert checked.shape(3) == 5
        assert checked.identity("bad") == "bad"
        outside = []
        assert checked.optional(outside) is outside
        assert checked.shape(1, 2, "bad") == 3
        assert checked.shape(1, extra="bad") == 3
        rejected(lambda: checked.shape("bad", 2, second=3), "multiple values")
        rejected(lambda: checked.identity("bad", unexpected=1), "unexpected keyword")
        rejected(lambda: checked.identity(), "missing")
        assert checked.bad_return(outside) is outside
        for number in range(30):
            assert checked.caller(number) == number
        assert checked.caller("bad") == "bad"
        try:
            checked.raises("bad")
        except LookupError as error:
            assert str(error) == "body wins"
        else:
            raise AssertionError("body exception was replaced or lost")
        assert "annotation evaluated" not in events
        print("ordinary-binders-and-annotation-only-values")
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_return_identity_and_body_errors_preserve_source_and_cleanup(
    strict_functions, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_functions.run(
        f"""
        import gc, sys, weakref
        import checked
        from soac import _soac_ext

        assert _soac_ext.strict_function_entry_kind(checked.finish_with_result) == {expected_entry!r}
        events = []
        references = []
        class Payload:
            def __del__(self):
                events.append("drop")
        def create():
            payload = Payload()
            references.append(weakref.ref(payload))
            return payload

        outer = ValueError("caller handler")
        try:
            raise outer
        except ValueError:
            assert checked.finish_with_result(create, events.append, 7) == 7
            gc.collect()
            assert events.count("body") == events.count("drop") == 1, events
            assert references[-1]() is None
            events.clear()

            result = object()
            assert checked.finish_with_result(create, events.append, result) is result
            gc.collect()
            assert references[-1]() is None
            assert events.count("body") == events.count("drop") == 1, events
            events.clear()

            original = RuntimeError("explicit observer failure")
            def fail_observer(stage):
                events.append(stage)
                raise original
            try:
                checked.finish_with_result(create, fail_observer, result)
            except RuntimeError as error:
                assert error is original
                assert isinstance(error.__context__, LookupError)
                assert str(error.__context__) == "source handler"
                error.__context__.__traceback__ = None
                error.__context__ = None
                error.__traceback__ = None
                gc.collect()
                assert references[-1]() is None
                assert events.count("body") == events.count("drop") == 1, events
            else:
                raise AssertionError("explicit observer exception was lost")
            assert sys.exception() is outer
        assert sys.exception() is None
        print("return-identity-body-error-and-cleanup")
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_unsealed_default_replacement_keeps_only_active_values_alive(
    strict_functions, entry_interpreter
):
    strict_functions.run(
        """
        import gc
        import checked
        from support import events
        gc.collect()
        assert [event for event in events if not event.startswith("drop:")] == [
            "use:active-old", "after-active", "after-idle",
        ], events
        assert sorted(event for event in events if event.startswith("drop:")) == [
            "drop:active-old", "drop:idle-old",
        ], events
        assert checked.active_default.__defaults__[0].name == "active-new"
        assert checked.idle_default().name == "idle-new"
        print("default-replacement-and-cleanup")
        """,
        entry_interpreter=entry_interpreter,
    )


def test_cpython_annotations_preserve_ordinary_binding_and_body_errors(
    tmp_path,
):
    project = create_strict_project(
        tmp_path,
        {"checked.py": _SOURCE, "support.py": _SUPPORT},
        modules={"checked": "checked.py"},
        backend="cpython",
    )
    project.run_case(
        "checked",
        """
import ctypes
from pathlib import Path
from types import ModuleType
import checked
from support import events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

# The ordinary binder control is exactly the analyzed source without opt-in;
# it does not borrow authenticated code objects or publish another contract.
ordinary = ModuleType("ordinary_disabled_boundaries")
source = Path(checked.__file__).read_text()
exec(compile(source.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\\n"),
             "<ordinary-disabled-boundaries>", "exec", dont_inherit=True),
     vars(ordinary))

call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object

def python_call(function, args, keywords):
    return function(*args, **keywords)

def error_from(operation, expected_type, expected_args=None):
    try:
        operation()
    except Exception as error:
        # StrictMutationError (a TypeError subclass) is not a value-check result.
        assert type(error) is expected_type, (type(error), error)
        if expected_args is not None:
            assert error.args == expected_args, error.args
        return error.args
    raise AssertionError("required failure was skipped")

def exercise(invoke):
    assert invoke(checked.identity, (7,), {}) == 7
    assert invoke(checked.optional, (None,), {}) is None
    assert invoke(checked.shape, (3,), {}) == 5
    error_from(lambda: invoke(checked.raises, (1,), {}),
               LookupError, ("body wins",))
    error_from(lambda: invoke(checked.raises, ("bad",), {}),
               LookupError, ("body wins",))

    result = object()
    assert invoke(checked.bad_return, (result,), {}) is result
    assert invoke(checked.shape, (1, 2, "bad"),
                  {"named": [], "extra": "bad"}) == 3
    assert invoke(checked.identity, (result,), {}) is result
    value = []
    assert invoke(checked.optional, (value,), {}) is value

    # Ordinary argument binding still precedes the body. An annotation does
    # not change the native error's kind, arguments or priority.
    for name, args, keywords in (
        ("identity", (), {}),
        ("identity", ("bad", 2), {}),
        ("identity", ("bad",), {"unexpected": 1}),
        ("shape", ("bad", 2), {"second": 3}),
    ):
        expected = error_from(
            lambda: invoke(getattr(ordinary, name), args, keywords), TypeError)
        assert error_from(
            lambda: invoke(getattr(checked, name), args, keywords), TypeError
        ) == expected

exercise(python_call)
for value in range(128):
    assert checked.identity(value) == value
    assert checked.shape(value) == value + 2
exercise(python_call)
exercise(call)
assert "annotation evaluated" not in events
diagnostic = _soac_ext.strict_module_diagnostics(checked)
for function in (checked.identity, checked.shape, checked.raises):
    observed = _assert_cpython_function_witness(
        function, diagnostic)
    assert observed["original_code_entered"] is True
_assert_cpython_function_witness(
    checked.bad_return, diagnostic)
""",
        Path(__file__),
        required_functions=("identity", "optional", "shape", "raises",
                            "bad_return", "idle_default"),
        
        backend="cpython",
    )


def test_cpython_callable_signature_facts_do_not_enforce_union_values(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "checked.py": _SOURCE + """
def mixed_union(value: int | list[int]) -> int | list[int]:
    return value
""",
            "support.py": _SUPPORT,
        },
        modules={"checked": "checked.py"},
        backend="cpython",
    )
    # Inspect the actual checker's structured output, not a manually supplied
    # contract. run_case below independently authenticates this publication.
    artifact = Path(project.publication["artifact_directory"])
    manifest = json.loads((artifact / "manifest.json").read_text())["manifest"]
    index, = [
        item for item in manifest["modules"]
        if item["module"]["module_name"] == "checked"
    ]
    shard = json.loads(
        (artifact / "modules" / f'{index["shard_digest"]}.soac-types').read_text()
    )
    mixed, = [
        item for item in shard["functions"]
        if item["identity"]["lexical_qualname"] == "mixed_union"
    ]
    signature = mixed["signature"]
    parameter, = signature["parameters"]
    assert parameter["annotation_origin"] == "explicit"
    assert signature["return_annotation_origin"] == "explicit"
    for value_type in (parameter["value_type"], signature["return_type"]):
        assert value_type["kind"] == "union", value_type
        assert len(value_type["data"]) == 2, value_type
        assert {
            "kind": "nominal_builtin",
            "data": {"builtin": "int", "allow_subclasses": True},
        } in value_type["data"], value_type
        assert {
            "kind": "unsupported",
            "data": {"kind": "mutable_generic", "reason": "no_runtime_enforcement"},
        } in value_type["data"], value_type

    project.run_case(
        "checked",
        """
import ctypes
import checked
from support import events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

one = ctypes.pythonapi.PyObject_CallOneArg
one.argtypes = [ctypes.py_object, ctypes.py_object]
one.restype = ctypes.py_object

def exercise(invoke):
    # Static signature facts remain available without turning either union
    # into a runtime predicate.
    for value in (7, [1], ["not an int"], "outside either arm", object()):
        assert invoke(checked.mixed_union, value) is value
    original = [1]
    assert invoke(checked.mixed_union, original) is original
    original.append(object())
    assert invoke(checked.mixed_union, original) is original

    assert invoke(checked.optional, "supported") == "supported"
    assert invoke(checked.optional, None) is None
    outside = []
    assert invoke(checked.optional, outside) is outside

def python_call(function, value):
    return function(value)

exercise(python_call)
for value in range(128):
    assert checked.mixed_union(value) == value
exercise(python_call)
exercise(one)
assert "annotation evaluated" not in events
diagnostic = _soac_ext.strict_module_diagnostics(checked)
observed = _assert_cpython_function_witness(
    checked.mixed_union, diagnostic)
assert observed["original_code_entered"] is True
_assert_cpython_function_witness(
    checked.optional, diagnostic)
""",
        Path(__file__),
        required_functions=("mixed_union", "optional", "identity"),
        
        backend="cpython",
    )


def test_owner_is_opaque_cycles_collect_and_public_ids_cannot_forge_functions(
    strict_functions,
):
    strict_functions.run(
        """
        import ctypes, gc, weakref
        import checked
        from soac.strict import StrictRuntimeUnavailableError

        owners = [value for value in gc.get_referents(checked.identity)
                  if type(value).__name__ == "_StrictFunctionOwner"]
        assert len(owners) == 1
        for operation in [lambda: type(owners[0])(),
                          lambda: setattr(type(owners[0]), "mutable", True),
                          lambda: setattr(owners[0], "mutable", True)]:
            try:
                operation()
            except (TypeError, AttributeError):
                pass
            else:
                raise AssertionError("opaque source authority was mutable")

        inner = checked.make_cycle()
        assert inner(3) == 4
        reference = weakref.ref(inner)
        del inner
        gc.collect()
        assert reference() is None, "hidden environment edges retained a closure cycle"

        # Trusted native test introspection, not a production authority path:
        # an ID readable from implementation metadata must not be a capability.
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        class Prefix(ctypes.Structure):
            _fields_ = [("environment", ctypes.c_void_p), ("function_id", ctypes.c_uint64)]
        pointer = metadata(checked.identity)
        assert pointer
        identifier = Prefix.from_address(pointer).function_id
        try:
            _soac_ext.make_function(identifier, "function", (), (), module_globals=checked.__dict__)
        except StrictRuntimeUnavailableError as error:
            assert "Python-supplied function IDs" in str(error), str(error)
        else:
            raise AssertionError("a public integer forged strict source provenance")
        print("opaque-owner-and-cycle")
        """
    )


def test_native_source_matching_handles_decorators_and_same_line_lambdas(tmp_path):
    source = (
        _SOURCE
        + """
class Decorated:
    @final
    def decorated(self, value: int) -> int:
        return value + 1

decorated = Decorated().decorated
"""
    )
    project = create_strict_project(
        tmp_path,
        {"checked.py": source, "support.py": _SUPPORT},
        modules={"checked": "checked.py"},
    )
    project.run(
        """
        import ctypes, types
        import checked
        from soac.strict import StrictRuntimeUnavailableError
        assert checked.decorated(2) == 3
        assert checked.first_lambda(2) == 2
        assert checked.second_lambda(2) == 3
        assert checked.first_lambda.__code__ is not checked.second_lambda.__code__
        assert checked.first_lambda.__code__.co_firstlineno == checked.second_lambda.__code__.co_firstlineno
        source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
        source_id.argtypes = [ctypes.py_object]
        source_id.restype = ctypes.c_uint64
        identities = {source_id(function.__code__) for function in
                      [checked.identity, checked.decorated, checked.first_lambda, checked.second_lambda]}
        assert len(identities) == 1 and 0 not in identities
        clone = types.FunctionType(checked.identity.__code__, checked.identity.__globals__)
        try:
            clone(1)
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError("a code object alone became strict entry authority")
        print("authenticated-native-source-tree")
        """
    )


_LAMBDA_SCOPE_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

module_list = [lambda: index for index in range(3)]
module_set = {lambda: index for index in range(3)}
module_dict = {index: (lambda: index) for index in range(3)}
module_generator = (lambda: index for index in range(3))
generator_input = (item for item in (lambda: range(3))())
module_nested = lambda: (lambda: "nested")

class Owner:
    values = [lambda: index for index in range(3)]
    generated = (lambda: index for index in range(3))
    nested = [lambda: (lambda: "class-nested")]

def factory():
    local = [lambda: index for index in range(3)]
    class Local:
        def method(self):
            super()
            return __class__
        values = [lambda: index for index in range(3)]
        generated = (lambda: index for index in range(3))
        nested = [lambda: (lambda: "local-nested")]
    return local, Local
"""

_LAMBDA_DEFAULT_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

events = []
def mark(name, value):
    events.append(name)
    return value

module_lambda = lambda positional=mark("positional", lambda: 7), /, *, keyword=mark("keyword", lambda: 9): (positional(), keyword())

def factory(value):
    result = lambda callback=(lambda: value): callback()
    value += 1
    return result
"""


def _lambda_scopes_project(root, *, backend="soac"):
    sources = {
        "lambdas.py": _LAMBDA_SCOPE_SOURCE,
        "defaults.py": _LAMBDA_DEFAULT_SOURCE,
    }
    for name, source in tuple(sources.items()):
        # Keep native line numbers equal while the control remains ordinary.
        sources[f"ordinary_{name}"] = source.replace(
            "# soac: module(strict_assign=true, checked_attr=true)", "# ordinary source control", 1
        )
    return create_strict_project(
        root,
        sources,
        modules={"lambdas": "lambdas.py", "defaults": "defaults.py"},
        backend=backend,
    )


@pytest.fixture(scope="module")
def strict_lambda_scopes(tmp_path_factory):
    return _lambda_scopes_project(tmp_path_factory.mktemp("strict-lambda-scopes"))


@pytest.fixture(scope="module")
def cpython_lambda_scopes(tmp_path_factory):
    return _lambda_scopes_project(
        tmp_path_factory.mktemp("strict-cpython-lambda-scopes"), backend="cpython",
    )


def _lambda_scope_validation(expected_entry):
    return textwrap.dedent(f"""
        def validate(module):
            import ctypes
            import ordinary_lambdas
            from soac import _soac_ext

            source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
            source_id.argtypes = [ctypes.py_object]
            source_id.restype = ctypes.c_uint64

            def observations(mod, strict):
                local, cls = mod.factory()
                assert cls().method() is cls
                assert list(mod.generator_input) == [0, 1, 2]
                functions = [
                    *mod.module_list, *mod.module_set, *mod.module_dict.values(),
                    *mod.module_generator, mod.module_nested(),
                    *mod.Owner.values, *mod.Owner.generated, mod.Owner.nested[0](),
                    *local, *cls.values, *cls.generated, cls.nested[0](),
                ]
                expected = {expected_entry!r} if strict else None
                for function in functions:
                    assert _soac_ext.strict_function_entry_kind(function) == expected
                    assert bool(source_id(function.__code__)) is strict
                if strict:
                    assert len({{source_id(function.__code__) for function in functions}}) == 1
                result = [
                    (function.__qualname__, function.__code__.co_qualname,
                     function.__code__.co_firstlineno, function.__code__.co_freevars,
                     function())
                    for function in functions
                ]
                for function in functions:
                    assert _soac_ext.strict_function_entry_kind(function) == expected
                return result

            assert observations(module, True) == observations(ordinary_lambdas, False)
        """)


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_lambda_class_comprehensions_preserve_source_identities_and_scopes(
    strict_lambda_scopes, entry_interpreter,
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_lambda_scopes.run_case(
        "lambdas", _lambda_scope_validation(expected_entry),
        strict_lambda_scopes.project / "lambdas.py",
        required_functions=("factory",),
        entry_interpreter=entry_interpreter,
    )

def test_cpython_lambda_source_identities_match_native_comprehension_scopes(
    cpython_lambda_scopes,
):
    # Preserve the original source and its full ordinary comparison. The
    # authenticated native consumer owns these original code objects; it never
    # needs the retained class projection or SOAC generated code.
    cpython_lambda_scopes.run_case(
        "lambdas", _lambda_scope_validation("original_code"),
        cpython_lambda_scopes.project / "lambdas.py",
        required_functions=("factory",), 
        backend="cpython",
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_lambda_defaults_are_created_in_the_enclosing_execution(
    strict_lambda_scopes, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_lambda_scopes.run_case(
        "defaults",
        textwrap.dedent(f"""
        def validate(module):
            import ordinary_defaults
            from soac import _soac_ext

            def observations(mod, strict):
                assert mod.events == ["positional", "keyword"]
                assert mod.module_lambda() == (7, 9)
                first = mod.factory(20)
                second = mod.factory(40)
                assert first() == 21 and second() == 41
                functions = [
                    mod.module_lambda, mod.module_lambda.__defaults__[0],
                    mod.module_lambda.__kwdefaults__["keyword"],
                    first, first.__defaults__[0], second, second.__defaults__[0],
                ]
                expected = {expected_entry!r} if strict else None
                for function in functions:
                    assert _soac_ext.strict_function_entry_kind(function) == expected
                return [
                    (function.__qualname__, function.__code__.co_qualname,
                     function.__code__.co_firstlineno, function.__code__.co_freevars)
                    for function in functions
                ]

            assert observations(module, True) == observations(ordinary_defaults, False)
        """),
        strict_lambda_scopes.project / "defaults.py",
        entry_interpreter=entry_interpreter,
        required_functions=("factory", "module_lambda"),
    )


_CAPTURED_GENERATOR_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from support import Payload, events

def captured(reason):
    value = Payload(reason)
    try:
        yield lambda: value
    finally:
        events.append("finished:" + reason)

def deleted():
    value = Payload("explicit")
    yield lambda: value
    del value
    yield None
"""


@pytest.fixture(scope="module")
def strict_captured_generators(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-captured-generators"),
        {
            "captured.py": _CAPTURED_GENERATOR_SOURCE,
            "ordinary_captured.py": _CAPTURED_GENERATOR_SOURCE.replace(
                "# soac: module(strict_assign=true, checked_attr=true)", "# ordinary source control", 1
            ),
            "support.py": """
events = []

class Payload:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append("released:" + self.name)
""",
        },
        modules={"captured": "captured.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generator_termination_releases_its_cell_not_escaped_contents(
    strict_captured_generators, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_captured_generators.run_case(
        "captured",
        textwrap.dedent(f"""
        def validate(module):
            import gc
            import weakref
            import ordinary_captured
            from soac import _soac_ext
            from support import events

            def observations(mod, expected):
                events.clear()
                for reason in ("exhausted", "closed", "thrown"):
                    frame = mod.captured(reason)
                    callback = next(frame)
                    assert _soac_ext.strict_function_entry_kind(callback) == expected
                    reference = weakref.ref(callback())
                    if reason == "exhausted":
                        assert next(frame, "done") == "done"
                    elif reason == "closed":
                        assert frame.close() is None
                    else:
                        error = LookupError("original exception")
                        try:
                            frame.throw(error)
                        except LookupError as actual:
                            assert actual is error
                        else:
                            raise AssertionError("generator swallowed the exception")
                        # Retained exception tracebacks legitimately keep the
                        # completed ordinary generator frame and its locals.
                        del error
                    assert reference() is not None
                    assert callback() is reference()
                    assert callback().name == reason
                    assert _soac_ext.strict_function_entry_kind(callback) == expected
                    assert events[-1] == "finished:" + reason
                    del callback
                    gc.collect()
                    assert reference() is None, (reason, expected, "finished frame retained an owned cell")
                    assert events[-1] == "released:" + reason
                    # Keep the completed generator alive until after the value
                    # dies: clearing its ownership must not await deallocation.
                    assert next(frame, "done") == "done"

                frame = mod.deleted()
                callback = next(frame)
                assert _soac_ext.strict_function_entry_kind(callback) == expected
                reference = weakref.ref(callback())
                assert next(frame) is None
                try:
                    callback()
                except NameError:
                    pass
                else:
                    raise AssertionError("source del did not empty the shared cell")
                assert next(frame, "done") == "done"
                gc.collect()
                assert reference() is None
                return list(events)

            ordinary = observations(ordinary_captured, None)
            assert observations(module, {expected_entry!r}) == ordinary
        """),
        strict_captured_generators.project / "captured.py",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("backend", "entry_interpreter"),
    [pytest.param("soac", False, id="False"),
     pytest.param("soac", True, id="True"),
     pytest.param("cpython", False, id="cpython")],
)
def test_strict_suspended_frames_keep_original_cells_and_execution_timing(
    tmp_path, backend, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "support.py": """
import ctypes
events = []

class Pause:
    def __await__(self):
        yield "paused"
        return None

def pause():
    return Pause()

def capture_frames(factory):
    function = factory()
    first = function(1)
    def cell(value):
        return (lambda: value).__closure__[0]
    replace = ctypes.pythonapi.PyFunction_SetClosure
    replace.argtypes = [ctypes.py_object, ctypes.py_object]
    replace.restype = ctypes.c_int
    assert len(function.__closure__) == 1
    assert replace(function, (cell(30),)) == 0
    second = function(2)
    return first, second
""",
            "suspended.py": """
# soac: module(strict_assign=true, checked_attr=true)
from support import capture_frames, events, pause

def make_frames():
    captured = 10
    def numbers(delta: int):
        events.append("number-entered")
        yield captured + delta
        events.append("number-resumed")
        yield captured + delta + 1
    return numbers

old_frame, new_frame = capture_frames(make_frames)

def make_async(base):
    async def compute(value: int):
        events.append("coroutine-entered")
        await pause()
        return base + value
    async def stream(value: int):
        events.append("async-generator-entered")
        yield base + value
        yield base + value + 1
    return compute, stream

def four(first: int, second: int, third: int, fourth: int) -> int:
    return first + second + third + fourth
""",
        },
        modules={"suspended": "suspended.py"},
        backend=backend,
    )
    program = f"CPYTHON_BACKEND = {backend == 'cpython'!r}\n" + """
import asyncio
import suspended
from support import events
from soac.strict import StrictRuntimeUnavailableError
assert events == [], events
assert next(suspended.old_frame) == 11
assert next(suspended.new_frame) == 32
assert next(suspended.old_frame) == 12
assert next(suspended.new_frame) == 33
assert next(suspended.old_frame, "done") == "done"
assert next(suspended.new_frame, "done") == "done"
assert events == ["number-entered", "number-entered", "number-resumed", "number-resumed"]

compute, stream = suspended.make_async(100)
coroutine = compute(2)
async_generator = stream(3)
assert len(events) == 4, "suspended bodies ran at object creation"
assert coroutine.send(None) == "paused"
try:
    coroutine.send(None)
except StopIteration as completed:
    assert completed.value == 102
else:
    raise AssertionError("coroutine completion was lost")
async def consume():
    return [value async for value in async_generator]
assert asyncio.run(consume()) == [103, 104]
assert events[-2:] == ["coroutine-entered", "async-generator-entered"]

# The synchronous mandatory subset must not be applied to suspended function
# annotations at object creation, or to the internal resume-control operands.
pending = compute("bad")
assert events[-1] == "async-generator-entered"
pending.close()
# This helper takes a retained resume implementation and preserved-state
# capsule, not a source function's ordinary arguments. Test malformed input
# separately so it cannot stand in for the strict wrong-role barrier.
import ctypes
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p
if CPYTHON_BACKEND:
    assert metadata(suspended.four) is None
else:
    assert metadata(suspended.four) is not None

def rejected_resume(state, expected_error, expected_message):
    before_probe = tuple(events)
    try:
        _soac_ext.resume_generator(suspended.four, object(), state, None, None)
    except expected_error as error:
        assert type(error) is expected_error, (type(error), error)
        assert str(error) == expected_message, str(error)
    else:
        raise AssertionError("public resume control ABI entered a synchronous body")
    assert tuple(events) == before_probe, events

if CPYTHON_BACKEND:
    rejected_resume(object(), RuntimeError, "missing CLIF vectorcall metadata")
else:
    rejected_resume(
        object(), ValueError,
        "PyCapsule_GetPointer called with invalid PyCapsule object")

# Existing public construction creates a valid empty, unmanaged state. It
# contains no source snapshot, generator identity or permission to run a body.
empty_state = _soac_ext.make_preserved_state((), (), [])
if CPYTHON_BACKEND:
    # Native source witnesses and ordinary binders below own this backend's
    # checks; a retained-only API cannot invent a JIT owner for the function.
    rejected_resume(empty_state, RuntimeError, "missing CLIF vectorcall metadata")
    assert metadata(suspended.four) is None
else:
    rejected_resume(
        empty_state, StrictRuntimeUnavailableError,
        "strict resume entry requires an authenticated generator or coroutine body")
print("owned-suspended-frames")
"""
    if backend == "soac":
        project.run(program, entry_interpreter=entry_interpreter)
        return

    project.run_case(
        "suspended",
        "from soac import _soac_ext\n" + program + """
import ctypes
from pathlib import Path
from types import ModuleType, GeneratorType, CoroutineType, AsyncGeneratorType
from tests._strict_integration import _assert_cpython_function_witness

# Compile only the ordinary reference, without source opt-in or inherited
# flags. The strict functions below retain their genuine original native code.
ordinary = ModuleType("ordinary_suspended_binders")
source = Path(suspended.__file__).read_text()
exec(compile(source.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\\n"),
             "<ordinary-suspended-binders>", "exec", dont_inherit=True),
     vars(ordinary))
ordinary.old_frame.close()
ordinary.new_frame.close()
numbers = suspended.make_frames()
compute, stream = suspended.make_async(100)
plain_compute, plain_stream = ordinary.make_async(100)
pairs = (
    (numbers, ordinary.make_frames(), GeneratorType, "gi_frame", "delta"),
    (compute, plain_compute, CoroutineType, "cr_frame", "value"),
    (stream, plain_stream, AsyncGeneratorType, "ag_frame", "value"),
)
call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object

def python_call(function, args, keywords):
    return function(*args, **keywords)

def close_unstarted(value, kind, frame_name):
    assert type(value) is kind, type(value)
    if kind is AsyncGeneratorType:
        pending_close = value.aclose()
        try:
            pending_close.send(None)
        except StopIteration as stopped:
            assert stopped.value is None
        else:
            raise AssertionError("unstarted async generator close did not complete")
    else:
        assert value.close() is None
    assert getattr(value, frame_name) is None

def binding_error(invoke, function, args, keywords):
    try:
        invoke(function, args, keywords)
    except TypeError as error:
        assert type(error) is TypeError, (type(error), error)
        return error.args
    raise AssertionError("invalid ordinary binding returned a suspended object")

diagnostic = _soac_ext.strict_module_diagnostics(suspended)
before = tuple(events)
for function, control, kind, frame_name, argument in pairs:
    original_code = function.__code__
    _assert_cpython_function_witness(function, diagnostic)
    assert _soac_ext.strict_function_diagnostics(control) is None
    for invoke in (python_call, call):
        for args, keywords in (
            ((), {}), ((1, 2), {}), ((1,), {"unexpected": 2}),
            ((1,), {argument: 2}),
        ):
            expected = binding_error(invoke, control, args, keywords)
            assert binding_error(invoke, function, args, keywords) == expected
            assert tuple(events) == before

        # The annotation is int, but object creation uses ordinary binding,
        # not the synchronous selected-value predicate or body execution.
        for value in (1, "bad"):
            close_unstarted(invoke(control, (value,), {}), kind, frame_name)
            close_unstarted(invoke(function, (value,), {}), kind, frame_name)
            assert tuple(events) == before
    for index in range(128):
        close_unstarted(python_call(function, ("bad",), {}), kind, frame_name)
    assert tuple(events) == before
    assert function.__code__ is original_code
    _assert_cpython_function_witness(function, diagnostic)

assert suspended.four(1, 2, 3, 4) == 10
# Keep the same final source/function/zero-compilation witnesses as cold entry;
# a suspended object's existence alone never supplies strict source authority.
""",
        Path(__file__),
        required_functions=("make_frames", "make_async", "four"),
        
        backend="cpython",
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("same_code", [False, True])
def test_code_replacement_during_initialization_preserves_actual_body_semantics(
    tmp_path, same_code, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "support.py": """
from soac.strict import StrictMutationError

events = []

def replacement(value):
    return "not an int"

def patch(function, same_code):
    original = function.__code__
    try:
        function.__code__ = original if same_code else replacement.__code__
    except StrictMutationError:
        assert function.__code__ is original
        events.append("rejected before mutation")
    else:
        events.append("original" if same_code else "replaced")
    for value in ("bad argument", 1):
        result = function(value)
        if function.__code__ is original:
            assert result is value
        else:
            assert result == "not an int"
        events.append(("body", result))
""",
            "checked_patch.py": f"""
# soac: module(strict_assign=true, checked_attr=true)
from support import patch

def checked(value: int) -> int:
    return value

patch(checked, {same_code!r})
""",
        },
        modules={"checked_patch": "checked_patch.py"},
    )
    project.run(
        f"same_code = {same_code!r}\n"
        f"expected_entry_kind = {('entry_interpreter' if entry_interpreter else 'checked_native')!r}\n"
        + """
import sys
from support import events
from soac.strict import StrictMutationError, StrictRuntimeUnavailableError

if same_code:
    import checked_patch
    # No annotation-driven early freeze may reject the same-code assignment.
    assert events == ["original", ("body", "bad argument"), ("body", 1)]
    assert _soac_ext.strict_module_diagnostics(checked_patch)["sealed"]
    function = checked_patch.checked
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry_kind

    import ctypes
    get_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    get_owner.argtypes = [ctypes.py_object]
    get_owner.restype = ctypes.c_void_p
    get_seal = ctypes.pythonapi.PyFunction_GetSoacStrictId
    get_seal.argtypes = [ctypes.py_object]
    get_seal.restype = ctypes.c_uint64
    get_source = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
    get_source.argtypes = [ctypes.py_object]
    get_source.restype = ctypes.c_uint64
    assert get_owner(function) and get_seal(function)
    code = function.__code__
    assert get_source(code) > 0
    marker = object()
    assert function(marker) is marker  # No argument or return-type check.
    try:
        function.__code__ = code
    except StrictMutationError:
        pass
    else:
        raise AssertionError("successfully sealed source metadata stayed mutable")
    assert function.__code__ is code
    assert function("after seal") == "after seal"
else:
    # Ordinary replacement code can run before freezing, but it cannot receive
    # the original source contract at the independent module-sealing boundary.
    try:
        import checked_patch
    except StrictRuntimeUnavailableError as error:
        assert "strict function native metadata changed" in str(error)
    else:
        raise AssertionError("retained replacement acquired the original source contract")
    assert events == ["replaced", ("body", "not an int"), ("body", "not an int")]
    assert "checked_patch" not in sys.modules
print("preseal-body-and-source-sealing")
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_keyword_defaults_match_native_lookup_order_errors_and_lifetimes(
    tmp_path, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "probe.py": """
events = []
results = []

def stock(first: int = 1, *, left: int = 2, right: int = 3) -> int:
    return first + left + right

def stock_lifetime(*, value):
    replace_then_observe(stock_lifetime, value)

def stock_factory():
    value = 10
    def captured(*, left: int = 1) -> int:
        return value + left
    return captured

class Marker:
    def __init__(self, name):
        self.name = name
    def __del__(self):
        events.append("drop:" + self.name)

def replace_then_observe(function, value):
    function.__kwdefaults__ = {"value": None}
    events.append("use:" + value.name)

def scenarios(function, lifetime):
    import gc
    saved = function.__kwdefaults__
    saved_lifetime = lifetime.__kwdefaults__
    observed = {}

    def attempt(name, call):
        events.clear()
        try:
            value = call()
        except BaseException as error:
            value = (type(error).__name__, str(error) if isinstance(error, LookupError) else None)
        observed[name] = (value, tuple(events))

    class RaisingKey:
        def __hash__(self):
            return hash("left")
        def __eq__(self, other):
            events.append("lookup:left")
            raise LookupError("default-key equality failed")

    events.clear()
    function.__kwdefaults__ = {RaisingKey(): 7, "right": 3}
    assert events == [], "assigning kwdefaults must not look up parameter keys"
    attempt("provided", lambda: function(left=4, right=5))
    attempt("duplicate", lambda: function(1, first=2, left=4, right=5))
    attempt("unexpected", lambda: function(extra=1))
    attempt("lookup-error", lambda: function(right=5))

    class RaisingLaterKey:
        def __hash__(self):
            return hash("right")
        def __eq__(self, other):
            events.append("lookup:right")
            raise LookupError("later default lookup failed")

    function.__kwdefaults__ = {RaisingLaterKey(): 3}
    attempt("missing-before-later-error", lambda: function())

    class ReplacingKey:
        def __hash__(self):
            return hash("left")
        def __eq__(self, other):
            events.append("replace:left")
            function.__kwdefaults__ = {"left": 11, "right": 20}
            return other == "left"

    # Retain this dictionary to isolate lookup order from the lifetime case:
    # each missing parameter must observe the then-current function metadata.
    original = {ReplacingKey(): 7, "right": 3}
    function.__kwdefaults__ = original
    attempt("replaced", lambda: function())
    function.__kwdefaults__ = saved

    events.clear()
    lifetime.__kwdefaults__ = {"value": Marker("old-keyword")}
    lifetime()
    events.append("after-call")
    gc.collect()
    observed["lifetime"] = tuple(events)
    lifetime.__kwdefaults__ = saved_lifetime
    return observed

def closure_scenario(factory):
    from ctypes import c_int, py_object, pythonapi
    set_closure = pythonapi.PyFunction_SetClosure
    set_closure.argtypes = [py_object, py_object]
    set_closure.restype = c_int
    function = factory()
    value = 30
    replacement = (lambda: value).__closure__
    class ClosureKey:
        def __hash__(self):
            return hash("left")
        def __eq__(self, other):
            events.append("replace:closure")
            assert set_closure(function, replacement) == 0
            return other == "left"
    original = {ClosureKey(): 1}
    function.__kwdefaults__ = original
    events.clear()
    return function(), tuple(events)

def compare(function, lifetime, factory):
    expected = scenarios(stock, stock_lifetime)
    assert expected["provided"] == (10, ())
    assert expected["duplicate"] == (("TypeError", None), ())
    assert expected["unexpected"] == (("TypeError", None), ())
    assert expected["lookup-error"] == (("LookupError", "default-key equality failed"), ("lookup:left",))
    assert expected["missing-before-later-error"] == (("LookupError", "later default lookup failed"), ("lookup:right",))
    assert expected["replaced"] == (28, ("replace:left",))
    assert expected["lifetime"] == ("use:old-keyword", "drop:old-keyword", "after-call")
    actual = scenarios(function, lifetime)
    actual_lifetime = actual.pop("lifetime")
    expected_lifetime = expected.pop("lifetime")
    assert actual == expected, (actual, expected)
    assert tuple(event for event in actual_lifetime if not event.startswith("drop:")) == (
        "use:old-keyword", "after-call",
    ), actual_lifetime
    assert actual_lifetime.count("drop:old-keyword") == 1, actual_lifetime
    actual["lifetime"] = actual_lifetime
    expected["lifetime"] = expected_lifetime
    expected_closure = closure_scenario(stock_factory)
    assert expected_closure == (31, ("replace:closure",))
    assert closure_scenario(factory) == expected_closure
    results.append(actual)
""",
            "checked_defaults.py": """
# soac: module(strict_assign=true, checked_attr=true)
import probe

def checked(first: int = 1, *, left: int = 2, right: int = 3) -> int:
    return first + left + right

def lifetime(*, value):
    probe.replace_then_observe(lifetime, value)

def factory():
    value = 10
    def captured(*, left: int = 1) -> int:
        return value + left
    return captured

probe.compare(checked, lifetime, factory)
""",
        },
        modules={"checked_defaults": "checked_defaults.py"},
    )
    project.run(
        """
import checked_defaults
import probe
assert len(probe.results) == 1
assert checked_defaults.checked() == 6
print("native-keyword-default-order")
""",
        entry_interpreter=entry_interpreter,
    )


def test_annotation_providers_follow_actual_function_adoption_without_retention(
    tmp_path,
):
    project = create_strict_project(
        tmp_path,
        {
            "provider_probe.py": """
import weakref

events = []
retained = []
released_providers = []

def foreign(format):
    raise AssertionError("annotation provider must not be evaluated by adoption")

def replacement(format):
    return {}

def retain_and_replace(function):
    retained.append(function.__annotate__)
    function.__annotate__ = foreign

def replace_and_observe_release(function):
    reference = weakref.ref(function.__annotate__, lambda _: events.append("provider released"))
    released_providers.append(reference)
    function.__annotate__ = foreign
    events.append("after replacement")

def dynamic(function):
    # An unknown decorator cannot grant a frozen source contract, even when
    # this particular invocation returns its input unchanged.
    return function
""",
            "owned_annotations.py": """
# soac: module(strict_assign=true, checked_attr=true)
from provider_probe import dynamic, retain_and_replace, replace_and_observe_release

def owned(value: int) -> int:
    return value

def replaced(value: int) -> int:
    return value

retain_and_replace(replaced)

def released(value: int) -> int:
    return value

replace_and_observe_release(released)

@dynamic
def unsupported(value: int) -> int:
    return value
""",
        },
        modules={"owned_annotations": "owned_annotations.py"},
    )
    project.run(
        """
import gc
import owned_annotations as module
import provider_probe as probe

gc.collect()
assert all(reference() is None for reference in probe.released_providers)
assert sorted(probe.events) == ["after replacement", "provider released"], probe.events
assert module.owned(1) == 1
assert module.replaced(2) == 2
assert module.released(3) == 3

owned_provider = module.owned.__annotate__
try:
    owned_provider.__code__ = owned_provider.__code__
except TypeError:
    pass
else:
    raise AssertionError("the actually owned provider was not sealed with its function")

# None of these providers belongs to an adopted target now. Source helper
# provenance, a retained old pointer, or use as a replacement is not enough.
for provider in [probe.foreign, probe.retained[0], module.unsupported.__annotate__]:
    provider.__code__ = probe.replacement.__code__
    assert provider(1) == {}
assert module.replaced.__annotate__ is probe.foreign
assert module.released.__annotate__ is probe.foreign
print("annotation-provider-adoption")
        """
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_active_unannotated_frames_survive_code_changes_and_preserve_body_errors(
    tmp_path, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "active_probe.py": """
events = []
error = LookupError("the original body exception")

def dynamic(function):
    return function

def new_default(*, value):
    return ("new-default", value)

def new_body(value):
    return ("new-body", value)

def new_error():
    return "new-error"

def replace_body(function):
    function.__code__ = new_body.__code__

def replace_error(function):
    function.__code__ = new_error.__code__

def install_changing_default(function):
    class Key:
        def __hash__(self):
            return hash("value")
        def __eq__(self, other):
            events.append("default callback")
            function.__code__ = new_default.__code__
            return other == "value"
    function.__kwdefaults__ = {Key(): 7}
""",
            "active_frames.py": """
# soac: module(strict_assign=true, checked_attr=true)
from active_probe import dynamic, events, error, replace_body, replace_error

# Unknown decorators deliberately keep these functions outside frozen source
# contracts. Their authentic first calls still enter transformed source bodies.
@dynamic
def from_default(*, value=1):
    events.append("old default body")
    return ("old-default", value)

@dynamic
def from_body(value):
    replace_body(from_body)
    events.append("old body continued")
    return ("old-body", value)

@dynamic
def from_error():
    replace_error(from_error)
    raise error
""",
        },
        modules={"active_frames": "active_frames.py"},
    )
    project.run(
        """
import active_frames as module
import active_probe as probe

probe.install_changing_default(module.from_default)
assert module.from_default() == ("old-default", 7)
assert probe.events == ["default callback", "old default body"], probe.events
assert module.from_default(value=8) == ("new-default", 8)

assert module.from_body(9) == ("old-body", 9)
assert probe.events[-1] == "old body continued"
assert module.from_body(10) == ("new-body", 10)

try:
    module.from_error()
except LookupError as error:
    assert error is probe.error
else:
    raise AssertionError("the original frame's exception disappeared")
assert module.from_error() == "new-error"
print("active-source-frame-survives-mutation")
""",
        entry_interpreter=entry_interpreter,
    )


_EXPLICIT_KEYWORD_FUNCTIONS = """
from keyword_probe import dynamic, events

@dynamic
def named(alpha=1, beta=2, *, gamma=3):
    events.append("named body")
    return (alpha, beta, gamma)

@dynamic
def positional_only(alpha=1, /, beta=2):
    events.append("positional body")
    return (alpha, beta)

@dynamic
def collecting(alpha=1, /, beta=2, **extras):
    events.append("collecting body")
    return (alpha, beta, extras)

@dynamic
def changing(alpha=1, beta=2):
    events.append("original changing body")
    return ("old", alpha, beta)
"""

_EXPLICIT_KEYWORD_PROBE = """
events = []
comparison_error = LookupError("keyword comparison failed")

def dynamic(function):
    return function

class Keyword(str):
    __hash__ = str.__hash__

    def __new__(cls, text, target, *, error=None, callback=None):
        value = super().__new__(cls, text)
        value.target = target
        value.error = error
        value.callback = callback
        return value

    def __eq__(self, other):
        events.append(("compare", other))
        if self.callback is not None:
            self.callback(other)
        if self.error is not None:
            raise self.error
        return other == self.target

def replacement(alpha=1, beta=2):
    return ("new", alpha, beta)

def exercise(module):
    events.clear()
    key = Keyword("not-a-parameter", "beta")
    assert module.named(**{key: 9}) == (1, 9, 3)
    assert events == [("compare", "alpha"), ("compare", "beta"), "named body"], events

    # String payload equality must not bypass a subclass's false comparison.
    events.clear()
    key = Keyword("alpha", "absent")
    try:
        module.named(**{key: 9})
    except TypeError:
        pass
    else:
        raise AssertionError("keyword payload bypassed __eq__")
    assert events == [("compare", "alpha"), ("compare", "beta"), ("compare", "gamma")], events

    # Explicit-keyword errors precede excess positional arguments and defaults.
    events.clear()
    key = Keyword("not-a-parameter", "alpha", error=comparison_error)
    try:
        module.named(1, 2, 3, **{key: 4})
    except LookupError as error:
        assert error is comparison_error
    else:
        raise AssertionError("keyword comparison exception disappeared")
    assert events == [("compare", "alpha")], events

    events.clear()
    key = Keyword("not-a-parameter", "alpha")
    try:
        module.named(1, **{key: 4})
    except TypeError as error:
        assert "multiple values" in str(error), str(error)
    else:
        raise AssertionError("duplicate binding bypassed keyword equality")
    assert events == [("compare", "alpha")], events

    # Positional-only names are excluded from ordinary keyword matching.
    events.clear()
    key = Keyword("alpha", "alpha")
    alpha, beta, extras = module.collecting(**{key: 9})
    assert (alpha, beta) == (1, 2)
    assert list(extras.values()) == [9]
    assert next(iter(extras)) is key
    assert events == [("compare", "beta"), "collecting body"], events

    events.clear()
    key = Keyword("not-a-parameter", "alpha")
    try:
        module.positional_only(1, **{key: 4})
    except TypeError as error:
        assert "positional-only" in str(error), str(error)
    else:
        raise AssertionError("positional-only conflict was accepted")
    assert events == [("compare", "beta"), ("compare", "alpha")], events

    # The original active frame and name objects survive code replacement.
    events.clear()
    def replace_code(other):
        if other == "alpha":
            module.changing.__code__ = replacement.__code__
    key = Keyword("not-a-parameter", "beta", callback=replace_code)
    assert module.changing(**{key: 11}) == ("old", 1, 11)
    assert events == [("compare", "alpha"), ("compare", "beta"), "original changing body"], events
    assert module.changing(beta=12) == ("new", 1, 12)
"""


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_explicit_keyword_subclasses_preserve_native_binding_callbacks(
    tmp_path, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "keyword_calls.py": "# soac: module(strict_assign=true, checked_attr=true)\n"
            + _EXPLICIT_KEYWORD_FUNCTIONS,
            "keyword_control.py": _EXPLICIT_KEYWORD_FUNCTIONS,
            "keyword_probe.py": _EXPLICIT_KEYWORD_PROBE,
        },
        modules={"keyword_calls": "keyword_calls.py"},
    )
    project.run(
        """
import keyword_calls
import keyword_control
from keyword_probe import exercise

exercise(keyword_control)
exercise(keyword_calls)
print("keyword-comparison-binding")
""",
        entry_interpreter=entry_interpreter,
    )


def test_dynamic_function_code_replaced_by_decorator_is_not_adopted(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "dynamic_probe.py": """
def replacement(value):
    return ("replacement", value)

def later(value):
    return ("later", value)

def replace(function):
    function.__code__ = replacement.__code__
    return function
""",
            "dynamic_function.py": """
# soac: module(strict_assign=true, checked_attr=true)
from dynamic_probe import replace

@replace
def dynamic(value):
    return ("source", value)
""",
        },
        modules={"dynamic_function": "dynamic_function.py"},
    )
    project.run(
        """
import dynamic_function
import dynamic_probe

assert dynamic_function.dynamic(1) == ("replacement", 1)
dynamic_function.dynamic.__code__ = dynamic_probe.later.__code__
assert dynamic_function.dynamic(2) == ("later", 2)
print("dynamic-function-not-adopted")
"""
    )


@pytest.mark.parametrize(
    ("backend", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
)
def test_statically_dynamic_framework_methods_keep_ordinary_annotations(
    tmp_path, backend, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "framework_probe.py": """
def replacement(self, value):
    return ("framework", value)

def instrument(cls):
    vars(cls)["method"].__code__ = replacement.__code__
    return cls

class Meta(type):
    def __new__(metaclass, name, bases, namespace):
        namespace["method"].__code__ = replacement.__code__
        return super().__new__(metaclass, name, bases, namespace)
""",
            "framework_methods.py": """
# soac: module(strict_assign=true, checked_attr=true)
from framework_probe import Meta, instrument

class Managed(metaclass=Meta):
    def method(self, value: int) -> int:
        return value

@instrument
class Decorated:
    def method(self, value: int) -> int:
        return value

def independent(value: int) -> int:
    return value
""",
        },
        modules={"framework_methods": "framework_methods.py"},
        backend=backend,
    )
    validation = """
import framework_methods as module

# These source classes were already classified as dynamic before any method
# object existed. Framework instrumentation preserves ordinary code mutation.
for cls in (module.Managed, module.Decorated):
    assert cls().method("not an integer") == ("framework", "not an integer")

assert module.independent(3) == 3
assert module.independent("bad") == "bad"
print("static-framework-boundaries")
"""
    if backend == "cpython":
        native_before = """
import ctypes
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
get_type_owner.argtypes = [ctypes.py_object]
get_type_owner.restype = ctypes.c_void_p
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p

import framework_methods as module
module_witness = _soac_ext.strict_module_diagnostics(module)
for cls in (module.Managed, module.Decorated):
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
    assert get_type_owner(cls) is None
    function = vars(cls)["method"]
    witness = _soac_ext.strict_function_diagnostics(function)
    assert witness is not None
    assert witness["schema"] == 2 and witness["backend"] == "cpython"
    assert witness["entry_kind"] == "ordinary_replacement"
    assert witness["finalized"] is False
    assert metadata(function) is None
    for key in (
        "source_path", "source_sha256", "artifact_generation",
        "startup_identity", "interpreter_id",
    ):
        assert witness[key] == module_witness[key]
independent_witness = _assert_cpython_function_witness(
    module.independent, module_witness,
)
assert independent_witness["finalized"]
"""
        native_after = """
from soac.strict import StrictMutationError

call_one = ctypes.pythonapi.PyObject_CallOneArg
call_one.argtypes = [ctypes.py_object, ctypes.py_object]
call_one.restype = ctypes.py_object
for cls in (module.Managed, module.Decorated):
    instance = cls()
    for _ in range(128):
        assert instance.method("ordinary") == ("framework", "ordinary")
    assert call_one(instance.method, "C") == ("framework", "C")
assert call_one(module.independent, 4) == 4
assert call_one(module.independent, "bad") == "bad"
try:
    module.independent.__code__ = module.independent.__code__
except StrictMutationError:
    pass
else:
    raise AssertionError("framework fallback revoked independent code protection")
independent_after = _assert_cpython_function_witness(
    module.independent, module_witness,
)
assert independent_after["finalized"] and independent_after["original_code_entered"]
"""
        project.run_case(
            "framework_methods", native_before + validation + native_after,
            Path(__file__), required_functions=("independent",),
             backend="cpython",
        )
    else:
        project.run(validation, entry_interpreter=entry_interpreter)


_BINDING_IDENTITY_FUNCTIONS = """
from binding_identity_probe import dynamic, events

@dynamic
def plain(*, value=1):
    return value

@dynamic
def stream(*, value=1):
    events.append(("source body", value))
    yield value
"""

_BINDING_IDENTITY_PROBE = """
events = []
held = []

def dynamic(function):
    return function

def replacement(*, value=1):
    yield value + 100

class IdentityKey:
    def __init__(self, function, reenter=False):
        self.function = function
        self.expected = function.__code__.co_varnames[0]
        self.reenter = reenter

    def __hash__(self):
        return hash(self.expected)

    def __eq__(self, other):
        events.append(("name identity", other is self.expected))
        assert other is self.expected, "binder replaced the native parameter-name object"
        if self.reenter:
            # This completes another binding/construction on the same function
            # without recursively consulting its defaults or running its body.
            held.append(self.function(value=99))
            self.function.__kwdefaults__ = {"value": 20}
            self.function.__code__ = replacement.__code__
        return True

def exercise(module):
    events.clear()
    held.clear()
    module.plain.__kwdefaults__ = {IdentityKey(module.plain): 7}
    assert module.plain() == 7
    assert events == [("name identity", True)], events
    module.plain.__kwdefaults__ = {}

    events.clear()
    module.stream.__kwdefaults__ = {IdentityKey(module.stream, reenter=True): 7}
    created = module.stream()
    assert events == [("name identity", True)], events
    assert list(created) == [7]
    assert list(held.pop()) == [99]
    assert events == [("name identity", True), ("source body", 7), ("source body", 99)], events
    assert list(module.stream()) == [120]
"""


@pytest.fixture(scope="module")
def binding_identity_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-binding-identity"),
        {
            "binding_identity.py": "# soac: module(strict_assign=true, checked_attr=true)\n"
            + _BINDING_IDENTITY_FUNCTIONS,
            "binding_identity_control.py": _BINDING_IDENTITY_FUNCTIONS,
            "binding_identity_probe.py": _BINDING_IDENTITY_PROBE,
        },
        modules={"binding_identity": "binding_identity.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_name_identity_and_reentrant_generator_creation_match_native(
    binding_identity_project, entry_interpreter
):
    binding_identity_project.run(
        """
import binding_identity
import binding_identity_control
from binding_identity_probe import exercise

exercise(binding_identity_control)
exercise(binding_identity)
print("original-name-and-generator-binding")
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("timing", ["idle", "binding"])
def test_same_code_assignment_keeps_dynamic_source_owner(
    binding_identity_project, entry_interpreter, timing
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    program = (
        f"expected_entry = {expected_entry!r}\ntiming = {timing!r}\n"
        + r"""
import ctypes
import binding_identity as actual
import binding_identity_control as ordinary
from soac import _soac_ext

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = arguments
    function.restype = result
    return function

obj = ctypes.py_object
owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
seal = api("PyFunction_GetSoacStrictId", ctypes.c_uint64, obj)
get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
assert _soac_ext.strict_module_diagnostics(actual)["sealed"]
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert owner(actual.plain) and not owner(ordinary.plain)
assert _soac_ext.strict_function_entry_kind(actual.plain) == expected_entry

def exercise(function):
    # The existing unknown-decorator fixture is deliberately dynamic. This
    # must not weaken the same-code rejection for metadata-sealed functions.
    assert seal(function) == 0
    code = function.__code__
    original_owner = owner(function)
    original_entry = get_vectorcall(function)
    original_defaults = function.__kwdefaults__
    marker = object()
    events = []
    parameter_name = code.co_varnames[0]

    class SameCodeKey:
        def __hash__(self):
            return hash(parameter_name)
        def __eq__(self, other):
            events.append(other is parameter_name)
            function.__code__ = code
            return other == parameter_name

    try:
        if timing == "binding":
            function.__kwdefaults__ = {SameCodeKey(): marker}
            assert function() is marker
            assert events == [True], events
        else:
            function.__code__ = code
        assert function.__code__ is code and owner(function) == original_owner
        assert get_vectorcall(function) == original_entry
        # The active binder must finish once, and the next invocation must
        # preserve source authority after the public same-code assignment.
        assert function(value=marker) is marker
        assert events == ([True] if timing == "binding" else []), events
    finally:
        function.__kwdefaults__ = original_defaults
    assert function() == 1

exercise(ordinary.plain)
exercise(actual.plain)
"""
    )
    binding_identity_project.run(program, entry_interpreter=entry_interpreter)


@pytest.fixture(scope="module")
def changed_code_functions(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-changed-code"),
        {
            "fallback_probe.py": """
marker = LookupError("replacement body exception")

def replacement(value):
    if value is marker:
        raise marker
    return value

def replace(function):
    function.__code__ = replacement.__code__
    return function
""",
            "changed_code.py": """
# soac: module(strict_assign=true, checked_attr=true)
from fallback_probe import marker, replace

@replace
def changed(value):
    return value
""",
        },
        modules={"changed_code": "changed_code.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("disposition", ["sealed", "terminal"])
def test_warmed_changed_code_calls_keep_live_owner_checks(
    changed_code_functions, entry_interpreter, disposition
):
    changed_code_functions.run(
        f"disposition = {disposition!r}\n"
        + """
import ctypes
import gc
import changed_code
import fallback_probe
from soac.strict import StrictRuntimeUnavailableError

function = changed_code.changed
vectorcall = ctypes.pythonapi.PyVectorcall_Function
vectorcall.argtypes = [ctypes.py_object]
vectorcall.restype = ctypes.c_void_p
native_vectorcall = vectorcall(fallback_probe.replacement)

# This caller is ordinary native CPython code. Repeated calls exercise its
# adaptive CALL path, not a manufactured Rust helper or SOAC direct target.
def invoke():
    return function(7)

for _ in range(256):
    assert invoke() == 7
assert vectorcall(function) != native_vectorcall, "fallback published unchecked native vectorcall"
try:
    function(fallback_probe.marker)
except LookupError as error:
    assert error is fallback_probe.marker
else:
    raise AssertionError("native replacement lost its original exception")

owner, = [value for value in gc.get_referents(function)
          if type(value).__name__ == "_StrictFunctionOwner"]

# Trusted C test probes change only the real native/GC state. They are not
# production authority paths and do not manufacture a replacement contract.
if disposition == "sealed":
    seal = ctypes.pythonapi.PyFunction_SealSoacStrict
    seal.argtypes = [ctypes.py_object, ctypes.c_uint64]
    seal.restype = ctypes.c_int
    assert seal(function, 0x5EA1) == 0
else:
    get_slot = ctypes.pythonapi.PyType_GetSlot
    get_slot.argtypes = [ctypes.py_object, ctypes.c_int]
    get_slot.restype = ctypes.c_void_p
    # Py_tp_clear from the selected CPython's stable typeslots.h API.
    clear_address = get_slot(type(owner), 51)
    assert clear_address
    clear = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object)(clear_address)
    assert clear(owner) == 0

for _ in range(256):
    try:
        invoke()
    except StrictRuntimeUnavailableError:
        pass
    else:
        raise AssertionError("a warmed native CALL bypassed owner/contract rejection")
print("warmed-replacement-owner-check", disposition)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def late_function_definitions(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-late-function-definitions"),
        {
            "late_functions.py": """
# soac: module(strict_assign=true, checked_attr=true)
from late_function_support import make_default, reuse_previous

def factory():
    class Token:
        pass
    Alias = Token
    def accept(value: Alias = Token()) -> Alias:
        return value
    return Token, accept

def decorated_factory():
    @reuse_previous
    def candidate(value=make_default()):
        return value
    return candidate
""",
            "late_function_support.py": """
from typing import Any

events = []
previous = None
sequence = 0

class Marker:
    def __init__(self, index: int):
        self.index = index

    def __del__(self):
        events.append(f"release:{self.index}")

def make_default() -> Any:
    global sequence
    sequence += 1
    events.append(f"create:{sequence}")
    return Marker(sequence)

def reuse_previous(function: Any) -> Any:
    global previous
    if previous is None:
        events.append("keep")
        previous = function
    else:
        events.append("reuse")
    return previous
""",
        },
        modules={"late_functions": "late_functions.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_late_free_function_completion_freezes_defaults_not_annotated_value_types(
    late_function_definitions, entry_interpreter
):
    late_function_definitions.run(
        """
import ctypes
import late_functions as module
from soac.strict import StrictMutationError

get_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
get_identity.argtypes = [ctypes.py_object]
get_identity.restype = ctypes.c_uint64

first, function = module.factory()
second, _ = module.factory()
assert get_identity(function) != 0, "late definition escaped its completion boundary"
defaults = function.__defaults__
assert type(defaults[0]) is first
assert function() is defaults[0]

try:
    function.__defaults__ = (second(),)
except StrictMutationError:
    pass
else:
    raise AssertionError("completed free function still has replaceable defaults")
assert function.__defaults__ is defaults

provider = function.__annotate__
cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
cells["Alias"].cell_contents = second
assert function() is defaults[0]
value = first()
assert function(value) is value
other = second()
assert function(other) is other
print("late-function-completion")
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_late_free_function_decorator_cannot_adopt_a_previous_execution(
    late_function_definitions, entry_interpreter
):
    late_function_definitions.run(
        """
import ctypes
import gc
import late_functions as module
from late_function_support import events

get_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
get_identity.argtypes = [ctypes.py_object]
get_identity.restype = ctypes.c_uint64

first = module.decorated_factory()
assert events == ["create:1", "keep"]
second = module.decorated_factory()
assert second is first
gc.collect()
assert [event for event in events if not event.startswith("release:")] == [
    "create:1", "keep", "create:2", "reuse",
]
assert [event for event in events if event.startswith("release:")] == ["release:2"]
assert get_identity(first) == 0, "source equality authorized foreign decorator output"

# Neither a completion ticket nor idle metadata may retain discarded default
# objects, or freeze a function whose arbitrary decorator remains dynamic.
first.__defaults__ = ("replacement",)
gc.collect()
assert sorted(event for event in events if event.startswith("release:")) == [
    "release:1", "release:2",
]
assert first() == "replacement"
print("late-decorator-execution-isolation")
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def lexical_function_ownership(request, tmp_path_factory):
    backend = request.param
    return create_strict_project(
        tmp_path_factory.mktemp(f"strict-lexical-function-ownership-{backend}"),
        {
            "lexical_functions.py": """
# soac: module(strict_assign=true, checked_attr=true)
from lexical_function_support import DynamicMeta, remember, replacement

def standalone(value: int = 1) -> int:
    return value

class Dynamic(metaclass=DynamicMeta):
    borrowed = standalone

    def overwritten(self):
        return "original"

    preserved = remember(overwritten)
    overwritten = replacement()

    def factory(self):
        def nested(value: int = 3) -> int:
            return value
        return nested
""",
            "lexical_function_support.py": """
from typing import Any

class DynamicMeta(type):
    pass

def remember(function: Any) -> Any:
    return function

def changed(self):
    return "changed"

def replacement() -> Any:
    return changed
""",
        },
        modules={"lexical_functions": "lexical_functions.py"},
        backend=backend,
    )


@pytest.mark.parametrize(
    ("lexical_function_ownership", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["lexical_function_ownership"],
    scope="module",
)
def test_function_adoption_follows_lexical_ownership_not_class_aliases(
    lexical_function_ownership, entry_interpreter
):
    project = lexical_function_ownership
    if project.backend == "cpython":
        native_validation = """
import ctypes
import lexical_functions as module
from lexical_function_support import changed
from soac.strict import StrictMutationError

from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
get_type_owner.argtypes = [ctypes.py_object]
get_type_owner.restype = ctypes.c_void_p
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p

module_witness = _soac_ext.strict_module_diagnostics(module)
cls = module.Dynamic
info = ConstructionInfoV1()
assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
assert (
    info.abi_version, info.struct_size, info.phase,
    info.permanent_contract_published, info.owner, info.root_construction,
) == (0, 0, 0, 0, None, None)
assert get_type_owner(cls) is None

standalone_witness = _assert_cpython_function_witness(
    module.standalone, module_witness,
)
assert standalone_witness["finalized"]

assert module.Dynamic.borrowed is module.standalone
assert module.standalone(4) == 4
assert module.standalone("bad") == "bad"

# Overwriting the final class member does not make the old lexical method a
# free function. Its statically dynamic framework keeps ordinary mutability.
preserved = module.Dynamic.preserved
preserved_witness = _assert_cpython_function_witness(
    preserved, module_witness,
)
assert preserved_witness["finalized"] is False
preserved.__code__ = changed.__code__
assert preserved(None) == "changed"
factory_witness = _assert_cpython_function_witness(
    vars(module.Dynamic)["factory"], module_witness,
)
assert factory_witness["finalized"] is False

# A definition inside a method has that function as its immediate scope, not
# the enclosing class. Its late free-function completion still applies.
nested = module.Dynamic().factory()
nested_witness = _assert_cpython_function_witness(
    nested, module_witness,
)
assert nested_witness["finalized"]
assert nested() == 3
try:
    nested.__defaults__ = (5,)
except StrictMutationError:
    pass
else:
    raise AssertionError("an enclosing dynamic class captured a nested free definition")
print("lexical-function-ownership")

# Dynamic class writes must not revoke or retarget the independent function.
module.Dynamic.borrowed = changed
assert module.standalone is not module.Dynamic.borrowed
for _ in range(128):
    assert module.standalone(4) == 4
    assert nested(5) == 5
call_one = ctypes.pythonapi.PyObject_CallOneArg
call_one.argtypes = [ctypes.py_object, ctypes.py_object]
call_one.restype = ctypes.py_object
for function, value in ((module.standalone, 6), (nested, 7)):
    assert call_one(function, value) == value
    assert call_one(function, "bad") == "bad"
    try:
        function.__code__ = function.__code__
    except StrictMutationError:
        pass
    else:
        raise AssertionError("dynamic class revoked independent code protection")
    witness = _assert_cpython_function_witness(
        function, module_witness,
    )
    assert witness["finalized"] and witness["original_code_entered"]
preserved_after = _soac_ext.strict_function_diagnostics(preserved)
assert preserved_after is not None
assert preserved_after["backend"] == "cpython"
assert preserved_after["entry_kind"] == "ordinary_replacement"
assert preserved_after["finalized"] is False
for key in (
    "source_path", "source_sha256", "artifact_generation",
    "startup_identity", "interpreter_id",
):
    assert preserved_after[key] == module_witness[key]
assert metadata(preserved) is None
"""
        project.run_case(
            "lexical_functions", native_validation, Path(__file__),
            required_functions=("standalone",),
             backend="cpython",
        )
    else:
        project.run(
            """
import ctypes
import lexical_functions as module
from lexical_function_support import changed
from soac.strict import StrictMutationError

get_identity = ctypes.pythonapi.PyFunction_GetSoacStrictId
get_identity.argtypes = [ctypes.py_object]
get_identity.restype = ctypes.c_uint64

assert module.Dynamic.borrowed is module.standalone
assert get_identity(module.standalone) != 0
assert module.standalone(4) == 4
assert module.standalone("bad") == "bad"

# Overwriting the final class member does not make the old lexical method a
# free function. Its statically dynamic framework keeps ordinary mutability.
preserved = module.Dynamic.preserved
assert get_identity(preserved) == 0
preserved.__code__ = changed.__code__
assert preserved(None) == "changed"
assert get_identity(module.Dynamic.factory) == 0

# A definition inside a method has that function as its immediate scope, not
# the enclosing class. Its late free-function completion still applies.
nested = module.Dynamic().factory()
assert get_identity(nested) != 0
assert nested() == 3
try:
    nested.__defaults__ = (5,)
except StrictMutationError:
    pass
else:
    raise AssertionError("an enclosing dynamic class captured a nested free definition")
print("lexical-function-ownership")
""",
            entry_interpreter=entry_interpreter,
        )


_NAMED_KEYWORD_OPERANDS = """
from keyword_operand_support import keyword_referrers, make_marker, mixed_referrers

def inspect_keyword(value):
    return keyword_referrers(value=value)

def inspect_mixed(value):
    return mixed_referrers(value, named=1)

def release_keywords(callback):
    return callback(first=make_marker("first"), second=make_marker("second"))

def captured_keyword_attribute(holder, first, second):
    return holder.callback(first=first(), second=second())

def captured_keyword_cell(callback):
    def replace(value):
        nonlocal callback
        callback = value
    def invoke(first, second):
        return callback(first=first(), second=second())
    return invoke, replace
"""


@pytest.fixture(scope="module")
def named_keyword_operands(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-named-keyword-operands"),
        {
            "keyword_operands.py": "# soac: module(strict_assign=true, checked_attr=true)\n"
            + _NAMED_KEYWORD_OPERANDS,
            "keyword_operand_control.py": _NAMED_KEYWORD_OPERANDS,
            "keyword_operand_support.py": """
import gc
import weakref

events: list[str] = []
observed_arguments = []

class Marker:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append("drop:" + self.name)

def make_marker(name):
    events.append("create:" + name)
    return Marker(name)

def keyword_referrers(*, value):
    observed_arguments.append(weakref.ref(value))
    return [
        (type(referrer).__name__, tuple(referrer))
        for referrer in gc.get_referrers(value)
        if type(referrer) is dict and referrer.get("value") is value
    ]

def mixed_referrers(value, *, named):
    assert named == 1
    observed_arguments.append(weakref.ref(value))
    return [
        (type(referrer).__name__, len(referrer))
        for referrer in gc.get_referrers(value)
        if type(referrer) is tuple and any(item is value for item in referrer)
    ]

def discard(*, first, second):
    events.append("body")

class Holder:
    def __init__(self, callback):
        self.callback = callback

class Sink:
    def __call__(self, *, first, second):
        events.append("body:original")

    def __del__(self):
        events.append("drop:callable")
""",
        },
        modules={"keyword_operands": "keyword_operands.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("mixed", [False, True])
def test_named_keyword_calls_preserve_argument_identity_without_retention(
    named_keyword_operands, entry_interpreter, mixed
):
    function_name = "inspect_mixed" if mixed else "inspect_keyword"
    named_keyword_operands.run_case(
        "keyword_operands",
        f"""
import gc
import weakref
import keyword_operand_control as control
from keyword_operand_support import observed_arguments
from soac import _soac_ext

def validate_module(module):
    assert _soac_ext.strict_module_diagnostics(control) is None
    assert _soac_ext.strict_function_entry_kind(control.{function_name}) is None
    class Payload:
        pass
    for candidate in (control, module):
        observed_arguments.clear()
        released = []
        payload = Payload()
        reference = weakref.ref(payload, lambda _: released.append("payload"))
        result = candidate.{function_name}(payload)
        assert len(observed_arguments) == 1
        assert observed_arguments[0]() is payload
        assert isinstance(result, list)
        if candidate is control:
            assert result == [], result
        # SOAC may own different temporary containers during the call. None
        # may keep the argument alive after the call and explicit collection.
        del payload
        gc.collect()
        assert reference() is None, result
        assert released == ["payload"], released
""",
        __file__,
        entry_interpreter=entry_interpreter,
        required_functions=(function_name,),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("rejected", [False, True])
def test_named_keyword_call_preserves_binding_and_releases_values_once(
    named_keyword_operands, entry_interpreter, rejected
):
    named_keyword_operands.run_case(
        "keyword_operands",
        f"""
import gc
import keyword_operand_control as control
from keyword_operand_support import discard, events
from soac import _soac_ext

def validate_module(module):
    assert _soac_ext.strict_module_diagnostics(control) is None
    assert _soac_ext.strict_function_entry_kind(control.release_keywords) is None
    def observe(function):
        events.clear()
        error_info = None
        try:
            function(object if {rejected!r} else discard)
        except TypeError as error:
            error_info = (type(error).__name__, error.args)
        events.append("after")
        gc.collect()
        return tuple(events), error_info

    expected = observe(control.release_keywords)
    actual = observe(module.release_keywords)
    prefix = ("create:first", "create:second")
    if not {rejected!r}:
        prefix += ("body",)
    assert expected[0] == prefix + ("drop:second", "drop:first", "after"), expected
    assert (expected[1] is not None) == {rejected!r}, expected
    assert actual[1] == expected[1], (actual, expected)
    assert tuple(event for event in actual[0] if not event.startswith("drop:")) == prefix + ("after",), actual
    assert sorted(event for event in actual[0] if event.startswith("drop:")) == [
        "drop:first", "drop:second",
    ], actual
""",
        __file__,
        entry_interpreter=entry_interpreter,
        required_functions=("release_keywords",),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("capture", ["attribute", "cell"])
@pytest.mark.parametrize("argument_error", [False, True])
def test_named_keyword_callbacks_keep_the_captured_callable_and_cleanup(
    named_keyword_operands, entry_interpreter, capture, argument_error
):
    named_keyword_operands.run_case(
        "keyword_operands",
        f"""
import gc
import keyword_operand_control as control
from keyword_operand_support import Holder, Sink, events, make_marker
from soac import _soac_ext

def validate_module(module):
    def observe(candidate):
        events.clear()
        marker = LookupError("keyword expression failed")
        def replacement(*, first, second):
            raise AssertionError("call target was reloaded after an argument callback")
        if {capture!r} == "cell":
            invoke, replace = candidate.captured_keyword_cell(Sink())
            entry = _soac_ext.strict_function_entry_kind(invoke)
            expected_entry = None if candidate is control else (
                "entry_interpreter" if __dp_integration_entry__ else "checked_native"
            )
            assert entry == expected_entry, entry
        else:
            holder = Holder(Sink())
            def replace(value):
                holder.callback = value
            def invoke(first, second):
                return candidate.captured_keyword_attribute(holder, first, second)
        def first():
            return make_marker("first")
        def second():
            events.append("second")
            replace(replacement)
            if {argument_error!r}:
                raise marker
            return make_marker("second")
        try:
            invoke(first, second)
        except LookupError as error:
            assert {argument_error!r}
            assert error is marker
            events.append("caught")
        else:
            assert not {argument_error!r}
        events.append("after")
        marker.__traceback__ = None
        gc.collect()
        return tuple(events)

    expected = observe(control)
    actual = observe(module)
    prefix = ("create:first", "second")
    if {argument_error!r}:
        suffix = ("drop:first", "drop:callable", "caught", "after")
    else:
        suffix = ("create:second", "body:original", "drop:second", "drop:first", "drop:callable", "after")
    assert expected == prefix + suffix, expected
    assert tuple(event for event in actual if not event.startswith("drop:")) == tuple(
        event for event in expected if not event.startswith("drop:")
    ), (actual, expected)
    assert sorted(event for event in actual if event.startswith("drop:")) == sorted(
        event for event in expected if event.startswith("drop:")
    ), (actual, expected)
""",
        __file__,
        entry_interpreter=entry_interpreter,
        required_functions=("captured_keyword_" + capture,),
    )


_NONLOCAL_CLASS_CELL_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def factory():
    class Model:
        def reader(self):
            def read():
                nonlocal __class__
                return __class__
            return read

        def replace(self, value):
            nonlocal __class__
            __class__ = value

        def erase(self):
            nonlocal __class__
            del __class__

        def direct(self):
            return __class__
    return Model
"""


@pytest.fixture(scope="module")
def strict_nonlocal_class_cell(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-nonlocal-class-cell"),
        {
            "class_cell.py": _NONLOCAL_CLASS_CELL_SOURCE,
            "ordinary_class_cell.py": _NONLOCAL_CLASS_CELL_SOURCE.replace(
                "# soac: module(strict_assign=true, checked_attr=true)", "# ordinary source control", 1
            ),
        },
        modules={"class_cell": "class_cell.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_nonlocal_implicit_class_cell_read_write_delete(
    strict_nonlocal_class_cell, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_nonlocal_class_cell.run_case(
        "class_cell",
        textwrap.dedent(f"""
        def validate(module):
            import ctypes
            import ordinary_class_cell
            from soac import _soac_ext

            class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
            class_owner.argtypes = [ctypes.py_object]
            class_owner.restype = ctypes.c_void_p

            def observations(candidate, strict):
                first = candidate.factory()
                second = candidate.factory()
                instance = first()
                other = second()
                read = instance.reader()
                other_read = other.reader()
                expected = {expected_entry!r} if strict else None
                assert _soac_ext.strict_function_entry_kind(read) == expected
                assert _soac_ext.strict_function_entry_kind(other_read) == expected
                assert bool(class_owner(first)) is strict
                assert bool(class_owner(second)) is strict
                assert read() is first and instance.direct() is first
                assert other_read() is second and other.direct() is second

                replacement = object()
                instance.replace(replacement)
                assert read() is replacement and instance.direct() is replacement
                assert other_read() is second and other.direct() is second
                instance.erase()
                errors = []
                for callback in (read, instance.direct):
                    try:
                        callback()
                    except NameError as error:
                        errors.append((type(error).__name__, error.args))
                    else:
                        raise AssertionError("deleted class cell remained readable")
                instance.replace(first)
                assert read() is first and instance.direct() is first
                assert other_read() is second and other.direct() is second
                assert "__class__" not in vars(candidate)
                assert "__class__" not in vars(first)
                assert "__class__" not in vars(second)
                assert _soac_ext.strict_function_entry_kind(read) == expected
                return errors

            expected = observations(ordinary_class_cell, False)
            actual = observations(module, True)
            assert actual == expected, (actual, expected)
        """),
        strict_nonlocal_class_cell.project / "class_cell.py",
        entry_interpreter=entry_interpreter,
        required_functions=("factory",),
    )


_EAGER_CLASS_CELL_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
__class__ = 100
def factory():
    class Outer:
        class Inner:
            nonlocal __class__
            __class__ = "construction"
            saved: str = __class__
            def own_class(self):
                return __class__
            def replace(self, value):
                nonlocal __class__
                __class__ = value
        def own_class(self):
            return __class__
        def replace(self, value):
            nonlocal __class__
            __class__ = value
    return Outer
"""


@pytest.fixture(scope="module")
def strict_eager_class_cell(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-eager-class-cell"),
        {
            "eager_cell.py": _EAGER_CLASS_CELL_SOURCE,
            "ordinary_eager_cell.py": _EAGER_CLASS_CELL_SOURCE.replace(
                "# soac: module(strict_assign=true, checked_attr=true)", "# ordinary source control", 1
            ),
        },
        modules={"eager_cell": "eager_cell.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_eager_nested_class_cell_has_distinct_outer_and_inner_owners(
    strict_eager_class_cell, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_eager_class_cell.run_case(
        "eager_cell",
        textwrap.dedent(f"""
        def validate(module):
            import ctypes
            import ordinary_eager_cell
            from soac import _soac_ext

            class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
            class_owner.argtypes = [ctypes.py_object]
            class_owner.restype = ctypes.c_void_p

            def observe(candidate, strict):
                first, second = candidate.factory(), candidate.factory()
                expected = {expected_entry!r} if strict else None
                for outer in (first, second):
                    inner = outer.Inner
                    assert bool(class_owner(outer)) is strict
                    assert bool(class_owner(inner)) is strict
                    assert inner.saved == "construction"
                    for cls in (outer, inner):
                        assert cls.own_class.__code__.co_freevars == ("__class__",)
                        assert _soac_ext.strict_function_entry_kind(cls.own_class) == expected
                        assert cls().own_class() is cls
                        assert "__class__" not in vars(cls)
                    assert outer.own_class.__closure__[0] is not inner.own_class.__closure__[0]
                assert first.own_class.__closure__[0] is not second.own_class.__closure__[0]
                assert first.Inner.own_class.__closure__[0] is not second.Inner.own_class.__closure__[0]

                outer_marker, inner_marker = object(), object()
                first().replace(outer_marker)
                assert first().own_class() is outer_marker
                assert first.Inner().own_class() is first.Inner
                first.Inner().replace(inner_marker)
                assert first().own_class() is outer_marker
                assert first.Inner().own_class() is inner_marker
                assert second().own_class() is second
                assert second.Inner().own_class() is second.Inner
                assert candidate.__dict__["__class__"] == 100
                first().replace(first)
                first.Inner().replace(first.Inner)

            observe(ordinary_eager_cell, False)
            observe(module, True)
        """),
        strict_eager_class_cell.project / "eager_cell.py",
        entry_interpreter=entry_interpreter,
        required_functions=("factory",),
    )


_PRIVATE_CLASS_CAPTURE_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from private_capture_support import argument, observe_namespace, observe_target, pause, replace_public_closure

class Base:
    def __init_subclass__(cls, *, flag: bool = False):
        super().__init_subclass__()

def namespace_factory():
    marker = object()
    class Captured:
        captured = marker
    return Captured

def build(should_fail: bool = False):
    class Target:
        pass
    observe_target(Target)
    class Holder(Base, flag=argument(should_fail)):
        def __init__(self, value):
            self.payload: Target = value
    return Target, Holder

def nested_namespace(should_fail: bool = False):
    class Target:
        pass
    observe_target(Target)
    class Outer:
        observe_namespace(False)
        class Holder:
            observe_namespace(should_fail)
            def __init__(self, value):
                self.payload: Target = value
    return Target, Outer.Holder

def make_public_bridge():
    class Target:
        pass
    observe_target(Target)
    def create():
        current = Target
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return current, Holder
    return create

# An ordinary callback changes the native closure while this module is still
# initializing. The eligible source function freezes only at module sealing.
public_bridge = replace_public_closure(make_public_bridge())

def private_bridge_family():
    class Target:
        pass
    def create():
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Holder
    return Target, create

def private_generator_family():
    class Target:
        pass
    def create():
        yield None
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        yield Holder
    return Target, create

def private_coroutine_family():
    class Target:
        pass
    async def create():
        await pause()
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Holder
    return Target, create

def private_async_generator_family():
    class Target:
        pass
    async def create():
        yield None
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        yield Holder
    return Target, create

def terminal_generator_family(payload):
    def create():
        yield None
        return payload is None
    return create

def terminal_coroutine_family(payload):
    async def create():
        await pause()
        return payload is None
    return create

def terminal_async_generator_family(payload):
    async def create():
        yield None
        if payload is None:
            return
    return create
"""


@pytest.fixture(scope="module")
def private_class_capture_project(tmp_path_factory):
    project = create_strict_project(
        tmp_path_factory.mktemp("strict-private-class-captures"),
        {
            "private_capture_model.py": _PRIVATE_CLASS_CAPTURE_SOURCE,
            "ordinary_private_capture.py": _PRIVATE_CLASS_CAPTURE_SOURCE.replace(
                "# soac: module(strict_assign=true, checked_attr=true)", "# ordinary metadata control", 1
            ),
            "private_capture_support.py": """
from collections.abc import Callable
import gc
import weakref
import ctypes

targets: list[weakref.ReferenceType[type]] = []
replay: Callable[[], None] | None = None
namespace_handles: list[object] = []

class PublicReplacement:
    pass

class PublicAfter:
    pass

class Pause:
    def __await__(self):
        yield "paused"
        return None

def pause():
    return Pause()

def make_cell(value):
    return (lambda: value).__closure__[0]

def replace_public_closure(function):
    setter = ctypes.pythonapi.PyFunction_SetClosure
    setter.argtypes = [ctypes.py_object, ctypes.py_object]
    setter.restype = ctypes.c_int
    assert function.__code__.co_freevars == ("Target",)
    assert setter(function, (make_cell(PublicReplacement),)) == 0
    return function

def observe_target(value: type) -> None:
    targets.append(weakref.ref(value))

def observe_namespace(should_fail: bool) -> None:
    for value in gc.get_objects():
        if type(value).__name__ == "_StrictNamespaceExecution":
            if not any(value is previous for previous in namespace_handles):
                namespace_handles.append(value)
    if should_fail:
        raise RuntimeError("nested namespace failed")

def argument(should_fail: bool) -> bool:
    callback = replay
    if callback is not None:
        callback()
    if should_fail:
        raise RuntimeError("class argument failed")
    return False
""",
        },
        modules={"private_capture_model": "private_capture_model.py"},
    )
    # Native class ownership alone does not enable field predicates. Assert
    # the actual exported policy before treating a write as a checked boundary.
    shards = list((project.root / "artifacts/objects").glob("*.soac-types"))
    assert len(shards) == 1
    facts = json.loads(shards[0].read_text())
    assert facts["language_policy"]["checked_attr"] is True
    return project


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_private_class_helpers_reject_create_calls_and_clear_failed_argument_captures(
    private_class_capture_project, function_create_watch_extension, entry_interpreter
):
    private_class_capture_project.run_case(
        "private_capture_model",
        textwrap.dedent(f"""
        def validate(module):
            import gc
            import importlib.util
            import ordinary_private_capture
            import private_capture_support as support
            from soac.strict import StrictRuntimeUnavailableError

            spec = importlib.util.spec_from_file_location(
                "_strict_function_create_watch", {str(function_create_watch_extension)!r}
            )
            watcher = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(watcher)
            events = watcher.watch(module.__dict__, "_dp_class_ns_Captured", ())
            try:
                captured = module.namespace_factory()
            finally:
                watcher.stop()
            assert len(events) == 1
            event = events[0]
            assert event["freevars"] == 1 and not event["closure_present"]
            assert event["flags"] & 0x10000000
            assert event["source_id"] == 0 and not event["owner_present"]
            assert not event["success"]
            assert isinstance(event["result"], StrictRuntimeUnavailableError)
            assert captured.captured is not None
            events.clear()
            del event, captured

            events = watcher.watch(module.__dict__, "_dp_define_class_Holder", ())
            try:
                try:
                    module.build(True)
                except RuntimeError as error:
                    assert str(error) == "class argument failed"
                else:
                    raise AssertionError("class argument failure was lost")
            finally:
                watcher.stop()
            assert len(events) == 1
            event = events[0]
            assert event["freevars"] == 0 and not event["owner_present"]
            assert event["flags"] & 0x10000000 and event["source_id"] == 0
            assert isinstance(event["result"], StrictRuntimeUnavailableError)
            escaped_helper = event["function"]
            assert escaped_helper.__closure__ is None
            gc.collect()
            assert support.targets[-1]() is None, "escaped failed helper retained its original Target cell"
            assert module.build.__code__.co_cellvars == ordinary_private_capture.build.__code__.co_cellvars == ()
            assert module.build.__code__.co_freevars == ordinary_private_capture.build.__code__.co_freevars == ()
        """),
        private_class_capture_project.project / "private_capture_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=("build", "namespace_factory"),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_private_class_capture_rejects_another_factory_namespace_birth(
    private_class_capture_project, function_create_watch_extension, entry_interpreter
):
    private_class_capture_project.run_case(
        "private_capture_model",
        textwrap.dedent(f"""
        def validate(module):
            import gc
            import importlib.util
            import private_capture_support as support
            from soac import _soac_ext
            from soac.strict import StrictRuntimeUnavailableError

            spec = importlib.util.spec_from_file_location(
                "_strict_function_create_watch", {str(function_create_watch_extension)!r}
            )
            watcher = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(watcher)
            first_events = watcher.watch(module.__dict__, "_dp_class_ns_Holder", (), invoke=False)
            try:
                target, holder = module.build(False)
            finally:
                watcher.stop()
            assert len(first_events) == 1
            first_namespace = first_events[0]["function"]
            assert holder.__init__.__closure__ is None
            assert holder.__init__.__annotate__ is None
            value = holder(target())
            try:
                holder(object())
            except TypeError:
                pass
            else:
                raise AssertionError("required method-only field check was absent")

            second_events = watcher.watch(module.__dict__, "_dp_define_class_Holder", ())
            def replay():
                assert len(second_events) == 1
                helper = second_events[0]["function"]
                # Compiler-owned class constructors use the entry interpreter
                # in both modes; the source build function is witnessed below.
                assert _soac_ext.strict_function_entry_kind(helper) == "entry_interpreter"
                helper(first_namespace, module.__dict__, (module.Base,), {{"flag": False}})
            support.replay = replay
            try:
                try:
                    module.build(False)
                except StrictRuntimeUnavailableError as error:
                    assert "another namespace function" in str(error)
                else:
                    raise AssertionError("same-source foreign namespace acquired original cells")
            finally:
                support.replay = None
                watcher.stop()
            gc.collect()
            assert support.targets[-1]() is None
            assert value.payload.__class__ is target
        """),
        private_class_capture_project.project / "private_capture_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=("build",),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_escaped_namespace_handles_release_completed_and_failed_private_cells(
    private_class_capture_project, entry_interpreter
):
    private_class_capture_project.run_case(
        "private_capture_model",
        textwrap.dedent("""
        def validate(module):
            import ctypes
            import gc
            import weakref
            import private_capture_support as support

            owner = ctypes.pythonapi.PyType_GetSoacContractOwner
            owner.argtypes = [ctypes.py_object]
            owner.restype = ctypes.c_void_p
            target, holder = module.nested_namespace()
            assert owner(holder), "nested field class did not obtain its actual contract"
            instance = holder(target())
            assert type(instance.payload) is target
            try:
                holder(object())
            except TypeError:
                pass
            else:
                raise AssertionError("nested field predicate was not enforced")
            target_ref, holder_ref = weakref.ref(target), weakref.ref(holder)
            del target, holder, instance
            gc.collect()
            assert support.namespace_handles
            assert target_ref() is None and holder_ref() is None

            try:
                module.nested_namespace(True)
            except RuntimeError as error:
                assert str(error) == "nested namespace failed"
            else:
                raise AssertionError("nested class body failure was lost")
            gc.collect()
            assert support.targets[-1]() is None, "failed namespace retained its original cell"
            for handle in support.namespace_handles:
                assert all(
                    value is None or value is type(handle)
                    for value in gc.get_referents(handle)
                ), "escaped namespace handle kept temporary Python edges"
        """),
        private_class_capture_project.project / "private_capture_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=("nested_namespace",),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_private_field_forwarding_uses_the_current_native_closure_after_preseal_replacement(
    private_class_capture_project, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    private_class_capture_project.run_case(
        "private_capture_model",
        textwrap.dedent(f"""
        def validate(module):
            import ctypes
            import ordinary_private_capture
            import private_capture_support as support
            from soac import _soac_ext
            from soac.strict import StrictMutationError

            owner = ctypes.pythonapi.PyType_GetSoacContractOwner
            owner.argtypes = [ctypes.py_object]
            owner.restype = ctypes.c_void_p
            setter = ctypes.pythonapi.PyFunction_SetClosure
            setter.argtypes = [ctypes.py_object, ctypes.py_object]
            setter.restype = ctypes.c_int
            ordinary = ordinary_private_capture.public_bridge
            actual = module.public_bridge
            assert actual.__code__.co_freevars == ordinary.__code__.co_freevars == ("Target",)
            assert _soac_ext.strict_function_entry_kind(actual) == {expected_entry!r}
            assert actual.__closure__[0].cell_contents is support.PublicReplacement
            expected_type, _ = ordinary()
            selected, first = actual()
            assert selected is expected_type is support.PublicReplacement
            assert owner(first)
            first(support.PublicReplacement())
            try:
                first(support.PublicAfter())
            except TypeError:
                pass
            else:
                raise AssertionError("field used the original pre-replacement cell")

            # Full freeze forbids another tuple substitution, but never freezes
            # the contents of the actual, originally adopted cell.
            saved = actual.__closure__
            try:
                setter(actual, (support.make_cell(support.PublicAfter),))
            except StrictMutationError:
                pass
            else:
                raise AssertionError("sealed source function allowed closure replacement")
            assert actual.__closure__ is saved
            saved[0].cell_contents = support.PublicAfter
            ordinary.__closure__[0].cell_contents = support.PublicAfter
            expected_type, _ = ordinary()
            selected, second = actual()
            assert selected is expected_type is support.PublicAfter
            assert owner(second)
            second(support.PublicAfter())
            first(support.PublicReplacement())
            for cls, wrong in ((first, support.PublicAfter), (second, support.PublicReplacement)):
                try:
                    cls(wrong())
                except TypeError:
                    pass
                else:
                    raise AssertionError("a later cell mutation retargeted an existing field policy")
        """),
        private_class_capture_project.project / "private_capture_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=("make_public_bridge", "public_bridge"),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("kind", ["bridge", "generator", "coroutine"])
def test_private_lexical_cells_live_with_their_function_or_suspended_frame_only(
    private_class_capture_project, entry_interpreter, kind
):
    factory_name = f"private_{kind}_family"
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    private_class_capture_project.run_case(
        "private_capture_model",
        textwrap.dedent(f"""
        def validate(module):
            import ctypes
            import gc
            import weakref
            import ordinary_private_capture
            from soac import _soac_ext

            owner = ctypes.pythonapi.PyType_GetSoacContractOwner
            owner.argtypes = [ctypes.py_object]
            owner.restype = ctypes.c_void_p
            factory = getattr(module, {factory_name!r})
            ordinary_factory = getattr(ordinary_private_capture, {factory_name!r})
            ordinary_target, ordinary_create = ordinary_factory()
            target, create = factory()
            assert factory.__code__.co_cellvars == ordinary_factory.__code__.co_cellvars == ()
            assert create.__code__.co_freevars == ordinary_create.__code__.co_freevars == ()
            assert create.__closure__ is ordinary_create.__closure__ is None
            assert create.__annotate__ is ordinary_create.__annotate__ is None
            expected = "generator_factory" if {kind!r} != "bridge" else {expected_entry!r}
            assert _soac_ext.strict_function_entry_kind(create) == expected
            target_ref = weakref.ref(target)
            del ordinary_target, ordinary_create, target
            gc.collect()
            assert target_ref() is not None, "returned private bridge lost its required original cell"
            frame = None
            if {kind!r} == "bridge":
                holder = create()
                del create
            elif {kind!r} == "generator":
                frame = create()
                del create
                assert next(frame) is None
                gc.collect()
                assert target_ref() is not None, "suspension lost its private cell"
                holder = next(frame)
            else:
                frame = create()
                del create
                assert frame.send(None) == "paused"
                gc.collect()
                assert target_ref() is not None, "coroutine suspension lost its private cell"
                try:
                    frame.send(None)
                except StopIteration as completed:
                    holder = completed.value
                else:
                    raise AssertionError("coroutine did not return its class")

            assert owner(holder)
            instance = holder(target_ref()())
            assert type(instance.payload) is target_ref()
            try:
                holder(object())
            except TypeError:
                pass
            else:
                raise AssertionError("private field predicate was lost")
            if frame is not None:
                frame.close()
            holder_ref = weakref.ref(holder)
            del holder, instance
            gc.collect()
            assert holder_ref() is None, "closed private frame retained the constructed class"
            assert target_ref() is None, "private cells outlived every consumer, including a closed frame"
        """),
        private_class_capture_project.project / "private_capture_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=(factory_name,),
    )


_TEMPORARY_LIFETIME_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
import gc

def failed_unpack(make, record, reject, key):
    values = make(3)
    try:
        first, second = iter(values)
    except ValueError:
        record("handler")
    del values
    gc.collect()
    record("after")

def transient_unpack(make, record, reject, key):
    try:
        first, second = iter(make(3))
    except ValueError:
        record("handler")
    record("after")

def partial_target(make, record, reject, key):
    try:
        first, reject().field = make(2)
    except AttributeError:
        record("handler")
    del first
    record("after")

def subscript_target(make, record, reject, key):
    try:
        reject()[key()] = make(1)[0]
    except TypeError:
        record("handler")
    record("after")

def suspended_target(make, record, reject, key):
    try:
        first, reject().field = yield "ready"
    except AttributeError:
        record("handler")
    del first
    record("after")
    yield "done"

def choose_key():
    yield "key"
    return "slot"

def suspended_live_operand(make, record, reject, key):
    target = {}
    target[(yield from choose_key())] = make(1)[0]
    record("stored")
    del target
    record("after")
    yield "done"
"""


_UNPACKED_SETITEM_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def unpacked_subscript_target(make, record, reject, key):
    try:
        reject()[0], = make(1)
    except TypeError:
        record("handler")
    record("after")

def escaping_unpacked_subscript_target(make, record, reject, key):
    reject()[0], = make(1)
    record("after")

def escaping_named_subscript_target(make, record, reject, key):
    replacement = make(1)[0]
    reject()[0] = replacement
    del replacement

def escaping_prefixed_subscript_target(make, record, reject, key):
    _dp_tmp_source = make(1)[0]
    reject()[0] = _dp_tmp_source
    del _dp_tmp_source
"""

_SETITEM_LIFETIME_CASES = (
    "unpacked_subscript_target",
    "escaping_unpacked_subscript_target",
    "escaping_named_subscript_target",
    "escaping_prefixed_subscript_target",
)


def _operand_lifetime_project(tmp_path_factory, label, source):
    ordinary = source.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1)
    return create_strict_project(
        tmp_path_factory.mktemp(label),
        {
            "operand_model.py": source,
            "ordinary_operand_model.py": ordinary,
        },
        modules={"operand_model": "operand_model.py"},
    )


@pytest.fixture(scope="module")
def temporary_lifetime_project(tmp_path_factory):
    return _operand_lifetime_project(
        tmp_path_factory, "strict-temporary-lifetimes", _TEMPORARY_LIFETIME_SOURCE
    )


@pytest.fixture(scope="module")
def unpacked_setitem_project(tmp_path_factory):
    # Eager compilation must reach these assignments independently of unrelated
    # suspended functions covered by the other fixture.
    return _operand_lifetime_project(
        tmp_path_factory, "strict-unpacked-setitem", _UNPACKED_SETITEM_SOURCE
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize(
    "case",
    [
        "failed_unpack",
        "transient_unpack",
        "partial_target",
        "subscript_target",
        *_SETITEM_LIFETIME_CASES,
        "suspended_target",
        "suspended_live_operand",
    ],
)
def test_assignment_operands_preserve_exceptions_suspension_and_cleanup(
    request, tmp_path, entry_interpreter, case
):
    project = request.getfixturevalue(
        "unpacked_setitem_project"
        if case in _SETITEM_LIFETIME_CASES
        else "temporary_lifetime_project"
    )
    expected_entry = (
        "generator_factory"
        if case.startswith("suspended_")
        else "entry_interpreter"
        if entry_interpreter
        else "checked_native"
    )
    program = f"""
        import ctypes
        import gc
        import sys
        import operand_model
        import ordinary_operand_model
        from soac import _soac_ext

        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        function = getattr(operand_model, {case!r})
        ordinary = getattr(ordinary_operand_model, {case!r})
        assert owner(function)
        assert not owner(ordinary)
        assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
        assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None
        assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}

        def exercise(function, successful_setitem=False):
            events = []
            live = [0]
            def exception_name():
                current = sys.exception()
                return None if current is None else type(current).__name__
            class Tracked:
                def __init__(self, label):
                    self.label = label
                    live[0] += 1
                def __del__(self):
                    live[0] -= 1
                    events.append(('drop', self.label, exception_name()))
            class ReadOnly:
                @property
                def field(self):
                    raise AssertionError('the setter must not invoke the getter')
                def __del__(self):
                    events.append(('drop', 'container', exception_name()))
            class Writable(ReadOnly):
                def __setitem__(self, index, value):
                    events.append(('store', index.label, value.label, exception_name()))
            target_type = Writable if successful_setitem else ReadOnly
            def make(count):
                return [Tracked(str(index)) for index in range(count)]
            def key():
                return Tracked('key')
            def record(stage):
                events.append((stage, live[0], exception_name()))
            try:
                raise KeyError('caller handler')
            except KeyError as marker:
                if {case!r}.startswith('escaping_'):
                    try:
                        function(make, record, target_type, key)
                    except TypeError as failure:
                        record('caller handler')
                        if function is ordinary and {case!r} in {{'escaping_named_subscript_target',
                                                                 'escaping_prefixed_subscript_target'}}:
                            source_name = ('replacement' if {case!r} == 'escaping_named_subscript_target'
                                           else '_dp_tmp_source')
                            traceback = failure.__traceback__
                            while traceback.tb_frame.f_code.co_name != function.__name__:
                                traceback = traceback.tb_next
                                assert traceback is not None, 'error lost its source frame'
                            binding = traceback.tb_frame.f_locals[source_name]
                            assert isinstance(binding, Tracked) and binding.label == '0'
                            del binding, traceback
                        failure.__traceback__ = None
                        record('traceback cleared')
                    else:
                        raise AssertionError('unsupported item assignment did not fail')
                    result = None
                else:
                    result = function(make, record, target_type, key)
                if {case!r} == 'suspended_target':
                    assert next(result) == 'ready'
                    assert result.send(make(2)) == 'done'
                    result.close()
                elif {case!r} == 'suspended_live_operand':
                    assert next(result) == 'key'
                    assert result.send(None) == 'done'
                    result.close()
                else:
                    assert result is None
                assert sys.exception() is marker
            del result
            gc.collect()
            assert live[0] == 0, (events, live)
            # Explicit callbacks and handler order remain exact. Temporary
            # owners and implicit destructor order are engine-specific.
            semantic = [
                event if event[0] == 'store' else (event[0], event[-1])
                for event in events if event[0] != 'drop'
            ]
            cleanup = sorted(event[1] for event in events if event[0] == 'drop')
            return semantic, cleanup

        # Preserve the original failing-target case, and exercise the same
        # source's successful STORE_SUBSCR cleanup without retaining the value.
        for successful in ((False, True) if {case!r} == 'subscript_target' else (False,)):
            expected = exercise(ordinary, successful)
            observed = exercise(function, successful)
            assert observed == expected, (successful, observed, expected)
        """
    modes = (
        ("none", "profile", "apply", "verify")
        if case in _SETITEM_LIFETIME_CASES
        and not entry_interpreter
        else ("none",)
    )
    work = tmp_path / "assignment-operand-profile"
    for mode in modes:
        project.run(
            program,
            entry_interpreter=entry_interpreter,
            opt_mode=mode,
            extra_env={
                "SOAC_WORK_DIR": str(work),
                "SOAC_LOG": f"soac_jit_codegen=debug;json={tmp_path / f'operand-{mode}.jsonl'}",
            },
        )
    if "profile" in modes:
        assert (work / "profile.bin").is_file()
        assert (work / "verify.bin").is_file()


_AUGMENTED_AWAIT_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

async def augmented_name(make, wait):
    value = make()
    value += await wait
    return value

async def augmented_attribute(make, wait):
    make().value += await wait

async def augmented_subscript(make, wait):
    make()[0] += await wait
"""


@pytest.fixture(scope="module")
def augmented_await_project(tmp_path_factory):
    return _operand_lifetime_project(
        tmp_path_factory, "strict-augmented-await", _AUGMENTED_AWAIT_SOURCE
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("target", ["name", "attribute", "subscript"])
@pytest.mark.parametrize("outcome", ["complete", "fail", "close"])
def test_augmented_await_retires_each_operand_once(
    augmented_await_project, entry_interpreter, target, outcome
):
    augmented_await_project.run(
        f"""
        import ctypes
        import gc
        import sys
        import operand_model
        import ordinary_operand_model
        from soac import _soac_ext

        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        function = getattr(operand_model, 'augmented_' + {target!r})
        ordinary = getattr(ordinary_operand_model, 'augmented_' + {target!r})
        assert owner(function) and not owner(ordinary)
        assert _soac_ext.strict_module_diagnostics(operand_model)['sealed']
        assert _soac_ext.strict_module_diagnostics(ordinary_operand_model) is None

        def exercise(function):
            events = []
            live = [0, 0]

            def context():
                error = sys.exception()
                return None if error is None else type(error).__name__

            class Value:
                def __init__(self):
                    live[0] += 1
                def __iadd__(self, other):
                    events.append(('add', other, context()))
                    return 'updated'
                def __del__(self):
                    live[0] -= 1
                    events.append(('drop-value', context()))

            class Target:
                def __init__(self):
                    live[1] += 1
                @property
                def value(self):
                    events.append(('get', context()))
                    return Value()
                @value.setter
                def value(self, value):
                    assert value == 'updated'
                    events.append(('set', value, context()))
                def __getitem__(self, key):
                    assert key == 0
                    return self.value
                def __setitem__(self, key, value):
                    assert key == 0
                    self.value = value
                def __del__(self):
                    live[1] -= 1
                    events.append(('drop-target', context()))

            class Wait:
                def __await__(self):
                    events.append(('wait', context()))
                    yield 'ready'
                    if {outcome!r} == 'fail':
                        raise LookupError('await failed')
                    return 4

            try:
                raise KeyError('caller')
            except KeyError as caller:
                coroutine = function(Value if {target!r} == 'name' else Target, Wait())
                assert coroutine.send(None) == 'ready'
                assert live == [1, int({target!r} != 'name')]
                if {outcome!r} == 'close':
                    coroutine.close()
                    events.append(('closed', context()))
                else:
                    try:
                        coroutine.send(None)
                    except StopIteration as done:
                        assert {outcome!r} == 'complete'
                        assert done.value == ('updated' if {target!r} == 'name' else None)
                        events.append(('complete', done.value, context()))
                    except LookupError as failure:
                        assert {outcome!r} == 'fail' and str(failure) == 'await failed'
                        failure.__traceback__ = None
                        events.append(('failed', context()))
                    else:
                        raise AssertionError('coroutine unexpectedly suspended twice')
                assert sys.exception() is caller
                del coroutine
            gc.collect()
            assert live == [0, 0], (events, live)
            return (
                [event for event in events if not event[0].startswith('drop-')],
                sorted(event[0] for event in events if event[0].startswith('drop-')),
            )

        expected = exercise(ordinary)
        observed = exercise(function)
        assert observed == expected, (observed, expected)
        """,
        entry_interpreter=entry_interpreter,
    )


_SETATTR_OPERAND_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def attribute_assignment(target, make):
    target.value = make()
"""


_SETATTR_OPERAND_OBSERVER = """
def observe_attribute_assignment(function, outcome, *, native_schedule=False):
    import gc
    import sys
    import weakref

    events = []
    values = []
    caller = KeyError('caller handler')
    failure = LookupError('setter failed')

    def context():
        current = sys.exception()
        if current is caller:
            return 'caller'
        if current is failure:
            return 'setter-error'
        return None if current is None else type(current).__name__

    class Payload:
        def __del__(self):
            events.append(('drop-value', context()))

    class Target:
        def __setattr__(self, name, value):
            assert name == 'value'
            assert values[0]() is value
            count = sys.getrefcount(value) if native_schedule else None
            events.append(('set', count, context()))
            if outcome == 'error':
                raise failure

        def __del__(self):
            events.append(('drop-target', context()))

    def make():
        value = Payload()
        values.append(weakref.ref(value))
        events.append(('made', context()))
        return value

    try:
        raise caller
    except KeyError:
        try:
            result = function(Target(), make)
        except LookupError as caught:
            assert outcome == 'error' and caught is failure
            assert caught.__context__ is caller
            events.append(('error', context(), values[0]() is not None))
            caught.__traceback__ = None
            events.append(('traceback-cleared', context(), values[0]() is not None))
        else:
            assert outcome == 'success' and result is None
            events.append(('returned', context(), values[0]() is not None))
        assert sys.exception() is caller
        events.append(('after-call', context(), values[0]() is not None))
    gc.collect()
    events.append(('after-handler', context(), values[0]() is not None))
    return events

def attribute_assignment_semantics(events):
    labels = [event[0] for event in events]
    assert labels.count('drop-value') == labels.count('drop-target') == 1, events
    assert events[-1] == ('after-handler', None, False), events
    return [
        (event[0], event[-1]) if event[0] == 'set' else event[:2]
        for event in events if not event[0].startswith('drop-')
    ]
"""


@pytest.fixture(scope="module")
def setattr_operand_project(tmp_path_factory):
    return _operand_lifetime_project(
        tmp_path_factory, "strict-setattr-operand", _SETATTR_OPERAND_SOURCE
    )


@pytest.mark.parametrize("outcome", ["success", "error"])
def test_native_attribute_assignment_replacement_ownership(outcome):
    namespace = {}
    exec(_SETATTR_OPERAND_SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1), namespace)
    exec(_SETATTR_OPERAND_OBSERVER, namespace)
    events = namespace["observe_attribute_assignment"](
        namespace["attribute_assignment"], outcome, native_schedule=True
    )
    labels = [event[0] for event in events]
    assert labels[:2] == ["made", "set"]
    assert labels.count("set") == 1
    assert labels.count("drop-value") == labels.count("drop-target") == 1
    assert events[-2:] == [("after-call", "caller", False), ("after-handler", None, False)]
    if outcome == "success":
        assert labels.index("drop-value") < labels.index("drop-target") < labels.index("returned")
    else:
        assert ("error", "setter-error", True) in events
        assert labels.index("error") < labels.index("drop-value") < labels.index("traceback-cleared")
        assert labels.index("error") < labels.index("drop-target") < labels.index("traceback-cleared")


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("outcome", ["success", "error"])
def test_attribute_assignment_preserves_callbacks_and_cleans_up(
    setattr_operand_project, entry_interpreter, outcome
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    program = "\n".join(
        [
            "import ctypes",
            "import operand_model as actual",
            "import ordinary_operand_model as ordinary",
            "from soac import _soac_ext",
            "owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner",
            "owner.argtypes = [ctypes.py_object]",
            "owner.restype = ctypes.c_void_p",
            "assert owner(actual.attribute_assignment)",
            "assert not owner(ordinary.attribute_assignment)",
            "assert _soac_ext.strict_module_diagnostics(actual)['sealed']",
            "assert _soac_ext.strict_module_diagnostics(ordinary) is None",
            f"assert _soac_ext.strict_function_entry_kind(actual.attribute_assignment) == {expected_entry!r}",
            _SETATTR_OPERAND_OBSERVER,
            f"expected = observe_attribute_assignment(ordinary.attribute_assignment, {outcome!r})",
            f"observed = observe_attribute_assignment(actual.attribute_assignment, {outcome!r})",
            "assert attribute_assignment_semantics(observed) == attribute_assignment_semantics(expected), (observed, expected)",
        ]
    )
    setattr_operand_project.run(program, entry_interpreter=entry_interpreter)


_SETATTR_CAPTURE_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def captured_receiver(first, second, make):
    first().value = make()

def chained_receivers(first, second, make):
    first().value = second().value = make()
"""


_SETATTR_CAPTURE_OBSERVER = """
def observe_captured_attribute_assignment(function, case, outcome, *, native_schedule=False):
    import gc
    import sys
    import weakref

    events = []
    values = []
    targets = []
    caller = KeyError('caller handler')
    failure = LookupError('assignment failed')
    failing_receiver = 'second' if case == 'chained_receivers' else 'first'

    def context():
        current = sys.exception()
        if current is caller:
            return 'caller'
        if current is failure:
            return 'assignment-error'
        return None if current is None else type(current).__name__

    def alive():
        return (values[0]() is not None, tuple(reference() is not None for reference in targets))

    class Payload:
        def __del__(self):
            events.append(('drop-value', context()))

    class Target:
        def __init__(self, label):
            object.__setattr__(self, 'label', label)

        def __setattr__(self, name, value):
            assert name == 'value'
            assert values[0]() is value
            assert any(reference() is self for reference in targets)
            self_count = sys.getrefcount(self) if native_schedule else None
            value_count = sys.getrefcount(value) if native_schedule else None
            events.append(('set', self.label, self_count, value_count, context()))
            if outcome == 'setter-error' and self.label == failing_receiver:
                raise failure

        def __del__(self):
            events.append(('drop-target', self.label, context()))

    def make():
        value = Payload()
        values.append(weakref.ref(value))
        events.append(('made', context()))
        return value

    def receiver(label):
        events.append(('receiver', label, context()))
        if outcome == 'receiver-error' and label == failing_receiver:
            raise failure
        value = Target(label)
        targets.append(weakref.ref(value))
        return value

    def first():
        return receiver('first')

    def second():
        return receiver('second')

    try:
        raise caller
    except KeyError:
        try:
            result = function(first, second, make)
        except LookupError as caught:
            assert outcome != 'success' and caught is failure
            assert caught.__context__ is caller
            events.append(('error', context(), alive()))
            # The Python setter's own traceback legitimately retains self and
            # value. Remove it at the same explicit point in both executions.
            caught.__traceback__ = None
            events.append(('traceback-cleared', context(), alive()))
        else:
            assert outcome == 'success' and result is None
            events.append(('returned', context(), alive()))
        assert sys.exception() is caller
        events.append(('after-call', context(), alive()))
    gc.collect()
    events.append(('after-handler', context(), alive()))
    return events

def captured_attribute_assignment_semantics(events):
    final = events[-1]
    assert final[:2] == ('after-handler', None), events
    assert final[2][0] is False and not any(final[2][1]), events
    assert sum(event[0] == 'drop-value' for event in events) == 1, events
    target_drops = [event[1] for event in events if event[0] == 'drop-target']
    assert len(target_drops) == len(set(target_drops)) == len(final[2][1]), events
    semantic = []
    for event in events:
        if event[0].startswith('drop-'):
            continue
        if event[0] == 'set':
            semantic.append((event[0], event[1], event[-1]))
        elif event[0] in {'returned', 'error', 'traceback-cleared', 'after-call', 'after-handler'}:
            semantic.append(event[:2])
        else:
            semantic.append(event)
    return semantic, sorted(target_drops)
"""


@pytest.fixture(scope="module")
def setattr_capture_project(tmp_path_factory):
    return _operand_lifetime_project(
        tmp_path_factory, "strict-setattr-captured-operands", _SETATTR_CAPTURE_SOURCE
    )


@pytest.mark.parametrize("case", ["captured_receiver", "chained_receivers"])
@pytest.mark.parametrize("outcome", ["success", "receiver-error", "setter-error"])
def test_native_attribute_assignment_captured_owners(case, outcome):
    namespace = {}
    exec(_SETATTR_CAPTURE_SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1), namespace)
    exec(_SETATTR_CAPTURE_OBSERVER, namespace)
    events = namespace["observe_captured_attribute_assignment"](
        namespace[case], case, outcome, native_schedule=True
    )
    sets = [event for event in events if event[0] == "set"]
    expected_values = [3, 2] if case == "chained_receivers" else [2]
    if outcome == "receiver-error":
        expected_values.pop()
    # The observer's identity-check genexpr captures self in a cell. Loading
    # that cell for getrefcount owns a reference in addition to STORE_ATTR's
    # receiver and the cell; this is not a borrowed local-argument load.
    assert [event[2] for event in sets] == [3] * len(sets)
    assert [event[3] for event in sets] == expected_values
    assert events[0] == ("made", "caller")
    assert events[-2][0:2] == ("after-call", "caller")
    assert events[-1][0:2] == ("after-handler", None)
    assert events[-1][2][0] is False
    assert not any(events[-1][2][1])
    assert sum(event[0] == "drop-value" for event in events) == 1
    if outcome == "success":
        target_drops = [index for index, event in enumerate(events) if event[0] == "drop-target"]
        value_drop = next(index for index, event in enumerate(events) if event[0] == "drop-value")
        returned = next(index for index, event in enumerate(events) if event[0] == "returned")
        assert target_drops and max(target_drops) < value_drop < returned
    elif outcome == "receiver-error":
        # No Python setter frame owns the RHS on this edge; it must unwind
        # before the caller catches the factory's error, without a GC cycle.
        value_drop = next(index for index, event in enumerate(events) if event[0] == "drop-value")
        caught = next(index for index, event in enumerate(events) if event[0] == "error")
        assert value_drop < caught


def test_native_attribute_assignment_source_local_uses_borrowed_rhs():
    import sys

    observed = []

    class Target:
        def __setattr__(self, name, value):
            observed.append(sys.getrefcount(value))

    def source_local(target, value):
        before = sys.getrefcount(value)
        target.value = value
        return before

    before = source_local(Target(), object())
    # The setter's Python argument is the one extra owner; there is no owned
    # expression-stack COPY of the existing source local.
    assert observed == [before + 1]


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_attribute_assignment_captured_receivers_preserve_identity_and_cleanup(
    setattr_capture_project, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    program = "\n".join(
        [
            "import ctypes",
            "import operand_model as actual",
            "import ordinary_operand_model as ordinary",
            "from soac import _soac_ext",
            "owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner",
            "owner.argtypes = [ctypes.py_object]",
            "owner.restype = ctypes.c_void_p",
            "assert _soac_ext.strict_module_diagnostics(actual)['sealed']",
            "assert _soac_ext.strict_module_diagnostics(ordinary) is None",
            _SETATTR_CAPTURE_OBSERVER,
            "mismatches = []",
            "for case in ('captured_receiver', 'chained_receivers'):",
            "    function = getattr(actual, case)",
            "    control = getattr(ordinary, case)",
            "    assert owner(function) and not owner(control)",
            f"    assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}",
            "    for outcome in ('success', 'receiver-error', 'setter-error'):",
            "        expected = observe_captured_attribute_assignment(control, case, outcome)",
            "        observed = observe_captured_attribute_assignment(function, case, outcome)",
            "        if captured_attribute_assignment_semantics(observed) != captured_attribute_assignment_semantics(expected):",
            "            mismatches.append((case, outcome, observed, expected))",
            "assert not mismatches, mismatches",
        ]
    )
    setattr_capture_project.run(program, entry_interpreter=entry_interpreter)


_SETATTR_BORROWED_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def source_local(target, value):
    target.value = value
"""


@pytest.fixture(scope="module")
def setattr_borrowed_project(tmp_path_factory):
    return _operand_lifetime_project(
        tmp_path_factory, "strict-setattr-source-locals", _SETATTR_BORROWED_SOURCE
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_attribute_assignment_source_local_preserves_identity_and_cleanup(
    setattr_borrowed_project, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    program = "\n".join(
        [
            "import ctypes",
            "import operand_model as actual",
            "import ordinary_operand_model as ordinary",
            "from soac import _soac_ext",
            "owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner",
            "owner.argtypes = [ctypes.py_object]",
            "owner.restype = ctypes.c_void_p",
            "assert owner(actual.source_local) and not owner(ordinary.source_local)",
            "assert _soac_ext.strict_module_diagnostics(actual)['sealed']",
            "assert _soac_ext.strict_module_diagnostics(ordinary) is None",
            f"assert _soac_ext.strict_function_entry_kind(actual.source_local) == {expected_entry!r}",
            _SETATTR_OPERAND_OBSERVER,
            "def adapt(function):",
            "    def invoke(target, make):",
            "        return function(target, make())",
            "    return invoke",
            "mismatches = []",
            "for outcome in ('success', 'error'):",
            "    expected = observe_attribute_assignment(adapt(ordinary.source_local), outcome)",
            "    observed = observe_attribute_assignment(adapt(actual.source_local), outcome)",
            "    if attribute_assignment_semantics(observed) != attribute_assignment_semantics(expected):",
            "        mismatches.append((outcome, observed, expected))",
            "assert not mismatches, mismatches",
        ]
    )
    setattr_borrowed_project.run(program, entry_interpreter=entry_interpreter)


_AUGMENTED_OPERAND_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def local_target(start, update, target, key, record):
    value = start()
    try:
        value += update()
    except (LookupError, OSError):
        record("handler")
    else:
        record("after")
        del value
    record("end")

def attribute_target(start, update, target, key, record):
    try:
        target().field += update()
    except (LookupError, OSError):
        record("handler")
    else:
        record("after")
    record("end")

def subscript_target(start, update, target, key, record):
    try:
        target()[key()] += update()
    except (LookupError, OSError):
        record("handler")
    else:
        record("after")
    record("end")
"""


@pytest.fixture(scope="module")
def augmented_operand_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-augmented-operands"),
        {
            "augmented_operand_model.py": _AUGMENTED_OPERAND_SOURCE,
            "ordinary_augmented_operand_model.py": _AUGMENTED_OPERAND_SOURCE.replace(
                "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1
            ),
        },
        modules={"augmented_operand_model": "augmented_operand_model.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("case", ["local_target", "attribute_target", "subscript_target"])
def test_augmented_operands_preserve_callbacks_handlers_and_cleanup(
    augmented_operand_project, entry_interpreter, case
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    augmented_operand_project.run(
        f"""
        import ctypes
        import gc
        import sys
        import augmented_operand_model
        import ordinary_augmented_operand_model
        from soac import _soac_ext

        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        function = getattr(augmented_operand_model, {case!r})
        ordinary = getattr(ordinary_augmented_operand_model, {case!r})
        assert owner(function) and not owner(ordinary)
        assert _soac_ext.strict_module_diagnostics(augmented_operand_model)['sealed']
        assert _soac_ext.strict_module_diagnostics(ordinary_augmented_operand_model) is None
        assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}

        def exercise(function, outcome):
            events = []
            live = [0]
            def record(*event):
                current = sys.exception()
                events.append((*event, None if current is None else type(current).__name__))
            class Value:
                def __init__(self, label):
                    self.label = label
                    live[0] += 1
                def __iadd__(self, other):
                    record('iadd', self.label, other.label)
                    if outcome == 'operation_error':
                        raise LookupError('operation failed')
                    return self if outcome == 'inplace' else Value('result')
                def __del__(self):
                    live[0] -= 1
                    record('drop', self.label)
            class Target:
                def __init__(self):
                    live[0] += 1
                @property
                def field(self):
                    record('get')
                    return Value('old')
                @field.setter
                def field(self, value):
                    record('set', value.label)
                    if outcome == 'target_error':
                        raise OSError('target failed')
                def __getitem__(self, key):
                    record('getitem')
                    return Value('old')
                def __setitem__(self, key, value):
                    record('setitem', value.label)
                    if outcome == 'target_error':
                        raise OSError('target failed')
                def __del__(self):
                    live[0] -= 1
                    record('drop', 'target')
            class Key:
                def __init__(self):
                    live[0] += 1
                def __del__(self):
                    live[0] -= 1
                    record('drop', 'key')
            try:
                raise KeyError('caller handler')
            except KeyError as marker:
                function(lambda: Value('old'), lambda: Value('rhs'), Target, Key, record)
                assert sys.exception() is marker
            gc.collect()
            assert live[0] == 0, (outcome, events, live)
            return (
                [event for event in events if event[0] != 'drop'],
                sorted(event[1] for event in events if event[0] == 'drop'),
            )

        for outcome in ('replacement', 'inplace', 'operation_error', 'target_error'):
            expected = exercise(ordinary, outcome)
            observed = exercise(function, outcome)
            assert observed == expected, (outcome, observed, expected)
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("kind", ["generator", "coroutine", "async_generator"])
def test_closing_one_frame_keeps_another_frames_original_private_cells(
    private_class_capture_project, entry_interpreter, kind
):
    factory_name = f"private_{kind}_family"
    private_class_capture_project.run_case(
        "private_capture_model",
        textwrap.dedent(f"""
        def validate(module):
            import ctypes
            import gc
            import weakref
            import ordinary_private_capture
            from soac import _soac_ext

            def finish_awaitable(awaitable):
                iterator = awaitable.__await__()
                try:
                    unexpected = next(iterator)
                except StopIteration as completed:
                    return completed.value
                raise AssertionError(("unexpected async suspension", unexpected))

            def start(frame):
                if {kind!r} == "async_generator":
                    assert finish_awaitable(frame.__anext__()) is None
                elif {kind!r} == "coroutine":
                    assert frame.send(None) == "paused"
                else:
                    assert next(frame) is None

            def close(frame):
                if {kind!r} == "async_generator":
                    assert finish_awaitable(frame.aclose()) is None
                else:
                    frame.close()

            def code_of(frame):
                if {kind!r} == "async_generator":
                    return frame.ag_code
                if {kind!r} == "coroutine":
                    return frame.cr_code
                return frame.gi_code

            owner = ctypes.pythonapi.PyType_GetSoacContractOwner
            owner.argtypes = [ctypes.py_object]
            owner.restype = ctypes.c_void_p
            ordinary_target, ordinary_create = getattr(ordinary_private_capture, {factory_name!r})()
            target, create = getattr(module, {factory_name!r})()
            assert create.__code__.co_freevars == ordinary_create.__code__.co_freevars == ()
            assert create.__closure__ is ordinary_create.__closure__ is None
            assert create.__annotate__ is ordinary_create.__annotate__ is None
            assert _soac_ext.strict_function_entry_kind(create) == "generator_factory"
            original_code = create.__code__
            target_ref, create_ref = weakref.ref(target), weakref.ref(create)
            first, second = create(), create()
            del ordinary_target, ordinary_create, target, create
            start(first)
            start(second)
            close(first)
            assert code_of(first) is original_code
            gc.collect()
            assert target_ref() is not None and create_ref() is not None, "one close cleared another frame's cells"

            if {kind!r} == "async_generator":
                holder = finish_awaitable(second.__anext__())
            elif {kind!r} == "coroutine":
                try:
                    second.send(None)
                except StopIteration as completed:
                    holder = completed.value
                else:
                    raise AssertionError("coroutine did not return its class")
            else:
                holder = next(second)
            assert owner(holder)
            instance = holder(target_ref()())
            assert type(instance.payload) is target_ref()
            try:
                holder(object())
            except TypeError:
                pass
            else:
                raise AssertionError("the second frame lost its required field predicate")
            close(second)
            assert code_of(first) is code_of(second) is original_code
            holder_ref = weakref.ref(holder)
            del holder, instance
            gc.collect()
            assert holder_ref() is None
            assert create_ref() is None, "closed wrappers retained their source function"
            assert target_ref() is None, "closed wrappers retained their private target"
        """),
        private_class_capture_project.project / "private_capture_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=(factory_name,),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("kind", ["generator", "coroutine", "async_generator"])
def test_terminal_source_owners_preserve_outer_handler_and_release_payloads(
    private_class_capture_project, entry_interpreter, kind
):
    factory_name = f"terminal_{kind}_family"
    private_class_capture_project.run_case(
        "private_capture_model",
        textwrap.dedent(f"""
        def validate(module):
            import gc
            import sys
            import weakref
            import ordinary_private_capture
            from soac import _soac_ext

            def finish_awaitable(awaitable):
                iterator = awaitable.__await__()
                try:
                    unexpected = next(iterator)
                except StopIteration as completed:
                    return completed.value
                raise AssertionError(("unexpected async suspension", unexpected))

            def exercise(factory, termination):
                events = []
                class Payload:
                    def __del__(self):
                        current = sys.exception()
                        events.append((type(current).__name__, str(current)))
                payload = Payload()
                create = factory(payload)
                assert create.__code__.co_freevars == ("payload",)
                payload_ref, create_ref = weakref.ref(payload), weakref.ref(create)
                original_code = create.__code__
                frame = create()
                del payload, create
                if {kind!r} == "async_generator":
                    assert finish_awaitable(frame.__anext__()) is None
                elif {kind!r} == "coroutine":
                    assert frame.send(None) == "paused"
                else:
                    assert next(frame) is None
                assert payload_ref() is not None
                try:
                    raise ValueError("surrounding caller")
                except ValueError as outer:
                    if termination == "close":
                        if {kind!r} == "async_generator":
                            assert finish_awaitable(frame.aclose()) is None
                        else:
                            frame.close()
                    else:
                        try:
                            if {kind!r} == "async_generator":
                                finish_awaitable(frame.__anext__())
                            else:
                                frame.send(None)
                        except (StopIteration, StopAsyncIteration):
                            pass
                        else:
                            raise AssertionError("suspended function did not finish")
                    assert sys.exception() is outer
                    gc.collect()
                    assert len(events) == 1, (termination, events)
                    assert payload_ref() is None, "terminal frame retained its captured payload"
                    assert create_ref() is None, "terminal wrapper retained its source function"
                if {kind!r} == "async_generator":
                    assert frame.ag_code is original_code
                elif {kind!r} == "coroutine":
                    assert frame.cr_code is original_code
                else:
                    assert frame.gi_code is original_code
                # Finalization is required exactly once; its implicit-release
                # handler context is not compared between execution engines.
                return len(events)

            actual_factory = getattr(module, {factory_name!r})
            ordinary_factory = getattr(ordinary_private_capture, {factory_name!r})
            for termination in ("close", "complete"):
                expected = exercise(ordinary_factory, termination)
                actual = exercise(actual_factory, termination)
                assert actual == expected
        """),
        private_class_capture_project.project / "private_capture_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=(factory_name,),
    )


_PREFIXED_SOURCE_BINDINGS = """
# soac: module(strict_assign=true, checked_attr=true)

_dp_module_value = 40

def make_prefixed(_dp_parameter):
    _dp_local = _dp_parameter + 1

    def read():
        return _dp_parameter, _dp_local, _dp_module_value

    def replace(value):
        nonlocal _dp_local
        _dp_local = value

    def clear():
        nonlocal _dp_local
        del _dp_local

    class Box:
        _dp_field = _dp_parameter

        def read(self):
            return _dp_parameter, _dp_local

    def expressions():
        return ((_dp_parameter, _dp_local, item) for item in (1, 2))

    return read, replace, clear, Box, expressions
"""


@pytest.fixture(scope="module")
def prefixed_source_bindings_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-prefixed-source-bindings"),
        {
            "prefixed_model.py": _PREFIXED_SOURCE_BINDINGS,
            "ordinary_prefixed_model.py": _PREFIXED_SOURCE_BINDINGS.replace(
                "# soac: module(strict_assign=true, checked_attr=true)", ""
            ),
        },
        modules={"prefixed_model": "prefixed_model.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_source_prefixed_bindings_keep_ordinary_closure_and_namespace_semantics(
    prefixed_source_bindings_project, entry_interpreter
):
    prefixed_source_bindings_project.run_case(
        "prefixed_model",
        """
def validate(module):
    import ordinary_prefixed_model

    def exercise(factory):
        read, replace, clear, Box, expressions = factory(_dp_parameter=3)
        assert read.__code__.co_freevars == ("_dp_local", "_dp_parameter")
        assert Box.read.__code__.co_freevars == ("_dp_local", "_dp_parameter")
        assert Box._dp_field == 3
        assert read() == (3, 4, 40)
        assert Box().read() == (3, 4)
        assert list(expressions()) == [(3, 4, 1), (3, 4, 2)]
        replace(9)
        assert read() == (3, 9, 40)
        assert Box().read() == (3, 9)
        assert list(expressions()) == [(3, 9, 1), (3, 9, 2)]
        clear()
        try:
            read()
        except NameError:
            pass
        else:
            raise AssertionError("the source nonlocal delete was ignored")
        replace(12)
        assert read() == (3, 12, 40)
        return Box().read(), list(expressions())

    expected = exercise(ordinary_prefixed_model.make_prefixed)
    assert exercise(module.make_prefixed) == expected
""",
        prefixed_source_bindings_project.project / "prefixed_model.py",
        entry_interpreter=entry_interpreter,
        required_functions=("make_prefixed",),
    )


# Public argument ownership and cleanup, with a separate ordinary-CPython
# schedule control. SOAC comparisons exclude transient counts and opcode choice.

_SOURCE_ARGUMENT_OWNERSHIP_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def argument_keep(value, probe, make, finish):
    probe("entered")
    finish()
    probe("before-return")

def argument_delete(value, probe, make, finish):
    probe("entered")
    del value
    probe("after-delete")
    finish()
    probe("before-return")

def argument_rebind(value, probe, make, finish):
    probe("entered")
    value = make("replacement")
    probe("after-rebind")
    finish()
    probe("before-return")

def argument_alias(value, probe, make, finish):
    probe("entered")
    alias = value
    probe("aliased")
    del value
    probe("after-delete")
    finish()
    probe("before-return")

def argument_expanded(value, *, keyword, probe, finish):
    probe("entered")
    del value
    del keyword
    probe("after-delete")
    finish()
    probe("before-return")

def retire_arguments(first, second):
    return None
"""


_SOURCE_ARGUMENT_OWNERSHIP_NAMES = (
    "argument_keep",
    "argument_delete",
    "argument_rebind",
    "argument_alias",
    "argument_expanded",
)


_SOURCE_ARGUMENT_OWNERSHIP_CASES = (
    tuple(
        (name, caller, outcome, 0)
        for name in _SOURCE_ARGUMENT_OWNERSHIP_NAMES[:-1]
        for caller in ("factory", "local")
        for outcome in ("success", "error")
    )
    + tuple(
        ("argument_keep", "borrowed-c", outcome, 0) for outcome in ("success", "error")
    )
    + tuple(
        ("argument_expanded", "expanded", outcome, warmups)
        for warmups in (0, 64)
        for outcome in ("success", "error")
    )
)


_SOURCE_ARGUMENT_OWNERSHIP_OBSERVER = r"""
def observe_source_argument_ownership(
    function, caller_kind, outcome, warmups, *, native_schedule=False,
):
    import dis
    import gc
    import sys
    import weakref

    events = []
    references = {}
    caller_error = KeyError("caller handler")
    failure = LookupError("source failure")
    measuring = False
    labels = ("input", "replacement", "keyword")

    def context():
        current = sys.exception()
        if current is caller_error:
            return "caller"
        if current is failure:
            return "failure"
        return None if current is None else type(current).__name__

    def snapshot():
        values = []
        for label in labels:
            reference = references.get(label)
            value = None if reference is None else reference()
            # The ordinary observer's temporary strong reference is the only
            # observer edge. No observed payload is passed into a source-call
            # argument, so outbound argument preparation cannot inflate this.
            count = 0 if value is None else sys.getrefcount(value) - 1 if native_schedule else 1
            values.append((label, count))
            del value
        return tuple(values)

    def probe(label):
        events.append(("probe", label, snapshot(), context()))

    class Payload:
        def __init__(self, label):
            self.label = label
        def __del__(self):
            events.append(("drop", self.label, context()))

    def make(label):
        value = Payload(label)
        references[label] = weakref.ref(value)
        events.append(("made", label, context()))
        return value

    def finish():
        events.append(("finish", context()))
        if measuring and outcome == "error":
            raise failure

    # A keyword-name subclass is owned by the transient kwargs/kwnames
    # containers but is not the canonical string stored in the callee's code.
    # Its destructor exposes container retirement without a referrer snapshot.
    class Keyword(str):
        def __del__(self):
            events.append(("drop-key", context()))

    def positionals():
        return (make("input"),)

    def keywords():
        return {Keyword("keyword"): make("keyword")}

    # Fresh ordinary code gives the cold case a genuinely cold CALL_FUNCTION_EX
    # site. Only this caller is compiled here; `function` is the actual
    # published source function, never a replacement or synthetic grant.
    caller_namespace = {}
    exec(compile(
        "def invoke(function, positionals, keywords, probe, finish):\n"
        "    return function(*positionals(), probe=probe, finish=finish, **keywords())\n",
        "<ordinary expanded source caller>", "exec", dont_inherit=True,
    ), caller_namespace)
    invoke_expanded = caller_namespace["invoke"]

    def expanded_opcode():
        instructions = [
            instruction.opname
            for instruction in dis.get_instructions(invoke_expanded, adaptive=True)
            if instruction.opname in {
                "CALL_FUNCTION_EX", "CALL_EX_PY", "CALL_EX_NON_PY_GENERAL",
                "INSTRUMENTED_CALL_FUNCTION_EX",
            }
        ]
        assert len(instructions) == 1, instructions
        return instructions[0]

    def invoke():
        if caller_kind == "factory":
            return function(make("input"), probe, make, finish)
        if caller_kind == "local":
            value = make("input")
            try:
                return function(value, probe, make, finish)
            finally:
                probe("caller-before-release")
                del value
                probe("caller-after-release")
        if caller_kind == "borrowed-c":
            import _testcapi
            arguments = (make("input"), probe, make, finish)
            try:
                # This existing helper borrows the tuple's element array and
                # calls the public PyObject_Vectorcall. It does not grant an
                # interpreter-owned source-stack transfer.
                return _testcapi.pyobject_vectorcall(function, arguments, None)
            finally:
                probe("caller-before-release")
                assert references["input"]() is not None
                assert arguments[0] is references["input"]()
                del arguments
                probe("caller-after-release")
        assert caller_kind == "expanded"
        return invoke_expanded(function, positionals, keywords, probe, finish)

    call_shape = None
    try:
        raise caller_error
    except KeyError:
        if caller_kind == "expanded":
            if native_schedule:
                # Exact opcode/container observations belong only to the
                # ordinary CPython control, never to SOAC's calling convention.
                assert sys.gettrace() is None and sys.getprofile() is None
                call_events = (
                    sys.monitoring.events.CALL
                    | sys.monitoring.events.C_RETURN
                    | sys.monitoring.events.C_RAISE
                )
                for tool in range(6):
                    if sys.monitoring.get_tool(tool) is not None:
                        assert not sys.monitoring.get_events(tool) & call_events
            for _ in range(warmups):
                assert invoke() is None
                gc.collect()
                assert not any(reference() is not None for reference in references.values())
            if native_schedule:
                call_shape = expanded_opcode()
                if warmups:
                    assert call_shape in {"CALL_EX_PY", "CALL_EX_NON_PY_GENERAL"}, call_shape
                else:
                    assert call_shape == "CALL_FUNCTION_EX", call_shape
            events.clear()
            references.clear()
        else:
            assert warmups == 0
        measuring = True
        try:
            result = invoke()
        except LookupError as caught:
            assert outcome == "error" and caught is failure
            assert caught.__context__ is caller_error
            probe("caught")
            # A retained native source frame may own a surviving source alias
            # or replacement. Clear the traceback at the same explicit point.
            caught.__traceback__ = None
            probe("traceback-cleared")
        else:
            assert outcome == "success" and result is None
            probe("returned")
        assert sys.exception() is caller_error
        probe("after-call")
    gc.collect()
    probe("after-handler")
    return {"events": events, "expanded_call_shape": call_shape, "caller_kind": caller_kind}

def source_argument_semantics(observed):
    events = observed['events']
    final = events[-1]
    assert final[:2] == ('probe', 'after-handler') and final[3] is None, events
    assert not any(dict(final[2]).values()), ('dead argument retained after collection', events)
    made = sorted(event[1] for event in events if event[0] == 'made')
    dropped = sorted(event[1] for event in events if event[0] == 'drop')
    assert made == dropped, ('missing or duplicate required finalizer', events)
    assert sum(event[0] == 'drop-key' for event in events) == int(observed['caller_kind'] == 'expanded'), events
    assert sum(event[0] == 'finish' for event in events) == 1, events
    return [
        (event[0], event[1], event[3]) if event[0] == 'probe' else event
        for event in events if event[0] not in {'drop', 'drop-key'}
    ]
"""


def _source_argument_probe(events, label):
    matching = [event for event in events if event[:2] == ("probe", label)]
    assert len(matching) == 1, (label, matching, events)
    return dict(matching[0][2]), matching[0][3]


@pytest.mark.parametrize(
    "case",
    _SOURCE_ARGUMENT_OWNERSHIP_CASES,
    ids=["-".join(map(str, case)) for case in _SOURCE_ARGUMENT_OWNERSHIP_CASES],
)
def test_native_source_argument_owner_handoff(case):
    name, caller, outcome, warmups = case
    namespace = {}
    exec(
        _SOURCE_ARGUMENT_OWNERSHIP_SOURCE.replace(
            "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1
        ),
        namespace,
    )
    exec(_SOURCE_ARGUMENT_OWNERSHIP_OBSERVER, namespace)
    observed = namespace["observe_source_argument_ownership"](
        namespace[name], caller, outcome, warmups, native_schedule=True
    )
    events = observed["events"]
    entered, context = _source_argument_probe(events, "entered")
    assert context == "caller"
    assert entered["input"] == (2 if caller == "borrowed-c" else 1)
    if name == "argument_alias":
        aliased, _ = _source_argument_probe(events, "aliased")
        assert aliased["input"] == (2 if caller == "factory" else 1)
        after_delete, _ = _source_argument_probe(events, "after-delete")
        assert after_delete["input"] == 1
    elif name == "argument_delete":
        after_delete, _ = _source_argument_probe(events, "after-delete")
        assert after_delete["input"] == (1 if caller == "local" else 0)
    elif name == "argument_rebind":
        after_rebind, _ = _source_argument_probe(events, "after-rebind")
        assert after_rebind["input"] == (1 if caller == "local" else 0)
        assert after_rebind["replacement"] == 1
    elif name == "argument_expanded":
        assert entered["keyword"] == 1
        assert events.count(("drop-key", "caller")) == 1
        assert events.index(("drop-key", "caller")) < next(
            index
            for index, event in enumerate(events)
            if event[:2] == ("probe", "entered")
        )
        after_delete, _ = _source_argument_probe(events, "after-delete")
        assert after_delete["input"] == after_delete["keyword"] == 0
        if warmups:
            assert observed["expanded_call_shape"] == "CALL_EX_PY"
    if caller == "borrowed-c" and outcome == "success":
        before_release, _ = _source_argument_probe(events, "caller-before-release")
        assert before_release["input"] == 1
    if outcome == "error":
        _, context = _source_argument_probe(events, "caught")
        assert context == "failure"
        _, context = _source_argument_probe(events, "traceback-cleared")
        assert context == "failure"
    final, context = _source_argument_probe(events, "after-handler")
    assert context is None and not any(final.values())
    assert sum(event[:2] == ("drop", "input") for event in events) == 1
    if name == "argument_rebind":
        assert sum(event[:2] == ("drop", "replacement") for event in events) == 1


@pytest.fixture(scope="module")
def source_argument_ownership_project(tmp_path_factory):
    return _operand_lifetime_project(
        tmp_path_factory,
        "strict-source-argument-ownership",
        _SOURCE_ARGUMENT_OWNERSHIP_SOURCE,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_source_arguments_preserve_callbacks_errors_and_eventual_cleanup(
    source_argument_ownership_project, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    program = "\n".join(
        [
            "import ctypes",
            "import operand_model as actual",
            "import ordinary_operand_model as ordinary",
            "from soac import _soac_ext",
            "owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner",
            "owner.argtypes = [ctypes.py_object]",
            "owner.restype = ctypes.c_void_p",
            "assert _soac_ext.strict_module_diagnostics(actual)['sealed']",
            "assert _soac_ext.strict_module_diagnostics(ordinary) is None",
            f"for name in {_SOURCE_ARGUMENT_OWNERSHIP_NAMES!r}:",
            "    function = getattr(actual, name)",
            "    assert owner(function) and not owner(getattr(ordinary, name)), name",
            f"    assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}, name",
            _SOURCE_ARGUMENT_OWNERSHIP_OBSERVER,
            "mismatches = []",
            f"for name, caller, outcome, warmups in {_SOURCE_ARGUMENT_OWNERSHIP_CASES!r}:",
            "    expected = observe_source_argument_ownership(getattr(ordinary, name), caller, outcome, warmups)",
            "    observed = observe_source_argument_ownership(getattr(actual, name), caller, outcome, warmups)",
            "    if source_argument_semantics(observed) != source_argument_semantics(expected):",
            "        mismatches.append(((name, caller, outcome, warmups), observed, expected))",
            "assert not mismatches, mismatches",
        ]
    )
    source_argument_ownership_project.run(program, entry_interpreter=entry_interpreter)


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_source_argument_cleanup_runs_finalizers_and_weakrefs_once_with_reentry(
    source_argument_ownership_project, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    source_argument_ownership_project.run(
        f"""
import ctypes
import gc
import weakref
import operand_model as actual
import ordinary_operand_model as ordinary
from soac import _soac_ext

owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert _soac_ext.strict_module_diagnostics(actual)["sealed"]
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert _soac_ext.strict_function_entry_kind(actual.retire_arguments) == {expected_entry!r}
assert owner(actual.retire_arguments) and not owner(ordinary.retire_arguments)

def observe(function):
    events = []
    references, callbacks, reentry_errors = [], [], []
    class Payload:
        def __init__(self, name):
            self.name = name
            references.append(weakref.ref(self, lambda _: callbacks.append(name)))
        def __del__(self):
            events.append(self.name)
            try:
                assert function(None, None) is None
            except BaseException as error:
                reentry_errors.append((type(error).__name__, str(error)))
    # Both arguments are fresh caller-owned values, with no tuple, saved
    # payload alias, frame introspection, or callback retaining either value.
    assert function(Payload("first"), Payload("second")) is None
    gc.collect()
    assert not reentry_errors, reentry_errors
    assert all(reference() is None for reference in references)
    assert sorted(callbacks) == ['first', 'second'], callbacks
    return events

assert observe(ordinary.retire_arguments) == ["second", "first"]
assert sorted(observe(actual.retire_arguments)) == ["first", "second"]
""",
        entry_interpreter=entry_interpreter,
    )



@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("mutation", ["same-pointer", "forwarder", "restored"])
def test_public_vectorcall_preserves_owned_continuation(
    strict_functions, entry_interpreter, mutation
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    program = (
        f"expected_entry = {expected_entry!r}\nmutation = {mutation!r}\n"
        + r"""
import ctypes
import pytest
import checked
from soac import _soac_ext

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = arguments
    function.restype = result
    return function

obj = ctypes.py_object
owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
set_vectorcall = api("PyFunction_SetVectorcall", None, obj, ctypes.c_void_p)
incref = api("Py_IncRef", None, obj)
vectorcall = api(
    "PyObject_Vectorcall", obj, obj, ctypes.POINTER(obj),
    ctypes.c_size_t, ctypes.c_void_p,
)

def ordinary(value):
    return value

function = checked.identity
assert _soac_ext.strict_module_diagnostics(checked)["sealed"]
assert owner(function) and not owner(ordinary)
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
assert function("wrong") == "wrong"
assert ordinary("wrong") == "wrong"

code, globals_, source_owner = function.__code__, function.__globals__, owner(function)
original_pointer = get_vectorcall(function)
assert original_pointer
signature = ctypes.PYFUNCTYPE(
    ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
    ctypes.c_size_t, ctypes.c_void_p,
)
original = signature(original_pointer)
argument_count_mask = (1 << (8 * ctypes.sizeof(ctypes.c_size_t) - 1)) - 1
calls, callback_errors = [], []
safe_failure_result = object()

@signature
def forward(actual, arguments, nargsf, kwnames):
    # Forward the saved checked ABI, not stock bytecode or public vectorcall
    # again. Record failures outside ctypes so it cannot swallow an exception
    # and return an undefined pointer; successful results transfer unchanged.
    try:
        calls.append((actual, nargsf & argument_count_mask, kwnames))
        result = original(actual, arguments, nargsf, kwnames)
        if result:
            return result
        callback_errors.append("saved entry returned NULL without an exception")
    except BaseException as error:
        callback_errors.append((type(error).__name__, str(error)))
    incref(safe_failure_result)
    return id(safe_failure_result)

wrapper_pointer = ctypes.cast(forward, ctypes.c_void_p).value

def python_caller(value):
    return function(value)

def c_caller(value):
    arguments = (obj * 1)(value)
    return vectorcall(function, arguments, 1, None)

try:
    if mutation == "same-pointer":
        set_vectorcall(function, original_pointer)
    else:
        set_vectorcall(function, wrapper_pointer)
        if mutation == "restored":
            set_vectorcall(function, original_pointer)
    # Real public entry replacement preserves source ownership and the saved
    # continuation's ordinary callable semantics.
    assert owner(function) == source_owner
    for invoke in (python_caller, c_caller):
        for value in range(32):
            result = invoke(value)
            assert not callback_errors, callback_errors
            assert result == value
    expected_calls = [(id(function), 1, None)] * 64 if mutation == "forwarder" else []
    assert calls == expected_calls, calls
    if mutation == "forwarder":
        assert get_vectorcall(function) == wrapper_pointer
        assert _soac_ext.strict_function_entry_kind(function) == "public_override"
    else:
        assert function("wrong") == "wrong"
finally:
    # Keep the ctypes callback alive until the real public entry is restored,
    # including assertion failures.
    set_vectorcall(function, original_pointer)

assert not callback_errors, callback_errors
assert function(73) == 73
assert function("wrong") == "wrong"
assert get_vectorcall(function) == original_pointer
assert function.__code__ is code and function.__globals__ is globals_
assert owner(function) == source_owner
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
"""
    )
    strict_functions.run(program, entry_interpreter=entry_interpreter)


_ANNOTATION_ONLY_RUNTIME_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import InitVar, dataclass, field
from typing import Any, cast
from annotation_runtime_support import body_error, default_value, events, factory

def echo(value: int) -> int:
    events.append(('echo', value))
    return value

def returned(value: Any) -> int:
    events.append(('returned', value))
    return cast(int, value)

def shape(first: int, /, second: int = 2, *items: int,
          named: int = 3, **extras: int):
    events.append(('shape',))
    return first, second, items, named, extras

def defaulted(value: int = default_value) -> int:
    return value

def finish(value: Any, create) -> int:
    temporary = create()
    try:
        events.append(('finish', value))
        return cast(int, value)
    finally:
        events.append(('finally',))

def fail(value: int) -> int:
    events.append(('fail', value))
    try:
        raise body_error
    finally:
        events.append(('finally',))

class Token:
    pass

class Methods:
    def accept(self, value: Token) -> Token:
        events.append(('method', value))
        return value

class Stored:
    def __init__(self):
        self.value: int = 1

    def store(self, value: int) -> None:
        events.append(('before-store', self.value))
        self.value = value
        events.append(('after-store', self.value))

@dataclass
class Record:
    value: int = field(default_factory=factory)
    seed: InitVar[int] = 0

    def __post_init__(self, seed: int) -> None:
        events.append(('post', seed))

@dataclass(slots=True)
class Slotted:
    value: int = field(default_factory=factory)
    seed: InitVar[int] = 0

    def __post_init__(self, seed: int) -> None:
        events.append(('post', seed))
"""


@pytest.fixture(
    scope="module",
    params=[
        pytest.param(("soac", False), id="compiled"),
        pytest.param(("soac", True), id="entry"),
        pytest.param(("cpython", False), id="cpython"),
    ],
)
def annotation_only_runtime(tmp_path_factory, request):
    backend, entry_interpreter = request.param
    project = create_strict_project(
        tmp_path_factory.mktemp(f"annotation-only-runtime-{request.node.name}-{backend}"),
        {
            "fields_enabled.py": _ANNOTATION_ONLY_RUNTIME_SOURCE,
            "fields_disabled.py": _ANNOTATION_ONLY_RUNTIME_SOURCE.replace(
                "checked_attr=true", "checked_attr=false", 1
            ),
            "annotation_runtime_support.py": """
from typing import Any

events = []
default_value: Any = 'ordinary default'
produced: Any = 11
body_error = LookupError('original body error')
factory_error = RuntimeError('original factory error')
fail_factory = False

def factory() -> Any:
    events.append(('factory',))
    if fail_factory:
        raise factory_error
    return produced
""",
        },
        modules={
            "fields_enabled": "fields_enabled.py",
            "fields_disabled": "fields_disabled.py",
        },
        backend=backend,
    )
    return project, entry_interpreter


def _run_annotation_only_case(fixture, module_name, validation):
    project, entry_interpreter = fixture
    checked_attr = project.policies[module_name]["checked_attr"]
    ordinary_source = _ANNOTATION_ONLY_RUNTIME_SOURCE.replace(
        "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1
    )
    project.run_case(
        module_name,
        "def validate_module(module):\n" + textwrap.indent(
            f"ordinary_source = {ordinary_source!r}\n"
            f"checked_attr = {checked_attr!r}\n" + """
import ctypes
import gc
import sys
import types
import weakref
import annotation_runtime_support as support
from soac import _soac_ext
from soac.strict import StrictMutationError

ordinary = types.ModuleType('ordinary_annotation_runtime')
sys.modules[ordinary.__name__] = ordinary
exec(compile(ordinary_source, '<ordinary-annotation-runtime>', 'exec'), vars(ordinary))

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.restype = result
    function.argtypes = arguments
    return function

obj = ctypes.py_object
function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p, obj)
class_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p, obj)
sealed = api('PyType_IsSoacSealed', ctypes.c_int, obj)
assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary) is None
for name in ('Token', 'Methods', 'Stored', 'Record', 'Slotted'):
    selected, control = getattr(module, name), getattr(ordinary, name)
    assert bool(class_owner(selected)) is checked_attr, name
    assert sealed(selected) == int(checked_attr), name
    assert not class_owner(control) and sealed(control) == 0, name
for name in ('Record', 'Slotted'):
    assert bool(function_owner(getattr(module, name).__init__)) is checked_attr
    assert not function_owner(getattr(ordinary, name).__init__)
"""
            + validation,
            "    ",
        ),
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=(
            "echo", "returned", "shape", "defaulted", "finish", "fail",
            "Methods.accept", "Stored.store", "Record.__post_init__",
            "Slotted.__post_init__",
        ),
    )


def test_annotations_do_not_add_runtime_argument_or_return_checks(annotation_only_runtime):
    _run_annotation_only_case(
        annotation_only_runtime,
        "fields_enabled",
        """
c_call = api('PyObject_Call', obj, obj, obj, obj)
marker = object()

def python_call(function, args, keywords):
    return function(*args, **keywords)

def binding_error(invoke, function, args, keywords):
    support.events.clear()
    try:
        invoke(function, args, keywords)
    except TypeError as error:
        assert type(error) is TypeError
        assert support.events == [], support.events
        return error.args
    raise AssertionError('ordinary argument binding accepted an invalid call')

for target in (ordinary, module):
    for invoke in (python_call, c_call):
        support.events.clear()
        assert invoke(target.echo, (marker,), {}) is marker
        assert invoke(target.returned, (marker,), {}) is marker
        assert invoke(target.defaulted, (), {}) is support.default_value
        assert invoke(target.Methods().accept, (marker,), {}) is marker
        actual = invoke(target.shape, (marker, marker, marker),
                        {'named': marker, 'extra': marker})
        assert actual[0] is actual[1] is actual[3] is marker
        assert actual[2] == (marker,) and actual[4] == {'extra': marker}
        assert support.events == [
            ('echo', marker), ('returned', marker), ('method', marker), ('shape',),
        ], support.events
        for name, args, keywords in (
            ('echo', (), {}),
            ('echo', (marker, marker), {}),
            ('echo', (marker,), {'unexpected': marker}),
            ('shape', (marker, marker), {'second': marker}),
        ):
            expected = binding_error(invoke, getattr(ordinary, name), args, keywords)
            assert binding_error(invoke, getattr(target, name), args, keywords) == expected
    support.events.clear()
    try:
        target.fail(marker)
    except LookupError as error:
        assert error is support.body_error
        error.__traceback__ = None
    else:
        raise AssertionError('the original body exception was lost')
    assert support.events == [('fail', marker), ('finally',)]

    released, references = [], []
    class Payload:
        def __del__(self):
            released.append('released')
    def create():
        payload = Payload()
        references.append(weakref.ref(payload))
        return payload
    support.events.clear()
    assert target.finish(marker, create) is marker
    assert support.events == [('finish', marker), ('finally',)]
    gc.collect()
    assert released == ['released'] and references[0]() is None
""",
    )


def test_annotated_calls_check_protected_fields_at_the_write(annotation_only_runtime):
    _run_annotation_only_case(
        annotation_only_runtime,
        "fields_enabled",
        """
c_setattr = api('PyObject_SetAttr', ctypes.c_int, obj, obj, obj)
c_generic_setattr = api('PyObject_GenericSetAttr', ctypes.c_int, obj, obj, obj)
c_setitem = api('PyDict_SetItem', ctypes.c_int, obj, obj, obj)
marker = object()
receiver, control = module.Stored(), ordinary.Stored()
escaped, ordinary_dictionary = vars(receiver), vars(control)

def rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('a protected field accepted a wrong value')

for setter in (setattr, object.__setattr__, c_setattr, c_generic_setattr):
    setter(control, 'value', marker)
    assert control.value is marker
    rejected(lambda: setter(receiver, 'value', marker))
    assert receiver.value == 1
for setter in (dict.__setitem__, c_setitem):
    setter(ordinary_dictionary, 'value', marker)
    assert control.value is marker
    rejected(lambda: setter(escaped, 'value', marker))
    assert receiver.value == 1

support.events.clear()
control.store(marker)
assert control.value is marker
assert support.events == [('before-store', marker), ('after-store', marker)]
support.events.clear()
rejected(lambda: receiver.store(marker))
assert support.events == [('before-store', 1)], support.events
assert receiver.value == 1
support.events.clear()
receiver.store(7)
assert support.events == [('before-store', 1), ('after-store', 7)]

for name in ('Record', 'Slotted'):
    kind = getattr(module, name)
    support.produced = marker
    support.events.clear()
    rejected(kind)
    assert support.events == [('factory',)], support.events
    support.events.clear()
    result = kind(9, marker)
    assert result.value == 9
    assert support.events == [('post', marker)], support.events
    rejected(lambda: c_generic_setattr(result, 'value', marker))
    assert result.value == 9
""",
    )


def test_dataclass_annotations_do_not_check_initvars_or_factory_results(annotation_only_runtime):
    _run_annotation_only_case(
        annotation_only_runtime,
        "fields_disabled",
        """
import dataclasses
import fields_enabled

marker = object()
support.produced = marker
for target in (ordinary, module):
    for name in ('Record', 'Slotted'):
        kind = getattr(target, name)
        support.events.clear()
        result = kind(seed=marker)
        assert result.value is marker
        assert support.events == [('factory',), ('post', marker)]
        support.events.clear()
        result = kind(marker, marker)
        assert result.value is marker
        assert support.events == [('post', marker)]
        assert [item.name for item in dataclasses.fields(kind)] == ['value']
        assert 'seed' not in (vars(result) if hasattr(result, '__dict__') else kind.__slots__)
        # Passing CPython's factory sentinel explicitly follows the original
        # generated expression; it is not a separate runtime type obligation.
        support.events.clear()
        result = kind(dataclasses._HAS_DEFAULT_FACTORY, marker)
        assert result.value is marker
        assert support.events == [('factory',), ('post', marker)]

assignments = []
class Foreign:
    def __setattr__(self, name, value):
        assignments.append((name, value))
        object.__setattr__(self, name, value)
    def __post_init__(self, seed):
        support.events.append(('foreign-post', seed))

assert _soac_ext.strict_module_diagnostics(fields_enabled)['sealed']
for target in (ordinary, module, fields_enabled):
    for name in ('Record', 'Slotted'):
        kind = getattr(target, name)
        if target is fields_enabled:
            assert class_owner(kind) and sealed(kind) == 1
            assert function_owner(kind.__init__)
        foreign = Foreign()
        assignments.clear()
        support.events.clear()
        assert kind.__init__(foreign, seed=marker) is None
        assert assignments == [('value', marker)] and foreign.value is marker
        assert support.events == [('factory',), ('foreign-post', marker)]
        assignments.clear()
        support.events.clear()
        del foreign.value
        support.fail_factory = True
        try:
            kind.__init__(foreign, seed=marker)
        except RuntimeError as error:
            assert error is support.factory_error
            error.__traceback__ = None
        else:
            raise AssertionError('the original factory exception was lost')
        finally:
            support.fail_factory = False
        assert assignments == [] and not hasattr(foreign, 'value')
        assert support.events == [('factory',)]
""",
    )


_METADATA_REPLACEMENT_API = r"""
import ctypes
import gc
import pytest
from soac import _soac_ext

def api(name, result, *arguments):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = arguments
    function.restype = result
    return function

obj = ctypes.py_object
get_metadata = api("PyFunction_GetSoacMetadata", ctypes.c_void_p, obj)
set_metadata = api(
    "PyFunction_SetSoacMetadata", ctypes.c_int,
    obj, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_void_p,
)
get_owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
get_seal = api("PyFunction_GetSoacStrictId", ctypes.c_uint64, obj)
source_id = api("PyCode_GetSoacStrictSourceId", ctypes.c_uint64, obj)
legacy_id = api("PyFunction_GetSoacFunctionId", ctypes.c_uint64, obj)
get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
vectorcall = api(
    "PyObject_Vectorcall", obj, obj, ctypes.POINTER(obj),
    ctypes.c_size_t, ctypes.c_void_p,
)
entry_signature = ctypes.PYFUNCTYPE(
    obj, obj, ctypes.POINTER(obj), ctypes.c_size_t, ctypes.c_void_p,
)
destructor_signature = ctypes.CFUNCTYPE(None, ctypes.c_void_p)

def c_call(function, *values):
    arguments = (obj * len(values))(*values) if values else None
    return vectorcall(function, arguments, len(values), None)

def saved_entry(function):
    # Keep the actual public ABI before replacement. PYFUNCTYPE checks the
    # pending exception, so NULL-without-error is not a passing refusal.
    pointer = get_vectorcall(function)
    assert pointer
    entry = entry_signature(pointer)
    def invoke(*values):
        arguments = (obj * len(values))(*values) if values else None
        return entry(function, arguments, len(values), None)
    return invoke

class MetadataSlot:
    def __init__(self, kind):
        self.kind = kind
        # This is a real, deliberately small foreign allocation with its own
        # destructor, never a forged SOAC payload/destructor pairing.
        self.storage = ctypes.c_ubyte(123)
        self.pointer = ctypes.addressof(self.storage)
        self.releases = []
        self.destructor = destructor_signature(self.releases.append)

    def replace(self, function):
        if self.kind == "foreign":
            pointer = self.pointer
            destructor = ctypes.cast(self.destructor, ctypes.c_void_p)
        else:
            pointer = destructor = None
        assert set_metadata(function, 0, pointer, destructor) == 0
        assert get_metadata(function) == pointer
        assert legacy_id(function) == 0

    def clear(self, function):
        # Keep the callback and its allocation alive until native ownership
        # ends. A Python function reference does not own an opaque payload.
        assert set_metadata(function, 0, None, None) == 0
        gc.collect()
        assert self.releases == ([self.pointer] if self.kind == "foreign" else [])

def assert_metadata_unavailable(function, saved, *values):
    for invoke in (lambda: function(*values), lambda: c_call(function, *values),
                   lambda: saved(*values)):
        with pytest.raises(RuntimeError, match="metadata"):
            invoke()
    with pytest.raises(RuntimeError, match="metadata"):
        _soac_ext.strict_function_entry_kind(function)
"""


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("replacement", ["cleared", "foreign"])
def test_saved_checked_entries_reject_unowned_metadata(
    strict_functions, entry_interpreter, replacement
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_functions.run(
        f"expected_entry = {expected_entry!r}\nreplacement = {replacement!r}\n"
        + _METADATA_REPLACEMENT_API
        + r"""
import checked

function = checked.identity
assert _soac_ext.strict_module_diagnostics(checked)["sealed"]
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
source_owner, seal, code_source = get_owner(function), get_seal(function), source_id(function.__code__)
assert source_owner and seal and code_source and get_metadata(function)
assert legacy_id(function) == 0
saved = saved_entry(function)
assert saved(7) == 7
assert saved("wrong") == "wrong"
original_entry = get_vectorcall(function)
slot = MetadataSlot(replacement)
try:
    slot.replace(function)
    assert get_vectorcall(function) == original_entry
    assert_metadata_unavailable(function, saved, 7)
    # Replacing an optional implementation neither revokes the native owner
    # nor grants permission to execute without an authenticated implementation.
    assert get_owner(function) == source_owner and get_seal(function) == seal
    assert source_id(function.__code__) == code_source
finally:
    slot.clear(function)

def ordinary(value):
    return value

ordinary_saved = saved_entry(ordinary)
slot = MetadataSlot(replacement)
try:
    slot.replace(ordinary)
    ordinary.__code__ = ordinary.__code__
    ordinary.__defaults__ = ("default",)
    ordinary.__kwdefaults__ = {}
    marker = object()
    assert ordinary(marker) is marker
    assert c_call(ordinary, marker) is marker
    assert ordinary_saved(marker) is marker
    assert ordinary() == "default"
    assert get_owner(ordinary) is None
    assert _soac_ext.strict_function_entry_kind(ordinary) is None
    assert get_metadata(ordinary) == (slot.pointer if replacement == "foreign" else None)
finally:
    slot.clear(ordinary)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("replacement", ["cleared", "foreign"])
def test_keyword_default_metadata_replacement_keeps_captured_invocation(
    binding_identity_project, entry_interpreter, replacement
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    binding_identity_project.run(
        f"expected_entry = {expected_entry!r}\nreplacement = {replacement!r}\n"
        + _METADATA_REPLACEMENT_API
        + r"""
import binding_identity as actual
import binding_identity_control as ordinary

assert _soac_ext.strict_module_diagnostics(actual)["sealed"]
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert _soac_ext.strict_function_entry_kind(actual.plain) == expected_entry

def exercise(function, selected):
    # Unknown decorators classify this existing source function as dynamic;
    # it owns an authenticated SOAC entry but has no sealed metadata/defaults.
    assert get_seal(function) == 0
    source_owner = get_owner(function)
    assert bool(source_owner) is selected
    assert bool(get_metadata(function)) is selected
    saved = saved_entry(function)
    assert saved() == 1
    original_entry, defaults = get_vectorcall(function), function.__kwdefaults__
    parameter_name = function.__code__.co_varnames[0]
    slot = MetadataSlot(replacement)
    marker = object()
    events = []
    class ReplacingKey:
        def __hash__(self):
            return hash(parameter_name)
        def __eq__(self, other):
            events.append(("default", other is parameter_name))
            slot.replace(function)
            if selected:
                with pytest.raises(RuntimeError, match="metadata"):
                    function(value=marker)
            else:
                assert function(value=marker) is marker
            events.append(("reentered", True))
            return other == parameter_name
    try:
        function.__kwdefaults__ = {ReplacingKey(): marker}
        assert function() is marker
        assert events == [("default", True), ("reentered", True)], events
        assert get_owner(function) == source_owner
        assert get_vectorcall(function) == original_entry
        assert get_seal(function) == 0
        function.__kwdefaults__ = defaults
        if selected:
            assert_metadata_unavailable(function, saved)
        else:
            assert function() == c_call(function) == saved() == 1
        assert events == [("default", True), ("reentered", True)], events
    finally:
        function.__kwdefaults__ = defaults
        slot.clear(function)

exercise(ordinary.plain, False)
exercise(actual.plain, True)
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("replacement", ["cleared", "foreign"])
@pytest.mark.parametrize("outcome", ["value", "annotated-value", "callback-error"])
def test_body_metadata_replacement_preserves_results_exceptions_and_cleanup(
    strict_functions, entry_interpreter, replacement, outcome
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    strict_functions.run(
        f"expected_entry = {expected_entry!r}\nreplacement = {replacement!r}\noutcome = {outcome!r}\n"
        + _METADATA_REPLACEMENT_API
        + r"""
import sys
import weakref
import checked

function = checked.finish_with_result
assert _soac_ext.strict_function_entry_kind(function) == expected_entry
source_owner, seal = get_owner(function), get_seal(function)
assert source_owner and seal
assert get_metadata(function) and legacy_id(function) == 0
saved = saved_entry(function)
slot = MetadataSlot(replacement)
events, references, callbacks = [], [], []
class Payload:
    def __del__(self):
        events.append("drop")
def create():
    events.append("create")
    payload = Payload()
    references.append(weakref.ref(payload, lambda _: callbacks.append("weakref")))
    return payload

callback_error = RuntimeError("callback wins")
def observe(stage):
    events.append(stage)
    assert isinstance(sys.exception(), LookupError)
    assert str(sys.exception()) == "source handler"
    slot.replace(function)
    if outcome == "callback-error":
        raise callback_error

value = int("12345678901234567890") if outcome == "value" else "wrong"
outer = ValueError("caller handler")
try:
    try:
        raise outer
    except ValueError:
        try:
            result = function(create, observe, value)
        except Exception as error:
            assert outcome == "callback-error" and error is callback_error
            assert isinstance(error.__context__, LookupError)
            assert str(error.__context__) == "source handler"
            error.__context__.__traceback__ = None
            error.__context__ = None
            error.__traceback__ = None
        else:
            assert outcome in ("value", "annotated-value") and result is value
        assert sys.exception() is outer
    assert sys.exception() is None
    assert [event for event in events if event != "drop"] == ["create", "body"]
    assert get_owner(function) == source_owner and get_seal(function) == seal
    assert_metadata_unavailable(function, saved, create, observe, 7)
finally:
    slot.clear(function)
gc.collect()
assert len(references) == 1 and references[0]() is None
assert events.count("drop") == 1 and callbacks == ["weakref"], (events, callbacks)
assert [event for event in events if event != "drop"] == ["create", "body"]
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def cpython_strict_functions(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("cpython-strict-functions"),
        {"checked.py": _SOURCE, "support.py": _SUPPORT},
        modules={"checked": "checked.py"}, backend="cpython",
    )


def test_cpython_backend_public_binders_returns_and_c_callers(cpython_strict_functions):
    from pathlib import Path

    cpython_strict_functions.run_case(
        "checked",
        """
import ctypes
import types
import checked
from support import events
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError
from tests._strict_integration import _assert_cpython_function_witness

def rejected(call, contains=None):
    try:
        call()
    except TypeError as error:
        if contains is not None:
            assert contains in str(error), str(error)
    else:
        raise AssertionError('ordinary argument binding error was skipped')

def exercise():
    assert checked.identity(True) is True
    value = int('12345678901234567890')
    assert checked.identity(value) is value
    assert checked.widened(value) is value
    assert checked.optional(None) is None
    assert checked.optional('ok') == 'ok'
    assert checked.shape(3, 4, 5, 6, named=None, extra=7) == 7
    assert checked.shape(3) == 5
    assert checked.identity('bad') == 'bad'
    marker = []
    assert checked.optional(marker) is marker
    assert checked.shape(1, 2, 'bad') == 3
    assert checked.shape(1, extra='bad') == 3
    rejected(lambda: checked.shape('bad', 2, second=3), 'multiple values')
    rejected(lambda: checked.identity('bad', unexpected=1), 'unexpected keyword')
    rejected(lambda: checked.identity(), 'missing')
    assert checked.bad_return('bad') == 'bad'
    assert checked.caller('bad') == 'bad'
    try:
        checked.raises(1)
    except LookupError as error:
        assert str(error) == 'body wins'
    else:
        raise AssertionError('body exception was replaced or lost')

# Cold and then repeatedly exercised ordinary CPython caller bytecode. Safe
# generic fallback is allowed; no particular specialization opcode is required.
exercise()
for number in range(128):
    assert checked.caller(number) == number
    assert checked.shape(number, named=None) == number + 2
exercise()

call = ctypes.pythonapi.PyObject_Call
call.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
one = ctypes.pythonapi.PyObject_CallOneArg
one.argtypes = [ctypes.py_object, ctypes.py_object]
one.restype = ctypes.py_object
assert one(checked.identity, 7) == 7
assert call(checked.shape, (3, 4, 5), {'named': None, 'extra': 6}) == 7
assert one(checked.identity, 'bad') == 'bad'
assert call(checked.shape, (1,), {'extra': 'bad'}) == 3
rejected(lambda: call(checked.identity, ('bad',), {'unexpected': 1}), 'unexpected keyword')
assert one(checked.bad_return, 'bad') == 'bad'
assert 'annotation evaluated' not in events

diagnostic = _soac_ext.strict_module_diagnostics(checked)
for function in (checked.identity, checked.shape, checked.bad_return, checked.raises):
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed['original_code_entered'] is True
ordinary = lambda value: value
assert _soac_ext.strict_function_diagnostics(ordinary) is None
copy = types.FunctionType(checked.identity.__code__, checked.__dict__)
copy.__dict__.update(checked.identity.__dict__)
assert _soac_ext.strict_function_diagnostics(copy) is None
try:
    copy(7)
except StrictRuntimeUnavailableError:
    pass
else:
    raise AssertionError('copied source code acquired original-body ownership')
assert checked.identity(8) == 8
""",
        Path(__file__),
        required_functions=("identity", "shape", "bad_return", "raises", "idle_default"),
        
        backend="cpython",
    )


def test_cpython_backend_returns_and_callback_errors_preserve_frame_cleanup(cpython_strict_functions):
    from pathlib import Path

    cpython_strict_functions.run_case(
        "checked",
        """
import gc
import sys
import weakref
import checked
from soac import _soac_ext

events = []
references = []
class Payload:
    def __del__(self):
        events.append(('drop', sys.exception()))
def create():
    payload = Payload()
    references.append(weakref.ref(payload))
    return payload

outer = ValueError('caller handler')
try:
    raise outer
except ValueError:
    assert checked.finish_with_result(create, events.append, 7) == 7
    assert events == ['body', ('drop', outer)], events
    assert references[-1]() is None
    events.clear()
    assert checked.finish_with_result(create, events.append, 'wrong') == 'wrong'
    assert events == ['body', ('drop', outer)], events
    assert references[-1]() is None
    events.clear()
    failure = RuntimeError('explicit callback failure')
    def fail(stage):
        events.append(stage)
        raise failure
    try:
        checked.finish_with_result(create, fail, 'wrong')
    except RuntimeError as error:
        assert error is failure
        assert isinstance(error.__context__, LookupError)
        assert str(error.__context__) == 'source handler'
        assert error.__context__.__context__ is outer
        assert [event for event in events if event == 'body'] == ['body']
        error.__context__.__traceback__ = None
        error.__traceback__ = None
    else:
        raise AssertionError('the explicit callback error was lost')
    gc.collect()
    assert references[-1]() is None
    assert len([event for event in events if isinstance(event, tuple) and event[0] == 'drop']) == 1
    assert sys.exception() is outer
assert sys.exception() is None
assert _soac_ext.strict_function_diagnostics(
    checked.finish_with_result
)['original_code_entered'] is True
""",
        Path(__file__), required_functions=("finish_with_result",),
         backend="cpython",
    )


_NATIVE_COMMON_OWNER_CYCLE_SOURCE = """
def make_cycle():
    saved = []
    def checked(value: int = 7) -> int:
        if saved:
            return value
        return 0
    saved.append(checked)
    return checked
"""


def test_native_common_owner_metadata_does_not_pin_function_code_defaults_or_cells(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "native_owner.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _NATIVE_COMMON_OWNER_CYCLE_SOURCE,
            "ordinary_owner.py": _NATIVE_COMMON_OWNER_CYCLE_SOURCE,
        },
        modules={"native_owner": "native_owner.py"},
        backend="cpython",
    )
    project.run_case(
        "native_owner",
        textwrap.dedent("""
        def validate(module):
            import ctypes, gc, sys, weakref
            import ordinary_owner
            from soac import _soac_ext

            def counts(function):
                return tuple(sys.getrefcount(value) for value in (
                    function, function.__code__, function.__defaults__, function.__closure__,
                ))

            ordinary = ordinary_owner.make_cycle()
            function = module.make_cycle()
            before = _soac_ext.strict_function_diagnostics(function)
            assert before["backend"] == "cpython"
            assert before["entry_kind"] == "original_code"
            assert before["finalized"] is True
            assert before["original_code_entered"] is False
            assert function("wrong") == "wrong"
            assert _soac_ext.strict_function_diagnostics(function)["original_code_entered"] is True
            assert function() == ordinary() == 7
            # The later referent genexpr makes only function a cell variable.
            # Measure both through the same argument-loading site, so a
            # LOAD_DEREF temporary is not mistaken for an owner-metadata edge.
            measured = [counts(value) for value in (function, ordinary)]
            assert measured[0] == measured[1], measured

            get_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            get_owner.argtypes = [ctypes.py_object]
            get_owner.restype = ctypes.c_void_p
            owner = ctypes.cast(get_owner(function), ctypes.py_object).value
            references = gc.get_referents(owner)
            assert not any(reference is value for reference in references for value in (
                function, function.__code__, function.__globals__,
                function.__defaults__, function.__kwdefaults__, function.__closure__,
            ) if value is not None)
            witness = weakref.ref(function)
            ordinary_witness = weakref.ref(ordinary)
            del references, function, ordinary
            gc.collect()
            assert ordinary_witness() is None
            assert witness() is None, "metadata retained a source function cycle"
            # Keeping the metadata shell itself alive must not keep the function.
            assert owner is not None
        """),
        tmp_path / "native_owner_validation.py",
        required_functions=("make_cycle",),
        
    )


def test_cpython_function_birth_captures_live_builtins_not_parent_capture(tmp_path):
    source = """
import builtins

INITIAL_BUILTINS = builtins.__dict__

def make():
    def read():
        return len((1, 2, 3))
    return read

def replacement_len(values):
    return 37

CAPTURED_BUILTINS = dict(INITIAL_BUILTINS)
CAPTURED_BUILTINS['len'] = replacement_len
__builtins__ = CAPTURED_BUILTINS
created = make()
__builtins__ = INITIAL_BUILTINS
"""
    project = create_strict_project(
        tmp_path,
        {
            "builtin_birth.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + source,
            "ordinary_builtin_birth.py": source,
        },
        modules={"builtin_birth": "builtin_birth.py"},
        backend="cpython",
    )
    project.run_case(
        "builtin_birth",
        """
def validate_module(module):
    import ctypes
    import types
    import pytest
    import ordinary_builtin_birth as ordinary
    from soac import _soac_ext, StrictRuntimeUnavailableError
    from tests._strict_integration import _assert_cpython_function_witness

    get_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    get_owner.argtypes = [ctypes.py_object]
    get_owner.restype = ctypes.c_void_p
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    assert diagnostic['sealed'] and diagnostic['backend'] == 'cpython'
    assert _soac_ext.strict_module_diagnostics(ordinary) is None

    for target in (ordinary, module):
        assert target.make.__builtins__ is target.INITIAL_BUILTINS
        assert vars(target)['__builtins__'] is target.INITIAL_BUILTINS
        assert target.created.__globals__ is vars(target)
        assert target.created.__builtins__ is target.CAPTURED_BUILTINS
        assert target.created.__builtins__ is not target.make.__builtins__
        assert target.created() == 37
        later = target.make()
        assert later.__code__ is target.created.__code__
        assert later.__globals__ is vars(target)
        assert later.__builtins__ is target.INITIAL_BUILTINS
        assert later() == 3
        assert target.created() == 37
        for function in (target.make, target.replacement_len, target.created, later):
            if target is ordinary:
                assert get_owner(function) is None
                assert _soac_ext.strict_function_diagnostics(function) is None
            else:
                assert get_owner(function)
                observed = _assert_cpython_function_witness(function, diagnostic)
                assert observed['original_code_entered'] is True

    # Captured builtins do not grant a copied strict code object an owner.
    ordinary_copy = types.FunctionType(ordinary.created.__code__, vars(ordinary))
    assert ordinary_copy.__builtins__ is ordinary.INITIAL_BUILTINS
    assert ordinary_copy() == 3
    unowned = types.FunctionType(module.created.__code__, vars(module))
    assert get_owner(unowned) is None
    assert _soac_ext.strict_function_diagnostics(unowned) is None
    with pytest.raises(StrictRuntimeUnavailableError):
        unowned()
    assert module.created() == 37
""",
        Path(__file__),
        required_functions=("make", "replacement_len", "created"),
    )


def test_native_common_owner_provider_replacement_keeps_ordinary_calls_and_actual_adoption(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "native_provider_pair.py": """
# soac: module(strict_assign=true, checked_attr=true)
from provider_pair_probe import exercise

def build():
    pairs = []
    for ordinal in (0, 1):
        class Local:
            pass
        def checked(value: Local) -> Local:
            return value
        pairs.append((checked, Local))
    exercise(pairs)
    return pairs

pairs = build()
""",
            "ordinary_provider_pair.py": """
def build():
    pairs = []
    for ordinal in (0, 1):
        class Local:
            pass
        def checked(value: Local) -> Local:
            return value
        pairs.append((checked, Local))
    return pairs
pairs = build()
""",
            "provider_pair_probe.py": """
events = []

def exercise(pairs):
    from soac import _soac_ext
    (first, First), (second, Second) = pairs
    assert first.__code__ is second.__code__
    assert first.__annotate__.__code__ is second.__annotate__.__code__
    assert first.__annotate__ is not second.__annotate__
    assert not _soac_ext.strict_function_diagnostics(first)["finalized"]
    assert not _soac_ext.strict_function_diagnostics(second)["finalized"]
    saved = second.__annotate__
    second.__annotate__ = first.__annotate__
    try:
        try:
            second()
        except TypeError as error:
            assert "required" in str(error) and "value" in str(error)
            events.append("ordinary missing argument")
        else:
            raise AssertionError("the original missing-argument error was lost")
        value = Second()
        assert second(value) is value
        assert _soac_ext.strict_function_diagnostics(second)["original_code_entered"]
        events.append("foreign provider leaves calls ordinary")
    finally:
        second.__annotate__ = saved
    # The loop rebinds one real Local cell. Both original providers read
    # Second, just as the ordinary CPython control does.
    second_value = Second()
    assert first(second_value) is second_value
    assert second(second_value) is second_value
    first_value = First()
    assert second(first_value) is first_value
    events.append("restored provider leaves calls ordinary")
""",
        },
        modules={"native_provider_pair": "native_provider_pair.py"},
        backend="cpython",
    )
    project.run_case(
        "native_provider_pair",
        textwrap.dedent("""
        def validate(module):
            import annotationlib
            import ordinary_provider_pair as ordinary
            from provider_pair_probe import events
            from soac import _soac_ext
            for control, _ in ordinary.pairs:
                annotations = annotationlib.get_annotations(control)
                assert annotations == {"value": ordinary.pairs[-1][1], "return": ordinary.pairs[-1][1]}
            assert events == [
                "ordinary missing argument", "foreign provider leaves calls ordinary",
                "restored provider leaves calls ordinary",
            ]
            LastLocal = module.pairs[-1][1]
            for function, _ in module.pairs:
                diagnostic = _soac_ext.strict_function_diagnostics(function)
                assert diagnostic["backend"] == "cpython"
                assert diagnostic["finalized"] is True
                assert diagnostic["original_code_entered"] is True
                assert function(LastLocal()).__class__ is LastLocal
                provider = function.__annotate__
                assert annotationlib.get_annotations(function) == {
                    "value": LastLocal, "return": LastLocal,
                }
                assert _soac_ext.strict_function_diagnostics(provider)["finalized"] is True
                try:
                    provider.__code__ = provider.__code__
                except TypeError:
                    pass
                else:
                    raise AssertionError("original provider was not frozen with its function")
        """),
        tmp_path / "native_provider_pair_validation.py",
        required_functions=("build",),
        
    )


@pytest.mark.parametrize("unused_key", [False, True])
def test_cpython_original_annotation_provider_preserves_keyword_defaults(tmp_path, unused_key):
    body = """
from provider_defaults_probe import prepare

def build():
    class Local:
        pass
    def checked(value: Local) -> Local:
        return value
    prepare(checked)
    return checked, Local

pair = build()
"""
    project = create_strict_project(
        tmp_path,
        {
            "provider_defaults.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + body,
            "ordinary_provider_defaults.py": body,
            "provider_defaults_probe.py": f"""
import weakref
events = []
records = []

class UnusedKey:
    def __hash__(self):
        events.append('hash')
        return hash('unused')
    def __eq__(self, other):
        events.append('equality')
        raise AssertionError('provider sealing must not look up unused keys')

def prepare(function):
    provider = function.__annotate__
    mapping = {{UnusedKey(): 7}} if {unused_key!r} else {{}}
    provider.__kwdefaults__ = mapping
    records.append((mapping, weakref.ref(provider)))
    events.clear()
""",
        },
        modules={"provider_defaults": "provider_defaults.py"},
        backend="cpython",
    )
    project.run_case(
        "provider_defaults",
        textwrap.dedent("""
        def validate(module):
            import ctypes, sys
            import provider_defaults_probe as probe
            from soac import _soac_ext
            assert probe.events == [], probe.events
            function, Local = module.pair
            mapping, witness = probe.records[0]
            provider = function.__annotate__
            assert provider is witness()
            assert provider.__kwdefaults__ is mapping
            assert _soac_ext.strict_function_diagnostics(provider)['finalized'] is True
            assert _soac_ext.strict_function_diagnostics(provider)['original_code_entered'] is False
            value = Local()
            assert function(value) is value
            call = ctypes.pythonapi.PyObject_CallOneArg
            call.argtypes = [ctypes.py_object, ctypes.py_object]
            call.restype = ctypes.py_object
            assert call(function, value) is value
            for invoke in (function, lambda value: call(function, value)):
                marker = object()
                assert invoke(marker) is marker
            assert probe.events == [], probe.events
            assert provider(1) == {'value': Local, 'return': Local}
            try:
                mapping.clear()
            except TypeError:
                pass
            else:
                raise AssertionError('provider keyword defaults were not protected')
            import ordinary_provider_defaults as ordinary
            control = ordinary.pair[0].__annotate__
            assert tuple(sys.getrefcount(item) for item in (provider.__code__, provider.__closure__)) == tuple(
                sys.getrefcount(item) for item in (control.__code__, control.__closure__)
            )
        """),
        tmp_path / "provider_defaults_validation.py",
        required_functions=("build",), 
        backend="cpython",
    )



def _native_function_capi_validation(body):
    """Reuse the original signed subject and its ordinary source-only control."""
    ordinary_source = _SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1)
    return (
        "import ctypes\nimport types\nimport pytest\nimport checked\n"
        "from soac import _soac_ext\n"
        "from soac.strict import StrictMutationError\n"
        "from tests._strict_integration import _assert_cpython_function_witness\n"
        "ordinary = types.ModuleType('ordinary_function_capi_control')\n"
        f"exec(compile({ordinary_source!r}, '<ordinary-function-capi>', 'exec', "
        "dont_inherit=True), ordinary.__dict__)\n"
        + textwrap.dedent("""
        def api(name, result, *arguments):
            function = getattr(ctypes.pythonapi, name)
            function.argtypes = list(arguments)
            function.restype = result
            return function

        obj = ctypes.py_object
        get_owner = api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p, obj)
        diagnostic = _soac_ext.strict_module_diagnostics(checked)
        for name in ("identity", "shape", "bad_return", "caller"):
            function = getattr(checked, name)
            assert get_owner(function)
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["finalized"] is True
            control = getattr(ordinary, name)
            assert not get_owner(control)
            assert _soac_ext.strict_function_diagnostics(control) is None
        """)
        + textwrap.dedent(body)
        + textwrap.dedent("""
        assert _soac_ext.runtime_compilation_activity() == {
            "schema": 1, "lowering_entries": 0, "blockpy_cache_entries": 0,
            "jit_engine_entries": 0,
        }
        """)
    )


def test_cpython_function_c_metadata_setters_preserve_frozen_source_ownership(
    cpython_strict_functions,
):
    cpython_strict_functions.run_case(
        "checked",
        _native_function_capi_validation("""
        inner = checked.make_cycle()
        control_inner = ordinary.make_cycle()
        assert inner(3) == control_inner(3) == 4
        assert get_owner(inner)
        observed = _assert_cpython_function_witness(
            inner, diagnostic,
        )
        assert observed["finalized"] is True and observed["original_code_entered"] is True
        assert not get_owner(control_inner)
        assert _soac_ext.strict_function_diagnostics(control_inner) is None

        functions = (checked.shape, inner)
        originals = tuple(
            (function.__code__, function.__globals__, function.__defaults__,
             function.__kwdefaults__, function.__closure__, function.__annotate__,
             get_owner(function))
            for function in functions
        )
        keywords = checked.shape.__kwdefaults__
        assert type(keywords) is dict and keywords == {"named": None}
        keyword_items = tuple(keywords.items())
        defaults = (9,)
        replacements = {"named": "changed"}
        annotations = {"first": str}
        closure = (types.CellType(["first", "second"]),)
        assert len(closure) == len(inner.__closure__) == len(control_inner.__closure__) == 1

        for name, native, control, attribute, replacement in (
            ("PyFunction_SetDefaults", checked.shape, ordinary.shape, "__defaults__", defaults),
            ("PyFunction_SetKwDefaults", checked.shape, ordinary.shape, "__kwdefaults__", replacements),
            ("PyFunction_SetAnnotations", checked.shape, ordinary.shape, "__annotations__", annotations),
            ("PyFunction_SetClosure", inner, control_inner, "__closure__", closure),
        ):
            setter = api(name, ctypes.c_int, obj, obj)
            assert setter(control, replacement) == 0
            assert getattr(control, attribute) is replacement
            with pytest.raises(StrictMutationError):
                setter(native, replacement)
            for function, original in zip(functions, originals):
                actual = (
                    function.__code__, function.__globals__, function.__defaults__,
                    function.__kwdefaults__, function.__closure__, function.__annotate__,
                )
                assert all(value is expected for value, expected in zip(actual, original[:-1]))
                assert get_owner(function) == original[-1]
            assert checked.shape.__kwdefaults__ is keywords
            assert tuple(keywords.items()) == keyword_items

        # The actual keyword-default mapping remains protected against aliases,
        # not only against replacement through the function's setter.
        set_item = api("PyDict_SetItem", ctypes.c_int, obj, obj, obj)
        del_item = api("PyDict_DelItem", ctypes.c_int, obj, obj)
        for operation in (
            lambda mapping: set_item(mapping, "named", "other"),
            lambda mapping: del_item(mapping, "named"),
        ):
            assert operation(replacements) == 0
            with pytest.raises(StrictMutationError):
                operation(keywords)
            assert tuple(keywords.items()) == keyword_items
        assert checked.shape(1) == 3
        assert ordinary.shape(1, named=None) == 10
        assert inner(3) == 4 and control_inner(3) == 5
        with pytest.raises(TypeError):
            checked.shape("wrong")
        with pytest.raises(TypeError):
            inner("wrong")
        from support import events
        assert "annotation evaluated" not in events
        for function in functions:
            observed = _assert_cpython_function_witness(
                function, diagnostic,
            )
            assert observed["original_code_entered"] is True
        """),
        Path(__file__),
        required_functions=("shape", "make_cycle"),
        
        backend="cpython",
    )


@pytest.mark.parametrize("entry", ["restored-stock", "forwarder"])
def test_cpython_function_public_vectorcall_preserves_ordinary_calls_and_source_ownership(
    cpython_strict_functions, function_create_watch_extension, entry,
):
    validation = (
        f"entry = {entry!r}\nextension_path = {str(function_create_watch_extension)!r}\n"
        + textwrap.dedent("""
        import importlib.util
        spec = importlib.util.spec_from_file_location("_strict_function_create_watch", extension_path)
        native_probe = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(native_probe)
        with pytest.raises(TypeError):
            native_probe.install_stock_forwarder(object())

        get_vectorcall = api("PyVectorcall_Function", ctypes.c_void_p, obj)
        set_vectorcall = api("PyFunction_SetVectorcall", None, obj, ctypes.c_void_p)
        stock = ctypes.cast(ctypes.pythonapi._PyFunction_Vectorcall, ctypes.c_void_p).value
        assert stock
        targets = tuple(
            getattr(module, name)
            for module in (checked, ordinary)
            for name in ("identity", "shape", "bad_return", "caller")
        )
        original = tuple(
            (function, get_vectorcall(function), get_owner(function),
             function.__code__, function.__globals__)
            for function in targets
        )
        call = api("PyObject_Call", obj, obj, obj, obj)
        one = api("PyObject_CallOneArg", obj, obj, obj)
        vector = api(
            "PyObject_Vectorcall", obj, obj, ctypes.POINTER(obj),
            ctypes.c_size_t, ctypes.c_void_p,
        )

        def vector_one(function, value):
            arguments = (obj * 1)(value)
            return vector(function, arguments, 1, None)

        def invoke_direct(function, value):
            return function(value)

        callers = (
            invoke_direct,
            lambda function, value: call(function, (value,), {}),
            one,
            vector_one,
        )
        try:
            for function, _, _, _, _ in original:
                native_probe.install_stock_forwarder(function)
                forwarder = get_vectorcall(function)
                assert forwarder and forwarder != stock
                if entry == "restored-stock":
                    set_vectorcall(function, stock)
                    assert get_vectorcall(function) == stock

            # Legal public entry changes preserve original-code execution;
            # annotations do not add argument or result predicates.
            assert _soac_ext.strict_function_diagnostics(checked.identity)["original_code_entered"] is False
            for invoke in callers:
                assert invoke(ordinary.identity, "wrong") == "wrong"
                assert invoke(checked.identity, "wrong") == "wrong"
                assert _soac_ext.strict_function_diagnostics(checked.identity)["original_code_entered"] is True
                assert invoke(ordinary.bad_return, "wrong") == "wrong"
                assert invoke(checked.bad_return, "wrong") == "wrong"
                assert _soac_ext.strict_function_diagnostics(checked.bad_return)["original_code_entered"] is True

            # Warm ordinary caller bytecode as well as each public C entry.
            # The inner call in caller must still use identity's actual owner.
            for value in range(128):
                for invoke in callers:
                    assert invoke(checked.identity, value) == invoke(ordinary.identity, value) == value
                    assert invoke(checked.caller, value) == invoke(ordinary.caller, value) == value
                    assert invoke(checked.shape, value) == invoke(ordinary.shape, value) == value + 2
            for invoke in callers:
                assert invoke(checked.identity, "wrong") == "wrong"
                assert invoke(checked.caller, "wrong") == "wrong"
                assert invoke(checked.bad_return, "wrong") == "wrong"
            assert call(checked.shape, (3, 4, 5), {"named": None, "extra": 6}) == 7
            assert call(ordinary.shape, (3, 4, 5), {"named": None, "extra": 6}) == 7
            with pytest.raises(TypeError, match="unexpected keyword"):
                call(checked.identity, ("wrong",), {"unexpected": 1})
            assert call(checked.shape, (1,), {"extra": "wrong"}) == 3
            assert call(ordinary.shape, (1,), {"extra": "wrong"}) == 3
            for function, _, owner, code, globals_ in original:
                assert get_owner(function) == owner
                assert function.__code__ is code and function.__globals__ is globals_
                if owner:
                    observed = _assert_cpython_function_witness(
                        function, diagnostic,
                    )
                    assert observed["finalized"] is True and observed["original_code_entered"] is True
                else:
                    assert _soac_ext.strict_function_diagnostics(function) is None
            from support import events
            assert "annotation evaluated" not in events
        finally:
            for function, pointer, _, _, _ in original:
                set_vectorcall(function, pointer)
                assert get_vectorcall(function) == pointer
        assert checked.identity(11) == 11
        assert checked.identity("wrong") == "wrong"
        """)
    )
    cpython_strict_functions.run_case(
        "checked",
        _native_function_capi_validation(validation),
        Path(__file__),
        required_functions=("identity", "shape", "bad_return", "caller"),
        
        backend="cpython",
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_untraced_source_callbacks_preserve_values_and_cleanup(
    strict_functions, entry_interpreter
):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    program = (
        f"expected_entry = {expected!r}\n"
        + textwrap.dedent("""
        import gc
        import sys
        import weakref
        import checked
        from soac import _soac_ext

        assert _soac_ext.strict_function_entry_kind(checked.replace_result) == expected_entry
        calls, released, references = [], [], []

        class Iterator:
            def __init__(self):
                references.append(weakref.ref(self))
            def __next__(self):
                calls.append("next")
                return 41
            def __del__(self):
                released.append("iterator")

        def factory():
            calls.append("factory")
            return object()

        # Source-defined callbacks and cleanup do not depend on observer coverage.
        assert checked.replace_result(Iterator(), factory) == 41
        gc.collect()
        assert calls == ["factory", "next"], calls
        assert released == ["iterator"], released
        assert all(reference() is None for reference in references)
        assert _soac_ext.strict_function_entry_kind(checked.replace_result) == expected_entry
        assert checked.identity(13) == 13
        assert checked.identity("bad") == "bad"

        # Ordinary CPython functions still receive their actual trace events.
        observed = []
        def ordinary(value):
            return value
        def ordinary_trace(frame, event, arg):
            if frame.f_code is ordinary.__code__:
                observed.append(event)
            return ordinary_trace
        sys.settrace(ordinary_trace)
        try:
            value = object()
            assert ordinary(value) is value
        finally:
            sys.settrace(None)
        assert observed[0] == "call" and observed[-1] == "return", observed
        """)
    )
    strict_functions.run(program, entry_interpreter=entry_interpreter)
