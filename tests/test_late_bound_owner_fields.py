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

_PROFILE_FUNCTIONS = (
    'Point.__init__',
    'Point.read',
    'Point.write',
    'Record.__init__',
    'Record.read',
    'Record.write',
    'UnseededRecord.__init__',
    'UnseededRecord.read',
    'ObservedPoint.__getattribute__',
    'ObservedPoint.__setattr__',
    'WatchedValue.__init__',
    'WatchedValue.__del__',
    'StaticReader.read',
    'exercise',
    'transient_class',
)


def test_eager_late_bound_slot_and_split_fields_preserve_python_semantics(
    tmp_path: Path,
) -> None:
    module_name = "late_bound_owner_fields_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            EVENTS = []


            class Point:
                __slots__ = ("value",)

                def __init__(self, value):
                    self.value = value

                def read(self):
                    return self.value

                def write(self, value):
                    self.value = value
                    return self.value


            class Record:
                def __init__(self, value):
                    self.value = value

                def read(self):
                    return self.value

                def write(self, value):
                    self.value = value
                    return self.value


            class UnseededRecord:
                def __init__(instance, first, middle, mark):
                    instance.first = first
                    instance.middle = middle
                    instance.mark = mark

                def read(self):
                    return self.first, self.middle, self.mark


            class ObservedPoint(Point):
                __slots__ = ()

                def __getattribute__(self, name):
                    if name == "value":
                        EVENTS.append("subclass:get")
                    return object.__getattribute__(self, name)

                def __setattr__(self, name, value):
                    if name == "value":
                        EVENTS.append(("subclass:set", value))
                    return object.__setattr__(self, name, value)


            class WatchedValue:
                __slots__ = ("owner", "name")

                def __init__(self, owner, name):
                    self.owner = owner
                    self.name = name

                def __del__(self):
                    EVENTS.append(("destroyed", self.name, self.owner.read()))


            class StaticReader:
                @staticmethod
                def read(value):
                    return value.value


            def exercise(point, record, value):
                point.write(value)
                record.write(value)
                return point.read() + record.read()


            def transient_class():
                class Temporary:
                    __slots__ = ("value",)

                    def __init__(self, value):
                        self.value = value

                    def read(self):
                        return self.value

                return Temporary
            """
        ),
        encoding="utf-8",
    )

    script = textwrap.dedent(
        """
        import gc
        import sys
        import weakref

        import builtins
        from pathlib import Path
        import types

        name = __NAME__
        source = Path(__ORIGINAL_SOURCE__).read_text(encoding="utf-8")
        stock = types.ModuleType(name)
        stock.__dict__["__builtins__"] = builtins.__dict__
        exec(compile(source, "<stock-owner-fields>", "exec"), stock.__dict__)

        # This ordinary arm retains the complete original workload and validator.
        def exercise(module):
            point = module.Point(0)
            record = module.Record(0)
            for value in range(16):
                assert point.write(value) == value
                assert record.write(value) == value
                assert point.read() == value
                assert record.read() == value
                assert module.exercise(point, record, value) == value * 2

            assert module.UnseededRecord.__static_attributes__ == ()
            for value in range(16):
                fresh = module.UnseededRecord(value, value + 1, value + 2)
                module.UnseededRecord.__init__(fresh, value, value + 1, value + 2)
                assert object.__getattribute__(fresh, "first") == value
                assert object.__getattribute__(fresh, "mark") == value + 2
                assert fresh.read() == (value, value + 1, value + 2)
            assert fresh.__dict__ == {
                "first": 15,
                "middle": 16,
                "mark": 17,
            }

            assert module.StaticReader.read(point) == 15

            observed = module.ObservedPoint(4)
            module.EVENTS.clear()
            assert observed.read() == 4
            assert observed.write(8) == 8
            assert module.EVENTS == [
                "subclass:get", ("subclass:set", 8), "subclass:get"
            ], module.EVENTS

            missing = module.Point(3)
            del missing.value
            try:
                missing.read()
            except AttributeError as error:
                assert "value" in str(error), error
            else:
                raise AssertionError("a deleted slot must raise AttributeError")

            missing_record = module.Record(3)
            del missing_record.value
            try:
                missing_record.read()
            except AttributeError as error:
                assert "value" in str(error), error
            else:
                raise AssertionError("a deleted split-dict field must raise AttributeError")
            assert missing_record.write(9) == 9

            materialized = module.Record(11)
            materialized_dict = materialized.__dict__
            assert materialized_dict["value"] == 11
            materialized_dict["value"] = 12
            assert materialized.read() == 12
            materialized_dict[1] = "promoted"
            assert materialized.write(13) == 13
            assert materialized_dict["value"] == 13
            del materialized_dict["value"]
            try:
                materialized.read()
            except AttributeError as error:
                assert "value" in str(error), error
            else:
                raise AssertionError("a removed promoted-dict field must raise AttributeError")

            module.EVENTS.clear()
            point.write(module.WatchedValue(point, "slot"))
            assert point.write("slot-new") == "slot-new"
            assert module.EVENTS == [("destroyed", "slot", "slot-new")], module.EVENTS

            module.EVENTS.clear()
            record.write(module.WatchedValue(record, "split"))
            assert record.write("split-new") == "split-new"
            assert module.EVENTS == [("destroyed", "split", "split-new")], module.EVENTS

            first_type = module.transient_class()
            second_type = module.transient_class()
            assert first_type is not second_type
            first = first_type(21)
            second = second_type(34)
            assert first.read() == 21
            assert second.read() == 34
            first_reference = weakref.ref(first_type)
            second_reference = weakref.ref(second_type)
            del first, second, first_type, second_type
            gc.collect()
            assert first_reference() is None
            assert second_reference() is None

            original_descriptor = module.Point.__dict__["value"]

            def replacement_get(instance):
                module.EVENTS.append("property:get")
                return 701

            def replacement_set(instance, value):
                module.EVENTS.append(("property:set", value))

            module.Point.value = property(replacement_get, replacement_set)
            try:
                module.EVENTS.clear()
                assert point.read() == 701
                assert point.write(18) == 701
                assert module.EVENTS == [
                    "property:get", ("property:set", 18), "property:get"
                ], module.EVENTS
            finally:
                module.Point.value = original_descriptor

            assert point.read() == "slot-new"

            def record_replacement_get(instance):
                module.EVENTS.append("record-property:get")
                return 801

            def record_replacement_set(instance, value):
                module.EVENTS.append(("record-property:set", value))

            module.Record.value = property(record_replacement_get, record_replacement_set)
            try:
                module.EVENTS.clear()
                assert record.read() == 801
                assert record.write(18) == 801
                assert module.EVENTS == [
                    "record-property:get", ("record-property:set", 18),
                    "record-property:get",
                ], module.EVENTS
            finally:
                del module.Record.value

            assert record.read() == "split-new"

        # Selected classes/modules are sealed; only their mutation outcomes differ.
        def exercise_strict(module):
            point = module.Point(0)
            record = module.Record(0)
            for value in range(16):
                assert point.write(value) == value
                assert record.write(value) == value
                assert point.read() == value
                assert record.read() == value
                assert module.exercise(point, record, value) == value * 2

            assert module.UnseededRecord.__static_attributes__ == ()
            for value in range(16):
                fresh = module.UnseededRecord(value, value + 1, value + 2)
                module.UnseededRecord.__init__(fresh, value, value + 1, value + 2)
                assert object.__getattribute__(fresh, "first") == value
                assert object.__getattribute__(fresh, "mark") == value + 2
                assert fresh.read() == (value, value + 1, value + 2)
            assert fresh.__dict__ == {
                "first": 15,
                "middle": 16,
                "mark": 17,
            }

            assert module.StaticReader.read(point) == 15

            observed = module.ObservedPoint(4)
            module.EVENTS.clear()
            assert observed.read() == 4
            assert observed.write(8) == 8
            assert module.EVENTS == [
                "subclass:get", ("subclass:set", 8), "subclass:get"
            ], module.EVENTS

            missing = module.Point(3)
            del missing.value
            try:
                missing.read()
            except AttributeError as error:
                assert "value" in str(error), error
            else:
                raise AssertionError("a deleted slot must raise AttributeError")

            missing_record = module.Record(3)
            del missing_record.value
            try:
                missing_record.read()
            except AttributeError as error:
                assert "value" in str(error), error
            else:
                raise AssertionError("a deleted split-dict field must raise AttributeError")
            assert missing_record.write(9) == 9

            materialized = module.Record(11)
            materialized_dict = materialized.__dict__
            assert _testinternalcapi.dict_has_indexed_keys(materialized_dict) is False
            assert materialized_dict["value"] == 11
            materialized_dict["value"] = 12
            assert materialized.read() == 12
            materialized_dict[1] = "promoted"
            assert _testinternalcapi.dict_has_indexed_keys(materialized_dict) is False
            assert materialized.write(13) == 13
            assert materialized_dict["value"] == 13
            del materialized_dict["value"]
            try:
                materialized.read()
            except AttributeError as error:
                assert "value" in str(error), error
            else:
                raise AssertionError("a removed promoted-dict field must raise AttributeError")

            module.EVENTS.clear()
            point.write(module.WatchedValue(point, "slot"))
            assert point.write("slot-new") == "slot-new"
            assert module.EVENTS == [("destroyed", "slot", "slot-new")], module.EVENTS

            module.EVENTS.clear()
            record.write(module.WatchedValue(record, "split"))
            assert record.write("split-new") == "split-new"
            assert module.EVENTS == [("destroyed", "split", "split-new")], module.EVENTS

            first_type = module.transient_class()
            second_type = module.transient_class()
            assert first_type is not second_type
            # These callback-free observations do not retain either transient type.
            assert type_owner(first_type) and type_owner(second_type)
            assert type_owner(first_type) != type_owner(second_type)
            assert_object_slot(first_type, "value")
            assert_object_slot(second_type, "value")
            assert_native_function(first_type.__dict__["__init__"])
            assert_native_function(first_type.__dict__["read"])
            assert_native_function(second_type.__dict__["__init__"])
            assert_native_function(second_type.__dict__["read"])
            first = first_type(21)
            second = second_type(34)
            assert first.read() == 21
            assert second.read() == 34
            first_reference = weakref.ref(first_type)
            second_reference = weakref.ref(second_type)
            del first, second, first_type, second_type
            gc.collect()
            assert first_reference() is None
            assert second_reference() is None

            original_descriptor = module.Point.__dict__["value"]

            def replacement_get(instance):
                module.EVENTS.append("property:get")
                return 701

            def replacement_set(instance, value):
                module.EVENTS.append(("property:set", value))

            with reject_sealed_mutation("native_slot_descriptor"):
                module.Point.value = property(replacement_get, replacement_set)
            assert module.Point.__dict__["value"] is original_descriptor
            assert_object_slot(module.Point, "value")
            module.EVENTS.clear()
            assert point.read() == "slot-new"
            assert point.write(18) == 18
            assert module.EVENTS == [], module.EVENTS
            assert point.read() == 18

            def record_replacement_get(instance):
                module.EVENTS.append("record-property:get")
                return 801

            def record_replacement_set(instance, value):
                module.EVENTS.append(("record-property:set", value))

            with reject_sealed_mutation("dictionary_descriptor"):
                module.Record.value = property(record_replacement_get, record_replacement_set)
            module.EVENTS.clear()
            assert record.read() == "split-new"
            assert record.write(18) == 18
            assert module.EVENTS == [], module.EVENTS

            assert record.read() == 18

        assert_ordinary_bindings(stock)
        exercise(stock)
        assert_ordinary_bindings(stock)
        exercise_strict(module)
        assert sealed_rejections == ['native_slot_descriptor', 'dictionary_descriptor']
        """
    )

    # Preserve the exact original file for ordinary execution. Only this
    # separately analyzed, future-only copy receives startup authority.
    relative = f"{module_name}.py"
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in((tmp_path / relative).read_bytes(), relative)[0].decode()},
        modules={module_name: relative},
    )

    script = (
        script.replace("__NAME__", repr(module_name))
        .replace("__ORIGINAL_SOURCE__", repr(str(tmp_path / f"{module_name}.py")))
    )
    witnesses = f"""
import ctypes
import types
from contextlib import contextmanager
import _testinternalcapi
import pytest
from soac import _soac_ext
from soac.strict import StrictMutationError
from tests._strict_integration import _plain_function_witness

# This legacy slot authorizes unchecked direct targets, not source identity.
# Source-owned entries deliberately leave it zero.
unchecked_target_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
unchecked_target_id.argtypes = [ctypes.py_object]
unchecked_target_id.restype = ctypes.c_uint64
sealed_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
sealed_id.argtypes = [ctypes.py_object]
sealed_id.restype = ctypes.c_uint64
native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
native_owner.argtypes = [ctypes.py_object]
native_owner.restype = ctypes.c_void_p
native_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
native_metadata.argtypes = [ctypes.py_object]
native_metadata.restype = ctypes.c_void_p
type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
type_owner.argtypes = [ctypes.py_object]
type_owner.restype = ctypes.c_void_p
type_sealed = ctypes.pythonapi.PyType_IsSoacSealed
type_sealed.argtypes = [ctypes.py_object]
type_sealed.restype = ctypes.c_int
slot_matches = ctypes.pythonapi.PyType_MatchesSoacObjectSlotDescriptor
slot_matches.argtypes = [ctypes.py_object, ctypes.c_void_p,
    ctypes.c_ssize_t, ctypes.py_object]
slot_matches.restype = ctypes.c_int

dynamic_source_methods = (
    'ObservedPoint.__getattribute__', 'ObservedPoint.__setattr__',
)

def assert_native_function(function, path=None):
    assert type(function) is types.FunctionType, (path, type(function))
    path = function.__qualname__ if path is None else path
    actual = (path, unchecked_target_id(function), sealed_id(function),
        native_owner(function), native_metadata(function),
        _soac_ext.strict_function_entry_kind(function))
    assert actual[1] == 0, actual
    # These exact custom-hook methods are owned selected source functions, but
    # their statically dynamic class does not authorize a permanent method seal.
    if path in dynamic_source_methods:
        assert actual[2] == 0, actual
    else:
        assert actual[2] > 0, actual
    assert actual[3], actual
    assert actual[4], actual
    assert actual[5] == "checked_native", actual

def function_snapshot(function, path=None):
    assert_native_function(function, path)
    return (function, function.__code__, function.__defaults__,
        function.__kwdefaults__, unchecked_target_id(function), sealed_id(function),
        native_owner(function))

def assert_function_snapshot(function, saved, path=None):
    assert_native_function(function, path)
    assert function is saved[0] and function.__code__ is saved[1]
    assert function.__defaults__ is saved[2]
    assert function.__kwdefaults__ is saved[3]
    assert unchecked_target_id(function) == saved[4]
    assert sealed_id(function) == saved[5]
    assert native_owner(function) == saved[6]

def type_snapshot(cls):
    assert type(cls) is type
    owner = type_owner(cls)
    assert owner and type_sealed(cls) == 1
    return (cls, owner, cls.__bases__, tuple(cls.__dict__.items()))

def assert_type_snapshot(cls, saved):
    assert cls is saved[0] and type_owner(cls) == saved[1]
    assert type_sealed(cls) == 1 and cls.__bases__ is saved[2]
    actual = cls.__dict__
    assert len(actual) == len(saved[3])
    for name, value in saved[3]:
        assert actual[name] is value, (cls, name)

def assert_object_slot(cls, name):
    owner = type_owner(cls)
    descriptor = cls.__dict__[name]
    assert owner and type_sealed(cls) == 1
    assert type(descriptor) is types.MemberDescriptorType
    assert descriptor.__objclass__ is cls and descriptor.__name__ == name
    assert cls.__dictoffset__ == 0
    assert slot_matches(cls, owner, 0, descriptor) == 1

saved_functions = {{path: function_snapshot(_plain_function_witness(module, path), path)
    for path in {_PROFILE_FUNCTIONS!r}}}
saved_types = {{name: type_snapshot(module.__dict__[name])
    for name in ('Point', 'Record', 'UnseededRecord', 'WatchedValue', 'StaticReader')}}

def assert_selected_bindings():
    for path, saved in saved_functions.items():
        assert_function_snapshot(_plain_function_witness(module, path), saved, path)
    for name, saved in saved_types.items():
        assert_type_snapshot(module.__dict__[name], saved)
    # Source functions remain owned, but custom attribute hooks select an
    # ordinary subclass. Inherited checks and actual hooks still run below.
    for name in ('ObservedPoint',):
        cls = module.__dict__[name]
        assert type_owner(cls) is None, name
        assert type_sealed(cls) == 0, name
    for name, field in (('Point', 'value'),):
        assert_object_slot(module.__dict__[name], field)

def assert_ordinary_bindings(stock):
    for path in {_PROFILE_FUNCTIONS!r}:
        function = _plain_function_witness(stock, path)
        assert unchecked_target_id(function) == 0, path
        assert sealed_id(function) == 0, path
        assert native_owner(function) is None, path
        assert native_metadata(function) is None, path

sealed_rejections = []

@contextmanager
def reject_sealed_mutation(name):
    assert_selected_bindings()
    with pytest.raises(StrictMutationError) as caught:
        yield
    assert type(caught.value) is StrictMutationError, (name, caught.value)
    assert_selected_bindings()
    sealed_rejections.append(name)

assert_selected_bindings()
"""
    validation = "def validate_module(module):\n" + textwrap.indent(
        witnesses + script + "\nassert_selected_bindings()\n", "    "
    )
    program = _VALIDATION_PRELUDE + project._validation_program(
        module_name,
        StrictValidationCase(
            validation, Path(__file__), required_functions=_PROFILE_FUNCTIONS,
            
        ),
        entry_interpreter=False,
    )

    work_dir = tmp_path / "soac-work"

    profile = project.run(
                  program, opt_mode='profile',
                  extra_env={"SOAC_WORK_DIR": str(work_dir)},
                  timeout=60, check=False,
              )
    assert profile.returncode == 0, profile.stdout + profile.stderr

    from soac import _soac_ext

    profile_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    profile_records = [
        record
        for record in profile_dump["records"]
        if record["module_name"] == module_name
    ]
    assert profile_records, profile_dump

    def field_counts(records: list[dict], branch: str) -> dict[str, int]:
        counts: dict[str, int] = {}
        for record in records:
            for row in record["rows"]:
                if row["kind"] == "field_access":
                    qualname = row["function_qualname"]
                    counts[qualname] = counts.get(qualname, 0) + row.get(
                        "branches", {}
                    ).get(branch, 0)
        return counts

    profile_gets = field_counts(profile_records, "generic_getattr")
    profile_sets = field_counts(profile_records, "generic_setattr")
    for owner in ("Point", "Record"):
        assert profile_gets.get(f"{owner}.read", 0) >= 16, profile_gets
        assert profile_sets.get(f"{owner}.write", 0) >= 16, profile_sets
    assert profile_sets.get("UnseededRecord.__init__", 0) >= 48, profile_sets

    owner_entries = [
        entry
        for record in profile_dump["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
        and entry["qualname"] in {"Point", "Record"}
    ]
    assert not any(entry["qualname"] == "Point" for entry in owner_entries), (
        "slot specialization must not require adding a synthetic type-key profile",
        owner_entries,
    )
    record_type_ids = {
        entry["type_id"]
        for entry in owner_entries
        if entry["qualname"] == "Record"
    }
    assert record_type_ids, owner_entries
    assert any(
        key["owner_type_id"] in record_type_ids and key["key"] == "value"
        for record in profile_dump["records"]
        for key in record["type_keys"]
    ), profile_dump
    unseeded_type_ids = {
        entry["type_id"]
        for record in profile_dump["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
        and entry["qualname"] == "UnseededRecord"
    }
    assert {
        key["key"]
        for record in profile_dump["records"]
        for key in record["type_keys"]
        if key["owner_type_id"] in unseeded_type_ids
    } >= {"first", "middle", "mark"}, profile_dump

    verify = project.run(
                 program, opt_mode='verify',
                 extra_env={"SOAC_WORK_DIR": str(work_dir)},
                 timeout=60, check=False,
             )
    assert verify.returncode == 0, verify.stdout + verify.stderr

    verify_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "verify.bin"))
    )
    verify_records = [
        record
        for record in verify_dump["records"]
        if record["module_name"] == module_name
    ]
    assert verify_records, verify_dump
    indexed_hits = field_counts(verify_records, "indexed_hit")
    # Native member identity is actually installed for Point, and its reads
    # must still use that capability. Record's ordinary dictionaries have no
    # indexed-layout capability (also proved by the unchanged validator).
    for name in ("Point.read", "Point.write"):
        assert indexed_hits.get(name, 0) >= 16, (name, indexed_hits)
    for name in ("Record.read", "Record.write"):
        assert indexed_hits.get(name, 0) == 0, (name, indexed_hits)
    checked_reads = field_counts(verify_records, "indexed_fallback")
    checked_writes = field_counts(verify_records, "generic_setattr")
    for name in ("Record.read", "Record.write"):
        assert checked_reads.get(name, 0) >= 16, (name, checked_reads)
    for name in ("Point.write", "Record.write"):
        assert checked_writes.get(name, 0) >= 16, (name, checked_writes)

    apply = project.run(
                program, opt_mode='apply',
                extra_env={"SOAC_WORK_DIR": str(work_dir)},
                timeout=60, check=False,
            )
    assert apply.returncode == 0, apply.stdout + apply.stderr
