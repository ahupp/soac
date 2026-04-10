import pytest

from tests._integration import integration_module


@pytest.mark.parametrize("mode", ["soac"])
def test_bound_methods_compare_like_cpython(tmp_path, mode):
    source = """

class C:
    def f(self):
        return None


def run():
    c1 = C()
    c2 = C()
    return (
        c1.f == c1.f,
        c1.f != c1.f,
        c1.f == c2.f,
        c1.f != c2.f,
    )
"""

    with integration_module(tmp_path, "bound_method_equality", source, mode=mode) as module:
        assert module.run() == (True, False, False, True)
