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

    assert module.main() == [1, 2]
    with pytest.raises(TypeError, match=r"object is not iterable"):
        module.replay(42)
