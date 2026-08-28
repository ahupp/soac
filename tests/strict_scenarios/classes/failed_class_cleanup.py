# modes:soac,entry
# module:failed_class_namespace
# soac: module(strict_assign=true, checked_attr=true)
import namespace_failure_support as support

def fail_class():
    class Broken:
        value = support.Payload()
        raise ValueError('namespace failure')
# module:namespace_failure_support
import weakref

events = []
references = []

class Payload:
    def __init__(self):
        events.append('created')
        references.append(weakref.ref(self))

    def __del__(self):
        events.append('released')
# ok
# test_failed_class_namespace_preserves_errors_and_releases_values
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('fail_class',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
def check_failed_namespace(action, failed_import=None, *, native_frames=True):
    import gc
    import sys
    import namespace_failure_support as support

    try:
        action()
    except ValueError as error:
        assert type(error) is ValueError and error.args == ('namespace failure',)
        retained_traceback = error.__traceback__
    else:
        raise AssertionError('source body did not raise its original ValueError')

    # The ordinary CPython control retains its source namespace through the
    # traceback. SOAC does not reconstruct or retain a source frame for this.
    if native_frames:
        assert retained_traceback is not None
    if failed_import is not None:
        assert failed_import not in sys.modules, 'failed import remained published'
    assert len(support.references) == 1, support.events
    reference = support.references[0]
    gc.collect()
    if native_frames:
        assert reference() is not None, ('ordinary traceback lost namespace owner', support.events)
        assert support.events == ['created'], support.events

    retained_traceback = None
    gc.collect()
    assert reference() is None, ('namespace survived traceback release', support.events)
    assert support.events == ['created', 'released'], support.events

check_failed_namespace(module.fail_class, native_frames=False)
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('fail_class',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
