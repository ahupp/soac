from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


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
        f"""
        import gc
        import sys
        import weakref

        sys.path.insert(0, {str(tmp_path)!r})
        from soac.import_hook import install
        install()
        import {module_name} as module

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
        assert fresh.__dict__ == {{
            "first": 15,
            "middle": 16,
            "mark": 17,
        }}

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

    profile = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        timeout=60,
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

    verify = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
        timeout=60,
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
    assert {
        name: indexed_hits.get(name, 0)
        for name in ("Point.read", "Point.write", "Record.read", "Record.write")
    } == {
        name: count
        for name, count in indexed_hits.items()
        if name in {"Point.read", "Point.write", "Record.read", "Record.write"}
        and count >= 16
    }, (
        "eagerly compiled slot and split-dict methods must bind their owner after "
        "class creation and use the existing indexed_hit field counter",
        indexed_hits,
    )

    apply = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
        timeout=60,
    )
    assert apply.returncode == 0, apply.stdout + apply.stderr
