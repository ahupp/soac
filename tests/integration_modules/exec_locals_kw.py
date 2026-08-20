def run():
    ns = {}
    exec("global x\nx = 1", locals=ns)
    return ns


# diet-python: validate

def validate_module(module):
    if __dp_integration_soac__:
        import pytest

        with pytest.raises(
            NotImplementedError, match="frame-sensitive globals/locals/eval/exec"
        ):
            module.run()
    else:
        assert module.run() == {}
