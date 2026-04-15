import pytest

FRAME_SENSITIVE_BUILTINS_XFAIL = (
    "frame-sensitive locals()/vars()/dir()/eval()/exec() behavior is not supported"
)

def test_eval_sees_closure_cells(run_integration_module):
    pytest.xfail(FRAME_SENSITIVE_BUILTINS_XFAIL)
    with run_integration_module("eval_closure") as module:
        with pytest.raises(NotImplementedError):
            module.run()
