def test_nested_named_generator_list_consumption(run_integration_module):
    with run_integration_module("sync_generator_nested_list") as module:
        assert module.collect_single() == [3]
        assert module.collect_tupled() == [(0, 1, 2)]
