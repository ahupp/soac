from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def test_transformed_functions_preserve_their_captured_builtins_mapping(
    tmp_path: Path,
) -> None:
    module_name = "captured_function_builtins_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            def make_reader(offset):
                def read(value):
                    return sentinel_builtin(value) + offset

                return read
            """
        ),
        encoding="utf-8",
    )

    script = (
        textwrap.dedent(
            """
            import json
            import sys
            import types

            sys.path.insert(0, MODULE_DIRECTORY_TOKEN)
            from soac import _soac_ext
            from soac.import_hook import install

            install()
            import MODULE_NAME_TOKEN as module

            namespace = module.__dict__
            original_builtins = namespace["__builtins__"]
            results = {}

            try:
                first_builtins = {"sentinel_builtin": lambda value: 10 + value}
                second_builtins = {"sentinel_builtin": lambda value: 20 + value}

                namespace["__builtins__"] = first_builtins
                first_reader = module.make_reader(1)
                assert first_reader.__builtins__ is first_builtins

                namespace["__builtins__"] = second_builtins
                second_reader = module.make_reader(2)
                assert second_reader.__builtins__ is second_builtins

                results["captured"] = [first_reader(0), second_reader(0)]
                assert results["captured"] == [11, 22], results

                first_builtins["sentinel_builtin"] = lambda value: 30 + value
                results["mutated"] = [first_reader(1), second_reader(1)]
                assert results["mutated"] == [32, 23], results

                namespace["sentinel_builtin"] = lambda value: 40 + value
                try:
                    results["global_shadow"] = [first_reader(2), second_reader(2)]
                    assert results["global_shadow"] == [43, 44], results
                finally:
                    del namespace["sentinel_builtin"]

                results["shadow_removed"] = [first_reader(3), second_reader(3)]
                assert results["shadow_removed"] == [34, 25], results

                dict_accesses = []

                class ObservableBuiltins(dict):
                    def __getitem__(self, name):
                        dict_accesses.append(name)
                        return dict.__getitem__(self, name)

                observable_builtins = ObservableBuiltins(
                    sentinel_builtin=lambda value: 50 + value
                )
                namespace["__builtins__"] = observable_builtins
                dict_reader = module.make_reader(3)
                assert dict_reader.__builtins__ is observable_builtins
                results["dict_subclass"] = dict_reader(4)
                assert results["dict_subclass"] == 57, results
                assert dict_accesses == ["sentinel_builtin"], dict_accesses

                mapping_accesses = []

                class ObservableMapping:
                    def __getitem__(self, name):
                        mapping_accesses.append(name)
                        if name == "sentinel_builtin":
                            return lambda value: 60 + value
                        raise KeyError(name)

                observable_mapping = ObservableMapping()
                namespace["__builtins__"] = observable_mapping
                mapping_reader = module.make_reader(4)
                assert mapping_reader.__builtins__ is observable_mapping
                results["custom_mapping"] = mapping_reader(5)
                assert results["custom_mapping"] == 69, results
                assert mapping_accesses == ["sentinel_builtin"], mapping_accesses

                builtin_module = types.ModuleType("captured_builtin_module")
                builtin_module.sentinel_builtin = lambda value: 70 + value
                namespace["__builtins__"] = builtin_module
                module_reader = module.make_reader(5)
                assert module_reader.__builtins__ is builtin_module.__dict__
                results["module_builtins"] = module_reader(6)
                assert results["module_builtins"] == 81, results

                builtin_module.sentinel_builtin = lambda value: 80 + value
                results["module_mutated"] = module_reader(7)
                assert results["module_mutated"] == 92, results

                del first_builtins["sentinel_builtin"]
                try:
                    first_reader(0)
                except NameError as error:
                    results["missing"] = str(error)
                    assert "sentinel_builtin" in str(error), error
                else:
                    raise AssertionError("a missing captured builtin must raise NameError")

                entry_builtins = {"sentinel_builtin": lambda value: 90 + value}
                namespace["__builtins__"] = entry_builtins
                previous = _soac_ext.force_entry_interpreter_for_tests(True)
                try:
                    entry_reader = module.make_reader(6)
                    assert entry_reader.__builtins__ is entry_builtins
                    namespace["__builtins__"] = second_builtins
                    results["entry_interpreter"] = entry_reader(8)
                    assert results["entry_interpreter"] == 104, results
                finally:
                    _soac_ext.force_entry_interpreter_for_tests(previous)

                results["dict_accesses"] = dict_accesses
                results["mapping_accesses"] = mapping_accesses
            finally:
                namespace.pop("sentinel_builtin", None)
                namespace["__builtins__"] = original_builtins

            print(json.dumps(results))
            """
        )
        .replace("MODULE_DIRECTORY_TOKEN", repr(str(tmp_path)))
        .replace("MODULE_NAME_TOKEN", module_name)
    )

    base_env = dict(os.environ)
    base_env.pop("SOAC_LOG", None)
    base_env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(tmp_path / "soac-work"),
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        }
    )

    for mode in ("profile", "apply"):
        completed = subprocess.run(
            [sys.executable, "-c", script],
            check=False,
            capture_output=True,
            text=True,
            env={**base_env, "SOAC_OPT_MODE": mode},
            timeout=60,
        )
        assert completed.returncode == 0, (
            f"{mode} mode must resolve globals through each function's captured builtins",
            completed.stdout,
            completed.stderr,
        )
        results = json.loads(completed.stdout.splitlines()[-1])
        assert results["captured"] == [11, 22]
        assert results["mutated"] == [32, 23]
        assert results["global_shadow"] == [43, 44]
        assert results["shadow_removed"] == [34, 25]
        assert results["dict_subclass"] == 57
        assert results["dict_accesses"] == ["sentinel_builtin"]
        assert results["custom_mapping"] == 69
        assert results["mapping_accesses"] == ["sentinel_builtin"]
        assert results["module_builtins"] == 81
        assert results["module_mutated"] == 92
        assert results["entry_interpreter"] == 104
        assert "sentinel_builtin" in results["missing"]
