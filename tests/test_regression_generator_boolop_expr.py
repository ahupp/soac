def test_generator_boolop_expr_lowers_without_fragment_entry_panic(run_integration_module):
    with run_integration_module("generator_boolop_expr") as module:
        assert module.main() == ([1], [False])
