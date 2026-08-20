def run():
    ns = {}
    exec("x = 1", globals=ns)
    return ns["x"]


# diet-python: validate

def validate_module(module):
    if __dp_integration_soac__:
        import pytest

        with pytest.raises(
            NotImplementedError, match="frame-sensitive globals/locals/eval/exec"
        ):
            module.run()
    else:
        assert module.run() == 1
