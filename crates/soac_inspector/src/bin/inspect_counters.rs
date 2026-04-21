use serde_json::json;
use soac_core::block_py::RuntimeFunctionId;
use soac_core::profile::{
    CounterDumpFile, CounterDumpKeyLayoutView, CounterDumpRowView, CounterDumpTypeKeyLayoutView,
    render_call_target_specializations,
};
use std::path::PathBuf;

struct Args {
    emit_specializations: bool,
    emit_json: bool,
    path: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut emit_specializations = false;
    let mut emit_json = false;
    let mut positionals = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--specializations" => {
                emit_specializations = true;
            }
            "--json" => {
                emit_json = true;
            }
            _ if arg.starts_with('-') => {
                return Err(format!("unknown option: {arg}"));
            }
            _ => positionals.push(arg),
        }
    }
    if positionals.len() != 1 {
        return Err("expected <counter-dump-file>".to_string());
    }
    Ok(Args {
        emit_specializations,
        emit_json,
        path: PathBuf::from(&positionals[0]),
    })
}

fn print_usage() {
    eprintln!("usage: inspect_counters [--json | --specializations] <counter-dump-file>");
}

fn format_counter_row(row: &CounterDumpRowView<'_>) -> String {
    let observed_value = if row.kind == "call_hot_targets" {
        row.observed_value
            .map(RuntimeFunctionId::from_packed_runtime_u64)
            .map(|function_id| format!("observed_function_id={function_id}"))
            .unwrap_or_else(|| "observed_function_id=-".to_string())
    } else {
        row.observed_value
            .map(|value| format!("observed_value={value}"))
            .unwrap_or_else(|| "observed_value=-".to_string())
    };
    let max_overcount = row
        .max_overcount
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    format!(
        "  counter={} scope={} kind={} site={} site_function_id={} current_function_id={} instr_id={} function={} block={} value={} {} max_overcount={}",
        row.counter_id,
        row.scope,
        row.kind,
        row.site_kind,
        row.function_id
            .map(|function_id| function_id.to_packed_runtime_u64().to_string())
            .unwrap_or_else(|| "-".to_string()),
        row.current_function_id
            .map(|function_id| function_id.to_packed_runtime_u64().to_string())
            .unwrap_or_else(|| "-".to_string()),
        row.instr_id
            .map(|instr_id| instr_id.to_string())
            .unwrap_or_else(|| "-".to_string()),
        row.function_qualname.unwrap_or("-"),
        row.block_label.unwrap_or("-"),
        row.value,
        observed_value,
        max_overcount,
    )
}

fn format_key_layout_row(kind: &str, row: &CounterDumpKeyLayoutView<'_>) -> String {
    format!(
        "  {kind}_key owner={} key={} index={}",
        row.owner, row.key, row.index
    )
}

fn format_type_key_layout_row(row: &CounterDumpTypeKeyLayoutView<'_>) -> String {
    format!(
        "  type_key owner_type_id={} key={} index={}",
        row.owner_type_id, row.key, row.index
    )
}

fn main() -> Result<(), String> {
    let args = parse_args().inspect_err(|_| print_usage())?;
    if args.emit_json && args.emit_specializations {
        return Err("--json and --specializations are mutually exclusive".to_string());
    }
    let dump = CounterDumpFile::open(args.path.as_path())?;
    let records = dump.records()?;
    if args.emit_specializations {
        println!("{}", render_call_target_specializations(&records)?);
        return Ok(());
    }
    if args.emit_json {
        let mut json_records = Vec::new();
        for record in records.iter() {
            let mut module_keys = Vec::new();
            for key_index in 0..record.module_key_count() {
                let key = record.module_key(key_index)?;
                module_keys.push(json!({
                    "owner": key.owner,
                    "key": key.key,
                    "index": key.index,
                }));
            }

            let mut type_keys = Vec::new();
            for key_index in 0..record.type_key_count() {
                let key = record.type_key(key_index)?;
                type_keys.push(json!({
                    "owner_type_id": key.owner_type_id,
                    "key": key.key,
                    "index": key.index,
                }));
            }

            let mut type_table = Vec::new();
            for entry_index in 0..record.type_table_count() {
                let entry = record.type_table_entry(entry_index)?;
                type_table.push(json!({
                    "type_id": entry.type_id,
                    "module_name": entry.module_name,
                    "qualname": entry.qualname,
                }));
            }

            let mut rows = Vec::new();
            for row_index in 0..record.row_count() {
                let row = record.row(row_index)?;
                rows.push(json!({
                    "counter_id": row.counter_id,
                    "scope": row.scope,
                    "kind": row.kind,
                    "site_kind": row.site_kind,
                    "function_id": row.function_id.map(|function_id| function_id.to_packed_runtime_u64()),
                    "current_function_id": row.current_function_id.map(|function_id| function_id.to_packed_runtime_u64()),
                    "instr_id": row.instr_id.map(|instr_id| instr_id.to_string()),
                    "function_qualname": row.function_qualname,
                    "block_label": row.block_label,
                    "value": row.value,
                    "observed_value": row.observed_value,
                    "max_overcount": row.max_overcount,
                }));
            }

            json_records.push(json!({
                "source_hash": format!("0x{:016x}", record.source_hash()),
                "module_name": record.module_name()?,
                "package_name": record.package_name()?,
                "module_keys": module_keys,
                "type_keys": type_keys,
                "type_table": type_table,
                "rows": rows,
            }));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "records": json_records }))
                .map_err(|err| format!("failed to encode counter dump JSON: {err}"))?
        );
        return Ok(());
    }
    for (record_index, record) in records.iter().enumerate() {
        println!(
            "record={} source_hash=0x{:016x} module={} package={} rows={} module_keys={} type_keys={} type_table={}",
            record_index,
            record.source_hash(),
            record.module_name()?,
            record.package_name()?.unwrap_or("-"),
            record.row_count(),
            record.module_key_count(),
            record.type_key_count(),
            record.type_table_count()
        );
        for key_index in 0..record.module_key_count() {
            let key = record.module_key(key_index)?;
            println!("{}", format_key_layout_row("module", &key));
        }
        for key_index in 0..record.type_key_count() {
            let key = record.type_key(key_index)?;
            println!("{}", format_type_key_layout_row(&key));
        }
        for entry_index in 0..record.type_table_count() {
            let entry = record.type_table_entry(entry_index)?;
            println!(
                "  type_table type_id={} module={} qualname={}",
                entry.type_id, entry.module_name, entry.qualname
            );
        }
        for row_index in 0..record.row_count() {
            let row = record.row(row_index)?;
            println!("{}", format_counter_row(&row));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::format_counter_row;
    use soac_core::block_py::{BlockLabel, InstrId, RuntimeFunctionId};
    use soac_core::profile::{
        CounterDumpRecord, CounterDumpRow, CounterDumpRowView, parse_counter_dump_records,
        render_call_target_specializations,
    };

    #[test]
    fn row_output_includes_current_function_id() {
        let row = CounterDumpRowView {
            counter_id: 3,
            scope: "function",
            kind: "runtime_incref",
            site_kind: "runtime",
            function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
            current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
            instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
            function_qualname: Some("pkg.mod.f"),
            block_label: None,
            value: 11,
            observed_value: Some(12),
            max_overcount: Some(1),
        };

        let rendered = format_counter_row(&row);
        assert!(
            rendered.contains(
                format!(
                    "site_function_id={}",
                    RuntimeFunctionId::from_raw_parts(1, 7).to_packed_runtime_u64()
                )
                .as_str()
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                format!(
                    "current_function_id={}",
                    RuntimeFunctionId::from_raw_parts(1, 7).to_packed_runtime_u64()
                )
                .as_str()
            ),
            "{rendered}"
        );
        assert!(rendered.contains("instr_id=bb2:4"), "{rendered}");
        assert!(rendered.contains("observed_value=12"), "{rendered}");
        assert!(rendered.contains("max_overcount=1"), "{rendered}");
    }

    #[test]
    fn global_row_output_uses_global_function_id() {
        let row = CounterDumpRowView {
            counter_id: 3,
            scope: "global",
            kind: "runtime_incref",
            site_kind: "runtime",
            function_id: Some(RuntimeFunctionId::global()),
            current_function_id: Some(RuntimeFunctionId::global()),
            instr_id: None,
            function_qualname: None,
            block_label: None,
            value: 11,
            observed_value: None,
            max_overcount: None,
        };

        let rendered = format_counter_row(&row);
        assert!(
            rendered.contains(
                format!(
                    "site_function_id={}",
                    RuntimeFunctionId::global().to_packed_runtime_u64()
                )
                .as_str()
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                format!(
                    "current_function_id={}",
                    RuntimeFunctionId::global().to_packed_runtime_u64()
                )
                .as_str()
            ),
            "{rendered}"
        );
    }

    #[test]
    fn call_hot_target_row_outputs_observed_function_id() {
        let row = CounterDumpRowView {
            counter_id: 8,
            scope: "this",
            kind: "call_hot_targets",
            site_kind: "runtime",
            function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
            current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
            instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
            function_qualname: Some("pkg.mod.f"),
            block_label: None,
            value: 11,
            observed_value: Some(RuntimeFunctionId::from_raw_parts(1, 9).to_packed_runtime_u64()),
            max_overcount: Some(1),
        };

        let rendered = format_counter_row(&row);
        assert!(rendered.contains("observed_function_id=1:9"), "{rendered}");
    }

    #[test]
    fn specialization_output_reads_directly_from_counter_dump() {
        let record = CounterDumpRecord {
            source_hash: 0,
            module_name: "mod".to_string(),
            package_name: None,
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
            rows: vec![
                CounterDumpRow {
                    counter_id: 1,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 11,
                    observed_value: Some(
                        RuntimeFunctionId::from_raw_parts(1, 9).to_packed_runtime_u64(),
                    ),
                    max_overcount: Some(1),
                },
                CounterDumpRow {
                    counter_id: 2,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
                    current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 5,
                    observed_value: Some(
                        RuntimeFunctionId::from_raw_parts(1, 10).to_packed_runtime_u64(),
                    ),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::from_raw_parts(1, 8)),
                    current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 1)),
                    function_qualname: Some("pkg.mod.g".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(RuntimeFunctionId::global().to_packed_runtime_u64()),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 4,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(RuntimeFunctionId::from_raw_parts(1, 8)),
                    current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 2)),
                    function_qualname: Some("pkg.mod.h".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(0),
                    max_overcount: Some(0),
                },
            ],
        };
        let bytes = record.encode().expect("counter dump should encode");
        let records =
            parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let rendered =
            render_call_target_specializations(&records).expect("specializations should render");
        assert_eq!(rendered, "mod|4294967303|2|4=4294967305,4294967306");
    }
}
