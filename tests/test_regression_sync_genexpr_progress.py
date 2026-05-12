def test_sync_genexpr_advances_between_resumes(run_integration_module):
    with run_integration_module("sync_genexpr_progress") as module:
        assert module.progress() == [0, 1, 2]
        assert module.stops() == [0, 1, 2]
