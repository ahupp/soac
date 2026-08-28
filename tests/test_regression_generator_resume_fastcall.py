import inspect
from pathlib import Path

import pytest
from soac import _soac_ext
from soac.runtime import NO_DEFAULT

from tests._strict_integration import create_strict_project


def _resume_arguments(generator, handle, send_value=None):
    import ctypes
    import gc
    import types

    # Deliberately discover the real retained owner through supported GC/C APIs.
    # Association is not permission to resume it outside its native operation.
    assert type(generator) is types.GeneratorType
    valid = ctypes.pythonapi.PyCapsule_IsValid
    valid.argtypes = (ctypes.py_object, ctypes.c_char_p)
    valid.restype = ctypes.c_int
    (preserved_state,) = (
        value for value in gc.get_referents(generator)
        if valid(value, b"soac.PreservedState")
    )
    matches = ctypes.pythonapi.PyGen_MatchesSoacOwner
    matches.argtypes = (ctypes.py_object, ctypes.py_object)
    matches.restype = ctypes.c_int
    assert matches(generator, preserved_state) == 1
    assert any(value is handle for value in gc.get_referents(preserved_state))
    return {
        "handle": handle,
        "owner": generator,
        "preserved_state": preserved_state,
        "send_value": send_value,
        "resume_exc": NO_DEFAULT,
    }


def _call_resume(function, arguments, send_value, calling_convention):
    arguments = dict(arguments, send_value=send_value)
    if calling_convention == "positional":
        return function(*arguments.values())
    if calling_convention == "keywords":
        return function(**arguments)
    handle = arguments.pop("handle")
    return function(handle, **arguments)


def test_generator_resume_fastcall_preserves_callable_metadata():
    fastcall = _soac_ext.resume_generator
    fallback = _soac_ext._resume_generator_pyo3_fallback

    assert fastcall.__name__ == fallback.__name__ == "resume_generator"
    assert fastcall.__qualname__ == fallback.__qualname__
    assert fastcall.__module__ == fallback.__module__ == _soac_ext.__name__
    assert fastcall.__self__ is _soac_ext
    assert fallback.__self__ is None
    assert fastcall.__doc__ == fallback.__doc__
    assert fastcall.__text_signature__ == (
        "($module, handle, owner, preserved_state, send_value, resume_exc)"
    )
    assert fallback.__text_signature__ == (
        "(handle, owner, preserved_state, send_value, resume_exc)"
    )
    assert inspect.signature(fastcall) == inspect.signature(fallback)


@pytest.fixture(scope="module")
def strict_resume_project(tmp_path_factory):
    source = """
# soac: module(strict_assign=true, checked_attr=true)

def values():
    sent = yield 3
    yield sent

def resume():
    return values()
"""
    return create_strict_project(
        tmp_path_factory.mktemp("strict-resume-fastcall"),
        {
            "resume_model.py": source,
            "ordinary_resume_model.py": source.replace(
                "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1
            ),
        },
        modules={"resume_model": "resume_model.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("calling_convention", ["positional", "keywords", "mixed"])
def test_generator_resume_helpers_cannot_bypass_native_operation_ownership(
    strict_resume_project, calling_convention, entry_interpreter
):
    strict_resume_project.run_case(
        "resume_model",
        f"""
import ctypes
import pytest
from soac import _soac_ext
import types
from tests.test_regression_generator_resume_fastcall import _call_resume, _resume_arguments
import ordinary_resume_model

def validate_module(module):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert not owner(ordinary_resume_model.resume)
    assert _soac_ext.strict_module_diagnostics(ordinary_resume_model) is None
    assert owner(module.values)
    assert _soac_ext.strict_function_entry_kind(module.values) == 'generator_factory'
    calling_convention = {calling_convention!r}
    # Authenticated source generators are exact native objects. Their public
    # operations own entry; discovering the old resume operands cannot grant
    # a private body execution outside that operation, for either ABI wrapper.
    fast_generator = module.resume()
    fallback_generator = module.resume()
    ordinary_generator = ordinary_resume_model.resume()
    assert type(fast_generator) is type(fallback_generator) is types.GeneratorType
    fast_arguments = _resume_arguments(fast_generator, module.values)
    fallback_arguments = _resume_arguments(fallback_generator, module.values)

    def reject_private_resume(send_value):
        for function, arguments in (
            (_soac_ext.resume_generator, fast_arguments),
            (_soac_ext._resume_generator_pyo3_fallback, fallback_arguments),
        ):
            with pytest.raises(RuntimeError) as rejected:
                _call_resume(function, arguments, send_value, calling_convention)
            assert type(rejected.value) is RuntimeError
            assert str(rejected.value) == 'managed generator resume requires its active native step'

    for send_value in (None, "sent"):
        reject_private_resume(send_value)
        assert fast_generator.send(send_value) == fallback_generator.send(send_value) == ordinary_generator.send(send_value)
    reject_private_resume(None)
    for generator in (fast_generator, fallback_generator, ordinary_generator):
        with pytest.raises(StopIteration):
            generator.send(None)
    # Completion removes both the live association and the strict snapshot.
    # Retaining its former operands cannot revive either execution authority.
    from soac.strict import StrictRuntimeUnavailableError

    for function, arguments in (
        (_soac_ext.resume_generator, fast_arguments),
        (_soac_ext._resume_generator_pyo3_fallback, fallback_arguments),
    ):
        with pytest.raises(StrictRuntimeUnavailableError) as rejected:
            _call_resume(function, arguments, None, calling_convention)
        assert type(rejected.value) is StrictRuntimeUnavailableError
        assert str(rejected.value) == 'strict suspended state is absent or terminal'
""",
        Path(__file__),
        required_functions=("resume",),
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("args", "kwargs"),
    [
        pytest.param((), {}, id="missing-all-arguments"),
        pytest.param((None,), {}, id="missing-positional-arguments"),
        pytest.param((None, None, None, None), {}, id="missing-final-argument"),
        pytest.param((None, None, None, None, None, None), {}, id="extra-positional"),
        pytest.param((), {"unexpected": None}, id="unexpected-keyword"),
        pytest.param((None,), {"handle": None}, id="duplicate-keyword"),
        pytest.param(
            (None, None, None, None, None),
            {"unexpected": None},
            id="unexpected-keyword-with-positionals",
        ),
    ],
)
def test_generator_resume_fastcall_preserves_pyo3_argument_errors(args, kwargs):
    with pytest.raises(TypeError) as expected:
        _soac_ext._resume_generator_pyo3_fallback(*args, **kwargs)

    with pytest.raises(TypeError) as actual:
        _soac_ext.resume_generator(*args, **kwargs)

    assert str(actual.value) == str(expected.value)
