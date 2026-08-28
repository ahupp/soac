import ctypes
import gc
import json
import textwrap
import types
from pathlib import Path

import pytest
from soac import runtime

from tests._integration import stock_module
from tests._strict_integration import StrictValidationCase, create_strict_project


class _BoolRaisesStopIteration:
    def __bool__(self):
        raise StopIteration("truth")


class _RecordingIterable:
    def __init__(self, events, values):
        self.events = events
        self.values = values

    def __iter__(self):
        self.events.append("iter")
        return iter(self.values)


class _SetKey:
    def __init__(self, value, events, fail_mode):
        self.value = value
        self.events = events
        self.fail_mode = fail_mode

    def __hash__(self):
        self.events.append(("hash", self.value))
        if self.fail_mode == "hash" and self.value == 2:
            raise StopIteration("hash")
        return 0

    def __eq__(self, other):
        self.events.append(("eq", self.value, other.value))
        if self.fail_mode == "eq" and other.value == 2:
            raise StopIteration("eq")
        return self.value == other.value


def test_map_from_iter_eagerly_gets_iterator_and_stops_on_callback_stop_iteration():
    events = []
    iterable = _RecordingIterable(events, [0, 1, 2, 3])

    def convert(value):
        events.append(("map", value))
        if value == 2:
            raise StopIteration("map")
        return value + 10

    mapped = runtime.map_from_iter(convert, iterable)
    assert events == ["iter"]
    assert list(mapped) == [10, 11]
    assert events == ["iter", ("map", 0), ("map", 1), ("map", 2)]


@pytest.mark.parametrize("function", [None, lambda _value: _BoolRaisesStopIteration()])
def test_filter_from_iter_stops_on_truth_stop_iteration(function):
    values = [_BoolRaisesStopIteration()] if function is None else [1]
    assert list(runtime.filter_from_iter(function, values)) == []



# Source bodies and ordinary validators are retained from the original tests.
# Only their obsolete loader contexts were replaced with supplied real modules.
_REVIEWED_PIPELINE_CASES = json.loads(
    (Path(__file__).parent / "fixtures/strict_closed_pipeline_cases.json").read_text()
)
_FROZEN_PIPELINE_CASE = "named_generator_pipeline_preserves_code_and_default_mutation"


def _assert_ordinary_pipeline_module(module, functions):
    from soac import _soac_ext

    assert _soac_ext.strict_module_diagnostics(module) is None
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    for name in functions:
        assert not owner(getattr(module, name))


@pytest.mark.parametrize("name", _REVIEWED_PIPELINE_CASES)
def test_reviewed_closed_pipeline_keeps_ordinary_behavior(tmp_path, name):
    case = _REVIEWED_PIPELINE_CASES[name]
    with (
        stock_module(tmp_path, name + "_stock", case["source"]) as stock,
        stock_module(tmp_path, name + "_ordinary", case["source"]) as module,
    ):
        functions = (*case["required_functions"], *case["factory_functions"])
        _assert_ordinary_pipeline_module(stock, functions)
        _assert_ordinary_pipeline_module(module, functions)
        namespace = {
            "stock": stock,
            "module": module,
            "pytest": pytest,
            "types": types,
            "_BoolRaisesStopIteration": _BoolRaisesStopIteration,
            "_RecordingIterable": _RecordingIterable,
            "_SetKey": _SetKey,
        }
        # This is retained ordinary validation code, never analyzed source.
        exec(compile(case["validation"], __file__, "exec", dont_inherit=True), namespace)  # noqa: S102


@pytest.fixture(scope="module")
def strict_reviewed_pipeline_project(tmp_path_factory, request):
    sources, modules = {}, {}
    for name, case in _REVIEWED_PIPELINE_CASES.items():
        relative = name + ".py"
        sources[relative] = "# soac: module(strict_assign=true, checked_attr=true)\n" + case["source"]
        sources["ordinary_" + relative] = case["source"]
        modules[name] = relative
    return create_strict_project(
        tmp_path_factory.mktemp("strict-reviewed-pipelines"),
        sources,
        modules=modules,
        analysis_timeout=600,
        backend=request.param,
    )


def _strict_pipeline_validation(name, case, *, backend):
    validation = case["validation"]
    if name == _FROZEN_PIPELINE_CASE:
        validation = """
def run(ordinary):
    before = ordinary.collect()
    ordinary.values.__defaults__ = (3,)
    after_defaults = ordinary.collect()
    ordinary.values.__code__ = ordinary.replacement.__code__
    after_code = ordinary.collect()
    return before, after_defaults, after_code

assert run(stock) == ([0, 1], [0, 1, 2], [100, 101, 102])
before_code = module.values.__code__
before_defaults = module.values.__defaults__
assert module.collect() == [0, 1]
with pytest.raises(StrictMutationError):
    module.values.__defaults__ = (3,)
with pytest.raises(StrictMutationError):
    module.values.__code__ = module.replacement.__code__
assert module.values.__code__ is before_code
assert module.values.__defaults__ is before_defaults
assert module.collect() == [0, 1]
"""
    factory_import = ""
    factory_witness = (
        "        assert _soac_ext.strict_function_entry_kind(function) == 'generator_factory'\n"
    )
    if backend == "cpython":
        factory_import = (
            "from tests._strict_integration import _assert_cpython_function_witness\n"
        )
        factory_witness = (
            "        _assert_cpython_function_witness(\n"
            "            function, _soac_ext.strict_module_diagnostics(module),\n"
            "        )\n"
        )
    return (
        "import ctypes\nimport importlib\nimport pytest\nimport types\n"
        "from soac import _soac_ext\nfrom soac.strict import StrictMutationError\n"
        + factory_import
        + "from tests.test_closed_iterator_pipeline import "
        "_BoolRaisesStopIteration, _RecordingIterable, _SetKey, _assert_ordinary_pipeline_module\n"
        "def validate_module(module):\n"
        + f"    stock = importlib.import_module({'ordinary_' + name!r})\n"
        + f"    _assert_ordinary_pipeline_module(stock, {tuple(case['required_functions'] + case['factory_functions'])!r})\n"
        + "    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner\n"
        + "    owner.argtypes = [ctypes.py_object]\n"
        + "    owner.restype = ctypes.c_void_p\n"
        + f"    for name in {tuple(case['factory_functions'])!r}:\n"
        + "        function = getattr(module, name)\n"
        + "        assert owner(function)\n"
        + factory_witness
        + textwrap.indent(textwrap.dedent(validation), "    ")
    )


@pytest.mark.parametrize(
    ("strict_reviewed_pipeline_project", "entry_interpreter"),
    [("cpython", False), ("soac", False), ("soac", True)],
    ids=["cpython", "compiled", "entry"],
    indirect=["strict_reviewed_pipeline_project"],
)
def test_reviewed_closed_pipelines_use_authenticated_entries(
    strict_reviewed_pipeline_project, entry_interpreter
):
    backend = strict_reviewed_pipeline_project.backend
    cases = {}
    for name, case in _REVIEWED_PIPELINE_CASES.items():
        functions = tuple(case["required_functions"])
        if backend == "cpython":
            # Native generators/coroutines retain original source-function
            # ownership, not the retained backend's generator-factory entry.
            functions += tuple(case["factory_functions"])
        cases[name] = StrictValidationCase(
            _strict_pipeline_validation(name, case, backend=backend),
            Path(__file__),
            required_functions=functions,
            
        )
    results = strict_reviewed_pipeline_project.run_cases(
        cases, entry_interpreter=entry_interpreter
    )
    assert all(error is None for error in results.values()), {
        name: error for name, error in results.items() if error is not None
    }


_INTRINSIC_PIPELINE_SOURCE = """
def mapped(callback):
    return list(map(callback, (value for value in range(5))))

def filtered(callback):
    return tuple(filter(callback, (value for value in range(5))))
"""

_NATIVE_ITERATOR_PIPELINE_SOURCE = """
def mapped_native(callback, limit=5):
    return list(map(callback, range(limit)))

def filtered_native(callback, values):
    return tuple(filter(callback, values))
"""

_PIPELINE_FUNCTIONS = ("mapped", "filtered", "mapped_native", "filtered_native")

_GUARDED_PIPELINE_SOURCE = """
def install(map_value, list_value):
    global map, list
    map = map_value
    list = list_value

def guarded_{case}(callback_factory, iterable_factory):
    return list(map(callback_factory(), iterable_factory()))
"""


def _native_iterator_guard_outcome(module, case):
    """Declare/write real source globals; never replace capsule or code pointers."""
    events = []

    class Iterable:
        def __iter__(self):
            events.append("iter")
            return iter(range(3))

    def callback(value):
        events.append(("callback", value))
        return value + 10

    def callback_factory():
        events.append("callback-expr")
        return callback

    def iterable_factory():
        events.append("iterable-expr")
        return Iterable()

    def replacement_stage(function, values):
        events.append("stage-call")
        return map(function, values)

    def replacement_materializer(values):
        events.append("materializer-call")
        return list(values)

    module.install(
        replacement_stage if case == "stage" else map,
        replacement_materializer if case == "materializer" else list,
    )
    result = getattr(module, f"guarded_{case}")(callback_factory, iterable_factory)
    assert type(result) is list
    return result, events


def _native_iterator_callable_class_outcome(module, *, native_schedule=False):
    import weakref

    events = []
    references, callbacks = [], []

    class Item:
        def __init__(self, value):
            self.value = value
            events.append(("init", value))
            references.append(weakref.ref(self, lambda _: callbacks.append(value)))
            if value == 2:
                raise StopIteration("constructor")

        def __del__(self):
            events.append(("drop", self.value))

    metadata = _pipeline_function_pointer_getter("PyFunction_GetSoacMetadata")
    assert not metadata(Item.__init__) and not metadata(Item.__del__)
    result = module.mapped_native(Item)
    assert type(result) is list
    values = tuple(item.value for item in result)
    assert values == (0, 1), values
    events.append(("result", values))
    result.clear()
    if not native_schedule:
        gc.collect()
        assert len(references) == 3
        assert all(reference() is None for reference in references)
        assert sorted(
            event[1] for event in events if event[0] == "drop"
        ) == [0, 1, 2], events
        assert sorted(callbacks) == [0, 1, 2], callbacks
        # Initializers and the partial result remain ordered. Implicit
        # destruction may occur at a different safe point in SOAC execution.
        return values, [event for event in events if event[0] != "drop"]
    return values, events


def _native_iterator_constructor_refs_outcome(module, *, native_schedule=False):
    import sys
    import weakref

    events = []
    references = {}
    callbacks = []

    def observe(label):
        callback = references["callback"]()
        assert callback is not None
        events.append((label, sys.getrefcount(callback)))

    class Callback:
        def __call__(self, value):
            assert references["callback"]() is self
            events.append(("callback", value))
            return value

        def __del__(self):
            events.append(("callback-drop",))

    class Iterable:
        def __iter__(self):
            if native_schedule:
                observe("iter-refcount")
            else:
                assert references["callback"]() is not None
                events.append(("iter",))
            return iter((0,))

        def __del__(self):
            if native_schedule:
                observe("operand-cleanup-refcount")
            else:
                # A weakly observed callback need not outlive this unrelated
                # implicit destructor. Both objects must still retire once.
                events.append(("operand-drop",))

    def callback_factory():
        callback = Callback()
        references["callback"] = weakref.ref(
            callback, lambda _: callbacks.append("callback")
        )
        return callback

    def iterable_factory():
        iterable = Iterable()
        references["iterable"] = weakref.ref(
            iterable, lambda _: callbacks.append("iterable")
        )
        return iterable

    module.install(map, list)
    result = module.guarded_canonical(callback_factory, iterable_factory)
    assert type(result) is list and result == [0]
    if native_schedule:
        assert references["callback"]() is None
    events.append(("returned",))
    if not native_schedule:
        gc.collect()
        assert all(reference() is None for reference in references.values()), events
        assert events.count(("callback-drop",)) == 1, events
        assert events.count(("operand-drop",)) == 1, events
        assert sorted(callbacks) == ["callback", "iterable"], callbacks
        return [
            event for event in events
            if event[0] not in {"callback-drop", "operand-drop"}
        ]
    return events


def _native_iterator_next_slot_outcome(module):
    from itertools import count

    events = []

    class Original(count):
        pass

    class Changed(count):
        def __next__(self):
            value = count.__next__(self)
            events.append(("new-next", value))
            return value + 100

    get_slot = ctypes.pythonapi.PyType_GetSlot
    get_slot.argtypes = [ctypes.py_object, ctypes.c_int]
    get_slot.restype = ctypes.c_void_p
    # Stable-ABI Py_tp_iternext. Unlike replacing a Python __next__ method,
    # these types really have different C next slots: native count vs wrapper.
    assert get_slot(Original, 63) != get_slot(Changed, 63)
    iterator = Original()
    calls = 0

    def callback(value):
        nonlocal calls
        calls += 1
        events.append(("callback", value))
        if calls == 1:
            iterator.__class__ = Changed
            return False
        if calls == 4:
            raise StopIteration("bounded fourth callback")
        return True

    result = module.filtered_native(callback, iterator)
    assert type(result) is tuple
    return result, events


@pytest.mark.parametrize("case", ["stage", "materializer"])
def test_native_iterator_guard_ordinary_control(tmp_path, case):
    with stock_module(
        tmp_path, "ordinary_guard", _GUARDED_PIPELINE_SOURCE.format(case=case)
    ) as module:
        result, events = _native_iterator_guard_outcome(module, case)
    assert result == [10, 11, 12]
    middle = (
        ["stage-call", "iter"] if case == "stage" else ["iter", "materializer-call"]
    )
    assert events == [
        "callback-expr",
        "iterable-expr",
        *middle,
        ("callback", 0),
        ("callback", 1),
        ("callback", 2),
    ]


def test_native_iterator_callable_class_ordinary_control(tmp_path):
    with stock_module(
        tmp_path, "ordinary_class_pipeline", _NATIVE_ITERATOR_PIPELINE_SOURCE
    ) as module:
        result, events = _native_iterator_callable_class_outcome(module, native_schedule=True)
    assert result == (0, 1)
    assert events == [
        ("init", 0),
        ("init", 1),
        ("init", 2),
        ("drop", 2),
        ("result", (0, 1)),
        ("drop", 1),
        ("drop", 0),
    ]


def test_native_iterator_constructor_refs_ordinary_control(tmp_path):
    with stock_module(
        tmp_path,
        "ordinary_constructor_refs",
        _GUARDED_PIPELINE_SOURCE.format(case="canonical"),
    ) as module:
        events = _native_iterator_constructor_refs_outcome(module, native_schedule=True)
    assert events[0][0] == "iter-refcount"
    assert events[1] == ("operand-cleanup-refcount", events[0][1] + 1)
    assert events[2:] == [("callback", 0), ("callback-drop",), ("returned",)]


def test_native_iterator_next_slot_ordinary_control(tmp_path):
    with stock_module(
        tmp_path, "ordinary_slot_pipeline", _NATIVE_ITERATOR_PIPELINE_SOURCE
    ) as module:
        result, events = _native_iterator_next_slot_outcome(module)
    assert result == (1, 102)
    assert events == [
        ("callback", 0),
        ("callback", 1),
        ("new-next", 2),
        ("callback", 102),
        ("new-next", 3),
        ("callback", 103),
    ]


def _pipeline_function_pointer_getter(symbol):
    getter = getattr(ctypes.pythonapi, symbol)
    getter.argtypes = [ctypes.py_object]
    getter.restype = ctypes.c_void_p
    return getter


def _intrinsic_pipeline_outcome(module, name, failure, observe=None, *, count=5):
    """Run ordinary callbacks and truth operations, not copied strict helpers."""
    events = []
    is_map = name in ("mapped", "mapped_native")

    class Truth:
        def __init__(self, value):
            self.value = value

        def __bool__(self):
            events.append(("truth", self.value))
            if self.value == 2 and failure == "truth_stop":
                raise StopIteration("truth")
            if self.value == 2 and failure == "truth_error":
                raise ValueError("truth")
            return self.value % 2 == 0

    def callback(value):
        events.append((name, value))
        if observe is not None and value == 0:
            observe()
        if value == 2 and failure == "callback_stop":
            raise StopIteration(name)
        if value == 2 and failure == "callback_error":
            raise ValueError(name)
        return value * 3 + 1 if is_map else Truth(value)

    metadata = _pipeline_function_pointer_getter("PyFunction_GetSoacMetadata")
    owner = _pipeline_function_pointer_getter("PyFunction_GetSoacStrictOwner")
    for function in (callback, Truth.__init__, Truth.__bool__):
        assert not metadata(function) and not owner(function)
    try:
        if name == "filtered_native":
            result = getattr(module, name)(callback, range(count))
        elif name == "mapped_native":
            result = getattr(module, name)(callback, count)
        else:
            result = getattr(module, name)(callback)
    except ValueError as error:
        return None, (type(error).__name__, str(error)), events
    assert type(result) is (list if is_map else tuple)
    return result, None, events


def _active_pipeline_source_capsule(function):
    """Read the real managed-generator owner retained by a native-input plan.

    This is not permission to execute an arbitrary capsule. The original code
    selects the actual generator, and the native API checks its owner identity.
    This is generator ownership, not a reconstructed CPython frame or locals
    observation.
    """
    from soac import _soac_ext

    (code,) = (
        value
        for value in function.__code__.co_consts
        if type(value) is types.CodeType and value.co_name == "<genexpr>"
    )
    generators = [
        value
        for value in gc.get_objects()
        if type(value) is types.GeneratorType and value.gi_code is code
    ]
    assert len(generators) == 1, (function.__qualname__, generators)
    generator = generators[0]
    valid_capsule = ctypes.pythonapi.PyCapsule_IsValid
    valid_capsule.argtypes = [ctypes.py_object, ctypes.c_char_p]
    valid_capsule.restype = ctypes.c_int
    capsules = [
        value
        for value in gc.get_referents(generator)
        if valid_capsule(value, b"soac.PreservedState")
    ]
    assert len(capsules) == 1, function.__qualname__
    capsule = capsules[0]
    matches = ctypes.pythonapi.PyGen_MatchesSoacOwner
    matches.argtypes = [ctypes.py_object, ctypes.py_object]
    matches.restype = ctypes.c_int
    assert matches(generator, capsule) == 1

    metadata = _pipeline_function_pointer_getter("PyFunction_GetSoacMetadata")
    owner = _pipeline_function_pointer_getter("PyFunction_GetSoacStrictOwner")
    factories = {
        id(value): value
        for value in gc.get_referents(capsule)
        if type(value) is types.FunctionType
        and value.__globals__ is function.__globals__
        and metadata(value)
        and owner(value)
        and _soac_ext.strict_function_entry_kind(value) == "generator_factory"
    }
    assert len(factories) == 1, (function.__qualname__, factories)
    factory = next(iter(factories.values()))
    # Its synthetic execution code is not the original source code exposed by
    # gi_code. Actual creation provenance, not that code pointer, admits it.
    assert factory.__code__ is not code
    return {"native_owner_matches": True, "factory_qualname": factory.__qualname__}


@pytest.mark.parametrize("name", _PIPELINE_FUNCTIONS)
def test_intrinsic_pipeline_ordinary_callback_control(tmp_path, name):
    with stock_module(
        tmp_path,
        "ordinary_intrinsic_pipeline",
        _INTRINSIC_PIPELINE_SOURCE + _NATIVE_ITERATOR_PIPELINE_SOURCE,
    ) as module:
        _assert_ordinary_pipeline_module(module, _PIPELINE_FUNCTIONS)
        is_map = name in ("mapped", "mapped_native")
        value, error, events = _intrinsic_pipeline_outcome(module, name, "normal")
        assert error is None
        assert value == ([1, 4, 7, 10, 13] if is_map else (0, 2, 4))
        assert [event for event in events if event[0] == name] == [
            (name, value) for value in range(5)
        ]
        partial, error, events = _intrinsic_pipeline_outcome(
            module, name, "callback_stop"
        )
        assert error is None
        assert partial == ([1, 4] if is_map else (0,))
        assert events[-1] == (name, 2)
        assert _intrinsic_pipeline_outcome(module, name, "callback_error")[1] == (
            "ValueError",
            name,
        )
        if not is_map:
            assert _intrinsic_pipeline_outcome(module, name, "truth_stop")[:2] == (
                (0,),
                None,
            )
            assert _intrinsic_pipeline_outcome(module, name, "truth_error")[1] == (
                "ValueError",
                "truth",
            )


@pytest.fixture(scope="module")
def strict_intrinsic_pipeline_project(tmp_path_factory):
    sources = {
        "intrinsic_pipeline": _INTRINSIC_PIPELINE_SOURCE
        + _NATIVE_ITERATOR_PIPELINE_SOURCE,
        **{
            f"intrinsic_guard_{case}": _GUARDED_PIPELINE_SOURCE.format(case=case)
            for case in ("stage", "materializer", "canonical")
        },
    }
    files = {}
    for name, source in sources.items():
        files[f"{name}.py"] = "# soac: module(strict_assign=true, checked_attr=true)\n" + source
        files[f"ordinary_{name}.py"] = source
    return create_strict_project(
        tmp_path_factory.mktemp("strict-intrinsic-pipeline"),
        files,
        modules={name: f"{name}.py" for name in sources},
    )


def test_checked_native_iterator_imports_survive_reserved_codegen(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "native_imports.py": (
                "# soac: module(strict_assign=true, checked_attr=true)\n"
                "def collect(callback, values):\n"
                "    return list(map(callback, values))\n"
            )
        },
        modules={"native_imports": "native_imports.py"},
    )
    work = project.root / "counters"
    for mode in ("profile", "apply"):
        events = work / f"{mode}.jsonl"
        project.run(
            """
            import native_imports
            assert _soac_ext.strict_module_diagnostics(native_imports)['sealed']
            assert _soac_ext.strict_function_entry_kind(native_imports.collect) == 'checked_native'
            for _ in range(32):
                assert native_imports.collect(lambda value: value + 1, range(3)) == [1, 2, 3]
            """,
            opt_mode=mode,
            extra_env={
                "SOAC_WORK_DIR": str(work),
                "SOAC_LOG": f"soac_native_iterator_pipeline=info;json={events}",
            },
        )
        if mode == "apply":
            rows = [json.loads(line) for line in events.read_text().splitlines()]
            assert any(
                row.get("message") == "typed_native_iterator_pipeline_committed"
                and row.get("function_qualname") == "collect"
                and row.get("remaining_template_calls") == 0
                for row in rows
            )


def _check_native_iterator_pipeline(project, entry_interpreter, *, source_activations):
    names = (
        ("mapped", "filtered")
        if source_activations
        else ("mapped_native", "filtered_native")
    )
    fixture_kind = "source-activations" if source_activations else "native-iterators"
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    work = (
        project.root
        / fixture_kind
        / ("entry-counters" if entry_interpreter else "compiled-counters")
    )
    reports = {}
    missing = {}
    for mode in ("profile", "apply", "verify"):
        events_path = work / f"{mode}-planning.jsonl"
        program = f"""
import json, os, sys
sys.path.insert(0, {str(Path(__file__).resolve().parents[1])!r})
import intrinsic_pipeline as checked
import ordinary_intrinsic_pipeline as ordinary
from tests.test_closed_iterator_pipeline import (
    _active_pipeline_source_capsule, _assert_ordinary_pipeline_module,
    _intrinsic_pipeline_outcome, _pipeline_function_pointer_getter,
    _native_iterator_guard_outcome, _native_iterator_callable_class_outcome,
    _native_iterator_next_slot_outcome, _native_iterator_constructor_refs_outcome,
)
diagnostic = _soac_ext.strict_module_diagnostics(checked)
assert diagnostic is not None and diagnostic['sealed'] is True
assert diagnostic['artifact_generation'] == {project.publication["generation"]!r}
assert diagnostic['source_path'] == {str(project.project / "intrinsic_pipeline.py")!r}
assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
metadata = _pipeline_function_pointer_getter('PyFunction_GetSoacMetadata')
owner = _pipeline_function_pointer_getter('PyFunction_GetSoacStrictOwner')
_assert_ordinary_pipeline_module(ordinary, {names!r})
assert not metadata(ordinary.mapped) and not metadata(ordinary.filtered)
assert not metadata(_intrinsic_pipeline_outcome) and not owner(_intrinsic_pipeline_outcome)
capsules = {{}}
outcomes = {{}}
for name in {names!r}:
    function = getattr(checked, name)
    assert metadata(function) and owner(function)
    assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}
    failures = ['normal', 'callback_stop', 'callback_error']
    if name in ('filtered', 'filtered_native'):
        failures.extend(['truth_stop', 'truth_error'])
    for failure in failures:
        expected = _intrinsic_pipeline_outcome(ordinary, name, failure)
        def observe():
            capsules[name] = _active_pipeline_source_capsule(function)
        probe = observe if {source_activations!r} and failure == 'normal' else None
        actual = _intrinsic_pipeline_outcome(checked, name, failure, probe)
        assert actual == expected, (name, failure, actual, expected)
        if type(actual[0]) is list:
            assert actual[0].__sizeof__() == expected[0].__sizeof__(), (name, failure)
        outcomes[name + ':' + failure] = actual
    if not {source_activations!r}:
        for count in (0, 1, 2, 3, 4, 7, 8, 9, 17):
            expected = _intrinsic_pipeline_outcome(ordinary, name, 'normal', count=count)
            actual = _intrinsic_pipeline_outcome(checked, name, 'normal', count=count)
            assert actual == expected, (name, count, actual, expected)
            if type(actual[0]) is list:
                assert actual[0].__sizeof__() == expected[0].__sizeof__(), (name, count)
        # Native construction accepts a noncallable callback if no item reaches it.
        arguments = (object(), 0) if name == 'mapped_native' else (object(), ())
        assert function(*arguments) == getattr(ordinary, name)(*arguments)
    for _ in range(32):
        assert _intrinsic_pipeline_outcome(checked, name, 'normal') == (
            _intrinsic_pipeline_outcome(ordinary, name, 'normal')
        )
    assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}
if {source_activations!r}:
    assert set(capsules) == set({names!r}), capsules
else:
    assert not capsules
    for edge_case, observe in (
        ('callable-class', _native_iterator_callable_class_outcome),
        ('next-slot', _native_iterator_next_slot_outcome),
    ):
        expected = observe(ordinary)
        actual = observe(checked)
        assert actual == expected, (edge_case, actual, expected)
        outcomes[edge_case] = actual
    for case in ('stage', 'materializer', 'canonical'):
        guarded = __import__('intrinsic_guard_' + case)
        ordinary_guarded = __import__('ordinary_intrinsic_guard_' + case)
        entry_name = 'guarded_' + case
        _assert_ordinary_pipeline_module(ordinary_guarded, ('install', entry_name))
        for function_name in ('install', entry_name):
            function = getattr(guarded, function_name)
            assert metadata(function) and owner(function)
            assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}
        if case == 'canonical':
            expected = _native_iterator_constructor_refs_outcome(ordinary_guarded)
            actual = _native_iterator_constructor_refs_outcome(guarded)
        else:
            expected = _native_iterator_guard_outcome(ordinary_guarded, case)
            actual = _native_iterator_guard_outcome(guarded, case)
        assert actual == expected, (case, actual, expected)
        outcomes['guard:' + case] = actual
print(json.dumps({{'process_id': os.getpid(), 'capsules': capsules, 'outcomes': outcomes}}))
"""
        result = project.run(
            program,
            entry_interpreter=entry_interpreter,
            opt_mode=mode,
            extra_env={
                "SOAC_WORK_DIR": str(work),
                "SOAC_LOG": (
                    "soac_builtin_consumer_planning=debug,"
                    "soac_native_iterator_pipeline=info"
                    f";json={events_path}"
                ),
            },
        )
        observation = json.loads(result.stdout.splitlines()[-1])
        rows = [
            json.loads(line)
            for line in events_path.read_text().splitlines()
            if line.strip()
        ]
        reports[mode] = {"runtime": observation, "implementations": {}}
        if mode == "profile" or entry_interpreter or source_activations:
            continue
        for name, stage, materializer in (
            ("mapped_native", "Map", "List"),
            ("filtered_native", "Filter", "Tuple"),
            ("guarded_stage", "Map", "List"),
            ("guarded_materializer", "Map", "List"),
            ("guarded_canonical", "Map", "List"),
        ):
            completed = [
                row
                for row in rows
                if row.get("message") == "typed_native_iterator_pipeline_committed"
                and row.get("function_qualname") == name
                and row.get("stage") == stage
                and row.get("materializer") == materializer
            ]
            valid_bundles = [
                row
                for row in completed
                if row.get("canonical_guard_count") == 2
                and row.get("native_input_count") == 1
                and row.get("eliminated_wrapper_count") == 1
                and row.get("remaining_template_calls") == 0
                and row.get("eliminated_source_activations") == 0
            ]
            evidence = {
                "committed_bundles": len(completed),
                "validated_bundles": len(valid_bundles),
            }
            reports[mode]["implementations"][name] = evidence
            if not evidence["validated_bundles"]:
                missing[f"{mode}:{name}"] = evidence
    # Preserve both missing operations and both optimized modes, even when the
    # ordinary fallback produces correct results. Attempted bindings and generic
    # body compilation are not evidence of a completed native-iterator bundle.
    (work / "intrinsic-pipeline-evidence.json").write_text(
        json.dumps(reports, indent=2) + "\n"
    )
    if not entry_interpreter:
        from soac import _soac_ext

        counters = json.loads(
            _soac_ext.inspect_counter_dump_json(str(work / "profile.bin"))
        )
        for name in names:
            assert any(
                record["module_name"] == "intrinsic_pipeline"
                and row["function_qualname"] == name
                and row["value"] > 0
                for record in counters["records"]
                for row in record["rows"]
            ), (name, counters)
        assert not missing, missing


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_checked_native_iterator_pipeline_commits_complete_bundle(
    strict_intrinsic_pipeline_project, entry_interpreter
):
    _check_native_iterator_pipeline(
        strict_intrinsic_pipeline_project, entry_interpreter, source_activations=False
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_checked_map_filter_source_activations_remain_native(
    strict_intrinsic_pipeline_project, entry_interpreter
):
    # The b008 receipt's missing instance plan was not authority to erase the
    # genexpr. Both entries and optimized modes retain its actual native owner.
    _check_native_iterator_pipeline(
        strict_intrinsic_pipeline_project, entry_interpreter, source_activations=True
    )
