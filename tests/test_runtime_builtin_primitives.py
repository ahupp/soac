import pytest

from tests._integration import soac_module


def test_ord_chr_roundtrip_uses_runtime_builtin_primitives(tmp_path):
    source = """
def codepoint(value):
    return ord(value)

def roundtrip(value):
    return chr(ord(value))

def from_literal():
    return chr(65)
"""

    with soac_module(tmp_path, "runtime_builtin_primitives", source) as module:
        assert module.codepoint("A") == 65
        assert module.roundtrip("Z") == "Z"
        assert module.from_literal() == "A"


def test_ord_chr_runtime_builtin_primitive_errors(tmp_path):
    source = """
def bad_ord(value):
    return ord(value)

def bad_chr(value):
    return chr(ord(value))
"""

    with soac_module(tmp_path, "runtime_builtin_errors", source) as module:
        with pytest.raises(TypeError):
            module.bad_ord("AB")
        with pytest.raises(TypeError):
            module.bad_chr("AB")
