use soac_core::block_py::RuntimeFunctionId;
use soac_inspector::{
    JitClifRenderOptions, lower_source_to_blockpy_module,
    lower_source_to_blockpy_module_with_module_id, profile_module_identity_from_env,
    render_instr_typed_for_module_with_options,
};
use std::fs;
use std::path::{Path, PathBuf};

const VALIDATE_DELIMITER: &str = "# diet-python: validate";

struct Args {
    source: PathBuf,
    function_id: RuntimeFunctionId,
    module_name: Option<String>,
    specialized: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut positionals = Vec::new();
    let mut module_name = None;
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
        .map(RuntimeFunctionId::from_packed_runtime_u64)
        .map_err(|err| format!("invalid function_id '{}': {err}", positionals[1]))?;
    Ok(Args {
        source: PathBuf::from(&positionals[0]),
        function_id,
        module_name,
        specialized,
    })
}

fn print_usage() {
    eprintln!(
        "usage: render_instr_typed <source> <function_id> [--module-name NAME] [--specialized]"
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
    soac_config::init_logging()?;
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
            .unwrap_or("render_instr_typed")
            .to_string()
    });
    let profile_module_identity = if args.specialized {
        profile_module_identity_from_env(&module_name)?
    } else {
        None
    };
    let profile_module_id = profile_module_identity.map(|identity| identity.module_id);
    let function_id = profile_module_id
        .filter(|_| args.function_id.runtime_module_id().as_u32() == 0)
        .map(|module_id| {
            RuntimeFunctionId::from_raw_parts(
                module_id,
                args.function_id.local_function_id().as_u32(),
            )
        })
        .unwrap_or(args.function_id);
    let module = if let Some(module_id) = profile_module_id {
        lower_source_to_blockpy_module_with_module_id(&source, module_id)?
    } else {
        lower_source_to_blockpy_module(&source)?
    };

    let rendered = render_instr_typed_for_module_with_options(
        &soac_inspector::repo_root(),
        &module_name,
        &module,
        function_id,
        JitClifRenderOptions {
            load_runtime_specializations: args.specialized,
            runtime_source_path: args.specialized.then_some(source_path.clone()),
            module_source_hash: profile_module_identity.map(|identity| identity.source_hash),
        },
    )?;
    print!("{}", rendered.instr_typed);
    if !rendered.instr_typed.ends_with('\n') {
        println!();
    }
    Ok(())
}
