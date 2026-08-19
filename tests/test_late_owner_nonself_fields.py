from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def test_hot_nonself_split_fields_reuse_unique_constructor_owner_cells(
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
                __static_attributes__ = ()

                def __init__(self, payload):
                    self.marker = "box"
                    self.payload = payload
                    self.cold = ["cold"]

                def read_self(self):
                    return self.payload


            class ObservedBox(Box):
                __static_attributes__ = ()

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
                __static_attributes__ = ()

                def __init__(self, value):
                    self.lineage = value

                def inherited_read(self):
                    return self.lineage


            class InheritedLeft(InheritedBase):
                __static_attributes__ = ()


            class InheritedRight(InheritedBase):
                __static_attributes__ = ()

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

    work_dir = tmp_path / "soac-work"
    base_env = {
        **os.environ,
        "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
        "SOAC_WORK_DIR": str(work_dir),
        "SOAC_COMPILE_MODE": "eager",
        "SOAC_BACKGROUND_JIT": "0",
    }

    def run_mode(mode: str) -> None:
        result = subprocess.run(
            [sys.executable, "-c", script],
            check=False,
            capture_output=True,
            text=True,
            env={**base_env, "SOAC_OPT_MODE": mode},
            timeout=90,
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
        records: list[dict], qualname: str, branch: str
    ) -> list[tuple[str, int]]:
        return sorted(
            (
                row["instr_id"],
                row.get("branches", {}).get(branch, 0),
            )
            for row in field_rows(records).values()
            if row["function_qualname"] == qualname
        )

    nested_consumers = {
        row["function_qualname"]
        for row in field_rows(profile_records).values()
        if row["function_qualname"].startswith("read_many.<locals>.")
        and row.get("branches", {}).get("generic_getattr", 0) >= 64
    }
    assert len(nested_consumers) == 1, nested_consumers
    nested_consumer = next(iter(nested_consumers))

    positive_reads = {
        "read_other": 64,
        "Consumer.consume": 32,
        "read_compound": 32,
        nested_consumer: 64,
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
        "the unique Box marker/payload/cold constructor cells must exist",
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
    assert payload_owners == {("Box", 1)}, (
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
        "read_ambiguous": 64,
        "read_generated": 32,
        "read_unanchored": 32,
        "read_slot": 32,
        "cold_read": 1,
    }
    for qualname, count in expected_generic_controls.items():
        rows = branch_rows(profile_records, qualname, "generic_getattr")
        assert any(value >= count for _, value in rows), (
            "Profile must expose every ambiguous/cold/unanchored/slot control",
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

    existing_hits = {
        qualname: branch_rows(verify_records, qualname, "indexed_hit")
        for qualname in (
            "Box.__init__",
            "Box.read_self",
            "InheritedBase.inherited_read",
        )
    }
    assert any(count >= 31 for _, count in existing_hits["Box.__init__"]), (
        "the preexisting constructor anchor must remain live",
        existing_hits,
    )
    assert any(count >= 32 for _, count in existing_hits["Box.read_self"]), (
        "the existing lexical self-field specialization must remain live",
        existing_hits,
    )
    assert any(
        count >= 64 for _, count in existing_hits["InheritedBase.inherited_read"]
    ), (
        "the existing unequal-index inherited-owner specialization must remain live",
        existing_hits,
    )

    for qualname in expected_generic_controls:
        hits = branch_rows(verify_records, qualname, "indexed_hit")
        assert all(count == 0 for _, count in hits), (
            "ambiguous, cold, missing-anchor, generated, and slot sites must "
            "retain the original generic access",
            qualname,
            hits,
        )

    required_hits = {**positive_reads, **positive_stores}
    source_hits = {
        qualname: branch_rows(verify_records, qualname, "indexed_hit")
        for qualname in required_hits
    }
    assert all(
        any(hits >= expected for _, hits in source_hits[qualname])
        for qualname, expected in required_hits.items()
    ), (
        "hot non-self object reads/stores, unrelated-method arguments, "
        "compound receivers, and nested eager comprehension fields must "
        "reuse the unique existing Box.payload constructor cell with "
        "original-source indexed-hit counters",
        {
            "expected_hits": required_hits,
            "actual_hits_by_source": source_hits,
            "existing_specialization_hits": existing_hits,
            "profile_payload_owners": payload_owners,
        },
    )
