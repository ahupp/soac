use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Deserialize)]
struct JitBbMapRecord {
    process_id: u32,
    code_id: u64,
    symbol: String,
    code_size: usize,
    function_id: String,
    function_qualname: String,
    bb_offsets: Vec<usize>,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
struct JitCodeKey {
    process_id: u32,
    code_id: u64,
}

#[derive(Debug)]
struct PerfSample {
    key: JitCodeKey,
    symbol: String,
    symoff: usize,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct BlockKey {
    jit: JitCodeKey,
    block: String,
    start: usize,
    end: usize,
}

#[derive(Debug, Default)]
struct BlockCount {
    samples: usize,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct FunctionBlockKey {
    function_local_id: String,
    function_qualname: String,
    block: String,
}

#[derive(Debug, Clone)]
struct FunctionBlockCount {
    samples: usize,
    start: usize,
    end: usize,
}

#[derive(Debug)]
struct FunctionInfo {
    local_id: String,
}

fn main() -> Result<(), String> {
    let result_dir = parse_result_dir()?;
    let map_path = result_dir.join("counters").join("jit-bb-map.jsonl");
    let perf_data = result_dir.join("perf.injected.data");
    let maps = load_jit_bb_maps(&map_path)?;
    let samples = perf_jit_samples(&perf_data)?;
    let mut counts: HashMap<BlockKey, BlockCount> = HashMap::new();
    let mut function_counts: HashMap<FunctionBlockKey, FunctionBlockCount> = HashMap::new();
    let mut total_jit_samples = 0usize;
    let mut mapped_jit_samples = 0usize;
    let mut missing_map_jit_samples = 0usize;
    let mut symbol_mismatches = 0usize;

    for sample in samples {
        total_jit_samples += 1;
        let Some(map) = maps.get(&sample.key) else {
            missing_map_jit_samples += 1;
            continue;
        };
        if map.symbol != sample.symbol {
            symbol_mismatches += 1;
        }
        let Some((block, start, end)) = block_for_offset(map, sample.symoff) else {
            continue;
        };
        mapped_jit_samples += 1;
        counts
            .entry(BlockKey {
                jit: sample.key,
                block: block.clone(),
                start,
                end,
            })
            .or_default()
            .samples += 1;
        function_counts
            .entry(FunctionBlockKey {
                function_local_id: function_local_id(&map.function_id).to_string(),
                function_qualname: map.function_qualname.clone(),
                block,
            })
            .and_modify(|count| {
                count.samples += 1;
                count.start = count.start.min(start);
                count.end = count.end.max(end);
            })
            .or_insert(FunctionBlockCount {
                samples: 1,
                start,
                end,
            });
    }

    write_annotated_vcode_files(
        &result_dir,
        &function_counts,
        total_jit_samples,
        mapped_jit_samples,
    )?;

    let mut rows = counts.into_iter().collect::<Vec<_>>();
    rows.sort_by(|(left_key, left_count), (right_key, right_count)| {
        right_count
            .samples
            .cmp(&left_count.samples)
            .then_with(|| left_key.jit.process_id.cmp(&right_key.jit.process_id))
            .then_with(|| left_key.jit.code_id.cmp(&right_key.jit.code_id))
            .then_with(|| left_key.block.cmp(&right_key.block))
    });

    println!("# result_dir={}", result_dir.display());
    println!("# perf_data={}", perf_data.display());
    println!("# jit_bb_map={}", map_path.display());
    println!("# total_jit_samples={total_jit_samples}");
    println!("# mapped_jit_samples={mapped_jit_samples}");
    println!("# missing_map_jit_samples={missing_map_jit_samples}");
    println!("# symbol_mismatches={symbol_mismatches}");
    println!(
        "samples\tpercent_jit\tpercent_mapped\tpid\tcode_id\tsymbol\tfunction_id\tfunction_qualname\tblock\tstart_hex\tend_hex"
    );
    for (key, count) in rows {
        let map = maps
            .get(&key.jit)
            .ok_or_else(|| "internal error: missing map for counted block".to_string())?;
        let percent_jit = percent(count.samples, total_jit_samples);
        let percent_mapped = percent(count.samples, mapped_jit_samples);
        println!(
            "{}\t{percent_jit:.2}\t{percent_mapped:.2}\t{}\t{}\t{}\t{}\t{}\t{}\t0x{:x}\t0x{:x}",
            count.samples,
            key.jit.process_id,
            key.jit.code_id,
            map.symbol,
            map.function_id,
            map.function_qualname,
            key.block,
            key.start,
            key.end,
        );
    }

    Ok(())
}

fn parse_result_dir() -> Result<PathBuf, String> {
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        return Err("usage: annotate_cranelift_perf <benchmark-result-dir>".to_string());
    }
    Ok(PathBuf::from(args.remove(0)))
}

fn load_jit_bb_maps(path: &Path) -> Result<HashMap<JitCodeKey, JitBbMapRecord>, String> {
    let input = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let mut out = HashMap::new();
    for (line_index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: JitBbMapRecord = serde_json::from_str(line).map_err(|err| {
            format!(
                "failed to parse {} line {} as jit bb map JSON: {err}",
                path.display(),
                line_index + 1
            )
        })?;
        out.insert(
            JitCodeKey {
                process_id: record.process_id,
                code_id: record.code_id,
            },
            record,
        );
    }
    Ok(out)
}

fn perf_jit_samples(perf_data: &Path) -> Result<Vec<PerfSample>, String> {
    let output = Command::new("perf")
        .arg("script")
        .arg("-G")
        .arg("-F")
        .arg("ip,sym,symoff,dso,dsoff,event")
        .arg("-i")
        .arg(perf_data)
        .output()
        .map_err(|err| format!("failed to run perf script: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "perf script failed with status {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|err| format!("perf script output was not UTF-8: {err}"))?;
    Ok(stdout.lines().filter_map(parse_perf_sample_line).collect())
}

fn parse_perf_sample_line(line: &str) -> Option<PerfSample> {
    let open = line.rfind(" (")?;
    let close = line.rfind(')')?;
    if close <= open {
        return None;
    }
    let dso_with_offset = &line[(open + 2)..close];
    let (dso_path, _dso_offset_hex) = dso_with_offset.rsplit_once("+0x")?;
    let key = parse_jitted_dso_key(dso_path)?;

    let before_dso = &line[..open];
    let mut fields = before_dso.split_whitespace();
    let _event = fields.next()?;
    let _ip = fields.next()?;
    let symbol_with_offset = fields.next()?;
    let (symbol, symoff_hex) = symbol_with_offset.rsplit_once("+0x")?;
    let symoff = usize::from_str_radix(symoff_hex, 16).ok()?;
    Some(PerfSample {
        key,
        symbol: symbol.to_string(),
        symoff,
    })
}

fn parse_jitted_dso_key(path: &str) -> Option<JitCodeKey> {
    let filename = Path::new(path).file_name()?.to_str()?;
    let stem = filename.strip_prefix("jitted-")?.strip_suffix(".so")?;
    let (pid, code_id) = stem.rsplit_once('-')?;
    Some(JitCodeKey {
        process_id: pid.parse().ok()?,
        code_id: code_id.parse().ok()?,
    })
}

fn block_for_offset(map: &JitBbMapRecord, symoff: usize) -> Option<(String, usize, usize)> {
    let first = *map.bb_offsets.first()?;
    if symoff < first {
        return Some(("prologue".to_string(), 0, first));
    }
    let index = map.bb_offsets.partition_point(|offset| *offset <= symoff) - 1;
    let start = map.bb_offsets[index];
    let end = map
        .bb_offsets
        .iter()
        .copied()
        .skip(index + 1)
        .find(|offset| *offset > start)
        .unwrap_or(map.code_size);
    Some((format!("block{index}"), start, end))
}

fn write_annotated_vcode_files(
    result_dir: &Path,
    function_counts: &HashMap<FunctionBlockKey, FunctionBlockCount>,
    total_jit_samples: usize,
    mapped_jit_samples: usize,
) -> Result<(), String> {
    let functions = load_functions(&result_dir.join("clif").join("functions.tsv"))?;
    let mut counts_by_function: HashMap<&str, HashMap<&str, &FunctionBlockCount>> = HashMap::new();
    for (key, count) in function_counts {
        counts_by_function
            .entry(key.function_local_id.as_str())
            .or_default()
            .insert(key.block.as_str(), count);
    }

    for function in functions {
        let Some(block_counts) = counts_by_function.get(function.local_id.as_str()) else {
            continue;
        };
        let input_path = find_vcode_path(&result_dir.join("clif"), &function.local_id)?;
        let output_path = input_path.with_extension("annotated.vcode");
        let input = std::fs::read_to_string(&input_path)
            .map_err(|err| format!("failed to read {}: {err}", input_path.display()))?;
        let annotated = annotate_vcode(&input, block_counts, total_jit_samples, mapped_jit_samples);
        std::fs::write(&output_path, annotated)
            .map_err(|err| format!("failed to write {}: {err}", output_path.display()))?;
    }
    Ok(())
}

fn find_vcode_path(clif_dir: &Path, function_local_id: &str) -> Result<PathBuf, String> {
    let prefix = format!("fn_{function_local_id}_");
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(clif_dir)
        .map_err(|err| format!("failed to read {}: {err}", clif_dir.display()))?
    {
        let entry =
            entry.map_err(|err| format!("failed to read {} entry: {err}", clif_dir.display()))?;
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if filename.starts_with(&prefix)
            && filename.ends_with(".vcode")
            && !filename.ends_with(".annotated.vcode")
        {
            matches.push(path);
        }
    }
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(format!(
            "failed to find VCode file in {} for function id {function_local_id}",
            clif_dir.display()
        )),
        _ => Err(format!(
            "found multiple VCode files in {} for function id {function_local_id}: {}",
            clif_dir.display(),
            matches
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn load_functions(path: &Path) -> Result<Vec<FunctionInfo>, String> {
    let input = std::fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    input
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(line_index, line)| {
            let (local_id, _qualname) = line.split_once('\t').ok_or_else(|| {
                format!(
                    "failed to parse {} line {} as '<function_id>\\t<qualname>'",
                    path.display(),
                    line_index + 1
                )
            })?;
            Ok(FunctionInfo {
                local_id: local_id.to_string(),
            })
        })
        .collect()
}

fn annotate_vcode(
    vcode: &str,
    block_counts: &HashMap<&str, &FunctionBlockCount>,
    total_jit_samples: usize,
    mapped_jit_samples: usize,
) -> String {
    let mut out = String::with_capacity(vcode.len() + (block_counts.len() * 96));
    let mut matched_blocks = 0usize;
    let mut matched_samples = 0usize;
    let prologue_count = block_counts.get("prologue").copied();

    out.push_str("; ---- perf sample annotations ----\n");
    out.push_str("; percent_jit is relative to all parsed JIT DSO samples in perf script output\n");
    out.push_str("; percent_mapped is relative to samples matched to direct JIT function code\n");
    if let Some(count) = prologue_count {
        matched_blocks += 1;
        matched_samples += count.samples;
        write_perf_annotation(
            &mut out,
            "prologue",
            count,
            total_jit_samples,
            mapped_jit_samples,
        );
    }

    for line in vcode.lines() {
        if let Some(block) = parse_vcode_block_label(line) {
            if let Some(count) = block_counts.get(block) {
                matched_blocks += 1;
                matched_samples += count.samples;
                write_perf_annotation(
                    &mut out,
                    block,
                    count,
                    total_jit_samples,
                    mapped_jit_samples,
                );
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    let total_annotated_samples = block_counts
        .values()
        .map(|count| count.samples)
        .sum::<usize>();
    let unmatched_blocks = block_counts.len().saturating_sub(matched_blocks);
    let unmatched_samples = total_annotated_samples.saturating_sub(matched_samples);
    let mut summary = String::new();
    writeln!(
        &mut summary,
        "; PERF annotation matched_blocks={matched_blocks} unmatched_blocks={unmatched_blocks} matched_samples={matched_samples} unmatched_samples={unmatched_samples}"
    )
    .expect("writing to string should not fail");
    out.insert_str(0, &summary);
    out
}

fn write_perf_annotation(
    out: &mut String,
    block: &str,
    count: &FunctionBlockCount,
    total_jit_samples: usize,
    mapped_jit_samples: usize,
) {
    writeln!(
        out,
        "; PERF block={block} samples={} percent_jit={:.2} percent_mapped={:.2} offset=0x{:x}..0x{:x}",
        count.samples,
        percent(count.samples, total_jit_samples),
        percent(count.samples, mapped_jit_samples),
        count.start,
        count.end,
    )
    .expect("writing to string should not fail");
}

fn parse_vcode_block_label(line: &str) -> Option<&str> {
    let label = line.strip_suffix(':')?;
    let block_index = label.strip_prefix("block")?;
    if block_index.chars().all(|ch| ch.is_ascii_digit()) {
        Some(label)
    } else {
        None
    }
}

fn function_local_id(function_id: &str) -> &str {
    function_id
        .rsplit_once(':')
        .map_or(function_id, |(_, local)| local)
}

fn percent(part: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (part as f64) * 100.0 / (total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jitted_dso_key() {
        assert_eq!(
            parse_jitted_dso_key("/tmp/result/counters/jitted-123-45.so"),
            Some(JitCodeKey {
                process_id: 123,
                code_id: 45
            })
        );
    }

    #[test]
    fn parses_perf_sample_line() {
        let line = "task-clock:ppp:      5bf1e02e50bc py:d:Proc0+0x10bc (/tmp/jitted-3291531-59.so+0x113c)";
        let sample = parse_perf_sample_line(line).expect("sample should parse");
        assert_eq!(sample.key.process_id, 3291531);
        assert_eq!(sample.key.code_id, 59);
        assert_eq!(sample.symbol, "py:d:Proc0");
        assert_eq!(sample.symoff, 0x10bc);
    }

    #[test]
    fn maps_offsets_to_blocks() {
        let map = JitBbMapRecord {
            process_id: 1,
            code_id: 2,
            symbol: "py:d:f".to_string(),
            code_size: 100,
            function_id: "1:1".to_string(),
            function_qualname: "f".to_string(),
            bb_offsets: vec![10, 20, 50],
        };
        assert_eq!(
            block_for_offset(&map, 5),
            Some(("prologue".to_string(), 0, 10))
        );
        assert_eq!(
            block_for_offset(&map, 25),
            Some(("block1".to_string(), 20, 50))
        );
        assert_eq!(
            block_for_offset(&map, 80),
            Some(("block2".to_string(), 50, 100))
        );
    }

    #[test]
    fn extracts_function_local_id() {
        assert_eq!(function_local_id("1:8"), "8");
        assert_eq!(function_local_id("8"), "8");
    }

    #[test]
    fn annotates_vcode_block_labels() {
        let block1 = FunctionBlockCount {
            samples: 3,
            start: 0x20,
            end: 0x30,
        };
        let mut counts = HashMap::new();
        counts.insert("block1", &block1);
        let annotated =
            annotate_vcode("; header\nblock0:\n  nop\nblock1:\n  ret\n", &counts, 10, 5);
        assert!(annotated.contains("; PERF annotation matched_blocks=1"));
        assert!(annotated.contains("; PERF block=block1 samples=3"));
        assert!(annotated.contains("block1:\n  ret"));
    }
}
