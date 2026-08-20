"""Exact Python exception metadata for an empty owned or captured cell."""

import pytest

from tests._strict_integration import create_strict_project

SOURCE = """from __future__ import strict
def free_reader(value):
    def read():
        return value
    return read

def owned_reader(value, clear):
    def read():
        return value
    clear(read)
    return value
"""

VALIDATION = """import types
import cell_errors
from soac import _soac_ext

control = types.ModuleType('stock_cell_errors')
exec(compile(STOCK_SOURCE, '<stock-cell-errors>', 'exec', dont_inherit=True), vars(control))
diagnostic = _soac_ext.strict_module_diagnostics(cell_errors)
assert diagnostic is not None and diagnostic['sealed'] is True

def witness(module, function):
    actual = _soac_ext.strict_function_entry_kind(function)
    expected = EXPECTED_ENTRY if module is cell_errors else None
    assert actual == expected, (function.__qualname__, actual, expected)

def clear_cell(function):
    slot = function.__code__.co_freevars.index('value')
    del function.__closure__[slot].cell_contents

def error_state(call, expected_type):
    try:
        call()
    except BaseException as error:
        assert type(error) is expected_type, (type(error), expected_type)
        return error.args, error.name
    raise AssertionError('an empty cell must raise')

def exercise_free(module):
    witness(module, module.free_reader)
    read = module.free_reader(17)
    witness(module, read)
    assert read() == 17
    clear_cell(read)
    state = error_state(read, NameError)
    assert state[1] == 'value', state
    witness(module, read)
    witness(module, module.free_reader)
    return state

def exercise_owned(module):
    witness(module, module.owned_reader)
    def clear(read):
        witness(module, read)
        assert read() == 17
        clear_cell(read)
    state = error_state(lambda: module.owned_reader(17, clear), UnboundLocalError)
    assert state[1] is None, state
    witness(module, module.owned_reader)
    return state

exercise = exercise_free if CASE == 'free' else exercise_owned
expected = exercise(control)
actual = exercise(cell_errors)
assert actual == expected, (actual, expected)
"""


@pytest.fixture(scope="module")
def cell_errors_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-cell-errors"),
        {"cell_errors.py": SOURCE},
        modules={"cell_errors": "cell_errors.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("case", ["free", "owned"])
def test_empty_cell_preserves_exact_exception_kind_and_name(
    cell_errors_project, entry_interpreter, case
):
    stock = SOURCE.removeprefix("from __future__ import strict\n")
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    cell_errors_project.run(
        f"STOCK_SOURCE = {stock!r}\nEXPECTED_ENTRY = {expected_entry!r}\nCASE = {case!r}\n"
        + VALIDATION,
        entry_interpreter=entry_interpreter,
    )
