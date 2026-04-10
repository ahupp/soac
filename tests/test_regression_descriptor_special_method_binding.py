import pytest

from tests._integration import integration_module


@pytest.mark.parametrize("mode", ["soac"])
def test_special_methods_bind_descriptor_before_call(tmp_path, mode):
    source = """
class BindingDescriptor:
    def __init__(self, label):
        self.label = label

    def __get__(self, obj, owner):
        def bound(other):
            return (self.label, obj.value, getattr(other, "value", other))
        return bound

    def __call__(self, *args, **kwargs):
        raise AssertionError("unbound descriptor called")


class C:
    __add__ = BindingDescriptor("add")
    __eq__ = BindingDescriptor("eq")

    def __init__(self, value):
        self.value = value


def run():
    lhs = C(10)
    rhs = C(3)
    return lhs + 5, lhs == rhs
"""

    with integration_module(tmp_path, "descriptor_special_binding", source, mode=mode) as module:
        assert module.run() == (("add", 10, 5), ("eq", 10, 3))
