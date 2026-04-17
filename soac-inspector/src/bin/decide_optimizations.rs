use anyhow::{Context, Result, anyhow, bail};
use soac_blockpy::codegen_cache::{
    CachedCodegenModule, load_codegen_module_cache, module_optimization_plan_path,
};
use soac_jit::optimization_plan::{OptimizationPlan, ProfileEvidenceStore};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
struct Args {
    counters: Option<PathBuf>,
    module: Option<PathBuf>,
    out: Option<PathBuf>,
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
        .ok_or_else(|| anyhow!("missing required --counters <profile.bin>"))?;
    let module_path = args
        .module
        .ok_or_else(|| anyhow!("missing required --module <mod.blockpy>"))?;
    let out_root = args
        .out
        .ok_or_else(|| anyhow!("missing required --out <root-dir>"))?;

    let evidence_store = ProfileEvidenceStore::from_counter_dump(counters_path.as_path())?;
    let cache = load_codegen_module_cache(module_path.as_path())
        .with_context(|| format!("load BlockPy module cache {}", module_path.display()))?;
    validate_counter_evidence_matches_module(&evidence_store, &cache)?;
    let plan = OptimizationPlan::from_evidence(&cache.metadata, &cache.module, &evidence_store);
    let output_path = module_optimization_plan_path(
        out_root.as_path(),
        cache.metadata.source,
        cache.metadata.module_name.as_str(),
    )
    .with_context(|| {
        format!(
            "construct optimization plan output path for module {}",
            cache.metadata.module_name
        )
    })?;
    write_optimization_plan(output_path.as_path(), &plan)?;
    println!(
        "wrote {} for module={} source_hash=0x{:016x} ({} functions)",
        output_path.display(),
        plan.module_name,
        plan.source_hash,
        plan.functions.len()
    );
    Ok(())
}

fn validate_counter_evidence_matches_module(
    evidence_store: &ProfileEvidenceStore,
    cache: &CachedCodegenModule,
) -> Result<()> {
    match evidence_store.module_source_hash(cache.metadata.module_name.as_str()) {
        Some(source_hash) if source_hash == cache.metadata.source_hash => Ok(()),
        Some(source_hash) => bail!(
            "counter dump source hash for module {} is 0x{source_hash:016x}, but cached BlockPy module has 0x{:016x}",
            cache.metadata.module_name,
            cache.metadata.source_hash
        ),
        None => bail!(
            "counter dump does not contain module {}",
            cache.metadata.module_name
        ),
    }
}

fn write_optimization_plan(path: &Path, plan: &OptimizationPlan) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create optimization plan dir {}", parent.display()))?;
    }
    let archive = rkyv::to_bytes::<rkyv::rancor::Error>(plan)
        .map_err(|err| anyhow!("serialize optimization plan: {err}"))?;
    let temp_path = path.with_extension("opt.tmp");
    {
        let mut temp_file = File::create(temp_path.as_path()).with_context(|| {
            format!("create temporary optimization plan {}", temp_path.display())
        })?;
        temp_file
            .write_all(archive.as_ref())
            .with_context(|| format!("write optimization plan {}", temp_path.display()))?;
    }
    fs::rename(temp_path.as_path(), path).with_context(|| {
        format!(
            "publish optimization plan {} -> {}",
            temp_path.display(),
            path.display()
        )
    })?;
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
            "--counters" => parsed.counters = Some(next_path(&mut args, flag)?),
            "--module" => parsed.module = Some(next_path(&mut args, flag)?),
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

fn print_usage() {
    println!(
        "usage: decide_optimizations --counters <profile.bin> --module <mod.blockpy> --out <root-dir>"
    );
}

#[cfg(test)]
mod test {
    use super::*;
    use soac_blockpy::block_py::{BlockLabel, FunctionId, InstrId, ModuleNameGen};
    use soac_blockpy::codegen_cache::{
        CachedCodegenModuleMetadata, PythonModuleCacheSource, codegen_module_cache_path,
    };
    use soac_blockpy::{LoweringOptions, lower_python_to_blockpy_recorded_with_options};
    use soac_jit::counter_dump::{
        CounterDumpRecord, CounterDumpRow, CounterDumpTypeKey, CounterDumpTypeKeyLayout,
        CounterDumpTypeTableEntry,
    };
    use soac_jit::module_type::hash_module_source;
    use soac_jit::optimization_plan::{PlannedAction, PlannedReplacement};
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
        fs::write(
            counters_path.as_path(),
            counter_record(module_name, source_hash, function_id, instr_id)
                .encode()
                .unwrap(),
        )
        .unwrap();
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
        assert_eq!(plan.functions.len(), 1);
        assert_eq!(plan.functions[0].function_id, function_id);
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

    fn counter_record(
        module_name: &str,
        source_hash: u64,
        function_id: FunctionId,
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
                    observed_value: Some(FunctionId::new(7, 2).packed()),
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
