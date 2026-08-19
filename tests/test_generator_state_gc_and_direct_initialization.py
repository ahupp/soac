from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


_SOURCE = """
def make(values):
    return (value for value in values)

def captured(marker, values):
    return (marker + value for value in values)

def child(value):
    return value + 1

def hot(value):
    return child(value)
"""


def _worker(tmp_path: Path, module_name: str, work_dir: Path, mode: str) -> dict:
    events = work_dir / f"{mode}-events.jsonl"
    script = textwrap.dedent(
        """
        import builtins, ctypes, gc, importlib, json, sys, types, weakref
        root, name = __ROOT__, __NAME__
        source = open(root + "/" + name + ".py", encoding="utf-8").read()
        stock = {"__name__": "stock_generator_state", "__builtins__": builtins.__dict__}
        exec(compile(source, "<stock-generator-state>", "exec"), stock)
        sys.path.insert(0, root)
        from soac import _soac_ext
        from soac.import_hook import install
        install()
        module = importlib.import_module(name)
        import soac.runtime as runtime

        watched = {"stock": 0, "soac": 0}
        callback_type = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
        @callback_type
        def watcher(event, pointer, _value):
            if event == 0:
                function = ctypes.cast(pointer, ctypes.py_object).value
                if function.__code__.co_name == "<genexpr>":
                    if function.__globals__ is stock:
                        watched["stock"] += 1
                    elif function.__globals__ is module.__dict__:
                        watched["soac"] += 1
            return 0
        add, clear = ctypes.pythonapi.PyFunction_AddWatcher, ctypes.pythonapi.PyFunction_ClearWatcher
        add.argtypes, add.restype = [callback_type], ctypes.c_int
        clear.argtypes, clear.restype = [ctypes.c_int], ctypes.c_int
        watcher_id = add(watcher)
        assert watcher_id >= 0
        try:
            assert list(stock["make"]((1, 2))) == list(module.make((1, 2))) == [1, 2]
        finally:
            assert clear(watcher_id) == 0
        assert watched == {"stock": 1, "soac": 1}, watched

        first, second = module.captured(3, (1, 2)), module.captured(30, (1, 2))
        assert first is not second and first.gi_code is second.gi_code
        assert first.__name__ is first.gi_code.co_name
        assert first.__qualname__ is first.gi_code.co_qualname
        assert (next(first), next(second), next(first), next(second)) == (4, 31, 5, 32)
        if hasattr(first, "_resume_function"):
            assert first._resume_function.__closure__[0] is not second._resume_function.__closure__[0]
        closing = module.make((3, 4))
        assert next(closing) == 3 and closing.close() is None
        assert next(closing, "closed") == "closed"
        throwing = module.make((4, 5))
        assert next(throwing) == 4
        try:
            throwing.throw(ValueError("expected"))
        except ValueError as error:
            assert str(error) == "expected"
        else:
            raise AssertionError("generator.throw disappeared")

        marker = object()
        values = tuple(marker if index % 2 == 0 else index for index in range(70))
        kinds = tuple(index % 2 for index in range(70))
        state = runtime.make_preserved_state(values, kinds)
        assert runtime.load_preserved_state(state, 0) is marker
        assert runtime.load_preserved_state(state, 63) == 63
        assert runtime.load_preserved_state(state, 69) == 69
        public_state_tracked = gc.is_tracked(state)
        public_state_visible = any(value is marker for value in gc.get_referents(state))
        del state

        generator_class = runtime.ClosureGenerator
        original_init = generator_class.__init__
        monitor_events = []
        monitoring = sys.monitoring
        tool = next(index for index in range(6) if monitoring.get_tool(index) is None)
        def monitor(code, _offset):
            if code is original_init.__code__:
                monitor_events.append(code.co_name)
        monitoring.use_tool_id(tool, "soac.generator-state-gc")
        try:
            monitoring.register_callback(tool, monitoring.events.PY_START, monitor)
            monitoring.set_local_events(tool, original_init.__code__, monitoring.events.PY_START)
            try:
                assert list(module.make((5,))) == [5]
            finally:
                monitoring.set_local_events(tool, original_init.__code__, 0)
        finally:
            monitoring.set_events(tool, 0)
            monitoring.register_callback(tool, monitoring.events.PY_START, None)
            monitoring.free_tool_id(tool)
        if __MODE__ == "apply":
            assert monitor_events, monitor_events

        changes = []
        def replacement_init(owner, *args):
            changes.append("init")
            return original_init(owner, *args)
        generator_class.__init__ = replacement_init
        try:
            assert list(module.make((6,))) == [6]
        finally:
            generator_class.__init__ = original_init

        delegate = types.FunctionType(original_init.__code__, original_init.__globals__)
        builtins._soac_generator_init_delegate = delegate
        builtins._soac_generator_init_events = changes
        def replacement_code(owner, *args):
            import builtins
            builtins._soac_generator_init_events.append("code")
            return builtins._soac_generator_init_delegate(owner, *args)
        old_code = original_init.__code__
        try:
            original_init.__code__ = replacement_code.__code__
            assert list(module.make((7,))) == [7]
        finally:
            original_init.__code__ = old_code
            del builtins._soac_generator_init_delegate
            del builtins._soac_generator_init_events

        missing = object()
        old_setattr = generator_class.__dict__.get("__setattr__", missing)
        def replacement_setattr(owner, key, value):
            changes.append("setattr:" + key)
            return object.__setattr__(owner, key, value)
        generator_class.__setattr__ = replacement_setattr
        try:
            assert list(module.make((9,))) == [9]
        finally:
            if old_setattr is missing:
                del generator_class.__setattr__
            else:
                generator_class.__setattr__ = old_setattr
        assert {"init", "code"}.issubset(changes), changes
        assert "setattr:_preserved_values" in changes, changes

        class Replacement(generator_class):
            __slots__ = ()

            def __new__(owner, *args, **kwargs):
                changes.append("new")
                return object.__new__(owner)
        runtime.ClosureGenerator = Replacement
        try:
            instance = module.make((10,))
            assert type(instance) is Replacement and list(instance) == [10]
        finally:
            runtime.ClosureGenerator = generator_class
        assert "new" in changes, changes

        previous = _soac_ext.force_entry_interpreter_for_tests(True)
        try:
            assert list(module.make((11,))) == [11]
        finally:
            _soac_ext.force_entry_interpreter_for_tests(previous)

        resurrection, finalizers = [], []
        class Resurrection:
            def __iter__(self): return self
            def __next__(self): raise StopIteration
            def __del__(self):
                finalizers.append("finalized")
                resurrection.append(self)
        iterator = Resurrection()
        instance = module.make(iterator)
        del iterator, instance
        gc.collect()
        assert finalizers == ["finalized"]
        resurrection.clear()
        gc.collect()
        assert finalizers == ["finalized"]

        def collect_cycle(factory):
            events = []
            class Iterator:
                def __iter__(self): return self
                def __next__(self): raise StopIteration
                def __del__(self): events.append("released")
            iterator = Iterator()
            generator = factory(iterator)
            capsule = getattr(generator, "_preserved_values", None)
            tracked = None if capsule is None else gc.is_tracked(capsule)
            iterator.generator = generator
            reference = weakref.ref(iterator)
            del capsule, generator, iterator
            original_debug = gc.get_debug()
            try:
                gc.set_debug(0)
                gc.collect()
            finally:
                gc.set_debug(original_debug)
            collected, released = reference() is None, list(events)
            survivor = reference()
            if survivor is not None:
                survivor.generator = None
                del survivor
                gc.collect()
            return {"collected": collected, "tracked": tracked, "finalizers": released}

        stock_cycle, soac_cycle = collect_cycle(stock["make"]), collect_cycle(module.make)
        for value in range(24):
            assert module.hot(value) == value + 1
        print(json.dumps({"mode": __MODE__, "stock_cycle": stock_cycle,
            "soac_cycle": soac_cycle, "public_state_tracked": public_state_tracked,
            "public_state_visible": public_state_visible, "watched": watched,
            "monitor_events": monitor_events, "changes": changes,
            "finalizers": finalizers}))
        """
    )
    script = script.replace("__ROOT__", repr(str(tmp_path)))
    script = script.replace("__NAME__", repr(module_name)).replace("__MODE__", repr(mode))
    environment = {
        **os.environ,
        "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
        "SOAC_WORK_DIR": str(work_dir),
        "SOAC_OPT_MODE": mode,
        "SOAC_COMPILE_MODE": "eager",
        "SOAC_BACKGROUND_JIT": "0",
        "SOAC_LOG": f"soac_generator_direct_state=debug;json={events}",
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
        f"{mode} transformed generator-state subprocess failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    result = json.loads(completed.stdout.splitlines()[-1])
    result["direct_events"] = [
        event
        for line in events.read_text(encoding="utf-8").splitlines()
        if (event := json.loads(line))["target"] == "soac_generator_direct_state"
    ]
    return result


def test_generator_state_gc_and_direct_initialization(tmp_path: Path) -> None:
    module_name = "generator_state_gc_and_direct_initialization_case"
    (tmp_path / f"{module_name}.py").write_text(textwrap.dedent(_SOURCE))
    work_dir = tmp_path / "soac-work"
    results = {
        mode: _worker(tmp_path, module_name, work_dir, mode)
        for mode in ("profile", "verify", "apply")
    }

    from soac import _soac_ext

    dump = json.loads(_soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin")))
    records = [record for record in dump["records"] if record["module_name"] == module_name]
    assert any(
        row["kind"] == "call_hot_targets"
        and row["function_qualname"] == "hot"
        and row["value"] >= 24
        for record in records
        for row in record["rows"]
    ), records

    native = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text().splitlines()
        if line.strip()
    ]
    for name in ("make", "captured", "hot", "child"):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == name
            for row in native
        ), (name, native)

    for mode, result in results.items():
        assert result["stock_cycle"] == {
            "collected": True, "tracked": None, "finalizers": ["released"]
        }, (mode, result)
        assert result["soac_cycle"]["collected"], (
            "generator preserved-state object references must be visible to cyclic GC",
            mode,
            result["stock_cycle"],
            result["soac_cycle"],
        )
        assert result["soac_cycle"]["tracked"]
        assert result["soac_cycle"]["finalizers"] == ["released"]
        assert result["public_state_tracked"] and result["public_state_visible"]
        assert any(
            event.get("constructor_path") == "direct_slots"
            for event in result["direct_events"]
        ), (mode, result["direct_events"])
