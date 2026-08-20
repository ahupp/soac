"""Entry/runtime behavior through offline contracts and native source admission.

The case IDs preserve the suffixes of the former
``jit::test::tests::blockpy_entry_interpreter_*`` Rust behavior tests. Raw binder
and deopt-kernel tests stay in Rust; these cases create real source functions,
so inspection-only IR is not sufficient authority to execute them.

The two source-function modes use separate processes, but cases within a mode
share one authenticated startup. These cases do not require per-case process
isolation: each imports a distinct module and validates in its own local scope.
Guards reject changes to shared process state or an earlier case's module.
Module initializers always follow their explicit interpreted lowering plan;
they do not acquire a second execution path from the function-mode request.
"""

import json
import textwrap
from dataclasses import dataclass
from pathlib import Path

import pytest

from tests._strict_integration import ROOT, create_strict_project


@dataclass(frozen=True)
class EntryCase:
    source: str
    witness: str
    assertions: str
    generator_factory: bool = False


# One analyzed project covers the family. Each case gets its own independently
# initialized module; the two entry modes run in different subprocesses.
CASES = {
    "uses_registered_function_env": EntryCase(
        """
        def add_default(left, right=9):
            return left + right
        """,
        "add_default",
        """
        assert module.add_default.__defaults__ == (9,)
        assert module.add_default(33) == 42
        """,
    ),
    "vectorcall_executes_class_creation": EntryCase(
        """
        class C:
            marker = 40

            def method(self):
                return self.marker + 2

        RESULT = C().method()
        """,
        "C.method",
        """
        assert isinstance(module.C, type)
        assert module.RESULT == 42
        """,
    ),
    "vectorcall_executes_class_super_closure": EntryCase(
        """
        class Base:
            def value(self):
                return 40

        class C(Base):
            def value(self):
                return super().value() + 2

        RESULT = C().value()
        """,
        "C.value",
        "assert module.RESULT == 42",
    ),
    "vectorcall_executes_decorator_and_metaclass": EntryCase(
        """
        def decorate(cls):
            cls.decorated = cls.flag + 1
            return cls

        class Meta(type):
            def __new__(mcls, name, bases, ns, **kw):
                cls = type.__new__(mcls, name, bases, ns)
                cls.flag = kw["flag"]
                return cls

        @decorate
        class C(metaclass=Meta, flag=41):
            pass

        RESULT = C.decorated
        """,
        "decorate",
        "assert module.RESULT == 42",
    ),
    "vectorcall_preserves_generator_call_semantics": EntryCase(
        """
        def gen():
            yield 40
            yield 2

        RESULT = list(gen())
        """,
        "gen",
        "assert module.RESULT == [40, 2]",
        generator_factory=True,
    ),
    "vectorcall_preserves_coroutine_call_semantics": EntryCase(
        """
        async def coro():
            return 42

        OBJ = coro()
        RESULT = hasattr(OBJ, "__await__")
        OBJ.close()
        """,
        "coro",
        "assert module.RESULT is True",
        generator_factory=True,
    ),
    "vectorcall_preserves_async_generator_call_semantics": EntryCase(
        """
        async def agen():
            yield 42

        OBJ = agen()
        RESULT = hasattr(OBJ, "__anext__")
        """,
        "agen",
        "assert module.RESULT is True",
        generator_factory=True,
    ),
    "executes_module_init_globals": EntryCase(
        """
        VALUE = 41

        def add_one(value):
            return value + 1

        RESULT = add_one(VALUE)
        """,
        "add_one",
        """
        assert module.RESULT == 42
        assert callable(module.add_one)
        """,
    ),
    "executes_attr_and_item_mutation": EntryCase(
        """
        def mutate(obj, data):
            obj.value = data["start"]
            data["next"] = obj.value + 1
            del data["start"]
            return obj.value, data["next"], "start" in data
        """,
        "mutate",
        """
        from types import ModuleType

        obj = ModuleType("entry_attr_item_target")
        data = {"start": 10}
        result = module.mutate(obj, data)
        assert type(result) is tuple
        assert result == (10, 11, False)
        assert result[2] is False
        assert obj.value == 10
        assert data == {"next": 11}
        """,
    ),
    "executes_local_store_and_tuple_return": EntryCase(
        """
        def build(value):
            next_value = value + 1
            return next_value, value
        """,
        "build",
        """
        result = module.build(41)
        assert type(result) is tuple
        assert result == (42, 41)
        """,
    ),
    "executes_branch_and_global_load": EntryCase(
        """
        VALUE = 40

        def choose(flag):
            if flag:
                return VALUE + 2
            return 5
        """,
        "choose",
        """
        assert module.choose(True) == 42
        assert module.choose(False) == 5
        """,
    ),
    "executes_global_keyword_call": EntryCase(
        """
        from entry_support import helper

        def call_helper(value):
            return helper(value, scale=3)
        """,
        "call_helper",
        "assert module.call_helper(11) == 40",
    ),
    "executes_nested_function_with_closure": EntryCase(
        """
        def outer(x):
            def inner(y):
                return x + y
            return inner(5)
        """,
        "outer",
        "assert module.outer(37) == 42",
    ),
    "catches_raised_exception": EntryCase(
        """
        def catch_value_error():
            try:
                raise ValueError("boom")
            except ValueError:
                return 42
        """,
        "catch_value_error",
        "assert module.catch_value_error() == 42",
    ),
    "reraises_current_exception": EntryCase(
        """
        def reraise_value_error():
            try:
                raise ValueError("boom")
            except ValueError:
                raise
        """,
        "reraise_value_error",
        """
        try:
            module.reraise_value_error()
        except ValueError as error:
            assert str(error) == "boom"
        else:
            raise AssertionError("bare raise must propagate the active ValueError")
        """,
    ),
    "runs_finally_before_return": EntryCase(
        """
        def return_through_finally():
            value = 40
            try:
                return value
            finally:
                value = 99
        """,
        "return_through_finally",
        "assert module.return_through_finally() == 40",
    ),
    "finally_return_overrides_exception": EntryCase(
        """
        def finally_overrides_exception():
            try:
                raise ValueError("boom")
            finally:
                return 42
        """,
        "finally_overrides_exception",
        "assert module.finally_overrides_exception() == 42",
    ),
    "preserves_exception_through_finally": EntryCase(
        """
        def exception_through_finally():
            marker = 0
            try:
                try:
                    raise ValueError("boom")
                finally:
                    marker = 40
            except ValueError:
                return marker + 2
        """,
        "exception_through_finally",
        "assert module.exception_through_finally() == 42",
    ),
    "runs_finally_before_loop_break": EntryCase(
        """
        def break_through_finally():
            total = 0
            for value in (1, 2, 3):
                try:
                    break
                finally:
                    total = total + 40
            return total + value
        """,
        "break_through_finally",
        "assert module.break_through_finally() == 41",
    ),
    "runs_finally_before_loop_continue": EntryCase(
        """
        def continue_through_finally():
            total = 0
            for value in (1, 2, 3):
                try:
                    if value == 2:
                        continue
                    total = total + value
                finally:
                    total = total + 10
            return total
        """,
        "continue_through_finally",
        "assert module.continue_through_finally() == 34",
    ),
    "executes_with_statement_value_flow": EntryCase(
        """
        def use_manager():
            class Manager:
                def __enter__(self):
                    return 40

                def __exit__(self, exc_type, exc, tb):
                    return False

            with Manager() as value:
                result = value + 2
            return result
        """,
        "use_manager",
        "assert module.use_manager() == 42",
    ),
    "executes_with_statement_exception_suppression": EntryCase(
        """
        def suppress_with_exception():
            class Manager:
                def __enter__(self):
                    return self

                def __exit__(self, exc_type, exc, tb):
                    self.saw_value_error = exc_type is ValueError
                    return True

            manager = Manager()
            with manager:
                raise ValueError("boom")
            return manager.saw_value_error
        """,
        "suppress_with_exception",
        "assert module.suppress_with_exception() is True",
    ),
    "executes_comprehensions_with_captures": EntryCase(
        """
        def build(values):
            scale = 2
            odd_list = [value + scale for value in values if value % 2]
            odd_dict = {value: value + scale for value in values if value % 2}
            odd_set = {value + scale for value in values if value % 2}
            return odd_list == [3, 5] and odd_dict == {1: 3, 3: 5} and odd_set == {3, 5}
        """,
        "build",
        "assert module.build((1, 2, 3)) is True",
    ),
    "dictcomp_loop_target_and_containing_walrus_have_distinct_frames": EntryCase(
        """
        def build():
            result = {(saved := item): saved for item in (1, 2)}
            return result, saved
        """,
        "build",
        "assert module.build() == ({1: 1, 2: 2}, 2)",
    ),
    "executes_generator_expression_with_capture": EntryCase(
        """
        def build(values):
            scale = 2
            return tuple(value + scale for value in values if value % 2) == (3, 5)
        """,
        "build",
        "assert module.build((1, 2, 3)) is True",
    ),
    "executes_import_statements": EntryCase(
        """
        def build():
            import collections as c
            from collections import deque
            values = deque()
            values.append(41)
            return c.deque is deque and values.pop() == 41
        """,
        "build",
        "assert module.build() is True",
    ),
    "executes_for_loop_control_flow": EntryCase(
        """
        def build(values):
            exhausted = []
            for value in values:
                if value == 2:
                    continue
                exhausted.append(value)
            else:
                exhausted.append(99)
            exhausted_last = value

            stopped = []
            for value in values:
                if value == 2:
                    continue
                if value == 3:
                    break
                stopped.append(value)
            else:
                stopped.append(99)
            stopped_last = value

            return (exhausted == [1, 3, 99] and exhausted_last == 3
                    and stopped == [1] and stopped_last == 3)
        """,
        "build",
        "assert module.build((1, 2, 3)) is True",
    ),
}


def _selected_entry_case_modes(items, *, test_path, test_function):
    """Read actual worker collection; this schedules tests, never admission."""
    test_path = Path(test_path).resolve()
    selected = set()
    for item in items:
        if (
            getattr(item, "obj", None) is not test_function
            or Path(item.path).resolve() != test_path
        ):
            continue
        callspec = getattr(item, "callspec", None)
        params = getattr(callspec, "params", None)
        if not isinstance(params, dict) or not {
            "case_name", "strict_entry_results"
        } <= params.keys():
            raise ValueError("collected strict entry case is missing its parameters")
        name = params["case_name"]
        entry_interpreter = params["strict_entry_results"]
        if type(name) is not str or name not in CASES:
            raise ValueError("collected strict entry case is not in the reviewed catalog")
        if type(entry_interpreter) is not bool:
            raise ValueError("collected strict entry case has an unexpected mode")
        selected.add((name, entry_interpreter))
    return frozenset(selected)


@pytest.fixture(scope="module")
def strict_entry_selected_case_modes(request):
    # pytest has applied this worker's command-line selection/deselection.
    # Match the actual function/path and callspec values, not rendered IDs.
    return _selected_entry_case_modes(
        request.session.items,
        test_path=Path(__file__),
        test_function=test_strict_entry_runtime,
    )


@pytest.fixture(scope="module")
def strict_entry_cases(tmp_path_factory, strict_entry_selected_case_modes):
    selected_names = {name for name, _ in strict_entry_selected_case_modes}
    if not selected_names:
        raise ValueError("strict entry project has no collected cases")
    modules = {
        f"entry_{name}": f"entry_{name}.py"
        for name in CASES if name in selected_names
    }
    sources = {
        modules[f"entry_{name}"]: (
            "from __future__ import strict\n\n"
            + textwrap.dedent(case.source).lstrip("\n")
        )
        for name, case in CASES.items() if name in selected_names
    }
    # The original keyword-call case deliberately called an ordinary function.
    sources["entry_support.py"] = """
def helper(value, scale=1):
    return value * scale + 7
"""
    return create_strict_project(
        tmp_path_factory.mktemp("strict-entry-runtime"), sources, modules=modules
    )


def _case_program(case_name, *, entry_interpreter):
    case = CASES[case_name]
    module_name = f"entry_{case_name}"
    expected_entry = (
        "generator_factory"
        if case.generator_factory
        else "entry_interpreter"
        if entry_interpreter
        else "checked_native"
    )
    bootstrap = f"""
        import ctypes
        import importlib.util
        import sys
        from soac.import_hook import SoacLoader

        assert {module_name!r} not in sys.modules, "case module was already initialized"
        spec = importlib.util.find_spec({module_name!r})
        assert spec is not None and isinstance(spec.loader, SoacLoader)
        module = importlib.util.module_from_spec(spec)
        assert isinstance(spec.loader, SoacLoader), "strict source was declined"
        sys.modules[spec.name] = module
        assert spec.loader.exec_module(module) is None
        diagnostic = _soac_ext.strict_module_diagnostics(module)
        assert diagnostic is not None and diagnostic['sealed'] is True
        assert diagnostic['module_name'] == {module_name!r}
        assert diagnostic['initializer_entry_kind'] == 'entry_interpreter', diagnostic

        witness = module.{case.witness}
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert metadata(witness), "source function did not register with SOAC"
        assert owner(witness), "source function has no native strict owner"
        actual_entry = _soac_ext.strict_function_entry_kind(witness)
        assert actual_entry == {expected_entry!r}, actual_entry
    """
    after = f"""
        actual_entry = _soac_ext.strict_function_entry_kind(witness)
        assert actual_entry == {expected_entry!r}, actual_entry
    """
    validation = textwrap.dedent(case.assertions).rstrip() + "\n"
    return (
        textwrap.dedent(bootstrap)
        + validation
        + textwrap.dedent(after)
    )


@pytest.fixture(scope="module", params=[False, True], ids=["compiled", "entry"])
def strict_entry_results(
    strict_entry_cases, strict_entry_selected_case_modes, request,
):
    if type(request.param) is not bool:
        raise ValueError("strict entry fixture has an unexpected mode")
    selected_names = tuple(
        name for name in CASES
        if (name, request.param) in strict_entry_selected_case_modes
    )
    if not selected_names:
        raise ValueError("strict entry mode has no collected cases")
    # Keep real startup/dependency authentication intact. Only this worker's
    # requested pairs run; a mixed-mode chunk must not run two full catalogs.
    preamble = f"""
        import json
        import os
        from pathlib import Path
        import sys
        sys.path.insert(0, {str(ROOT)!r})
        from tests._integration import ValidationBatch

        journal = Path(os.environ["SOAC_WORK_DIR"]).parent / "entry-cases.jsonl"
    """
    program = [textwrap.dedent(preamble)]
    program.append(
        f"batch = ValidationBatch({tuple(strict_entry_cases.modules)!r}, journal)\n"
    )
    for index, name in enumerate(selected_names):
        # Statically generate an ordinary validation function for each case;
        # no exec/compile of strict source or alternate module loader is used.
        program.append(f"\ndef case_{index}():\n")
        program.append(
            textwrap.indent(
                _case_program(name, entry_interpreter=request.param), "    "
            )
        )
        program.append(f"\nbatch.run({name!r}, case_{index})\n")
    program.append("print(json.dumps(batch.results))\n")
    completed = strict_entry_cases.run(
        "".join(program), entry_interpreter=request.param, timeout=600
    )
    results = json.loads(completed.stdout)
    assert set(results) == set(selected_names), "runtime did not report every requested case"
    return results


@pytest.mark.parametrize("case_name", CASES)
def test_strict_entry_runtime(strict_entry_results, case_name):
    error = strict_entry_results[case_name]
    assert error is None, f"{case_name} failed through the real strict entry:\n{error}"


_COMPREHENSION_CAPTURE_CASES = (
    "executes_comprehensions_with_captures",
    "dictcomp_loop_target_and_containing_walrus_have_distinct_frames",
)


@pytest.mark.parametrize("case_name", _COMPREHENSION_CAPTURE_CASES)
def test_eager_comprehension_original_stock_control(tmp_path, case_name):
    import ctypes
    from pathlib import Path
    from tests._integration import exec_integration_validation, stock_module

    case = CASES[case_name]
    source = textwrap.dedent(case.source).lstrip("\n")
    with stock_module(tmp_path, f"ordinary_{case_name}", source) as module:
        function = vars(module)[case.witness]
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        assert owner(function) is None and metadata(function) is None
        validation = "def validate_module(module):\n" + textwrap.indent(
            textwrap.dedent(case.assertions).strip() + "\n", "    "
        )
        exec_integration_validation(
            validation, module, Path(module.__file__), mode="stock"
        )
        assert owner(function) is None and metadata(function) is None


@pytest.fixture(scope="module")
def native_comprehension_cases(tmp_path_factory):
    modules = {
        f"entry_{name}": f"entry_{name}.py"
        for name in _COMPREHENSION_CAPTURE_CASES
    }
    sources = {
        modules[f"entry_{name}"]: (
            "from __future__ import strict\n\n"
            + textwrap.dedent(CASES[name].source).lstrip("\n")
        )
        for name in _COMPREHENSION_CAPTURE_CASES
    }
    return create_strict_project(
        tmp_path_factory.mktemp("strict-native-entry-comprehensions"),
        sources, modules=modules, backend="cpython",
    )


@pytest.mark.parametrize("case_name", _COMPREHENSION_CAPTURE_CASES)
def test_eager_comprehension_original_cpython_control(
    native_comprehension_cases, case_name,
):
    from pathlib import Path

    case = CASES[case_name]
    validation = (
        textwrap.dedent(case.assertions).rstrip()
        + "\nfrom soac import _soac_ext\n"
        + f"assert _soac_ext.strict_function_diagnostics(module.{case.witness})"
        + "['original_code_entered'] is True\n"
    )
    validation = "def validate_module(module):\n" + textwrap.indent(
        validation.lstrip("\n"), "    "
    )
    # The existing helper proves the exact startup/source/generation, native
    # owner and zero lowering/cache/JIT activity before/after.
    native_comprehension_cases.run_case(
        f"entry_{case_name}", validation, Path(__file__),
        required_functions=(case.witness,),
        
        backend="cpython",
    )


_LOOP_RECEIVER_SOURCE = """
def exhaust(iterator, observe):
    observe('before')
    for value in iterator:
        observe('body')
    observe('after')
"""


def _for_loop_receiver_observations(module):
    """Keep native count diagnostics separate from callback and cleanup checks."""
    import gc
    import sys
    import weakref

    events = []
    reference = None

    def observe(label):
        receiver = reference()
        assert receiver is not None
        events.append((label, sys.getrefcount(receiver)))

    # This class and observer remain ordinary Python outside the strict project.
    # The observer holds only a weak reference between calls, and uses the same
    # temporary strong receiver reference at every measurement point.
    class ObservedIterator:
        def __init__(self):
            self.position = 0

        def __iter__(self):
            return self

        def __next__(self):
            observe("next")
            if self.position == 2:
                raise StopIteration
            self.position += 1
            return self.position

    iterator = ObservedIterator()
    reference = weakref.ref(iterator)
    assert module.exhaust(iterator, observe) is None
    assert [label for label, _ in events] == [
        "before",
        "next",
        "body",
        "next",
        "body",
        "next",
        "after",
    ]
    baseline = events[0][1]
    body_baseline = events[2][1]
    del iterator
    gc.collect()
    assert reference() is None, 'completed loop retained its iterator'
    return {
        "entry_relative": tuple(count - baseline for _, count in events),
        "next_over_body": tuple(
            count - body_baseline for label, count in events if label == "next"
        ),
        "after_over_entry": events[-1][1] - baseline,
    }, events


def test_for_loop_next_receiver_native_control(tmp_path):
    from tests._integration import stock_module

    with stock_module(
        tmp_path, "ordinary_loop_receiver", _LOOP_RECEIVER_SOURCE
    ) as module:
        observed, counts = _for_loop_receiver_observations(module)
    (tmp_path / "receiver-counts.json").write_text(
        json.dumps(
            {
                "observed": observed,
                "absolute_counts": counts,
            },
            indent=2,
        )
        + "\n"
    )
    # FOR_ITER retains the loop iterator, and the ordinary __next__ activation
    # has its own self reference; it does not clone another call operand.
    assert observed["next_over_body"] == (1, 1, 1), (observed, counts)
    assert observed["after_over_entry"] == 0, (observed, counts)


@pytest.fixture(scope="module")
def strict_loop_receiver_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-loop-receiver"),
        {"loop_receiver.py": "from __future__ import strict\n" + _LOOP_RECEIVER_SOURCE},
        modules={"loop_receiver": "loop_receiver.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_for_loop_next_receiver_preserves_callbacks_and_safe_ownership(
    strict_loop_receiver_project,
    tmp_path,
    entry_interpreter,
):
    from pathlib import Path
    from tests._integration import stock_module

    with stock_module(
        tmp_path, "ordinary_loop_receiver", _LOOP_RECEIVER_SOURCE
    ) as module:
        _, native_counts = _for_loop_receiver_observations(module)
    expected_labels = [label for label, _ in native_counts]
    validation = f"""
def validate(module):
    import json
    from tests.test_strict_entry_runtime import _for_loop_receiver_observations
    actual, counts = _for_loop_receiver_observations(module)
    print(json.dumps({{'actual': actual, 'absolute_counts': counts,
                      'native_counts': {native_counts!r}}}))
    # Exact transient counts are a native-only diagnostic. The common helper
    # checks result, explicit callback order, live operands and eventual cleanup.
    assert [label for label, _ in counts] == {expected_labels!r}, counts
"""
    # run_case proves actual native ownership/sealing and the requested
    # checked-native versus entry-interpreter witness before and after the call.
    strict_loop_receiver_project.run_case(
        "loop_receiver",
        validation,
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("exhaust",),
    )


_LOOP_EXIT_SOURCE = """
def exhaust(make, observe, target):
    for value in make():
        observe('body')
    else:
        observe('else')
    observe('after')

def break_loop(make, observe, target):
    for value in make():
        observe('body')
        break
    else:
        observe('unexpected-else')
    observe('after')

def return_loop(make, observe, target):
    for value in make():
        return observe('return-value')

def failing_target(make, observe, target):
    try:
        for target.value in make():
            observe('unexpected-body')
    except ValueError:
        observe('caught-outer')
    observe('after')

def failing_body(make, observe, target):
    try:
        for value in make():
            observe('body')
            raise ValueError('body error')
    except ValueError:
        observe('caught-outer')
    observe('after')

def caught_continue(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            observe('caught-inner')
            continue
    observe('after')

def caught_break(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            observe('caught-inner')
            break
    observe('after')

def caught_return(make, observe, target):
    for value in make():
        try:
            raise LookupError('inner')
        except LookupError:
            return observe('return-value')

def failing_finally(make, observe, target):
    try:
        for value in make():
            try:
                raise ValueError('body error')
            finally:
                observe('finally')
    except ValueError:
        observe('caught-outer')
    observe('after')
"""

_LOOP_EXIT_CASES = (
    "exhaust", "break_loop", "return_loop", "failing_target", "failing_body",
    "caught_continue", "caught_break", "caught_return", "failing_finally",
)


def _for_loop_exit_observations(module, case):
    """Observe a temporary iterator without adding a lasting receiver owner."""
    import gc
    import sys
    import weakref

    events = []
    reference = None

    def context():
        error = sys.exception()
        return None if error is None else type(error).__name__

    def observe(label):
        events.append((label, reference() is not None, context()))
        return "return-token"

    class Iterator:
        def __init__(self):
            self.position = 0

        def __iter__(self):
            return self

        def __next__(self):
            observe("next")
            if self.position == 2:
                raise StopIteration
            self.position += 1
            return self.position

        def __del__(self):
            events.append(("drop", context()))

    def make():
        nonlocal reference
        iterator = Iterator()
        reference = weakref.ref(iterator)
        return iterator

    class Target:
        def __setattr__(self, name, value):
            observe("target")
            raise ValueError("target error")

    # Explicit source callbacks inherit the real handled-exception context.
    # An implicit finalizer's timing/context is recorded separately below.
    try:
        raise KeyError("caller")
    except KeyError:
        result = getattr(module, case)(make, observe, Target())
        observe("caller")
    gc.collect()
    assert reference() is None, ('completed loop retained its iterator', case, events)
    assert sum(event[0] == 'drop' for event in events) == 1, (case, events)
    return {"events": events, "result": result}


def _for_loop_exit_semantics(observed):
    """Only explicit callback order/context and the computed result are parity."""
    return {
        "events": [
            (event[0], event[2]) for event in observed["events"] if event[0] != "drop"
        ],
        "result": observed["result"],
    }


@pytest.mark.parametrize("case", _LOOP_EXIT_CASES)
def test_for_loop_exit_native_control(tmp_path, case):
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_loop_exit", _LOOP_EXIT_SOURCE) as module:
        observed = _for_loop_exit_observations(module, case)
    (tmp_path / "loop-exit.json").write_text(json.dumps(observed, indent=2) + "\n")
    events = observed["events"]
    assert [event for event in events if event[0] == "drop"] == [("drop", "KeyError")]
    assert events[-1] == ("caller", False, "KeyError")
    for event in events:
        if event[0] in {"body", "next", "target", "caught-inner", "return-value", "finally"}:
            assert event[1] is True, (case, observed)
        elif event[0] in {"else", "after", "caught-outer"}:
            assert event[1] is False, (case, observed)
        assert not event[0].startswith("unexpected-"), (case, observed)
    if case in {"return_loop", "caught_return"}:
        assert observed["result"] == "return-token"
    else:
        assert observed["result"] is None


@pytest.fixture(scope="module")
def strict_loop_exit_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-loop-exit"),
        {"loop_exit.py": "from __future__ import strict\n" + _LOOP_EXIT_SOURCE},
        modules={"loop_exit": "loop_exit.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_for_loop_exit_preserves_callbacks_context_and_required_cleanup(
    strict_loop_exit_project, tmp_path, entry_interpreter,
):
    from pathlib import Path
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_loop_exit", _LOOP_EXIT_SOURCE) as module:
        expected = {
            case: _for_loop_exit_semantics(_for_loop_exit_observations(module, case))
            for case in _LOOP_EXIT_CASES
        }
    validation = f"""
def validate(module):
    import json
    from tests.test_strict_entry_runtime import (
        _for_loop_exit_observations, _for_loop_exit_semantics,
    )
    expected = {expected!r}
    actual = {{
        case: _for_loop_exit_semantics(_for_loop_exit_observations(module, case))
        for case in expected
    }}
    print(json.dumps({{'actual': actual, 'expected': expected}}))
    failures = {{case: actual[case] for case in expected if actual[case] != expected[case]}}
    assert not failures, (failures, expected)
"""
    strict_loop_exit_project.run_case(
        "loop_exit", validation, Path(__file__),
        entry_interpreter=entry_interpreter, required_functions=_LOOP_EXIT_CASES,
    )


_LOOP_TRACEBACK_SOURCE = """
def exhaust(iterator, make_payload):
    payload = make_payload()
    for item in iterator:
        pass
"""


@pytest.fixture(scope="module")
def loop_error_native_extension(tmp_path_factory):
    """A real tp_iternext error without a Python callback frame."""
    import hashlib
    from pathlib import Path
    import shlex
    import subprocess
    import sys
    import sysconfig

    source = Path(sysconfig.get_config_var("abs_srcdir"))
    build = Path(sysconfig.get_config_var("abs_builddir"))
    assert (build / "python").resolve() == Path(sys._base_executable).resolve()
    out = tmp_path_factory.mktemp("native-loop-error")
    extension = out / ("_loop_error_native" + sysconfig.get_config_var("EXT_SUFFIX"))
    probe = ROOT / "tests/native/iterator_error.c"
    command = [
        *shlex.split(sysconfig.get_config_var("LDSHARED")),
        *shlex.split(sysconfig.get_config_var("CCSHARED")),
        "-O0", "-g", "-Wall", "-Wextra", "-Werror",
        f"-I{source / 'Include'}", f"-I{build}", str(probe), "-o", str(extension),
    ]
    result = subprocess.run(command, capture_output=True, text=True, timeout=60, check=False)
    (out / "build.log").write_text(shlex.join(command) + "\n" + result.stdout + result.stderr)
    assert result.returncode == 0, result.stderr
    (out / "identity.json").write_text(json.dumps({
        "source": str(probe),
        "source_sha256": hashlib.sha256(probe.read_bytes()).hexdigest(),
        "extension": str(extension),
        "extension_sha256": hashlib.sha256(extension.read_bytes()).hexdigest(),
        "native_source": str(source),
        "native_build": str(build),
    }, indent=2) + "\n")
    return extension


def _for_loop_error_traceback_observations(
    module, extension_path, native_slot, exhaustion, *, implicit=True, native_frames=True,
):
    import gc
    import importlib.util
    import weakref

    spec = importlib.util.spec_from_file_location("_loop_error_native", extension_path)
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    events = []
    payload_refs = []
    retained = StopIteration("exhausted") if exhaustion else ValueError("real error")

    class Payload:
        def __del__(self):
            events.append("deleted")

    def make_payload():
        value = Payload()
        events.append("created")
        payload_refs.append(weakref.ref(value))
        return value

    class PythonIterator:
        def __iter__(self):
            return self

        def __next__(self):
            raise retained

    iterator = native.make(retained) if native_slot else PythonIterator()
    if exhaustion and implicit:
        assert module.exhaust(iterator, make_payload) is None
    else:
        try:
            module.exhaust(iterator, make_payload)
        except type(retained) as error:
            assert error is retained
        else:
            raise AssertionError("the iterator error must propagate")

    # Frame shape and retained-local lifetime are ordinary CPython controls,
    # not prerequisites for SOAC exception propagation or required cleanup.
    source_lines = []
    if native_frames:
        source_code = module.exhaust.__code__
        traceback = retained.__traceback__
        while traceback is not None:
            if traceback.tb_frame.f_code is source_code:
                source_lines.append(traceback.tb_lineno - source_code.co_firstlineno)
            traceback = traceback.tb_next
    observed = {
        "payload_alive_before_clear": payload_refs[0]() is not None,
        "events_before_clear": events.copy(),
        "source_lines": source_lines,
        "traceback_absent": retained.__traceback__ is None,
    }
    retained.__traceback__ = None
    gc.collect()
    observed["payload_alive_after_clear"] = payload_refs[0]() is not None
    observed["events_after_clear"] = events.copy()
    return observed


def _loop_error_semantics(observed):
    assert observed["payload_alive_after_clear"] is False, observed
    assert sorted(observed["events_after_clear"]) == ["created", "deleted"], observed
    return observed["payload_alive_after_clear"], sorted(observed["events_after_clear"])


@pytest.mark.parametrize("native_slot", [True, False], ids=["c-slot", "python-next"])
@pytest.mark.parametrize("exhaustion", [True, False], ids=["exhaustion", "real-error"])
def test_for_loop_error_traceback_native_control(
    tmp_path, loop_error_native_extension, native_slot, exhaustion,
):
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_loop_traceback", _LOOP_TRACEBACK_SOURCE) as module:
        observed = _for_loop_error_traceback_observations(
            module, loop_error_native_extension, native_slot, exhaustion,
        )
    (tmp_path / "loop-traceback.json").write_text(json.dumps(observed, indent=2) + "\n")
    # C exhaustion has no traceback. Python __next__ does retain its callback
    # traceback, whose frame-back edge keeps the source frame's payload alive.
    # A real error adds the source loop frame for either iterator backend.
    should_retain = not exhaustion or not native_slot
    assert observed == {
        "payload_alive_before_clear": should_retain,
        "events_before_clear": ["created"] if should_retain else ["created", "deleted"],
        "source_lines": [] if exhaustion else [2],
        "traceback_absent": native_slot and exhaustion,
        "payload_alive_after_clear": False,
        "events_after_clear": ["created", "deleted"],
    }


@pytest.fixture(scope="module")
def strict_loop_traceback_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-loop-traceback"),
        {"loop_traceback.py": "from __future__ import strict\n" + _LOOP_TRACEBACK_SOURCE},
        modules={"loop_traceback": "loop_traceback.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("native_slot", [True, False], ids=["c-slot", "python-next"])
@pytest.mark.parametrize("exhaustion", [True, False], ids=["exhaustion", "real-error"])
def test_for_loop_error_preserves_exception_identity_and_cleanup(
    strict_loop_traceback_project, tmp_path, loop_error_native_extension,
    entry_interpreter, native_slot, exhaustion,
):
    from pathlib import Path
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_loop_traceback", _LOOP_TRACEBACK_SOURCE) as module:
        expected = _loop_error_semantics(_for_loop_error_traceback_observations(
            module, loop_error_native_extension, native_slot, exhaustion,
        ))
    validation = f"""
def validate(module):
    import json
    from tests.test_strict_entry_runtime import (
        _for_loop_error_traceback_observations, _loop_error_semantics,
    )
    actual = _loop_error_semantics(_for_loop_error_traceback_observations(
        module, {str(loop_error_native_extension)!r}, {native_slot!r}, {exhaustion!r},
        native_frames=False,
    ))
    print(json.dumps({{'actual': actual, 'expected': {expected!r}}}))
    assert actual == {expected!r}, (actual, {expected!r})
"""
    strict_loop_traceback_project.run_case(
        "loop_traceback", validation, Path(__file__),
        entry_interpreter=entry_interpreter, required_functions=("exhaust",),
    )


_EXPLICIT_NEXT_TRACEBACK_SOURCE = """
def exhaust(iterator, make_payload):
    payload = make_payload()
    next(iterator)
"""


@pytest.mark.parametrize("native_slot", [True, False], ids=["c-slot", "python-next"])
def test_explicit_next_traceback_native_control(tmp_path, loop_error_native_extension, native_slot):
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_next_traceback", _EXPLICIT_NEXT_TRACEBACK_SOURCE) as module:
        observed = _for_loop_error_traceback_observations(
            module, loop_error_native_extension, native_slot, True, implicit=False,
        )
    assert observed == {
        "payload_alive_before_clear": True,
        "events_before_clear": ["created"],
        "source_lines": [2],
        "traceback_absent": False,
        "payload_alive_after_clear": False,
        "events_after_clear": ["created", "deleted"],
    }


@pytest.fixture(scope="module")
def strict_explicit_next_traceback_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-next-traceback"),
        {"next_traceback.py": "from __future__ import strict\n" + _EXPLICIT_NEXT_TRACEBACK_SOURCE},
        modules={"next_traceback": "next_traceback.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("native_slot", [True, False], ids=["c-slot", "python-next"])
def test_explicit_next_preserves_exception_identity_and_cleanup(
    strict_explicit_next_traceback_project, tmp_path, loop_error_native_extension,
    entry_interpreter, native_slot,
):
    from pathlib import Path
    from tests._integration import stock_module

    with stock_module(tmp_path, "ordinary_next_traceback", _EXPLICIT_NEXT_TRACEBACK_SOURCE) as module:
        expected = _loop_error_semantics(_for_loop_error_traceback_observations(
            module, loop_error_native_extension, native_slot, True, implicit=False,
        ))
    validation = f"""
def validate(module):
    import json
    from tests.test_strict_entry_runtime import (
        _for_loop_error_traceback_observations, _loop_error_semantics,
    )
    actual = _loop_error_semantics(_for_loop_error_traceback_observations(
        module, {str(loop_error_native_extension)!r}, {native_slot!r}, True, implicit=False,
        native_frames=False,
    ))
    print(json.dumps({{'actual': actual, 'expected': {expected!r}}}))
    assert actual == {expected!r}, (actual, {expected!r})
"""
    strict_explicit_next_traceback_project.run_case(
        "next_traceback", validation, Path(__file__),
        entry_interpreter=entry_interpreter, required_functions=("exhaust",),
    )


_EAGER_COMPREHENSION_FRAME_SOURCE = """
def nested_builtin_positional(mapping, key):
    return [mapping.get(value) for value in (key,)][0]

def schedule(owners):
    return [owner.link for owner in owners]

def preserve_outer(make, visit):
    item = make('outer')
    [visit() for item in (make('inner'),)]
    return 'done'
"""


def _eager_comprehension_frame_observations(module, exceptional=False):
    import gc
    import weakref

    class Owner:
        def __init__(self, link):
            self.link = link

    # These are the unchanged bodies that exposed the missing parent-slot
    # projection. No hand-written loop or replacement validator stands in.
    result = {
        'builtin': module.nested_builtin_positional({'key': 41}, 'key'),
        'field': module.schedule([Owner(11), Owner(31)]),
    }
    events = []
    references = {}
    errors = []
    callbacks = []
    marker = ValueError('unwind the original region')

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(self.label)

    def make(label):
        callbacks.append(('make', label))
        value = Payload(label)
        references[label] = weakref.ref(value)
        return value

    def visit():
        callbacks.append(('visit',))
        if exceptional:
            raise marker
        try:
            raise ValueError('retain the callback traceback')
        except ValueError as error:
            errors.append(error)

    try:
        if exceptional:
            try:
                module.preserve_outer(make, visit)
            except ValueError as error:
                assert error is marker
                errors.append(error)
            else:
                raise AssertionError('the unchanged callback must raise')
        else:
            assert module.preserve_outer(make, visit) == 'done'
        assert len(errors) == 1
        assert callbacks == [('make', 'outer'), ('make', 'inner'), ('visit',)]
        # Ordinary retained traceback/frame-back ownership keeps the restored
        # outer local, not the temporary target, after normal or error exit.
        # No f_locals/f_back introspection is needed to observe this lifetime.
        result['outer_before_clear'] = references['outer']() is not None
        result['inner_before_clear'] = references['inner']() is not None
        result['events_before_clear'] = events.copy()
        errors[0].__traceback__ = None
        gc.collect()
        result['outer_after_clear'] = references['outer']() is not None
        result['inner_after_clear'] = references['inner']() is not None
        result['events_after_clear'] = events.copy()
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()
    return result


def _eager_comprehension_semantics(observed):
    assert not observed['outer_after_clear'] and not observed['inner_after_clear'], observed
    assert sorted(observed['events_after_clear']) == ['inner', 'outer'], observed
    return observed['builtin'], observed['field'], sorted(observed['events_after_clear'])


@pytest.mark.parametrize('exceptional', [False, True], ids=['normal', 'exception'])
def test_eager_comprehension_source_frame_native_control(tmp_path, exceptional):
    from tests._integration import stock_module

    with stock_module(
        tmp_path, 'ordinary_comprehension_source_frame',
        _EAGER_COMPREHENSION_FRAME_SOURCE,
    ) as module:
        observed = _eager_comprehension_frame_observations(module, exceptional)
    assert observed == {
        'builtin': 41,
        'field': [11, 31],
        'outer_before_clear': True,
        'inner_before_clear': False,
        'events_before_clear': ['inner'],
        'outer_after_clear': False,
        'inner_after_clear': False,
        'events_after_clear': ['inner', 'outer'],
    }


@pytest.fixture(scope='module')
def strict_eager_comprehension_frame_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp('strict-comprehension-source-frame'),
        {
            'comprehension_source_frame.py': (
                'from __future__ import strict\n' + _EAGER_COMPREHENSION_FRAME_SOURCE
            ),
        },
        modules={'comprehension_source_frame': 'comprehension_source_frame.py'},
    )


@pytest.mark.parametrize('entry_interpreter', [False, True], ids=['compiled', 'entry'])
@pytest.mark.parametrize('exceptional', [False, True], ids=['normal', 'exception'])
def test_eager_comprehension_preserves_callbacks_errors_and_cleanup(
    strict_eager_comprehension_frame_project, tmp_path, entry_interpreter, exceptional,
):
    from tests._integration import stock_module

    with stock_module(
        tmp_path, 'ordinary_comprehension_source_frame',
        _EAGER_COMPREHENSION_FRAME_SOURCE,
    ) as module:
        expected = _eager_comprehension_semantics(
            _eager_comprehension_frame_observations(module, exceptional)
        )
    validation = f"""
def validate(module):
    from tests.test_strict_entry_runtime import (
        _eager_comprehension_frame_observations, _eager_comprehension_semantics,
    )
    actual = _eager_comprehension_semantics(
        _eager_comprehension_frame_observations(module, {exceptional!r})
    )
    assert actual == {expected!r}, (actual, {expected!r})
"""
    strict_eager_comprehension_frame_project.run_case(
        'comprehension_source_frame', validation, Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=('nested_builtin_positional', 'schedule', 'preserve_outer'),
    )


_LAMBDA_COMPREHENSION_FRAME_SOURCE = """
def make():
    return lambda target, values, inner, visit, item: (
        [visit(target.value, item) for target.value in values for item in inner()],
        item,
    )
"""


def _lambda_comprehension_frame_observations(function, exceptional):
    import gc
    import weakref

    events = []
    finalized = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            references.append(weakref.ref(self))

        def __del__(self):
            finalized.append(self.label)

    class Target:
        def __setattr__(self, name, value):
            assert name == 'value', name
            events.append(('store', value))
            object.__setattr__(self, name, value)

    target = Target()

    def inner():
        events.append(('inner', target.value))
        return [Payload(f'{target.value}:a'), Payload(f'{target.value}:b')]

    def visit(outer, item):
        events.append(('visit', outer, item.label))
        if exceptional and item.label == '2:b':
            raise ValueError('lambda target callback')
        return outer, item.label

    saved = Payload('saved')
    try:
        result = function(target, (1, 2), inner, visit, saved)
    except ValueError as error:
        assert exceptional
        assert type(error) is ValueError
        outcome = ('error', str(error))
        # Explicitly retire the retained traceback before checking eventual
        # cleanup; implicit release timing is not a cross-engine requirement.
        error.__traceback__ = None
    else:
        assert not exceptional
        values, restored = result
        assert restored is saved, 'the comprehension must restore its outer parameter'
        outcome = ('return', values)
        del result, restored
    del saved
    gc.collect()
    assert all(reference() is None for reference in references)
    assert sorted(finalized) == ['1:a', '1:b', '2:a', '2:b', 'saved']
    assert events == [
        ('store', 1), ('inner', 1), ('visit', 1, '1:a'), ('visit', 1, '1:b'),
        ('store', 2), ('inner', 2), ('visit', 2, '2:a'), ('visit', 2, '2:b'),
    ]
    assert target.value == 2
    return {'events': events, 'outcome': outcome, 'finalized': sorted(finalized)}


@pytest.fixture(scope='module')
def strict_lambda_comprehension_frame_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp('strict-lambda-comprehension-frame'),
        {'lambda_comprehension_frame.py': (
            'from __future__ import strict\n' + _LAMBDA_COMPREHENSION_FRAME_SOURCE
        )},
        modules={'lambda_comprehension_frame': 'lambda_comprehension_frame.py'},
    )


@pytest.mark.parametrize('entry_interpreter', [False, True], ids=['compiled', 'entry'])
@pytest.mark.parametrize('exceptional', [False, True], ids=['normal', 'exception'])
def test_lambda_comprehension_preserves_targets_outer_binding_and_cleanup(
    strict_lambda_comprehension_frame_project, tmp_path, entry_interpreter, exceptional,
):
    from tests._integration import stock_module

    with stock_module(
        tmp_path, 'ordinary_lambda_comprehension_frame', _LAMBDA_COMPREHENSION_FRAME_SOURCE,
    ) as module:
        expected = _lambda_comprehension_frame_observations(module.make(), exceptional)
    validation = f"""
def validate(module):
    import ctypes
    from tests.test_strict_entry_runtime import _lambda_comprehension_frame_observations
    function = module.make()
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    unchecked = ctypes.pythonapi.PyFunction_GetSoacFunctionId
    unchecked.argtypes = [ctypes.py_object]
    unchecked.restype = ctypes.c_uint64
    source = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
    source.argtypes = [ctypes.py_object]
    source.restype = ctypes.c_uint64
    assert metadata(function) and owner(function) and source(function.__code__)
    assert unchecked(function) == 0
    actual = _lambda_comprehension_frame_observations(function, {exceptional!r})
    assert actual == {expected!r}, (actual, {expected!r})
"""
    strict_lambda_comprehension_frame_project.run_case(
        'lambda_comprehension_frame', validation, Path(__file__),
        entry_interpreter=entry_interpreter, required_functions=('make',),
    )


_NESTED_COMPREHENSION_BINDING_SOURCE = """
def nested(values, value, inner, visit, observe):
    try:
        result = {value: [visit(value, inner) for inner in (value, value)]
                  for value in values}
    except ValueError:
        observe('error', value, inner)
        raise
    finally:
        observe('finally', value, inner)
    return result, value, inner
"""


def _nested_comprehension_binding_observations(module, exceptional):
    import gc
    import weakref

    events = []
    finalized = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            references.append(weakref.ref(self))

        def __del__(self):
            finalized.append(self.label)

    first = Payload('first')
    second = Payload('second')
    saved_value = Payload('outer-value')
    saved_inner = Payload('outer-inner')
    failure = ValueError('nested iteration callback')
    failing = exceptional

    def visit(value, inner):
        assert inner is value, 'the nested target must alias its own iterable element'
        events.append(('visit', value.label))
        if failing and value is second:
            raise failure
        return inner

    def observe(event, value, inner):
        assert value is saved_value, 'outer target must be restored before source cleanup'
        assert inner is saved_inner, 'child target must be restored before source cleanup'
        events.append((event, value.label, inner.label))

    outcomes = []
    # The second call proves recovery after error; the third never enters the
    # child region. All observations are explicit source operations or aliases.
    for failing, values in [(exceptional, (first, second)), (False, (first, second)), (False, ())]:
        start = len(events)
        try:
            result, outer, inner = module.nested(
                values, saved_value, saved_inner, visit, observe,
            )
        except ValueError as error:
            assert failing and error is failure
            failure.__traceback__ = None
            outcomes.append('error')
            expected_events = [
                ('visit', 'first'), ('visit', 'first'), ('visit', 'second'),
                ('error', 'outer-value', 'outer-inner'),
                ('finally', 'outer-value', 'outer-inner'),
            ]
        else:
            assert not failing
            assert outer is saved_value and inner is saved_inner
            assert list(result) == list(values)
            for value in values:
                assert len(result[value]) == 2
                assert result[value][0] is value and result[value][1] is value
            if values:
                del value
            outcomes.append([(key.label, [item.label for item in result[key]]) for key in values])
            expected_events = [
                *[('visit', value.label) for value in values for _ in range(2)],
                ('finally', 'outer-value', 'outer-inner'),
            ]
            del result, outer, inner
        assert events[start:] == expected_events
    del first, second, saved_value, saved_inner, visit, observe, failure
    gc.collect()
    assert all(reference() is None for reference in references)
    assert sorted(finalized) == ['first', 'outer-inner', 'outer-value', 'second']
    return {'events': events, 'outcomes': outcomes, 'finalized': sorted(finalized)}


@pytest.fixture(scope='module')
def strict_nested_comprehension_binding_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp('strict-nested-comprehension-binding'),
        {'nested_comprehension_binding.py': (
            'from __future__ import strict\n' + _NESTED_COMPREHENSION_BINDING_SOURCE
        )},
        modules={'nested_comprehension_binding': 'nested_comprehension_binding.py'},
    )


@pytest.mark.parametrize('entry_interpreter', [False, True], ids=['compiled', 'entry'])
@pytest.mark.parametrize('exceptional', [False, True], ids=['normal', 'exception'])
def test_nested_comprehension_preserves_source_scoping_aliases_and_recovery(
    strict_nested_comprehension_binding_project, tmp_path, entry_interpreter, exceptional,
):
    from tests._integration import stock_module

    with stock_module(
        tmp_path, 'ordinary_nested_comprehension_binding', _NESTED_COMPREHENSION_BINDING_SOURCE,
    ) as module:
        expected = _nested_comprehension_binding_observations(module, exceptional)
    validation = f"""
def validate(module):
    from tests.test_strict_entry_runtime import _nested_comprehension_binding_observations
    actual = _nested_comprehension_binding_observations(module, {exceptional!r})
    assert actual == {expected!r}, (actual, {expected!r})
"""
    strict_nested_comprehension_binding_project.run_case(
        'nested_comprehension_binding', validation, Path(__file__),
        entry_interpreter=entry_interpreter, required_functions=('nested',),
    )


_ASYNC_COMPREHENSION_SEMANTIC_SOURCE = """
async def collect(outer, values, step, record):
    item = outer
    selected = None

    def read():
        return selected

    record('start', item, read)
    try:
        result = [(selected := await step(item)) async for item in values()]
        record('result', result)
        return item, selected, result, read
    except BaseException as error:
        record('error', error)
        raise
    finally:
        record('finally', item, read)
"""


def _async_comprehension_semantic_observations(module):
    import asyncio
    import gc
    import weakref

    async def exercise(cancel):
        events, finalized, references = [], [], {}
        arrivals, permits = asyncio.Queue(), asyncio.Queue()
        outer = object()
        saved = {}

        class Payload:
            def __init__(self, label):
                self.label = label
                assert label not in references
                references[label] = weakref.ref(self)

            def __del__(self):
                finalized.append(self.label)

        class Values:
            def __init__(self):
                self.position = 0
                assert 'iterator' not in references
                references['iterator'] = weakref.ref(self)

            def __aiter__(self):
                events.append('aiter')
                return self

            async def __anext__(self):
                if self.position == 2:
                    events.append('stop')
                    raise StopAsyncIteration
                label = ('first', 'second')[self.position]
                self.position += 1
                events.append(('next', label))
                return Payload(label)

            def __del__(self):
                finalized.append('iterator')

        def values():
            events.append('values')
            return Values()

        async def step(value):
            label = value.label
            events.append(('wait', label))
            arrivals.put_nowait(label)
            try:
                await permits.get()
                assert value is references[label]()
                events.append(('resume', label))
                return value
            finally:
                events.append(('step-finally', value.label))

        def record(event, *arguments):
            if event == 'start':
                actual_outer, read = arguments
                assert actual_outer is outer and read() is None
                saved['read'] = read
                events.append('start')
            elif event == 'result':
                events.append(('result', tuple(value.label for value in arguments[0])))
            elif event == 'error':
                error, = arguments
                saved['error'] = error
                events.append(('error', type(error).__name__))
            elif event == 'finally':
                actual_outer, read = arguments
                assert actual_outer is outer, 'the comprehension target escaped its scope'
                assert read is saved['read'], 'the containing captured cell was replaced'
                current = read()
                events.append(('finally', None if current is None else current.label))
            else:
                raise AssertionError(event)

        task = asyncio.create_task(module.collect(outer, values, step, record))
        try:
            assert await asyncio.wait_for(arrivals.get(), 15) == 'first'
            assert not task.done(), 'the first await did not suspend'
            assert saved['read']() is None, 'walrus committed before its await completed'
            permits.put_nowait(None)
            assert await asyncio.wait_for(arrivals.get(), 15) == 'second'
            assert not task.done(), 'the second await did not suspend'
            assert saved['read']() is references['first']()
            if cancel:
                assert task.cancel('cancel semantic comprehension')
                try:
                    await task
                except asyncio.CancelledError as error:
                    assert saved.pop('error') is error
                    assert error.args == ('cancel semantic comprehension',)
                    error.__traceback__ = None
                else:
                    raise AssertionError('cancellation disappeared')
                assert task.cancelled()
            else:
                permits.put_nowait(None)
                result = await task
                assert result[0] is outer and result[3] is saved['read']
                assert result[2][0] is references['first']()
                assert result[2][1] is references['second']()
                assert result[1] is result[2][1] is result[3]()
                assert 'error' not in saved
        finally:
            if not task.done():
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass
            saved.clear()
        assert task.done()
        return events, references, finalized

    observations = []
    # Normal completion after cancellation also proves that no suspended helper
    # activation or containing walrus cell leaked into the next source call.
    for cancel in (True, False):
        events, references, finalized = asyncio.run(exercise(cancel))
        prefix = [
            'start', 'values', 'aiter', ('next', 'first'), ('wait', 'first'),
            ('resume', 'first'), ('step-finally', 'first'),
            ('next', 'second'), ('wait', 'second'),
        ]
        suffix = (
            [('step-finally', 'second'), ('error', 'CancelledError'), ('finally', 'first')]
            if cancel else
            [('resume', 'second'), ('step-finally', 'second'), 'stop',
             ('result', ('first', 'second')), ('finally', 'second')]
        )
        assert events == prefix + suffix, events
        # All coroutine, result, closure and exception handles are now out of
        # scope. Only quiescent eventual cleanup is compared, not drop order.
        gc.collect()
        assert set(references) == {'first', 'second', 'iterator'}
        assert all(reference() is None for reference in references.values())
        assert sorted(finalized) == ['first', 'iterator', 'second']
        observations.append({
            'cancelled': cancel, 'events': events, 'finalized': sorted(finalized),
        })
    return observations


@pytest.fixture(scope='module')
def strict_async_comprehension_semantics_project(request, tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp('strict-async-comprehension-semantics'),
        {'async_comprehension_semantics.py': (
            'from __future__ import strict\n' + _ASYNC_COMPREHENSION_SEMANTIC_SOURCE
        )},
        modules={'async_comprehension_semantics': 'async_comprehension_semantics.py'},
        backend=getattr(request, 'param', 'soac'),
    )


@pytest.mark.parametrize(
    ('strict_async_comprehension_semantics_project', 'entry_interpreter'),
    [
        pytest.param('soac', False, id='compiled'),
        pytest.param('soac', True, id='entry'),
        pytest.param('cpython', False, id='cpython'),
    ],
    indirect=['strict_async_comprehension_semantics_project'],
    scope='module',
)
def test_async_comprehension_preserves_suspension_cancellation_and_outer_bindings(
    strict_async_comprehension_semantics_project, tmp_path, entry_interpreter,
):
    from tests._integration import stock_module

    with stock_module(
        tmp_path, 'ordinary_async_comprehension_semantics',
        _ASYNC_COMPREHENSION_SEMANTIC_SOURCE,
    ) as module:
        expected = _async_comprehension_semantic_observations(module)
    backend = strict_async_comprehension_semantics_project.backend
    validation = f"""
def validate_module(module):
    from tests.test_strict_entry_runtime import _async_comprehension_semantic_observations

    collect = module.collect

    def assert_factory_witness():
        assert module.collect is collect
        if {backend == 'soac'!r}:
            import ctypes
            from soac import _soac_ext

            metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            metadata.argtypes = [ctypes.py_object]
            metadata.restype = ctypes.c_void_p
            owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            owner.argtypes = [ctypes.py_object]
            owner.restype = ctypes.c_void_p
            assert metadata(collect), 'coroutine factory has no SOAC metadata'
            assert owner(collect), 'coroutine factory has no native strict owner'
            actual_entry = _soac_ext.strict_function_entry_kind(collect)
            assert actual_entry == 'generator_factory', actual_entry

    assert_factory_witness()
    actual = _async_comprehension_semantic_observations(module)
    assert actual == {expected!r}, (actual, {expected!r})
    assert_factory_witness()
"""
    strict_async_comprehension_semantics_project.run_case(
        'async_comprehension_semantics', validation, Path(__file__),
        entry_interpreter=entry_interpreter,
        backend=backend,
        # run_case's SOAC required_functions are synchronous entry witnesses.
        # The validator above authenticates the coroutine factory instead.
        required_functions=('collect',) if backend == 'cpython' else (),
    )
