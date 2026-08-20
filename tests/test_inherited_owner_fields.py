from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def test_inherited_split_fields_use_exact_profiled_concrete_owner_guards(
    tmp_path: Path,
) -> None:
    module_name = "inherited_owner_fields_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            EVENTS = []


            class DeltaBase:
                def __init__(self, value):
                    self.direction = True
                    self.value = value

                def read(self):
                    return self.value

                def direction_value(self):
                    if self.direction:
                        return self.value
                    return -self.value

                def write(self, value):
                    self.value = value


            class DeltaLeft(DeltaBase):
                def __init__(self, value):
                    DeltaBase.__init__(self, value)


            class DeltaRight(DeltaBase):
                def __init__(self, value):
                    self.padding = "first"
                    DeltaBase.__init__(self, value)


            class StateBase:
                def __init__(self):
                    self.packet_pending = False
                    self.task_waiting = True
                    self.task_holding = False

                def is_holding_or_waiting(self):
                    return self.task_holding or (
                        not self.packet_pending and self.task_waiting
                    )

                def is_waiting_with_packet(self):
                    return (
                        self.packet_pending
                        and self.task_waiting
                        and not self.task_holding
                    )

                def set_pending(self, value):
                    self.packet_pending = value


            class StateRoot(StateBase):
                def __init__(self, marker):
                    self.marker = marker
                    self.ident = marker
                    self.priority = marker
                    self.input = None
                    StateBase.__init__(self)


            class StateAlpha(StateRoot):
                def __init__(self, marker):
                    StateRoot.__init__(self, marker)


            class StateBeta(StateRoot):
                def __init__(self, marker):
                    StateRoot.__init__(self, marker)


            class StateGamma(StateRoot):
                def __init__(self, marker):
                    StateRoot.__init__(self, marker)


            class StateDelta(StateRoot):
                def __init__(self, marker):
                    StateRoot.__init__(self, marker)


            class ObservedLeft(DeltaLeft):
                def __getattribute__(self, name):
                    if name == "value":
                        EVENTS.append("observed:get")
                    return object.__getattribute__(self, name)

                def __setattr__(self, name, value):
                    if name == "value":
                        EVENTS.append(("observed:set", value))
                    return object.__setattr__(self, name, value)


            class ReplacementDeltaBase:
                def read(self):
                    EVENTS.append("replacement:read")
                    return self.value + 900


            class SlottedBase:
                __slots__ = ("value",)

                def __init__(self, value):
                    self.value = value

                def read(self):
                    return self.value


            class SlottedChild(SlottedBase):
                __slots__ = ()


            class WatchedValue:
                __slots__ = ("owner",)

                def __init__(self, owner):
                    self.owner = owner

                def __del__(self):
                    EVENTS.append(("destroyed", self.owner.read()))
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

        delta_owners = (module.DeltaLeft, module.DeltaRight)
        state_owners = (
            module.StateAlpha,
            module.StateBeta,
            module.StateGamma,
            module.StateDelta,
        )
        for value in range(32):
            for owner in delta_owners:
                instance = owner(value)
                assert instance.read() == value
                assert instance.direction_value() == value
                assert instance.write(value + 100) is None
                assert object.__getattribute__(instance, "value") == value + 100

            for owner in state_owners:
                state = owner(value)
                assert state.is_holding_or_waiting() is True
                assert state.set_pending(True) is None
                assert state.is_waiting_with_packet() is True

            exact_base = module.StateBase()
            assert exact_base.is_holding_or_waiting() is True
            assert exact_base.set_pending(True) is None
            assert exact_base.is_waiting_with_packet() is True

        if os.environ.get("SOAC_OPT_MODE") != "profile":
            observed = module.ObservedLeft(7)
            module.EVENTS.clear()
            assert observed.read() == 7
            assert module.EVENTS == ["observed:get"], module.EVENTS

            module.EVENTS.clear()
            assert observed.write(8) is None
            assert module.EVENTS == [("observed:set", 8)], module.EVENTS

            module.EVENTS.clear()
            assert observed.direction_value() == 8
            assert module.EVENTS == ["observed:get"], module.EVENTS

            materialized = module.DeltaRight(11)
            instance_dict = materialized.__dict__
            assert instance_dict == {{
                "padding": "first",
                "direction": True,
                "value": 11,
            }}, instance_dict
            instance_dict["value"] = 12
            assert materialized.read() == 12
            instance_dict[1] = "promoted"
            assert materialized.write(13) is None
            assert instance_dict["value"] == 13
            del instance_dict["value"]
            try:
                materialized.read()
            except AttributeError as error:
                assert "value" in str(error), error
            else:
                raise AssertionError("a deleted promoted field must raise")
            instance_dict["value"] = 14
            assert materialized.read() == 14

            growing = module.DeltaLeft(21)
            for index in range(40):
                setattr(growing, "extra_" + str(index), index)
            assert growing.read() == 21
            assert growing.write(22) is None
            assert growing.read() == 22
            del growing.value
            try:
                growing.read()
            except AttributeError as error:
                assert "value" in str(error), error
            else:
                raise AssertionError("a deleted inherited field must raise")
            assert growing.write(23) is None
            assert growing.read() == 23

            watched_owner = module.DeltaLeft(0)
            old_value = module.WatchedValue(watched_owner)
            assert watched_owner.write(old_value) is None
            del old_value
            module.EVENTS.clear()
            assert watched_owner.write("replacement") is None
            assert module.EVENTS == [
                ("destroyed", "replacement")
            ], module.EVENTS

            left = module.DeltaLeft(31)

            def leaf_get(instance):
                module.EVENTS.append("leaf-property:get")
                return 701

            def leaf_set(instance, value):
                module.EVENTS.append(("leaf-property:set", value))

            module.DeltaLeft.value = property(leaf_get, leaf_set)
            try:
                module.EVENTS.clear()
                assert left.read() == 701
                assert module.EVENTS == ["leaf-property:get"], module.EVENTS

                module.EVENTS.clear()
                assert left.write(33) is None
                assert module.EVENTS == [
                    ("leaf-property:set", 33)
                ], module.EVENTS
            finally:
                del module.DeltaLeft.value
            assert left.read() == 31

            def base_get(instance):
                module.EVENTS.append("base-property:get")
                return 811

            def base_set(instance, value):
                module.EVENTS.append(("base-property:set", value))

            module.DeltaBase.value = property(base_get, base_set)
            try:
                module.EVENTS.clear()
                assert left.read() == 811
                assert module.EVENTS == ["base-property:get"], module.EVENTS

                module.EVENTS.clear()
                assert left.write(34) is None
                assert module.EVENTS == [
                    ("base-property:set", 34)
                ], module.EVENTS
            finally:
                del module.DeltaBase.value
            assert left.read() == 31

            module.DeltaLeft.value = "non-data-class-value"
            try:
                assert left.read() == 31
            finally:
                del module.DeltaLeft.value

            changed_mro = module.DeltaRight(41)
            original_bases = module.DeltaRight.__bases__
            module.DeltaRight.__bases__ = (module.ReplacementDeltaBase,)
            try:
                module.EVENTS.clear()
                assert changed_mro.read() == 941
                assert module.EVENTS == ["replacement:read"], module.EVENTS
                assert module.DeltaBase.read(changed_mro) == 41
            finally:
                module.DeltaRight.__bases__ = original_bases
            assert changed_mro.read() == 41

            original_read = module.DeltaBase.read

            def rebound_read(instance):
                module.EVENTS.append("base:rebound")
                return object.__getattribute__(instance, "value") + 500

            module.DeltaBase.read = rebound_read
            try:
                module.EVENTS.clear()
                assert left.read() == 531
                assert module.EVENTS == ["base:rebound"], module.EVENTS
            finally:
                module.DeltaBase.read = original_read
            assert left.read() == 31

            state = module.StateBeta(1)

            def pending_get(instance):
                module.EVENTS.append("state-property:get")
                return True

            def pending_set(instance, value):
                module.EVENTS.append(("state-property:set", value))

            module.StateBeta.packet_pending = property(
                pending_get, pending_set
            )
            try:
                module.EVENTS.clear()
                assert state.is_waiting_with_packet() is True
                assert module.EVENTS == ["state-property:get"], module.EVENTS

                module.EVENTS.clear()
                assert state.set_pending(False) is None
                assert module.EVENTS == [
                    ("state-property:set", False)
                ], module.EVENTS
            finally:
                del module.StateBeta.packet_pending
            assert state.is_waiting_with_packet() is False

            original_left = module.DeltaLeft

            class ReplacementLeft:
                def __init__(self, value):
                    self.value = value

            module.DeltaLeft = ReplacementLeft
            try:
                assert module.DeltaBase.read(module.DeltaLeft(52)) == 52
            finally:
                module.DeltaLeft = original_left

            slot = module.SlottedChild(61)
            assert slot.read() == 61
            del slot.value
            try:
                slot.read()
            except AttributeError as error:
                assert "value" in str(error), error
            else:
                raise AssertionError("an inherited deleted slot must raise")
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
        rows: dict[tuple, dict] = {}
        for record in records:
            for row in record["rows"]:
                if row["kind"] != "field_access":
                    continue
                key = (
                    row["function_id"],
                    row["instr_id"],
                    row["counter_id"],
                )
                previous = rows.get(key)
                if previous is None or row["value"] >= previous["value"]:
                    rows[key] = row
        return rows

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

    expected_get_sites = {
        "DeltaBase.read": (1, 64),
        "DeltaBase.direction_value": (2, 64),
        "StateBase.is_holding_or_waiting": (3, 160),
        "StateBase.is_waiting_with_packet": (3, 160),
    }
    expected_set_sites = {
        "DeltaBase.write": (1, 64),
        "StateBase.set_pending": (1, 160),
    }

    for qualname, (site_count, calls) in expected_get_sites.items():
        rows = branch_rows(profile_records, qualname, "generic_getattr")
        hot_rows = [(source, count) for source, count in rows if count >= calls]
        assert len(hot_rows) >= site_count, (
            "Profile must expose each genuine inherited-base read source",
            qualname,
            {"required_sites": site_count, "calls_per_site": calls},
            rows,
        )

    for qualname, (site_count, calls) in expected_set_sites.items():
        rows = branch_rows(profile_records, qualname, "generic_setattr")
        hot_rows = [(source, count) for source, count in rows if count >= calls]
        assert len(hot_rows) >= site_count, (
            "Profile must expose each genuine inherited-base write source",
            qualname,
            {"required_sites": site_count, "calls_per_site": calls},
            rows,
        )

    concrete_owners = {
        "DeltaLeft",
        "DeltaRight",
        "StateAlpha",
        "StateBeta",
        "StateGamma",
        "StateDelta",
    }
    profiled_owners = concrete_owners | {"StateBase"}
    owner_type_ids = {
        entry["qualname"]: entry["type_id"]
        for record in profile_dump["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
        and entry["qualname"] in profiled_owners
    }
    assert set(owner_type_ids) == profiled_owners, owner_type_ids

    abstract_owner_entries = {
        entry["qualname"]
        for record in profile_dump["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
        and entry["qualname"] in {"DeltaBase", "StateRoot"}
    }
    assert not abstract_owner_entries, (
        "the lexical base owners must have no misleading exact-owner "
        "type_keys: only the actual concrete receivers are profiled",
        abstract_owner_entries,
    )

    owner_indexes: dict[str, dict[str, int]] = {
        owner: {} for owner in profiled_owners
    }
    for record in profile_dump["records"]:
        for key in record["type_keys"]:
            for owner, type_id in owner_type_ids.items():
                if key["owner_type_id"] == type_id:
                    owner_indexes[owner][key["key"]] = key["index"]

    left = owner_indexes["DeltaLeft"]
    right = owner_indexes["DeltaRight"]
    assert left["direction"] != right["direction"], owner_indexes
    assert left["value"] != right["value"], owner_indexes
    assert right["padding"] < right["direction"], owner_indexes

    exact_state_layout = tuple(
        owner_indexes["StateBase"][name]
        for name in ("packet_pending", "task_waiting", "task_holding")
    )
    assert exact_state_layout == (0, 2, 1), owner_indexes

    state_layouts = {
        owner: tuple(
            owner_indexes[owner][name]
            for name in (
                "marker",
                "packet_pending",
                "task_waiting",
                "task_holding",
            )
        )
        for owner in ("StateAlpha", "StateBeta", "StateGamma", "StateDelta")
    }
    assert len(set(state_layouts.values())) == 1, state_layouts
    assert all(layout == (0, 4, 5, 6) for layout in state_layouts.values()), (
        "all four concrete task subclasses must shift the inherited "
        "exact-base indices (0, 1, 2) to (4, 5, 6)",
        {"exact_base": exact_state_layout, "concrete_owners": state_layouts},
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

    expected_sites = {**expected_get_sites, **expected_set_sites}
    indexed_hits = {
        qualname: branch_rows(verify_records, qualname, "indexed_hit")
        for qualname in expected_sites
    }
    assert all(
        len([(source, hits) for source, hits in indexed_hits[qualname] if hits >= calls])
        >= site_count
        for qualname, (site_count, calls) in expected_sites.items()
    ), (
        "each inherited-base field source must dispatch through exact "
        "profiled concrete-owner guards: both unequal-index subclasses and "
        "all four equal-index subclasses must contribute source-attributed "
        "indexed hits",
        {
            "indexed_hits_by_base_source": indexed_hits,
            "required_sites_and_calls": expected_sites,
            "profiled_concrete_layouts": owner_indexes,
            "fallbacks_by_base_source": {
                qualname: branch_rows(
                    verify_records, qualname, "indexed_fallback"
                )
                for qualname in expected_sites
            },
            "generic_gets_by_base_source": {
                qualname: branch_rows(
                    verify_records, qualname, "generic_getattr"
                )
                for qualname in expected_get_sites
            },
            "generic_sets_by_base_source": {
                qualname: branch_rows(
                    verify_records, qualname, "generic_setattr"
                )
                for qualname in expected_set_sites
            },
        },
    )
