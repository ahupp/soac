use soac_core::block_py::blockpy_module_to_string;
use soac_driver::codegen_cache::load_codegen_module_cache;
use std::path::PathBuf;

struct Args {
    path: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut positionals = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
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
        return Err("expected <mod.blockpy>".to_string());
    }
    Ok(Args {
        path: PathBuf::from(&positionals[0]),
    })
}

fn print_usage() {
    eprintln!("usage: print_codegen_module_cache <mod.blockpy>");
}

fn main() -> Result<(), String> {
    let args = parse_args().inspect_err(|_| print_usage())?;
    let cache = load_codegen_module_cache(args.path.as_path()).map_err(|err| err.to_string())?;
    println!(
        "# metadata source={:?} module={} source_hash=0x{:016x} cache_identity={}",
        cache.metadata.source,
        cache.metadata.module_name,
        cache.metadata.source_hash,
        cache.metadata.cache_identity,
    );
    print!("{}", blockpy_module_to_string(&cache.module));
    Ok(())
}
