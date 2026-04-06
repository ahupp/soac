use soac_inspector::CounterDumpFile;
use soac_inspector::CounterDumpRowView;
use std::path::PathBuf;

struct Args {
    path: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut positionals = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
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
        path: PathBuf::from(&positionals[0]),
    })
}

fn print_usage() {
    eprintln!("usage: inspect_counters <counter-dump-file>");
}

fn format_counter_row(row: &CounterDumpRowView<'_>) -> String {
    let observed_value = row
        .observed_value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let max_overcount = row
        .max_overcount
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    format!(
        "  counter={} scope={} kind={} site={} site_function_id={} current_function_id={} instr_id={} function={} block={} value={} observed_value={} max_overcount={}",
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

fn main() -> Result<(), String> {
    let args = parse_args().inspect_err(|_| print_usage())?;
    let dump = CounterDumpFile::open(args.path.as_path())?;
    let records = dump.records()?;
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
    use super::format_counter_row;
    use soac_blockpy::block_py::{BlockLabel, FunctionId, InstrId};
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
}
