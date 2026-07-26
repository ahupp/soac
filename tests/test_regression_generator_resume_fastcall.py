import inspect

import pytest

from soac import _soac_ext
from soac.runtime import ClosureGenerator, NO_DEFAULT, make_generator_instance
from tests._integration import soac_module


def _make_closure_generator(resume_function):
    generator = make_generator_instance(
        resume_function,
        0,
        "resume",
        "resume",
        (),
        (),
        0,
        0,
        0,
    )
    assert isinstance(generator, ClosureGenerator)
    return generator


def _resume_arguments(generator, send_value=None):
    return {
        "handle": generator._resume_function,
        "owner": generator,
        "preserved_state": generator._preserved_values,
        "send_value": send_value,
        "resume_exc": NO_DEFAULT,
    }


def _call_resume(function, generator, send_value, calling_convention):
    arguments = _resume_arguments(generator, send_value)
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


@pytest.mark.parametrize("calling_convention", ["positional", "keywords", "mixed"])
def test_generator_resume_fastcall_preserves_resume_behavior(
    tmp_path, calling_convention
):
    source = """
def resume(owner, preserved_state, send_value, resume_exc):
    if send_value is None:
        return 3
    return send_value
"""

    with soac_module(tmp_path, "generator_resume_fastcall", source) as module:
        fast_generator = _make_closure_generator(module.resume)
        fallback_generator = _make_closure_generator(module.resume)

        assert _call_resume(
            _soac_ext.resume_generator,
            fast_generator,
            None,
            calling_convention,
        ) == _call_resume(
            _soac_ext._resume_generator_pyo3_fallback,
            fallback_generator,
            None,
            calling_convention,
        )
        assert _call_resume(
            _soac_ext.resume_generator,
            fast_generator,
            "sent",
            calling_convention,
        ) == _call_resume(
            _soac_ext._resume_generator_pyo3_fallback,
            fallback_generator,
            "sent",
            calling_convention,
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
