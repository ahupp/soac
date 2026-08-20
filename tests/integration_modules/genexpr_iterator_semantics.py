import types


def make():
    return (x for x in range(2))


def replay(value):
    g = make()
    gen_func = types.FunctionType(g.gi_code, {})
    return list(gen_func(value))


def main():
    return replay([1, 2])


# diet-python: validate

def validate_module(module):
    import pytest

    if __dp_integration_soac__:
        from soac.strict import StrictRuntimeUnavailableError

        with pytest.raises(
            StrictRuntimeUnavailableError,
            match="strict code execution requires an authenticated runtime entry",
        ):
            module.main()
        with pytest.raises(
            StrictRuntimeUnavailableError,
            match="strict code execution requires an authenticated runtime entry",
        ):
            module.replay(42)
    else:
        assert module.main() == [1, 2]
        with pytest.raises(TypeError, match=r"object is not iterable"):
            module.replay(42)
