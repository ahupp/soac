# modes:cpython
# module:ordinary_pending_setup
import pending_type_support as support
support.expect_pending = False
import ordinary_pending_type
# module:pending_type_support
import ctypes

expect_pending = True
observing = False
events = []
observed = []

def observe(cls):
    global observing
    if observing:
        return
    observing = True
    try:
        from soac.strict import StrictMutationError
        from soac import _soac_ext

        alloc = ctypes.pythonapi.PyType_GenericAlloc
        alloc.argtypes = [ctypes.py_object, ctypes.c_ssize_t]
        alloc.restype = ctypes.py_object
        call = ctypes.pythonapi.PyObject_CallNoArgs
        call.argtypes = [ctypes.py_object]
        call.restype = ctypes.py_object
        assign = ctypes.pythonapi.PyObject_SetAttr
        assign.argtypes = [ctypes.py_object] * 3
        assign.restype = ctypes.c_int
        own_contract = ctypes.pythonapi.PyType_HasSoacContract
        own_contract.argtypes = [ctypes.py_object]
        own_contract.restype = ctypes.c_int

        own_slots = vars(cls).get('__slots__')
        donor_namespace = {} if own_slots is None else {'__slots__': own_slots}
        Donor = type('Donor', cls.__bases__, donor_namespace)
        # Prove this really is a layout-compatible type transition. The
        # ordinary control executes every assignment successfully below.
        layout = ('__basicsize__', '__itemsize__', '__dictoffset__', '__weakrefoffset__')
        assert tuple(getattr(cls, key) for key in layout) == tuple(getattr(Donor, key) for key in layout)
        assert bool(cls.__flags__ & 4) == bool(Donor.__flags__ & 4)
        victim = Donor()
        victim.value = 41
        dictionary = vars(victim) if own_slots is None else None
        identity = id(victim)

        operations = (
            ('call', lambda: cls()),
            ('object-new', lambda: object.__new__(cls)),
            ('native-alloc', lambda: alloc(cls, 0)),
            ('native-call', lambda: call(cls)),
            ('subtype', lambda: type('EscapedSubtype', (cls,), {})),
        )
        for label, operation in operations:
            before = list(events)
            if expect_pending:
                try:
                    operation()
                except StrictMutationError:
                    pass
                else:
                    raise AssertionError('pending type admitted ' + label)
                assert events == before, 'rejection followed a constructor callback'
            else:
                operation()

        for setter in (setattr, object.__setattr__, assign):
            if expect_pending:
                try:
                    setter(victim, '__class__', cls)
                except StrictMutationError:
                    pass
                else:
                    raise AssertionError('pending type admitted a compatible __class__ assignment')
            else:
                setter(victim, '__class__', cls)
                assert type(victim) is cls
                setter(victim, '__class__', Donor)
            assert type(victim) is Donor and id(victim) == identity and victim.value == 41
            if dictionary is not None:
                assert vars(victim) is dictionary and dictionary == {'value': 41}

        if expect_pending:
            assert own_contract(cls) == 0, 'the final type contract was published provisionally'
            method = cls.checked
            witness = _soac_ext.strict_function_diagnostics(method)
            if witness is not None:
                assert not witness['finalized']
                assert not witness['original_code_entered']
            else:
                strict_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
                strict_id.argtypes = [ctypes.py_object]
                strict_id.restype = ctypes.c_uint64
                assert strict_id(method) == 0
                assert _soac_ext.strict_function_entry_kind(method) in (
                    'checked_native', 'entry_interpreter',
                )
            before = list(events)
            assert method(None, 'ordinary argument') == 'ordinary argument'
            if witness is not None:
                assert _soac_ext.strict_function_diagnostics(method)['original_code_entered']
            assert method(None, 3) == 3
            if witness is None:
                assert events == before + ['checked body', 'checked body']
            else:
                assert events == before
        observed.append(cls)
    finally:
        observing = False
# module:ordinary_pending_type
from pending_type_support import observe, events

class Base:
    __slots__ = ()

    def __init_subclass__(cls) -> None:
        observe(cls)

class Child(Base):
    __slots__ = ('value',)
    value: int

    def __init__(self) -> None:
        events.append("init")
        self.value = 1

    def checked(self, value: int) -> int:
        return value

# Instance admission must finish at the real class definition, before the
# enclosing module seals, rather than waiting for the end of import.
created = Child()
# ok
# test_pending_type_observation_ordinary_control[slots]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import pending_type_support as support
import ordinary_pending_type as module
assert support.observed == [module.Child]
assert type(module.created) is module.Child and module.created.value == 1
assert module.created.checked("ordinary") == "ordinary"
