from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


_SOURCE = """
class Base:
    def value(self):
        return 11

    def add(self, value, increment=3):
        return value + increment

    def pair(self, first, second):
        return first + second


class Child(Base):
    pass


class Alternate(Base):
    def value(self):
        return 29


class DataDescriptor:
    def __get__(self, instance, owner):
        return lambda: 37

    def __set__(self, instance, value):
        instance.ignored = value


class DescriptorChild(Child):
    value = DataDescriptor()


class CustomChild(Child):
    def __getattribute__(self, name):
        if name == "value":
            return lambda: 41
        return object.__getattribute__(self, name)


def immediate(owner):
    return owner.value()


def positional(owner, value):
    return owner.add(value)


def two_positional(owner, first, second):
    return owner.pair(first, second)


def discarded(owner):
    owner.value()


def builtin_method(mapping, key):
    return mapping.get(key)
"""


def _run_mode(
    tmp_path: Path, module_name: str, work_dir: Path, mode: str
) -> dict[str, object]:
    script = textwrap.dedent(
        """
        import builtins
        import ctypes
        import importlib
        import json
        import sys

        root, name, mode = __ROOT__, __NAME__, __MODE__
        source = open(root + "/" + name + ".py", encoding="utf-8").read()
        stock = {
            "__name__": "stock_resolved_method_descriptor_direct_calls",
            "__builtins__": builtins.__dict__,
        }
        exec(compile(source, "<stock-resolved-method-descriptor>", "exec"), stock)

        sys.path.insert(0, root)
        from soac.import_hook import install

        install()
        module = importlib.import_module(name)

        get_vectorcall = ctypes.pythonapi.PyVectorcall_Function
        get_vectorcall.argtypes = [ctypes.py_object]
        get_vectorcall.restype = ctypes.c_void_p
        set_vectorcall = ctypes.pythonapi.PyFunction_SetVectorcall
        set_vectorcall.argtypes = [ctypes.py_object, ctypes.c_void_p]
        set_vectorcall.restype = None

        def exercise(namespace):
            child = namespace["Child"]()
            alternate = namespace["Alternate"]()
            shadowed = namespace["Child"]()
            shadowed.value = lambda: 31

            outcomes = {
                "inherited": namespace["immediate"](child),
                "alternate": namespace["immediate"](alternate),
                "shadowed": namespace["immediate"](shadowed),
                "data_descriptor": namespace["immediate"](
                    namespace["DescriptorChild"]()
                ),
                "custom_getattribute": namespace["immediate"](
                    namespace["CustomChild"]()
                ),
                "one_positional": namespace["positional"](child, 7),
                "two_positional": namespace["two_positional"](child, 7, 5),
                "effect_only": namespace["discarded"](child),
                "builtin_method": namespace["builtin_method"]({"present": 17}, "present"),
            }

            original_method = namespace["Base"].value
            namespace["Base"].value = lambda self: 47
            try:
                outcomes["replaced_inherited"] = namespace["immediate"](child)
            finally:
                namespace["Base"].value = original_method

            original_defaults = namespace["Base"].add.__defaults__
            namespace["Base"].add.__defaults__ = (9,)
            try:
                outcomes["mutated_default"] = namespace["positional"](child, 7)
            finally:
                namespace["Base"].add.__defaults__ = original_defaults
            outcomes["restored_default"] = namespace["positional"](child, 7)

            for value in range(48):
                assert namespace["immediate"](child) == 11
                assert namespace["immediate"](alternate) == 29
                assert namespace["positional"](child, value) == value + 3
                assert namespace["two_positional"](child, value, 2) == value + 2
                assert namespace["discarded"](child) is None

            method = namespace["Base"].value
            original_vectorcall = get_vectorcall(method)
            assert original_vectorcall
            set_vectorcall(method, None)
            try:
                try:
                    namespace["immediate"](child)
                except TypeError:
                    outcomes["cleared_vectorcall"] = "TypeError"
                else:
                    raise AssertionError("cleared method vectorcall was ignored")
            finally:
                set_vectorcall(method, original_vectorcall)
            outcomes["restored_vectorcall"] = namespace["immediate"](child)

            def replacement(self):
                return 53

            original_code = method.__code__
            method.__code__ = replacement.__code__
            try:
                outcomes["replaced_code"] = namespace["immediate"](child)
            finally:
                method.__code__ = original_code
            outcomes["restored_code"] = namespace["immediate"](child)

            return outcomes

        stock_outcomes = exercise(stock)
        transformed_outcomes = exercise(module.__dict__)
        assert transformed_outcomes == stock_outcomes, (
            transformed_outcomes,
            stock_outcomes,
        )

        print(json.dumps({"mode": mode, "outcomes": transformed_outcomes}))
        """
    )
    script = (
        script.replace("__ROOT__", repr(str(tmp_path)))
        .replace("__NAME__", repr(module_name))
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
        capture_output=True,
        check=False,
        env=environment,
        text=True,
        timeout=90,
    )
    assert completed.returncode == 0, (
        f"{mode} resolved-method descriptor worker failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    return json.loads(completed.stdout.splitlines()[-1])


def test_resolved_method_descriptor_direct_calls_preserve_cpython_dispatch(
    tmp_path: Path,
) -> None:
    module_name = "resolved_method_descriptor_direct_calls_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(_SOURCE), encoding="utf-8"
    )
    work_dir = tmp_path / "soac-work"
    results = {
        mode: _run_mode(tmp_path, module_name, work_dir, mode)
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
    for function in ("immediate", "positional", "two_positional", "discarded"):
        assert any(
            row["kind"] == "call_hot_targets"
            and row["function_qualname"] == function
            and row["value"] >= 48
            for record in records
            for row in record["rows"]
        ), (function, records)

    native_rows = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl")
        .read_text(encoding="utf-8")
        .splitlines()
        if line.strip()
    ]
    for function in (
        "Base.value",
        "Base.add",
        "Base.pair",
        "Alternate.value",
        "immediate",
        "positional",
        "two_positional",
        "discarded",
    ):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == function
            for row in native_rows
        ), (function, native_rows)

    for mode, result in results.items():
        outcomes = result["outcomes"]
        assert outcomes["inherited"] == 11, (mode, outcomes)
        assert outcomes["alternate"] == 29, (mode, outcomes)
        assert outcomes["shadowed"] == 31, (mode, outcomes)
        assert outcomes["data_descriptor"] == 37, (mode, outcomes)
        assert outcomes["custom_getattribute"] == 41, (mode, outcomes)
        assert outcomes["replaced_inherited"] == 47, (mode, outcomes)
        assert outcomes["mutated_default"] == 16, (mode, outcomes)
        assert outcomes["restored_default"] == 10, (mode, outcomes)
        assert outcomes["cleared_vectorcall"] == "TypeError", (mode, outcomes)
        assert outcomes["restored_vectorcall"] == 11, (mode, outcomes)
        assert outcomes["replaced_code"] == 53, (mode, outcomes)
        assert outcomes["restored_code"] == 11, (mode, outcomes)


def test_profiled_inherited_method_descriptors_use_direct_calls(
    tmp_path: Path,
) -> None:
    module_name = "profiled_inherited_method_descriptor_direct_calls_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(_SOURCE), encoding="utf-8"
    )
    work_dir = tmp_path / "soac-work"
    _run_mode(tmp_path, module_name, work_dir, "profile")
    _run_mode(tmp_path, module_name, work_dir, "verify")

    from soac import _soac_ext

    profile = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    profile_rows = [
        row
        for record in profile["records"]
        if record["module_name"] == module_name
        for row in record["rows"]
        if row["kind"] == "call_hot_targets"
        and row["function_qualname"] == "immediate"
        and row["value"] >= 48
        and row.get("observed_value")
    ]
    assert profile_rows, profile

    verify = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "verify.bin"))
    )
    direct_rows = [
        row
        for record in verify["records"]
        if record["module_name"] == module_name
        for row in record["rows"]
        if row["kind"] == "call_direct"
        and row["function_qualname"] == "immediate"
    ]
    assert direct_rows, verify
    hit_count = sum(row["branches"].get("hit", 0) for row in direct_rows)
    assert hit_count > 0, (
        "an eagerly resolved inherited Python method must reach a profiled "
        "direct native body after authoritative descriptor lookup",
        direct_rows,
    )
