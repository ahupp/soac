def test_sync_nested_genexpr_advances_and_stops(run_integration_module):
    with run_integration_module("sync_nested_genexpr_progress") as module:
        assert module.collect_progress() == [[0, 1, 2]]
