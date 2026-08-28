# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[complex_literal]
# module:complex_literal
# soac: module(strict_assign=true, checked_attr=true)
VALUE = 1j
# ok
# complex_literal
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.VALUE == 1j
