"""Frame-sensitive class builtins use authenticated execution or ordinary controls."""

from pathlib import Path
import textwrap

import pytest

from tests._integration import exec_integration_validation, stock_module
from tests._strict_integration import assert_strict_source_rejected, create_strict_project


@pytest.mark.parametrize("mode", ["stock", "cpython", "soac", "entry"])
def test_class_locals_aliases_preserve_mapping_identity_and_binding(tmp_path, mode):
    source = """
import builtins

query = builtins.locals
query_vars = builtins.vars
query_globals = builtins.globals

class Box:
    before = query()
    value = 7
    positional = query(*())
    keywords = query_vars(**{})
    global_namespace = query_globals()
    query()['inserted'] = 11
    observed = inserted

class Shadowed:
    locals = lambda: {'custom': 19}
    result = locals()
    alias = query
    actual_namespace = alias()

class Invalid:
    try:
        query(1)
    except TypeError as error:
        positional_error = str(error)
    try:
        query_globals(unexpected=1)
    except TypeError as error:
        keyword_error = str(error)
"""
    if mode != "stock":
        # This original ordinary source relies on an undeclared name injected
        # through locals(), and deliberately makes invalid builtin calls.
        # Ordinary checker errors remain blocking strict-language diagnostics;
        # this analysis-only rejection does not claim a runtime admission.
        assert_strict_source_rejected(
            tmp_path, "from __future__ import strict\n" + textwrap.dedent(source),
            module_name="class_frame_builtins", diagnostic="unresolved-reference",
        )
        return
    with stock_module(tmp_path, "class_frame_builtins", source) as module:
        namespace = module.Box.before
        assert namespace is module.Box.positional is module.Box.keywords
        assert namespace["before"] is namespace
        assert namespace["value"] == 7
        assert module.Box.observed == namespace["inserted"] == 11
        assert module.Box.global_namespace is vars(module)
        assert not any(name.startswith("_dp_") for name in namespace)
        assert module.Shadowed.result == {"custom": 19}
        assert module.Shadowed.actual_namespace["result"] is module.Shadowed.result
        assert "locals" in module.Invalid.positional_error
        assert "globals" in module.Invalid.keyword_error



@pytest.mark.parametrize("mode", ["stock", "cpython"])
def test_class_context_is_not_inherited_by_python_callbacks(tmp_path, mode):
    source = """
import builtins
query = builtins.locals

def callback(query):
    # Only transformed optimized-function locals remain explicitly unsupported;
    # the callback must never see its caller's class namespace instead.
    marker = 13
    return query()

class Box:
    marker = 29
    try:
        result = callback(query)
    except NotImplementedError:
        result = None
"""
    # Keep the original frame-sensitive source as a positive ordinary/CPython
    # control; SOAC function-local inspection and refusal are out of scope.
    validation = """
def validate_module(module):
    result = module.Box.result
    assert result["marker"] == 13
"""
    if mode == "stock":
        with stock_module(tmp_path, "class_context_callback", source) as module:
            exec_integration_validation(validation, module, Path(module.__file__), mode=mode)
        return
    project = create_strict_project(
        tmp_path,
        {"class_context_callback.py": "from __future__ import strict\n" + textwrap.dedent(source)},
        modules={"class_context_callback": "class_context_callback.py"}, backend="cpython",
    )
    project.run_case(
        "class_context_callback", validation, Path(__file__),
        backend="cpython", required_functions=("callback",),
    )


@pytest.mark.parametrize("mode", ["stock", "cpython", "soac", "entry"])
def test_class_callbacks_keep_lexical_globals_and_explicit_order(tmp_path, mode):
    source = """
events = []
marker = object()

def record(label, value):
    events.append((label, value))
    return value

def callback(argument):
    local_marker = 13
    record("callback", local_marker)
    return local_marker, marker, argument

class Box:
    marker = object()
    local_marker = 29
    record("before", marker)
    result = callback(record("argument", marker))
    record("after", result)
"""
    validation = """
def validate_module(module):
    result = module.Box.result
    assert type(result) is tuple and len(result) == 3
    assert result[0] == 13 and module.Box.local_marker == 29
    assert result[1] is module.marker
    assert result[2] is module.Box.marker
    assert module.marker is not module.Box.marker
    assert module.events == [
        ("before", module.Box.marker),
        ("argument", module.Box.marker),
        ("callback", 13),
        ("after", result),
    ]
    assert module.events[-1][1] is result
"""
    name = "class_callback_lexical_context"
    if mode == "stock":
        with stock_module(tmp_path, name, source) as module:
            exec_integration_validation(validation, module, Path(module.__file__), mode=mode)
        return
    backend = "cpython" if mode == "cpython" else "soac"
    path = f"{name}.py"
    project = create_strict_project(
        tmp_path,
        {path: "from __future__ import strict\n" + textwrap.dedent(source)},
        modules={name: path}, backend=backend,
    )
    project.run_case(
        name, validation, Path(__file__),
        entry_interpreter=mode == "entry", backend=backend,
        required_functions=("callback", "record"),
    )
