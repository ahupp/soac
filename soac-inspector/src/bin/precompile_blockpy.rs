use soac_blockpy::codegen_cache::load_codegen_module_cache;
use soac_jit::precompile_codegen_module_to_object_file;
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Default)]
struct Args {
    module: Option<PathBuf>,
    counters: Option<PathBuf>,
    module_name: Option<String>,
    out: Option<PathBuf>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("precompile_blockpy: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(env::args_os().skip(1))?;
    let module_path = args
        .module
        .ok_or_else(|| "missing required --module <path>".to_string())?;
    let module_name = args
        .module_name
        .ok_or_else(|| "missing required --module-name <name>".to_string())?;
    let counters_path = args
        .counters
        .ok_or_else(|| "missing required --counters <path>".to_string())?;
    if !counters_path.exists() {
        return Err(format!(
            "counter dump does not exist: {}",
            counters_path.display()
        ));
    }
    let out_path = args
        .out
        .ok_or_else(|| "missing required --out <path>".to_string())?;
    let module = load_codegen_module_cache(&module_path)
        .map_err(|err| format!("failed to load {}: {err}", module_path.display()))?;
    let summary = precompile_codegen_module_to_object_file(
        module_name.as_str(),
        &module,
        Some(counters_path.as_path()),
        &out_path,
    )?;
    println!(
        "wrote {} ({} bytes, {} functions, {} data objects)",
        summary.output_path.display(),
        summary.object_size_bytes,
        summary.function_count,
        summary.data_object_count
    );
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let flag = arg
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 argument is unsupported: {arg:?}"))?;
        match flag {
            "--module" => parsed.module = Some(next_path(&mut args, flag)?),
            "--counters" => parsed.counters = Some(next_path(&mut args, flag)?),
            "--module-name" => parsed.module_name = Some(next_string(&mut args, flag)?),
            "--out" => parsed.out = Some(next_path(&mut args, flag)?),
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument {flag:?}")),
        }
    }
    Ok(parsed)
}

fn next_path(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(
        args.next()
            .ok_or_else(|| format!("{flag} requires a value"))?,
    ))
}

fn next_string(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))?
        .into_string()
        .map_err(|value| format!("{flag} value must be UTF-8, got {value:?}"))
}

fn print_usage() {
    println!(
        "usage: precompile_blockpy --module <module.blockpy.rkyv> --module-name <name> --counters <profile.bin> --out <module.o>"
    );
}
