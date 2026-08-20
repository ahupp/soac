import asyncio
import inspect
import types
import pytest

from soac import runtime


def _make_cell(value):
    cell = types.CellType()
    cell.cell_contents = value
    return cell


def _build_wrapped(names, is_async, is_generator):
    code = runtime.code_with_freevars(names, is_async, is_generator)
    closure = tuple(_make_cell(f"cell-{name}") for name in names)
    return types.FunctionType(code, globals(), name="wrapped", closure=closure)


def test_code_with_freevars_returns_requested_freevars():
    code = runtime.code_with_freevars(("x", "y"), False, False)
    assert code.co_freevars == ("x", "y")


def test_code_with_freevars_preserves_requested_freevar_order():
    code = runtime.code_with_freevars(("a", "_dp_eval_1", "_dp_pc"), False, False)

    assert code.co_freevars == ("a", "_dp_eval_1", "_dp_pc")

    captured_by_name = {
        "a": "captured-a",
        "_dp_eval_1": "captured-eval",
        "_dp_pc": "captured-pc",
    }
    closure = tuple(_make_cell(captured_by_name[name]) for name in code.co_freevars)
    wrapped = types.FunctionType(code, globals(), name="wrapped", closure=closure)

    assert {
        name: cell.cell_contents
        for name, cell in zip(wrapped.__code__.co_freevars, wrapped.__closure__)
    } == captured_by_name


def test_code_with_freevars_preserves_native_type_parameter_capture():
    names = ("outer", ".type_params")
    code = runtime.code_with_freevars(names, False, False)

    assert code.co_freevars == names
    assert runtime.code_with_freevars(names, False, False) is code
    closure = (_make_cell("outer-value"), _make_cell((int,)))
    wrapped = types.FunctionType(code, globals(), closure=closure)
    assert wrapped.__closure__ == closure
    with pytest.raises(RuntimeError, match="CLIF entry executed without vectorcall interception"):
        wrapped()


@pytest.mark.parametrize("name", ("arbitrary.name", "class"))
def test_code_with_freevars_rejects_unrecognized_source_names(name):
    with pytest.raises(ValueError, match="invalid freevar name"):
        runtime.code_with_freevars((name,), False, False)


def test_code_with_freevars_builds_sync_wrapper():
    wrapped = _build_wrapped(("x", "y"), False, False)

    with pytest.raises(RuntimeError, match="CLIF entry executed without vectorcall interception"):
        wrapped(1, value=2)


def test_code_with_freevars_builds_coroutine_wrapper():
    wrapped = _build_wrapped(("x",), True, False)

    assert inspect.iscoroutinefunction(wrapped)
    with pytest.raises(RuntimeError, match="CLIF entry executed without vectorcall interception"):
        asyncio.run(wrapped(1, value=2))


def test_code_with_freevars_builds_generator_wrapper():
    wrapped = _build_wrapped(("x",), False, True)

    assert inspect.isgeneratorfunction(wrapped)
    with pytest.raises(RuntimeError, match="CLIF entry executed without vectorcall interception"):
        list(wrapped(1, value=2))


def test_code_with_freevars_builds_async_generator_wrapper():
    wrapped = _build_wrapped(("x",), True, True)

    async def collect():
        return [item async for item in wrapped(1, value=2)]

    assert inspect.isasyncgenfunction(wrapped)
    with pytest.raises(RuntimeError, match="CLIF entry executed without vectorcall interception"):
        asyncio.run(collect())
