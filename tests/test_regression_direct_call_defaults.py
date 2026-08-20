from __future__ import annotations

import json
import textwrap

import pytest

from tests._integration import stock_module
from tests._strict_integration import (
    assert_strict_source_rejected,
    create_strict_project,
)

_WRONG_ARITY_SOURCE = """
def add(a, b):
    return a + b

def missing():
    return add(1)

def extra():
    return add(1, 2, 3)
"""

_DEFAULT_SOURCE = """
def callee(value, increment=5):
    return value + increment

def run():
    return callee(37)
"""


_CHECKED_FREE_CALL_SOURCE = """
from __future__ import strict
from checked_free_support import initialize

events = []

def checked(value: int, increment: int = 5) -> int:
    events.append(("body", value, increment))
    return value + increment

def run(value):
    return checked(value)

def forward(value: int) -> int:
    return checked(value)

def run_made(make):
    return checked(make())

def run_keyword(value):
    return checked(value, increment=9)

def run_star(arguments):
    return checked(*arguments)

def checked_result(callback, value: int) -> int:
    return callback(value)

def run_result(callback, value):
    return checked_result(callback, value)

def no_args() -> int:
    return 7

def call_no_args() -> int:
    return no_args()

def different(value: int, increment: int = 50) -> int:
    return increment - value

def make_family(default):
    def callee(value: int, increment: int = default) -> int:
        events.append(("family", value, increment))
        return value + increment
    def call(value):
        return callee(value)
    return callee, call

def make_callback_family(default):
    def callee(value: int, increment: int = default) -> int:
        return value + increment
    def call(make):
        return callee(make())
    return callee, call

initialize(checked, run, run_made)
"""

_CHECKED_FREE_SUPPORT_SOURCE = """
def initialize(function, invoke, invoke_made):
    function.__defaults__ = (8,)
    assert invoke(34) == 42

    def produce():
        function.__defaults__ = (11,)
        return 31

    assert invoke_made(produce) == 42
    function.__defaults__ = ("bad",)
    try:
        invoke(1)
    except TypeError:
        pass
    else:
        raise AssertionError("the original addition accepted an incompatible default")
    function.__defaults__ = (5,)
"""

_ARGUMENT_ERROR_SOURCE = """
def positional(first, second, third, fourth):
    return first, second, third, fourth

def keyword_only(*, first, second, third):
    return first, second, third

def defaulted(first, second=20, third=30, *, fourth, fifth=50):
    return first, second, third, fourth, fifth

def make_nested():
    def nested(first, second):
        return first, second
    return nested

def invoke(function, arguments, keywords):
    return function(*arguments, **keywords)
"""


def test_original_wrong_arity_source_is_a_checker_rejection_with_stock_control(
    tmp_path,
):
    # Known invalid calls are real checker negatives, not runtime-positive
    # strict source or suppressed diagnostics.
    diagnostics = assert_strict_source_rejected(
        tmp_path / "rejected",
        "from __future__ import strict\n" + textwrap.dedent(_WRONG_ARITY_SOURCE),
        module_name="direct_entry_wrong_arity_case",
        diagnostic="missing-argument",
    )
    assert "too-many-positional-arguments" in diagnostics
    with stock_module(
        tmp_path / "stock", "ordinary_wrong_arity", _WRONG_ARITY_SOURCE
    ) as module:
        with pytest.raises(TypeError, match="missing.*'b'"):
            module.missing()
        with pytest.raises(
            TypeError, match="takes 2 positional arguments but 3 were given"
        ):
            module.extra()


@pytest.fixture(scope="module")
def direct_default_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-direct-call-defaults"),
        {
            "direct_arity.py": """
from __future__ import strict

def add(a, b):
    return a + b

def invoke(function, arguments):
    return function(*arguments)
""",
            "direct_defaults.py": "from __future__ import strict\n"
            + textwrap.dedent(_DEFAULT_SOURCE),
            "ordinary_defaults.py": _DEFAULT_SOURCE,
            "ordinary_arity.py": _WRONG_ARITY_SOURCE,
            "argument_errors.py": "from __future__ import strict\n"
            + textwrap.dedent(_ARGUMENT_ERROR_SOURCE),
            "ordinary_argument_errors.py": _ARGUMENT_ERROR_SOURCE,
            "checked_free_defaults.py": _CHECKED_FREE_CALL_SOURCE,
            "ordinary_checked_free_defaults.py": _CHECKED_FREE_CALL_SOURCE.replace(
                "from __future__ import strict\n", "", 1
            ),
            "checked_free_support.py": _CHECKED_FREE_SUPPORT_SOURCE,
        },
        modules={
            "direct_arity": "direct_arity.py",
            "direct_defaults": "direct_defaults.py",
            "argument_errors": "argument_errors.py",
            "checked_free_defaults": "checked_free_defaults.py",
        },
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_vectorcall_wrong_arity_is_checked_before_direct_entry(
    direct_default_project, entry_interpreter
):
    direct_default_project.run_case(
        "direct_arity",
        textwrap.dedent("""
        def validate(module):
            import ordinary_arity

            for arguments in ((1,), (1, 2, 3)):
                errors = []
                for function in (ordinary_arity.add, module.add):
                    try:
                        module.invoke(function, arguments)
                    except TypeError as error:
                        errors.append(str(error))
                    else:
                        raise AssertionError("invalid argument count reached the body")
                assert errors[0] == errors[1], errors
            assert module.invoke(module.add, (20, 22)) == 42
        """),
        direct_default_project.project / "direct_arity.py",
        entry_interpreter=entry_interpreter,
        required_functions=("add", "invoke"),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_argument_binding_errors_match_native_counts_defaults_and_qualified_names(
    direct_default_project, entry_interpreter
):
    direct_default_project.run_case(
        "argument_errors",
        textwrap.dedent("""
        def validate(module):
            import ordinary_argument_errors as ordinary
            from soac import _soac_ext

            cases = [
                ("positional", (), {}),
                ("positional", (1,), {}),
                ("positional", (1, 2), {}),
                ("positional", (1, 2, 3), {}),
                ("positional", (), {"first": 1, "fourth": 4}),
                ("positional", (1, 2, 3, 4, 5), {}),
                ("keyword_only", (), {}),
                ("keyword_only", (), {"first": 1}),
                ("keyword_only", (), {"first": 1, "second": 2}),
                ("keyword_only", (1,), {}),
                ("keyword_only", (1,), {"first": 1}),
                ("defaulted", (), {"fourth": 4}),
                ("defaulted", (1,), {}),
                ("defaulted", (1, 2, 3, 4), {}),
                ("defaulted", (1, 2, 3, 4), {"fourth": 4}),
                ("defaulted", (1, 2, 3, 4), {"fourth": 4, "fifth": 5}),
            ]
            pairs = [
                (getattr(ordinary, name), getattr(module, name), arguments, keywords)
                for name, arguments, keywords in cases
            ]
            nested = module.make_nested()
            assert _soac_ext.strict_function_entry_kind(nested) == (
                _soac_ext.strict_function_entry_kind(module.invoke)
            )
            pairs.append((ordinary.make_nested(), nested, (), {}))
            for stock, strict, arguments, keywords in pairs:
                errors = []
                for function in (stock, strict):
                    try:
                        module.invoke(function, arguments, keywords)
                    except TypeError as error:
                        errors.append((type(error), str(error)))
                    else:
                        raise AssertionError("invalid binding reached the function body")
                assert errors[0] == errors[1], (strict.__qualname__, errors)
            assert module.invoke(module.defaulted, (1,), {"fourth": 4}) == (
                1, 20, 30, 4, 50
            )
        """),
        direct_default_project.project / "argument_errors.py",
        entry_interpreter=entry_interpreter,
        required_functions=(
            "positional",
            "keyword_only",
            "defaulted",
            "make_nested",
            "invoke",
        ),
    )


def test_apply_mode_direct_call_with_omitted_default_emits_direct_edge(
    direct_default_project,
):
    project = direct_default_project
    work_dir = project.root / "default-profile"
    log_path = project.root / "default-apply-events.jsonl"
    program = f"""
import direct_defaults as module
import ordinary_defaults
diagnostic = _soac_ext.strict_module_diagnostics(module)
assert diagnostic is not None and diagnostic['sealed']
assert diagnostic['artifact_generation'] == {project.publication["generation"]!r}
assert diagnostic['source_path'] == {str(project.project / "direct_defaults.py")!r}
assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
for name in ('callee', 'run'):
    assert _soac_ext.strict_function_entry_kind(vars(module)[name]) == 'checked_native'
assert module.run() == ordinary_defaults.run() == 42
for name in ('callee', 'run'):
    assert _soac_ext.strict_function_entry_kind(vars(module)[name]) == 'checked_native'
"""
    project.run(program, opt_mode="profile", extra_env={"SOAC_WORK_DIR": str(work_dir)})
    assert (work_dir / "profile.bin").is_file()
    project.run(
        program,
        opt_mode="apply",
        extra_env={
            "SOAC_WORK_DIR": str(work_dir),
            "SOAC_LOG": f"soac_jit_direct_edges=info;json={log_path}",
        },
    )
    rows = [
        json.loads(line) for line in log_path.read_text().splitlines() if line.strip()
    ]
    assert any(
        row.get("target") == "soac_jit_direct_edges"
        and row.get("clif_direct_edges", 0) > 0
        for row in rows
    ), rows


def test_omitted_default_preserves_entry_interpreter_behavior(direct_default_project):
    direct_default_project.run_case(
        "direct_defaults",
        """
def validate(module):
    import ordinary_defaults
    assert module.run() == ordinary_defaults.run() == 42
""",
        direct_default_project.project / "direct_defaults.py",
        entry_interpreter=True,
        required_functions=("callee", "run"),
    )


def _checked_free_call_validation(*, expected_entry, require_direct):
    return textwrap.dedent(f"""
    def validate(module):
        from soac import _soac_ext
        from soac.strict import StrictMutationError

        # Both initializer calls use the actually changed defaults, including
        # the change performed while an argument expression is evaluated.
        assert module.events == [("body", 34, 8), ("body", 31, 11), ("body", 1, "bad")]
        module.events.clear()
        assert module.run(37) == 42
        assert module.run_keyword(33) == 42
        assert module.run_star((40, 2)) == 42
        assert module.call_no_args() == 7

        before = _soac_ext.strict_function_call_statistics(module.checked)
        assert module.forward(37) == 42
        after = _soac_ext.strict_function_call_statistics(module.checked)
        if {require_direct!r}:
            assert after["direct_body_calls"] - before["direct_body_calls"] == 1
            assert after["fixed_body_calls"] - before["fixed_body_calls"] == 1

        count = len(module.events)
        operand = object()
        try:
            module.run(operand)
        except TypeError:
            pass
        else:
            raise AssertionError("the original addition accepted an incompatible operand")
        assert len(module.events) == count + 1
        assert module.events[-1] == ("body", operand, 5)
        count += 1

        marker = RuntimeError("argument callback")
        def failed_argument():
            raise marker
        try:
            module.run_made(failed_argument)
        except RuntimeError as error:
            assert error is marker
        else:
            raise AssertionError("argument failure was lost")
        assert len(module.events) == count
        marker.__traceback__ = None

        calls = []
        def wrong_result(value):
            calls.append(value)
            return "wrong"
        assert module.run_result(wrong_result, 7) == "wrong"
        assert calls == [7]

        first, first_call = module.make_family(3)
        second, second_call = module.make_family(20)
        assert first.__code__ is second.__code__
        assert first.__defaults__ == (3,) and second.__defaults__ == (20,)
        for function in (first, first_call, second, second_call):
            assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}
        assert first_call(10) == 13 and second_call(10) == 30
        assert first_call.__code__.co_freevars == ("callee",)
        cell = first_call.__closure__[0]
        assert cell.cell_contents is first

        # A sealed closure tuple does not freeze its cell's contents. Another
        # execution of the same source has its own defaults and environment.
        cell.cell_contents = second
        assert first_call(10) == 30
        cell.cell_contents = module.different
        assert first_call(10) == 40

        def ordinary(value):
            return "ordinary", value
        assert _soac_ext.strict_function_entry_kind(ordinary) is None
        cell.cell_contents = ordinary
        assert first_call(10) == ("ordinary", 10), "fallback did not call the actual ordinary callee"
        cell.cell_contents = first
        assert first_call(10) == 13

        try:
            first.__defaults__ = (99,)
        except StrictMutationError:
            pass
        else:
            raise AssertionError("sealed defaults were replaced")
        assert first.__defaults__ == (3,)

        bad, bad_call = module.make_family("bad")
        assert _soac_ext.strict_function_entry_kind(bad) == {expected_entry!r}
        count = len(module.events)
        try:
            bad_call(1)
        except TypeError:
            pass
        else:
            raise AssertionError("the original addition accepted an incompatible default")
        assert len(module.events) == count + 1
        assert module.events[-1] == ("family", 1, "bad")
    """)


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_checked_unbound_defaults_keep_actual_bindings_checks_and_callee_environments(
    direct_default_project, entry_interpreter
):
    direct_default_project.run_case(
        "checked_free_defaults",
        _checked_free_call_validation(
            expected_entry="entry_interpreter"
            if entry_interpreter
            else "checked_native",
            require_direct=False,
        ),
        direct_default_project.project / "checked_free_defaults.py",
        entry_interpreter=entry_interpreter,
        required_functions=(
            "checked",
            "run",
            "forward",
            "run_made",
            "run_keyword",
            "run_star",
            "checked_result",
            "run_result",
            "no_args",
            "call_no_args",
            "different",
            "make_family",
        ),
    )


def test_apply_checked_unbound_defaults_emit_direct_edges_without_skipping_default_checks(
    direct_default_project,
):
    project = direct_default_project
    work_dir = project.root / "checked-free-profile"
    events = project.root / "checked-free-apply-events.jsonl"
    header = f"""
import checked_free_defaults as module
diagnostic = _soac_ext.strict_module_diagnostics(module)
assert diagnostic is not None and diagnostic["sealed"]
assert diagnostic["artifact_generation"] == {project.publication["generation"]!r}
assert diagnostic["source_path"] == {str(project.project / "checked_free_defaults.py")!r}
assert diagnostic["initializer_entry_kind"] == "entry_interpreter"
for name in ("checked", "run", "forward", "run_made", "checked_result", "run_result", "no_args", "call_no_args", "different", "make_family"):
    assert _soac_ext.strict_function_entry_kind(vars(module)[name]) == "checked_native"
"""
    for mode in ("profile", "apply"):
        program = (
            header
            + _checked_free_call_validation(
                expected_entry="checked_native", require_direct=mode == "apply"
            )
            + "\nvalidate(module)\n"
        )
        environment = {"SOAC_WORK_DIR": str(work_dir)}
        if mode == "apply":
            environment["SOAC_LOG"] = f"soac_jit_direct_edges=info;json={events}"
        project.run(program, opt_mode=mode, extra_env=environment)
        assert (work_dir / "profile.bin").is_file()
    rows = [
        json.loads(line) for line in events.read_text().splitlines() if line.strip()
    ]
    assert any(
        row.get("target") == "soac_jit_direct_edges"
        and row.get("clif_direct_edges", 0) > 0
        for row in rows
    ), rows


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_checked_unbound_call_captures_before_argument_effects_and_releases_on_failure(
    direct_default_project, entry_interpreter
):
    project = direct_default_project
    work_dir = project.root / f"checked-capture-profile-{entry_interpreter}"
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    program = f"""
import checked_free_defaults as module
import ordinary_checked_free_defaults as ordinary
import ctypes
import gc
import weakref

diagnostic = _soac_ext.strict_module_diagnostics(module)
assert diagnostic is not None and diagnostic["sealed"]
assert diagnostic["artifact_generation"] == {project.publication["generation"]!r}
assert diagnostic["source_path"] == {str(project.project / "checked_free_defaults.py")!r}
assert diagnostic["initializer_entry_kind"] == "entry_interpreter"
for name in ("checked", "run_made", "make_callback_family"):
    assert _soac_ext.strict_function_entry_kind(vars(module)[name]) == {expected_entry!r}
assert _soac_ext.strict_function_entry_kind(ordinary.make_callback_family) is None

def ordinary_replacement(value):
    return "replacement", value

class Payload:
    pass

for source in (ordinary, module):
    callee, invoke = source.make_callback_family(5)
    assert invoke.__code__.co_freevars == ("callee",)
    cell = invoke.__closure__[0]
    assert cell.cell_contents is callee
    released = []
    callee_ref = weakref.ref(callee, lambda _: released.append("callee"))
    del callee
    timeline = []
    def replace_during_argument():
        timeline.append("argument")
        cell.cell_contents = ordinary_replacement
        assert callee_ref() is not None, "captured callee died inside its argument"
        return 37
    assert invoke(replace_during_argument) == 42
    assert timeline == ["argument"]
    gc.collect()
    assert callee_ref() is None and released == ["callee"]
    assert invoke(lambda: 9) == ("replacement", 9)

    for failure in ("argument", "binding"):
        payload = Payload()
        payload_ref = weakref.ref(payload)
        callee, invoke = source.make_callback_family(payload)
        cell = invoke.__closure__[0]
        callee_ref = weakref.ref(callee)
        del payload, callee
        marker = RuntimeError("argument failure")
        timeline = []
        def replace_then_fail():
            timeline.append("argument")
            cell.cell_contents = ordinary_replacement
            assert callee_ref() is not None and payload_ref() is not None
            if failure == "argument":
                raise marker
            return 37
        try:
            invoke(replace_then_fail)
        except RuntimeError as error:
            assert failure == "argument" and error is marker
        except TypeError:
            assert failure == "binding"
        else:
            raise AssertionError("captured callee/default failure was replayed as replacement")
        assert timeline == ["argument"]
        marker.__traceback__ = None
        gc.collect()
        gc.collect()
        assert callee_ref() is None, (source.__name__, failure, "callee")
        assert payload_ref() is None, (source.__name__, failure, "default")
        assert invoke(lambda: 9) == ("replacement", 9)

# The supported native vectorcall setter can change even a sealed function's
# public entry. The actual call must observe a change made by its argument,
# without evaluating that argument twice or bypassing the replacement entry.
get_vectorcall = ctypes.pythonapi.PyVectorcall_Function
get_vectorcall.argtypes = [ctypes.py_object]
get_vectorcall.restype = ctypes.c_void_p
set_vectorcall = ctypes.pythonapi.PyFunction_SetVectorcall
set_vectorcall.argtypes = [ctypes.py_object, ctypes.c_void_p]
set_vectorcall.restype = None
incref = ctypes.pythonapi.Py_IncRef
incref.argtypes = [ctypes.py_object]
incref.restype = None
signature = ctypes.PYFUNCTYPE(
    ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p),
    ctypes.c_size_t, ctypes.c_void_p,
)
mask = (1 << (8 * ctypes.sizeof(ctypes.c_size_t) - 1)) - 1
original_pointer = get_vectorcall(module.checked)
assert original_pointer
original = signature(original_pointer)
timeline = []
errors = []
safe_failure_result = object()

@signature
def replacement_entry(actual, arguments, nargsf, kwnames):
    # This is a new-reference C ABI, not a py_object callback return. Record
    # diagnostics outside the callback so ctypes never swallows a test error.
    try:
        timeline.append(("entry", actual, nargsf & mask, kwnames))
        result = original(actual, arguments, nargsf, kwnames)
        if result:
            return result
        errors.append("original entry returned NULL")
    except BaseException as error:
        errors.append((type(error).__name__, str(error)))
    incref(safe_failure_result)
    return id(safe_failure_result)

replacement_pointer = ctypes.cast(replacement_entry, ctypes.c_void_p).value
def change_entry():
    timeline.append("argument")
    set_vectorcall(module.checked, replacement_pointer)
    return 37

try:
    assert module.run_made(change_entry) == 42
finally:
    set_vectorcall(module.checked, original_pointer)
assert errors == [], errors
assert timeline == ["argument", ("entry", id(module.checked), 1, None)], timeline
assert module.run_made(lambda: 37) == 42
for name in ("checked", "run_made", "make_callback_family"):
    assert _soac_ext.strict_function_entry_kind(vars(module)[name]) == {expected_entry!r}
"""
    for mode in ("profile", "apply"):
        project.run(
            program,
            opt_mode=mode,
            entry_interpreter=entry_interpreter,
            extra_env={"SOAC_WORK_DIR": str(work_dir)},
        )
        assert (work_dir / "profile.bin").is_file()
