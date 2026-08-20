"""Export test metadata from the selected native compiler, without execution.

The code root pins every borrowed view until code identity and source identity
have been checked. Only pointer-free data crosses this test-process boundary;
it is neither a runtime admission token nor a checker fact.
"""

import ctypes
import hashlib
import inspect
import json
import sys
import types


class RawPySoacCodeView(ctypes.Structure):
    _fields_ = [
        ("abi_version", ctypes.c_uint),
        *[(name, ctypes.c_int) for name in (
            "flags", "argcount", "posonlyargcount", "kwonlyargcount",
            "stacksize", "firstlineno", "nlocalsplus", "framesize",
            "nlocals", "ncellvars", "nfreevars",
        )],
        ("code_units", ctypes.c_ssize_t),
        ("strict_source_id", ctypes.c_uint64),
        *[(name, ctypes.c_void_p) for name in (
            "consts", "names", "localsplusnames", "localspluskinds",
            "filename", "name", "qualname", "linetable", "exceptiontable",
        )],
    ]


compile_details = ctypes.pythonapi.PySoac_CompileVerifiedSourceDetails
compile_details.argtypes = [ctypes.c_char_p, ctypes.c_ssize_t, ctypes.py_object, ctypes.c_int]
compile_details.restype = ctypes.py_object
get_view = ctypes.pythonapi.PySoac_GetCodeView
get_view.argtypes = [ctypes.py_object, ctypes.POINTER(RawPySoacCodeView), ctypes.c_size_t]
get_view.restype = ctypes.c_int

source = sys.stdin.buffer.read()
source.decode("utf-8")
root, _annotations, packet = compile_details(
    source, len(source), "<lowering-native-class-bindings>", 0
)
assert type(packet) is tuple and len(packet) == 4
schema, rows, recipes, _operations = packet
assert type(schema) is int and schema == 7
assert type(rows) is tuple and type(recipes) is tuple

line_starts = [0, *(i + 1 for i, byte in enumerate(source) if byte == 10)]


def offset(line, column):
    assert type(line) is int and type(column) is int
    assert 1 <= line <= len(line_starts) and column >= 0
    start = line_starts[line - 1]
    end = line_starts[line] if line < len(line_starts) else len(source)
    result = start + column
    assert start <= result <= end
    source[:result].decode("utf-8")
    return result


def source_range(span):
    if span is None:
        return None
    assert type(span) is tuple and len(span) == 4
    result = [offset(*span[:2]), offset(*span[2:])]
    assert result[0] <= result[1]
    return result


nodes = []
native_views = []
seen = set()
pending = [(root, None)]
source_id = None
while pending:
    code, parent = pending.pop()
    assert type(code) is types.CodeType and id(code) not in seen
    seen.add(id(code))
    view = RawPySoacCodeView()
    assert get_view(code, ctypes.byref(view), ctypes.sizeof(view)) == 0
    assert view.abi_version == 1 and view.strict_source_id != 0
    if source_id is None:
        source_id = view.strict_source_id
    assert view.strict_source_id == source_id
    ordinal = len(nodes)
    row = rows[ordinal]
    assert type(row) is tuple and len(row) == 6
    assert type(row[0]) is int and row[0] == ordinal
    assert row[1] is None or type(row[1]) is int
    assert row[1] == parent and row[2] is code
    constants = ctypes.cast(view.consts, ctypes.py_object).value
    names = ctypes.cast(view.localsplusnames, ctypes.py_object).value
    kinds = ctypes.cast(view.localspluskinds, ctypes.py_object).value
    assert type(constants) is tuple and type(names) is tuple and type(kinds) is bytes
    assert len(names) == len(kinds) == view.nlocalsplus
    assert 0 <= view.nfreevars <= view.nlocalsplus
    assert all(type(name) is str for name in names)
    nodes.append([
        ordinal, parent, row[3], row[4], source_range(row[5]),
        list(zip(names, kinds, strict=True)), view.nfreevars, view.firstlineno,
    ])
    native_views.append(view)
    pending.extend(
        (constant, ordinal) for constant in reversed(constants)
        if type(constant) is types.CodeType
    )
assert len(nodes) == len(rows)


def pointer_free(value):
    if type(value) is tuple:
        return [pointer_free(item) for item in value]
    assert value is None or type(value) is int
    return value



def exact_tuple(value, length=None):
    assert type(value) is tuple
    assert length is None or len(value) == length
    return value


def unsigned(value):
    assert type(value) is int and 0 <= value <= 0xFFFFFFFF
    return value


def optional_unsigned(value):
    return None if value is None else unsigned(value)


def class_projection(recipe, view, native_node):
    """Class semantic cells only; comprehension helpers own their iteration state."""
    code_id, seeds, owners, regions, captures, accesses, actions = recipe
    assert not view.flags & (inspect.CO_GENERATOR | inspect.CO_COROUTINE | inspect.CO_ASYNC_GENERATOR)
    slots = native_node[5]
    entry_owners = {}
    entry_slots = set()
    for index, owner in enumerate(owners):
        owner_id, kind, slot, native_kind, region = exact_tuple(owner, 5)
        assert unsigned(owner_id) == index
        if unsigned(kind) != 0:
            assert kind in (1, 2)
            continue
        assert region is None and unsigned(slot) < len(slots)
        assert unsigned(native_kind) == slots[slot][1] and slot not in entry_slots
        entry_owners[owner_id] = slot
        entry_slots.add(slot)
    first_free = len(slots) - view.nfreevars
    assert first_free >= 0
    selected = set(range(first_free, len(slots)))
    projected_captures = []
    for capture in captures:
        child, creation, ordinal, current, region = exact_tuple(capture, 5)
        if optional_unsigned(region) is not None:
            continue
        tag, slot = exact_tuple(current, 2)
        assert unsigned(tag) == 0
        selected.add(unsigned(slot))
        projected_captures.append([
            unsigned(child), source_range(creation), unsigned(ordinal), pointer_free(current)
        ])
    projected_accesses = []
    for access in accesses:
        original, context, selection, current, region = exact_tuple(access, 5)
        if optional_unsigned(region) is not None:
            continue
        tag, slot = exact_tuple(current, 2)
        assert unsigned(tag) == 0
        selected.add(unsigned(slot))
        projected_accesses.append([
            source_range(original), unsigned(context), unsigned(selection), pointer_free(current)
        ])
    header, exports = exact_tuple(actions, 2)
    stores = []
    for action in exact_tuple(header):
        owner, kind, operand = exact_tuple(action, 3)
        assert unsigned(kind) in (3, 4) and operand is None
        slot = entry_owners[unsigned(owner)]
        selected.add(slot)
        stores.append([1, slot, kind, None])
    for export in exact_tuple(exports):
        kind, current = exact_tuple(export, 2)
        tag, slot = exact_tuple(current, 2)
        assert unsigned(tag) == 0
        selected.add(unsigned(slot))
    initializers = []
    for slot in sorted(selected):
        assert slot in entry_slots and slots[slot][1] & (0x40 | 0x80)
        if slot >= first_free:
            assert slots[slot][1] == 0x80
            initializers.append([0, slot, 2, slot - first_free])
        else:
            assert not slots[slot][1] & 0x80
            initializers.append([0, slot, 1, None])
    initializers.extend(stores)
    return [
        code_id, initializers, projected_captures,
        pointer_free(exact_tuple(exports)), projected_accesses,
    ]


# Only pointer-free class projections cross this test-process boundary. The
# exact original code tree, binder seeds and semantic slot/source identities
# are checked while root pins the tree; no execution-schedule proof is required.
decoded_recipes = []
assert len(recipes) == len(native_views) == len(nodes)
for index, (recipe, view, node) in enumerate(zip(
    recipes, native_views, nodes, strict=True
)):
    recipe = exact_tuple(recipe, 7)
    assert unsigned(recipe[0]) == index
    for field in (1, 2, 3, 4, 5):
        exact_tuple(recipe[field])
    is_class = node[2] == 1
    if is_class:
        header, exports = exact_tuple(recipe[6], 2)
        exact_tuple(header)
        exact_tuple(exports)
    else:
        assert recipe[6] is None
    assert 0 <= view.posonlyargcount <= view.argcount and view.kwonlyargcount >= 0
    parameters = view.argcount + view.kwonlyargcount
    parameters += bool(view.flags & inspect.CO_VARARGS) + bool(view.flags & inspect.CO_VARKEYWORDS)
    assert parameters <= len(node[5]) and (not is_class or parameters == 0)
    for slot, seed in enumerate(exact_tuple(recipe[1], len(node[5]))):
        native_slot, native_kind, kind, ordinal = exact_tuple(seed, 4)
        parameter = slot < parameters
        assert unsigned(native_slot) == slot and unsigned(native_kind) == node[5][slot][1]
        assert unsigned(kind) == int(parameter)
        assert optional_unsigned(ordinal) == (slot if parameter else None)
    for owner in recipe[2]:
        exact_tuple(owner, 5)
    for region in recipe[3]:
        exact_tuple(region, 8)
    for field in (4, 5):
        for row in recipe[field]:
            exact_tuple(row, 5)
    if is_class:
        decoded_recipes.append(class_projection(recipe, view, node))

json.dump({
    "native_schema": schema,
    "source_sha256": hashlib.sha256(source).hexdigest(),
    "nodes": nodes,
    "class_projection": decoded_recipes,
}, sys.stdout)
