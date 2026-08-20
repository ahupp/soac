from __future__ import annotations

import ast
import json
from pathlib import Path
import textwrap

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import (
    _VALIDATION_PRELUDE,
    assert_strict_source_rejected,
    create_strict_project,
)


_MODULE_SOURCE = """
def consume_any(values):
    return any(value for value in values)


def consume_all(values):
    return all(value for value in values)


def consume_filtered(values):
    return any(value for value in values if value is not None)


def consume_captured(marker, values):
    return any(value is marker for value in values)


def consume_iterator(iterator):
    return any(iterator)


def consume_dynamic(consumer, values):
    return consumer(value for value in values)


def make_generator(values):
    return (value for value in values)


def make_captured(marker, values):
    return (value is marker for value in values)


def raise_error(error):
    raise error


def consume_error(error):
    return any(raise_error(error) for _ in (0,))


def wrong_keyword(values):
    return any(iterable=values)


def increment(value):
    return value + 1


def unrelated(value):
    return increment(value)
"""


def _selected_interop_source() -> str:
    """A separate counterpart, not admission of the invalid original module."""
    source = textwrap.dedent(_MODULE_SOURCE)
    functions = ast.parse(source).body
    assert all(isinstance(node, ast.FunctionDef) for node in functions)
    rejected = [node for node in functions if node.name == "wrong_keyword"]
    assert len(rejected) == 1
    # Preserve the exact original function bodies. The full original is checked
    # for rejection below and remains the ordinary binding-error control.
    return "\n\n".join(
        ast.get_source_segment(source, node)
        for node in functions
        if node.name != "wrong_keyword"
    ) + "\n"


def _run_guarded_generator_worker(
    project, tmp_path: Path, module_name: str, work_dir: Path, mode: str
) -> dict:
    script = textwrap.dedent(
        """
        import builtins
        import ctypes
        import gc
        import importlib
        import json
        import sys
        import types

        module_name = __MODULE_NAME__
        root = __MODULE_ROOT__
        source = open(root + "/" + module_name + ".py", encoding="utf-8").read()
        from tests._integration import stock_module
        from soac import _soac_ext
        import soac.runtime as runtime

        with stock_module(Path(root), "stock_guarded_generator_control", source) as stock:
            stock_globals = vars(stock)
            for name, function in stock_globals.items():
                if type(function) is types.FunctionType:
                    assert owner(function) is None and metadata(function) is None, name

        module = importlib.import_module(module_name)

        def assert_selected():
            diagnostic = _soac_ext.strict_module_diagnostics(module)
            assert diagnostic is not None, "counter subject has no strict source owner"
            assert diagnostic["sealed"] is True
            assert diagnostic["backend"] == "soac"
            assert diagnostic["module_name"] == module_name
            assert diagnostic["source_path"] == __SELECTED_SOURCE__
            assert diagnostic["artifact_generation"] == __GENERATION__
            assert diagnostic["initializer_entry_kind"] == "entry_interpreter"
            witnesses = []
            for name in (
                "consume_any", "consume_all", "consume_filtered", "consume_captured",
                "consume_iterator", "consume_dynamic", "make_generator",
                "make_captured", "raise_error", "consume_error", "increment", "unrelated",
            ):
                function = vars(module)[name]
                assert owner(function) and metadata(function), name
                actual_entry = _soac_ext.strict_function_entry_kind(function)
                assert actual_entry == "checked_native", (name, actual_entry)
                witnesses.append((name, id(function), owner(function)))
            return tuple(witnesses)

        selected_owners = assert_selected()
        valid_capsule = ctypes.pythonapi.PyCapsule_IsValid
        valid_capsule.argtypes = [ctypes.py_object, ctypes.c_char_p]
        valid_capsule.restype = ctypes.c_int
        matches_owner = ctypes.pythonapi.PyGen_MatchesSoacOwner
        matches_owner.argtypes = [ctypes.py_object, ctypes.py_object]
        matches_owner.restype = ctypes.c_int

        def source_capsule(generator, function):
            assert type(generator) is types.GeneratorType
            (code,) = (
                value for value in function.__code__.co_consts
                if type(value) is types.CodeType and value.co_name == "<genexpr>"
            )
            assert generator.gi_code is code
            (capsule,) = (
                value for value in gc.get_referents(generator)
                if valid_capsule(value, b"soac.PreservedState")
            )
            assert gc.is_tracked(capsule) and matches_owner(generator, capsule) == 1
            return capsule

        watched = {"stock": [], "soac": []}
        watcher_errors = []
        watcher_type = ctypes.CFUNCTYPE(
            ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p
        )

        @watcher_type
        def watch(event, pointer, _new_value):
            if event != 0:
                return 0
            try:
                function = ctypes.cast(pointer, ctypes.py_object).value
                if function.__code__.co_name != "<genexpr>":
                    return 0
                if function.__globals__ is stock_globals:
                    watched["stock"].append(function.__code__.co_qualname)
                elif function.__globals__ is module.__dict__:
                    watched["soac"].append(function.__code__.co_qualname)
            except BaseException as error:
                watcher_errors.append(type(error).__name__)
            return 0

        add_watcher = ctypes.pythonapi.PyFunction_AddWatcher
        add_watcher.argtypes = [watcher_type]
        add_watcher.restype = ctypes.c_int
        clear_watcher = ctypes.pythonapi.PyFunction_ClearWatcher
        clear_watcher.argtypes = [ctypes.c_int]
        clear_watcher.restype = ctypes.c_int
        watcher_id = add_watcher(watch)
        assert watcher_id >= 0, watcher_id

        def record_outcome(call):
            try:
                return {"result": call(), "error": None}
            except BaseException as error:
                return {
                    "result": None,
                    "error": type(error).__name__,
                    "message": str(error),
                }

        def observe_empty_cancellation(any_call, all_call, *, raising):
            cancellation_checks = []

            class CancellationMeta(type):
                def __instancecheck__(cls, value):
                    cancellation_checks.append(type(value).__name__)
                    if raising:
                        raise RuntimeError("unexpected asyncio cancellation check")
                    return False

            class FakeCancelled(BaseException, metaclass=CancellationMeta):
                pass

            fake_asyncio = types.ModuleType("asyncio")
            fake_asyncio.CancelledError = FakeCancelled
            missing = object()
            previous = sys.modules.get("asyncio", missing)
            sys.modules["asyncio"] = fake_asyncio
            try:
                outcomes = [record_outcome(any_call), record_outcome(all_call)]
            finally:
                if previous is missing:
                    sys.modules.pop("asyncio", None)
                else:
                    sys.modules["asyncio"] = previous

            return {"outcomes": outcomes, "checks": cancellation_checks}

        native_resume = runtime.resume_generator
        try:
            assert stock_globals["consume_any"]((0, 1, 2)) is True
            assert stock_globals["consume_all"]((1, 1, 0)) is False
            stock_created = list(watched["stock"])
            assert module.consume_any((0, 1, 2)) is True
            assert module.consume_all((1, 1, 0)) is False
            soac_created = list(watched["soac"])
            assert len(stock_created) == len(soac_created) == 2, (
                stock_created,
                soac_created,
            )

            marker = object()
            for index in range(40):
                assert module.consume_any((0, index, 1)) is True
                assert module.consume_all((1, 1, index != 0)) is (index != 0)
                assert module.consume_filtered((None, 0, 1)) is True
                assert module.consume_captured(marker, (None, marker)) is True
                assert module.unrelated(index) == index + 1

            first = module.make_captured(marker, (marker,))
            second_marker = object()
            second = module.make_captured(second_marker, (second_marker,))
            assert source_capsule(first, module.make_captured) is not source_capsule(
                second, module.make_captured
            )
            assert next(first) is True
            assert next(second) is True

            stock_exhaustion = observe_empty_cancellation(
                lambda: stock_globals["consume_any"](()),
                lambda: stock_globals["consume_all"](()),
                raising=True,
            )
            assert stock_exhaustion == {
                "outcomes": [
                    {"result": False, "error": None},
                    {"result": True, "error": None},
                ],
                "checks": [],
            }, stock_exhaustion
            soac_exhaustion = observe_empty_cancellation(
                lambda: module.consume_any(()),
                lambda: module.consume_all(()),
                raising=True,
            )

            class Probe:
                def __init__(self, value, events):
                    self.value = value
                    self.events = events

                def __bool__(self):
                    self.events.append(("bool", self.value))
                    return self.value

                def __del__(self):
                    self.events.append(("drop", self.value))

            class Source:
                def __init__(self, values, events):
                    self.values = iter(values)
                    self.events = events
                    self.iter_calls = 0

                def __iter__(self):
                    self.iter_calls += 1
                    self.events.append(("iter", self.iter_calls))
                    return self

                def __next__(self):
                    value = next(self.values)
                    self.events.append(("next", value))
                    return Probe(value, self.events)

            lifetime_events = []
            source_values = Source((False, True, True), lifetime_events)
            assert module.consume_any(source_values) is True
            assert source_values.iter_calls == 1, lifetime_events
            assert [event for event in lifetime_events if event[0] == "bool"] == [
                ("bool", False),
                ("bool", True),
            ], lifetime_events
            gc.collect()
            assert lifetime_events.count(("drop", False)) == 1, lifetime_events
            # The existing transformed generator may retain its last yielded
            # object until the surrounding activation is released.
            assert lifetime_events.count(("drop", True)) <= 1, lifetime_events

            # The original invalid builtin call is ordinary interoperability,
            # not an admitted source function with its checker error hidden.
            for actual in (
                stock_globals["wrong_keyword"],
                lambda values: module.consume_dynamic(stock_globals["wrong_keyword"], values),
            ):
                outcome = record_outcome(lambda: actual((1,)))
                assert outcome["error"] == "TypeError", outcome
            for error in (ValueError("body error"), GeneratorExit("closing")):
                expected = record_outcome(
                    lambda: stock_globals["consume_error"](error)
                )
                actual = record_outcome(lambda: module.consume_error(error))
                assert actual["error"] == expected["error"], (actual, expected)
                assert actual["message"] == expected["message"], (actual, expected)

            original_any = builtins.any
            original_all = builtins.all
            shadow_calls = []

            def shadow_any(iterator):
                shadow_calls.append(("any", tuple(iterator)))
                return "shadow-any"

            def shadow_all(iterator):
                shadow_calls.append(("all", tuple(iterator)))
                return "shadow-all"

            builtins.any = shadow_any
            try:
                shadow_any_result = module.consume_dynamic(builtins.any, (0, 1))
            finally:
                builtins.any = original_any
            builtins.all = shadow_all
            try:
                shadow_all_result = module.consume_dynamic(builtins.all, (1, 0))
            finally:
                builtins.all = original_all
            assert (shadow_any_result, shadow_all_result) == (
                "shadow-any",
                "shadow-all",
            ), shadow_calls
            assert shadow_calls == [("any", (0, 1)), ("all", (1, 0))]

            # Preserve the original ordinary append/delete control. A first
            # appended selected-module binding is final and cannot be removed.
            stock.any = shadow_any
            try:
                module_shadow = stock.consume_dynamic(stock.any, (2, 3))
            finally:
                del stock.any
            assert module_shadow == "shadow-any", shadow_calls

            def ordinary_wrapper(values):
                # Exercise the actual mutable Python wrapper, not source resume.
                iterator = iter(values)

                def resume(wrapper, state, sent, raised):
                    if raised is not runtime.NO_DEFAULT:
                        wrapper._preserved_values = runtime.make_preserved_state(
                            (None, 1), (0, 1), (),
                        )
                        raise raised
                    try:
                        return next(iterator)
                    except BaseException:
                        wrapper._preserved_values = runtime.make_preserved_state(
                            (None, 1), (0, 1), (),
                        )
                        raise

                assert owner(resume) is None and metadata(resume) is None
                wrapper = runtime.make_generator_instance(
                    resume, 0, "ordinary_wrapper_control", "ordinary_wrapper_control",
                    (None, 0), (0, 1), 0, 1, (),
                )
                assert type(wrapper) is runtime.ClosureGenerator
                assert wrapper._resume_function is resume
                return wrapper

            def ordinary_resume_dispatch(resume, wrapper, state, sent, raised):
                # A deliberately ordinary mutable dependency. It exercises
                # ClosureGenerator's real Python protocol without forging JIT
                # metadata, source authority or a native resume permit.
                assert owner(resume) is None and metadata(resume) is None
                return resume(wrapper, state, sent, raised)

            runtime.resume_generator = ordinary_resume_dispatch
            helper_calls = []
            original_closed = runtime._is_generator_closed

            def observed_closed(owner):
                helper_calls.append("closed")
                return original_closed(owner)

            runtime._is_generator_closed = observed_closed
            try:
                assert original_any(ordinary_wrapper((True,))) is True
            finally:
                runtime._is_generator_closed = original_closed
            assert helper_calls, helper_calls

            original_resume = runtime.resume_generator

            def observed_resume(*arguments):
                helper_calls.append("resume")
                return original_resume(*arguments)

            runtime.resume_generator = observed_resume
            try:
                assert original_any(ordinary_wrapper((True,))) is True
            finally:
                runtime.resume_generator = original_resume
            assert "resume" in helper_calls, helper_calls

            original_reraise = runtime._reraise_control_flow

            def observed_reraise(error):
                helper_calls.append(type(error).__name__)
                return original_reraise(error)

            runtime._reraise_control_flow = observed_reraise
            try:
                assert original_any(ordinary_wrapper(())) is False
            finally:
                runtime._reraise_control_flow = original_reraise
            assert "StopIteration" in helper_calls, helper_calls

            def ordinary_resume_control(*arguments):
                raise AssertionError("closed ordinary wrapper must never resume")

            def ordinary_helper_control():
                # These are direct ordinary Python calls. Observers must not
                # select a SOAC fallback merely to expose equivalent events.
                assert stock_globals["consume_any"]((0, 1)) is True
                wrapper = runtime.make_generator_instance(
                    ordinary_resume_control, 0, "ordinary_closed_control",
                    "ordinary_closed_control", (None, 1), (0, 1), 0, 1, (),
                )
                assert type(wrapper) is runtime.ClosureGenerator
                assert iter(wrapper) is wrapper
                assert original_closed(wrapper) is True
                assert next(wrapper, "closed") == "closed"
                marker = StopIteration("ordinary helper error")
                try:
                    original_reraise(marker)
                except StopIteration as error:
                    assert error is marker
                else:
                    raise AssertionError("ordinary helper lost its exception")

            monitored_codes = [
                stock_globals["consume_any"].__code__,
                runtime.ClosureGenerator.__iter__.__code__,
                runtime.ClosureGenerator.__next__.__code__,
                runtime.ClosureGenerator.send.__code__,
                original_closed.__code__,
                original_reraise.__code__,
            ]
            monitoring = sys.monitoring
            tool_id = next(
                identifier
                for identifier in range(6)
                if monitoring.get_tool(identifier) is None
            )
            monitor_events = []

            def monitor(code, offset):
                if code in monitored_codes:
                    monitor_events.append(code.co_qualname)

            monitoring.use_tool_id(tool_id, "soac.guarded-generator-consumption")
            try:
                monitoring.register_callback(
                    tool_id, monitoring.events.PY_START, monitor
                )
                for code in monitored_codes:
                    monitoring.set_local_events(
                        tool_id, code, monitoring.events.PY_START
                    )
                    try:
                        ordinary_helper_control()
                    finally:
                        monitoring.set_local_events(tool_id, code, 0)
                    assert code.co_qualname in monitor_events, monitor_events

                monitoring.set_events(tool_id, monitoring.events.PY_START)
                try:
                    ordinary_helper_control()
                finally:
                    monitoring.set_events(tool_id, 0)
            finally:
                monitoring.set_events(tool_id, 0)
                for code in monitored_codes:
                    monitoring.set_local_events(tool_id, code, 0)
                monitoring.register_callback(tool_id, monitoring.events.PY_START, None)
                monitoring.free_tool_id(tool_id)
            assert module.consume_any(()) is False
            assert module.consume_any((0, 1)) is True

            observed_calls = {"trace": [], "profile": []}

            def tracer(frame, event, argument):
                if event == "call" and frame.f_code.co_name in {
                    "__next__",
                    "send",
                    "<genexpr>",
                }:
                    observed_calls["trace"].append(frame.f_code.co_name)
                return tracer

            sys.settrace(tracer)
            try:
                ordinary_helper_control()
            finally:
                sys.settrace(None)
            assert {"__next__", "send", "<genexpr>"} <= set(
                observed_calls["trace"]
            ), observed_calls
            assert module.consume_any((0, 1)) is True

            def profiler(frame, event, argument):
                if event == "call" and frame.f_code.co_name in {"__next__", "send"}:
                    observed_calls["profile"].append(frame.f_code.co_name)

            sys.setprofile(profiler)
            try:
                ordinary_helper_control()
            finally:
                sys.setprofile(None)
            assert {"__next__", "send"} <= set(
                observed_calls["profile"]
            ), observed_calls
            assert module.consume_all((1, 0)) is False
            previous_force = _soac_ext.force_entry_interpreter_for_tests(True)
            try:
                forced_exhaustion = observe_empty_cancellation(
                    lambda: module.consume_any(()),
                    lambda: module.consume_all(()),
                    raising=False,
                )
            finally:
                _soac_ext.force_entry_interpreter_for_tests(previous_force)
            assert forced_exhaustion["outcomes"] == [
                {"result": False, "error": None},
                {"result": True, "error": None},
            ], forced_exhaustion
            # The native source protocol consumes terminal StopIteration in
            # either source backend; it must not consult cancellation hooks.
            assert forced_exhaustion["checks"] == stock_exhaustion["checks"] == [], (
                forced_exhaustion, stock_exhaustion
            )

            generator_class = runtime.ClosureGenerator
            original_iter = generator_class.__iter__
            class_events = []

            def replacement_iter(owner):
                class_events.append("iter")
                return original_iter(owner)

            generator_class.__iter__ = replacement_iter
            try:
                assert original_any(ordinary_wrapper((1,))) is True
            finally:
                generator_class.__iter__ = original_iter
            assert class_events == ["iter"], class_events

            original_next = generator_class.__next__

            def replacement_next(owner):
                class_events.append("next")
                return original_next(owner)

            generator_class.__next__ = replacement_next
            try:
                assert original_all(ordinary_wrapper((1, 0))) is False
            finally:
                generator_class.__next__ = original_next
            assert class_events.count("next") == 2, class_events

            original_send = generator_class.send

            def replacement_send(owner, value):
                class_events.append("send")
                return original_send(owner, value)

            generator_class.send = replacement_send
            try:
                assert original_any(ordinary_wrapper((0, 1))) is True
            finally:
                generator_class.send = original_send
            assert class_events.count("send") == 2, class_events

            class MutatingTruth:
                def __init__(self, result):
                    self.result = result

                def __bool__(self):
                    class_events.append(("truth", self.result))
                    if not self.result:
                        generator_class.send = replacement_send
                    return self.result

            try:
                before = class_events.count("send")
                assert original_any(ordinary_wrapper(
                    (MutatingTruth(False), MutatingTruth(True))
                )) is True
            finally:
                generator_class.send = original_send
            assert class_events.count("send") == before + 1, class_events

            class ReplacementGenerator(generator_class):
                def __iter__(self):
                    class_events.append("replacement-generator")
                    return super().__iter__()

            runtime.ClosureGenerator = ReplacementGenerator
            try:
                assert original_any(ordinary_wrapper((1,))) is True
            finally:
                runtime.ClosureGenerator = generator_class
            assert "replacement-generator" in class_events, class_events

            builtins._soac_generator_original_send = types.FunctionType(
                original_send.__code__,
                original_send.__globals__,
                original_send.__name__,
                original_send.__defaults__,
                original_send.__closure__,
            )
            builtins._soac_generator_code_calls = []

            def replacement_send_code(owner, value):
                import builtins

                builtins._soac_generator_code_calls.append("send-code")
                return builtins._soac_generator_original_send(owner, value)

            original_send_code = original_send.__code__
            try:
                original_send.__code__ = replacement_send_code.__code__
                assert original_any(ordinary_wrapper((1,))) is True
            finally:
                original_send.__code__ = original_send_code
                code_events = list(builtins._soac_generator_code_calls)
                del builtins._soac_generator_code_calls
                del builtins._soac_generator_original_send
            assert code_events == ["send-code"], code_events

            short_generator = ordinary_wrapper((True,))
            short_generator._preserved_values = _soac_ext.make_preserved_state(
                (), (), ()
            )
            short_outcome = record_outcome(
                lambda: module.consume_iterator(short_generator)
            )
            assert short_outcome == {
                "result": None,
                "error": "RuntimeError",
                "message": "preserved-state slot out of range",
            }, short_outcome

            mutation_events = []
            owner_holder = []
            replacement_generator = ordinary_wrapper((False,))
            original_no_default = runtime.NO_DEFAULT

            class ReplaceActiveGeneratorState:
                def __iter__(self):
                    return self

                def __next__(self):
                    owner = owner_holder[0]
                    owner._resume_function = (
                        replacement_generator._resume_function
                    )
                    owner._preserved_values = (
                        replacement_generator._preserved_values
                    )
                    runtime.NO_DEFAULT = object()
                    mutation_events.append("replaced-active-owners")
                    return True

            owner_holder.append(
                ordinary_wrapper(ReplaceActiveGeneratorState())
            )
            try:
                assert module.consume_iterator(owner_holder[0]) is True
            finally:
                runtime.NO_DEFAULT = original_no_default
            assert mutation_events == ["replaced-active-owners"], mutation_events

            original_reraise = runtime._reraise_control_flow

            def replacement_reraise(error):
                mutation_events.append(("current-helper", type(error).__name__))
                raise error

            class PromoteGlobalsThenRaise:
                def __iter__(self):
                    return self

                def __next__(self):
                    runtime._soac_generator_promoted_guard = object()
                    runtime._reraise_control_flow = replacement_reraise
                    raise ValueError("mutated generator body")

            try:
                promoted_outcome = record_outcome(
                    lambda: module.consume_iterator(ordinary_wrapper(PromoteGlobalsThenRaise()))
                )
            finally:
                runtime._reraise_control_flow = original_reraise
                if hasattr(runtime, "_soac_generator_promoted_guard"):
                    del runtime._soac_generator_promoted_guard
            assert promoted_outcome == {
                "result": None,
                "error": "ValueError",
                "message": "mutated generator body",
            }, promoted_outcome
            assert mutation_events[-1] == ("current-helper", "ValueError"), (
                mutation_events
            )
            runtime.resume_generator = native_resume
            # Append only after the original unshadowed workload is complete.
            # Do not revoke a newly final binding merely to reset the fixture.
            from soac import StrictMutationError
            module.any = shadow_any
            assert module.consume_dynamic(module.any, (2, 3)) == "shadow-any"
            assert module.consume_any((2, 3)) == "shadow-any"
            try:
                del module.any
            except StrictMutationError:
                pass
            else:
                raise AssertionError("selected appended binding accepted deletion")
            try:
                module.any = shadow_all
            except StrictMutationError:
                pass
            else:
                raise AssertionError("selected appended binding accepted replacement")
            assert module.any is shadow_any and vars(module)["any"] is shadow_any
            assert assert_selected() == selected_owners
            assert not watcher_errors, watcher_errors
        finally:
            runtime.resume_generator = native_resume
            assert clear_watcher(watcher_id) == 0

        print(
            json.dumps(
                {
                    "mode": __MODE__,
                    "stock_created": stock_created,
                    "soac_created": soac_created,
                    "stock_exhaustion": stock_exhaustion,
                    "soac_exhaustion": soac_exhaustion,
                    "forced_exhaustion": forced_exhaustion,
                    "helper_calls": helper_calls,
                    "monitor_events": monitor_events,
                    "class_events": class_events,
                    "code_events": code_events,
                }
            )
        )
        """
    )
    script = (
        script.replace("__MODULE_NAME__", repr(module_name))
        .replace("__MODULE_ROOT__", repr(str(tmp_path)))
        .replace("__MODE__", repr(mode))
        .replace("__SELECTED_SOURCE__", repr(str(project.project / f"{module_name}.py")))
        .replace("__GENERATION__", repr(project.publication["generation"]))
    )
    completed = project.run(
        _VALIDATION_PRELUDE + script,
        opt_mode=mode,
        extra_env={"SOAC_WORK_DIR": str(work_dir)},
        timeout=90,
        check=False,
    )
    assert completed.returncode == 0, (
        f"{mode} transformed generator-consumer subprocess failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    return json.loads(completed.stdout.splitlines()[-1])


def test_generator_any_all_preserve_exhaustion_mutations_and_ordinary_observers(
    tmp_path: Path,
) -> None:
    module_name = "guarded_generator_builtin_consumption_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(_MODULE_SOURCE), encoding="utf-8"
    )
    relative = f"{module_name}.py"
    # Keep the complete invalid original as a real checker rejection. A
    # different diagnostic must not stand in for this positional-only error.
    assert_strict_source_rejected(
        tmp_path / "original-rejection",
        strict_opt_in(textwrap.dedent(_MODULE_SOURCE).encode(), relative)[0].decode(),
        module_name=module_name,
        diagnostic="positional-only-parameter-as-kwarg",
    )
    project = create_strict_project(
        tmp_path / "strict-counterpart",
        {relative: strict_opt_in(_selected_interop_source().encode(), relative)[0].decode()},
        modules={module_name: relative},
    )
    work_dir = tmp_path / "soac-work"
    results = {
        mode: _run_guarded_generator_worker(project, tmp_path, module_name, work_dir, mode)
        for mode in ("profile", "verify", "apply")
    }

    from soac import _soac_ext

    profile = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    records = [
        record
        for record in profile["records"]
        if record["module_name"] == module_name
    ]
    assert records, profile
    assert any(
        row["kind"] == "call_hot_targets"
        and row["function_qualname"] == "unrelated"
        and row["value"] >= 40
        for record in records
        for row in record["rows"]
    ), records

    code_summary = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text(
            encoding="utf-8"
        ).splitlines()
        if line.strip()
    ]
    for name in ("consume_any", "consume_all"):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == name
            for row in code_summary
        ), (name, code_summary)
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == f"{name}.<locals>.<genexpr>"
            for row in code_summary
        ), (name, code_summary)

    for mode, result in results.items():
        assert len(result["stock_created"]) == len(result["soac_created"]) == 2
        assert result["soac_exhaustion"] == result["stock_exhaustion"], (
            "CPython any/all consume ordinary generator StopIteration without "
            "consulting unrelated asyncio cancellation hooks",
            mode,
            result["stock_exhaustion"],
            result["soac_exhaustion"],
        )
