import ctypes
import importlib.util
import json

import pytest

from tests._integration import split_integration_case
from tests._strict_integration import ROOT, create_strict_project


def test_selected_native_handled_exception_layout(function_create_watch_extension):
    spec = importlib.util.spec_from_file_location(
        "_strict_function_create_watch", function_create_watch_extension
    )
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    layout = native.handled_exception_layout()
    print("selected native handled-exception layout:", layout)
    pointer_size = ctypes.sizeof(ctypes.c_void_p)
    assert layout["item_size"] == 2 * pointer_size
    assert layout["item_alignment"] == ctypes.alignment(ctypes.c_void_p)
    assert layout["item_value"] == 0
    assert layout["item_previous"] == pointer_size
    # Keep the selected native header coupled to the raw JIT mirror. Its
    # companion Rust layout test asserts this same pinned-CPython offset.
    assert layout["thread_exc_info"] == 136


_HANDLED_EXCEPTION_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
import sys

def exercise_plain():
    result = None
    try:
        raise ValueError('plain')
    except ValueError:
        result = sys.exception()
    return result

def exercise_group():
    result = None
    try:
        raise ExceptionGroup('group', [ValueError('member')])
    except* ValueError:
        result = sys.exception()
    return result

def make_handled_generator():
    def gen():
        try:
            raise ValueError('generator')
        except ValueError as error:
            yield error, sys.exception()
            yield error, sys.exception()
    return gen()

def make_handled_coroutine():
    async def coro():
        try:
            raise ValueError('coroutine')
        except ValueError:
            return sys.exception()
    return coro()

def make_nested_handled_generator(set_handled, replacement):
    def gen():
        try:
            raise ValueError('outer generator')
        except ValueError:
            set_handled(replacement)
            yield 'outer', sys.exception()
            try:
                raise LookupError('inner generator')
            except LookupError as inner:
                yield 'inner', sys.exception(), inner
                yield 'inner resumed', sys.exception(), inner
            yield 'outer restored', sys.exception()
        yield 'caller inherited', sys.exception()
    return gen()

def set_outside_handler(set_handled, replacement):
    set_handled(replacement)

def make_setter_generator(set_handled, replacement):
    def gen():
        set_handled(replacement)
        yield sys.exception()
        yield sys.exception()
    return gen()

def reraise_replaced_handler(set_handled, replacement):
    try:
        raise ValueError('original bare-raise handler')
    except ValueError:
        set_handled(replacement)
        raise

def cleanup_on_return(make_probe):
    try:
        raise ValueError('return handler')
    except ValueError:
        probe = make_probe('return')
        return probe is not None

def cleanup_by_delete(make_probe):
    try:
        raise ValueError('delete handler')
    except ValueError as caught:
        probe = make_probe('delete')
        del probe
        return caught

def cleanup_after_handler(make_probe):
    try:
        raise ValueError('jump handler')
    except ValueError:
        probe = make_probe('jump')
    del probe

def deopt_handled_add(produce, observe):
    try:
        raise ValueError('deopt handler')
    except ValueError:
        observe('before')
        value = produce()
        result = value + 1
        observe('after')
        return result

def make_cleanup_generator(make_probe, return_value=True):
    def gen():
        try:
            raise ValueError('generator cleanup handler')
        except ValueError:
            probe = make_probe('generator')
            yield None
            return return_value if probe is not None else False
    return gen()

def make_cleanup_coroutine(make_probe, pause, return_value=True):
    async def coro():
        try:
            raise ValueError('coroutine cleanup handler')
        except ValueError:
            probe = make_probe('coroutine')
            await pause()
            return return_value if probe is not None else False
    return coro()

def make_pep479_generator():
    def gen():
        try:
            raise ValueError('source handler before PEP 479')
        except ValueError as error:
            yield error
            raise StopIteration('source StopIteration')
    return gen()

def make_pep479_coroutine(pause):
    async def coro():
        try:
            raise ValueError('source handler before PEP 479')
        except ValueError as error:
            await pause(error)
            raise StopIteration('source StopIteration')
    return coro()
"""


@pytest.fixture(scope="module")
def strict_handled_exception_project(tmp_path_factory):
    source, _ = split_integration_case(
        ROOT / "tests/integration_modules/asyncio_taskgroup_base_error_refcycle.py"
    )
    return create_strict_project(
        tmp_path_factory.mktemp("strict-handled-exception-state"),
        {
            "handled_exception_state.py": _HANDLED_EXCEPTION_SOURCE,
            "taskgroup_exception_lifetime.py": "# soac: module(strict_assign=true, checked_attr=true)\n"
            + source,
        },
        modules={
            "handled_exception_state": "handled_exception_state.py",
            "taskgroup_exception_lifetime": "taskgroup_exception_lifetime.py",
        },
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("case", ["plain", "group", "generator", "coroutine"])
def test_strict_handled_exception_state_is_scoped_to_the_active_call(
    strict_handled_exception_project, entry_interpreter, case
):
    strict_handled_exception_project.run(
        f"case = {case!r}\n"
        f"expected_entry = {'entry_interpreter' if entry_interpreter else 'checked_native'!r}\n"
        + """
import asyncio
import ctypes
import sys
import handled_exception_state as module

assert _soac_ext.strict_module_diagnostics(module)['sealed']
name = {
    'plain': 'exercise_plain',
    'group': 'exercise_group',
    'generator': 'make_handled_generator',
    'coroutine': 'make_handled_coroutine',
}[case]
function = vars(module)[name]
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert owner(function)
assert _soac_ext.strict_function_entry_kind(function) == expected_entry

def exercise(caller_exception):
    assert sys.exception() is caller_exception
    if case == 'generator':
        generator = function()
        first = next(generator)
        assert first[0] is first[1] and type(first[0]) is ValueError
        assert sys.exception() is caller_exception, 'yield leaked the generator handler'
        second = next(generator)
        assert second[0] is second[1] is first[0]
        assert sys.exception() is caller_exception, 'resume leaked the generator handler'
        try:
            next(generator)
        except StopIteration:
            pass
        else:
            raise AssertionError('generator did not complete')
    elif case == 'coroutine':
        result = asyncio.run(function())
        assert type(result) is ValueError
    else:
        result = function()
        assert type(result) is (ExceptionGroup if case == 'group' else ValueError)
    assert sys.exception() is caller_exception, 'completed handler changed caller state'

exercise(None)
try:
    raise RuntimeError('outer caller handler')
except RuntimeError as caller_exception:
    exercise(caller_exception)
assert sys.exception() is None
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_strict_taskgroup_exception_does_not_retain_runtime_helper_frames(
    strict_handled_exception_project, entry_interpreter
):
    strict_handled_exception_project.run(
        f"expected_entry = {'entry_interpreter' if entry_interpreter else 'checked_native'!r}\n"
        + """
import sys
import taskgroup_exception_lifetime as module

assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert _soac_ext.strict_function_entry_kind(module.referrer_frames) == expected_entry
assert sys.exception() is None
# The semantic comprehension carrier now supplies this original source local.
# Keep the original post-GC leak observation, not the retired projection refusal.
frames = module.referrer_frames()
assert not frames, [(frame.f_code.co_name, frame.f_code.co_filename) for frame in frames]
assert sys.exception() is None
""",
        entry_interpreter=entry_interpreter,
    )


def test_cpython_taskgroup_exception_does_not_retain_runtime_helper_frames(tmp_path):
    source, _ = split_integration_case(
        ROOT / "tests/integration_modules/asyncio_taskgroup_base_error_refcycle.py"
    )
    project = create_strict_project(
        tmp_path / "cpython-taskgroup-exception-lifetime",
        {"taskgroup_exception_lifetime.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + source},
        modules={"taskgroup_exception_lifetime": "taskgroup_exception_lifetime.py"},
        backend="cpython",
    )
    project.run_case(
        "taskgroup_exception_lifetime",
        """
import sys
import taskgroup_exception_lifetime as module

assert sys.exception() is None
frames = module.referrer_frames()
assert not frames, [(frame.f_code.co_name, frame.f_code.co_filename) for frame in frames]
assert sys.exception() is None
""",
        ROOT / "tests/integration_modules/asyncio_taskgroup_base_error_refcycle.py",
        required_functions=("referrer_frames",),
        backend="cpython",
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_nested_generator_handlers_preserve_capi_changes_across_suspension(
    strict_handled_exception_project, function_create_watch_extension, entry_interpreter
):
    strict_handled_exception_project.run(
        f"extension_path = {str(function_create_watch_extension)!r}\n"
        + """
import gc
import importlib.util
import sys
import weakref
import handled_exception_state as module

spec = importlib.util.spec_from_file_location('_strict_function_create_watch', extension_path)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)
# Emit actual selected-header evidence for the raw runtime layout assertions.
print('handled_exception_layout', native.handled_exception_layout(), flush=True)
assert _soac_ext.strict_module_diagnostics(module)['sealed']

class Payload:
    pass

replacement = RuntimeError('C API replacement')
payload = Payload()
replacement.payload = payload
payload_ref = weakref.ref(payload)
del payload
generator = module.make_nested_handled_generator(native.set_handled_exception, replacement)
assert sys.exception() is None
assert next(generator) == ('outer', replacement)
assert sys.exception() is None
first_inner = next(generator)
assert first_inner[0] == 'inner' and first_inner[1] is first_inner[2]
assert type(first_inner[1]) is LookupError
assert first_inner[1].__context__ is replacement
assert sys.exception() is None
try:
    raise OSError('different resuming caller')
except OSError as caller:
    second_inner = next(generator)
    assert second_inner[0] == 'inner resumed'
    assert second_inner[1] is second_inner[2] is first_inner[1]
    assert sys.exception() is caller
    assert next(generator) == ('outer restored', replacement)
    assert sys.exception() is caller
    inherited = next(generator)
    assert inherited[0] == 'caller inherited' and inherited[1] is caller
    assert sys.exception() is caller
    try:
        next(generator)
    except StopIteration:
        pass
    else:
        raise AssertionError('generator did not finish')
    assert sys.exception() is caller
assert sys.exception() is None

# A normal function shares the actual current native item. Do not isolate a
# supported setter call merely because that function has no Python handler.
try:
    module.set_outside_handler(native.set_handled_exception, replacement)
    assert sys.exception() is replacement
finally:
    native.set_handled_exception(None)
assert sys.exception() is None

del generator, first_inner, second_inner, inherited, replacement
gc.collect()
gc.collect()
assert payload_ref() is None, 'suspended handler stack retained its exception after completion'
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_bare_raise_uses_the_current_capi_replaced_handler(
    strict_handled_exception_project, function_create_watch_extension, entry_interpreter
):
    strict_handled_exception_project.run(
        f"extension_path = {str(function_create_watch_extension)!r}\n"
        + """
import importlib.util
import sys
import handled_exception_state as module

spec = importlib.util.spec_from_file_location('_strict_function_create_watch', extension_path)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)
assert _soac_ext.strict_module_diagnostics(module)['sealed']
replacement = LookupError('replacement used by bare raise')
try:
    raise OSError('outside handler')
except OSError as outside:
    try:
        module.reraise_replaced_handler(native.set_handled_exception, replacement)
    except LookupError as actual:
        assert actual is replacement
        assert sys.exception() is replacement
    else:
        raise AssertionError('bare raise did not use the replaced handled exception')
    assert sys.exception() is outside
assert sys.exception() is None
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generator_without_static_handlers_still_owns_its_capi_exception_item(
    strict_handled_exception_project, function_create_watch_extension, entry_interpreter
):
    strict_handled_exception_project.run(
        f"extension_path = {str(function_create_watch_extension)!r}\n"
        + """
import importlib.util
import sys
import handled_exception_state as module

spec = importlib.util.spec_from_file_location('_strict_function_create_watch', extension_path)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)
assert _soac_ext.strict_module_diagnostics(module)['sealed']
replacement = RuntimeError('generator-private CAPI state without a source handler')
generator = module.make_setter_generator(native.set_handled_exception, replacement)
assert next(generator) is replacement
assert sys.exception() is None
try:
    raise OSError('different caller on resumption')
except OSError as outside:
    assert next(generator) is replacement
    assert sys.exception() is outside
    try:
        next(generator)
    except StopIteration as completed:
        assert completed.__context__ is outside
    else:
        raise AssertionError('generator did not complete')
    assert sys.exception() is outside
assert sys.exception() is None
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_handler_cleanup_preserves_deletion_and_releases_locals(
    strict_handled_exception_project, entry_interpreter
):
    control_source = _HANDLED_EXCEPTION_SOURCE.replace(
        "# soac: module(strict_assign=true, checked_attr=true)\n", ""
    )
    strict_handled_exception_project.run(
        f"control_source = {control_source!r}\n"
        f"expected_entry = {'entry_interpreter' if entry_interpreter else 'checked_native'!r}\n"
        + """
import gc
import sys
import types
import weakref
import handled_exception_state as module

assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert _soac_ext.strict_function_entry_kind(module.cleanup_on_return) == expected_entry
control = types.ModuleType('ordinary_handled_cleanup_control')
exec(compile(control_source, '<ordinary-handled-cleanup-control>', 'exec'), vars(control))

events = []
def exercise(target, outside):
    native_schedule = target is control
    references = {}
    delete_handler = []
    class Probe:
        def __init__(self, label):
            self.label = label
            references[label] = weakref.ref(self)
            if label == 'delete':
                # Construction is an explicit source callback. Keep the actual
                # caught exception identity independently of finalizer timing.
                delete_handler.append(sys.exception())
        def __del__(self):
            # Automatic finalization remains implicit even after source del.
            current = sys.exception() if native_schedule else None
            events.append((self.label, current))

    assert sys.exception() is outside
    assert target.cleanup_on_return(Probe) is True
    assert sys.exception() is outside
    if native_schedule:
        assert events == [('return', outside)], events
    else:
        gc.collect()
        assert events == [('return', None)], events
    assert references['return']() is None
    events.clear()
    caught = target.cleanup_by_delete(Probe)
    assert type(caught) is ValueError
    assert caught is delete_handler[0]
    assert caught.__context__ is outside
    assert sys.exception() is outside
    if native_schedule:
        assert events == [('delete', caught)], events
    else:
        gc.collect()
        assert events == [('delete', None)], events
    # Retain the returned exception and its source traceback: deleting probe
    # must still leave no live probe owner in that retained source frame.
    assert references['delete']() is None
    events.clear()
    target.cleanup_after_handler(Probe)
    assert sys.exception() is outside
    if native_schedule:
        assert events == [('jump', outside)], events
    else:
        gc.collect()
        assert events == [('jump', None)], events
    assert references['jump']() is None
    events.clear()
    assert sys.exception() is outside

for target in (control, module):
    exercise(target, None)
    try:
        raise OSError('caller observed by finalizers')
    except OSError as outside:
        exercise(target, outside)
    assert sys.exception() is None
""",
        entry_interpreter=entry_interpreter,
    )


def test_profiled_execution_keeps_the_original_active_handler_and_restores_once(
    strict_handled_exception_project, tmp_path
):
    program = """
import sys
import handled_exception_state as module

assert _soac_ext.strict_module_diagnostics(module)['sealed']
assert _soac_ext.strict_function_entry_kind(module.deopt_handled_add) == 'checked_native'
events = []
produced = [40]

def produce():
    return produced[0]

def observe(label):
    events.append((label, sys.exception()))

class Addition:
    def __init__(self, fail):
        self.fail = fail
    def __add__(self, right):
        assert right == 1
        events.append(('add', sys.exception()))
        if self.fail:
            raise LookupError('deoptimized operation')
        return 2001

try:
    raise OSError('original caller handler')
except OSError as outside:
    for _ in range(200):
        assert module.deopt_handled_add(produce, observe) == 41
        assert [label for label, _ in events] == ['before', 'after']
        assert events[0][1] is events[1][1]
        assert type(events[0][1]) is ValueError
        assert events[0][1].__context__ is outside
        assert sys.exception() is outside
        events.clear()
    if validate_misses:
        # The value comes from an ordinary callback *inside* the handler, not
        # from an entry argument whose guard could fail before handler entry.
        produced[0] = Addition(False)
        assert module.deopt_handled_add(produce, observe) == 2001
        assert [label for label, _ in events] == ['before', 'add', 'after']
        assert events[0][1] is events[1][1] is events[2][1]
        assert events[0][1].__context__ is outside
        assert sys.exception() is outside
        events.clear()
        produced[0] = Addition(True)
        try:
            module.deopt_handled_add(produce, observe)
        except LookupError as escaped:
            assert [label for label, _ in events] == ['before', 'add']
            assert events[0][1] is events[1][1] is escaped.__context__
            assert escaped.__context__.__context__ is outside
        else:
            raise AssertionError('deoptimized error did not escape')
        assert sys.exception() is outside
        events.clear()
assert sys.exception() is None
"""
    work = tmp_path / "handled-deopt-profile"
    for mode in ("profile", "apply", "verify"):
        strict_handled_exception_project.run(
            f"validate_misses = {mode != 'profile'!r}\n" + program,
            opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(work)},
        )
    from soac import _soac_ext

    counters = json.loads(_soac_ext.inspect_counter_dump_json(str(work / "verify.bin")))
    rows = [
        row
        for record in counters["records"]
        if record["module_name"] == "handled_exception_state"
        for row in record["rows"]
        if row["function_qualname"] == "deopt_handled_add"
    ]
    # This checks actual profiled execution of the authenticated body without
    # presuming that an optional optimization selects a native-to-deopt exit.
    # The structured runtime test
    # profiled_deopt_inside_handler_preserves_borrowed_argument_ownership
    # selects that exit explicitly and checks the same handler identity/context,
    # caller restoration and normal/error ownership through the real handoff.
    assert any(
        row["kind"] == "call_hot_targets" and row["value"] >= 202 for row in rows
    ), rows


@pytest.mark.parametrize("entry_interpreter", [None, False, True])
def test_suspended_completion_keeps_pep479_context_and_releases_saved_locals(
    strict_handled_exception_project, entry_interpreter
):
    control_source = _HANDLED_EXCEPTION_SOURCE.replace(
        "# soac: module(strict_assign=true, checked_attr=true)\n", ""
    )
    strict_handled_exception_project.run(
        f"native_schedule = {entry_interpreter is None!r}\n"
        f"control_source = {control_source!r}\n"
        + """
import gc
import sys
import types
import weakref

if native_schedule:
    module = types.ModuleType('ordinary_suspended_cleanup_control')
    exec(compile(control_source, '<ordinary-suspended-cleanup-control>', 'exec'), vars(module))
else:
    import handled_exception_state as module
    assert _soac_ext.strict_module_diagnostics(module)['sealed']
events = []
probe_refs = []
class Probe:
    def __init__(self, label):
        self.label = label
        probe_refs.append(weakref.ref(self))
    def __del__(self):
        events.append((self.label, sys.exception() if native_schedule else None))

class Pause:
    def __init__(self, value=None):
        self.value = value
    def __await__(self):
        yield self.value

def exercise(outside):
    for kind in ('generator', 'coroutine'):
        for action, return_value in (
            ('return', None), ('return', True), ('return', (1, 2)),
            ('return', ValueError('returned exception value, not a source raise')),
            ('close', True), ('throw', True),
        ):
            if kind == 'generator':
                suspended = module.make_cleanup_generator(Probe, return_value)
            else:
                suspended = module.make_cleanup_coroutine(Probe, Pause, return_value)
            assert suspended.send(None) is None
            assert events == []
            assert probe_refs[-1]() is not None
            assert sys.exception() is outside
            if action == 'return':
                try:
                    suspended.send(None)
                except StopIteration as completed:
                    assert completed.value is return_value
                    assert completed.__context__ is (outside if return_value is None else None)
                else:
                    raise AssertionError('suspended body did not complete')
            elif action == 'close':
                assert suspended.close() is None
            else:
                injected = LookupError('injected into active handler')
                try:
                    suspended.throw(injected)
                except LookupError as actual:
                    assert actual is injected
                    assert type(actual.__context__) is ValueError
                    assert actual.__context__.__context__ is outside
                else:
                    raise AssertionError('throw did not escape')
                # CPython tracebacks retain source frames until the caller
                # releases the injected exception; do not use that as a leak.
                injected.__traceback__ = None
                injected.__context__.__traceback__ = None
                del injected
            if native_schedule:
                assert events == [(kind, outside)], (kind, action, events)
            else:
                gc.collect()
                assert events == [(kind, None)], (kind, action, events)
            assert probe_refs[-1]() is None, 'completed frame retained its source local'
            assert sys.exception() is outside
            events.clear()
        if kind == 'generator':
            suspended = module.make_pep479_generator()
        else:
            suspended = module.make_pep479_coroutine(Pause)
        original = suspended.send(None)
        assert type(original) is ValueError
        assert original.__context__ is outside
        try:
            suspended.send(None)
        except RuntimeError as converted:
            assert type(converted.__cause__) is StopIteration
            assert converted.__context__ is converted.__cause__
            assert converted.__cause__.__context__ is original
        else:
            raise AssertionError('source StopIteration did not receive PEP 479 conversion')
        assert sys.exception() is outside

exercise(None)
try:
    raise OSError('caller during suspended frame cleanup')
except OSError as outside:
    exercise(outside)
assert sys.exception() is None
""",
        entry_interpreter=entry_interpreter is True,
    )


_EXCEPTION_TRANSPORT_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def nested_return(error_factory, observe, set_handled, replacement):
    try:
        raise error_factory('outer')
    except Exception:
        try:
            raise error_factory('inner')
        except Exception:
            return 17

def nested_fallthrough(error_factory, observe, set_handled, replacement):
    try:
        raise error_factory('outer')
    except Exception:
        try:
            raise error_factory('inner')
        except Exception:
            pass
        observe('after inner')
    observe('after outer')

def replace_active(error_factory, observe, set_handled, replacement):
    try:
        raise error_factory('original')
    except Exception:
        set_handled(replacement)
        observe('after replacement')
"""


@pytest.fixture(scope="module")
def strict_exception_transport_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-exception-transport"),
        {"exception_transport.py": _EXCEPTION_TRANSPORT_SOURCE},
        modules={"exception_transport": "exception_transport.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [None, False, True])
@pytest.mark.parametrize(
    "case", ["nested_return", "nested_fallthrough", "replace_active"]
)
def test_caught_exception_payload_preserves_callbacks_and_retires_every_owner(
    strict_exception_transport_project,
    function_create_watch_extension,
    entry_interpreter,
    case,
):
    control_source = _EXCEPTION_TRANSPORT_SOURCE.replace(
        "# soac: module(strict_assign=true, checked_attr=true)\n", ""
    )
    strict_exception_transport_project.run(
        f"case = {case!r}\n"
        f"native_schedule = {entry_interpreter is None!r}\n"
        f"control_source = {control_source!r}\n"
        f"extension_path = {str(function_create_watch_extension)!r}\n"
        f"expected_entry = {'entry_interpreter' if entry_interpreter else 'checked_native'!r}\n"
        + """
import gc
import importlib.util
import sys
import weakref

spec = importlib.util.spec_from_file_location('_strict_function_create_watch', extension_path)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)
if native_schedule:
    namespace = {}
    exec(compile(control_source, '<ordinary-exception-transport>', 'exec'), namespace)
    function = namespace[case]
else:
    import exception_transport as module
    assert _soac_ext.strict_module_diagnostics(module)['sealed']
    function = vars(module)[case]
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry

events = []
refs = []
class ObservedError(Exception):
    def __init__(self, label):
        self.label = label
        refs.append(weakref.ref(self))
    def __del__(self):
        if native_schedule:
            current = sys.exception()
            events.append(('drop', self.label, getattr(current, 'label', None)))
        else:
            events.append(('drop', self.label))

def observe(label):
    events.append((label, getattr(sys.exception(), 'label', None)))

replacement = RuntimeError('replacement')
replacement.label = 'replacement'
try:
    raise OSError('caller')
except OSError as caller:
    caller.label = 'caller'
    result = function(ObservedError, observe, native.set_handled_exception, replacement)
    assert sys.exception() is caller
    expected = {
        'nested_return': [('drop', 'inner', 'outer'), ('drop', 'outer', 'caller')],
        'nested_fallthrough': [
            ('drop', 'inner', 'outer'), ('after inner', 'outer'),
            ('drop', 'outer', 'caller'), ('after outer', 'caller'),
        ],
        'replace_active': [
            ('drop', 'original', 'replacement'), ('after replacement', 'replacement'),
        ],
    }[case]
    assert result == (17 if case == 'nested_return' else None)
    if native_schedule:
        assert events == expected, (case, 'before GC', events, expected)
        gc.collect()
        assert events == expected, (case, 'GC delayed exception destruction', events, expected)
    else:
        assert [event for event in events if event[0] != 'drop'] == [
            event for event in expected if event[0] != 'drop'
        ], (case, events)
        gc.collect()
        labels = ['original'] if case == 'replace_active' else ['inner', 'outer']
        assert sorted(event[1] for event in events if event[0] == 'drop') == labels, events
    assert all(ref() is None for ref in refs), 'completed call retained an exception'
    assert sys.exception() is caller
assert sys.exception() is None
""",
        entry_interpreter=entry_interpreter is True,
    )


_PENDING_RETURN_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def replace_return(make, observe):
    try:
        return make('pending')
    finally:
        observe('finally')
        return make('replacement')

def replace_raise(make, observe):
    try:
        return make('pending')
    finally:
        observe('finally')
        raise RuntimeError('replacement')

def replace_break(make, observe):
    for unused in (0,):
        try:
            return make('pending')
        finally:
            observe('finally')
            break
    observe('after finally')

def handled_return(make, observe):
    try:
        raise LookupError('source')
    except LookupError:
        try:
            return make('pending')
        finally:
            observe('finally')
            return make('replacement')

def handled_raise(make, observe):
    try:
        raise LookupError('source')
    except LookupError:
        try:
            return make('pending')
        finally:
            observe('finally')
            raise RuntimeError('replacement')

def interleaved_return(make, observe):
    try:
        return make('pending')
    finally:
        try:
            raise LookupError('source')
        except LookupError:
            try:
                return make('inner')
            finally:
                observe('finally')
                return make('replacement')
"""


_SUSPENDED_TRANSPORT_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def group():
    def generate():
        try:
            raise ExceptionGroup('group', [ValueError('handled'), TypeError('remaining')])
        except* ValueError:
            yield 1
    return generate()

def retire(error_factory, observe, set_handled, replacement):
    def generate():
        try:
            raise error_factory()
        except ValueError:
            yield 1
        observe('after handler')
        yield 2
    return generate()

def replace(error_factory, observe, set_handled, replacement):
    def generate():
        try:
            raise error_factory()
        except ValueError:
            set_handled(replacement)
            yield 1
            observe('inside handler')
        observe('after handler')
        yield 2
    return generate()
"""


@pytest.fixture(scope="module")
def strict_suspended_transport_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-suspended-transport"),
        {"suspended_transport.py": _SUSPENDED_TRANSPORT_SOURCE},
        modules={"suspended_transport": "suspended_transport.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [None, False, True])
@pytest.mark.parametrize("case", ["group", "retire", "replace"])
def test_suspended_exception_transport_keeps_semantic_reads_and_retires_every_owner(
    strict_suspended_transport_project,
    function_create_watch_extension,
    entry_interpreter,
    case,
):
    strict_suspended_transport_project.run(
        f"case = {case!r}\n"
        f"native_control = {entry_interpreter is None!r}\n"
        f"source = {_SUSPENDED_TRANSPORT_SOURCE!r}\n"
        f"extension_path = {str(function_create_watch_extension)!r}\n"
        f"expected_entry = {'entry_interpreter' if entry_interpreter else 'checked_native'!r}\n"
        + """
import gc
import importlib.util
import sys
import weakref

if native_control:
    namespace = {}
    exec(compile(source.replace('# soac: module(strict_assign=true, checked_attr=true)\\n', '\\n'),
                 '<native-suspended-transport>', 'exec'), namespace)
    factory = namespace[case]
else:
    import suspended_transport as module
    assert _soac_ext.strict_module_diagnostics(module)['sealed']
    factory = vars(module)[case]
    assert _soac_ext.strict_function_entry_kind(factory) == expected_entry

spec = importlib.util.spec_from_file_location('_strict_function_create_watch', extension_path)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)
events = []
refs = []
def current():
    error = sys.exception()
    return str(error) if error is not None else None

class ObservedError(ValueError):
    def __del__(self):
        events.append(('drop', current()) if native_control else ('drop',))

def error_factory():
    error = ObservedError('caught')
    refs.append(weakref.ref(error))
    return error

def observe(label):
    events.append((label, current()))
    if native_control:
        assert all(ref() is None for ref in refs), (label, 'stale exception transport')

replacement = RuntimeError('replacement')
try:
    raise OSError('caller')
except OSError as caller:
    if case == 'group':
        generator = factory()
        assert next(generator) == 1
        try:
            next(generator)
        except ExceptionGroup as remaining:
            assert len(remaining.exceptions) == 1
            assert type(remaining.exceptions[0]) is TypeError
            assert str(remaining.exceptions[0]) == 'remaining'
        else:
            raise AssertionError('post-yield except* merge lost its original group')
    else:
        generator = factory(error_factory, observe, native.set_handled_exception, replacement)
        assert next(generator) == 1
        assert sys.exception() is caller
        if case == 'replace':
            if native_control:
                assert events == [('drop', 'replacement')], ('before resume', events)
            else:
                assert not [event for event in events if event[0] != 'drop'], events
        else:
            assert events == []
        assert next(generator) == 2
        expected = ([('drop', 'caller'), ('after handler', 'caller')] if case == 'retire'
                    else [('drop', 'replacement'), ('inside handler', 'replacement'),
                          ('after handler', 'caller')])
        if native_control:
            assert events == expected, (case, 'before GC', events, expected)
        else:
            assert [event for event in events if event[0] != 'drop'] == [
                event for event in expected if event[0] != 'drop'
            ], (case, events)
    assert sys.exception() is caller
    generator.close()
    del generator
    before_gc = events.copy()
    gc.collect()
    if native_control:
        assert events == before_gc
    else:
        assert sum(event[0] == 'drop' for event in events) == (0 if case == 'group' else 1), events
    assert all(ref() is None for ref in refs)
assert sys.exception() is None
""",
        entry_interpreter=entry_interpreter is True,
    )


@pytest.fixture(scope="module")
def strict_pending_return_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-pending-return"),
        {"pending_return.py": _PENDING_RETURN_SOURCE},
        modules={"pending_return": "pending_return.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [None, False, True])
@pytest.mark.parametrize(
    "case",
    [
        "replace_return",
        "replace_raise",
        "replace_break",
        "handled_return",
        "handled_raise",
        "interleaved_return",
    ],
)
def test_pending_return_preserves_finally_values_and_retires_overridden_owners(
    strict_pending_return_project, entry_interpreter, case
):
    control_source = _PENDING_RETURN_SOURCE.replace(
        "# soac: module(strict_assign=true, checked_attr=true)\n", ""
    )
    strict_pending_return_project.run(
        f"case = {case!r}\n"
        f"native_schedule = {entry_interpreter is None!r}\n"
        f"control_source = {control_source!r}\n"
        f"expected_entry = {'entry_interpreter' if entry_interpreter else 'checked_native'!r}\n"
        + """
import gc
import sys
import weakref

if native_schedule:
    namespace = {}
    exec(compile(control_source, '<ordinary-pending-return>', 'exec'), namespace)
    function = namespace[case]
else:
    import pending_return as module
    assert _soac_ext.strict_module_diagnostics(module)['sealed']
    function = vars(module)[case]
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry
events = []
refs = {}
def current():
    error = sys.exception()
    return str(error) if error is not None else None

class Value:
    def __init__(self, label):
        self.label = label
        refs[label] = weakref.ref(self)
    def __del__(self):
        if native_schedule:
            events.append(('drop', self.label, current()))
        else:
            events.append(('drop', self.label))

def make(label):
    events.append(('make', label, current()))
    return Value(label)

def observe(label):
    if native_schedule:
        events.append((label, current(), [name for name, ref in refs.items() if ref() is not None]))
    else:
        events.append((label, current()))

try:
    raise OSError('caller')
except OSError as caller:
    try:
        result = function(make, observe)
    except RuntimeError as escaped:
        assert case.endswith('raise'), (case, escaped)
        events.append(('caught', current()))
    else:
        assert not case.endswith('raise')
        events.append(('returned', current()))
        if case.endswith('return'):
            assert refs['replacement']() is result
        else:
            assert result is None
        del result
    assert sys.exception() is caller
    active = 'source' if case.startswith('handled') else 'caller'
    expected = [('make', 'pending', active), ('finally', active, ['pending'])]
    if case == 'interleaved_return':
        expected = [
            ('make', 'pending', 'caller'), ('make', 'inner', 'source'),
            ('finally', 'source', ['pending', 'inner']),
            ('make', 'replacement', 'source'), ('drop', 'inner', 'source'),
            ('drop', 'pending', 'caller'), ('returned', 'caller'),
            ('drop', 'replacement', 'caller'),
        ]
    elif case.endswith('return'):
        expected += [
            ('make', 'replacement', active), ('drop', 'pending', active),
            ('returned', 'caller'), ('drop', 'replacement', 'caller'),
        ]
    elif case.endswith('raise'):
        expected += [('drop', 'pending', active), ('caught', 'replacement')]
    else:
        expected += [
            ('drop', 'pending', active), ('after finally', 'caller', []), ('returned', 'caller'),
        ]
    if native_schedule:
        assert events == expected, (case, 'before GC', events, expected)
        gc.collect()
        assert events == expected, (case, 'GC changed pending-return lifetime', events, expected)
    else:
        explicit = [event for event in events if event[0] != 'drop']
        expected_explicit = [
            event[:2] if event[0] in ('finally', 'after finally') else event
            for event in expected if event[0] != 'drop'
        ]
        assert explicit == expected_explicit, (case, explicit, expected_explicit)
        gc.collect()
        assert sorted(event[1] for event in events if event[0] == 'drop') == sorted(refs), events
    assert all(ref() is None for ref in refs.values())
assert sys.exception() is None
""",
        entry_interpreter=entry_interpreter is True,
    )
