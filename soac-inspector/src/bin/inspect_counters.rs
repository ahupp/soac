use soac_inspector::CounterDumpFile;
use soac_inspector::CounterDumpRecordView;
use soac_inspector::CounterDumpRowView;
use soac_blockpy::block_py::FunctionId;
use std::collections::HashSet;
use std::path::PathBuf;

struct Args {
    emit_specializations: bool,
    path: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut emit_specializations = false;
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
        path: PathBuf::from(&positionals[0]),
    })
}

fn print_usage() {
    eprintln!("usage: inspect_counters [--specializations] <counter-dump-file>");
}

fn format_counter_row(row: &CounterDumpRowView<'_>) -> String {
    let observed_value = if row.kind == "call_hot_targets" {
        row.observed_value
            .map(FunctionId::from_packed)
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
            .map(|function_id| function_id.packed().to_string())
            .unwrap_or_else(|| "-".to_string()),
        row.current_function_id
            .map(|function_id| function_id.packed().to_string())
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

fn format_specializations(records: &[CounterDumpRecordView<'_>]) -> Result<String, String> {
    let mut ordered_keys = Vec::new();
    let mut seen_targets = HashSet::new();
    let mut targets = std::collections::HashMap::<String, Vec<u64>>::new();
    for record in records {
        let module = record.module_name()?;
        for row_index in 0..record.row_count() {
            let row = record.row(row_index)?;
            if row.kind != "call_hot_targets" {
                continue;
            }
            let Some(site_function_id) = row.function_id else {
                continue;
            };
            let Some(instr_id) = row.instr_id else {
                continue;
            };
            let Some(observed_value) = row.observed_value else {
                continue;
            };
            if observed_value == 0 {
                continue;
            }
            let observed_function_id = FunctionId::from_packed(observed_value);
            if observed_function_id == FunctionId::global() {
                continue;
            }
            let key = format!(
                "{}|{}|{}|{}",
                module,
                site_function_id.packed(),
                instr_id.block_label().as_u32(),
                instr_id.instr_index_in_block(),
            );
            let target_key = format!("{key}|{}", observed_function_id.packed());
            if seen_targets.insert(target_key) {
                if !targets.contains_key(&key) {
                    ordered_keys.push(key.clone());
                }
                targets.entry(key).or_default().push(observed_function_id.packed());
            }
        }
    }
    let mut out = String::new();
    for (index, key) in ordered_keys.iter().enumerate() {
        if index > 0 {
            out.push(';');
        }
        out.push_str(key);
        out.push('=');
        let Some(values) = targets.get(key) else {
            return Err(format!("missing specialization targets for key {key}"));
        };
        for (value_index, value) in values.iter().enumerate() {
            if value_index > 0 {
                out.push(',');
            }
            out.push_str(&value.to_string());
        }
    }
    Ok(out)
}

fn main() -> Result<(), String> {
    let args = parse_args().inspect_err(|_| print_usage())?;
    let dump = CounterDumpFile::open(args.path.as_path())?;
    let records = dump.records()?;
    if args.emit_specializations {
        println!("{}", format_specializations(&records)?);
        return Ok(());
    }
    for (record_index, record) in records.iter().enumerate() {
        println!(
            "record={} module={} package={} rows={}",
            record_index,
            record.module_name()?,
            record.package_name()?.unwrap_or("-"),
            record.row_count()
        );
        for row_index in 0..record.row_count() {
            let row = record.row(row_index)?;
            println!("{}", format_counter_row(&row));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{format_counter_row, format_specializations};
    use soac_blockpy::block_py::{BlockLabel, FunctionId, InstrId};
    use soac_inspector::parse_counter_dump_records;
    use soac_jit::counter_dump::{CounterDumpRecord, CounterDumpRow};
    use soac_inspector::CounterDumpRowView;

    #[test]
    fn row_output_includes_current_function_id() {
        let row = CounterDumpRowView {
            counter_id: 3,
            scope: "function",
            kind: "runtime_incref",
            site_kind: "runtime",
            function_id: Some(FunctionId::new(1, 7)),
            current_function_id: Some(FunctionId::new(1, 7)),
            instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
            function_qualname: Some("pkg.mod.f"),
            block_label: None,
            value: 11,
            observed_value: Some(12),
            max_overcount: Some(1),
        };

        let rendered = format_counter_row(&row);
        assert!(
            rendered
                .contains(format!("site_function_id={}", FunctionId::new(1, 7).packed()).as_str()),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                format!("current_function_id={}", FunctionId::new(1, 7).packed()).as_str()
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
            function_id: Some(FunctionId::global()),
            current_function_id: Some(FunctionId::global()),
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
                format!("site_function_id={}", FunctionId::global().packed()).as_str()
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                format!("current_function_id={}", FunctionId::global().packed()).as_str()
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
            function_id: Some(FunctionId::new(1, 7)),
            current_function_id: Some(FunctionId::new(1, 7)),
            instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
            function_qualname: Some("pkg.mod.f"),
            block_label: None,
            value: 11,
            observed_value: Some(FunctionId::new(1, 9).packed()),
            max_overcount: Some(1),
        };

        let rendered = format_counter_row(&row);
        assert!(rendered.contains("observed_function_id=1:9"), "{rendered}");
    }

    #[test]
    fn specialization_output_reads_directly_from_counter_dump() {
        let record = CounterDumpRecord {
            module_name: "mod".to_string(),
            package_name: None,
            rows: vec![
                CounterDumpRow {
                    counter_id: 1,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(FunctionId::new(1, 7)),
                    current_function_id: Some(FunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 11,
                    observed_value: Some(FunctionId::new(1, 9).packed()),
                    max_overcount: Some(1),
                },
                CounterDumpRow {
                    counter_id: 2,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(FunctionId::new(1, 7)),
                    current_function_id: Some(FunctionId::new(1, 7)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(2), 4)),
                    function_qualname: Some("pkg.mod.f".to_string()),
                    block_label: None,
                    value: 5,
                    observed_value: Some(FunctionId::new(1, 10).packed()),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 3,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(FunctionId::new(1, 8)),
                    current_function_id: Some(FunctionId::new(1, 8)),
                    instr_id: Some(InstrId::new(BlockLabel::from_index(3), 1)),
                    function_qualname: Some("pkg.mod.g".to_string()),
                    block_label: None,
                    value: 4,
                    observed_value: Some(FunctionId::global().packed()),
                    max_overcount: Some(0),
                },
                CounterDumpRow {
                    counter_id: 4,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(FunctionId::new(1, 8)),
                    current_function_id: Some(FunctionId::new(1, 8)),
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
        let records = parse_counter_dump_records(bytes.as_slice()).expect("counter dump should parse");
        let rendered = format_specializations(&records).expect("specializations should render");
        assert_eq!(rendered, "mod|4294967303|2|4=4294967305,4294967306");
    }
}
