use std::fs;
use std::path::PathBuf;

const VALIDATE_DELIMITER: &str = "# diet-python: validate";

fn parse_args() -> Result<PathBuf, String> {
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
        return Err("expected <source>".to_string());
    }
    Ok(PathBuf::from(&positionals[0]))
}

fn print_usage() {
    eprintln!("usage: list_jit_functions <source>");
}

fn split_source(path: &PathBuf) -> Result<String, String> {
    let source = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let source = source
        .split_once(VALIDATE_DELIMITER)
        .map(|(before, _)| before)
        .unwrap_or(source.as_str());
    Ok(format!("{}\n", source.trim_end()))
}

fn main() -> Result<(), String> {
    let source_path = parse_args().inspect_err(|_| print_usage())?;
    let source_path = source_path.canonicalize().map_err(|err| {
        format!(
            "failed to resolve source path {}: {err}",
            source_path.display()
        )
    })?;
    let source = split_source(&source_path)?;
    let output = soac_blockpy::lower_python_to_blockpy_for_testing(&source)
        .map_err(|err| err.to_string())?;
    for function in &output.codegen_module.callable_defs {
        println!(
            "{}\t{}",
            function.function_id.packed(),
            function.names.qualname
        );
    }
    Ok(())
}
