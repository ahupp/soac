from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def test_source_function_templates_match_cpython_function_creation_events(
    tmp_path: Path,
) -> None:
    source = textwrap.dedent(
        """
        def genexpr_factory(values):
            return (value for value in values)


        def captured_factory(offset):
            def original_inner(value):
                return offset + value

            return original_inner


        def defaulted_factory(offset):
            def original_default(value=offset, *, scale=2):
                return value * scale

            return original_default
        """
    )
    stock_module_name = "source_function_template_stock"
    soac_module_name = "source_function_template_soac"
    for module_name in (stock_module_name, soac_module_name):
        (tmp_path / f"{module_name}.py").write_text(source, encoding="utf-8")

    script = textwrap.dedent(
        """
        import ctypes
        import importlib
        import json
        import sys

        sys.path.insert(0, __FIXTURE_PATH__)
        stock_module = importlib.import_module(__STOCK_MODULE__)

        from soac.import_hook import install

        install()
        soac_module = importlib.import_module(__SOAC_MODULE__)
        import soac.runtime as runtime

        EVENT_CREATE = 0
        EVENT_MODIFY_DEFAULTS = 3
        EVENT_MODIFY_KWDEFAULTS = 4
        EVENT_MODIFY_QUALNAME = 5
        target_names = ("<genexpr>", "original_inner", "original_default")
        target_globals = {
            "stock": stock_module.__dict__,
            "soac": soac_module.__dict__,
        }
        created = {
            mode: {name: [] for name in target_names}
            for mode in target_globals
        }
        creation_snapshots = {
            mode: {name: [] for name in target_names}
            for mode in target_globals
        }
        observed_events = {
            phase: {
                mode: {name: [] for name in target_names}
                for mode in target_globals
            }
            for phase in ("initialization", "mutation")
        }
        phase = "initialization"
        callback_errors = []

        callback_type = ctypes.CFUNCTYPE(
            ctypes.c_int,
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_void_p,
        )

        @callback_type
        def watch_function(event, function_ptr, _new_value):
            if event not in (
                EVENT_CREATE,
                EVENT_MODIFY_DEFAULTS,
                EVENT_MODIFY_KWDEFAULTS,
                EVENT_MODIFY_QUALNAME,
            ):
                return 0
            try:
                function = ctypes.cast(function_ptr, ctypes.py_object).value
                if function.__globals__ is target_globals["stock"]:
                    mode = "stock"
                elif function.__globals__ is target_globals["soac"]:
                    mode = "soac"
                else:
                    return 0
                code = function.__code__
                if code.co_name not in target_names:
                    return 0
                observed_events[phase][mode][code.co_name].append(event)
                if event == EVENT_CREATE:
                    # Retain each original function intentionally so identity,
                    # code reuse, and its eventual closure can be inspected.
                    created[mode][code.co_name].append(function)
                    creation_snapshots[mode][code.co_name].append(
                        {
                            "defaults_are_none": function.__defaults__ is None,
                            "kwdefaults_are_none": function.__kwdefaults__ is None,
                            "closure_is_none": function.__closure__ is None,
                        }
                    )
            except BaseException as error:
                callback_errors.append(f"{type(error).__name__}: {error}")
            return 0

        def delayed_values(offset, observations):
            observations.append(offset)
            yield offset

        def exercise_factory(module):
            captured_functions = []
            defaulted_functions = []
            for offset in (1, 10, 100):
                observations = []
                generator = module.genexpr_factory(
                    delayed_values(offset, observations)
                )
                assert observations == [], observations
                assert next(generator) == offset
                assert observations == [offset], observations
                assert next(generator, None) is None

                captured = module.captured_factory(offset)
                assert captured(7) == offset + 7
                captured_functions.append(captured)

                defaulted = module.defaulted_factory(offset)
                assert defaulted.__defaults__ == (offset,)
                assert defaulted.__kwdefaults__ == {"scale": 2}
                assert defaulted() == offset * 2
                defaulted_functions.append(defaulted)

            cells = [
                function.__closure__[
                    function.__code__.co_freevars.index("offset")
                ]
                for function in captured_functions
            ]
            assert len({id(cell) for cell in cells}) == 3
            assert [cell.cell_contents for cell in cells] == [1, 10, 100]
            return captured_functions, defaulted_functions

        add_watcher = ctypes.pythonapi.PyFunction_AddWatcher
        add_watcher.argtypes = [callback_type]
        add_watcher.restype = ctypes.c_int
        clear_watcher = ctypes.pythonapi.PyFunction_ClearWatcher
        clear_watcher.argtypes = [ctypes.c_int]
        clear_watcher.restype = ctypes.c_int
        watcher_id = add_watcher(watch_function)
        assert watcher_id >= 0, watcher_id

        summaries = {}
        try:
            returned_functions = {
                "stock": exercise_factory(stock_module),
                "soac": exercise_factory(soac_module),
            }

            for mode in target_globals:
                summaries[mode] = {
                    "created_counts": {
                        name: len(created[mode][name])
                        for name in target_names
                    },
                    "distinct_function_counts": {
                        name: len({id(function) for function in created[mode][name]})
                        for name in target_names
                    },
                    "shared_original_code": {
                        name: bool(created[mode][name])
                        and all(
                            function.__code__ is created[mode][name][0].__code__
                            for function in created[mode][name]
                        )
                        for name in target_names
                    },
                    "name_identity": {
                        name: [
                            function.__name__ is function.__code__.co_name
                            for function in created[mode][name]
                        ]
                        for name in target_names
                    },
                    "qualname_identity": {
                        name: [
                            function.__qualname__ is function.__code__.co_qualname
                            for function in created[mode][name]
                        ]
                        for name in target_names
                    },
                    "creation_snapshots": creation_snapshots[mode],
                }

            phase = "mutation"
            for mode, (_captured, defaults) in returned_functions.items():
                function = defaults[0]
                function.__defaults__ = (11,)
                function.__kwdefaults__ = {"scale": 3}
                function.__qualname__ = f"{mode}.mutated"
                assert function() == 33, (mode, function())
                assert function.__qualname__ == f"{mode}.mutated"
        finally:
            assert clear_watcher(watcher_id) == 0

        synthetic_factory_calls = []
        original_synthetic_factory = runtime.code_with_freevars

        def forbidden_synthetic_factory(*args):
            synthetic_factory_calls.append(args)
            raise AssertionError("source-backed functions requested synthetic code")

        runtime.code_with_freevars = forbidden_synthetic_factory
        try:
            assert soac_module.captured_factory(7)(1) == 8
            assert soac_module.defaulted_factory(4)() == 8
            assert list(soac_module.genexpr_factory((3, 4))) == [3, 4]
        finally:
            runtime.code_with_freevars = original_synthetic_factory

        print(
            json.dumps(
                {
                    "summaries": summaries,
                    "events": observed_events,
                    "callback_errors": callback_errors,
                    "synthetic_factory_calls": len(synthetic_factory_calls),
                }
            )
        )
        """
    )
    script = (
        script.replace("__FIXTURE_PATH__", repr(str(tmp_path)))
        .replace("__STOCK_MODULE__", repr(stock_module_name))
        .replace("__SOAC_MODULE__", repr(soac_module_name))
    )

    env = dict(os.environ)
    env.pop("SOAC_LOG", None)
    env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(tmp_path / "soac-work"),
            "SOAC_OPT_MODE": "apply",
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        }
    )
    completed = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env=env,
        timeout=45,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    result = json.loads(completed.stdout.splitlines()[-1])
    assert result["callback_errors"] == [], result
    assert result["synthetic_factory_calls"] == 0, result

    target_names = ("<genexpr>", "original_inner", "original_default")
    expected_counts = {name: 3 for name in target_names}
    expected_creation_events = {name: [0, 0, 0] for name in target_names}
    expected_mutations = {
        "<genexpr>": [],
        "original_inner": [],
        "original_default": [3, 4, 5],
    }
    expected_creation_snapshot = {
        "defaults_are_none": True,
        "kwdefaults_are_none": True,
        "closure_is_none": True,
    }

    stock = result["summaries"]["stock"]
    soac = result["summaries"]["soac"]
    for summary in (stock, soac):
        assert summary["created_counts"] == expected_counts, result
        assert summary["distinct_function_counts"] == expected_counts, result
        assert summary["shared_original_code"] == {
            name: True for name in target_names
        }, result
        for name in target_names:
            assert summary["creation_snapshots"][name] == [
                expected_creation_snapshot
            ] * 3, result

    assert result["events"]["initialization"]["stock"] == (
        expected_creation_events
    ), result
    assert result["events"]["mutation"]["stock"] == expected_mutations, result
    assert result["events"]["mutation"]["soac"] == expected_mutations, result

    # Stock MAKE_FUNCTION emits CREATE before SET_FUNCTION_ATTRIBUTE fills the
    # fresh defaults/closure slots. Original SOAC functions must match it.
    assert result["events"]["initialization"]["soac"] == (
        expected_creation_events
    ), result
    for name in target_names:
        assert stock["name_identity"][name] == [True, True, True], result
        assert stock["qualname_identity"][name] == [True, True, True], result
        assert soac["name_identity"][name] == [True, True, True], result
        assert soac["qualname_identity"][name] == [True, True, True], result
