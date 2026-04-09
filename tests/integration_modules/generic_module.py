from typing import Generic, TypeVar


T = TypeVar("T")


class Box(Generic[T]):
    pass


def make_specialization():
    class IntBox(Box[int]):
        pass

    return IntBox

# diet-python: validate

def validate_module(module):

    import sys
    import builtins
    from types import ModuleType


    def _assert_generic_module_invariants(module: ModuleType) -> None:
        transformed_typing = sys.modules["typing"]
        if __dp_integration_transformed__:
            assert type(module) is ModuleType, "transformed modules should use a real module object"
            assert "_dp_module_init" not in module.__dict__, "_dp_module_init should not leak into module globals"
            assert "runtime" not in module.__dict__, "runtime should not be injected into module globals"
            assert not hasattr(builtins, "runtime"), "runtime should not be injected into builtins"
            assert not hasattr(builtins, "__soac__"), "__soac__ should not be injected into builtins"

        assert module.Box.__orig_bases__ == (transformed_typing.Generic[module.T],)

        specialized = module.make_specialization()
        assert specialized.__orig_bases__[0].__args__ == (int,)
        assert issubclass(specialized, module.Box)

    _assert_generic_module_invariants(module)
