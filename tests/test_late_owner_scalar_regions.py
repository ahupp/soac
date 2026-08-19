from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def test_late_owner_cells_preserve_nonself_exact_int_field_regions(
    tmp_path: Path,
) -> None:
    module_name = "late_owner_scalar_regions_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            EVENTS = []


            class Record:
                def __init__(self, value):
                    self.value = value


            class Packet:
                def __init__(self, datum):
                    self.datum = datum


            class WorkState:
                __static_attributes__ = ()

                def __init__(self, count):
                    self.marker = "ready"
                    self.count = count


            class ObservedRecord(Record):
                def __getattribute__(self, name):
                    if name == "value":
                        EVENTS.append("subclass:get")
                    return object.__getattribute__(self, name)


            def record_branch(record):
                if record.value < 10:
                    return 1
                return 0


            def inline_record_branch(record):
                outcome = record_branch(record)
                return outcome


            class Handler:
                def packet_branch(self, packet):
                    if packet.datum < 10:
                        return 1
                    return 0

                def state_branch(self, state):
                    if state.count > 9:
                        return 1
                    return 0
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

        handler = module.Handler()
        for value in range(32):
            record = module.Record(value)
            packet = module.Packet(value)
            state = module.WorkState(value)

            # A second constructor call exercises an already-published
            # self-field anchor after the first insertion created its key.
            module.Record.__init__(record, value)
            module.Packet.__init__(packet, value)
            module.WorkState.__init__(state, value)

            assert module.record_branch(record) == int(value < 10)
            assert module.inline_record_branch(record) == int(value < 10)
            assert handler.packet_branch(packet) == int(value < 10)
            assert handler.state_branch(state) == int(value > 9)

        if os.environ.get("SOAC_OPT_MODE") != "profile":
            observed = module.ObservedRecord(4)
            module.EVENTS.clear()
            assert module.record_branch(observed) == 1
            assert module.EVENTS == ["subclass:get"], module.EVENTS

            module.EVENTS.clear()
            assert module.inline_record_branch(observed) == 1
            assert module.EVENTS == ["subclass:get"], module.EVENTS

            packet = module.Packet(4)
            materialized = packet.__dict__
            materialized["datum"] = 12
            assert handler.packet_branch(packet) == 0
            materialized[1] = "promoted"
            materialized["datum"] = 3
            assert handler.packet_branch(packet) == 1
            del materialized["datum"]
            try:
                handler.packet_branch(packet)
            except AttributeError as error:
                assert "datum" in str(error), error
            else:
                raise AssertionError("a deleted promoted field must raise")
            materialized["datum"] = 7
            assert handler.packet_branch(packet) == 1

            unseeded = module.WorkState(15)
            assert unseeded.__dict__ == {{"marker": "ready", "count": 15}}
            assert handler.state_branch(unseeded) == 1

            record = module.Record(3)

            def descriptor_get(instance):
                module.EVENTS.append("descriptor:get")
                return 17

            module.Record.value = property(descriptor_get)
            try:
                module.EVENTS.clear()
                assert module.record_branch(record) == 0
                assert module.EVENTS == ["descriptor:get"], module.EVENTS

                module.EVENTS.clear()
                assert module.inline_record_branch(record) == 0
                assert module.EVENTS == ["descriptor:get"], module.EVENTS
            finally:
                del module.Record.value
            assert module.record_branch(record) == 1

            original_record = module.Record

            class ReplacementRecord:
                def __init__(self, value):
                    self.value = value

            module.Record = ReplacementRecord
            try:
                assert module.record_branch(module.Record(6)) == 1
            finally:
                module.Record = original_record
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

    def run_mode(mode: str) -> Path:
        log_path = tmp_path / f"{mode}-events.jsonl"
        result = subprocess.run(
            [sys.executable, "-c", script],
            check=False,
            capture_output=True,
            text=True,
            env={
                **base_env,
                "SOAC_OPT_MODE": mode,
                "SOAC_LOG": (
                    "soac_jit_codegen=info,soac_typed_inline_fixpoint=debug;"
                    f"json={log_path}"
                ),
            },
            timeout=60,
        )
        assert result.returncode == 0, (
            f"{mode} subprocess failed:\n{result.stdout}{result.stderr}"
        )
        return log_path

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

    inline_call_targets = [
        row
        for record in profile_records
        for row in record["rows"]
        if row["kind"] == "call_hot_targets"
        and row["function_qualname"] == "inline_record_branch"
        and row["value"] >= 32
    ]
    assert inline_call_targets, (
        "the same-module wrapper must profile its real inline call target",
        [
            row
            for record in profile_records
            for row in record["rows"]
            if row["kind"] == "call_hot_targets"
            and row["function_qualname"] == "inline_record_branch"
        ],
    )

    def field_counts(records: list[dict], branch: str) -> dict[str, int]:
        counts: dict[str, int] = {}
        for record in records:
            for row in record["rows"]:
                if row["kind"] != "field_access":
                    continue
                qualname = row["function_qualname"]
                counts[qualname] = counts.get(qualname, 0) + row.get(
                    "branches", {}
                ).get(branch, 0)
        return counts

    constructors = {
        "Record.__init__": "value",
        "Packet.__init__": "datum",
        "WorkState.__init__": "count",
    }
    consumers = {
        "record_branch": ("Record", "value"),
        "Handler.packet_branch": ("Packet", "datum"),
        "Handler.state_branch": ("WorkState", "count"),
    }

    profile_sets = field_counts(profile_records, "generic_setattr")
    profile_gets = field_counts(profile_records, "generic_getattr")
    assert all(profile_sets.get(name, 0) >= 32 for name in constructors), (
        "existing class-owned constructor stores must supply hot anchor cells",
        profile_sets,
    )
    assert all(profile_gets.get(name, 0) >= 32 for name in consumers), (
        "the actual non-self consumers must record their own field sources",
        profile_gets,
    )

    owner_type_ids = {
        entry["qualname"]: entry["type_id"]
        for record in profile_dump["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
        and entry["qualname"] in {owner for owner, _ in consumers.values()}
    }
    assert set(owner_type_ids) == {"Record", "Packet", "WorkState"}, (
        owner_type_ids,
        profile_dump,
    )
    profiled_owner_fields = {
        (owner_name, key["key"])
        for record in profile_dump["records"]
        for key in record["type_keys"]
        for owner_name, type_id in owner_type_ids.items()
        if key["owner_type_id"] == type_id
    }
    assert set(consumers.values()) <= profiled_owner_fields, (
        "the existing owner-specific split-key profile must identify every "
        "constructor anchor and consumer field",
        profiled_owner_fields,
    )

    exact_int_operators = [
        row
        for record in profile_records
        for row in record["rows"]
        if row["kind"] == "operator_hot_shapes"
        and row["function_qualname"] in consumers
        and row.get("observed_value") == 0x0101
        and row["value"] >= 8
    ]
    assert {row["function_qualname"] for row in exact_int_operators} == set(
        consumers
    ), {
        "required_exact_int_pair_shape": 0x0101,
        "consumer_operator_rows": [
            {
                "function": row["function_qualname"],
                "instr_id": row["instr_id"],
                "shape": row.get("observed_value"),
                "count": row["value"],
            }
            for record in profile_records
            for row in record["rows"]
            if row["kind"] == "operator_hot_shapes"
            and row["function_qualname"] in consumers
        ],
    }

    verify_log = run_mode("verify")
    verify_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "verify.bin"))
    )
    verify_records = [
        record
        for record in verify_dump["records"]
        if record["module_name"] == module_name
    ]
    assert verify_records, verify_dump

    apply_log = run_mode("apply")

    indexed_hits = field_counts(verify_records, "indexed_hit")
    assert all(indexed_hits.get(name, 0) >= 16 for name in constructors), (
        "the existing late-bound constructor owner cells must already work",
        indexed_hits,
    )

    invalidations_by_mode = {}
    inline_rewrites_by_mode = {}
    for mode, log_path in (("verify", verify_log), ("apply", apply_log)):
        rows = [
            json.loads(line)
            for line in log_path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        inline_rewrites_by_mode[mode] = [
            {
                "rewritten_stores": row.get("rewritten_stores", 0),
                "inline_instance_source_count": row.get(
                    "inline_instance_source_count", 0
                ),
                "instr_id_mapping_count": row.get("instr_id_mapping_count", 0),
            }
            for row in rows
            if row.get("target") == "soac_typed_inline_fixpoint"
            and row.get("message") == "typed_inline_fixpoint_rewrite_stats"
            and row.get("function_qualname") == "inline_record_branch"
            and row.get("rewrote_inline") is True
            and row.get("rewritten_stores", 0) >= 1
            and row.get("inline_instance_source_count", 0) >= 1
            and row.get("instr_id_mapping_count", 0) >= 1
        ]
        invalidations_by_mode[mode] = [
            {
                "function": row["function_qualname"],
                "missing_field_sources": row.get("missing_field_sources"),
                "invalidated_branch_regions": row.get("invalidated_branch_regions"),
                "invalidated_return_regions": row.get("invalidated_return_regions"),
            }
            for row in rows
            if row.get("message")
            == "typed_scalar_regions_invalidated_without_live_indexed_field_guards"
            and row.get("function_qualname") in consumers
        ]

    assert all(inline_rewrites_by_mode.values()), (
        "Verify and Apply must actually inline the trained same-module "
        "consumer and remap its instruction sources",
        inline_rewrites_by_mode,
    )

    consumer_hits = {name: indexed_hits.get(name, 0) for name in consumers}
    assert all(count >= 16 for count in consumer_hits.values()), (
        "top-level and non-self exact-int consumers must reuse their owner's "
        "existing late-bound constructor guard and retain source-attributed "
        "indexed-hit counters",
        {
            "consumer_indexed_hits": consumer_hits,
            "constructor_indexed_hits": {
                name: indexed_hits.get(name, 0) for name in constructors
            },
            "scalar_region_invalidations": invalidations_by_mode,
        },
    )
    assert not any(invalidations_by_mode.values()), (
        "a matching late-bound owner cell must keep the exact-int branch "
        "region alive in both Verify and Apply",
        invalidations_by_mode,
    )
