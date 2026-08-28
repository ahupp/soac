from __future__ import annotations

import pytest

from tests._strict_integration import create_strict_project


# Original ordinary source, retained unchanged. Strict enrollment is explicit
# in the fixture; import-hook/mode settings alone are never admission evidence.
_MODULE_SOURCE = """
def captured(offset):
    return [offset + value for value in range(3)]


def noncanonical(offset):
    return [offset + value for value in range(3)]


def original_outer(offset):
    def original_inner(value):
        return offset + value

    return original_inner
"""


@pytest.fixture(scope="module", params=("soac", "cpython"))
def metadata_project(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp(f"source-metadata-{request.param}"),
        {
            "metadata_source.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _MODULE_SOURCE,
            "metadata_ordinary.py": _MODULE_SOURCE,
        },
        modules={"metadata_source": "metadata_source.py"},
        backend=request.param,
    )


def test_actual_source_function_metadata_cells_and_callbacks(metadata_project):
    path = metadata_project.root / "metadata-validation.py"
    path.write_text(_VALIDATION)
    for entry in ((False, True) if metadata_project.backend == "soac" else (False,)):
        metadata_project.run_case(
            "metadata_source", _VALIDATION, path, entry_interpreter=entry,
            required_functions=("captured", "noncanonical", "original_outer"),
        )


_VALIDATION = r'''
import ctypes
import sys
import types
import metadata_ordinary as stock
from soac import _soac_ext

def validate_module(module):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    seal = ctypes.pythonapi.PyFunction_GetSoacStrictId
    seal.argtypes = [ctypes.py_object]
    seal.restype = ctypes.c_uint64
    source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
    source_id.argtypes = [ctypes.py_object]
    source_id.restype = ctypes.c_uint64
    assert owner(stock.original_outer) is None and seal(stock.original_outer) == 0
    created = {"ordinary": [], "strict": []}
    qualname_changes, callback_errors, code_events, explicit_audits = [], [], [], []
    reject_audit = False
    audit_error = RuntimeError("explicit code audit rejected")

    def audit(event, arguments):
        if event == "code.__new__" and len(arguments) >= 3:
            # Observe all real events without prescribing whether an internal
            # comprehension code object must be allocated or eliminated.
            code_events.append(arguments[2])
            if arguments[2] == "soac_explicit_metadata_audit":
                explicit_audits.append("code.__new__")
                if reject_audit:
                    raise audit_error
    sys.addaudithook(audit)

    callback_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
    @callback_type
    def watch(event, pointer, new_value):
        if event not in (0, 5):
            return 0
        try:
            function = ctypes.cast(pointer, ctypes.py_object).value
            code = function.__code__
            if code.co_name != "original_inner":
                return 0
            if function.__globals__ is module.__dict__:
                group = "strict"
            elif function.__globals__ is stock.__dict__:
                group = "ordinary"
            else:
                return 0
            if event == 0:
                created[group].append(function)
            else:
                qualname_changes.append((group, function, ctypes.cast(new_value, ctypes.py_object).value))
        except BaseException as error:
            callback_errors.append(type(error).__name__)
        return 0

    add = ctypes.pythonapi.PyFunction_AddWatcher
    add.argtypes = [callback_type]
    add.restype = ctypes.c_int
    clear = ctypes.pythonapi.PyFunction_ClearWatcher
    clear.argtypes = [ctypes.c_int]
    clear.restype = ctypes.c_int
    watcher = add(watch)
    assert watcher >= 0
    try:
        for offset in (0, 1, 10, 100):
            assert module.captured(offset) == stock.captured(offset) == [offset + value for value in range(3)]
        for offset in (5, 50):
            assert module.noncanonical(offset) == stock.noncanonical(offset) == [offset + value for value in range(3)]

        ordinary = [stock.original_outer(offset) for offset in (1, 10, 100)]
        actual = [module.original_outer(offset) for offset in (1, 10, 100)]
        assert created == {"ordinary": ordinary, "strict": actual}
        for functions in (ordinary, actual):
            assert all(type(function) is types.FunctionType for function in functions)
            assert len({id(function) for function in functions}) == 3
            assert functions[0].__code__ is functions[1].__code__ is functions[2].__code__
            cells = [function.__closure__[function.__code__.co_freevars.index("offset")] for function in functions]
            assert len({id(cell) for cell in cells}) == 3
            assert [cell.cell_contents for cell in cells] == [1, 10, 100]
            assert [function(1) for function in functions] == [2, 11, 101]
            for function in functions:
                assert function.__name__ == "original_inner"
                assert function.__qualname__ == "original_outer.<locals>.original_inner"
                assert function.__name__ is function.__code__.co_name
                assert function.__qualname__ is function.__code__.co_qualname
        # Real source-created functions are observable. Unlike an eliminated
        # internal helper, their creation and initial metadata callbacks remain
        # required behavior, not an optimization-count assertion.
        assert qualname_changes == []
        before = [(owner(function), seal(function), function.__code__) for function in actual]
        assert all(actual_owner and actual_seal and source_id(code) for actual_owner, actual_seal, code in before)
        assert all(owner(function) is None and seal(function) == 0 and source_id(function.__code__) == 0 for function in ordinary)
        for function in actual:
            if __dp_integration_mode__ == "cpython":
                diagnostic = _soac_ext.strict_function_diagnostics(function)
                assert diagnostic["backend"] == "cpython" and diagnostic["entry_kind"] == "original_code"
            else:
                expected = "entry_interpreter" if __dp_integration_entry__ else "checked_native"
                assert _soac_ext.strict_function_entry_kind(function) == expected

        # Display names are not protected code/default/dispatch metadata. Keep
        # their actual supported setters, including their explicit watcher event.
        for functions in (ordinary, actual):
            functions[0].__name__ = "user_name"
            functions[0].__qualname__ = "user.qualname"
            assert functions[0].__name__ == "user_name"
            assert functions[0].__qualname__ == "user.qualname"
            assert functions[1].__name__ == "original_inner"
            assert functions[1].__qualname__ == "original_outer.<locals>.original_inner"
            assert functions[0].__code__.co_name == "original_inner"
            assert functions[0].__code__.co_qualname == "original_outer.<locals>.original_inner"
            assert [function(1) for function in functions] == [2, 11, 101]
        assert qualname_changes == [("ordinary", ordinary[0], "user.qualname"), ("strict", actual[0], "user.qualname")]
        assert before == [(owner(function), seal(function), function.__code__) for function in actual]
        try:
            actual[0].__code__ = actual[0].__code__
        except TypeError:
            pass
        else:
            raise AssertionError("display-name mutation reopened sealed source code")
        assert before == [(owner(function), seal(function), function.__code__) for function in actual]

        # These ordinary, explicit code operations must still deliver audit
        # callbacks, including callback exceptions. They grant no source owner.
        copied = ordinary[0].__code__.replace(co_name="soac_explicit_metadata_audit")
        assert copied.co_name == "soac_explicit_metadata_audit" and source_id(copied) == 0
        reject_audit = True
        try:
            ordinary[0].__code__.replace(co_name="soac_explicit_metadata_audit")
        except RuntimeError as error:
            assert error is audit_error
        else:
            raise AssertionError("code.__new__ audit exception was suppressed")
        finally:
            reject_audit = False
        assert explicit_audits == ["code.__new__", "code.__new__"]
        assert code_events.count("soac_explicit_metadata_audit") == 2
        assert callback_errors == []
        assert [function(1) for function in actual] == [2, 11, 101]
    finally:
        assert clear(watcher) == 0
'''
