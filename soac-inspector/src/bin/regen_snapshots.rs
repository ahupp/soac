use std::collections::HashMap;
use std::env;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;

use soac_core::block_py::BlockPyModule;
use soac_lowering::fixture::{FixtureBlock, parse_fixture, render_fixture};
use soac_lowering::passes::CodegenModuleShape;
use soac_lowering::{LoweringResult, lower_python_to_blockpy_for_testing};
use tracing::{Level, enabled, trace};

struct SnapshotSummaryRow {
    case_name: String,
    blockpy_blocks: Option<usize>,
    clif_blocks: Option<usize>,
    error: Option<String>,
}

fn collect_fixtures(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if enabled!(Level::TRACE) {
        trace!("collect_fixtures: entering {}", root.display());
    }
    let entries = fs::read_dir(root)
        .map_err(|err| format!("failed to read directory {}: {}", root.display(), err))?;
    for entry in entries {
        let entry = entry.map_err(|err| err.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_fixtures(&path, out)?;
        } else if path.file_name().is_some_and(|name| {
            name.to_string_lossy().starts_with("snapshot_")
                && path.extension().is_some_and(|ext| ext == "py")
        }) {
            if enabled!(Level::TRACE) {
                trace!("collect_fixtures: found {}", path.display());
            }
            out.push(path);
        }
    }
    Ok(())
}

fn repo_root() -> Result<PathBuf, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "failed to find workspace root from CARGO_MANIFEST_DIR".to_string())
}

fn snapshot_dir() -> Result<PathBuf, String> {
    Ok(repo_root()?.join("snapshot"))
}

fn fixture_root() -> Result<PathBuf, String> {
    snapshot_dir()
}

fn snapshot_output_path_for_fixture(path: &Path) -> Result<PathBuf, String> {
    let file_stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("invalid fixture filename: {}", path.display()))?;
    Ok(snapshot_dir()?.join(format!("{file_stem}.py")))
}

fn qualified_case_name(path: &Path, block: &FixtureBlock) -> Result<String, String> {
    let fixture_name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("invalid fixture filename: {}", path.display()))?;
    Ok(format!("{fixture_name}::{}", block.name))
}

fn render_blockpy_snapshot(_source: &str, result: &LoweringResult) -> (String, usize, usize) {
    let core_blockpy = result.pass_tracker.pass_core_blockpy_with_await_and_yield();
    let blockpy_rendered = result
        .pass_tracker
        .render_pass_text("core_blockpy_with_await_and_yield")
        .unwrap_or_else(|| "; no BlockPy module emitted".to_string());
    let blockpy_blocks = core_blockpy
        .as_ref()
        .map(|module| {
            module
                .callable_defs
                .iter()
                .map(|function| function.blocks.len())
                .sum()
        })
        .unwrap_or(0);
    let clif_blocks = count_clif_blocks(&result.codegen_module);
    (blockpy_rendered, blockpy_blocks, clif_blocks)
}

fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        "non-string panic payload".to_string()
    }
}

fn format_snapshot_error_message(message: &str) -> String {
    let message = message.trim();
    if message.is_empty() {
        "snapshot regeneration failed".to_string()
    } else {
        format!("snapshot regeneration failed\n{message}")
    }
}

fn summary_error_text(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn with_suppressed_panic_hook<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let result = panic::catch_unwind(AssertUnwindSafe(f));
    panic::set_hook(hook);
    match result {
        Ok(result) => result,
        Err(payload) => Err(format!("panic: {}", panic_payload_message(payload))),
    }
}

fn count_clif_blocks(module: &BlockPyModule<CodegenModuleShape>) -> usize {
    module
        .callable_defs
        .iter()
        .map(|function| function.blocks.len())
        .sum()
}

fn write_if_changed(path: &Path, contents: &str) -> Result<(), String> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing != contents {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {}", parent.display(), err))?;
        }
        fs::write(path, contents).map_err(|err| format!("{}: {}", path.display(), err))?;
    }
    Ok(())
}

fn render_snapshot_python_fixture(blocks: &[FixtureBlock]) -> String {
    let mut output = String::new();
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        output.push_str("# ");
        output.push_str(&block.name);
        output.push_str("\n\n");
        let input = block.input.trim_matches('\n');
        if !input.is_empty() {
            output.push_str(input);
            output.push('\n');
        }
        output.push('\n');
        output.push_str("# ==\n\n");
        let output_block = block.output.trim_matches('\n');
        if !output_block.is_empty() {
            for line in output_block.lines() {
                if line.is_empty() {
                    output.push('\n');
                } else {
                    output.push_str("# ");
                    output.push_str(line);
                    output.push('\n');
                }
            }
        }
    }
    output
}

fn is_fixture_header_line(line: &str) -> bool {
    line.starts_with("# ") && line != "# ==" && line != "# -- pre-bb --" && line != "# -- bb --"
}

fn next_nonempty_line<'a>(lines: &'a [String], start: usize) -> Option<&'a str> {
    lines[start..]
        .iter()
        .map(String::as_str)
        .find(|line| !line.trim().is_empty())
}

fn is_snapshot_block_header(lines: &[String], index: usize) -> bool {
    let Some(line) = lines.get(index) else {
        return false;
    };
    if !is_fixture_header_line(line) {
        return false;
    }
    next_nonempty_line(lines, index + 1).is_some_and(|line| !line.starts_with('#'))
}

fn parse_snapshot_fixture(contents: &str) -> Result<Vec<FixtureBlock>, String> {
    let lines = contents.lines().map(str::to_string).collect::<Vec<_>>();
    let mut blocks = Vec::new();
    let mut index = 0usize;

    while index < lines.len() {
        while index < lines.len() && lines[index].trim().is_empty() {
            index += 1;
        }
        if index >= lines.len() {
            break;
        }
        if !is_snapshot_block_header(&lines, index) {
            return Err(format!(
                "unexpected content outside of snapshot fixture blocks: `{}`",
                lines[index]
            ));
        }

        let name = lines[index][2..].trim().to_string();
        index += 1;

        let mut input_lines = Vec::new();
        let mut saw_separator = false;
        while index < lines.len() {
            let line = &lines[index];
            if line.trim() == "# ==" {
                saw_separator = true;
                index += 1;
                break;
            }
            input_lines.push(line.clone());
            index += 1;
        }

        if !saw_separator {
            return Err(format!(
                "missing `# ==` separator in snapshot fixture `{name}`"
            ));
        }

        while index < lines.len() && !is_snapshot_block_header(&lines, index) {
            index += 1;
        }

        let input = if input_lines.is_empty() {
            String::new()
        } else {
            let mut text = input_lines.join("\n");
            text.push('\n');
            text
        };

        blocks.push(FixtureBlock {
            name,
            input,
            output: String::new(),
            seen_separator: true,
        });
    }

    Ok(blocks)
}

fn load_fixture_blocks(path: &Path, contents: &str) -> Result<Vec<FixtureBlock>, String> {
    if path.starts_with(snapshot_dir()?) {
        parse_snapshot_fixture(contents)
    } else {
        parse_fixture(contents)
    }
}

fn regenerate_fixture(path: &Path, summary: &mut Vec<SnapshotSummaryRow>) -> Result<(), String> {
    if enabled!(Level::TRACE) {
        trace!("regenerate_fixture: start {}", path.display());
    }
    let contents =
        fs::read_to_string(path).map_err(|err| format!("{}: {}", path.display(), err))?;
    let mut blocks = load_fixture_blocks(path, &contents)?;
    if enabled!(Level::TRACE) {
        trace!("regenerate_fixture: parsed {} blocks", blocks.len());
    }

    let mut snapshot_blocks = Vec::with_capacity(blocks.len());
    for block in &blocks {
        if enabled!(Level::TRACE) {
            trace!("regenerate_fixture: transforming {}", block.name);
        }
        let case_name = qualified_case_name(path, block)?;
        let snapshot_result = with_suppressed_panic_hook(|| {
            let transformed = lower_python_to_blockpy_for_testing(&block.input)
                .map_err(|err| format!("{}: {}", path.display(), err))?;
            Ok(render_blockpy_snapshot(block.input.as_str(), &transformed))
        });
        let (output, blockpy_blocks, clif_blocks, error) = match snapshot_result {
            Ok((output, blockpy_blocks, clif_blocks)) => {
                (output, Some(blockpy_blocks), Some(clif_blocks), None)
            }
            Err(message) => {
                let error = format_snapshot_error_message(&message);
                (error.clone(), None, None, Some(summary_error_text(&error)))
            }
        };
        summary.push(SnapshotSummaryRow {
            case_name,
            blockpy_blocks,
            clif_blocks,
            error,
        });
        snapshot_blocks.push(FixtureBlock {
            name: block.name.clone(),
            input: block.input.clone(),
            output: format!("{}\n", output.trim_end()),
            seen_separator: true,
        });
    }

    for block in &mut blocks {
        block.output.clear();
    }
    let rendered_fixture = render_fixture(&blocks);
    write_if_changed(path, &rendered_fixture)?;
    let snapshot_path = snapshot_output_path_for_fixture(path)?;
    let rendered_snapshot = render_snapshot_python_fixture(&snapshot_blocks);
    write_if_changed(&snapshot_path, &rendered_snapshot)?;

    Ok(())
}

fn format_python_files(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut command = Command::new("ruff");
    command.arg("format");
    for path in paths {
        command.arg(path);
    }
    let status = command
        .status()
        .map_err(|err| format!("failed to run ruff format: {}", err))?;
    if !status.success() {
        return Err(format!("ruff format failed with status {}", status));
    }

    Ok(())
}

fn write_summary(summary: &[SnapshotSummaryRow]) -> Result<(), String> {
    if summary.is_empty() {
        return Ok(());
    }

    let summary_path = snapshot_dir()?.join("snapshot_summary.txt");
    let mut total_by_name = HashMap::new();
    for row in summary {
        *total_by_name.entry(row.case_name.clone()).or_insert(0usize) += 1;
    }

    let mut seen_by_name = HashMap::new();
    let mut contents = String::new();
    for row in summary {
        let seen = seen_by_name.entry(row.case_name.clone()).or_insert(0usize);
        *seen += 1;
        let case_name = if total_by_name[&row.case_name] > 1 {
            format!("{} [{}]", row.case_name, *seen)
        } else {
            row.case_name.clone()
        };
        if let Some(error) = &row.error {
            contents.push_str(&format!("{case_name}: ERROR {error}\n"));
        } else {
            contents.push_str(&format!(
                "{}: blockpy={}, clif={}\n",
                case_name,
                row.blockpy_blocks.unwrap_or(0),
                row.clif_blocks.unwrap_or(0)
            ));
        }
    }
    write_if_changed(&summary_path, &contents)
}

fn main() -> Result<(), String> {
    soac_config::init_logging()?;
    fs::create_dir_all(snapshot_dir()?)
        .map_err(|err| format!("failed to create snapshot dir: {}", err))?;
    let args: Vec<String> = env::args().skip(1).collect();
    let mut fixtures = Vec::new();

    if args.is_empty() {
        let root = fixture_root()?;
        collect_fixtures(&root, &mut fixtures)?;
    } else {
        for arg in args {
            fixtures.push(PathBuf::from(arg));
        }
    }

    fixtures.sort();
    let mut summary = Vec::new();
    for path in &fixtures {
        regenerate_fixture(path, &mut summary)?;
    }
    let mut python_files = fixtures.clone();
    for fixture in &fixtures {
        python_files.push(snapshot_output_path_for_fixture(fixture)?);
    }
    format_python_files(&python_files)?;
    write_summary(&summary)?;

    Ok(())
}

#[cfg(test)]
#[path = "regen_snapshots/test.rs"]
mod test;
