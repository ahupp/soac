import pytest

FRAME_SENSITIVE_BUILTINS_XFAIL = (
    "frame-sensitive locals()/vars()/dir()/eval()/exec() behavior is not supported"
)

def test_dir_filters_dp_internal_names(run_integration_module):
    pytest.xfail(FRAME_SENSITIVE_BUILTINS_XFAIL)
    with run_integration_module("dir_filters") as module:
        with pytest.raises(NotImplementedError):
            module.run()
