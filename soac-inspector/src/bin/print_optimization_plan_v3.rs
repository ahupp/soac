use anyhow::{Result, anyhow, bail};
use soac_optimization::optimization_artifacts_v3::{
    ExactIntBranchV3Artifacts, load_optimization_artifacts_v3,
};
use soac_optimization::optimization_emit_v3::{
    MechanicalFunctionEmission, MechanicalRegionEmission,
};
use soac_optimization::optimization_plan_v3::{RegionId, RegionPlan};
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Default)]
struct Args {
    plan: Option<PathBuf>,
    details: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct FormatOptions {
    details: bool,
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
    print!(
        "{}",
        format_optimization_artifacts_v3_with_options(
            &artifacts,
            FormatOptions {
                details: args.details,
            },
        )
    );
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
            "--details" => parsed.details = true,
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
    println!("usage: print_optimization_plan_v3 --plan <mod.optv3> [--details]");
}

fn format_optimization_artifacts_v3(artifacts: &ExactIntBranchV3Artifacts) -> String {
    format_optimization_artifacts_v3_with_options(artifacts, FormatOptions::default())
}

fn format_optimization_artifacts_v3_with_options(
    artifacts: &ExactIntBranchV3Artifacts,
    options: FormatOptions,
) -> String {
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
        let emitted_function = artifacts
            .emission
            .functions
            .iter()
            .find(|emitted| emitted.function == function.function.function);
        let emitted_regions = emitted_function
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
            "  regions={} emitted_regions={} scalar_threads={} direct_calls={} deopt_points={} ownership_actions={} diagnostics={}\n",
            function.regions.len(),
            emitted_regions,
            function.scalar_threads.len(),
            function.direct_calls.len(),
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
            if options.details {
                format_region_details(&mut out, region, emitted_function);
            }
        }
        for thread in &function.scalar_threads {
            out.push_str(&format!(
                "  scalar_thread local={} producer={:?} consumer={:?} fallback={:?}\n",
                thread.local.name, thread.producer, thread.consumer, thread.fallback
            ));
        }
        for direct_call in &function.direct_calls {
            out.push_str(&format!(
                "  direct_call source={} target={} reason={}\n",
                direct_call.source, direct_call.target, direct_call.reason
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

fn format_region_details(
    out: &mut String,
    region: &RegionPlan,
    emitted_function: Option<&MechanicalFunctionEmission>,
) {
    out.push_str(&format!("    source={:?}\n", region.source));
    for input in &region.inputs {
        out.push_str(&format!(
            "    input {:?}: {:?} <- {:?}\n",
            input.value.id, input.value.rep, input.source
        ));
    }
    for node in &region.nodes {
        out.push_str(&format!(
            "    node {:?} source={:?}: {:?}\n",
            node.id, node.source, node.kind
        ));
    }
    for exit in &region.exits {
        out.push_str(&format!(
            "    exit source={:?}: {:?}\n",
            exit.source, exit.kind
        ));
    }
    if let Some(emitted_region) = emitted_region_for_plan_region(emitted_function, region.id) {
        out.push_str("    emitted:\n");
        for step in &emitted_region.steps {
            out.push_str(&format!(
                "      step {:?} source={:?}: {:?}\n",
                step.node, step.source, step.op
            ));
        }
        for exit in &emitted_region.exits {
            out.push_str(&format!(
                "      exit source={:?}: {:?}\n",
                exit.source, exit.kind
            ));
        }
    }
}

fn emitted_region_for_plan_region(
    emitted_function: Option<&MechanicalFunctionEmission>,
    region: RegionId,
) -> Option<&MechanicalRegionEmission> {
    emitted_function?
        .regions
        .iter()
        .find(|emitted| emitted.region == region)
}

#[cfg(test)]
mod test {
    use super::*;
    use soac_core::block_py::{LocalFunctionId, SerializedFunctionId, SerializedModuleId};
    use soac_optimization::optimization_emit_v3::{
        MechanicalFunctionEmission, MechanicalModuleEmission,
    };
    use soac_optimization::optimization_plan_v3::{
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
                    scalar_threads: Vec::new(),
                    direct_calls: Vec::new(),
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
        assert!(formatted.contains("regions=0 emitted_regions=0 scalar_threads=0 direct_calls=0"));
    }
}
