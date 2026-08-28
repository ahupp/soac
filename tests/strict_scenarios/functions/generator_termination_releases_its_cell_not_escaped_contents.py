# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:captured
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
# module:ordinary_captured
# ordinary source control
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
# module:support
events = []

class Payload:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append("released:" + self.name)
# ok
# tests/test_strict_function_boundaries.py::test_generator_termination_releases_its_cell_not_escaped_contents
import sys
from soac import _soac_ext, import_hook

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
    assert observations(module, ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')) == ordinary

validate(module)
