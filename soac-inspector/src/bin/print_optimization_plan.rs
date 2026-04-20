use anyhow::{Context, Result, anyhow, bail};
use soac_optimization::optimization_plan::{OptimizationPlan, format_optimization_plan};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Default)]
struct Args {
    plan: Option<PathBuf>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("print_optimization_plan: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args(env::args_os().skip(1))?;
    let path = args
        .plan
        .ok_or_else(|| anyhow!("missing required --plan <mod.opt>"))?;
    let bytes = fs::read(path.as_path())
        .with_context(|| format!("read optimization plan {}", path.display()))?;
    let plan = rkyv::from_bytes::<OptimizationPlan, rkyv::rancor::Error>(bytes.as_slice())
        .map_err(|err| anyhow!("deserialize optimization plan {}: {err}", path.display()))?;
    print!("{}", format_optimization_plan(&plan));
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let flag = arg
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF-8 argument is unsupported: {arg:?}"))?;
        match flag {
            "--plan" => {
                parsed.plan = Some(
                    args.next()
                        .map(PathBuf::from)
                        .ok_or_else(|| anyhow!("{flag} requires a value"))?,
                )
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ => bail!("unknown argument {flag:?}"),
        }
    }
    Ok(parsed)
}

fn print_usage() {
    println!("usage: print_optimization_plan --plan <mod.opt>");
}
