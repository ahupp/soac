from __future__ import annotations

import json
from pathlib import Path
import textwrap

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import _VALIDATION_PRELUDE, create_strict_project


_FUNCTION_WITNESS = """
_native_function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
_native_function_id.argtypes = [ctypes.py_object]
_native_function_id.restype = ctypes.c_uint64
_native_seal_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
_native_seal_id.argtypes = [ctypes.py_object]
_native_seal_id.restype = ctypes.c_uint64

def _assert_selected_functions(module):
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    assert diagnostic is not None, 'selected source executed as ordinary Python'
    assert diagnostic['sealed'] is True
    assert diagnostic['module_name'] == __EXPECTED_MODULE__
    assert diagnostic['source_path'] == __EXPECTED_SOURCE__
    assert diagnostic['artifact_generation'] == __EXPECTED_GENERATION__
    assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
    observed = []
    for name in ('make', 'captured', 'child', 'hot'):
        function = _plain_function_witness(module, name)
        assert metadata(function), name
        actual_owner = owner(function)
        assert actual_owner, name
        function_id = _native_function_id(function)
        seal_id = _native_seal_id(function)
        # Sealing does not publish the optional unchecked direct-call ID.
        assert function_id == 0 and seal_id > 0, (name, function_id, seal_id)
        actual_entry = _soac_ext.strict_function_entry_kind(function)
        assert actual_entry == 'checked_native', (name, actual_entry)
        observed.append((name, actual_owner, function_id, seal_id))
    return tuple(observed)

def _assert_ordinary_functions(namespace):
    for name in ('make', 'captured', 'child', 'hot'):
        function = namespace[name]
        assert owner(function) is None and metadata(function) is None, name
        assert _native_function_id(function) == 0, name
        assert _native_seal_id(function) == 0, name
"""


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


def _worker(project, tmp_path: Path, module_name: str, work_dir: Path, mode: str) -> dict:
    events = work_dir / f"{mode}-events.jsonl"
    script = textwrap.dedent(
        """
        import builtins, ctypes, gc, importlib, json, sys, types, weakref
        root, name = __ROOT__, __NAME__
        source = open(root + "/" + name + ".py", encoding="utf-8").read()
        stock = {"__name__": "stock_generator_state", "__builtins__": builtins.__dict__}
        exec(compile(source, "<stock-generator-state>", "exec"), stock)
        from soac import _soac_ext
        _assert_ordinary_functions(stock)
        module = importlib.import_module(name)
        source_owners = _assert_selected_functions(module)
        import soac.runtime as runtime

        valid_capsule = ctypes.pythonapi.PyCapsule_IsValid
        valid_capsule.argtypes = [ctypes.py_object, ctypes.c_char_p]
        valid_capsule.restype = ctypes.c_int
        matches_owner = ctypes.pythonapi.PyGen_MatchesSoacOwner
        matches_owner.argtypes = [ctypes.py_object, ctypes.py_object]
        matches_owner.restype = ctypes.c_int
        has_type_contract = ctypes.pythonapi.PyType_HasSoacContract
        has_type_contract.argtypes = [ctypes.py_object]
        has_type_contract.restype = ctypes.c_int

        (make_code,) = (
            value for value in module.make.__code__.co_consts
            if type(value) is types.CodeType and value.co_name == "<genexpr>"
        )
        (captured_code,) = (
            value for value in module.captured.__code__.co_consts
            if type(value) is types.CodeType and value.co_name == "<genexpr>"
        )

        def managed_capsule(instance, expected_code):
            assert type(instance) is types.GeneratorType
            assert instance.gi_code is expected_code
            (capsule,) = (
                value for value in gc.get_referents(instance)
                if valid_capsule(value, b"soac.PreservedState")
            )
            assert gc.is_tracked(capsule)
            assert matches_owner(instance, capsule) == 1
            return capsule

        def observed_native_generator(instance, expected_code):
            # Read the actual native association; never mint a resume permit.
            managed_capsule(instance, expected_code)
            return instance

        def ordinary_resume_control(*args):
            raise AssertionError("ordinary construction control must never resume")

        def ordinary_wrapper():
            # Exercise only the existing ordinary constructor. This function
            # has no compiler metadata or native source owner and is never sent.
            assert owner(ordinary_resume_control) is None
            assert metadata(ordinary_resume_control) is None
            assert _native_function_id(ordinary_resume_control) == 0
            wrapper = runtime.make_generator_instance(
                ordinary_resume_control, 0, "ordinary_wrapper_control",
                "ordinary_wrapper_control", (None, 0), (0, 1), 0, 1, (),
            )
            assert type(wrapper) is runtime.ClosureGenerator
            assert has_type_contract(type(wrapper)) == 0
            assert wrapper._resume_function is ordinary_resume_control
            assert gc.is_tracked(wrapper._preserved_values)
            assert runtime.load_preserved_state(wrapper._preserved_values, 0) is None
            assert runtime.load_preserved_state(wrapper._preserved_values, 1) == 0
            assert wrapper.__name__ == wrapper.__qualname__ == "ordinary_wrapper_control"
            return wrapper

        for ordinary_helper in (
            runtime.make_generator_instance, runtime.ClosureGenerator.__init__,
        ):
            assert owner(ordinary_helper) is None
            assert metadata(ordinary_helper) is None
            assert _native_function_id(ordinary_helper) == 0
            assert _native_seal_id(ordinary_helper) == 0

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
        assert managed_capsule(first, captured_code) is not managed_capsule(second, captured_code)
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
        state = runtime.make_preserved_state(values, kinds, ())
        assert runtime.load_preserved_state(state, 0) is marker
        assert runtime.load_preserved_state(state, 63) == 63
        assert runtime.load_preserved_state(state, 69) == 69
        public_state_tracked = gc.is_tracked(state)
        public_state_visible = any(value is marker for value in gc.get_referents(state))
        del state

        generator_class = runtime.ClosureGenerator
        original_init = generator_class.__init__
        monitor_events = []
        native_mutation_events = []
        # The actual capsule/type/source association, not an absent observer
        # event, proves that this is native managed construction.
        assert list(observed_native_generator(module.make((5,)), make_code)) == [5]
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
                # Preserve a positive PY_START observer of the ordinary wrapper.
                ordinary = ordinary_wrapper()
                del ordinary
                assert monitor_events == ["__init__"], monitor_events
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
            before_native = len(changes)
            assert list(observed_native_generator(module.make((6,)), make_code)) == [6]
            native_mutation_events.append(len(changes) - before_native)
            assert len(changes) == before_native, changes
            ordinary = ordinary_wrapper()
            del ordinary
            assert changes[-1] == "init", changes
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
            before_native = len(changes)
            assert list(observed_native_generator(module.make((7,)), make_code)) == [7]
            native_mutation_events.append(len(changes) - before_native)
            assert len(changes) == before_native, changes
            ordinary = ordinary_wrapper()
            del ordinary
            assert changes[-1] == "code", changes
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
            before_native = len(changes)
            assert list(observed_native_generator(module.make((9,)), make_code)) == [9]
            native_mutation_events.append(len(changes) - before_native)
            assert len(changes) == before_native, changes
            ordinary = ordinary_wrapper()
            del ordinary
            assert "setattr:_preserved_values" in changes, changes
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
            before_native = len(changes)
            instance = module.make((10,))
            managed_capsule(instance, make_code)
            assert type(instance) is types.GeneratorType and list(instance) == [10]
            native_mutation_events.append(len(changes) - before_native)
            assert len(changes) == before_native, changes
            ordinary = ordinary_wrapper()
            assert type(ordinary) is Replacement
            del ordinary
        finally:
            runtime.ClosureGenerator = generator_class
        assert "new" in changes, changes
        assert native_mutation_events == [0, 0, 0, 0], native_mutation_events

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
            capsule = (
                managed_capsule(generator, make_code)
                if factory is module.make else getattr(generator, "_preserved_values", None)
            )
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
        _assert_ordinary_functions(stock)
        assert _assert_selected_functions(module) == source_owners
        print(json.dumps({"mode": __MODE__, "stock_cycle": stock_cycle,
            "soac_cycle": soac_cycle, "public_state_tracked": public_state_tracked,
            "public_state_visible": public_state_visible, "watched": watched,
            "monitor_events": monitor_events, "changes": changes,
            "native_mutation_events": native_mutation_events,
            "finalizers": finalizers}))
        """
    )
    script = script.replace("__ROOT__", repr(str(tmp_path)))
    script = script.replace("__NAME__", repr(module_name)).replace("__MODE__", repr(mode))
    witness = (
        _FUNCTION_WITNESS.replace("__EXPECTED_MODULE__", repr(module_name))
        .replace("__EXPECTED_SOURCE__", repr(str(project.project / f"{module_name}.py")))
        .replace("__EXPECTED_GENERATION__", repr(project.publication["generation"]))
    )
    completed = project.run(
        _VALIDATION_PRELUDE + witness + script,
        opt_mode=mode,
        extra_env={
            "SOAC_WORK_DIR": str(work_dir),
            "SOAC_LOG": f"soac_generator_direct_state=debug;json={events}",
        },
        timeout=90,
        check=False,
    )
    assert completed.returncode == 0, (
        f"{mode} transformed generator-state subprocess failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    result = json.loads(completed.stdout.splitlines()[-1])
    result["direct_events"] = [
        event
        for line in (events.read_text(encoding="utf-8").splitlines() if events.exists() else ())
        if (event := json.loads(line))["target"] == "soac_generator_direct_state"
    ]
    return result


def test_generator_state_gc_and_direct_initialization(tmp_path: Path) -> None:
    module_name = "generator_state_gc_and_direct_initialization_case"
    (tmp_path / f"{module_name}.py").write_text(textwrap.dedent(_SOURCE))
    # Preserve the exact ordinary source; authenticate only the separate copy.
    relative = f"{module_name}.py"
    original_source = (tmp_path / relative).read_bytes()
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in(original_source, relative)[0].decode()},
        modules={module_name: relative},
    )
    work_dir = tmp_path / "soac-work"
    results = {
        mode: _worker(project, tmp_path, module_name, work_dir, mode)
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
        # Selected source uses the installed native generator owner, not the
        # optional compiler-intrinsic wrapper constructor optimization.
        assert result["direct_events"] == [], (mode, result["direct_events"])
        assert result["native_mutation_events"] == [0, 0, 0, 0], (mode, result)
        assert result["monitor_events"] == ["__init__"], (mode, result)
        assert {"init", "code", "new", "setattr:_preserved_values"}.issubset(
            result["changes"]
        ), (mode, result)
