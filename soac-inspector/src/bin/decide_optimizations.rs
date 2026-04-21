use anyhow::{Result, anyhow, bail};
use soac_opt::pipeline_v3::generate_optimization_plans_v3_for_cached_modules;
use soac_opt::plan::{
    CachedModuleOptimizationInput, ProfileEvidenceStore, cached_module_paths_under_root,
    generate_optimization_plans_for_cached_modules,
};
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
struct Args {
    counters: Option<PathBuf>,
    modules: Vec<PathBuf>,
    module_root: Option<PathBuf>,
    out: Option<PathBuf>,
    mode: OptimizationMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OptimizationMode {
    #[default]
    Legacy,
    V3,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("decide_optimizations: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    run_with_args(env::args_os().skip(1))
}

fn run_with_args(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let args = parse_args(args)?;
    let counters_path = args
        .counters
        .as_deref()
        .ok_or_else(|| anyhow!("missing required --counters <profile.bin>"))?;
    let out_root = args
        .out
        .as_deref()
        .ok_or_else(|| anyhow!("missing required --out <root-dir>"))?;

    let evidence_store = ProfileEvidenceStore::from_counter_dump(counters_path)?;
    let module_inputs = module_inputs_for_args(&args, out_root)?;

    let summary = match args.mode {
        OptimizationMode::Legacy => generate_optimization_plans_for_cached_modules(
            &evidence_store,
            module_inputs,
            out_root,
        )?,
        OptimizationMode::V3 => generate_optimization_plans_v3_for_cached_modules(
            &evidence_store,
            module_inputs,
            out_root,
        )?,
    };
    for report in &summary.reports {
        println!(
            "wrote {} for module={} source_hash=0x{:016x} ({} functions)",
            report.output_path.display(),
            report.module_name,
            report.source_hash,
            report.function_count
        );
    }
    println!(
        "optimization decisions: wrote {} module plan(s), skipped {} module(s)",
        summary.written(),
        summary.skipped
    );
    Ok(())
}

fn module_inputs_for_args(
    args: &Args,
    out_root: &Path,
) -> Result<Vec<CachedModuleOptimizationInput>> {
    let mut module_inputs = BTreeMap::<PathBuf, bool>::new();
    for module_path in &args.modules {
        module_inputs.insert(module_path.clone(), true);
    }

    let should_scan_root = args.module_root.is_some() || args.modules.is_empty();
    if should_scan_root {
        let root = args.module_root.as_deref().unwrap_or(out_root);
        let root_modules = cached_module_paths_under_root(root)?;
        if root_modules.is_empty() && args.modules.is_empty() {
            bail!("no cached BlockPy modules found under {}", root.display());
        }
        for module_path in root_modules {
            module_inputs.entry(module_path).or_insert(false);
        }
    }

    if module_inputs.is_empty() {
        bail!(
            "missing module input: pass --module <mod.blockpy> or use an output root containing cached modules"
        );
    }
    Ok(module_inputs
        .into_iter()
        .map(|(module_path, strict)| CachedModuleOptimizationInput::new(module_path, strict))
        .collect())
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let flag = arg
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF-8 argument is unsupported: {arg:?}"))?;
        match flag {
            "--counters" => parsed.counters = Some(next_path(&mut args, flag)?),
            "--module" => parsed.modules.push(next_path(&mut args, flag)?),
            "--module-root" => parsed.module_root = Some(next_path(&mut args, flag)?),
            "--mode" => parsed.mode = next_mode(&mut args, flag)?,
            "--out" => parsed.out = Some(next_path(&mut args, flag)?),
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

fn next_mode(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<OptimizationMode> {
    let mode = args
        .next()
        .ok_or_else(|| anyhow!("{flag} requires a value"))?;
    let mode = mode
        .to_str()
        .ok_or_else(|| anyhow!("non-UTF-8 {flag} value is unsupported: {mode:?}"))?;
    match mode {
        "legacy" => Ok(OptimizationMode::Legacy),
        "v3" => Ok(OptimizationMode::V3),
        _ => bail!("{flag} must be 'legacy' or 'v3', got {mode:?}"),
    }
}

fn print_usage() {
    println!(
        "usage: decide_optimizations --counters <profile.bin> [--mode legacy|v3] [--module <mod.blockpy> ...] [--module-root <root-dir>] --out <root-dir>\n\nBy default, scans <root-dir> for cached mod.blockpy files and writes sibling mod.opt files. Use --mode v3 to write sibling mod.optv3 files from raw profile evidence and cached unoptimized BlockPy modules. Use --module-root to scan a different input root, or --module for narrow debugging."
    );
}

#[cfg(test)]
mod test {
    use super::*;
    use soac_core::block_py::{BlockLabel, InstrId, ModuleNameGen, RuntimeFunctionId};
    use soac_driver::codegen_cache::{
        CachedCodegenModuleMetadata, PythonModuleCacheSource, codegen_module_cache_path,
        module_optimization_plan_path, module_optimization_plan_v3_path,
    };
    use soac_driver::{LoweringOptions, lower_python_to_blockpy_recorded_with_options};
    use soac_jit::module_type::hash_module_source;
    use soac_opt::artifacts_v3::load_optimization_artifacts_v3;
    use soac_opt::plan::{OptimizationPlan, PlannedAction, PlannedReplacement};
    use soac_profile::{
        CounterDumpRecord, CounterDumpRow, CounterDumpTypeKey, CounterDumpTypeKeyLayout,
        CounterDumpTypeTableEntry,
    };
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_BUILD_IDENTITY: &str = "test-build-identity";

    #[test]
    fn emits_mod_opt_under_requested_root() {
        let root = unique_temp_dir();
        let module_name = "pkg.mod";
        let source = "def f(obj):\n    return obj.x\n";
        let source_hash = hash_module_source(source);
        let metadata = CachedCodegenModuleMetadata {
            source: PythonModuleCacheSource::Project,
            module_name: module_name.to_string(),
            source_hash,
            cache_identity: TEST_BUILD_IDENTITY.to_string(),
        };
        let module_cache_root = root.join("modules-in");
        let module_cache_path = codegen_module_cache_path(
            module_cache_root.as_path(),
            metadata.source,
            metadata.module_name.as_str(),
        )
        .unwrap();
        let lowered = lower_python_to_blockpy_recorded_with_options(
            source,
            ModuleNameGen::new(7),
            LoweringOptions {
                runtime_names_as_globals: false,
                pre_optimization_cache_path: Some(module_cache_path.clone()),
                pre_optimization_cache_metadata: Some(metadata.clone()),
            },
        )
        .unwrap();
        let function_id = lowered
            .codegen_module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .map(|function| function.function_id)
            .expect("test module should contain f");
        let instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let counters_path = root.join("profile.bin");
        let mut counters = counter_record(module_name, source_hash, function_id, instr_id)
            .encode()
            .unwrap();
        counters.extend_from_slice(
            counter_record_for_module_identity(
                "pkg.callee",
                0x5678,
                RuntimeFunctionId::from_raw_parts(8, 99),
            )
            .encode()
            .unwrap()
            .as_slice(),
        );
        fs::write(counters_path.as_path(), counters).unwrap();
        let out_root = root.join("modules-out");

        run_with_args([
            OsString::from("--counters"),
            counters_path.into_os_string(),
            OsString::from("--module"),
            module_cache_path.into_os_string(),
            OsString::from("--out"),
            out_root.clone().into_os_string(),
        ])
        .unwrap();

        let output_path = module_optimization_plan_path(
            out_root.as_path(),
            PythonModuleCacheSource::Project,
            module_name,
        )
        .unwrap();
        let bytes = fs::read(output_path.as_path()).unwrap();
        let plan =
            rkyv::from_bytes::<OptimizationPlan, rkyv::rancor::Error>(bytes.as_slice()).unwrap();
        assert_eq!(plan.module_name, module_name);
        assert_eq!(plan.source_hash, source_hash);
        assert_eq!(plan.identity_tables.modules[0].module_name, module_name);
        assert_eq!(plan.identity_tables.modules[0].source_hash, source_hash);
        assert_eq!(plan.functions.len(), 1);
        assert_eq!(
            plan.functions[0].local_function_id(),
            function_id.local_function_id()
        );
        assert_eq!(plan.functions[0].decisions.len(), 3);
        assert!(matches!(
            plan.functions[0].decisions[0].replacement,
            PlannedReplacement::Guarded { .. }
        ));
        assert!(
            plan.functions[0].decisions.iter().any(|decision| {
                let PlannedReplacement::Guarded { alternatives, .. } = &decision.replacement else {
                    return false;
                };
                alternatives.iter().any(|alternative| {
                    let PlannedAction::DirectCall { ref target } = alternative.action else {
                        return false;
                    };
                    let target = plan
                        .persistent_function_id(target.function)
                        .expect("direct-call target should resolve through plan identity table");
                    target.module.module_name == "pkg.callee"
                        && target.module.source_hash == 0x5678
                        && target.local.as_u32() == 2
                })
            }),
            "expected a cross-module direct-call decision"
        );
        assert!(
            plan.functions[0].decisions.iter().any(|decision| {
                let PlannedReplacement::Guarded { alternatives, .. } = &decision.replacement else {
                    return false;
                };
                alternatives.iter().any(|alternative| {
                    matches!(
                        alternative.action,
                        PlannedAction::IndexedField { ref specialization }
                            if specialization.attr_name == "x"
                                && specialization.expected_index == 2
                                && specialization.owner_type.module_name == "pkg.types"
                                && specialization.owner_type.qualname == "Point"
                    )
                })
            }),
            "expected a per-instruction indexed-field decision"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn default_scan_writes_all_counter_referenced_cached_modules() {
        let root = unique_temp_dir();
        let module_cache_root = root.join("modules");
        let out_root = module_cache_root.clone();

        let first = store_test_module(
            module_cache_root.as_path(),
            "pkg.first",
            "def f(obj):\n    return obj.x\n",
            10,
        );
        let second = store_test_module(
            module_cache_root.as_path(),
            "pkg.second",
            "def g(obj):\n    return obj.x\n",
            11,
        );
        let _unused = store_test_module(
            module_cache_root.as_path(),
            "pkg.unused",
            "def h(obj):\n    return obj.x\n",
            12,
        );
        let instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let mut counters =
            counter_record("pkg.first", first.source_hash, first.function_id, instr_id)
                .encode()
                .unwrap();
        counters.extend_from_slice(
            counter_record(
                "pkg.second",
                second.source_hash,
                second.function_id,
                instr_id,
            )
            .encode()
            .unwrap()
            .as_slice(),
        );
        let counters_path = root.join("profile.bin");
        fs::write(counters_path.as_path(), counters).unwrap();

        run_with_args([
            OsString::from("--counters"),
            counters_path.into_os_string(),
            OsString::from("--out"),
            out_root.clone().into_os_string(),
        ])
        .unwrap();

        for module_name in ["pkg.first", "pkg.second"] {
            let output_path = module_optimization_plan_path(
                out_root.as_path(),
                PythonModuleCacheSource::Project,
                module_name,
            )
            .unwrap();
            assert!(
                output_path.exists(),
                "expected optimization plan for {module_name} at {}",
                output_path.display()
            );
        }
        let unused_path = module_optimization_plan_path(
            out_root.as_path(),
            PythonModuleCacheSource::Project,
            "pkg.unused",
        )
        .unwrap();
        assert!(
            !unused_path.exists(),
            "module-root mode should skip cached modules absent from the counter dump"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mode_v3_emits_mod_optv3_under_requested_root() {
        let root = unique_temp_dir();
        let module_name = "pkg.modv3";
        let module = store_test_module(
            root.join("modules-in").as_path(),
            module_name,
            "def f(a, b):\n    return a + b\n",
            17,
        );
        let instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let counters = counter_record(
            module_name,
            module.source_hash,
            module.function_id,
            instr_id,
        )
        .encode()
        .unwrap();
        let counters_path = root.join("profile.bin");
        fs::write(counters_path.as_path(), counters).unwrap();
        let out_root = root.join("modules-out");
        let module_cache_path = codegen_module_cache_path(
            root.join("modules-in").as_path(),
            PythonModuleCacheSource::Project,
            module_name,
        )
        .unwrap();

        run_with_args([
            OsString::from("--counters"),
            counters_path.into_os_string(),
            OsString::from("--mode"),
            OsString::from("v3"),
            OsString::from("--module"),
            module_cache_path.into_os_string(),
            OsString::from("--out"),
            out_root.clone().into_os_string(),
        ])
        .unwrap();

        let output_path = module_optimization_plan_v3_path(
            out_root.as_path(),
            PythonModuleCacheSource::Project,
            module_name,
        )
        .unwrap();
        let artifacts = load_optimization_artifacts_v3(output_path.as_path()).unwrap();
        assert_eq!(artifacts.plan.module.module_name, module_name);
        assert_eq!(artifacts.plan.module.source_hash, module.source_hash);
        assert_eq!(artifacts.emission.module_name, module_name);
        assert!(
            artifacts.plan.functions.iter().any(|function| {
                function.function.function.local_function_id()
                    == module.function_id.local_function_id()
            }),
            "v3 plan should include the profiled function"
        );

        let legacy_path = module_optimization_plan_path(
            out_root.as_path(),
            PythonModuleCacheSource::Project,
            module_name,
        )
        .unwrap();
        assert!(
            !legacy_path.exists(),
            "v3 mode should not write the legacy optimization plan"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn module_root_can_scan_different_input_root() {
        let root = unique_temp_dir();
        let module_cache_root = root.join("modules-in");
        let out_root = root.join("modules-out");

        let module = store_test_module(
            module_cache_root.as_path(),
            "pkg.scanned",
            "def f(obj):\n    return obj.x\n",
            13,
        );
        let instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let counters = counter_record(
            "pkg.scanned",
            module.source_hash,
            module.function_id,
            instr_id,
        )
        .encode()
        .unwrap();
        let counters_path = root.join("profile.bin");
        fs::write(counters_path.as_path(), counters).unwrap();

        run_with_args([
            OsString::from("--counters"),
            counters_path.into_os_string(),
            OsString::from("--module-root"),
            module_cache_root.clone().into_os_string(),
            OsString::from("--out"),
            out_root.clone().into_os_string(),
        ])
        .unwrap();

        let output_path = module_optimization_plan_path(
            out_root.as_path(),
            PythonModuleCacheSource::Project,
            "pkg.scanned",
        )
        .unwrap();
        assert!(
            output_path.exists(),
            "expected optimization plan for pkg.scanned at {}",
            output_path.display()
        );
        let _ = fs::remove_dir_all(root);
    }

    struct StoredTestModule {
        source_hash: u64,
        function_id: RuntimeFunctionId,
    }

    fn store_test_module(
        module_cache_root: &Path,
        module_name: &str,
        source: &str,
        module_id: u32,
    ) -> StoredTestModule {
        let source_hash = hash_module_source(source);
        let metadata = CachedCodegenModuleMetadata {
            source: PythonModuleCacheSource::Project,
            module_name: module_name.to_string(),
            source_hash,
            cache_identity: TEST_BUILD_IDENTITY.to_string(),
        };
        let module_cache_path = codegen_module_cache_path(
            module_cache_root,
            metadata.source,
            metadata.module_name.as_str(),
        )
        .unwrap();
        let lowered = lower_python_to_blockpy_recorded_with_options(
            source,
            ModuleNameGen::new(module_id),
            LoweringOptions {
                runtime_names_as_globals: false,
                pre_optimization_cache_path: Some(module_cache_path),
                pre_optimization_cache_metadata: Some(metadata),
            },
        )
        .unwrap();
        let function_id = lowered
            .codegen_module
            .callable_defs
            .first()
            .map(|function| function.function_id)
            .expect("test module should contain one function");
        StoredTestModule {
            source_hash,
            function_id,
        }
    }

    fn counter_record(
        module_name: &str,
        source_hash: u64,
        function_id: RuntimeFunctionId,
        instr_id: InstrId,
    ) -> CounterDumpRecord {
        CounterDumpRecord {
            source_hash,
            module_name: module_name.to_string(),
            package_name: None,
            rows: vec![
                CounterDumpRow {
                    counter_id: 0,
                    scope: "function".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "call_hot_targets".to_string(),
                    function_id: Some(function_id),
                    current_function_id: Some(function_id),
                    instr_id: Some(instr_id),
                    function_qualname: Some("f".to_string()),
                    block_label: Some("bb0".to_string()),
                    value: 1,
                    observed_value: Some(
                        RuntimeFunctionId::from_raw_parts(8, 2).to_packed_runtime_u64(),
                    ),
                    max_overcount: None,
                },
                CounterDumpRow {
                    counter_id: 1,
                    scope: "function".to_string(),
                    kind: "operator_hot_shapes".to_string(),
                    site_kind: "operator_hot_shapes".to_string(),
                    function_id: Some(function_id),
                    current_function_id: Some(function_id),
                    instr_id: Some(instr_id),
                    function_qualname: Some("f".to_string()),
                    block_label: Some("bb0".to_string()),
                    value: 1,
                    observed_value: Some(257),
                    max_overcount: None,
                },
            ],
            module_keys: Vec::new(),
            type_keys: vec![CounterDumpTypeKeyLayout {
                owner_type_id: 44,
                key: "x".to_string(),
                index: 2,
            }],
            type_table: vec![CounterDumpTypeTableEntry {
                type_id: 44,
                key: CounterDumpTypeKey {
                    module_name: "pkg.types".to_string(),
                    qualname: "Point".to_string(),
                },
            }],
        }
    }

    fn counter_record_for_module_identity(
        module_name: &str,
        source_hash: u64,
        function_id: RuntimeFunctionId,
    ) -> CounterDumpRecord {
        CounterDumpRecord {
            source_hash,
            module_name: module_name.to_string(),
            package_name: None,
            rows: vec![CounterDumpRow {
                counter_id: 0,
                scope: "function".to_string(),
                kind: "operator_hot_shapes".to_string(),
                site_kind: "operator_hot_shapes".to_string(),
                function_id: Some(function_id),
                current_function_id: Some(function_id),
                instr_id: Some(InstrId::new(BlockLabel::from_index(0), 0)),
                function_qualname: Some("unrelated".to_string()),
                block_label: Some("bb0".to_string()),
                value: 1,
                observed_value: Some(257),
                max_overcount: None,
            }],
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        }
    }

    fn unique_temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac-decide-optimizations-test-{}-{unique}",
            std::process::id()
        ))
    }
}
