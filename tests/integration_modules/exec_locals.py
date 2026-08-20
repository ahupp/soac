def run():
    x = 10
    code = compile("x + 1", "", "exec")
    exec(code)
    return True


# diet-python: validate

def validate_module(module):
    import pytest

    if __dp_integration_soac__:
        with pytest.raises(NotImplementedError):
            module.run()
    else:
        assert module.run() is True
