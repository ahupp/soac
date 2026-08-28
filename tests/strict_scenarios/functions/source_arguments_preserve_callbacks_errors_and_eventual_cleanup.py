# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:operand_model
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
# module:ordinary_operand_model
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
# ok
# tests/test_strict_function_boundaries.py::test_source_arguments_preserve_callbacks_errors_and_eventual_cleanup
import sys
from soac import _soac_ext, import_hook

import ctypes
import operand_model as actual
import ordinary_operand_model as ordinary
from soac import _soac_ext
owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert _soac_ext.strict_module_diagnostics(actual)['sealed']
assert _soac_ext.strict_module_diagnostics(ordinary) is None
for name in ('argument_keep', 'argument_delete', 'argument_rebind', 'argument_alias', 'argument_expanded'):
    function = getattr(actual, name)
    assert owner(function) and not owner(getattr(ordinary, name)), name
    assert _soac_ext.strict_function_entry_kind(function) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native'), name

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

mismatches = []
for name, caller, outcome, warmups in (('argument_keep', 'factory', 'success', 0), ('argument_keep', 'factory', 'error', 0), ('argument_keep', 'local', 'success', 0), ('argument_keep', 'local', 'error', 0), ('argument_delete', 'factory', 'success', 0), ('argument_delete', 'factory', 'error', 0), ('argument_delete', 'local', 'success', 0), ('argument_delete', 'local', 'error', 0), ('argument_rebind', 'factory', 'success', 0), ('argument_rebind', 'factory', 'error', 0), ('argument_rebind', 'local', 'success', 0), ('argument_rebind', 'local', 'error', 0), ('argument_alias', 'factory', 'success', 0), ('argument_alias', 'factory', 'error', 0), ('argument_alias', 'local', 'success', 0), ('argument_alias', 'local', 'error', 0), ('argument_keep', 'borrowed-c', 'success', 0), ('argument_keep', 'borrowed-c', 'error', 0), ('argument_expanded', 'expanded', 'success', 0), ('argument_expanded', 'expanded', 'error', 0), ('argument_expanded', 'expanded', 'success', 64), ('argument_expanded', 'expanded', 'error', 64)):
    expected = observe_source_argument_ownership(getattr(ordinary, name), caller, outcome, warmups)
    observed = observe_source_argument_ownership(getattr(actual, name), caller, outcome, warmups)
    if source_argument_semantics(observed) != source_argument_semantics(expected):
        mismatches.append(((name, caller, outcome, warmups), observed, expected))
assert not mismatches, mismatches
# ok
# tests/test_strict_function_boundaries.py::test_source_argument_cleanup_runs_finalizers_and_weakrefs_once_with_reentry
import sys
from soac import _soac_ext, import_hook

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
assert _soac_ext.strict_function_entry_kind(actual.retire_arguments) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
