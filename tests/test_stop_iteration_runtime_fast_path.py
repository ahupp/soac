from __future__ import annotations

import json
from pathlib import Path
import subprocess
import textwrap

import pytest
from tests._strict_integration import StrictProject, create_strict_project


_MODULE_SOURCE = """
StopIteration = ValueError


def invoke(callback, error, expected):
    return callback(error, expected)


def collect(values):
    return [value for value in values]


def ordinary_loop(values):
    result = []
    for value in values:
        result.append(value)
    return result


def increment(value):
    return value + 1


def unrelated_direct(value):
    return increment(value)


def explicit_shadow(error):
    try:
        raise error
    except StopIteration:
        return "shadow"
    except BaseException:
        return "other"
"""


def _write_module(tmp_path: Path) -> str:
    name = "stop_iteration_runtime_fast_path_case"
    (tmp_path / f"{name}.py").write_text(
        textwrap.dedent(_MODULE_SOURCE), encoding="utf-8"
    )
    return name


def _worker_environment(tmp_path, mode, scenario, *, work_dir=None):
    log_path = tmp_path / f"{scenario}-{mode}.jsonl"
    work_dir = tmp_path / f"{scenario}-work" if work_dir is None else work_dir
    return {
        "SOAC_WORK_DIR": str(work_dir),
        "SOAC_OPT_MODE": mode,
        "SOAC_COMPILE_MODE": "eager",
        "SOAC_BACKGROUND_JIT": "0",
        "SOAC_LOG": f"soac_jit_direct_edges=info;json={log_path}",
    }


def _create_project(tmp_path, module_name, mode, scenario, *, work_dir=None):
    # Only this original caller module is selected. soac.runtime stays ordinary;
    # neither import-hook installation nor a test prefix grants it authority.
    environment = _worker_environment(
        tmp_path, mode, scenario, work_dir=work_dir,
    )
    source = (tmp_path / f"{module_name}.py").read_text(encoding="utf-8")
    with pytest.MonkeyPatch.context() as patch:
        patch.delenv("SOAC_MODULE_ENABLED", raising=False)
        for name, value in environment.items():
            patch.setenv(name, value)
        return create_strict_project(
            tmp_path / "strict-publication",
            {f"{module_name}.py": "# soac: module(strict_assign=true, checked_attr=true)\n" + source.lstrip("\n")},
            modules={module_name: f"{module_name}.py"},
            backend="soac",
        )


def _run_worker(
    tmp_path: Path,
    module_name: str,
    mode: str,
    scenario: str,
    *,
    project: StrictProject,
    work_dir: Path | None = None,
) -> tuple[subprocess.CompletedProcess[str], Path]:
    log_path = tmp_path / f"{scenario}-{mode}.jsonl"
    script = textwrap.dedent(
        """
        import builtins
        import ctypes
        import importlib
        import json
        import os
        import sys

        from soac import _soac_ext
        module = importlib.import_module(__MODULE_NAME__)
        import soac.runtime as runtime

        diagnostic = _soac_ext.strict_module_diagnostics(module)
        assert diagnostic is not None, "caller source ran without strict ownership"
        assert diagnostic["sealed"] is True
        assert diagnostic["module_name"] == __MODULE_NAME__
        assert diagnostic["source_path"] == __SOURCE_PATH__
        assert diagnostic["artifact_generation"] == __GENERATION__
        assert diagnostic["initializer_entry_kind"] == "entry_interpreter"
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = (ctypes.py_object,)
        owner.restype = ctypes.c_void_p
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = (ctypes.py_object,)
        metadata.restype = ctypes.c_void_p
        for function_name in (
            "invoke", "collect", "ordinary_loop", "increment",
            "unrelated_direct", "explicit_shadow",
        ):
            function = vars(module)[function_name]
            assert owner(function), function_name
            assert metadata(function), function_name
        del function

        # These are ordinary mutable dependencies, not newly enrolled helpers.
        assert _soac_ext.strict_module_diagnostics(runtime) is None
        assert not owner(runtime.exception_matches)
        assert not owner(runtime._validate_exception_type)

        # Configuration premise only: this is not raw indexed-store guard
        # coverage. The ordinary runtime has no installed indexed prefix.
        import _testinternalcapi
        assert not _testinternalcapi.dict_has_indexed_keys(runtime.__dict__)

        scenario = __SCENARIO__
        events = []
        original_helper = runtime.exception_matches
        original_validator = runtime._validate_exception_type
        stop_class = builtins.StopIteration

        def call(error=None):
            if error is None:
                error = stop_class("done")
            return module.invoke(runtime.exception_matches, error, stop_class)

        if scenario == "pre_first_replacement":
            def replacement(error, expected):
                events.append("replacement")
                return original_helper(error, expected)

            runtime.exception_matches = replacement
            try:
                assert call() is True
            finally:
                runtime.exception_matches = original_helper
            assert events == ["replacement"], events

        elif scenario == "helper_code":
            assert call() is True
            sys._soac_stop_iteration_test_events = events

            def replacement(error, expected):
                _sys._soac_stop_iteration_test_events.append("helper-code")
                return isinstance(error, expected)

            original_code = original_helper.__code__
            try:
                original_helper.__code__ = replacement.__code__
                assert call() is True
            finally:
                original_helper.__code__ = original_code
                del sys._soac_stop_iteration_test_events
            assert events == ["helper-code"], events

        elif scenario == "validator_code":
            assert call() is True
            sys._soac_stop_iteration_test_events = events

            def replacement(expected):
                _sys._soac_stop_iteration_test_events.append("validator-code")

            original_code = original_validator.__code__
            try:
                original_validator.__code__ = replacement.__code__
                assert call() is True
            finally:
                original_validator.__code__ = original_code
                del sys._soac_stop_iteration_test_events
            assert events == ["validator-code"], events

        elif scenario == "dependencies_and_observers":
            assert call() is True

            def replace_validator(expected):
                events.append("validator")
                return original_validator(expected)

            runtime._validate_exception_type = replace_validator
            try:
                assert call() is True
            finally:
                runtime._validate_exception_type = original_validator
            assert events == ["validator"], events
            events.clear()

            original_isinstance = runtime.isinstance

            def replace_isinstance(value, expected):
                events.append("isinstance")
                return original_isinstance(value, expected)

            runtime.isinstance = replace_isinstance
            try:
                assert call() is True
            finally:
                runtime.isinstance = original_isinstance
            assert len(events) >= 3, events
            events.clear()

            class ObservedTupleMeta(type):
                def __instancecheck__(self, value):
                    events.append("tuple")
                    return False

            class ObservedTuple(metaclass=ObservedTupleMeta):
                pass

            original_tuple = runtime.tuple
            runtime.tuple = ObservedTuple
            try:
                assert call() is True
            finally:
                runtime.tuple = original_tuple
            assert events == ["tuple"], events
            events.clear()

            class ObservedTypeMeta(type):
                def __instancecheck__(self, value):
                    events.append("type")
                    return True

            class ObservedType(metaclass=ObservedTypeMeta):
                pass

            original_type = runtime.type
            runtime.type = ObservedType
            try:
                assert call() is True
            finally:
                runtime.type = original_type
            assert events == ["type"], events
            events.clear()

            original_issubclass = builtins.issubclass

            def replace_issubclass(value, expected):
                events.append("issubclass")
                return original_issubclass(value, expected)

            builtins.issubclass = replace_issubclass
            try:
                assert call() is True
            finally:
                builtins.issubclass = original_issubclass
            assert events == ["issubclass"], events
            events.clear()

            assert "issubclass" not in runtime.__dict__
            runtime.issubclass = replace_issubclass
            try:
                assert call() is True
            finally:
                del runtime.issubclass
            assert events == ["issubclass"], events
            events.clear()

            class ObservedBaseMeta(type):
                def __subclasscheck__(self, value):
                    events.append("base-exception")
                    return True

            class ObservedBase(metaclass=ObservedBaseMeta):
                pass

            original_base = builtins.BaseException
            builtins.BaseException = ObservedBase
            try:
                assert call() is True
            finally:
                builtins.BaseException = original_base
            assert events == ["base-exception"], events
            events.clear()

            class ObservedRecursionMeta(type):
                def __instancecheck__(self, value):
                    events.append("recursion")
                    return False

            class ObservedRecursion(metaclass=ObservedRecursionMeta):
                pass

            original_recursion = builtins.RecursionError
            builtins.RecursionError = ObservedRecursion
            try:
                assert call() is True
            finally:
                builtins.RecursionError = original_recursion
            assert events == ["recursion"], events
            events.clear()

            # The two historical watcher-bypassing indexed-slot observers
            # required a runtime dictionary capability not installed here.
            # They are archived, not replaced by an ordinary-dict write test.

            class PlainStop(stop_class):
                pass

            assert call(PlainStop("subclass")) is True

            class ObservedStop(stop_class):
                @property
                def __class__(self):
                    events.append("stop-class")
                    return stop_class

            assert call(ObservedStop("observed")) is True
            assert events == ["stop-class"], events
            events.clear()

            class SpoofedStop(stop_class):
                @property
                def __class__(self):
                    events.append("spoofed-stop")
                    return builtins.RecursionError

            assert call(SpoofedStop("spoofed")) is True
            assert events == ["spoofed-stop"], events
            events.clear()

            class RaisingStop(stop_class):
                @property
                def __class__(self):
                    events.append("raising-stop")
                    raise RuntimeError("stop class observed")

            try:
                call(RaisingStop("raising"))
            except RuntimeError as error:
                assert str(error) == "stop class observed"
            else:
                raise AssertionError("subclass __class__ must be evaluated")
            assert events == ["raising-stop"], events
            events.clear()

            class SpoofedException(Exception):
                @property
                def __class__(self):
                    events.append("spoofed-exception")
                    return stop_class

            assert call(SpoofedException("fake")) is True
            assert len(events) == 2, events
            events.clear()

            class RaisingException(Exception):
                @property
                def __class__(self):
                    events.append("raising-exception")
                    raise RuntimeError("exception class observed")

            try:
                call(RaisingException("raising"))
            except RuntimeError as error:
                assert str(error) == "exception class observed"
            else:
                raise AssertionError("nonmatching __class__ must be evaluated")
            assert events == ["raising-exception"], events
            events.clear()

            assert module.invoke(original_helper, stop_class, stop_class) is False
            assert call(ValueError("different")) is False

            def ordinary_call():
                # Exercise the ordinary helper itself. A SOAC call is not
                # required to choose a fallback just to reproduce its events.
                return original_helper(stop_class("done"), stop_class)

            monitoring = sys.monitoring
            tool_id = next(
                identifier
                for identifier in range(6)
                if monitoring.get_tool(identifier) is None
            )
            helper_code = original_helper.__code__
            validator_code = original_validator.__code__
            observed = {"helper-local": [], "validator-local": [], "global": []}
            phase = "helper-local"

            def monitor(code, _offset):
                if code is helper_code or code is validator_code:
                    observed[phase].append(code.co_name)

            monitoring.use_tool_id(tool_id, "soac.stop-iteration-fast-path")
            try:
                monitoring.register_callback(
                    tool_id, monitoring.events.PY_START, monitor
                )
                monitoring.set_local_events(
                    tool_id, helper_code, monitoring.events.PY_START
                )
                assert ordinary_call() is True
                monitoring.set_local_events(tool_id, helper_code, 0)
                assert "exception_matches" in observed["helper-local"], observed

                phase = "validator-local"
                monitoring.set_local_events(
                    tool_id, validator_code, monitoring.events.PY_START
                )
                assert ordinary_call() is True
                monitoring.set_local_events(tool_id, validator_code, 0)
                assert (
                    "_validate_exception_type" in observed["validator-local"]
                ), observed

                phase = "global"
                monitoring.set_events(tool_id, monitoring.events.PY_START)
                try:
                    assert ordinary_call() is True
                finally:
                    monitoring.set_events(tool_id, 0)
                assert {"exception_matches", "_validate_exception_type"} <= set(
                    observed["global"]
                ), observed
            finally:
                monitoring.set_events(tool_id, 0)
                monitoring.set_local_events(tool_id, helper_code, 0)
                monitoring.set_local_events(tool_id, validator_code, 0)
                monitoring.register_callback(
                    tool_id, monitoring.events.PY_START, None
                )
                monitoring.free_tool_id(tool_id)

            profile_events = []

            def profile(frame, event, _argument):
                if event == "call" and frame.f_code in (helper_code, validator_code):
                    profile_events.append(frame.f_code.co_name)

            sys.setprofile(profile)
            try:
                assert ordinary_call() is True
            finally:
                sys.setprofile(None)
            assert {"exception_matches", "_validate_exception_type"} <= set(
                profile_events
            ), profile_events

            trace_events = []

            def trace(frame, event, _argument):
                if event == "call" and frame.f_code in (helper_code, validator_code):
                    trace_events.append(frame.f_code.co_name)
                return trace

            sys.settrace(trace)
            try:
                assert ordinary_call() is True
            finally:
                sys.settrace(None)
            assert {"exception_matches", "_validate_exception_type"} <= set(
                trace_events
            ), trace_events
            assert call() is True

        elif scenario == "training":
            for index in range(40):
                assert module.collect((index, index + 1)) == [index, index + 1]
                assert module.ordinary_loop((index,)) == [index]
                assert module.unrelated_direct(index) == index + 1
            assert module.explicit_shadow(ValueError("shadow")) == "shadow"
            assert module.explicit_shadow(stop_class("real")) == "other"

        else:
            raise AssertionError(scenario)

        print(json.dumps({"scenario": scenario, "ok": True}))
        """
    )
    script = (
        script.replace("__MODULE_NAME__", repr(module_name))
        .replace("__SOURCE_PATH__", repr(str(project.project / f"{module_name}.py")))
        .replace("__GENERATION__", repr(project.publication["generation"]))
        .replace("__SCENARIO__", repr(scenario))
    )
    environment = _worker_environment(
        tmp_path, mode, scenario, work_dir=work_dir,
    )
    assert environment["SOAC_COMPILE_MODE"] == project.environment["SOAC_COMPILE_MODE"]
    assert environment["SOAC_BACKGROUND_JIT"] == project.environment["SOAC_BACKGROUND_JIT"]
    environment.pop("SOAC_OPT_MODE")
    completed = project.run(
        script, opt_mode=mode, extra_env=environment,
        timeout=90, check=False, backend="soac",
    )
    return completed, log_path


@pytest.mark.parametrize(
    "scenario",
    (
        "pre_first_replacement",
        "helper_code",
        "validator_code",
        "dependencies_and_observers",
    ),
)
def test_stop_iteration_fast_path_preserves_dependencies_and_ordinary_observers(
    tmp_path: Path, scenario: str
) -> None:
    module_name = _write_module(tmp_path)
    project = _create_project(tmp_path, module_name, "apply", scenario)
    result, _ = _run_worker(tmp_path, module_name, "apply", scenario, project=project)
    assert result.returncode == 0, (
        f"{scenario} must preserve Python-visible callbacks and exception matching:\n"
        f"{result.stdout}{result.stderr}"
    )
    assert json.loads(result.stdout.splitlines()[-1]) == {
        "scenario": scenario,
        "ok": True,
    }


def test_stop_iteration_fast_path_uses_generic_edge_only_for_runtime_handler(
    tmp_path: Path,
) -> None:
    module_name = _write_module(tmp_path)
    work_dir = tmp_path / "trained-work"
    project = _create_project(
        tmp_path, module_name, "profile", "training", work_dir=work_dir,
    )
    profile, _ = _run_worker(
        tmp_path, module_name, "profile", "training",
        project=project, work_dir=work_dir,
    )
    assert profile.returncode == 0, profile.stdout + profile.stderr

    from soac import _soac_ext

    dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    records = [
        record for record in dump["records"] if record["module_name"] == module_name
    ]
    assert records, dump
    hot_calls = [
        row
        for record in records
        for row in record["rows"]
        if row["kind"] == "call_hot_targets" and row["value"] >= 32
    ]
    nested_names = {
        row["function_qualname"]
        for row in hot_calls
        if row["function_qualname"].startswith("collect.<locals>.")
    }
    assert len(nested_names) == 1, (
        "Profile must train the actual nested synthetic StopIteration handler",
        hot_calls,
    )
    nested_name = next(iter(nested_names))
    assert any(
        row["function_qualname"] == "unrelated_direct" for row in hot_calls
    ), (
        "Profile must independently train an unrelated ordinary direct target",
        hot_calls,
    )

    summary_path = work_dir / "jit-code-summary.jsonl"
    for mode in ("verify", "apply"):
        previous_summary_count = len(
            summary_path.read_text(encoding="utf-8").splitlines()
        )
        result, event_path = _run_worker(
            tmp_path, module_name, mode, "training", project=project, work_dir=work_dir
        )
        assert result.returncode == 0, result.stdout + result.stderr
        compiled_nested_bodies = [
            row
            for line in summary_path.read_text(encoding="utf-8").splitlines()[
                previous_summary_count:
            ]
            if line.strip()
            if (row := json.loads(line)).get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == nested_name
        ]
        assert compiled_nested_bodies, (
            "each mode must compile the profiled nested comprehension body",
            mode,
            nested_name,
        )
        events = [
            event
            for line in event_path.read_text(encoding="utf-8").splitlines()
            if line.strip()
            if (event := json.loads(line)).get("target") == "soac_jit_direct_edges"
            and event.get("module") == module_name
        ]
        nested = [event for event in events if event.get("qualname") == nested_name]
        unrelated = [
            event for event in events if event.get("qualname") == "unrelated_direct"
        ]
        assert unrelated, (mode, events)
        assert all(event.get("clif_direct_edges", 0) >= 1 for event in unrelated), (
            "ordinary profiled direct targets must remain direct",
            mode,
            unrelated,
        )
        assert not nested, (
            "a compiled compiler-owned exact StopIteration handler must have "
            "zero direct edges; empty DirectEdgeStats deliberately emits no event",
            mode,
            nested,
        )
