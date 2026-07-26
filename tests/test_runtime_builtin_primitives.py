import pytest

from tests._integration import soac_module


def test_ord_chr_roundtrip_uses_runtime_builtin_primitives(tmp_path):
    source = """
def codepoint(value):
    return ord(value)

def bytes_codepoint(value):
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
        assert module.bytes_codepoint(b"A") == 65
        assert module.bytes_codepoint(bytearray(b"B")) == 66
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
            module.bad_ord(b"AB")
        with pytest.raises(TypeError):
            module.bad_ord(bytearray(b"AB"))
        with pytest.raises(TypeError):
            module.bad_chr("AB")
        with pytest.raises(TypeError):
            module.bad_len(5)


def test_runtime_range_is_reusable_iterable(tmp_path):
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

def collect_reversed_stop(stop):
    return list(reversed(range(stop)))

def collect_reversed_step(start, stop, step):
    return list(reversed(range(start, stop, step)))

def make_range(stop):
    return range(stop)

def iterator_type_name(stop):
    return type(iter(range(stop))).__name__

def range_type_name(stop):
    value = range(stop)
    return type(value).__module__, type(value).__name__

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
        assert module.collect_reversed_stop(4) == [3, 2, 1, 0]
        assert module.collect_reversed_step(1, 8, 2) == [7, 5, 3, 1]
        assert module.collect_reversed_step(8, 1, -3) == [2, 5, 8]
        actual_range = module.make_range(3)
        assert type(actual_range) is range
        assert actual_range == range(3)
        assert actual_range[1:] == range(1, 3)
        assert module.iterator_type_name(3) == type(iter(range(3))).__name__
        assert module.range_type_name(3) == (range.__module__, range.__name__)
        assert module.collect_indexable() == [1, 3]
        with pytest.raises(ValueError):
            module.zero_step()
        with pytest.raises(TypeError):
            module.no_args()
        with pytest.raises(TypeError):
            module.bad_index()
        with pytest.raises(TypeError):
            module.no_index()


def test_for_range_stop_iteration_match_preserves_loop_semantics(tmp_path):
    source = """
def total(n):
    result = 0
    for value in range(n):
        result += value
    return result

def loop_else_complete(n):
    result = []
    for value in range(n):
        result.append(value)
    else:
        result.append("done")
    return result

def loop_else_break(n):
    result = []
    for value in range(n):
        if value == 2:
            break
        result.append(value)
    else:
        result.append("done")
    return result
"""

    with soac_module(tmp_path, "runtime_stop_iteration_match", source) as module:
        assert module.total(6) == 15
        assert module.loop_else_complete(3) == [0, 1, 2, "done"]
        assert module.loop_else_break(5) == [0, 1]
