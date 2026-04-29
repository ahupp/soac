def test_effect_only_selected_expr_semantics(run_integration_module):
    with run_integration_module("effect_only_selected_expr_semantics") as module:
        assert module.ifexpr_effect() == ["call:cond", "bool:cond", "final:then"]
        assert module.boolop_and_effect() == ["call:left", "bool:left", "final:right"]
        assert module.boolop_or_effect() == [
            "call:left",
            "bool:left",
            "final:fallback",
        ]
        assert module.not_boolop_effect() == [
            "call:left",
            "bool:left",
            "call:right",
            "bool:right",
        ]
        assert module.compare_chain_effect() == ["lt:a:b", "bool:a:b", "lt:b:c"]
