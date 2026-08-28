"""Method families through actual checker, construction, and ordinary calls."""

import json
from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project
from tests.test_strict_class_runtime import _CPYTHON_CLASS_CONSTRUCTION

SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from collections.abc import Callable

EVENTS = []
LIFETIME_EVENTS = []

class Base:
    def method(self, value: int = 1) -> int:
        EVENTS.append('base')
        return value + 10

    def invoke(self, argument):
        return self.method(argument())

class Override(Base):
    def method(self, value: int = 1) -> int:
        EVENTS.append('override')
        return value + 20

class Inherited(Base):
    pass

class FieldShadow(Base):
    method: Callable[[int], int]

    def __init__(self, callback):
        self.method = callback

def make_family(offset):
    class Local:
        def method(self, value):
            return offset + value

        def invoke(self, argument):
            return self.method(argument())
    return Local

def evaluate_pair(factory, first, second):
    return factory()(first(), second())

def temporary_method(factory):
    return factory().method()

class LifetimeTarget:
    def __init__(self, label, fail=False):
        self.label = label
        self.fail = fail

    def __del__(self):
        LIFETIME_EVENTS.append(self.label)

    def make_target(self, fail):
        return LifetimeTarget('receiver', fail)

    def method(self, first, second):
        if self.fail:
            raise ValueError('method failed')
        return 7

    def invoke(self, fail, first, second):
        return self.make_target(fail).method(first(), second())

    def invoke_then(self, fail, first, second):
        result = self.make_target(fail).method(first(), second())
        LIFETIME_EVENTS.append('continued')
        return result

    def replace_result(self, fail, first, second):
        result = LifetimeTarget('previous')
        result = self.make_target(fail).method(first(), second())
        LIFETIME_EVENTS.append('continued')
        return result
"""


@pytest.fixture(scope="module")
def methods(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-method-dispatch"),
        {"methods.py": SOURCE},
        modules={"methods": "methods.py"},
        backend=getattr(request, "param", "soac"),
    )


TRAINING = """
import ctypes
import methods

is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int
for cls in (methods.Base, methods.Override, methods.Inherited, methods.FieldShadow):
    assert is_sealed(cls) == 1, cls

base, override, inherited = methods.Base(), methods.Override(), methods.Inherited()
field = methods.FieldShadow(lambda value: value + 90)
first, second = methods.make_family(30), methods.make_family(40)
assert first is not second and is_sealed(first) and is_sealed(second)
left, right = first(), second()
for unused in range(100):
    assert base.invoke(lambda: 2) == 12
    assert override.invoke(lambda: 2) == 22
    assert inherited.invoke(lambda: 2) == 12
    assert field.invoke(lambda: 2) == 92
    assert left.invoke(lambda: 2) == 32 and right.invoke(lambda: 2) == 42
"""


VALIDATION = """
methods.EVENTS.clear()
for receiver in (base, override, inherited):
    methods.EVENTS.clear()
    try:
        receiver.invoke(lambda: 'wrong')
    except TypeError:
        pass
    else:
        raise AssertionError('virtual dispatch lost the original addition error')
    expected_body = 'override' if receiver is override else 'base'
    assert methods.EVENTS == [expected_body], 'an annotation prevented body entry'

operand_events = []
class Operand:
    def __add__(self, offset):
        operand_events.append(offset)
        return ('ordinary result', offset)
for receiver, offset in ((base, 10), (override, 20), (inherited, 10)):
    assert receiver.invoke(Operand) == ('ordinary result', offset)
assert operand_events == [10, 20, 10]

events = []
class OrdinaryChild(methods.Base):
    @property
    def method(self):
        events.append('lookup')
        return lambda value: ('ordinary', value)
child = OrdinaryChild()
assert not is_sealed(OrdinaryChild)
def argument():
    events.append('argument')
    return 3
assert child.invoke(argument) == ('ordinary', 3)
assert events == ['lookup', 'argument']

original = ValueError('lookup failure precedes argument effects')
class Broken(methods.Base):
    @property
    def method(self):
        raise original
try:
    Broken().invoke(argument)
except ValueError as error:
    assert error is original
else:
    raise AssertionError('lookup exception was lost')
assert events == ['lookup', 'argument']

# Equal source identities do not put independent factory classes in one family.
assert first.invoke(right, lambda: 2) == 42
assert second.invoke(left, lambda: 2) == 32
"""


_CPYTHON_METHOD_VALIDATION = """
from tests._strict_integration import _assert_cpython_function_witness

module_witness = _soac_ext.strict_module_diagnostics(methods)
for cls in (methods.Base, methods.Override, methods.Inherited, methods.FieldShadow, first, second):
    assert_native_class(cls)
assert get_type_owner(first) != get_type_owner(second)
for cls in (first, second):
    for name in ("method", "invoke"):
        witness = _assert_cpython_function_witness(
            vars(cls)[name], module_witness,
        )
        assert witness["finalized"] is True
        assert witness["original_code_entered"] is True

# Exercise the owning-result helper directly, and its StackRef sibling through
# the public vectorcall method API. No private StackRef layout is mirrored.
get_method = ctypes.pythonapi._PyObject_GetMethod
get_method.argtypes = [
    ctypes.py_object, ctypes.py_object, ctypes.POINTER(ctypes.c_void_p),
]
get_method.restype = ctypes.c_int
decref = ctypes.pythonapi.Py_DecRef
decref.argtypes = [ctypes.c_void_p]
decref.restype = None
vectorcall_method = ctypes.pythonapi.PyObject_VectorcallMethod
vectorcall_method.argtypes = [
    ctypes.py_object, ctypes.POINTER(ctypes.py_object), ctypes.c_size_t, ctypes.c_void_p,
]
vectorcall_method.restype = ctypes.py_object

def resolve(receiver):
    result = ctypes.c_void_p()
    unbound = get_method(receiver, "method", ctypes.byref(result))
    assert unbound in (0, 1) and result.value is not None
    try:
        target = ctypes.cast(result, ctypes.py_object).value
    finally:
        # target has its own Python reference; consume the helper's owned one.
        decref(result)
    return unbound, target

def owning_method_call(receiver, *arguments):
    unbound, target = resolve(receiver)
    return target(receiver, *arguments) if unbound else target(*arguments)

def stackref_method_call(receiver, *arguments):
    values = (ctypes.py_object * (1 + len(arguments)))(receiver, *arguments)
    return vectorcall_method("method", values, len(values), None)

ordinary_namespace = {"__name__": "ordinary_method_control"}
exec(compile(ordinary_source, "<ordinary-method-control>", "exec"), ordinary_namespace)
ordinary_receivers = (
    ordinary_namespace["Base"](), ordinary_namespace["Override"](),
    ordinary_namespace["Inherited"](),
    ordinary_namespace["FieldShadow"](lambda value: value + 90),
)
for receiver, control, declaring, expected in (
    (base, ordinary_receivers[0], methods.Base, 12),
    (override, ordinary_receivers[1], methods.Override, 22),
    (inherited, ordinary_receivers[2], methods.Base, 12),
    (field, ordinary_receivers[3], None, 92),
):
    actual_kind, target = resolve(receiver)
    control_kind, control_target = resolve(control)
    assert actual_kind == control_kind == (declaring is not None)
    if declaring is not None:
        assert target is vars(declaring)["method"]
    else:
        assert target is vars(receiver)["method"]
        assert control_target is vars(control)["method"]
    for call in (owning_method_call, stackref_method_call):
        assert call(receiver, 2) == call(control, 2) == expected

# Both native lookup APIs must ignore hidden dictionary values only for actual
# protected receivers. The same original source without opt-in stays ordinary.
for receiver, control, expected in zip(
    (base, override, inherited), ordinary_receivers[:3], (11, 21, 11),
):
    hidden = lambda *arguments: "dictionary shadow"
    vars(receiver)["method"] = hidden
    vars(control)["method"] = hidden
    assert resolve(receiver)[0] == 1
    assert resolve(control) == (0, hidden)
    for call in (owning_method_call, stackref_method_call):
        assert call(receiver) == expected
        assert call(control) == "dictionary shadow"
        methods.EVENTS.clear()
        try:
            call(receiver, "wrong")
        except TypeError as error:
            assert type(error) is TypeError
        else:
            raise AssertionError("C method lookup lost the original addition error")
        expected_body = "override" if receiver is override else "base"
        assert methods.EVENTS == [expected_body], "an annotation prevented C-call body entry"
    assert vars(receiver)["method"] is hidden

# The existing ordinary property override retains descriptor binding and lookup
# errors through both helpers, not a source-catalogued base-method shortcut.
events.clear()
kind, target = resolve(child)
assert kind == 0 and events == ["lookup"]
assert target(argument()) == ("ordinary", 3)
assert events == ["lookup", "argument"]
events.clear()
assert stackref_method_call(child, 3) == ("ordinary", 3)
assert events == ["lookup"]
for call in (owning_method_call, stackref_method_call):
    try:
        call(Broken(), 3)
    except ValueError as error:
        assert error is original
    else:
        raise AssertionError("C method lookup discarded the actual descriptor error")
assert events == ["lookup"]
"""


@pytest.mark.parametrize(
    ("methods", "entry_interpreter"),
    [("soac", False), ("soac", True), ("cpython", False)],
    indirect=["methods"],
    ids=["False", "True", "cpython"],
)
def test_virtual_calls_preserve_actual_binding_and_body_effects(
    methods, entry_interpreter
):
    if methods.backend == "cpython":
        ordinary_source = SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1)
        methods.run_case(
            "methods",
            _CPYTHON_CLASS_CONSTRUCTION + TRAINING + VALIDATION
            + f"\nordinary_source = {ordinary_source!r}\n"
            + _CPYTHON_METHOD_VALIDATION,
            Path(__file__),
            required_functions=(
                "Base.method", "Base.invoke", "Override.method",
                "FieldShadow.__init__", "make_family",
            ),
            
        )
        return
    methods.run(
        TRAINING + VALIDATION + _entry_witness(entry_interpreter),
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("methods", "entry_interpreter"),
    [("soac", False), ("soac", True), ("cpython", False)],
    indirect=["methods"],
    ids=["False", "True", "cpython"],
)
def test_temporary_receiver_cleanup_stays_inside_resolved_method_region(
    methods, entry_interpreter
):
    program = """
import methods

events = []
class Descriptor:
    def __get__(self, instance, owner):
        events.append('lookup')
        def invoke():
            events.append('body')
            return 42
        return invoke

class Temporary:
    method = Descriptor()
    def __del__(self):
        events.append('receiver released')

def ordinary(factory):
    return factory().method()

assert ordinary(Temporary) == 42
expected = list(events)
assert expected == ['lookup', 'receiver released', 'body']
events.clear()
assert methods.temporary_method(Temporary) == 42
assert events == expected

original = ValueError('lookup failed')
class BrokenDescriptor:
    def __get__(self, instance, owner):
        events.append('lookup failed')
        raise original
class Broken:
    method = BrokenDescriptor()
    def __del__(self):
        events.append('broken released')

def failed_lookup(callback):
    events.clear()
    try:
        callback(Broken)
    except ValueError as error:
        assert error is original
    else:
        raise AssertionError('lookup error was discarded')
    before_traceback_release = list(events)
    original.__traceback__ = None
    return before_traceback_release, list(events)

expected_failure = failed_lookup(ordinary)
assert expected_failure == (['lookup failed'], ['lookup failed', 'broken released'])
assert failed_lookup(methods.temporary_method) == expected_failure
"""
    if methods.backend == "cpython":
        methods.run_case(
            "methods", program + """
from soac import _soac_ext
witness = _soac_ext.strict_function_diagnostics(methods.temporary_method)
assert witness["finalized"] is True and witness["original_code_entered"] is True
""",
            Path(__file__),
            required_functions=("temporary_method",),
            
        )
        return
    profile = methods.run(program, opt_mode="profile")
    work = Path(profile.args[-1]).parent / "soac-work"
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    methods.run(
        program
        + f"assert _soac_ext.strict_function_entry_kind(methods.temporary_method) == {expected!r}\n",
        entry_interpreter=entry_interpreter,
        opt_mode="apply",
        extra_env={"SOAC_WORK_DIR": str(work)},
    )


def _entry_witness(entry_interpreter):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    return (
        "\nfor function in (methods.Base.invoke, methods.Base.method, "
        "methods.Override.method, methods.evaluate_pair, "
        "methods.LifetimeTarget.invoke, methods.LifetimeTarget.invoke_then, "
        "methods.LifetimeTarget.replace_result):\n"
        f"    assert _soac_ext.strict_function_entry_kind(function) == {expected!r}\n"
    )


def test_sealed_virtual_calls_select_source_plans_and_exercise_native_entries(
    methods, tmp_path
):
    work = tmp_path / "method-profile"
    methods.run(TRAINING, opt_mode="profile", extra_env={"SOAC_WORK_DIR": str(work)})
    events_path = tmp_path / "method-apply.jsonl"
    methods.run(
        TRAINING + VALIDATION,
        opt_mode="apply",
        extra_env={
            "SOAC_WORK_DIR": str(work),
            "SOAC_LOG": f"soac_jit_codegen=info;json={events_path}",
        },
    )
    events = [json.loads(line) for line in events_path.read_text().splitlines()]
    fields = [event.get("fields", event) for event in events]
    emitted = {
        event["function_qualname"]: event
        for event in fields
        if event.get("event") == "soac.strict_method_codegen"
    }
    for name in ("Base.invoke", "make_family.<locals>.Local.invoke"):
        assert emitted[name]["sealed_method_site_count"] == 1
        assert emitted[name]["machine_code_size_bytes"] > 0

    methods.run(
        TRAINING + VALIDATION,
        opt_mode="verify",
        extra_env={"SOAC_WORK_DIR": str(work)},
    )
    from soac import _soac_ext

    counters = json.loads(_soac_ext.inspect_counter_dump_json(str(work / "verify.bin")))
    paths = {
        name
        for record in counters["records"]
        if record["module_name"] == "methods"
        for row in record["rows"]
        if row["function_qualname"] == "Base.invoke"
        and row["kind"] == "method_dispatch"
        for name, count in row["branches"].items()
        if count
    }
    assert {"family_hit", "family_fallback", "checked_entry_hit"} <= paths


VECTORCALL_MUTATION_TRAINING = """
import ctypes
import methods

is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int
for cls in (methods.Base, methods.Override, methods.Inherited):
    assert is_sealed(cls) == 1
    receiver = cls()
    for unused in range(40):
        assert receiver.invoke(lambda: 1000) in (1010, 1020)
"""


VECTORCALL_MUTATION_VALIDATION = r"""
import gc
import sys
import types
import weakref

get_vectorcall = ctypes.pythonapi.PyVectorcall_Function
get_vectorcall.argtypes = [ctypes.py_object]
get_vectorcall.restype = ctypes.c_void_p
set_vectorcall = ctypes.pythonapi.PyFunction_SetVectorcall
set_vectorcall.argtypes = [ctypes.py_object, ctypes.c_void_p]
set_vectorcall.restype = None
incref = ctypes.pythonapi.Py_IncRef
incref.argtypes = [ctypes.py_object]
incref.restype = None
signature = ctypes.PYFUNCTYPE(
    ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
    ctypes.c_size_t, ctypes.c_void_p,
)
argument_count_mask = (1 << (8 * ctypes.sizeof(ctypes.c_size_t) - 1)) - 1

def exercise(module, class_name, observe_lookup):
    events = []
    calls = []
    errors = []
    cls = getattr(module, class_name)
    function = cls.method
    original_pointer = get_vectorcall(function)
    assert original_pointer
    original = signature(original_pointer)
    safe_failure_result = object()

    @signature
    def forward(actual, arguments, nargsf, kwnames):
        # Do not return py_object from a ctypes callback: this ABI transfers a
        # new reference. Forward the original owned raw pointer unchanged.
        # A diagnostic failure is recorded outside the callback rather than
        # letting ctypes swallow an exception and return an undefined pointer.
        try:
            events.append('wrapper')
            calls.append((actual, arguments[0], nargsf & argument_count_mask))
            result = original(actual, arguments, nargsf, kwnames)
            if result:
                return result
            errors.append('original vectorcall returned NULL without an exception')
        except BaseException as error:
            errors.append((type(error).__name__, str(error)))
        incref(safe_failure_result)
        return id(safe_failure_result)

    wrapper_pointer = ctypes.cast(forward, ctypes.c_void_p).value
    if observe_lookup:
        # This ordinary subclass takes the family-miss path in the strict
        # caller. Its descriptor makes lookup order and replay observable.
        class Observed(cls):
            @property
            def method(self):
                events.append('lookup')
                return types.MethodType(function, self)
        assert is_sealed(Observed) == 0
        receiver_class = Observed
    else:
        receiver_class = cls

    receiver = receiver_class()
    module.EVENTS.clear()
    expected_lookup = ['lookup'] if observe_lookup else []
    before_references = sys.getrefcount(function)

    def argument():
        events.append('argument')
        set_vectorcall(function, wrapper_pointer)
        if module is methods:
            assert _soac_ext.strict_function_entry_kind(function) == 'public_override'
        return 1000

    try:
        result = receiver.invoke(argument)
    finally:
        # The callback and original implementation both remain pinned through
        # restoration, including when the tested runtime reports an error.
        set_vectorcall(function, original_pointer)
    assert not errors, errors
    assert result == (1020 if class_name == 'Override' else 1010)
    assert calls == [(id(function), id(receiver), 2)], calls
    assert events == expected_lookup + ['argument', 'wrapper'], events
    assert module.EVENTS == ['override' if class_name == 'Override' else 'base']
    assert sys.getrefcount(function) == before_references
    assert get_vectorcall(function) == original_pointer

    # An argument exception must neither call the captured target nor release
    # its receiver during argument evaluation. Clear the real traceback before
    # comparing cleanup, since stock tracebacks retain frame locals.
    events.clear()
    calls.clear()
    module.EVENTS.clear()
    holder = [receiver_class()]
    receiver_ref = weakref.ref(holder[0], lambda unused: events.append('released'))
    original_error = ValueError('argument failed before invocation')

    def failing_argument():
        events.append('argument')
        holder.clear()
        assert receiver_ref() is not None, 'receiver released during argument evaluation'
        set_vectorcall(function, wrapper_pointer)
        raise original_error

    try:
        try:
            holder[0].invoke(failing_argument)
        except ValueError as error:
            assert error is original_error
        else:
            raise AssertionError('argument error was swallowed')
    finally:
        set_vectorcall(function, original_pointer)
        original_error.__traceback__ = None
    gc.collect()
    assert not errors and calls == []
    assert module.EVENTS == []
    assert receiver_ref() is None
    assert events == expected_lookup + ['argument', 'released'], events
    assert sys.getrefcount(function) == before_references
    return events

assert is_sealed(ordinary.Base) == 0
assert _soac_ext.strict_function_entry_kind(ordinary.Base.invoke) is None
for class_name in ('Base', 'Override', 'Inherited'):
    for observe_lookup in (False, True):
        stock_events = exercise(ordinary, class_name, observe_lookup)
        strict_events = exercise(methods, class_name, observe_lookup)
        assert strict_events == stock_events
"""


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_public_vectorcall_change_during_arguments_uses_the_captured_method(
    methods, tmp_path, entry_interpreter
):
    ordinary_source = SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1)
    control = (
        "import types\n"
        "ordinary = types.ModuleType('ordinary_method_control')\n"
        f"exec({ordinary_source!r}, vars(ordinary))\n"
    )
    program = (
        VECTORCALL_MUTATION_TRAINING
        + _entry_witness(entry_interpreter)
        + control
        + VECTORCALL_MUTATION_VALIDATION
        + _entry_witness(entry_interpreter)
    )
    work = tmp_path / "vectorcall-profile"
    for mode in ("profile", "apply"):
        methods.run(
            program,
            entry_interpreter=entry_interpreter,
            opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(work)},
        )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_argument_error_releases_captured_callable_and_earlier_values(
    methods, tmp_path, entry_interpreter
):
    program = r"""
import gc
import methods
import weakref

events = []
references = []
class Payload:
    def method(self, first, second):
        raise AssertionError('callee ran after failed argument')

def observe(value, label):
    references.append(weakref.ref(value, lambda unused: events.append(label)))
    return value

def factory():
    events.append('lookup')
    receiver = observe(Payload(), 'receiver released')
    return observe(receiver.method, 'callable released')

def first():
    events.append('first')
    return observe(Payload(), 'first released')

original = ValueError('second argument failed')
def second():
    events.append('second')
    assert all(reference() is not None for reference in references)
    raise original

try:
    methods.evaluate_pair(factory, first, second)
except ValueError as error:
    assert error is original
else:
    raise AssertionError('argument error was lost')
original.__traceback__ = None
gc.collect()
assert all(reference() is None for reference in references), events
assert events[:3] == ['lookup', 'first', 'second'], events
assert sorted(events[3:]) == ['callable released', 'first released', 'receiver released']
"""
    work = tmp_path / "argument-lifetime-profile"
    for mode in ("none", "profile", "apply"):
        methods.run(
            program + _entry_witness(entry_interpreter),
            entry_interpreter=entry_interpreter,
            opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(work)},
        )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_virtual_call_preserves_callbacks_and_releases_temporaries(
    methods, tmp_path, entry_interpreter
):
    ordinary_source = SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1)
    program = (
        "import methods\nimport types\n"
        "ordinary = types.ModuleType('ordinary_method_lifetime')\n"
        f"exec({ordinary_source!r}, vars(ordinary))\n"
        + r"""
import gc

class Argument:
    def __init__(self, events, label):
        self.events = events
        self.label = label
        events.append('call:' + label)

    def __del__(self):
        self.events.append(self.label)

def exercise(module, fail, operation):
    dispatcher = module.LifetimeTarget('dispatcher')
    module.LIFETIME_EVENTS.clear()
    def first():
        return Argument(module.LIFETIME_EVENTS, 'first')
    def second():
        return Argument(module.LIFETIME_EVENTS, 'second')
    try:
        assert getattr(dispatcher, operation)(fail, first, second) == 7
        assert not fail
    except ValueError as error:
        assert fail and str(error) == 'method failed'
        error.__traceback__ = None
    gc.collect()
    return list(module.LIFETIME_EVENTS)


def observe_quiescent(module, fail, operation):
    # Keep the original call/exception exercise and its in-exercise snapshot.
    # Its dispatcher is still alive at that snapshot; inspect required cleanup
    # only after the exercise returns, including release of that last receiver.
    snapshot = exercise(module, fail, operation)
    gc.collect()
    events = list(module.LIFETIME_EVENTS)
    explicit = ['call:first', 'call:second']
    if operation != 'invoke' and not fail:
        explicit.append('continued')
    implicit = {'dispatcher', 'receiver', 'first', 'second', 'previous'}
    assert [event for event in snapshot if event not in implicit] == explicit, snapshot
    assert [event for event in events if event not in implicit] == explicit, events
    released = ['dispatcher', 'receiver', 'first', 'second']
    if operation == 'replace_result':
        released.append('previous')
    actual_released = sorted(event for event in events if event in implicit)
    assert actual_released == sorted(released), events
    # Preserve exact-once cleanup, explicit argument/continuation order, return
    # values and source errors, not CPython's implicit release timing or order.
    return explicit, actual_released

for operation in ('invoke', 'invoke_then', 'replace_result'):
    for fail in (False, True):
        expected = observe_quiescent(ordinary, fail, operation)
        assert observe_quiescent(methods, fail, operation) == expected
"""
        + _entry_witness(entry_interpreter)
    )
    work = tmp_path / "method-lifetime-profile"
    for mode in ("none", "profile", "apply"):
        methods.run(
            program,
            entry_interpreter=entry_interpreter,
            opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(work)},
        )
