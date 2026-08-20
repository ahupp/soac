import sys
import unittest


def _boom():
    try:
        raise ValueError("boom")
    except ValueError:
        raise ValueError("boom")


def run():
    case = unittest.TestCase()
    before = sys.getrefcount(_boom)
    case.assertRaises(ValueError, _boom)
    return before, sys.getrefcount(_boom)

# diet-python: validate

def validate_module(module):
    import gc

    before, after = module.run()
    if not __dp_integration_soac__:
        # Preserve the original ordinary CPython in-body count control. SOAC
        # may choose different temporary owners while this function executes.
        assert before == after

    # Keep the exception behavior observable independently of assertRaises.
    # Clear the captured tracebacks before measuring post-return ownership.
    handled = sys.exception()
    try:
        module._boom()
    except ValueError as error:
        assert type(error) is ValueError and error.args == ("boom",)
        context = error.__context__
        assert type(context) is ValueError and context.args == ("boom",)
        assert context is not error and context.__context__ is handled
        assert error.__cause__ is None and not error.__suppress_context__
        assert context.__cause__ is None and not context.__suppress_context__
        error.__traceback__ = None
        context.__traceback__ = None
        del context
    else:
        raise AssertionError("_boom did not raise ValueError")
    assert sys.exception() is handled
    del handled

    # Both samples are in the ordinary caller, after the source frames and
    # assertRaises contexts have returned. Warm first so this checks retained
    # ownership across repeated calls, not one-time compilation metadata.
    for _ in range(8):
        module.run()
    gc.collect()
    before = sys.getrefcount(module._boom)
    for _ in range(64):
        module.run()
    gc.collect()
    assert sys.getrefcount(module._boom) == before
