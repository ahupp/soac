# modes:soac,entry
# test_strict_import_admission.py::test_reviewed_import_regressions_use_authenticated_entries[except_body_implicit_context]
# module:except_body_implicit_context
# soac: module(strict_assign=true, checked_attr=true)
import ast
import unittest

def value():
    case = unittest.TestCase()
    try:
        1 / 0
    except Exception:
        with case.assertRaises(SyntaxError) as caught:
            ast.literal_eval(r"'\U'")
        return type(caught.exception.__context__).__name__
    return "missing"
# ok
# except_body_implicit_context
import sys
import pytest
from soac import _soac_ext, import_hook
assert module.value() == "ZeroDivisionError"
