import pytest

FRAME_SENSITIVE_BUILTINS_XFAIL = (
    "frame-sensitive locals()/vars()/dir()/eval()/exec() behavior is not supported"
)

def test_exec_sees_locals(run_integration_module):
    pytest.xfail(FRAME_SENSITIVE_BUILTINS_XFAIL)
    with run_integration_module("exec_locals") as module:
        with pytest.raises(NotImplementedError):
            module.run()
