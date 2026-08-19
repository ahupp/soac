from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


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


def _run_guarded_generator_worker(
    tmp_path: Path, module_name: str, work_dir: Path, mode: str
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
        stock_globals = {
            "__name__": "stock_guarded_generator_control",
            "__builtins__": builtins.__dict__,
        }
        exec(compile(source, "<stock-guarded-generator-control>", "exec"), stock_globals)

        sys.path.insert(0, root)
        from soac import _soac_ext
        from soac.import_hook import install

        install()
        module = importlib.import_module(module_name)
        import soac.runtime as runtime

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
            assert next(first) is True
            assert next(second) is True
            assert first._resume_function is not second._resume_function

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

            for actual in (module.wrong_keyword, stock_globals["wrong_keyword"]):
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

            module.any = shadow_any
            try:
                module_shadow = module.consume_dynamic(module.any, (2, 3))
            finally:
                del module.any
            assert module_shadow == "shadow-any", shadow_calls

            helper_calls = []
            original_closed = runtime._is_generator_closed

            def observed_closed(owner):
                helper_calls.append("closed")
                return original_closed(owner)

            runtime._is_generator_closed = observed_closed
            try:
                assert module.consume_any((True,)) is True
            finally:
                runtime._is_generator_closed = original_closed
            assert helper_calls, helper_calls

            original_resume = runtime.resume_generator

            def observed_resume(*arguments):
                helper_calls.append("resume")
                return original_resume(*arguments)

            runtime.resume_generator = observed_resume
            try:
                assert module.consume_any((True,)) is True
            finally:
                runtime.resume_generator = original_resume
            assert "resume" in helper_calls, helper_calls

            original_reraise = runtime._reraise_control_flow

            def observed_reraise(error):
                helper_calls.append(type(error).__name__)
                return original_reraise(error)

            runtime._reraise_control_flow = observed_reraise
            try:
                assert module.consume_any(()) is False
            finally:
                runtime._reraise_control_flow = original_reraise
            assert "StopIteration" in helper_calls, helper_calls

            monitored_codes = [
                module.consume_any.__code__,
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
                        assert module.consume_any(()) is False
                    finally:
                        monitoring.set_local_events(tool_id, code, 0)

                monitoring.set_events(tool_id, monitoring.events.PY_START)
                try:
                    assert module.consume_any((0, 1)) is True
                finally:
                    monitoring.set_events(tool_id, 0)
            finally:
                monitoring.set_events(tool_id, 0)
                for code in monitored_codes:
                    monitoring.set_local_events(tool_id, code, 0)
                monitoring.register_callback(tool_id, monitoring.events.PY_START, None)
                monitoring.free_tool_id(tool_id)

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
                assert module.consume_any((0, 1)) is True
            finally:
                sys.settrace(None)

            def profiler(frame, event, argument):
                if event == "call" and frame.f_code.co_name in {"__next__", "send"}:
                    observed_calls["profile"].append(frame.f_code.co_name)

            sys.setprofile(profiler)
            try:
                assert module.consume_all((1, 0)) is False
            finally:
                sys.setprofile(None)
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
            assert forced_exhaustion["checks"], forced_exhaustion

            generator_class = runtime.ClosureGenerator
            original_iter = generator_class.__iter__
            class_events = []

            def replacement_iter(owner):
                class_events.append("iter")
                return original_iter(owner)

            generator_class.__iter__ = replacement_iter
            try:
                assert module.consume_any((1,)) is True
            finally:
                generator_class.__iter__ = original_iter
            assert class_events == ["iter"], class_events

            original_next = generator_class.__next__

            def replacement_next(owner):
                class_events.append("next")
                return original_next(owner)

            generator_class.__next__ = replacement_next
            try:
                assert module.consume_all((1, 0)) is False
            finally:
                generator_class.__next__ = original_next
            assert class_events.count("next") == 2, class_events

            original_send = generator_class.send

            def replacement_send(owner, value):
                class_events.append("send")
                return original_send(owner, value)

            generator_class.send = replacement_send
            try:
                assert module.consume_any((0, 1)) is True
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
                assert module.consume_any(
                    (MutatingTruth(False), MutatingTruth(True))
                ) is True
            finally:
                generator_class.send = original_send
            assert class_events.count("send") == before + 1, class_events

            class ReplacementGenerator(generator_class):
                def __iter__(self):
                    class_events.append("replacement-generator")
                    return super().__iter__()

            runtime.ClosureGenerator = ReplacementGenerator
            try:
                assert module.consume_any((1,)) is True
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
                assert module.consume_any((1,)) is True
            finally:
                original_send.__code__ = original_send_code
                code_events = list(builtins._soac_generator_code_calls)
                del builtins._soac_generator_code_calls
                del builtins._soac_generator_original_send
            assert code_events == ["send-code"], code_events

            short_generator = module.make_generator((True,))
            short_generator._preserved_values = _soac_ext.make_preserved_state(
                (), ()
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
            replacement_generator = module.make_generator((False,))
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
                module.make_generator(ReplaceActiveGeneratorState())
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
                    lambda: module.consume_any(PromoteGlobalsThenRaise())
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
            assert not watcher_errors, watcher_errors
        finally:
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
    )
    environment = {
        **os.environ,
        "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
        "SOAC_WORK_DIR": str(work_dir),
        "SOAC_OPT_MODE": mode,
        "SOAC_COMPILE_MODE": "eager",
        "SOAC_BACKGROUND_JIT": "0",
    }
    completed = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env=environment,
        timeout=90,
    )
    assert completed.returncode == 0, (
        f"{mode} transformed generator-consumer subprocess failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    return json.loads(completed.stdout.splitlines()[-1])


def test_generator_any_all_match_cpython_exhaustion_and_observable_fallbacks(
    tmp_path: Path,
) -> None:
    module_name = "guarded_generator_builtin_consumption_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(_MODULE_SOURCE), encoding="utf-8"
    )
    work_dir = tmp_path / "soac-work"
    results = {
        mode: _run_guarded_generator_worker(tmp_path, module_name, work_dir, mode)
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
