def run():
    junk = 1
    return dir()


# diet-python: validate

def validate_module(module):
    import pytest

    if __dp_integration_soac__:
        with pytest.raises(NotImplementedError):
            module.run()
    else:
        assert module.run() == ["junk"]
