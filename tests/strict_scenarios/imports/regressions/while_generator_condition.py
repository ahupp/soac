# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[while_generator_condition]
# module:while_generator_condition
# soac: module(strict_assign=true, checked_attr=true)
class Worker:
    def is_alive(self):
        return True

def value():
    count = 0
    workers = [Worker()]
    while count < 1 and all(worker.is_alive() for worker in workers):
        count += 1
    return count
# ok
# while_generator_condition
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.value() == 1
