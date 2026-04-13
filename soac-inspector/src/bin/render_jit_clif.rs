use soac_blockpy::block_py::FunctionId;
use soac_inspector::{
    JitClifRenderOptions, jit_debug_plan, lower_source_to_codegen_module,
    lower_source_to_codegen_module_with_module_id, profile_module_id_from_env,
    render_jit_clif_for_module_with_options,
};
use std::fs;
use std::path::{Path, PathBuf};

const VALIDATE_DELIMITER: &str = "# diet-python: validate";

struct Args {
    source: PathBuf,
    function_id: FunctionId,
    module_name: Option<String>,
    cfg_dot_out: Option<PathBuf>,
    vcode_out: Option<PathBuf>,
    debug_plan: bool,
    specialized: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut positionals = Vec::new();
    let mut module_name = None;
    let mut cfg_dot_out = None;
    let mut vcode_out = None;
    let mut debug_plan = false;
    let mut specialized = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--module-name" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--module-name requires a value".to_string())?;
                module_name = Some(value);
            }
            "--cfg-dot-out" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--cfg-dot-out requires a value".to_string())?;
                cfg_dot_out = Some(PathBuf::from(value));
            }
            "--vcode-out" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--vcode-out requires a value".to_string())?;
                vcode_out = Some(PathBuf::from(value));
            }
            "--debug-plan" => {
                debug_plan = true;
            }
            "--specialized" => {
                specialized = true;
            }
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
    if positionals.len() != 2 {
        return Err("expected <source> and <function_id>".to_string());
    }
    let function_id = positionals[1]
        .parse::<u64>()
        .map(FunctionId::from_packed)
        .map_err(|err| format!("invalid function_id '{}': {err}", positionals[1]))?;
    Ok(Args {
        source: PathBuf::from(&positionals[0]),
        function_id,
        module_name,
        cfg_dot_out,
        vcode_out,
        debug_plan,
        specialized,
    })
}

fn print_usage() {
    eprintln!(
        "usage: render_jit_clif <source> <function_id> [--module-name NAME] [--cfg-dot-out PATH] [--vcode-out PATH] [--debug-plan] [--specialized]"
    );
    eprintln!(
        "       --specialized renders the second-pass shape using SOAC_WORK_DIR + SOAC_OPT_MODE=apply"
    );
}

fn split_source(path: &Path) -> Result<String, String> {
    let source = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let source = source
        .split_once(VALIDATE_DELIMITER)
        .map(|(before, _)| before)
        .unwrap_or(source.as_str());
    Ok(format!("{}\n", source.trim_end()))
}

fn main() -> Result<(), String> {
    soac_blockpy::init_logging();
    let args = parse_args().inspect_err(|_| print_usage())?;
    let source_path = args.source.canonicalize().map_err(|err| {
        format!(
            "failed to resolve source path {}: {err}",
            args.source.display()
        )
    })?;
    let source = split_source(&source_path)?;
    let module_name = args.module_name.unwrap_or_else(|| {
        source_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("render_jit_clif")
            .to_string()
    });
    let profile_module_id = if args.specialized {
        profile_module_id_from_env(&module_name)?
    } else {
        None
    };
    let function_id = profile_module_id
        .filter(|_| args.function_id.module_id() == 0)
        .map(|module_id| FunctionId::new(module_id, args.function_id.function_id()))
        .unwrap_or(args.function_id);
    let module = if let Some(module_id) = profile_module_id {
        lower_source_to_codegen_module_with_module_id(&source, module_id)?
    } else {
        lower_source_to_codegen_module(&source)?
    };

    if args.debug_plan {
        eprintln!("{}", jit_debug_plan(&module_name, &module, function_id)?);
    }

    let rendered = render_jit_clif_for_module_with_options(
        &soac_inspector::repo_root(),
        &module_name,
        &module,
        function_id,
        JitClifRenderOptions {
            load_runtime_specializations: args.specialized,
            runtime_source_path: args.specialized.then_some(source_path.clone()),
        },
    )?;
    if let Some(path) = args.cfg_dot_out {
        fs::write(&path, rendered.cfg_dot.as_bytes())
            .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    }
    if let Some(path) = args.vcode_out {
        fs::write(&path, rendered.vcode_disasm.as_bytes())
            .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    }
    print!("{}", rendered.clif);
    if !rendered.clif.ends_with('\n') {
        println!();
    }
    Ok(())
}
