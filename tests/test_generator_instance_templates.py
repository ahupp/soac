from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def test_source_generator_instances_preserve_identity_and_observable_fallbacks(
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

    script = textwrap.dedent(
        """
        import gc
        import importlib
        import json
        import sys
        import weakref

        sys.path.insert(0, __FIXTURE_PATH__)
        stock = importlib.import_module(__STOCK_MODULE__)

        from soac.import_hook import install

        install()
        canonical = importlib.import_module(__CANONICAL_MODULE__)
        prepatched = importlib.import_module(__PREPATCHED_MODULE__)
        import soac.runtime as runtime

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
            assert list(plain) == [3, 4]
            assert list(captured) == [11, 12]
            return metadata

        metadata = {
            "stock": generator_metadata(stock),
            "soac": generator_metadata(canonical),
        }

        def assert_generator_semantics(module):
            first = module.captured(3, (1, 2))
            second = module.captured(10, (1, 2))
            assert first is not second
            assert first.gi_code is second.gi_code
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

            if hasattr(first, "_resume_function"):
                first_resume = first._resume_function
                second_resume = second._resume_function
                first_cell = first_resume.__closure__[
                    first_resume.__code__.co_freevars.index("offset")
                ]
                second_cell = second_resume.__closure__[
                    second_resume.__code__.co_freevars.index("offset")
                ]
                assert first_cell is not second_cell
                assert (first_cell.cell_contents, second_cell.cell_contents) == (
                    3,
                    10,
                )

        assert_generator_semantics(stock)
        assert_generator_semantics(canonical)

        original_helper = runtime.make_generator_instance
        prepatched_calls = []

        def replacement_helper(*args):
            prepatched_calls.append(args)
            return original_helper(*args)

        runtime.make_generator_instance = replacement_helper
        try:
            assert list(prepatched.plain((20,))) == [20]
        finally:
            runtime.make_generator_instance = original_helper
        assert len(prepatched_calls) == 1, prepatched_calls

        # Each transformed module caches the first runtime helper it resolves.
        assert list(prepatched.plain((21,))) == [21]
        assert len(prepatched_calls) == 2, prepatched_calls

        original_preserved = runtime.make_preserved_state
        preserved_calls = []

        def replacement_preserved(values, kinds):
            assert isinstance(values, tuple), values
            assert isinstance(kinds, tuple), kinds
            preserved_calls.append((len(values), len(kinds)))
            return original_preserved(values, kinds)

        runtime.make_preserved_state = replacement_preserved
        try:
            assert list(canonical.plain((30,))) == [30]
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
            assert list(canonical.plain((31,))) == [31]
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
            replacement = canonical.plain((32,))
            assert type(replacement) is ReplacementGenerator
            assert list(replacement) == [32]
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

            # Local events do not increment CPython's global monitoring version.
            # Exercise this before profiling or globally enabled monitoring.
            monitoring.set_local_events(
                tool_id,
                helper_code,
                monitoring.events.PY_START,
            )
            assert list(canonical.plain((40,))) == [40]
            monitoring.set_local_events(tool_id, helper_code, 0)
            assert observed_monitoring["local"], observed_monitoring

            monitoring_phase = "global"
            monitoring.set_events(tool_id, monitoring.events.PY_START)
            try:
                assert list(canonical.plain((41,))) == [41]
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

        profile_events = []

        def profile(frame, event, _argument):
            if event == "call" and frame.f_code is helper_code:
                profile_events.append(event)

        sys.setprofile(profile)
        try:
            assert list(canonical.plain((50,))) == [50]
        finally:
            sys.setprofile(None)
        assert profile_events, profile_events

        trace_events = []

        def trace(frame, event, _argument):
            if event == "call" and frame.f_code is helper_code:
                trace_events.append(event)
            return trace

        sys.settrace(trace)
        try:
            assert list(canonical.plain((51,))) == [51]
        finally:
            sys.settrace(None)
        assert trace_events, trace_events

        print(
            json.dumps(
                {
                    "metadata": metadata,
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
    )

    event_log = tmp_path / "generator-instance-events.jsonl"
    env = dict(os.environ)
    env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(tmp_path / "soac-work"),
            "SOAC_OPT_MODE": "apply",
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
            "SOAC_LOG": (
                "soac_generator_direct_state=debug;"
                f"json={event_log}"
            ),
        }
    )
    completed = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
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

    direct_events = [
        event
        for line in event_log.read_text(encoding="utf-8").splitlines()
        if (event := json.loads(line))["target"] == "soac_generator_direct_state"
    ]
    assert any(
        event.get("path") == "direct"
        and event.get("temporary_python_tuples") == 0
        for event in direct_events
    ), direct_events
