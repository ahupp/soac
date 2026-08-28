from __future__ import annotations

import ctypes
import hashlib
import json
import textwrap
from pathlib import Path

import pytest

from tests._integration import stock_module
from tests._strict_integration import create_strict_project


def _replace_global(module: object, mutation: str) -> None:
    if mutation == "module_attribute":
        module.target = module.replacement
    elif mutation == "module_dictionary":
        module.__dict__["target"] = module.replacement
    elif mutation == "function_globals":
        module.call.__globals__["target"] = module.replacement
    elif mutation == "exec":
        exec("target = replacement", module.__dict__)  # noqa: S102 - mutation seam under test
    elif mutation == "c_api":
        set_item = ctypes.pythonapi.PyDict_SetItem
        set_item.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
        set_item.restype = ctypes.c_int
        assert set_item(module.__dict__, "target", module.replacement) == 0
    else:
        raise AssertionError(f"unexpected mutation path: {mutation}")


def _observe_global_replacement(module: object, mutation: str) -> tuple[int, int]:
    before = module.call(2)
    _replace_global(module, mutation)
    return before, module.call(2)


def _observe_late_builtin_shadow(module: object) -> tuple[int, int]:
    before = module.call([1, 2, 3])
    module.__dict__["len"] = lambda value: 41
    return before, module.call([1, 2, 3])


def _observe_captured_builtin_mutation(module: object) -> tuple[int, int]:
    before = module.call([1, 2, 3])
    module.call.__builtins__["len"] = lambda value: 52
    return before, module.call([1, 2, 3])


# The fixture preserves all 19 original sources and ordinary validators.
# Only the five reviewed post-seal binding replacements differ under strict mode.
_REVIEWED_PRECONDITION_CASES = json.loads(
    (
        Path(__file__).parent / "fixtures/strict_module_precondition_cases.json"
    ).read_text()
)
_POST_SEAL_GLOBAL_REPLACEMENTS = {
    "unsealed_module_global_replacement_matches_cpython_module_attribute": "module_attribute",
    "unsealed_module_global_replacement_matches_cpython_module_dictionary": "module_dictionary",
    "unsealed_module_global_replacement_matches_cpython_function_globals": "function_globals",
    "unsealed_module_global_replacement_matches_cpython_exec": "exec",
    "unsealed_module_global_replacement_matches_cpython_c_api": "c_api",
}
_NATIVE_PARITY_PRECONDITIONS = frozenset(
    {
        "late_module_global_shadows_builtin_like_cpython",
        "named_builtin_uses_its_live_captured_mapping_like_cpython",
        "globals_builtin_returns_its_own_module_dictionary_like_cpython",
        "prebound_globals_name_calls_the_existing_module_binding_like_cpython",
        "known_builtin_uses_its_initial_captured_mapping_like_cpython",
        "scalar_builtin_argument_preserves_big_integer_intermediates_add",
        "scalar_builtin_argument_preserves_big_integer_intermediates_subtract",
        "scalar_builtin_argument_preserves_big_integer_intermediates_multiply",
        "dynamic_scalar_builtin_argument_preserves_big_integer_intermediates_add",
        "dynamic_scalar_builtin_argument_preserves_big_integer_intermediates_subtract",
        "dynamic_scalar_builtin_argument_preserves_big_integer_intermediates_multiply",
        "loop_carried_integer_arithmetic_preserves_big_integer_results_add",
        "loop_carried_integer_arithmetic_preserves_big_integer_results_subtract",
        "loop_carried_integer_arithmetic_preserves_big_integer_results_multiply",
    }
)


def _reviewed_precondition_case(name):
    assert set(_REVIEWED_PRECONDITION_CASES) == (
        set(_POST_SEAL_GLOBAL_REPLACEMENTS) | _NATIVE_PARITY_PRECONDITIONS
    ), "new precondition cases require an explicit compatibility review"
    case = _REVIEWED_PRECONDITION_CASES[name]
    assert hashlib.sha256(case["source"].encode()).hexdigest() == case["source_sha256"]
    return case


def _assert_ordinary_precondition_module(module, functions):
    from soac import _soac_ext

    assert _soac_ext.strict_module_diagnostics(module) is None
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    for name in functions:
        function = vars(module)[name]
        assert not owner(function), f"{name} unexpectedly acquired strict authority"
        assert not metadata(function), f"{name} is not an ordinary native control"


@pytest.mark.parametrize("name", _REVIEWED_PRECONDITION_CASES)
def test_reviewed_preconditions_preserve_ordinary_behavior(tmp_path, name):
    case = _reviewed_precondition_case(name)
    with (
        stock_module(tmp_path, "stock_" + name, case["source"]) as stock,
        stock_module(tmp_path, "ordinary_" + name, case["source"]) as module,
    ):
        for ordinary in (stock, module):
            _assert_ordinary_precondition_module(ordinary, case["required_functions"])
        namespace = {
            "stock": stock,
            "module": module,
            "pytest": pytest,
            "_observe_global_replacement": _observe_global_replacement,
            "_observe_late_builtin_shadow": _observe_late_builtin_shadow,
            "_observe_captured_builtin_mutation": _observe_captured_builtin_mutation,
        }
        exec(  # noqa: S102 - retained ordinary validator, never analyzed source
            compile(case["validation"], __file__, "exec", dont_inherit=True),
            namespace,
        )


@pytest.mark.parametrize(
    ("backend", "entry_interpreter", "opt_mode"),
    [
        pytest.param("soac", False, "none", id="compiled"),
        pytest.param("soac", False, "profile", id="profile"),
        pytest.param("soac", True, "none", id="entry"),
        pytest.param("cpython", False, "none", id="cpython"),
    ],
)
def test_late_builtin_bindings_preserve_lookup_order_and_actual_results(
    tmp_path, backend, entry_interpreter, opt_mode,
):
    source = """
def consume_any(values):
    return any(value for value in values)

def consume_all(values):
    return all(value for value in values)

def size(values):
    return len(values)
"""
    project = create_strict_project(
        tmp_path,
        {
            "late_builtins.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + source,
            "ordinary_late_builtins.py": source,
        },
        modules={"late_builtins": "late_builtins.py"},
        backend=backend,
    )
    project.run_case(
        "late_builtins",
        """
def validate(module):
    import ctypes
    import pytest
    import ordinary_late_builtins as ordinary
    from soac.strict import StrictMutationError
    from tests.test_strict_module_preconditions import _assert_ordinary_precondition_module

    _assert_ordinary_precondition_module(ordinary, ("consume_any", "consume_all", "size"))
    set_item = ctypes.pythonapi.PyDict_SetItemString
    set_item.argtypes = [ctypes.py_object, ctypes.c_char_p, ctypes.py_object]
    set_item.restype = ctypes.c_int

    for target in (ordinary, module):
        for _ in range(64):
            assert target.consume_any((0, 1)) is True
            assert target.consume_all((1, 0)) is False
            assert target.size((1, 2, 3)) == 3

        events = []
        any_result, all_result, len_result = object(), object(), object()

        def replacement_any(values):
            events.append(("any", tuple(values)))
            return any_result

        class AppendDuringIteration:
            def __iter__(self):
                events.append(("append",))
                target.any = replacement_any
                return iter((0, 0))

        # Callee lookup precedes evaluating the generator's outer iterator.
        # The current call keeps that callee; only later calls see the append.
        assert target.consume_any(AppendDuringIteration()) is False
        assert events == [("append",)], events
        assert target.consume_any((0, 1)) is any_result
        assert events == [("append",), ("any", (0, 1))], events

        def replacement_all(values):
            events.append(("all", tuple(values)))
            return all_result

        target.__dict__["all"] = replacement_all
        assert target.consume_all((1, 1)) is all_result
        assert events[-1] == ("all", (1, 1)), events
        assert set_item(target.__dict__, b"len", lambda values: len_result) == 0
        assert target.size(()) is len_result

        # The first bindings are legal; they do not revoke module finality.
        if target is module:
            with pytest.raises(StrictMutationError):
                del target.any
            with pytest.raises(StrictMutationError):
                target.__dict__["all"] = all
            assert target.any is replacement_any
            assert target.__dict__["all"] is replacement_all
""",
        Path(__file__),
        entry_interpreter=entry_interpreter,
        opt_mode=opt_mode,
        required_functions=("consume_any", "consume_all", "size"),
    )


# These controls select the actual native backend and checker publication. The
# ordinary files are byte-identical subjects except for the strict opt-in; the
# probe owns only deliberately escaped values and weak collection witnesses.
_MODULE_GLOBALS_SOURCE = """
from module_globals_probe import make_witness, observe

offset = 17
witness = make_witness(__name__)

def checked(value: int) -> int:
    return value + offset

observe(globals())
"""

_MODULE_GLOBALS_FAILURE_SOURCE = """
from module_globals_probe import abandon, make_witness

offset = 17
witness = make_witness(__name__)

def checked(value: int) -> int:
    return value + offset

abandon(globals(), checked)
"""

_MODULE_GLOBALS_PROBE = """
import ctypes
import gc
import sys
import weakref

events = []
observations = {}
witnesses = {}
escaped = {}
failure = RuntimeError("unconfigured initialization failure")
clear_during_import = False

class Witness:
    def __init__(self, label):
        self.label = label

    def __del__(self):
        events.append(self.label)

def make_witness(label):
    value = Witness(label)
    witnesses[label] = weakref.ref(value)
    return value

def observe(namespace):
    # Do not retain the actual dictionary in an observer or diagnostic record.
    name = namespace["__name__"]
    observations[name] = (
        sys.getrefcount(namespace),
        sum(item is namespace for item in gc.get_referents(sys.modules[name])),
    )

def native_module_clear(module):
    get_slot = ctypes.pythonapi.PyType_GetSlot
    get_slot.argtypes = [ctypes.py_object, ctypes.c_int]
    get_slot.restype = ctypes.c_void_p
    address = get_slot(type(module), 51)  # Py_tp_clear, stable typeslots.h API.
    assert address
    clear = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.py_object)(address)
    assert clear(module) == 0

def abandon(namespace, function):
    from soac import _soac_ext
    name = namespace["__name__"]
    module = sys.modules[name]
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    assert function(7) == 24
    escaped[name] = (
        namespace, function, weakref.ref(module), diagnostic,
        _soac_ext.strict_function_diagnostics(function),
    )
    if clear_during_import:
        native_module_clear(module)
    raise failure
"""


@pytest.fixture(scope="module")
def cpython_module_globals_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("cpython-module-globals"),
        {
            "native_globals.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _MODULE_GLOBALS_SOURCE,
            "ordinary_globals.py": _MODULE_GLOBALS_SOURCE,
            "native_globals_failure.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + _MODULE_GLOBALS_FAILURE_SOURCE,
            "ordinary_globals_failure.py": _MODULE_GLOBALS_FAILURE_SOURCE,
            "module_globals_probe.py": _MODULE_GLOBALS_PROBE,
        },
        modules={
            "native_globals": "native_globals.py",
            "native_globals_failure": "native_globals_failure.py",
        },
        backend="cpython",
    )


def _module_globals_witness_program(project):
    path = project.project / "native_globals.py"
    return textwrap.dedent(f"""
        import gc
        import importlib
        import sys
        import weakref
        sys.path.insert(0, {str(Path(__file__).resolve().parents[1])!r})
        import module_globals_probe as probe
        from soac import _soac_ext
        from tests._strict_integration import (
            _assert_cpython_function_witness, _assert_cpython_module_witness,
        )

        def assert_native_module(module):
            diagnostic = _assert_cpython_module_witness(
                module, module_name="native_globals",
                source_path={str(path)!r},
                source_sha256={hashlib.sha256(path.read_bytes()).hexdigest()!r},
                artifact_generation={project.publication["generation"]!r},
            )
            _assert_cpython_function_witness(
                module.checked, diagnostic,
            )
            return diagnostic
    """)


def test_cpython_module_globals_have_one_module_edge_and_ordinary_refcounts(
    cpython_module_globals_project,
):
    project = cpython_module_globals_project
    project.run(
        _module_globals_witness_program(project)
        + """
native = importlib.import_module("native_globals")
ordinary = importlib.import_module("ordinary_globals")
assert_native_module(native)
assert _soac_ext.strict_module_diagnostics(ordinary) is None
assert native.checked(7) == ordinary.checked(7) == 24
try:
    native.checked("wrong")
except TypeError:
    pass
else:
    raise AssertionError("native globals fixture lost its original addition error")

def observations(module):
    namespace = module.__dict__
    references = sys.getrefcount(namespace)
    edges = sum(item is namespace for item in gc.get_referents(module))
    return references, edges

expected = observations(ordinary)
actual = observations(native)
assert expected[1] == 1, expected
assert actual[1] == 1, actual
assert actual == expected, (actual, expected)
# While a body runs, stock importlib's argument tuple and builtin exec's
# globals/locals own three additional transient references. They are not
# module-owned edges and this loader uses the native evaluator directly.
# Compare module ownership during initialization, and exact raw refcounts
# above after both distinct loader call stacks have returned.
assert probe.observations["native_globals"][1] == probe.observations["ordinary_globals"][1] == 1, probe.observations
assert_native_module(native)
"""
    )


@pytest.mark.parametrize("clear_first", [False, True], ids=["wrapper-release", "native-clear"])
def test_cpython_module_globals_outlive_wrapper_then_collect_without_hidden_roots(
    cpython_module_globals_project, clear_first,
):
    project = cpython_module_globals_project
    project.run(
        _module_globals_witness_program(project)
        + f"\nclear_first = {clear_first!r}\n"
        + """
from soac.strict import StrictMutationError, StrictRuntimeUnavailableError

def retire(name, strict):
    module = importlib.import_module(name)
    if strict:
        assert_native_module(module)
    else:
        assert _soac_ext.strict_module_diagnostics(module) is None
    function = module.checked
    namespace = module.__dict__
    function_witness = weakref.ref(function)
    payload_witness = probe.witnesses[name]
    callbacks = []

    def wrapper_gone(reference):
        # Ordinary weakref ordering precedes native module-state release. The
        # callback may use escaped globals without resurrecting the wrapper.
        callbacks.append((reference() is None, function(7), namespace is function.__globals__))

    module_witness = weakref.ref(module, wrapper_gone)
    sys.modules.pop(name)
    if clear_first:
        probe.native_module_clear(module)
        if strict:
            try:
                _soac_ext.strict_module_diagnostics(module)
            except StrictRuntimeUnavailableError:
                pass
            else:
                raise AssertionError("native clear left initialized module state")
    del module
    assert module_witness() is None
    assert callbacks == [(True, 24, True)], callbacks
    assert function(7) == 24
    namespace["late_after_wrapper"] = 23
    assert namespace["late_after_wrapper"] == 23
    if strict:
        try:
            namespace["offset"] = 19
        except StrictMutationError:
            pass
        else:
            raise AssertionError("wrapper retirement revoked the permanent namespace policy")
        try:
            function("wrong")
        except TypeError:
            pass
        else:
            raise AssertionError("escaped source function lost its original addition error")
        assert function(7) == 24
    else:
        namespace["offset"] = 19
        assert function(7) == 26
    assert payload_witness() is not None
    del namespace, function
    gc.collect()
    assert function_witness() is None
    assert payload_witness() is None, "released module/global/function cycle stayed rooted"
    assert probe.events.count(name) == 1, probe.events

retire("ordinary_globals", False)
retire("native_globals", True)
"""
    )


@pytest.mark.parametrize("clear_first", [False, True], ids=["body-error", "clear-before-error"])
def test_cpython_module_globals_failure_terminalizes_without_retaining_dictionary(
    cpython_module_globals_project, clear_first,
):
    project = cpython_module_globals_project
    project.run(
        _module_globals_witness_program(project)
        + f"\nprobe.clear_during_import = {clear_first!r}\n"
        + """
from soac.strict import StrictRuntimeUnavailableError

def fail_import(name, strict):
    probe.failure = RuntimeError("original initialization failure")
    try:
        importlib.import_module(name)
    except RuntimeError as error:
        assert error is probe.failure
        assert error.__context__ is None
    else:
        raise AssertionError("failing initializer returned")
    namespace, function, module_witness, diagnostic, function_diagnostic = probe.escaped.pop(name)
    function_witness = weakref.ref(function)
    payload_witness = probe.witnesses[name]
    if strict:
        assert diagnostic["backend"] == "cpython"
        assert diagnostic["sealed"] is False
        assert diagnostic["original_code_entered"] is True
        assert function_diagnostic["backend"] == "cpython"
        assert function_diagnostic["original_code_entered"] is True
    else:
        assert diagnostic is None and function_diagnostic is None

    # Release the intentionally retained exception, not its traceback links.
    # GC must collect the native/ordinary unwind cycle without manual clearing.
    probe.failure = None
    gc.collect()
    assert name not in sys.modules
    assert module_witness() is None
    if strict:
        try:
            function(7)
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError("failed native module still granted source execution")
        try:
            namespace["after_failure"] = 1
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError("failed native module kept a mutable namespace")
    else:
        assert function(7) == 24
        namespace["after_failure"] = 1
    del namespace, function
    gc.collect()
    assert function_witness() is None
    assert payload_witness() is None
    assert probe.events.count(name) == 1, probe.events

fail_import("ordinary_globals_failure", False)
fail_import("native_globals_failure", True)
"""
    )


# The source-level global directive is part of the checker input, not a
# hand-installed namespace policy. The ordinary control uses the same body.
_C_API_MODULE_SOURCE = """
fixed = 7
counter = 0

def update(value: int) -> int:
    global counter
    counter = value
    return counter

def size(value) -> int:
    return len(value)
"""
