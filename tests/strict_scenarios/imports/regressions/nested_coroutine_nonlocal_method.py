# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[nested_coroutine_nonlocal_method]
# module:nested_coroutine_nonlocal_method
# soac: module(strict_assign=true, checked_attr=true)
def build():
    cancelled = False

    class Test:
        async def test_leaking_task(self):
            async def coro():
                nonlocal cancelled
                cancelled = True

            await coro()

        def was_cancelled(self):
            return cancelled

    return Test()
# ok
# nested_coroutine_nonlocal_method
import sys
import pytest
from soac import _soac_ext, import_hook
instance = module.build()
coroutine = instance.test_leaking_task()
try:
    coroutine.send(None)
except StopIteration as exc:
    assert exc.value is None
else:
    raise AssertionError("coroutine should finish without suspension")
assert instance.was_cancelled() is True
