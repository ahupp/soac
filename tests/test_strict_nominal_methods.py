"""Nominal annotations preserve ordinary calls and authenticated class ownership."""


import pytest

from tests._strict_integration import create_strict_project


_RETAINED_EARLY_MODULE_NOMINAL_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from retained_early_nominal_support import exercise, move_alias

class Token:
    pass

Alias = Token

class Holder:
    def accept(self, /, value: Alias) -> Alias:
        move_alias(Holder.accept, Token)
        return value

# The actual class result has already enabled instances. Metadata must be
# immutable now, while this module's actual alias values remain per-call.
holder = Holder()
exercise(holder, Token)
"""

_RETAINED_EARLY_MODULE_NOMINAL_SUPPORT = """
import ctypes

expect_strict = True
events = []
old_values = []
shifting = False

def move_alias(function, current):
    if shifting:
        function.__globals__['Alias'] = current
    events.append('body')

def exercise(receiver, current):
    global shifting
    shifting = True
    from soac.strict import StrictMutationError

    function = type(receiver).accept
    strict_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
    strict_id.argtypes = [ctypes.py_object]
    strict_id.restype = ctypes.c_uint64
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    if expect_strict:
        assert strict_id(function), 'instances opened before function metadata sealing'
        assert sealed(type(receiver)) == 1
        try:
            function.__defaults__ = (object(),)
        except StrictMutationError:
            pass
        else:
            raise AssertionError('admitted method metadata remained mutable')
    else:
        assert not strict_id(function) and not sealed(type(receiver))

    class Previous:
        pass
    old = Previous()
    old_values.append(old)

    class Keyword(str):
        __hash__ = str.__hash__
        def __eq__(self, other):
            # This is the same actual binder callback as the initializing
            # nominal fixture, now on a mandatorily frozen class method.
            function.__globals__['Alias'] = Previous
            events.append('keyword')
            return str.__eq__(self, other)

    assert receiver.accept(**{Keyword('value'): old}) is old
    assert function.__globals__['Alias'] is current
    fresh = current()
    assert receiver.accept(fresh) is fresh
    before = list(events)
    assert receiver.accept(old) is old
    assert events == before + ['body']
    events.append('ordinary-next-call')
    shifting = False
"""


# Retained harness: Only imports the ordinary control after the validator changes its shared
# support flag and clears callback events. Eager scenario imports would execute it before this
# required setup.
@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_soac_early_sealed_method_keeps_metadata_seals_and_ordinary_binding_callbacks(
    tmp_path, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "retained_early_nominals.py": _RETAINED_EARLY_MODULE_NOMINAL_SOURCE,
            "ordinary_early_nominals.py": _RETAINED_EARLY_MODULE_NOMINAL_SOURCE.replace(
                "# soac: module(strict_assign=true, checked_attr=true)\n", ""
            ),
            "retained_early_nominal_support.py": _RETAINED_EARLY_MODULE_NOMINAL_SUPPORT,
        },
        modules={"retained_early_nominals": "retained_early_nominals.py"},
        backend="soac",
    )
    project.run_case(
        "retained_early_nominals",
        """
import retained_early_nominals as module
import retained_early_nominal_support as support

assert support.events == ['keyword', 'body', 'body', 'body', 'ordinary-next-call']
old = support.old_values[0]
value = module.Token()
assert module.holder.accept(value) is value
assert module.holder.accept(old) is old
assert module.Holder.accept.__defaults__ is None

support.expect_strict = False
support.events.clear()
import ordinary_early_nominals as ordinary
assert support.events == [
    'keyword', 'body', 'body', 'body', 'ordinary-next-call',
]
assert type(ordinary.holder) is ordinary.Holder
""",
        tmp_path / "retained_early_nominal_validation.py",
        required_functions=("Holder.accept",),
        
        entry_interpreter=entry_interpreter,
        backend="soac",
        opt_mode="none",
    )
