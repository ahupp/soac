# modes:soac
# test_strict_import_admission.py::test_strict_module_docstring_is_visible_before_body_callbacks
# module:doc_plain
"""plain docs"""
# soac: module(strict_assign=true, checked_attr=true)
from doc_support import observe
observe(__name__)
VALUE = 1
# module:doc_deferred
"""deferred docs"""
# soac: module(strict_assign=true, checked_attr=true)
from doc_support import observe
observe(__name__)
VALUE: int = 1
# module:doc_stringized
"""stringized docs"""
# soac: module(strict_assign=true, checked_attr=true)
from __future__ import annotations
from doc_support import observe
observe(__name__)
VALUE: int = 1
# module:doc_support
import sys
events = []
def observe(name):
    events.append((name, sys.modules[name].__doc__))
# ok
# test_strict_module_docstring_is_visible_before_body_callbacks
import sys
import pytest
from soac import _soac_ext, import_hook
import doc_plain, doc_deferred, doc_stringized, doc_support
assert doc_support.events == [
    ("doc_plain", "plain docs"),
    ("doc_deferred", "deferred docs"),
    ("doc_stringized", "stringized docs"),
]
for module, document in (
    (doc_plain, "plain docs"),
    (doc_deferred, "deferred docs"),
    (doc_stringized, "stringized docs"),
):
    assert _soac_ext.strict_module_diagnostics(module)["sealed"] is True
    assert module.__doc__ == document and module.VALUE == 1
