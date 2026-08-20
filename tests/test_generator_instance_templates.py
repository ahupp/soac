from __future__ import annotations

import json
from pathlib import Path
import textwrap

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import _VALIDATION_PRELUDE, create_strict_project


def test_source_generator_instances_preserve_identity_mutations_and_ordinary_observers(
    tmp_path: Path,
) -> None:
    source = textwrap.dedent(
        """
        def plain(values):
            return (value for value in values)


        def captured(offset, values):
            return (offset + value for value in values)


        def observed(callback, values):
            return (callback(value) for value in values)
        """
    )
    module_names = {
        "stock": "generator_instance_template_stock",
        "canonical": "generator_instance_template_canonical",
        "prepatched": "generator_instance_template_prepatched",
    }
    for module_name in module_names.values():
        (tmp_path / f"{module_name}.py").write_text(source, encoding="utf-8")

    selected = {
        name: f"{name}.py"
        for name in (module_names["canonical"], module_names["prepatched"])
    }
    project = create_strict_project(
        tmp_path / "strict-project",
        {
            path: strict_opt_in(source.encode(), path)[0].decode()
            for path in selected.values()
        },
        modules=selected,
    )

    script = textwrap.dedent(
        """
        import gc
        import importlib
        import json
        import sys
        import weakref
        import types
        from tests._integration import stock_module
        from soac import _soac_ext
        import soac.runtime as runtime

        # Exit the ordinary-loader context before selected imports. The held
        # ordinary module still owns the complete original source functions.
        with stock_module(Path(__FIXTURE_PATH__), __STOCK_MODULE__, __SOURCE__) as stock:
            for name in ("plain", "captured", "observed"):
                assert owner(vars(stock)[name]) is None
                assert metadata(vars(stock)[name]) is None

        expected_paths = __SELECTED_PATHS__
        canonical = importlib.import_module(__CANONICAL_MODULE__)
        prepatched = importlib.import_module(__PREPATCHED_MODULE__)

        def assert_selected(module):
            diagnostic = _soac_ext.strict_module_diagnostics(module)
            assert diagnostic is not None, "source executed without strict ownership"
            assert diagnostic["sealed"] is True
            assert diagnostic["backend"] == "soac"
            assert diagnostic["module_name"] == module.__name__
            assert diagnostic["source_path"] == expected_paths[module.__name__]
            assert diagnostic["artifact_generation"] == __GENERATION__
            assert diagnostic["initializer_entry_kind"] == "entry_interpreter"
            witnesses = []
            for name in ("plain", "captured", "observed"):
                function = vars(module)[name]
                assert owner(function) and metadata(function), name
                actual_entry = _soac_ext.strict_function_entry_kind(function)
                assert actual_entry == "checked_native", (name, actual_entry)
                witnesses.append((name, id(function), owner(function)))
            return tuple(witnesses)

        selected_owners = {
            module.__name__: assert_selected(module)
            for module in (canonical, prepatched)
        }
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
            assert gc.is_tracked(capsule)
            assert matches_owner(generator, capsule) == 1
            return capsule

        def native_plain(module, values):
            generator = module.plain(values)
            source_capsule(generator, module.plain)
            return generator

        def generator_metadata(module):
            plain = module.plain((3, 4))
            captured = module.captured(10, (1, 2))
            metadata = {
                "plain_name_identity": plain.__name__ is plain.gi_code.co_name,
                "plain_qualname_identity": (
                    plain.__qualname__ is plain.gi_code.co_qualname
                ),
                "captured_name_identity": (
                    captured.__name__ is captured.gi_code.co_name
                ),
                "captured_qualname_identity": (
                    captured.__qualname__ is captured.gi_code.co_qualname
                ),
                "plain_name": plain.__name__,
                "plain_qualname": plain.__qualname__,
            }
            if module is not stock:
                source_capsule(plain, module.plain)
                source_capsule(captured, module.captured)
            assert list(plain) == [3, 4]
            assert list(captured) == [11, 12]
            return metadata

        observed_metadata = {
            "stock": generator_metadata(stock),
            "soac": generator_metadata(canonical),
        }

        def assert_generator_semantics(module):
            first = module.captured(3, (1, 2))
            second = module.captured(10, (1, 2))
            assert first is not second
            assert first.gi_code is second.gi_code
            if module is not stock:
                assert source_capsule(first, module.captured) is not source_capsule(
                    second, module.captured
                )
            assert (next(first), next(second), next(first), next(second)) == (
                4,
                11,
                5,
                12,
            )

            observations = []
            lazy = module.observed(observations.append, (3, 4))
            assert observations == [], observations
            assert next(lazy) is None
            assert observations == [3], observations
            assert lazy.send(None) is None
            assert observations == [3, 4], observations
            assert next(lazy, "finished") == "finished"

            throwing = module.plain((7, 8))
            assert next(throwing) == 7
            try:
                throwing.throw(ValueError("generator throw"))
            except ValueError as error:
                assert str(error) == "generator throw", error
            else:
                raise AssertionError("generator throw did not propagate")

            closing = module.plain((9, 10))
            assert next(closing) == 9
            assert closing.close() is None
            assert next(closing, "closed") == "closed"

            class ObservedIterator:
                def __init__(self, finalized):
                    self.finalized = finalized
                    self.remaining = 1

                def __iter__(self):
                    return self

                def __next__(self):
                    if not self.remaining:
                        raise StopIteration
                    self.remaining -= 1
                    return 1

                def __del__(self):
                    self.finalized.append("released")

            finalized = []
            iterator = ObservedIterator(finalized)
            reference = weakref.ref(iterator)
            generator = module.plain(iterator)
            del iterator
            assert reference() is not None
            del generator
            gc.collect()
            assert reference() is None
            assert finalized == ["released"], finalized

        assert_generator_semantics(stock)
        assert_generator_semantics(canonical)

        original_helper = runtime.make_generator_instance

        def ordinary_resume_control(*args):
            raise AssertionError("ordinary construction control must not resume")

        def ordinary_generator_control():
            # Real ordinary factory entry; no source metadata or native permit
            # is installed on this callback, and the wrapper is never resumed.
            assert owner(ordinary_resume_control) is None
            assert metadata(ordinary_resume_control) is None
            generator = runtime.make_generator_instance(
                ordinary_resume_control, 0, "ordinary_generator_control",
                "ordinary_generator_control", (None, 0), (0, 1), 0, 1, (),
            )
            assert type(generator) is runtime.ClosureGenerator
            assert generator._resume_function is ordinary_resume_control
            return generator

        prepatched_calls = []

        def replacement_helper(*args):
            prepatched_calls.append(args)
            return original_helper(*args)

        runtime.make_generator_instance = replacement_helper
        try:
            for value in (20, 21):
                ordinary = ordinary_generator_control()
                del ordinary
                assert list(native_plain(prepatched, (value,))) == [value]
        finally:
            runtime.make_generator_instance = original_helper
        # The ordinary public helper observes its current binding; source
        # generators have a distinct native owner, not a cached wrapper factory.
        ordinary = ordinary_generator_control()
        del ordinary
        assert list(native_plain(prepatched, (21,))) == [21]
        assert len(prepatched_calls) == 2, prepatched_calls

        original_preserved = runtime.make_preserved_state
        preserved_calls = []

        def replacement_preserved(values, kinds, operand_slots):
            assert isinstance(values, tuple), values
            assert isinstance(kinds, tuple), kinds
            assert isinstance(operand_slots, tuple), operand_slots
            preserved_calls.append((len(values), len(kinds)))
            return original_preserved(values, kinds, operand_slots)

        runtime.make_preserved_state = replacement_preserved
        try:
            ordinary = ordinary_generator_control()
            del ordinary
            assert list(native_plain(canonical, (30,))) == [30]
        finally:
            runtime.make_preserved_state = original_preserved
        assert len(preserved_calls) == 1, preserved_calls

        original_getattr = runtime.getattr
        getattr_calls = []

        def replacement_getattr(obj, name, *default):
            if name == "__code__":
                getattr_calls.append(obj)
            return original_getattr(obj, name, *default)

        runtime.getattr = replacement_getattr
        try:
            ordinary = ordinary_generator_control()
            del ordinary
            assert list(native_plain(canonical, (31,))) == [31]
        finally:
            runtime.getattr = original_getattr
        assert len(getattr_calls) == 1, getattr_calls

        original_generator_class = runtime.ClosureGenerator
        class_constructions = []

        class ReplacementGenerator(original_generator_class):
            __slots__ = ()

            def __init__(self, *args):
                class_constructions.append(args)
                super().__init__(*args)

        runtime.ClosureGenerator = ReplacementGenerator
        try:
            replacement = ordinary_generator_control()
            assert type(replacement) is ReplacementGenerator
            assert list(native_plain(canonical, (32,))) == [32]
            del replacement
        finally:
            runtime.ClosureGenerator = original_generator_class
        assert len(class_constructions) == 1, class_constructions

        helper_code = original_helper.__code__
        monitoring = sys.monitoring
        tool_id = next(
            identifier
            for identifier in range(6)
            if monitoring.get_tool(identifier) is None
        )
        observed_monitoring = {"local": [], "global": []}
        monitoring_phase = "local"

        def monitoring_callback(code, _offset):
            if code is helper_code:
                observed_monitoring[monitoring_phase].append(code.co_name)

        monitoring.use_tool_id(tool_id, "soac.generator-instance-template")
        try:
            monitoring.register_callback(
                tool_id,
                monitoring.events.PY_START,
                monitoring_callback,
            )

            # Keep both ordinary code-local and global monitoring controls.
            monitoring.set_local_events(
                tool_id,
                helper_code,
                monitoring.events.PY_START,
            )
            ordinary = ordinary_generator_control()
            del ordinary
            monitoring.set_local_events(tool_id, helper_code, 0)
            assert observed_monitoring["local"], observed_monitoring

            monitoring_phase = "global"
            monitoring.set_events(tool_id, monitoring.events.PY_START)
            try:
                ordinary = ordinary_generator_control()
                del ordinary
            finally:
                monitoring.set_events(tool_id, 0)
            assert observed_monitoring["global"], observed_monitoring
        finally:
            monitoring.set_events(tool_id, 0)
            monitoring.set_local_events(tool_id, helper_code, 0)
            monitoring.register_callback(
                tool_id,
                monitoring.events.PY_START,
                None,
            )
            monitoring.free_tool_id(tool_id)
        assert list(canonical.plain((40,))) == [40]
        assert list(canonical.plain((41,))) == [41]

        profile_events = []

        def profile(frame, event, _argument):
            if event == "call" and frame.f_code is helper_code:
                profile_events.append(event)

        sys.setprofile(profile)
        try:
            ordinary = ordinary_generator_control()
            del ordinary
        finally:
            sys.setprofile(None)
        assert profile_events, profile_events
        assert list(canonical.plain((50,))) == [50]

        trace_events = []

        def trace(frame, event, _argument):
            if event == "call" and frame.f_code is helper_code:
                trace_events.append(event)
            return trace

        sys.settrace(trace)
        try:
            ordinary = ordinary_generator_control()
            del ordinary
        finally:
            sys.settrace(None)
        assert trace_events, trace_events
        assert list(canonical.plain((51,))) == [51]

        for module in (canonical, prepatched):
            assert assert_selected(module) == selected_owners[module.__name__]

        print(
            json.dumps(
                {
                    "metadata": observed_metadata,
                    "prepatched_calls": len(prepatched_calls),
                    "preserved_calls": len(preserved_calls),
                    "getattr_calls": len(getattr_calls),
                    "class_constructions": len(class_constructions),
                    "monitoring": observed_monitoring,
                    "profile_events": len(profile_events),
                    "trace_events": len(trace_events),
                }
            )
        )
        """
    )
    script = (
        script.replace("__FIXTURE_PATH__", repr(str(tmp_path)))
        .replace("__STOCK_MODULE__", repr(module_names["stock"]))
        .replace("__CANONICAL_MODULE__", repr(module_names["canonical"]))
        .replace("__PREPATCHED_MODULE__", repr(module_names["prepatched"]))
        .replace("__SOURCE__", repr(source))
        .replace("__SELECTED_PATHS__", repr({
            name: str(project.project / path) for name, path in selected.items()
        }))
        .replace("__GENERATION__", repr(project.publication["generation"]))
    )

    event_log = tmp_path / "generator-instance-events.jsonl"
    completed = project.run(
        _VALIDATION_PRELUDE + script,
        opt_mode="apply",
        extra_env={
            "SOAC_WORK_DIR": str(tmp_path / "soac-work"),
            "SOAC_LOG": f"soac_generator_preserved_layout=info;json={event_log}",
        },
        timeout=60,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    result = json.loads(completed.stdout.splitlines()[-1])
    stock_metadata = result["metadata"]["stock"]
    soac_metadata = result["metadata"]["soac"]

    identity_fields = (
        "plain_name_identity",
        "plain_qualname_identity",
        "captured_name_identity",
        "captured_qualname_identity",
    )
    assert {field: stock_metadata[field] for field in identity_fields} == {
        field: True for field in identity_fields
    }, result
    assert {field: soac_metadata[field] for field in identity_fields} == {
        field: True for field in identity_fields
    }, result

    assert result["prepatched_calls"] == 2, result
    assert result["preserved_calls"] == 1, result
    assert result["getattr_calls"] == 1, result
    assert result["class_constructions"] == 1, result
    assert result["monitoring"]["local"], result
    assert result["monitoring"]["global"], result
    assert result["profile_events"] > 0, result
    assert result["trace_events"] > 0, result

    layout_events = [
        event
        for line in event_log.read_text(encoding="utf-8").splitlines()
        if (event := json.loads(line))["target"] == "soac_generator_preserved_layout"
    ]
    assert any(
        event.get("qualname") == "plain.<locals>.<genexpr>"
        for event in layout_events
    ), layout_events
