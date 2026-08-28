from __future__ import annotations

import pytest

from tests._strict_integration import create_strict_project


# Original ordinary source, retained unchanged. Strict enrollment is explicit
# in the fixture; import-hook/mode settings alone are never admission evidence.
_MODULE_SOURCE = """
import gc


events = []
generation = object()


def canonical(offset, values):
    return (
        [(offset, value) for value in values],
        {(offset, value) for value in values},
        {value: (offset, value) for value in values},
        {value: [(offset, inner) for inner in (value, value)]
         for value in values},
    )


def mixed(offset):
    return [sum((offset, value)) for value in range(2)]


def prepatched(offset):
    return [sum((offset, value)) for value in range(2)]


def postpatched(offset):
    return [sum((offset, value)) for value in range(2)]


def modified_factory_code(offset):
    return [sum((offset, value)) for value in range(2)]


def both_factories_replaced(offset):
    return [sum((offset, value)) for value in range(2)]


def reentrant(offset):
    return [sum((offset, value)) for value in range(2)]


def replaced_module(offset):
    return [sum((offset, value)) for value in range(2)]


def observed(offset):
    return [sum((offset, value)) for value in range(2)]


def forced(offset):
    return [sum((offset, value)) for value in range(2)]


def lazy(offset):
    return (sum((offset, value)) for value in range(2))


def source_function(offset):
    def original(value):
        return sum((offset, value))

    return original


def spoofed_source_function(offset):
    def _dp_listcomp_777(value):
        return sum((offset, value))

    return _dp_listcomp_777


def increment(value):
    return value + 1


def unrelated(value):
    return increment(value)


class Item:
    def __init__(self, fail=False):
        self.fail = fail
        self.generation = generation

    def read(self):
        events.append("read")
        if self.fail:
            raise RuntimeError("body failed")
        return 7

    def __del__(self):
        if self.generation is generation:
            events.append("drop-item")


class Items:
    def __init__(self, fail=False):
        self.fail = fail
        self.done = False
        self.generation = generation

    def __iter__(self):
        events.append("iter")
        return self

    def __next__(self):
        if self.done:
            raise StopIteration
        self.done = True
        return Item(self.fail)

    def __del__(self):
        if self.generation is generation:
            events.append("drop-iterator")


def lifetime(fail=False):
    global generation
    generation = object()
    events.clear()
    values = Items(fail)
    try:
        result = [item.read() for item in values]
    except RuntimeError as error:
        assert str(error) == "body failed"
        events.append("caught")
        result = None
    del values
    gc.collect()
    return result, tuple(events)


class Cycle:
    def __init__(self):
        self.cycle = self
        self.value = 11
        self.generation = generation

    def __del__(self):
        if self.generation is generation:
            events.append("drop-cycle")


def collect_cycle():
    global generation
    generation = object()
    events.clear()
    owner = Cycle()
    result = [(gc.collect(), owner.value)[1] for _ in range(2)]
    del owner
    gc.collect()
    return result, events.count("drop-cycle")
"""


@pytest.fixture(scope="module", params=("soac", "cpython"))
def eager_project(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp(f"eager-semantic-{request.param}"),
        {
            "eager_source.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _MODULE_SOURCE,
            "eager_ordinary.py": _MODULE_SOURCE,
        },
        modules={"eager_source": "eager_source.py"},
        backend=request.param,
    )


def _run_modes(project, name, validation, required_functions):
    path = project.root / f"{name}-validation.py"
    path.write_text(validation)
    for entry in ((False, True) if project.backend == "soac" else (False,)):
        project.run_case(
            name,
            validation,
            path,
            entry_interpreter=entry,
            required_functions=required_functions,
        )


def test_set_comprehension_does_not_read_shadowable_constructor(eager_project):
    validation = """
import eager_ordinary
import eager_source

events = []

def shadowed_set(*args, **kwargs):
    events.append((args, kwargs))
    raise AssertionError("set comprehension looked up the source name 'set'")

for module in (eager_ordinary, eager_source):
    module.set = shadowed_set
    assert module.canonical(4, (1, 2)) == (
        [(4, 1), (4, 2)],
        {(4, 1), (4, 2)},
        {1: (4, 1), 2: (4, 2)},
        {1: [(4, 1), (4, 1)], 2: [(4, 2), (4, 2)]},
    )
assert events == []
"""
    _run_modes(
        eager_project,
        "eager_source",
        validation,
        required_functions=["canonical"],
    )


_LAMBDA_DEFAULT_SOURCE = """
body_only = -99


def record(events, label, value):
    events.append((label, value))
    return value


def direct(value, events):
    # These containing-scope bindings exist only because defaults write them.
    callback = (
        lambda argument=(saved := record(events, "pos", value)), /,
        *, keyword=(keyword_saved := record(events, "kw", saved + 10)):
        (body_only := record(events, "body", argument + keyword), saved, keyword_saved)
    )
    def read():
        return saved, keyword_saved, body_only
    return callback, read


def eager(values, events):
    saved = keyword_saved = -1
    callbacks = [
        (lambda argument=(saved := record(events, "pos", value)), /,
         *, keyword=(keyword_saved := record(events, "kw", saved + 10)):
         (body_only := record(events, "body", argument + keyword), saved, keyword_saved))
        for value in values
    ]
    def read():
        return saved, keyword_saved, body_only
    return callbacks, read


def lazy(values, events):
    saved = keyword_saved = -1
    callbacks = (
        (lambda argument=(saved := record(events, "pos", value)), /,
         *, keyword=(keyword_saved := record(events, "kw", saved + 10)):
         (body_only := record(events, "body", argument + keyword), saved, keyword_saved))
        for value in values
    )
    def read():
        return saved, keyword_saved, body_only
    return callbacks, read
"""


@pytest.mark.parametrize("backend", ("soac", "cpython"))
def test_lambda_default_walrus_preserves_containing_scope_and_evaluation_order(
    tmp_path, backend,
):
    project = create_strict_project(
        tmp_path,
        {
            "lambda_defaults.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _LAMBDA_DEFAULT_SOURCE,
            "ordinary_lambda_defaults.py": _LAMBDA_DEFAULT_SOURCE,
        },
        modules={"lambda_defaults": "lambda_defaults.py"},
        backend=backend,
    )
    _run_modes(
        project,
        "lambda_defaults",
        r'''
import ctypes
import ordinary_lambda_defaults as stock

def validate_module(module):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    for name in ("record", "direct", "eager", "lazy"):
        assert owner(getattr(stock, name)) is None

    for target in (stock, module):
        events = []
        callback, read = target.direct(3, events)
        assert events == [("pos", 3), ("kw", 13)]
        assert read() == (3, 13, -99)
        assert callback() == (16, 3, 13)
        assert callback(20, keyword=2) == (22, 3, 13)
        assert read() == (3, 13, -99)
        assert events == [("pos", 3), ("kw", 13), ("body", 16), ("body", 22)]

        events = []
        callbacks, read = target.eager((2, 4), events)
        assert events == [("pos", 2), ("kw", 12), ("pos", 4), ("kw", 14)]
        assert read() == (4, 14, -99)
        # Defaults keep each iteration's value; the body reads the shared cells.
        assert [callback() for callback in callbacks] == [(14, 4, 14), (18, 4, 14)]
        assert read() == (4, 14, -99)
        assert events == [
            ("pos", 2), ("kw", 12), ("pos", 4), ("kw", 14),
            ("body", 14), ("body", 18),
        ]

        events = []
        callbacks, read = target.lazy((5, 7), events)
        try:
            assert events == [] and read() == (-1, -1, -99)
            first = next(callbacks)
            assert events == [("pos", 5), ("kw", 15)]
            assert read() == (5, 15, -99)
            assert first() == (20, 5, 15)
            second = next(callbacks)
            assert read() == (7, 17, -99)
            assert events == [
                ("pos", 5), ("kw", 15), ("body", 20), ("pos", 7), ("kw", 17),
            ]
            assert first() == (20, 7, 17)
            assert second() == (24, 7, 17)
            assert list(callbacks) == []
            assert read() == (7, 17, -99)
            assert events == [
                ("pos", 5), ("kw", 15), ("body", 20), ("pos", 7), ("kw", 17),
                ("body", 20), ("body", 24),
            ]
        finally:
            callbacks.close()
        # A lambda-body walrus is local to that lambda, not a containing write.
        assert target.body_only == -99
''',
        ("record", "direct", "eager", "lazy"),
    )



_NESTED_DEFAULT_LAMBDA_SUPER_SOURCE = """
class Base:
    pass


class Derived(Base):
    def build(self):
        def nested(callback=lambda receiver: (super(), [item for item in (1,)])):
            return callback
        return nested()
"""


@pytest.fixture(scope="module")
def nested_default_lambda_super_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("nested-default-lambda-super"),
        {
            "nested_default_lambda_super.py": (
                "# soac: module(strict_assign=true, checked_attr=true)\n" + _NESTED_DEFAULT_LAMBDA_SUPER_SOURCE
            ),
            "ordinary_nested_default_lambda_super.py": _NESTED_DEFAULT_LAMBDA_SUPER_SOURCE,
        },
        modules={"nested_default_lambda_super": "nested_default_lambda_super.py"},
    )


@pytest.mark.parametrize("entry_interpreter", (False, True), ids=("compiled", "entry"))
def test_nested_default_lambda_super_uses_its_own_receiver(
    nested_default_lambda_super_project, entry_interpreter,
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    validation = f"""
def validate_module(module):
    import ctypes
    import types
    from soac import _soac_ext
    from tests._strict_integration import _plain_function_witness
    import ordinary_nested_default_lambda_super as stock

    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    source = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
    source.argtypes = [ctypes.py_object]
    source.restype = ctypes.c_uint64

    ordinary_build = _plain_function_witness(stock, "Derived.build")
    assert owner(ordinary_build) is None and metadata(ordinary_build) is None
    for target in (stock, module):
        builder = target.Derived()
        receiver = target.Derived()
        assert receiver is not builder
        callback = builder.build()
        assert type(callback) is types.FunctionType
        # The default expression belongs to build, not the nested def's body.
        assert callback.__qualname__ == "Derived.build.<locals>.<lambda>"
        code = callback.__code__
        if target is module:
            assert metadata(callback) and owner(callback) and source(code)
            assert _soac_ext.strict_function_entry_kind(callback) == {expected_entry!r}
        else:
            assert owner(callback) is None and metadata(callback) is None
            assert source(code) == 0

        proxy, items = callback(receiver)
        assert type(proxy) is super
        assert proxy.__self__ is receiver
        assert proxy.__self__ is not builder
        assert proxy.__thisclass__ is target.Derived
        assert items == [1]
        assert callback.__code__ is code
        if target is module:
            assert metadata(callback) and owner(callback) and source(code)
            assert _soac_ext.strict_function_entry_kind(callback) == {expected_entry!r}
"""
    project = nested_default_lambda_super_project
    validation_path = project.root / f"nested-default-lambda-super-{expected_entry}-validation.py"
    validation_path.write_text(validation)
    project.run_case(
        "nested_default_lambda_super", validation, validation_path,
        entry_interpreter=entry_interpreter,
        required_functions=("Derived.build",),
    )


_CLASS_COMPREHENSION_SOURCE = """
label = "module"
events = []


def record(kind, value):
    events.append(kind)
    return value


def build(outside):
    class Box:
        label = "class"
        inputs = (1, 2)
        values = [record("value", item + outside)
                  for item in record("iterable", inputs)]
        labels = [label for _ in inputs]
        pairs = [(left, right, outside) for left, right in ((3, 4), (5, 6))]
        nested = [[left + right for right in (1, 2)] for left in (10, 20)]
        callbacks = [lambda: outside for outside in (5, 6)]
        owner_callbacks = [lambda: __class__ for _ in (0, 1)]

        def read_outer(self):
            return outside

        def owner(self):
            return __class__

    return Box
"""


@pytest.mark.parametrize("backend", ("soac", "cpython"))
def test_class_comprehensions_keep_lexical_cells_without_native_slot_correspondence(
    tmp_path, backend,
):
    project = create_strict_project(
        tmp_path,
        {
            "class_comprehensions.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _CLASS_COMPREHENSION_SOURCE,
            "ordinary_class_comprehensions.py": _CLASS_COMPREHENSION_SOURCE,
        },
        modules={"class_comprehensions": "class_comprehensions.py"},
        backend=backend,
    )
    _run_modes(
        project,
        "class_comprehensions",
        r'''
import ctypes
import ordinary_class_comprehensions as stock

def validate_module(module):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(stock.build) is None and owner(stock.record) is None
    for target in (stock, module):
        target.events.clear()
        box = target.build(10)
        assert target.events == ["iterable", "value", "value"]
        assert box.values == [11, 12]
        # Only the first iterable is evaluated in the class namespace.
        assert box.labels == ["module", "module"] and box.label == "class"
        assert box.pairs == [(3, 4, 10), (5, 6, 10)]
        assert box.nested == [[11, 12], [21, 22]]
        assert [callback() for callback in box.callbacks] == [6, 6]
        assert all(callback() is box for callback in box.owner_callbacks)
        assert box().owner() is box
        # The helper's loop cell cannot replace the genuine outer FREE cell.
        assert box().read_outer() == 10
        assert not {"item", "left", "right", "outside", "_"} & vars(box).keys()
''',
        ("record", "build"),
    )


def test_eager_comprehensions_preserve_semantics_and_eventual_cleanup(eager_project):
    _run_modes(
        eager_project,
        "eager_source",
        _SEMANTIC_VALIDATION,
        (
            "canonical", "mixed", "prepatched", "postpatched", "modified_factory_code",
            "both_factories_replaced", "reentrant", "replaced_module", "observed", "forced",
            "source_function", "spoofed_source_function", "increment", "unrelated",
            "lifetime", "collect_cycle",
        ),
    )


_SEMANTIC_VALIDATION = r'''
import ctypes
import gc
import types
import eager_ordinary as stock

def validate_module(module):
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    seal = ctypes.pythonapi.PyFunction_GetSoacStrictId
    seal.argtypes = [ctypes.py_object]
    seal.restype = ctypes.c_uint64
    assert owner(stock.canonical) is None and seal(stock.canonical) == 0

    for offset in (3, 30):
        assert module.canonical(offset, (1, 2)) == stock.canonical(offset, (1, 2))
    for offset in range(32):
        assert module.mixed(offset) == stock.mixed(offset) == [offset, offset + 1]
        assert module.unrelated(offset) == stock.unrelated(offset) == offset + 1
    for name, offsets in (
        ("prepatched", (5, 100)), ("postpatched", (6, 60)),
        ("modified_factory_code", (7,)), ("both_factories_replaced", (8,)),
        ("reentrant", (9, 80)), ("replaced_module", (10,)),
        ("observed", (20, 21, 22, 23)), ("forced", (24,)),
    ):
        for offset in offsets:
            assert getattr(module, name)(offset) == getattr(stock, name)(offset)

    stock_lazy = stock.lazy(12)
    actual_lazy = module.lazy(12)
    assert next(actual_lazy) == next(stock_lazy) == 12
    assert list(actual_lazy) == list(stock_lazy) == [13]

    first, second = module.source_function(13), module.source_function(130)
    assert type(first) is type(second) is types.FunctionType
    assert first is not second and first(1) == 14 and second(1) == 131
    assert first.__code__ is second.__code__
    assert first.__closure__[0] is not second.__closure__[0]
    assert owner(first) and owner(second) and seal(first) and seal(second)
    spoofed = module.spoofed_source_function(14)
    assert type(spoofed) is types.FunctionType and spoofed(1) == 15
    assert spoofed.__code__.co_name == "_dp_listcomp_777"
    assert owner(spoofed) and seal(spoofed)

    for fail in (False, True):
        previous_generation = module.generation
        expected = stock.lifetime(fail)
        actual = module.lifetime(fail)
        # Preserve the original ordinary control, including its ordinary
        # exception cleanup. Only SOAC's implicit-release schedule is relaxed.
        if fail:
            assert expected == (None, ("iter", "read", "caught", "drop-item", "drop-iterator"))
        assert actual[0] == expected[0]
        gc.collect()
        explicit = tuple(event for event in module.events if not event.startswith("drop-"))
        assert explicit == (("iter", "read", "caught") if fail else ("iter", "read"))
        assert module.events.count("drop-item") == 1, module.events
        assert module.events.count("drop-iterator") == 1, module.events
        assert module.generation is not previous_generation
    assert stock.collect_cycle() == ([11, 11], 1)
    previous_generation = module.generation
    values, _inside_body_release_count = module.collect_cycle()
    assert values == [11, 11]
    gc.collect()
    assert module.events.count("drop-cycle") == 1, module.events
    assert module.generation is not previous_generation
'''


def test_declared_global_rebinding_preserves_frozen_other_bindings(eager_project):
    _run_modes(
        eager_project,
        "eager_source",
        r'''
import pytest
import eager_ordinary as stock
from soac.strict import StrictMutationError

def validate_module(module):
    # A lexical `global generation` explicitly declares this binding mutable.
    # The original source is legal; it needs no holder or per-class opt-in.
    events = module.events
    module.events.append("unchanged")
    with pytest.raises(StrictMutationError):
        module.events = []
    assert module.events is events and events == ["unchanged"]
    for operation in (module.lifetime, module.collect_cycle):
        generation = module.generation
        operation()
        assert module.generation is not generation
        assert module.events is events
    replacement = object()
    module.generation = replacement
    assert module.generation is replacement
    assert module.events is events
    old = stock.generation
    assert stock.lifetime()[0] == [7]
    assert stock.generation is not old
    old = stock.generation
    assert stock.collect_cycle() == ([11, 11], 1)
    assert stock.generation is not old
''',
        ("lifetime", "collect_cycle"),
    )


def test_eager_untraced_callbacks_and_ordinary_observers(eager_project):
    _run_modes(
        eager_project,
        "eager_source",
        r'''
import sys
import eager_ordinary as stock

def validate_module(module):
    # Only ordinary and CPython-backend source execution promises these events.
    # SOAC validates the same explicit arithmetic callbacks without observers.
    def exercise(function, observer=None):
        calls, observed = [], []
        code = function.__code__
        class Offset:
            def __radd__(self, value):
                calls.append(("add", value))
                return 20 + value
        def trace(frame, event, argument):
            if frame.f_code is code and event == "call":
                observed.append("call")
            return trace
        def monitor(actual_code, offset):
            if actual_code is code:
                observed.append("start")
        tool = None
        if observer is None:
            install = remove = lambda: None
        elif observer == "trace":
            install = lambda: sys.settrace(trace)
            remove = lambda: sys.settrace(None)
        elif observer == "profile":
            install = lambda: sys.setprofile(trace)
            remove = lambda: sys.setprofile(None)
        else:
            tool = next(index for index in range(6) if sys.monitoring.get_tool(index) is None)
            sys.monitoring.use_tool_id(tool, "eager-comprehension-policy")
            sys.monitoring.register_callback(tool, sys.monitoring.events.PY_START, monitor)
            if observer == "monitor-local":
                install = lambda: sys.monitoring.set_local_events(tool, code, sys.monitoring.events.PY_START)
                remove = lambda: sys.monitoring.set_local_events(tool, code, 0)
            else:
                install = lambda: sys.monitoring.set_events(tool, sys.monitoring.events.PY_START)
                remove = lambda: sys.monitoring.set_events(tool, 0)
        install()
        try:
            assert function(Offset()) == [20, 21]
            assert calls == [("add", 0), ("add", 0)]
            if observer is not None:
                assert observed == (["call"] if observer in ("trace", "profile") else ["start"])
        finally:
            remove()
            if tool is not None:
                sys.monitoring.register_callback(tool, sys.monitoring.events.PY_START, None)
                sys.monitoring.free_tool_id(tool)
        assert sys.gettrace() is None and sys.getprofile() is None
        assert function(20) == [20, 21]

    for observer in ("trace", "profile", "monitor-local", "monitor-global"):
        exercise(stock.observed, observer)
        if not __dp_integration_soac__:
            exercise(module.observed, observer)
    exercise(module.observed)
''',
        ("observed",),
    )
