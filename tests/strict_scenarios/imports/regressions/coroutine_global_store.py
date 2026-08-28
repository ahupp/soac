# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[coroutine_global_store]
# module:coroutine_global_store
# soac: module(strict_assign=true, checked_attr=true)
flag = False

async def set_flag():
    global flag
    flag = True

def value():
    coroutine = set_flag()
    try:
        coroutine.send(None)
    except StopIteration as exc:
        assert exc.value is None
    else:
        raise AssertionError("coroutine should finish without suspension")
    return flag
# ok
# coroutine_global_store
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.value() is True
assert module.flag is True
