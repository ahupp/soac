import pytest


def test_genexpr_original_code_accepts_iterable(run_integration_module):
    with run_integration_module("genexpr_iterator_semantics") as module:
        assert module.main() == [1, 2]


def test_genexpr_original_code_rejects_non_iterable(run_integration_module):
    with run_integration_module("genexpr_iterator_semantics") as module:
        with pytest.raises(TypeError, match=r"object is not iterable"):
            module.replay(42)
