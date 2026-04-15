import pytest

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
    pytest.xfail(
        "SOAC-loaded functions wrap lowered functions with synthetic entry parameters; "
        "exec(code, ..., closure=...) is not yet compatible"
    )
    with run_integration_module("exec_closure_kw") as module:
        assert module.run() == 2
