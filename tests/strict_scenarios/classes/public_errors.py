# modes:soac
# module:model
# soac: module(strict_assign=true, checked_attr=true)
value = 1
# ok
# test_public_strict_errors_are_native_shared_classes
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
import pickle
import soac
import soac.strict
import _soac_ext
import model

for name, base in [('StrictMutationError', TypeError), ('StrictRuntimeUnavailableError', ImportError)]:
    exception = getattr(soac.strict, name)
    assert exception is getattr(soac, name) is getattr(_soac_ext, name)
    assert issubclass(exception, base)
    assert type(pickle.loads(pickle.dumps(exception('message')))) is exception
    try:
        exception.changed = True
    except TypeError:
        pass
    else:
        raise AssertionError('native strict exception class is mutable')
try:
    model.value = 2
except soac.strict.StrictMutationError:
    pass
else:
    raise AssertionError('module mutation did not use the shared exception')
