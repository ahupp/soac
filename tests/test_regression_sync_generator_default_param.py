def test_named_generator_uses_default_parameter_when_building_preserved_state(
    run_integration_module,
):
    with run_integration_module("sync_generator_default_param") as module:
        assert module.collect_default() == [0, 1, 2]
        assert module.collect_explicit() == [0, 1]
