def test_functiontype_injects_dp_globals(run_integration_module):
    with run_integration_module("functiontype_globals") as module:
        assert module.run() == 2
