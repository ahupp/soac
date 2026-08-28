# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:cyclic_traceback
# soac: module(strict_assign=true, checked_attr=true)
def make(save, payload_factory, connect):
    async def source():
        payload = payload_factory()
        try:
            raise ValueError('retained before suspension')
        except ValueError as error:
            save(error)
        yield 'ready'
    value = source()
    connect(source, value)
    return value
# ok
# tests/test_strict_generator_protocols.py::test_cyclic_async_traceback_does_not_root_suspended_source_state
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

def validate(module):
    import gc
    import sys
    import weakref

    saved = []
    events = []
    function_refs = []
    source_codes = []

    class Payload:
        def __init__(self):
            events.append('payload-created')

        def __del__(self):
            events.append('payload-finalized')

    def connect(function, value):
        function.cycle = value
        function_refs.append(weakref.ref(function))
        source_codes.append(function.__code__)

    def finalize(value):
        # A supported hook may decline to close or resurrect this generator.
        events.append('async-finalizer-hook')

    original_hooks = sys.get_asyncgen_hooks()
    old_enabled = gc.isenabled()
    gc.disable()
    value = None
    operation = None
    try:
        sys.set_asyncgen_hooks(firstiter=None, finalizer=finalize)
        value = module.make(saved.append, Payload, connect)
        value_ref = weakref.ref(value)
        operation = value.__anext__()
        try:
            operation.send(None)
        except StopIteration as completed:
            assert completed.value == 'ready'
        else:
            raise AssertionError('initial async yield did not complete the ASend')
        operation = None
        assert len(saved) == 1
        assert type(saved[0]) is ValueError and saved[0].args == ('retained before suspension',)
        if not __dp_integration_soac__:
            assert events == ['payload-created']
            # Preserve the ordinary CPython frame control only. SOAC errors
            # need no reconstructed source frame or matching local retention.
            traceback = saved[0].__traceback__
            try:
                while traceback is not None and traceback.tb_frame.f_code is not source_codes[0]:
                    traceback = traceback.tb_next
                assert traceback is not None, 'ordinary error omitted its original source frame'
            finally:
                del traceback

        value = None
        if __dp_integration_soac__:
            # SOAC cleanup is checked after releasing the retained traceback,
            # without requiring a particular source-frame retention policy.
            saved[0].__traceback__ = None
            saved.clear()
        gc.collect()
        assert events[0] == 'payload-created', events
        assert sorted(events[1:]) == ['async-finalizer-hook', 'payload-finalized'], (
            'cyclic async cleanup must run each required finalizer once', events
        )
        assert value_ref() is None, 'quiescent cleanup retained the generator'
        assert function_refs[0]() is None, 'quiescent cleanup retained the function'

        if not __dp_integration_soac__:
            expected = events.copy()
            saved[0].__traceback__ = None
            saved.clear()
            gc.collect()
            assert events == expected, 'clearing the traceback repeated a GC finalizer'
    finally:
        operation = None
        value = None
        for error in saved:
            error.__traceback__ = None
        saved.clear()
        gc.collect()
        sys.set_asyncgen_hooks(*original_hooks)
        if old_enabled:
            gc.enable()

validate(module)

_assert_source_function_witnesses()
