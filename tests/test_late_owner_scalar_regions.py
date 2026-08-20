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
from tests.test_late_owner_nonself_fields import _OWNER_WITNESSES


_SOURCE_FUNCTIONS = (
    "Record.__init__", "Packet.__init__", "WorkState.__init__",
    "ObservedRecord.__getattribute__", "record_branch", "inline_record_branch",
    "Handler.packet_branch", "Handler.state_branch",
)


def test_late_owner_scalar_consumers_preserve_ordinary_storage_and_callbacks(
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

    # Preserve the complete original source and validator literals above.
    import_line = f"import {module_name} as module\n"
    assert script.count(import_line) == 1
    ordinary_body = script.split(import_line, 1)[1]
    witness_setup = (
        f"source_functions = {_SOURCE_FUNCTIONS!r}\n"
        "dynamic_source_methods = ('ObservedRecord.__getattribute__',)\n"
        "dynamic_classes = ('ObservedRecord',)\n"
        "observed_classes = ('Record', 'Packet', 'WorkState', 'ObservedRecord', 'Handler')\n"
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

    mutation_start = "    module.Record.value = property(descriptor_get)\n"
    assert ordinary_body.count(mutation_start) == 1
    selected_body = ordinary_body.split(mutation_start, 1)[0]
    selected_body += textwrap.indent(textwrap.dedent("""
        with reject_sealed_mutation('field_descriptor'):
            module.Record.value = property(descriptor_get)
        module.EVENTS.clear()
        assert module.record_branch(record) == 1
        assert module.inline_record_branch(record) == 1
        assert module.EVENTS == [], module.EVENTS

        # The selected direct/wrapper callers still honor a permitted ordinary
        # descriptor exactly once; only mutation of the sealed Record differs.
        class DescriptorRecord:
            value = property(descriptor_get)

        ordinary_descriptor = DescriptorRecord()
        assert module.record_branch(ordinary_descriptor) == 0
        assert module.EVENTS == ["descriptor:get"], module.EVENTS
        module.EVENTS.clear()
        assert module.inline_record_branch(ordinary_descriptor) == 0
        assert module.EVENTS == ["descriptor:get"], module.EVENTS

        original_record = module.Record

        class ReplacementRecord:
            def __init__(self, value):
                self.value = value

        with reject_sealed_mutation('module_class_binding'):
            module.Record = ReplacementRecord
        assert module.Record is original_record
        assert module.record_branch(ReplacementRecord(6)) == 1
        assert module.inline_record_branch(ReplacementRecord(6)) == 1
        assert module.record_branch(module.Record(6)) == 1
    """).lstrip("\n"), "    ")
    for old, new in (
        (
            "    materialized = packet.__dict__\n",
            "    materialized = packet.__dict__\n"
            "    assert type(materialized) is dict\n"
            "    assert _testinternalcapi.dict_has_indexed_keys(materialized) is False\n",
        ),
        (
            '    materialized[1] = "promoted"\n',
            '    materialized[1] = "promoted"\n'
            "    assert _testinternalcapi.dict_has_indexed_keys(materialized) is False\n",
        ),
    ):
        assert selected_body.count(old) == 1, old
        selected_body = selected_body.replace(old, new)
    selected_body += """
if os.environ.get('SOAC_OPT_MODE') != 'profile':
    assert sealed_rejections == ['field_descriptor', 'module_class_binding'], sealed_rejections
assert_bindings()
import json
print(json.dumps({'source_ids': {
    path: saved[4][1] for path, saved in saved_functions.items()
    if path not in dynamic_source_methods
}}, sort_keys=True))
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

    def run_mode(mode: str, *, entry_interpreter: bool = False) -> dict[str, int]:
        program = _VALIDATION_PRELUDE + project._validation_program(
            module_name, case, entry_interpreter=entry_interpreter
        )
        result = project.run(
            program, opt_mode=mode, entry_interpreter=entry_interpreter,
            extra_env={"SOAC_WORK_DIR": str(work_dir)}, timeout=60, check=False,
        )
        assert result.returncode == 0, (
            f"{mode} subprocess failed:\n{result.stdout}{result.stderr}"
        )
        return json.loads(result.stdout)["source_ids"]

    source_ids = run_mode("profile")

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
        and row["function_id"] == source_ids["inline_record_branch"]
        and row.get("observed_value") == source_ids["record_branch"]
        and row["value"] >= 32
    ]
    assert inline_call_targets, (
        "the same-module wrapper must profile its actual checked source target",
        [
            row
            for record in profile_records
            for row in record["rows"]
            if row["kind"] == "call_hot_targets"
            and row["function_qualname"] == "inline_record_branch"
        ],
    )

    def field_counts(records: list[dict], *branches: str) -> dict[str, int]:
        counts: dict[str, int] = {}
        for record in records:
            for row in record["rows"]:
                if row["kind"] != "field_access":
                    continue
                qualname = row["function_qualname"]
                counts[qualname] = counts.get(qualname, 0) + sum(
                    row.get("branches", {}).get(branch, 0) for branch in branches
                )
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
        "the original constructor stores must retain their field observations",
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
        "constructor owner and consumer field",
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

    indexed_hits = field_counts(verify_records, "indexed_hit")
    assert all(indexed_hits.get(name, 0) == 0 for name in constructors | consumers), (
        "ordinary checked dictionaries acquired an unchecked field hit",
        indexed_hits,
    )
    # Generic lookup and an indexed proposal's checked fallback both preserve
    # the required read; the fixture does not require optional plan selection.
    checked_reads = field_counts(verify_records, "generic_getattr", "indexed_fallback")
    checked_writes = field_counts(verify_records, "generic_setattr")
    assert all(checked_reads.get(name, 0) >= 32 for name in consumers), (
        "each observed consumer must retain its checked lookup fallback",
        checked_reads,
    )
    assert all(checked_writes.get(name, 0) >= 32 for name in constructors), (
        "each constructor must retain its checked attribute writes",
        checked_writes,
    )
    # A source ID, shared-key observation or int/int sample is not a live
    # indexed-field guard. Missing guards must invalidate dependent scalar
    # selections. Matching/mismatched guards and atomic getter/comparison
    # lowering remain covered by the typed-pipeline structured unit tests;
    # this behavioral fixture does not require scalarization or inlining.
    run_mode("none", entry_interpreter=True)
