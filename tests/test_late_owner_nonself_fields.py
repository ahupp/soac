from __future__ import annotations

import json
from pathlib import Path
import textwrap

import pytest

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._integration import exec_integration_validation, stock_module
from tests._strict_integration import (
    StrictValidationCase,
    _VALIDATION_PRELUDE,
    create_strict_project,
)


# Shared by the two late-owner compatibility fixtures. These are observations
# of actual native objects, never synthesized source IDs or runtime authority.
_OWNER_WITNESSES = """
import ctypes
import gc
import os
import types
import weakref
from contextlib import contextmanager
import _testinternalcapi
import pytest
from soac import _soac_ext
from soac.strict import StrictMutationError
from tests._strict_integration import _plain_function_witness

apis = {}
for name, result in (
    ('PyFunction_GetSoacFunctionId', ctypes.c_uint64),
    ('PyFunction_GetSoacStrictId', ctypes.c_uint64),
    ('PyFunction_GetSoacStrictOwner', ctypes.c_void_p),
    ('PyFunction_GetSoacMetadata', ctypes.c_void_p),
    ('PyType_GetSoacContractOwner', ctypes.c_void_p),
    ('PyType_IsSoacSealed', ctypes.c_int),
):
    api = getattr(ctypes.pythonapi, name)
    api.argtypes = [ctypes.py_object]
    api.restype = result
    apis[name] = api

def function_snapshot(path):
    function = _plain_function_witness(module, path)
    actual = tuple(apis[name](function) for name in (
        'PyFunction_GetSoacFunctionId', 'PyFunction_GetSoacStrictId',
        'PyFunction_GetSoacStrictOwner', 'PyFunction_GetSoacMetadata',
    )) + (_soac_ext.strict_function_entry_kind(function),)
    if __dp_integration_strict__:
        # Checked source entries do not authorize legacy unchecked targets.
        assert actual[0] == 0, (path, actual)
        assert (actual[1] == 0) == (path in dynamic_source_methods), (path, actual)
        assert actual[2] and actual[3], (path, actual)
        expected_entry = 'entry_interpreter' if __dp_integration_entry__ else 'checked_native'
        assert actual[4] == expected_entry, (path, actual)
    else:
        assert actual == (0, 0, None, None, None), (path, actual)
    return (function, function.__code__, function.__defaults__, function.__kwdefaults__, actual)

def type_snapshot(name):
    cls = vars(module)[name]
    assert type(cls) is type, (name, type(cls))
    owner = apis['PyType_GetSoacContractOwner'](cls)
    sealed = apis['PyType_IsSoacSealed'](cls)
    if __dp_integration_strict__ and name not in dynamic_classes:
        assert owner and sealed == 1, (name, owner, sealed)
    else:
        assert owner is None and sealed == 0, (name, owner, sealed)
    return (cls, owner, sealed, cls.__bases__, tuple(vars(cls).items()))

saved_functions = {path: function_snapshot(path) for path in source_functions}
saved_types = {name: type_snapshot(name) for name in observed_classes}

def assert_bindings():
    if not __dp_integration_strict__:
        # Ordinary class/global mutation is permitted. Observe the current
        # objects' lack of authority without imposing strict identity/finality.
        for path in source_functions:
            function_snapshot(path)
        for name in observed_classes:
            type_snapshot(name)
        assert _soac_ext.strict_module_diagnostics(module) is None
        return
    for path, saved in saved_functions.items():
        actual = function_snapshot(path)
        assert all(actual[index] is saved[index] for index in range(4)), path
        assert actual[4] == saved[4], (path, actual[4], saved[4])
    for name, saved in saved_types.items():
        actual = type_snapshot(name)
        assert actual[0] is saved[0] and actual[3] is saved[3], name
        assert actual[1:3] == saved[1:3], (name, actual[1:3], saved[1:3])
        namespace = vars(actual[0])
        assert len(namespace) == len(saved[4]), (name, tuple(namespace))
        assert all(namespace[key] is value for key, value in saved[4]), name

sealed_rejections = []

@contextmanager
def reject_sealed_mutation(name):
    assert __dp_integration_strict__
    assert_bindings()
    with pytest.raises(StrictMutationError) as caught:
        yield
    assert type(caught.value) is StrictMutationError, (name, caught.value)
    assert_bindings()
    sealed_rejections.append(name)

if not __dp_integration_strict__:
    assert _soac_ext.strict_module_diagnostics(module) is None
"""

_SOURCE_FUNCTIONS = (
    "Box.__init__", "Box.read_self", "ObservedBox.__getattribute__",
    "ObservedBox.__setattr__", "Consumer.consume", "AmbiguousLeft.__init__",
    "AmbiguousRight.__init__", "SlotRecord.__init__", "InheritedBase.__init__",
    "InheritedBase.inherited_read", "InheritedRight.__init__",
    "WatchedPayload.__init__", "WatchedPayload.__del__", "read_other",
    "write_other", "read_compound", "read_many", "cold_read", "read_ambiguous",
    "read_unanchored", "read_generated", "read_slot",
)


def test_late_owner_nonself_fields_preserve_ordinary_storage_and_callbacks(
    tmp_path: Path,
) -> None:
    module_name = "late_owner_nonself_fields_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            from dataclasses import dataclass


            EVENTS = []


            class OriginalBase:
                pass


            class ReplacementBase:
                @property
                def payload(self):
                    EVENTS.append("replacement-base:get")
                    return ["replacement-base"]


            class Box(OriginalBase):
                def __init__(self, payload):
                    self.marker = "box"
                    self.payload = payload
                    self.cold = ["cold"]

                def read_self(self):
                    return self.payload


            class ObservedBox(Box):
                def __getattribute__(self, name):
                    if name == "payload":
                        EVENTS.append("subclass:get")
                    return object.__getattribute__(self, name)

                def __setattr__(self, name, value):
                    if name == "payload":
                        EVENTS.append(("subclass:set", value))
                    object.__setattr__(self, name, value)


            class Consumer:
                def consume(self, box):
                    return box.payload


            class AmbiguousLeft:
                def __init__(self, value):
                    self.shared = value


            class AmbiguousRight:
                def __init__(self, value):
                    self.padding = "right"
                    self.shared = value


            class Unanchored:
                pass


            @dataclass
            class GeneratedRecord:
                generated_payload: object


            class SlotRecord:
                __slots__ = ("slot_payload",)

                def __init__(self, value):
                    self.slot_payload = value


            class InheritedBase:
                def __init__(self, value):
                    self.lineage = value

                def inherited_read(self):
                    return self.lineage


            class InheritedLeft(InheritedBase):
                pass


            class InheritedRight(InheritedBase):
                def __init__(self, value):
                    self.padding = "right"
                    InheritedBase.__init__(self, value)


            class WatchedPayload:
                def __init__(self, owner):
                    self.owner = owner

                def __del__(self):
                    EVENTS.append(("destroyed", read_other(self.owner)))


            def read_other(box):
                return box.payload


            def write_other(box, value):
                box.payload = value


            def read_compound(boxes):
                return boxes[0].payload


            def read_many(boxes):
                return [box.payload for box in boxes]


            def cold_read(box):
                return box.cold


            def read_ambiguous(item):
                return item.shared


            def read_unanchored(item):
                return item.unanchored


            def read_generated(item):
                return item.generated_payload


            def read_slot(item):
                return item.slot_payload
            """
        ),
        encoding="utf-8",
    )

    script = textwrap.dedent(
        f"""
        import os
        import sys

        sys.path.insert(0, {str(tmp_path)!r})
        from soac.import_hook import install
        install()
        import {module_name} as module

        consumer = module.Consumer()
        for index in range(32):
            initial = ["initial", index]
            replacement = ["replacement", index]
            box = module.Box(initial)
            module.Box.__init__(box, initial)

            assert module.read_other(box) is initial
            assert consumer.consume(box) is initial
            assert module.read_compound([box]) is initial
            assert module.read_many([box, box]) == [initial, initial]
            assert module.write_other(box, replacement) is None
            assert module.read_other(box) is replacement
            assert box.read_self() is replacement

            left = module.AmbiguousLeft(["left", index])
            right = module.AmbiguousRight(["right", index])
            assert module.read_ambiguous(left) == ["left", index]
            assert module.read_ambiguous(right) == ["right", index]

            unanchored = module.Unanchored()
            unanchored.unanchored = ["unanchored", index]
            assert module.read_unanchored(unanchored) == ["unanchored", index]

            generated = module.GeneratedRecord(["generated", index])
            assert module.read_generated(generated) == ["generated", index]

            slot = module.SlotRecord(["slot", index])
            assert module.read_slot(slot) == ["slot", index]

            assert module.InheritedLeft(["left", index]).inherited_read() == [
                "left", index
            ]
            assert module.InheritedRight(["right", index]).inherited_read() == [
                "right", index
            ]

            if index == 0:
                assert module.cold_read(box) == ["cold"]

        if os.environ.get("SOAC_OPT_MODE") != "profile":
            observed = module.ObservedBox(["observed"])
            module.EVENTS.clear()
            assert module.read_other(observed) == ["observed"]
            assert module.EVENTS == ["subclass:get"], module.EVENTS

            observed_replacement = ["subclass-replacement"]
            module.EVENTS.clear()
            assert module.write_other(observed, observed_replacement) is None
            assert module.EVENTS == [
                ("subclass:set", observed_replacement)
            ], module.EVENTS

            class ObservedBoxes(list):
                def __getitem__(self, index):
                    module.EVENTS.append(("receiver:getitem", index))
                    return list.__getitem__(self, index)

            compound = module.Box(["compound"])
            module.EVENTS.clear()
            assert module.read_compound(ObservedBoxes([compound])) == [
                "compound"
            ]
            assert module.EVENTS == [
                ("receiver:getitem", 0)
            ], module.EVENTS

            watched_owner = module.Box(["original"])
            old_value = module.WatchedPayload(watched_owner)
            assert module.write_other(watched_owner, old_value) is None
            del old_value
            replacement = ["visible-during-finalizer"]
            module.EVENTS.clear()
            assert module.write_other(watched_owner, replacement) is None
            assert module.EVENTS == [
                ("destroyed", replacement)
            ], module.EVENTS

            materialized = module.Box(["materialized"])
            instance_dict = materialized.__dict__
            instance_dict["payload"] = ["updated"]
            assert module.read_other(materialized) == ["updated"]
            instance_dict[1] = "promoted"
            promoted = ["promoted"]
            assert module.write_other(materialized, promoted) is None
            assert instance_dict["payload"] is promoted
            del instance_dict["payload"]
            try:
                module.read_other(materialized)
            except AttributeError as error:
                assert "payload" in str(error), error
            else:
                raise AssertionError("a deleted promoted field must raise")
            reinserted = ["reinserted"]
            assert module.write_other(materialized, reinserted) is None
            assert module.read_other(materialized) is reinserted

            growing = module.Box(["growing"])
            for index in range(40):
                setattr(growing, "extra_" + str(index), index)
            assert module.read_other(growing) == ["growing"]
            grown = ["grown"]
            assert module.write_other(growing, grown) is None
            assert module.read_other(growing) is grown

            unseeded = object.__new__(module.Box)
            first = ["first-insertion"]
            assert module.write_other(unseeded, first) is None
            assert module.read_other(unseeded) is first

            box = module.Box(["instance-payload"])

            def descriptor_get(instance):
                module.EVENTS.append("descriptor:get")
                return ["descriptor-value"]

            def descriptor_set(instance, value):
                module.EVENTS.append(("descriptor:set", value))

            module.Box.payload = property(descriptor_get, descriptor_set)
            try:
                module.EVENTS.clear()
                assert module.read_other(box) == ["descriptor-value"]
                assert module.EVENTS == ["descriptor:get"], module.EVENTS

                assigned = ["descriptor-assigned"]
                module.EVENTS.clear()
                assert module.write_other(box, assigned) is None
                assert module.EVENTS == [
                    ("descriptor:set", assigned)
                ], module.EVENTS
            finally:
                del module.Box.payload
            assert module.read_other(box) == ["instance-payload"]

            module.Box.payload = ["class-payload"]
            try:
                assert module.read_other(box) == ["instance-payload"]
            finally:
                del module.Box.payload

            original_bases = module.Box.__bases__
            module.Box.__bases__ = (module.ReplacementBase,)
            try:
                module.EVENTS.clear()
                assert module.read_other(box) == ["replacement-base"]
                assert module.EVENTS == [
                    "replacement-base:get"
                ], module.EVENTS
            finally:
                module.Box.__bases__ = original_bases
            assert module.read_other(box) == ["instance-payload"]

            original_box = module.Box

            class ReplacementBox:
                def __init__(self, payload):
                    self.payload = payload

            module.Box = ReplacementBox
            try:
                assert module.read_other(module.Box(["new-owner"])) == [
                    "new-owner"
                ]
            finally:
                module.Box = original_box

            slot = module.SlotRecord(["delete-slot"])
            del slot.slot_payload
            try:
                module.read_slot(slot)
            except AttributeError as error:
                assert "slot_payload" in str(error), error
            else:
                raise AssertionError("a deleted object slot must raise")
        """
    )

    # Keep both original literals above unchanged. Only the obsolete import-hook
    # bootstrap is excluded from the validator, which runs outside either module.
    import_line = f"import {module_name} as module\n"
    assert script.count(import_line) == 1
    ordinary_body = script.split(import_line, 1)[1]
    witness_setup = (
        f"source_functions = {_SOURCE_FUNCTIONS!r}\n"
        "dynamic_source_methods = ('ObservedBox.__getattribute__', 'ObservedBox.__setattr__')\n"
        "dynamic_classes = ('ObservedBox',)\n"
        "observed_classes = ('OriginalBase', 'ReplacementBase', 'Box', 'ObservedBox', "
        "'Consumer', 'AmbiguousLeft', 'AmbiguousRight', 'Unanchored', 'GeneratedRecord', "
        "'SlotRecord', 'InheritedBase', 'InheritedLeft', 'InheritedRight', 'WatchedPayload')\n"
        + _OWNER_WITNESSES
    )
    ordinary_validation = "def validate_module(module):\n" + textwrap.indent(
        witness_setup + ordinary_body + "\nassert_bindings()\n", "    "
    )
    relative = f"{module_name}.py"
    original_source = (tmp_path / relative).read_text(encoding="utf-8")
    with pytest.MonkeyPatch.context() as patch:
        patch.setenv("SOAC_OPT_MODE", "none")
        with stock_module(tmp_path / "ordinary", module_name, original_source) as stock:
            exec_integration_validation(
                ordinary_validation, stock, Path(__file__), mode="stock"
            )

    # Strict class/global bindings cannot be replaced. Preserve the successful
    # original operations in the ordinary arm; selected callers still exercise
    # descriptor and replacement-owner interoperability below.
    mutation_start = '    module.Box.payload = property(descriptor_get, descriptor_set)\n'
    mutation_end = '    slot = module.SlotRecord(["delete-slot"])\n'
    assert ordinary_body.count(mutation_start) == ordinary_body.count(mutation_end) == 1
    before_mutations, mutations = ordinary_body.split(mutation_start, 1)
    _, after_mutations = mutations.split(mutation_end, 1)
    selected_mutations = textwrap.indent(textwrap.dedent("""
        with reject_sealed_mutation('field_descriptor'):
            module.Box.payload = property(descriptor_get, descriptor_set)
        module.EVENTS.clear()
        assert module.read_other(box) == ["instance-payload"]
        assert module.EVENTS == [], module.EVENTS

        class DescriptorBox:
            payload = property(descriptor_get, descriptor_set)

        ordinary_descriptor = DescriptorBox()
        assert module.read_other(ordinary_descriptor) == ["descriptor-value"]
        assigned = ["descriptor-assigned"]
        assert module.write_other(ordinary_descriptor, assigned) is None
        assert module.EVENTS == ["descriptor:get", ("descriptor:set", assigned)], module.EVENTS

        with reject_sealed_mutation('field_class_binding'):
            module.Box.payload = ["class-payload"]
        assert module.read_other(box) == ["instance-payload"]

        class ShadowingBox:
            payload = ["class-payload"]

        shadowing = ShadowingBox()
        shadowing.payload = ["instance-payload"]
        assert module.read_other(shadowing) is shadowing.__dict__["payload"]

        original_bases = module.Box.__bases__
        with reject_sealed_mutation('class_bases'):
            module.Box.__bases__ = (module.ReplacementBase,)
        assert module.Box.__bases__ is original_bases
        module.EVENTS.clear()
        assert module.read_other(box) == ["instance-payload"]
        assert module.EVENTS == [], module.EVENTS
        assert module.read_other(module.ReplacementBase()) == ["replacement-base"]
        assert module.EVENTS == ["replacement-base:get"], module.EVENTS

        original_box = module.Box

        class ReplacementBox:
            def __init__(self, payload):
                self.payload = payload

        with reject_sealed_mutation('module_class_binding'):
            module.Box = ReplacementBox
        assert module.Box is original_box
        assert module.read_other(ReplacementBox(["new-owner"])) == ["new-owner"]
        assert module.read_other(module.Box(["original-owner"])) == ["original-owner"]
    """).lstrip("\n"), "    ")
    selected_body = before_mutations + selected_mutations + mutation_end + after_mutations

    def replace_once(old: str, new: str) -> None:
        nonlocal selected_body
        assert selected_body.count(old) == 1, old
        selected_body = selected_body.replace(old, new)

    replace_once(
        "    del old_value\n",
        "    watched_reference = weakref.ref(old_value)\n    del old_value\n",
    )
    replace_once(
        "    assert module.write_other(watched_owner, replacement) is None\n",
        "    assert module.write_other(watched_owner, replacement) is None\n"
        "    gc.collect()\n    assert watched_reference() is None\n",
    )
    replace_once(
        "    instance_dict = materialized.__dict__\n",
        "    instance_dict = materialized.__dict__\n"
        "    assert type(instance_dict) is dict\n"
        "    assert _testinternalcapi.dict_has_indexed_keys(instance_dict) is False\n",
    )
    replace_once(
        '    instance_dict[1] = "promoted"\n',
        '    instance_dict[1] = "promoted"\n'
        "    assert _testinternalcapi.dict_has_indexed_keys(instance_dict) is False\n",
    )
    selected_body += """
if os.environ.get('SOAC_OPT_MODE') != 'profile':
    assert sealed_rejections == [
        'field_descriptor', 'field_class_binding', 'class_bases', 'module_class_binding',
    ], sealed_rejections
assert_bindings()
"""
    validation = "def validate_module(module):\n" + textwrap.indent(
        witness_setup + selected_body, "    "
    )
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in(original_source.encode(), relative)[0].decode()},
        modules={module_name: relative},
    )
    case = StrictValidationCase(validation, Path(__file__), required_functions=_SOURCE_FUNCTIONS)
    work_dir = tmp_path / "soac-work"

    def run_mode(mode: str, *, entry_interpreter: bool = False) -> None:
        program = _VALIDATION_PRELUDE + project._validation_program(
            module_name, case, entry_interpreter=entry_interpreter
        )
        result = project.run(
            program, opt_mode=mode, entry_interpreter=entry_interpreter,
            extra_env={"SOAC_WORK_DIR": str(work_dir)}, timeout=90, check=False,
        )
        assert result.returncode == 0, (
            f"{mode} subprocess failed:\n{result.stdout}{result.stderr}"
        )

    run_mode("profile")

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

    def field_rows(records: list[dict]) -> dict[tuple, dict]:
        selected: dict[tuple, dict] = {}
        for record in records:
            for row in record["rows"]:
                if row["kind"] != "field_access":
                    continue
                key = (
                    row["function_id"],
                    row["instr_id"],
                    row["counter_id"],
                )
                previous = selected.get(key)
                if previous is None or row["value"] >= previous["value"]:
                    selected[key] = row
        return selected

    def branch_rows(
        records: list[dict], qualname: str, *branches: str
    ) -> list[tuple[str, int]]:
        # The read_many element belongs to this source comprehension whether
        # lowered in its parent or in a helper. Keep each actual counter separate.
        return sorted(
            (
                row["instr_id"],
                sum(row.get("branches", {}).get(branch, 0) for branch in branches),
            )
            for row in field_rows(records).values()
            if row["function_qualname"] == qualname
            or (
                qualname == "read_many"
                and row["function_qualname"].startswith(
                    "read_many.<locals>._dp_listcomp_"
                )
            )
        )

    positive_reads = {
        "read_other": 64,
        "read_ambiguous": 64,
        "Consumer.consume": 32,
        "read_compound": 32,
        "read_many": 64,
    }
    positive_stores = {"write_other": 32}
    for qualname, count in positive_reads.items():
        rows = branch_rows(profile_records, qualname, "generic_getattr")
        assert any(value >= count for _, value in rows), (
            "Profile must expose the genuine non-self object field source",
            qualname,
            count,
            rows,
        )
    for qualname, count in positive_stores.items():
        rows = branch_rows(profile_records, qualname, "generic_setattr")
        assert any(value >= count for _, value in rows), (
            "Profile must expose the genuine non-self object store source",
            qualname,
            count,
            rows,
        )

    constructor_stores = branch_rows(
        profile_records, "Box.__init__", "generic_setattr"
    )
    assert len([row for row in constructor_stores if row[1] >= 32]) >= 3, (
        "the original Box marker/payload/cold stores must be observed",
        constructor_stores,
    )

    type_names = {
        entry["type_id"]: entry["qualname"]
        for record in profile_dump["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
    }
    type_keys = {
        (type_names[key["owner_type_id"]], key["key"], key["index"])
        for record in profile_dump["records"]
        for key in record["type_keys"]
        if key["owner_type_id"] in type_names
    }
    payload_owners = {
        (owner, index) for owner, key, index in type_keys if key == "payload"
    }
    assert payload_owners == {("Box", 2)}, (
        "the positive payload must have exactly one profiled owner/index",
        payload_owners,
        type_keys,
    )
    shared_owners = {
        (owner, index) for owner, key, index in type_keys if key == "shared"
    }
    assert shared_owners == {
        ("AmbiguousLeft", 0),
        ("AmbiguousRight", 1),
    }, shared_owners
    assert ("GeneratedRecord", "generated_payload", 0) in type_keys, type_keys
    assert ("Unanchored", "unanchored", 0) in type_keys, type_keys

    expected_generic_controls = {
        "read_generated": 32,
        "read_unanchored": 32,
        "read_slot": 32,
        "cold_read": 1,
    }
    for qualname, count in expected_generic_controls.items():
        rows = branch_rows(profile_records, qualname, "generic_getattr")
        assert any(value >= count for _, value in rows), (
            "Profile must expose every cold/unanchored/generated/slot control",
            qualname,
            count,
            rows,
        )

    run_mode("verify")
    verify_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "verify.bin"))
    )
    verify_records = [
        record
        for record in verify_dump["records"]
        if record["module_name"] == module_name
    ]
    assert verify_records, verify_dump

    run_mode("apply")

    for qualname in expected_generic_controls:
        hits = branch_rows(verify_records, qualname, "indexed_hit")
        assert all(count == 0 for _, count in hits), (
            "cold, missing-anchor, generated, and slot sites must "
            "retain the original generic access",
            qualname,
            hits,
        )

    dictionary_reads = {
        **positive_reads,
        "Box.read_self": 32,
        "InheritedBase.inherited_read": 64,
    }
    dictionary_stores = {**positive_stores, "Box.__init__": 32}
    indexed_hits = {
        qualname: branch_rows(verify_records, qualname, "indexed_hit")
        for qualname in dictionary_reads | dictionary_stores
    }
    # Source/profile ownership and shared-key observations do not install an
    # indexed layout or late-owner cell. The actual ordinary dictionaries are
    # witnessed above; no direct access may bypass their checked fallback.
    assert all(
        count == 0 for rows in indexed_hits.values() for _, count in rows
    ), ("ordinary checked dictionaries acquired an unchecked field hit", indexed_hits)
    for qualname, count in dictionary_reads.items():
        # An unselected site uses generic lookup; a selected indexed proposal
        # may instead reach its checked fallback. Count either on this exact
        # source/counter row without requiring optional plan eligibility.
        rows = branch_rows(
            verify_records, qualname, "generic_getattr", "indexed_fallback"
        )
        assert any(value >= count for _, value in rows), (qualname, count, rows)
    for qualname, count in dictionary_stores.items():
        rows = branch_rows(verify_records, qualname, "generic_setattr")
        required_sites = 3 if qualname == "Box.__init__" else 1
        assert sum(value >= count for _, value in rows) >= required_sites, (
            qualname, count, required_sites, rows,
        )

    # The same semantic and permanent-contract checks also cover the retained
    # entry interpreter, without requiring optional optimization eligibility.
    run_mode("none", entry_interpreter=True)


def test_profiled_two_field_compare_releases_left_value_when_right_getter_raises(
    tmp_path: Path,
) -> None:
    module_name = "two_field_comparison_cleanup_case"
    relative = f"{module_name}.py"
    source = textwrap.dedent(
        """
        class Record:
            def __init__(self, value):
                self.value = value


        def compare(left, right):
            if left.value < right.value:
                return 1
            return 0
        """
    )
    source_functions = ("Record.__init__", "compare")
    witness_setup = (
        f"source_functions = {source_functions!r}\n"
        "dynamic_source_methods = ()\n"
        "dynamic_classes = ()\n"
        "observed_classes = ('Record',)\n"
        + _OWNER_WITNESSES
    )
    body = textwrap.dedent(
        """
        # The retained profile contains only ordinary exact-int operands.
        for value in range(80):
            left = module.Record(value)
            right = module.Record(40)
            assert module.compare(left, right) == int(value < 40)
        del left, right

        if not __dp_integration_strict__ or os.environ.get("SOAC_OPT_MODE") != "profile":
            events = []
            released = []
            marker = RuntimeError("right field failed")

            class Payload:
                def __lt__(self, other):
                    events.append("payload:compare")
                    return False

                def __del__(self):
                    released.append("payload")

            class RaisingRight:
                @property
                def value(self):
                    events.append("rhs:get")
                    raise marker

            payload = Payload()
            payload_reference = weakref.ref(payload)
            left = module.Record(payload)
            right = RaisingRight()
            assert left.value is payload
            del payload

            try:
                module.compare(left, right)
            except RuntimeError as caught:
                assert caught is marker
                events.append("caught")
            else:
                raise AssertionError("the right getter must propagate its exception")
            assert events == ["rhs:get", "caught"], events
            assert left.value is payload_reference()

            # No saved exception/owner may intentionally keep the payload alive.
            # Clear the actual field and ordinary traceback, then test quiescence;
            # the implicit finalizer need not run at any particular instruction.
            left.value = None
            del left, right
            marker.__traceback__ = None
            gc.collect()
            assert payload_reference() is None, "failed RHS leaked the owned LHS value"
            assert released == ["payload"], released

            assert module.compare(module.Record(2), module.Record(3)) == 1
            assert module.compare(module.Record(3), module.Record(2)) == 0

        assert_bindings()
        if __dp_integration_strict__:
            import json
            print(json.dumps({"source_id": saved_functions["compare"][4][1]}))
        """
    )
    validation = "def validate_module(module):\n" + textwrap.indent(
        witness_setup + body, "    "
    )
    with pytest.MonkeyPatch.context() as patch:
        patch.setenv("SOAC_OPT_MODE", "none")
        with stock_module(tmp_path / "ordinary", module_name, source) as ordinary:
            exec_integration_validation(
                validation, ordinary, Path(__file__), mode="stock"
            )

    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in(source.encode(), relative)[0].decode()},
        modules={module_name: relative},
    )
    case = StrictValidationCase(
        validation, Path(__file__), required_functions=source_functions
    )
    work_dir = tmp_path / "soac-work"

    def run_mode(mode: str) -> int:
        program = _VALIDATION_PRELUDE + project._validation_program(
            module_name, case, entry_interpreter=False
        )
        result = project.run(
            program,
            opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(work_dir)},
            timeout=60,
            check=False,
        )
        assert result.returncode == 0, (
            f"{mode} subprocess failed:\n{result.stdout}{result.stderr}"
        )
        return json.loads(result.stdout)["source_id"]

    compare_source_id = run_mode("profile")

    from soac import _soac_ext

    profile_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    field_sites: dict[int, int] = {}
    for record in profile_dump["records"]:
        if record["module_name"] != module_name:
            continue
        for row in record["rows"]:
            if (
                row["kind"] == "field_access"
                and row["function_qualname"] == "compare"
                and row["function_id"] == compare_source_id
            ):
                count = row.get("branches", {}).get("generic_getattr", 0)
                field_sites[row["instr_id"]] = max(
                    field_sites.get(row["instr_id"], 0), count
                )
    assert len(field_sites) == 2, field_sites
    assert all(count >= 80 for count in field_sites.values()), field_sites

    # Exercise real profile consumption without manufacturing a scalar plan or
    # requiring an optional indexed hit. Ownership must hold on every fallback.
    run_mode("verify")
    run_mode("apply")
