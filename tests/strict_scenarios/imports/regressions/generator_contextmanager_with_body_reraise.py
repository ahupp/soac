# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[generator_contextmanager_with_body_reraise]
# module:generator_contextmanager_with_body_reraise
# soac: module(strict_assign=true, checked_attr=true)
from contextlib import contextmanager

class Manager:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

@contextmanager
def manager():
    with Manager():
        yield

class MarkerError(Exception):
    pass

def check_exit():
    cm = manager()
    cm.__enter__()
    try:
        raise MarkerError("boom")
    except MarkerError as exc:
        return cm.__exit__(type(exc), exc, exc.__traceback__)
# ok
# generator_contextmanager_with_body_reraise
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.check_exit() is False
