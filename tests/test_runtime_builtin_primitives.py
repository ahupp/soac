import pytest

from tests._integration import soac_module


def test_ord_chr_roundtrip_uses_runtime_builtin_primitives(tmp_path):
    source = """
def codepoint(value):
    return ord(value)

def roundtrip(value):
    return chr(ord(value))

def item_count(value):
    return len(value)

def chr_count(value):
    return chr(len(value))

def from_literal():
    return chr(65)

def from_local():
    value = 65
    return chr(value)

def local_overflow():
    value = 1267650600228229401496703205376
    return chr(value)
"""

    with soac_module(tmp_path, "runtime_builtin_primitives", source) as module:
        assert module.codepoint("A") == 65
        assert module.roundtrip("Z") == "Z"
        assert module.item_count("abc") == 3
        assert module.item_count([1, 2, 3]) == 3
        assert module.chr_count("x" * 65) == "A"
        assert module.from_literal() == "A"
        assert module.from_local() == "A"
        with pytest.raises(ValueError):
            module.local_overflow()


def test_ord_chr_runtime_builtin_primitive_errors(tmp_path):
    source = """
def bad_ord(value):
    return ord(value)

def bad_chr(value):
    return chr(ord(value))

def bad_len(value):
    return len(value)
"""

    with soac_module(tmp_path, "runtime_builtin_errors", source) as module:
        with pytest.raises(TypeError):
            module.bad_ord("AB")
        with pytest.raises(TypeError):
            module.bad_chr("AB")
        with pytest.raises(TypeError):
            module.bad_len(5)
