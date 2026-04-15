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


def test_range_runtime_builtin_is_reusable_iterable(tmp_path):
    source = """
def collect_stop(stop):
    return list(range(stop))

def collect_start_stop(start, stop):
    return list(range(start, stop))

def collect_step(start, stop, step):
    return list(range(start, stop, step))

def reuse_range(stop):
    values = range(stop)
    return list(values), list(values)

def reuse_iterator(stop):
    iterator = iter(range(stop))
    return list(iterator), list(iterator)

def zero_step():
    return list(range(1, 3, 0))

def no_args():
    return range()

class Indexable:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value

class BadIndex:
    def __index__(self):
        return "not-int"

def collect_indexable():
    return list(range(Indexable(1), Indexable(5), Indexable(2)))

def bad_index():
    return list(range(BadIndex()))

def no_index():
    return list(range(object()))
"""

    with soac_module(tmp_path, "runtime_range_builtin", source) as module:
        assert module.collect_stop(4) == [0, 1, 2, 3]
        assert module.collect_start_stop(2, 5) == [2, 3, 4]
        assert module.collect_step(5, 1, -2) == [5, 3]
        assert module.reuse_range(3) == ([0, 1, 2], [0, 1, 2])
        assert module.reuse_iterator(3) == ([0, 1, 2], [])
        assert module.collect_indexable() == [1, 3]
        with pytest.raises(ValueError):
            module.zero_step()
        with pytest.raises(TypeError):
            module.no_args()
        with pytest.raises(TypeError):
            module.bad_index()
        with pytest.raises(TypeError):
            module.no_index()
