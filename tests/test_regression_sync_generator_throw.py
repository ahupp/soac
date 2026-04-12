import pytest


def test_sync_generator_throw_handles_except_name_cleanup(run_integration_module):
    with run_integration_module("sync_generator_throw_cleanup") as module:
        gen_obj = module.make_gen()
        assert next(gen_obj) == 3
        assert gen_obj.throw(ValueError("boom")) == 7
        with pytest.raises(StopIteration):
            next(gen_obj)
        assert module.exercise() == (3, 7)


def test_yield_from_throw_clears_delegate(run_integration_module):
    with run_integration_module("yield_from_throw_clears_delegate") as module:
        assert module.throw_check() is None
