use serde_json::json;
use soac_core::block_py::RuntimeFunctionId;
use soac_core::profile::{
    CounterDumpFile, CounterDumpKeyLayoutView, CounterDumpRowView, CounterDumpTypeKeyLayoutView,
    render_call_target_specializations,
};
use std::collections::HashMap;
use std::path::PathBuf;

struct Args {
    emit_specializations: bool,
    emit_json: bool,
    emit_pretty: bool,
    path: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut emit_specializations = false;
    let mut emit_json = false;
    let mut emit_pretty = false;
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
            "--pretty" => {
                emit_pretty = true;
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
        emit_pretty,
        path: PathBuf::from(&positionals[0]),
    })
}

fn print_usage() {
    eprintln!(
        "usage: inspect_counters [--json | --pretty | --specializations] <counter-dump-file>"
    );
}

fn format_counter_row(row: &CounterDumpRowView<'_>) -> String {
    let branches = if row.branch_values.is_empty() {
        String::new()
    } else {
        format!(" branches={}", format_branch_values(row))
    };
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
        "  counter={} scope={} kind={} site={} site_function_id={} current_function_id={} instr_id={} function={} block={} value={}{} {} max_overcount={}",
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
        branches,
        observed_value,
        max_overcount,
    )
}

fn format_branch_values(row: &CounterDumpRowView<'_>) -> String {
    row.branch_values
        .iter()
        .map(|branch| format!("{}:{}", branch.branch, branch.value))
        .collect::<Vec<_>>()
        .join(",")
}

fn build_function_qualname_map<'a>(
    rows: &[CounterDumpRowView<'a>],
) -> HashMap<RuntimeFunctionId, &'a str> {
    let mut out = HashMap::new();
    for row in rows {
        if let (Some(function_id), Some(qualname)) = (row.function_id, row.function_qualname) {
            out.entry(function_id).or_insert(qualname);
        }
    }
    out
}

fn pretty_observed_function(
    observed_value: u64,
    function_qualnames: &HashMap<RuntimeFunctionId, &str>,
) -> String {
    let function_id = RuntimeFunctionId::from_packed_runtime_u64(observed_value);
    if function_id == RuntimeFunctionId::global() {
        return "<global>".to_string();
    }
    function_qualnames
        .get(&function_id)
        .copied()
        .unwrap_or("<unknown>")
        .to_string()
}

fn format_pretty_counter_row(
    row: &CounterDumpRowView<'_>,
    function_qualnames: &HashMap<RuntimeFunctionId, &str>,
) -> String {
    let mut fields = vec![
        format!("counter={}", row.counter_id),
        format!("kind={}", row.kind),
        format!("scope={}", row.scope),
        format!("site={}", row.site_kind),
    ];
    if let Some(qualname) = row.function_qualname {
        fields.push(format!("function={qualname}"));
    }
    if let Some(block_label) = row.block_label {
        fields.push(format!("block={block_label}"));
    }
    if let Some(instr_id) = row.instr_id {
        fields.push(format!("instr={instr_id}"));
    }
    fields.push(format!("value={}", row.value));
    if !row.branch_values.is_empty() {
        fields.push(format!("branches={}", format_branch_values(row)));
    }
    if let Some(observed_value) = row.observed_value {
        if row.kind == "call_hot_targets" {
            fields.push(format!(
                "observed_function={}",
                pretty_observed_function(observed_value, function_qualnames)
            ));
        } else {
            fields.push(format!("observed_value={observed_value}"));
        }
    }
    format!("  {}", fields.join(" "))
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
    let output_mode_count = usize::from(args.emit_json)
        + usize::from(args.emit_pretty)
        + usize::from(args.emit_specializations);
    if output_mode_count > 1 {
        return Err("--json, --pretty, and --specializations are mutually exclusive".to_string());
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
                    "branches": row.branch_values.iter().map(|branch| {
                        (branch.branch.to_string(), json!(branch.value))
                    }).collect::<serde_json::Map<String, serde_json::Value>>(),
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
        let mut rows = Vec::new();
        for row_index in 0..record.row_count() {
            rows.push(record.row(row_index)?);
        }
        let function_qualnames = build_function_qualname_map(rows.as_slice());
        for row in rows {
            if args.emit_pretty {
                println!("{}", format_pretty_counter_row(&row, &function_qualnames));
            } else {
                println!("{}", format_counter_row(&row));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_function_qualname_map, format_counter_row, format_pretty_counter_row};
    use soac_core::block_py::{BlockLabel, InstrId, RuntimeFunctionId};
    use soac_core::profile::{
        CounterDumpBranchValueView, CounterDumpRecord, CounterDumpRow, CounterDumpRowView,
        parse_counter_dump_records, render_call_target_specializations,
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
            branch_values: Vec::new(),
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
            branch_values: Vec::new(),
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
            branch_values: Vec::new(),
            observed_value: Some(RuntimeFunctionId::from_raw_parts(1, 9).to_packed_runtime_u64()),
            max_overcount: Some(1),
        };

        let rendered = format_counter_row(&row);
        assert!(rendered.contains("observed_function_id=1:9"), "{rendered}");
    }

    #[test]
    fn pretty_row_uses_qualname_and_omits_debug_only_empty_fields() {
        let row = CounterDumpRowView {
            counter_id: 55,
            scope: "this",
            kind: "block_entry",
            site_kind: "block_entry",
            function_id: Some(RuntimeFunctionId::from_raw_parts(1, 8)),
            current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 8)),
            instr_id: None,
            function_qualname: Some("run"),
            block_label: Some("bb9"),
            value: 10200,
            branch_values: Vec::new(),
            observed_value: None,
            max_overcount: Some(1),
        };

        let rows = vec![row];
        let function_qualnames = build_function_qualname_map(rows.as_slice());
        let rendered = format_pretty_counter_row(&rows[0], &function_qualnames);
        assert_eq!(
            rendered,
            "  counter=55 kind=block_entry scope=this site=block_entry function=run block=bb9 value=10200"
        );
        assert!(!rendered.contains("site_function_id"), "{rendered}");
        assert!(!rendered.contains("current_function_id"), "{rendered}");
        assert!(!rendered.contains("observed_value"), "{rendered}");
        assert!(!rendered.contains("max_overcount"), "{rendered}");
    }

    #[test]
    fn pretty_call_hot_target_row_resolves_observed_function_qualname() {
        let rows = vec![
            CounterDumpRowView {
                counter_id: 1,
                scope: "this",
                kind: "call_hot_targets",
                site_kind: "runtime",
                function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
                current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
                instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                function_qualname: Some("pkg.mod.caller"),
                block_label: None,
                value: 11,
                branch_values: Vec::new(),
                observed_value: Some(
                    RuntimeFunctionId::from_raw_parts(1, 9).to_packed_runtime_u64(),
                ),
                max_overcount: Some(1),
            },
            CounterDumpRowView {
                counter_id: 2,
                scope: "this",
                kind: "runtime_incref",
                site_kind: "runtime",
                function_id: Some(RuntimeFunctionId::from_raw_parts(1, 9)),
                current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 9)),
                instr_id: None,
                function_qualname: Some("pkg.mod.target"),
                block_label: None,
                value: 1,
                branch_values: Vec::new(),
                observed_value: None,
                max_overcount: None,
            },
        ];

        let function_qualnames = build_function_qualname_map(rows.as_slice());
        let rendered = format_pretty_counter_row(&rows[0], &function_qualnames);
        assert!(rendered.contains("function=pkg.mod.caller"), "{rendered}");
        assert!(
            rendered.contains("observed_function=pkg.mod.target"),
            "{rendered}"
        );
        assert!(!rendered.contains("observed_function_id"), "{rendered}");
        assert!(!rendered.contains("4294967305"), "{rendered}");
        assert!(!rendered.contains("max_overcount"), "{rendered}");
    }

    #[test]
    fn pretty_row_outputs_branch_values() {
        let row = CounterDumpRowView {
            counter_id: 4,
            scope: "this",
            kind: "operator_specialized",
            site_kind: "runtime",
            function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
            current_function_id: Some(RuntimeFunctionId::from_raw_parts(1, 7)),
            instr_id: Some(InstrId::new(BlockLabel::from_index(0), 0)),
            function_qualname: Some("add"),
            block_label: None,
            value: 0,
            branch_values: vec![
                CounterDumpBranchValueView {
                    branch: "hit",
                    value: 3,
                },
                CounterDumpBranchValueView {
                    branch: "fallback",
                    value: 1,
                },
            ],
            observed_value: None,
            max_overcount: None,
        };

        let rows = vec![row];
        let function_qualnames = build_function_qualname_map(rows.as_slice());
        let rendered = format_pretty_counter_row(&rows[0], &function_qualnames);
        assert_eq!(
            rendered,
            "  counter=4 kind=operator_specialized scope=this site=runtime function=add instr=bb0:0 value=0 branches=hit:3,fallback:1"
        );
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
                    branch_values: Vec::new(),
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
                    branch_values: Vec::new(),
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
                    branch_values: Vec::new(),
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
                    branch_values: Vec::new(),
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
