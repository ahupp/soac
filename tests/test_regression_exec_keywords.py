import pytest

FRAME_SENSITIVE_BUILTINS_XFAIL = (
    "frame-sensitive locals()/vars()/dir()/eval()/exec() behavior is not supported"
)

def test_exec_accepts_globals_keyword(run_integration_module):
    with run_integration_module("exec_globals_kw") as module:
        with pytest.raises(
            NotImplementedError, match="frame-sensitive globals/locals/eval/exec"
        ):
            module.run()


def test_exec_accepts_locals_keyword(run_integration_module):
    with run_integration_module("exec_locals_kw") as module:
        with pytest.raises(
            NotImplementedError, match="frame-sensitive globals/locals/eval/exec"
        ):
            module.run()


def test_exec_accepts_closure_keyword(run_integration_module):
    pytest.xfail(FRAME_SENSITIVE_BUILTINS_XFAIL)
    with run_integration_module("exec_closure_kw") as module:
        assert module.run() == 2
