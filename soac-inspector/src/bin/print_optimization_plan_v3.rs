use anyhow::{Result, anyhow, bail};
use soac_jit::optimization_pipeline_v3::{
    ExactIntBranchV3Artifacts, load_optimization_artifacts_v3,
};
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Default)]
struct Args {
    plan: Option<PathBuf>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("print_optimization_plan_v3: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    run_with_args(env::args_os().skip(1))
}

fn run_with_args(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let args = parse_args(args)?;
    let path = args
        .plan
        .as_deref()
        .ok_or_else(|| anyhow!("missing required --plan <mod.optv3>"))?;
    let artifacts = load_optimization_artifacts_v3(path)?;
    print!("{}", format_optimization_artifacts_v3(&artifacts));
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
            "--plan" => parsed.plan = Some(next_path(&mut args, flag)?),
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ => bail!("unknown argument {flag:?}"),
        }
    }
    Ok(parsed)
}

fn next_path(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<PathBuf> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn print_usage() {
    println!("usage: print_optimization_plan_v3 --plan <mod.optv3>");
}

fn format_optimization_artifacts_v3(artifacts: &ExactIntBranchV3Artifacts) -> String {
    let mut out = String::new();
    let plan = &artifacts.plan;
    out.push_str(&format!(
        "module {} source_hash=0x{:016x} cache_identity={}\n",
        plan.module.module_name, plan.module.source_hash, plan.module.cache_identity
    ));
    out.push_str(&format!(
        "helper_catalog_version={} cost_model_version={}\n",
        plan.helper_catalog_version, plan.cost_model_version
    ));
    out.push_str(&format!(
        "functions={} emitted_functions={}\n",
        plan.functions.len(),
        artifacts.emission.functions.len()
    ));
    for function in &plan.functions {
        let emitted_regions = artifacts
            .emission
            .functions
            .iter()
            .find(|emitted| emitted.function == function.function.function)
            .map(|emitted| emitted.regions.len())
            .unwrap_or(0);
        out.push_str(&format!(
            "\nfunction {}",
            function
                .function
                .debug_name
                .as_deref()
                .unwrap_or("<unknown>")
        ));
        out.push_str(&format!(" id={}\n", function.function.function));
        out.push_str(&format!(
            "  regions={} emitted_regions={} deopt_points={} ownership_actions={} diagnostics={}\n",
            function.regions.len(),
            emitted_regions,
            function.deopt_points.len(),
            function.ownership.actions.len(),
            function.diagnostics.len()
        ));
        for region in &function.regions {
            out.push_str(&format!(
                "  region {:?}: inputs={} nodes={} exits={}\n",
                region.id,
                region.inputs.len(),
                region.nodes.len(),
                region.exits.len()
            ));
        }
        for diagnostic in &function.diagnostics {
            out.push_str(&format!(
                "  diagnostic {:?}: {}\n",
                diagnostic.source, diagnostic.message
            ));
        }
    }
    out
}

#[cfg(test)]
mod test {
    use super::*;
    use soac_core::block_py::{LocalFunctionId, SerializedFunctionId, SerializedModuleId};
    use soac_jit::optimization_emit_v3::{MechanicalFunctionEmission, MechanicalModuleEmission};
    use soac_jit::optimization_plan_v3::{
        FunctionOptimizationPlanV3, FunctionOwnershipPlan, FunctionPlanIdentity,
        ModuleOptimizationPlanV3, ModulePlanIdentity,
    };

    #[test]
    fn formats_module_identity_and_function_counts() {
        let function =
            SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(1));
        let artifacts = ExactIntBranchV3Artifacts {
            plan: ModuleOptimizationPlanV3 {
                module: ModulePlanIdentity {
                    module_name: "pkg.mod".to_string(),
                    source_hash: 0x1234,
                    cache_identity: "test-cache".to_string(),
                },
                helper_catalog_version: 1,
                cost_model_version: 1,
                functions: vec![FunctionOptimizationPlanV3 {
                    function: FunctionPlanIdentity {
                        function,
                        debug_name: Some("f".to_string()),
                    },
                    regions: Vec::new(),
                    deopt_points: Vec::new(),
                    ownership: FunctionOwnershipPlan::default(),
                    diagnostics: Vec::new(),
                }],
            },
            emission: MechanicalModuleEmission {
                module_name: "pkg.mod".to_string(),
                functions: vec![MechanicalFunctionEmission {
                    function,
                    debug_name: Some("f".to_string()),
                    regions: Vec::new(),
                }],
            },
        };

        let formatted = format_optimization_artifacts_v3(&artifacts);
        assert!(formatted.contains("module pkg.mod source_hash=0x0000000000001234"));
        assert!(formatted.contains("function f"));
        assert!(formatted.contains("regions=0 emitted_regions=0"));
    }
}
