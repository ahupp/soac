from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


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


def _run_eager_worker(
    tmp_path: Path, module_name: str, work_dir: Path, mode: str
) -> tuple[dict, Path]:
    event_path = work_dir / f"{mode}-events.jsonl"
    script = textwrap.dedent(
        """
        import builtins as _builtins
        import ctypes
        import gc
        import importlib
        import json
        import sys
        import types

        module_name = __MODULE_NAME__
        root = __MODULE_ROOT__
        source = open(root + "/" + module_name + ".py", encoding="utf-8").read()
        stock_globals = {"__name__": "stock_eager_control", "__builtins__": _builtins.__dict__}
        exec(compile(source, "<stock-eager-control>", "exec"), stock_globals)

        sys.path.insert(0, root)
        from soac import _soac_ext
        from soac.import_hook import install
        install()
        module = importlib.import_module(module_name)
        import soac.bootstrap as bootstrap
        import soac.runtime as runtime

        names = {"<listcomp>", "<setcomp>", "<dictcomp>", "<genexpr>"}
        watched = {"stock": [], "soac": []}
        audits = {"stock": [], "soac": [], "controls": []}
        errors = []
        phase = "stock"

        def audit(event, args):
            if event == "code.__new__" and len(args) >= 3 and args[2] in names:
                audits[phase].append(args[2])

        sys.addaudithook(audit)
        watcher_type = ctypes.CFUNCTYPE(
            ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p
        )

        @watcher_type
        def watch(event, pointer, _new_value):
            if event != 0:
                return 0
            try:
                function = ctypes.cast(pointer, ctypes.py_object).value
                code = function.__code__
                if code.co_name in names:
                    if function.__globals__ is stock_globals:
                        watched["stock"].append((code.co_name, code.co_qualname))
                    elif function.__globals__ is module.__dict__:
                        watched["soac"].append((code.co_name, code.co_qualname))
            except BaseException as error:
                errors.append(type(error).__name__)
            return 0

        add = ctypes.pythonapi.PyFunction_AddWatcher
        add.argtypes = [watcher_type]
        add.restype = ctypes.c_int
        clear = ctypes.pythonapi.PyFunction_ClearWatcher
        clear.argtypes = [ctypes.c_int]
        clear.restype = ctypes.c_int
        watcher_id = add(watch)
        assert watcher_id >= 0, watcher_id

        def synthetic_count():
            return sum(name != "<genexpr>" for name, _ in watched["soac"])

        try:
            expected = stock_globals["canonical"](3, (1, 2))
            stock_created = list(watched["stock"])
            stock_audits = list(audits["stock"])

            phase = "soac"
            assert module.canonical(3, (1, 2)) == expected
            canonical_created = list(watched["soac"])
            canonical_audits = list(audits["soac"])
            assert module.canonical(30, (1, 2)) == stock_globals["canonical"](
                30, (1, 2)
            )
            phase = "controls"

            for offset in range(32):
                assert module.mixed(offset) == [offset, offset + 1]
                assert module.unrelated(offset) == offset + 1

            original_factory = runtime.code_with_freevars
            assert original_factory is bootstrap.code_with_freevars
            callbacks = []

            def patched_factory(freevars, is_async, is_generator):
                callbacks.append(tuple(freevars))
                return original_factory(freevars, is_async, is_generator)

            runtime.code_with_freevars = patched_factory
            try:
                assert module.prepatched(5) == [5, 6]
                assert module.mixed(100) == [100, 101]
            finally:
                runtime.code_with_freevars = original_factory
            assert len(callbacks) == 2, callbacks

            assert module.postpatched(6) == [6, 7]
            runtime.code_with_freevars = patched_factory
            try:
                assert module.postpatched(60) == [60, 61]
            finally:
                runtime.code_with_freevars = original_factory
            assert len(callbacks) == 3, callbacks

            original_code = original_factory.__code__
            delegated_factory = types.FunctionType(
                original_code,
                original_factory.__globals__,
                original_factory.__name__,
                original_factory.__defaults__,
                original_factory.__closure__,
            )
            _builtins._soac_eager_delegate = delegated_factory
            _builtins._soac_eager_code_calls = []

            def alternate_code(freevars, is_async, is_generator):
                _builtins._soac_eager_code_calls.append(tuple(freevars))
                return _builtins._soac_eager_delegate(
                    freevars, is_async, is_generator
                )

            try:
                original_factory.__code__ = alternate_code.__code__
                assert module.modified_factory_code(7) == [7, 8]
                code_calls = list(_builtins._soac_eager_code_calls)
            finally:
                original_factory.__code__ = original_code
                del _builtins._soac_eager_delegate
                del _builtins._soac_eager_code_calls
            assert len(code_calls) == 1, code_calls

            runtime.code_with_freevars = patched_factory
            bootstrap.code_with_freevars = patched_factory
            try:
                assert module.both_factories_replaced(8) == [8, 9]
            finally:
                runtime.code_with_freevars = original_factory
                bootstrap.code_with_freevars = original_factory
            assert len(callbacks) == 4, callbacks

            original_cache = bootstrap._DP_CODE_WITH_FREEVARS_CACHE
            reentered = []

            class ReentrantCache(dict):
                active = False

                def get(self, key, default=None):
                    if not self.active:
                        self.active = True
                        try:
                            reentered.append(module.reentrant(80))
                        finally:
                            self.active = False
                    return super().get(key, default)

            bootstrap._DP_CODE_WITH_FREEVARS_CACHE = ReentrantCache(original_cache)
            try:
                assert module.reentrant(9) == [9, 10]
            finally:
                bootstrap._DP_CODE_WITH_FREEVARS_CACHE = original_cache
            assert reentered == [[80, 81]], reentered

            original_runtime = sys.modules["soac.runtime"]
            replacement = types.ModuleType("soac.runtime")
            replacement.__dict__.update(original_runtime.__dict__)
            replacement.code_with_freevars = patched_factory
            sys.modules["soac.runtime"] = replacement
            try:
                assert module.replaced_module(10) == [10, 11]
            finally:
                sys.modules["soac.runtime"] = original_runtime
            assert len(callbacks) == 5, callbacks

            stock_lazy = stock_globals["lazy"](12)
            soac_lazy = module.lazy(12)
            assert next(stock_lazy) == next(soac_lazy) == 12
            assert list(stock_lazy) == list(soac_lazy) == [13]
            assert any(name == "<genexpr>" for name, _ in watched["stock"])
            assert any(name == "<genexpr>" for name, _ in watched["soac"])

            first = module.source_function(13)
            second = module.source_function(130)
            assert isinstance(first, types.FunctionType)
            assert isinstance(second, types.FunctionType)
            assert first is not second and first(1) == 14 and second(1) == 131
            spoofed = module.spoofed_source_function(14)
            assert isinstance(spoofed, types.FunctionType)
            assert spoofed(1) == 15

            for fail in (False, True):
                expected_lifetime = stock_globals["lifetime"](fail)
                actual_lifetime = module.lifetime(fail)
                if fail:
                    assert expected_lifetime == (
                        None,
                        ("iter", "read", "caught", "drop-item", "drop-iterator"),
                    ), expected_lifetime
                    assert actual_lifetime == (
                        None,
                        ("iter", "read", "drop-item", "caught", "drop-iterator"),
                    ), actual_lifetime
                else:
                    assert actual_lifetime == expected_lifetime, (
                        actual_lifetime,
                        expected_lifetime,
                    )
                assert actual_lifetime[1].count("drop-item") == 1
                assert actual_lifetime[1].count("drop-iterator") == 1
            assert module.collect_cycle() == ([11, 11], 1)

            observer_counts = {}
            before = synthetic_count()

            def trace(frame, event, argument):
                return trace

            sys.settrace(trace)
            try:
                assert module.observed(20) == [20, 21]
            finally:
                sys.settrace(None)
            observer_counts["trace"] = synthetic_count() - before

            before = synthetic_count()

            def profile(frame, event, argument):
                return None

            sys.setprofile(profile)
            try:
                assert module.observed(21) == [21, 22]
            finally:
                sys.setprofile(None)
            observer_counts["profile"] = synthetic_count() - before

            monitoring = sys.monitoring
            tool_id = next(
                identifier
                for identifier in range(6)
                if monitoring.get_tool(identifier) is None
            )
            monitor_events = []

            def monitor(code, offset):
                if code is module.observed.__code__:
                    monitor_events.append(offset)

            monitoring.use_tool_id(tool_id, "soac.eager-comprehension-elision")
            try:
                monitoring.register_callback(
                    tool_id, monitoring.events.PY_START, monitor
                )
                monitoring.set_local_events(
                    tool_id, module.observed.__code__, monitoring.events.PY_START
                )
                before = synthetic_count()
                assert module.observed(22) == [22, 23]
                observer_counts["monitor-local"] = synthetic_count() - before
                monitoring.set_local_events(tool_id, module.observed.__code__, 0)

                monitoring.set_events(tool_id, monitoring.events.PY_START)
                try:
                    before = synthetic_count()
                    assert module.observed(23) == [23, 24]
                    observer_counts["monitor-global"] = synthetic_count() - before
                finally:
                    monitoring.set_events(tool_id, 0)
            finally:
                monitoring.set_events(tool_id, 0)
                monitoring.set_local_events(tool_id, module.observed.__code__, 0)
                monitoring.register_callback(tool_id, monitoring.events.PY_START, None)
                monitoring.free_tool_id(tool_id)
            assert all(count >= 1 for count in observer_counts.values()), (
                observer_counts
            )

            before = synthetic_count()
            previous = _soac_ext.force_entry_interpreter_for_tests(True)
            try:
                assert module.forced(24) == [24, 25]
            finally:
                _soac_ext.force_entry_interpreter_for_tests(previous)
            assert synthetic_count() > before
            assert errors == [], errors
        finally:
            assert clear(watcher_id) == 0

        print(json.dumps({
            "mode": __MODE__,
            "stock_created": stock_created,
            "stock_audits": stock_audits,
            "soac_created": canonical_created,
            "soac_audits": canonical_audits,
            "observer_counts": observer_counts,
            "factory_calls": len(callbacks),
            "factory_code_calls": len(code_calls),
        }))
        """
    )
    script = (
        script.replace("__MODULE_ROOT__", repr(str(tmp_path)))
        .replace("__MODULE_NAME__", repr(module_name))
        .replace("__MODE__", repr(mode))
    )
    env = {
        **os.environ,
        "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
        "SOAC_WORK_DIR": str(work_dir),
        "SOAC_OPT_MODE": mode,
        "SOAC_COMPILE_MODE": "eager",
        "SOAC_BACKGROUND_JIT": "0",
        "SOAC_LOG": f"soac_jit_direct_edges=info;json={event_path}",
    }
    completed = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env=env,
        timeout=90,
    )
    assert completed.returncode == 0, (
        f"{mode} transformed subprocess failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    return json.loads(completed.stdout.splitlines()[-1]), event_path


def test_eager_comprehensions_match_stock_function_and_code_creation(
    tmp_path: Path,
) -> None:
    module_name = "eager_comprehension_function_elision_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(_MODULE_SOURCE), encoding="utf-8"
    )
    work_dir = tmp_path / "soac-work"
    results = {}
    event_paths = {}

    for mode in ("profile", "verify", "apply"):
        results[mode], event_paths[mode] = _run_eager_worker(
            tmp_path, module_name, work_dir, mode
        )

    from soac import _soac_ext

    dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    records = [
        record for record in dump["records"] if record["module_name"] == module_name
    ]
    assert records, dump
    unrelated = [
        row
        for record in records
        for row in record["rows"]
        if row["kind"] == "call_hot_targets"
        and row["function_qualname"] == "unrelated"
        and row["value"] >= 32
    ]
    assert unrelated, records

    summary = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text(
            encoding="utf-8"
        ).splitlines()
        if line.strip()
    ]
    assert any(
        row.get("entry_kind") == "direct_function_body"
        and row.get("function_qualname") == "canonical"
        for row in summary
    ), summary
    assert any(
        row.get("entry_kind") == "direct_function_body"
        and row.get("function_qualname", "").startswith("canonical.<locals>.")
        for row in summary
    ), summary
    assert any(
        row.get("entry_kind") == "direct_function_body"
        and row.get("function_qualname") == "unrelated"
        for row in summary
    ), summary

    for mode, result in results.items():
        assert result["stock_created"] == [], result
        assert result["stock_audits"] == [], result
        assert (result["soac_created"], result["soac_audits"]) == ([], []), (
            "CPython inlines eager list, set, and dict comprehensions without "
            "creating throwaway function objects or audited code objects",
            mode,
            result,
        )

    for mode in ("verify", "apply"):
        events = [
            event
            for line in event_paths[mode].read_text(encoding="utf-8").splitlines()
            if line.strip()
            if (event := json.loads(line)).get("target") == "soac_jit_direct_edges"
            and event.get("module") == module_name
        ]
        assert any(
            event.get("qualname") != "mixed"
            and event.get("clif_direct_edges", 0) > 0
            for event in events
        ), events
        assert not any(event.get("qualname") == "mixed" for event in events), events
