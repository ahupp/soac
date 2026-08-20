from __future__ import annotations

import json
from pathlib import Path
import textwrap

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import (
    StrictValidationCase,
    _VALIDATION_PRELUDE,
    create_strict_project,
)

_PROFILE_FUNCTIONS = ('Base.value', 'Base.add', 'Base.pair', 'Alternate.value', 'immediate', 'positional', 'two_positional', 'discarded', 'builtin_method')


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
    project, tmp_path: Path, module_name: str, work_dir: Path, mode: str
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
        assert all(function_id(stock[path]) == 0 and sealed_id(stock[path]) == 0
            and native_owner(stock[path]) is None for path in ('immediate', 'positional', 'two_positional', 'discarded', 'builtin_method'))


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

        def exercise_strict(namespace):
            child = namespace["Child"]()
            alternate = namespace["Alternate"]()
            shadowed = namespace["Child"]()
            original_value = function_snapshot(namespace["Base"].value)
            child_type = type_snapshot(namespace["Child"])
            with pytest.raises(StrictMutationError) as caught:
                shadowed.value = lambda: 31
            assert type(caught.value) is StrictMutationError
            assert "value" not in vars(shadowed)
            assert_function_snapshot(namespace["Base"].value, original_value)
            assert_type_snapshot(namespace["Child"], child_type)
            sealed_rejections = ["instance_method_shadow"]

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

            original_method = function_snapshot(namespace["Base"].value)
            base_type = type_snapshot(namespace["Base"])
            with pytest.raises(StrictMutationError) as caught:
                namespace["Base"].value = lambda self: 47
            assert type(caught.value) is StrictMutationError
            assert_function_snapshot(namespace["Base"].value, original_method)
            assert_type_snapshot(namespace["Base"], base_type)
            sealed_rejections.append("class_method_replace")
            with pytest.raises(StrictMutationError) as caught:
                del namespace["Base"].value
            assert type(caught.value) is StrictMutationError
            assert_function_snapshot(namespace["Base"].value, original_method)
            assert_type_snapshot(namespace["Base"], base_type)
            sealed_rejections.append("class_method_delete")
            outcomes["replaced_inherited"] = namespace["immediate"](child)

            original_defaults = function_snapshot(namespace["Base"].add)
            with pytest.raises(StrictMutationError) as caught:
                namespace["Base"].add.__defaults__ = (9,)
            assert type(caught.value) is StrictMutationError
            assert_function_snapshot(namespace["Base"].add, original_defaults)
            assert_type_snapshot(namespace["Base"], base_type)
            sealed_rejections.append("function_defaults")
            outcomes["mutated_default"] = namespace["positional"](child, 7)
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

            original_code = function_snapshot(method)
            with pytest.raises(StrictMutationError) as caught:
                method.__code__ = replacement.__code__
            assert type(caught.value) is StrictMutationError
            assert_function_snapshot(method, original_code)
            assert_type_snapshot(namespace["Base"], base_type)
            sealed_rejections.append("function_code")
            outcomes["replaced_code"] = namespace["immediate"](child)
            outcomes["restored_code"] = namespace["immediate"](child)

            # Public vectorcall changes are allowed; all native source owner,
            # code/default/strict-ID facts survive the original clear/restore controls.
            assert_function_snapshot(namespace["Base"].value, original_method)
            assert_function_snapshot(namespace["Base"].add, original_defaults)
            assert_type_snapshot(namespace["Base"], base_type)
            outcomes["sealed_rejections"] = sealed_rejections
            return outcomes

        stock_outcomes = exercise(stock)
        assert all(function_id(stock[path]) == 0 and sealed_id(stock[path]) == 0
            and native_owner(stock[path]) is None for path in ('immediate', 'positional', 'two_positional', 'discarded', 'builtin_method'))
        transformed_outcomes = exercise_strict(module.__dict__)
        # Preserve the complete ordinary mutation exercise. The authenticated
        # arm changes only the four explicit sealed-mutation results below.
        strict_expected = dict(
            stock_outcomes, shadowed=11, replaced_inherited=11,
            mutated_default=10, replaced_code=11,
            sealed_rejections=["instance_method_shadow", "class_method_replace",
                "class_method_delete", "function_defaults", "function_code"],
        )
        assert transformed_outcomes == strict_expected, (
            transformed_outcomes, strict_expected, stock_outcomes,
        )

        # The existing outer ordinary assertions retain their exact inputs;
        # authenticated outcomes and every named rejection are reported separately.
        print(json.dumps({"mode": mode, "outcomes": stock_outcomes,
            "strict_outcomes": transformed_outcomes}))
        """
    )
    script = (
        script.replace("__ROOT__", repr(str(tmp_path)))
        .replace("__NAME__", repr(module_name))
        .replace("__MODE__", repr(mode))
    )
    witnesses = f"""
import ctypes
import pytest
from soac.strict import StrictMutationError
from tests._strict_integration import _plain_function_witness
function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
function_id.argtypes = [ctypes.py_object]
function_id.restype = ctypes.c_uint64
sealed_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
sealed_id.argtypes = [ctypes.py_object]
sealed_id.restype = ctypes.c_uint64
native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
native_owner.argtypes = [ctypes.py_object]
native_owner.restype = ctypes.c_void_p
type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
type_owner.argtypes = [ctypes.py_object]
type_owner.restype = ctypes.c_void_p
type_sealed = ctypes.pythonapi.PyType_IsSoacSealed
type_sealed.argtypes = [ctypes.py_object]
type_sealed.restype = ctypes.c_int
def assert_profile_functions():
    for path in {_PROFILE_FUNCTIONS!r}:
        function = _plain_function_witness(module, path)
        # The old ID grants unchecked dispatch, not source admission.
        assert function_id(function) == 0, path
        assert sealed_id(function) > 0, path
        assert native_owner(function), path
def function_snapshot(function):
    assert function_id(function) == 0 and sealed_id(function) > 0
    assert native_owner(function)
    return (function, function.__code__, function.__defaults__, function.__kwdefaults__,
        native_owner(function), function_id(function), sealed_id(function))
def assert_function_snapshot(function, saved):
    assert function is saved[0] and function.__code__ is saved[1]
    assert function.__defaults__ is saved[2] and function.__kwdefaults__ is saved[3]
    assert native_owner(function) == saved[4]
    assert function_id(function) == saved[5] and sealed_id(function) == saved[6]
def type_snapshot(cls):
    owner = type_owner(cls)
    assert owner and type_sealed(cls) == 1
    return (cls, owner)
def assert_type_snapshot(cls, saved):
    assert cls is saved[0] and type_owner(cls) == saved[1]
    assert type_sealed(cls) == 1
assert_profile_functions()
"""
    validation = "def validate_module(module):\n" + textwrap.indent(
        witnesses + script + "\nassert_profile_functions()\n", "    "
    )
    program = _VALIDATION_PRELUDE + project._validation_program(
        module_name,
        StrictValidationCase(
            validation, Path(__file__), required_functions=_PROFILE_FUNCTIONS,
            
        ),
        entry_interpreter=False,
    )
    completed = project.run(
        program, opt_mode=mode, extra_env={"SOAC_WORK_DIR": str(work_dir)},
        timeout=90, check=False,
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
    # The original ordinary file/source remains the stock control.
    # Only this separately analyzed copy acquires startup-selected authority.
    relative = f"{module_name}.py"
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in((tmp_path / relative).read_bytes(), relative)[0].decode()},
        modules={module_name: relative},
    )

    work_dir = tmp_path / "soac-work"
    results = {
        mode: _run_mode(project, tmp_path, module_name, work_dir, mode)
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

        strict_outcomes = result["strict_outcomes"]
        expected_strict = dict(
            outcomes, shadowed=11, replaced_inherited=11, mutated_default=10,
            replaced_code=11,
            sealed_rejections=["instance_method_shadow", "class_method_replace",
                "class_method_delete", "function_defaults", "function_code"],
        )
        assert strict_outcomes == expected_strict, (mode, strict_outcomes, expected_strict)


def test_profiled_inherited_method_descriptors_keep_checked_entries(
    tmp_path: Path,
) -> None:
    module_name = "profiled_inherited_method_descriptor_direct_calls_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(_SOURCE), encoding="utf-8"
    )
    # The original ordinary file/source remains the stock control.
    # Only this separately analyzed copy acquires startup-selected authority.
    relative = f"{module_name}.py"
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in((tmp_path / relative).read_bytes(), relative)[0].decode()},
        modules={module_name: relative},
    )

    work_dir = tmp_path / "soac-work"
    _run_mode(project, tmp_path, module_name, work_dir, "profile")
    _run_mode(project, tmp_path, module_name, work_dir, "verify")

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
    assert hit_count == 0, (
        "observational profile identities do not authorize unchecked direct "
        "bodies; the actual owner/seal and checked-entry witnesses above must "
        "remain enforced after authoritative descriptor lookup",
        direct_rows,
    )
    _run_mode(project, tmp_path, module_name, work_dir, "apply")
