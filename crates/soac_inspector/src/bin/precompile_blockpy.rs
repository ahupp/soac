use soac_config::SoacEnvConfig;
use soac_core::block_py::{BlockPyModule, ModuleNameGen, RuntimeFunctionId};
use soac_core::profile::{CounterDumpFile, CounterDumpRecordView, CounterDumpRowView};
use soac_driver::codegen_cache::{
    CachedCodegenModuleMetadata, PythonModuleCacheSource, codegen_module_cache_path,
    hash_module_source, load_codegen_module_cache, module_optimization_plan_v3_path,
    module_optimized_codegen_v3_path, remap_cached_codegen_module_function_ids,
    validate_codegen_module_cache_metadata,
};
use soac_jit::{
    PrecompileModuleIndex, PrecompileModuleIndexEntry, PrecompileOptimizationPlanInput,
    precompile_codegen_module_to_object_file,
};
use soac_lowering::passes::CodegenModuleShape;
use std::collections::HashSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SOAC_BUILD_IDENTITY: &str = env!("SOAC_BUILD_IDENTITY");
const SOAC_RUNTIME_MODULE_NAME: &str = "soac.runtime";

#[derive(Debug, Default)]
struct Args {
    counters: Option<PathBuf>,
    module_cache_dir: Option<PathBuf>,
    build_identity: Option<String>,
    out: Option<PathBuf>,
    object_dir: Option<PathBuf>,
    linker: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct CounterModuleRef {
    module_name: String,
    source_hash: u64,
    module_id: Option<u32>,
}

#[derive(Debug)]
struct CompiledModuleObject {
    object_path: PathBuf,
    function_count: usize,
    data_object_count: usize,
    object_size_bytes: usize,
}

#[derive(Debug)]
struct LoadedModule {
    module_ref: CounterModuleRef,
    metadata: CachedCodegenModuleMetadata,
    module: BlockPyModule<CodegenModuleShape>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("precompile_blockpy: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    run_with_args(env::args_os().skip(1))
}

fn run_with_args(args: impl IntoIterator<Item = OsString>) -> Result<(), String> {
    let args = parse_args(args)?;
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
        .ok_or_else(|| "missing required --out <shared-library>".to_string())?;
    let module_cache_dir = match args.module_cache_dir {
        Some(path) => path,
        None => default_module_cache_dir()?,
    };
    let object_dir = args
        .object_dir
        .unwrap_or_else(|| default_object_dir(out_path.as_path()));
    let linker = args.linker.unwrap_or_else(|| OsString::from("cc"));

    soac_inspector::prepare_python();

    let counter_dump = CounterDumpFile::open(counters_path.as_path())?;
    let records = counter_dump.records()?;
    let mut modules = counter_modules_from_records(records.as_slice())?;
    if modules.is_empty() {
        return Err(format!(
            "counter dump {} does not reference any modules",
            counters_path.display()
        ));
    }
    include_soac_runtime_module(
        &mut modules,
        module_cache_dir.as_path(),
        args.build_identity.as_deref(),
    )?;

    fs::create_dir_all(object_dir.as_path()).map_err(|err| {
        format!(
            "failed to create precompile object dir {}: {err}",
            object_dir.display()
        )
    })?;
    if let Some(parent) = out_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create shared-library output dir {}: {err}",
                parent.display()
            )
        })?;
    }

    let mut loaded_modules = Vec::new();
    for module_ref in modules {
        let cache_path = resolve_module_cache_path(
            module_cache_dir.as_path(),
            &module_ref,
            args.build_identity.as_deref(),
        )?;
        let mut cache = load_codegen_module_cache(cache_path.as_path())
            .map_err(|err| format!("failed to load {}: {err}", cache_path.display()))?;
        if let Some(module_id) = module_ref.module_id {
            remap_cached_codegen_module_function_ids(&mut cache, ModuleNameGen::new(module_id));
        }
        let metadata = cache.metadata.clone();
        loaded_modules.push(LoadedModule {
            module_ref,
            metadata,
            module: cache.module,
        });
    }
    let module_index = PrecompileModuleIndex::from_entries(loaded_modules.iter().map(|loaded| {
        PrecompileModuleIndexEntry {
            module_name: loaded.module_ref.module_name.as_str(),
            source_hash: loaded.module_ref.source_hash,
            module: &loaded.module,
        }
    }))?;

    let mut compiled = Vec::new();
    for loaded in loaded_modules {
        let module_ref = loaded.module_ref;
        let metadata = loaded.metadata;
        let optimization_plan_v3_path = module_optimization_plan_v3_path(
            module_cache_dir.as_path(),
            metadata.source,
            module_ref.module_name.as_str(),
        )
        .map_err(|err| err.to_string())?;
        let optimized_module_v3_path = module_optimized_codegen_v3_path(
            module_cache_dir.as_path(),
            metadata.source,
            module_ref.module_name.as_str(),
        )
        .map_err(|err| err.to_string())?;
        let optimization_plan = PrecompileOptimizationPlanInput {
            v3_path: Some(optimization_plan_v3_path.as_path()),
            optimized_module_path: Some(optimized_module_v3_path.as_path()),
            cache_identity: metadata.cache_identity.as_str(),
        };

        let object_path = object_dir.join(object_file_name(&module_ref));
        let summary = precompile_codegen_module_to_object_file(
            module_ref.module_name.as_str(),
            module_ref.source_hash,
            &loaded.module,
            Some(counters_path.as_path()),
            Some(optimization_plan),
            Some(&module_index),
            object_path.as_path(),
        )?;
        println!(
            "wrote {} for module={} source_hash=0x{:016x} module_id={} ({} bytes, {} functions, {} data objects)",
            summary.output_path.display(),
            module_ref.module_name,
            module_ref.source_hash,
            module_ref
                .module_id
                .map(|module_id| module_id.to_string())
                .unwrap_or_else(|| "cached".to_string()),
            summary.object_size_bytes,
            summary.function_count,
            summary.data_object_count
        );
        compiled.push(CompiledModuleObject {
            object_path: summary.output_path,
            function_count: summary.function_count,
            data_object_count: summary.data_object_count,
            object_size_bytes: summary.object_size_bytes,
        });
    }

    link_shared_library(
        linker.as_os_str(),
        compiled
            .iter()
            .map(|compiled| compiled.object_path.as_path())
            .collect::<Vec<_>>()
            .as_slice(),
        out_path.as_path(),
    )?;
    let total_size = compiled
        .iter()
        .map(|compiled| compiled.object_size_bytes)
        .sum::<usize>();
    let total_functions = compiled
        .iter()
        .map(|compiled| compiled.function_count)
        .sum::<usize>();
    let total_data_objects = compiled
        .iter()
        .map(|compiled| compiled.data_object_count)
        .sum::<usize>();
    println!(
        "linked {} from {} objects ({} object bytes, {} functions, {} data objects)",
        out_path.display(),
        compiled.len(),
        total_size,
        total_functions,
        total_data_objects
    );
    Ok(())
}

fn counter_modules_from_records(
    records: &[CounterDumpRecordView<'_>],
) -> Result<Vec<CounterModuleRef>, String> {
    let mut modules = HashSet::new();
    for record in records {
        modules.insert(CounterModuleRef {
            module_name: record.module_name()?.to_string(),
            source_hash: record.source_hash(),
            module_id: module_id_for_record(record)?,
        });
    }
    let mut modules = modules.into_iter().collect::<Vec<_>>();
    modules.sort();
    Ok(modules)
}

fn include_soac_runtime_module(
    modules: &mut Vec<CounterModuleRef>,
    module_cache_dir: &Path,
    build_identity: Option<&str>,
) -> Result<(), String> {
    if modules
        .iter()
        .any(|module_ref| module_ref.module_name == SOAC_RUNTIME_MODULE_NAME)
    {
        return Ok(());
    }

    let source_path = soac_runtime_source_path();
    let source = fs::read_to_string(source_path.as_path()).map_err(|err| {
        format!(
            "failed to read {SOAC_RUNTIME_MODULE_NAME} source {}: {err}",
            source_path.display()
        )
    })?;
    let module_ref = CounterModuleRef {
        module_name: SOAC_RUNTIME_MODULE_NAME.to_string(),
        source_hash: hash_module_source(source.as_str()),
        module_id: None,
    };
    resolve_module_cache_path(module_cache_dir, &module_ref, build_identity).map_err(|err| {
        format!(
            "{err}; precompiled shared libraries include {SOAC_RUNTIME_MODULE_NAME}, so run a profile/import pass first to populate its module cache"
        )
    })?;
    modules.push(module_ref);
    modules.sort();
    Ok(())
}

fn soac_runtime_source_path() -> PathBuf {
    repo_root()
        .join("soac_py")
        .join("src")
        .join("soac")
        .join("runtime.py")
}

fn module_id_for_record(record: &CounterDumpRecordView<'_>) -> Result<Option<u32>, String> {
    let mut module_id = None;
    for row_index in 0..record.row_count() {
        let row = record.row(row_index)?;
        for function_id in row_function_ids(&row) {
            if function_id == RuntimeFunctionId::global() {
                continue;
            }
            match module_id {
                Some(current) if current != function_id.runtime_module_id().as_u32() => {
                    return Err(format!(
                        "counter record for module {} mixes function ids from module ids {} and {}",
                        record.module_name()?,
                        current,
                        function_id.runtime_module_id().as_u32()
                    ));
                }
                Some(_) => {}
                None => module_id = Some(function_id.runtime_module_id().as_u32()),
            }
        }
    }
    Ok(module_id)
}

fn row_function_ids(row: &CounterDumpRowView<'_>) -> impl IntoIterator<Item = RuntimeFunctionId> {
    [row.function_id, row.current_function_id]
        .into_iter()
        .flatten()
}

fn resolve_module_cache_path(
    cache_root: &Path,
    module_ref: &CounterModuleRef,
    build_identity: Option<&str>,
) -> Result<PathBuf, String> {
    let matches = matching_module_cache_paths(cache_root, module_ref, build_identity)?;
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(format!(
            "cached BlockPy module for module={} source_hash=0x{:016x}{} not found under {}",
            module_ref.module_name,
            module_ref.source_hash,
            build_identity
                .map(|identity| format!(" build_identity={identity}"))
                .unwrap_or_default(),
            cache_root.display()
        )),
        _ => Err(format!(
            "multiple cached BlockPy modules for module={} source_hash=0x{:016x}; pass --build-identity or remove stale cache entries: {}",
            module_ref.module_name,
            module_ref.source_hash,
            matches
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
fn module_cache_path_for_identity(
    cache_root: &Path,
    module_ref: &CounterModuleRef,
    build_identity: &str,
) -> Result<PathBuf, String> {
    module_cache_path_for_source(
        cache_root,
        PythonModuleCacheSource::Project,
        module_ref,
        build_identity,
    )
}

#[cfg(test)]
fn module_cache_path_for_source(
    cache_root: &Path,
    source: PythonModuleCacheSource,
    module_ref: &CounterModuleRef,
    build_identity: &str,
) -> Result<PathBuf, String> {
    soac_jit::config::pre_optimization_module_cache_path(
        cache_root,
        source,
        module_ref.module_name.as_str(),
        module_ref.source_hash,
        build_identity,
        module_ref.module_name == SOAC_RUNTIME_MODULE_NAME,
    )
}

fn module_cache_metadata_for_source(
    source: PythonModuleCacheSource,
    module_ref: &CounterModuleRef,
    build_identity: &str,
) -> CachedCodegenModuleMetadata {
    soac_jit::config::pre_optimization_module_cache_metadata(
        source,
        module_ref.module_name.as_str(),
        module_ref.source_hash,
        build_identity,
        module_ref.module_name == SOAC_RUNTIME_MODULE_NAME,
    )
}

fn matching_module_cache_paths(
    cache_root: &Path,
    module_ref: &CounterModuleRef,
    build_identity: Option<&str>,
) -> Result<Vec<PathBuf>, String> {
    let identities = build_identity
        .map(|identity| vec![identity.to_string()])
        .unwrap_or_else(|| vec![SOAC_BUILD_IDENTITY.to_string()]);
    let mut matches = Vec::new();
    for source in [
        PythonModuleCacheSource::Project,
        PythonModuleCacheSource::PythonStdlib,
    ] {
        let path = codegen_module_cache_path(cache_root, source, module_ref.module_name.as_str())
            .map_err(|err| err.to_string())?;
        if !path.exists() {
            continue;
        }
        let cache = load_codegen_module_cache(path.as_path())
            .map_err(|err| format!("failed to load {}: {err}", path.display()))?;
        for identity in &identities {
            let expected = module_cache_metadata_for_source(source, module_ref, identity.as_str());
            if validate_codegen_module_cache_metadata(&cache.metadata, &expected).is_ok() {
                matches.push(path.clone());
                break;
            }
        }
    }
    matches.sort();
    Ok(matches)
}

fn link_shared_library(
    linker: &OsStr,
    object_paths: &[&Path],
    out_path: &Path,
) -> Result<(), String> {
    if object_paths.is_empty() {
        return Err("cannot link shared library without object inputs".to_string());
    }
    let status = Command::new(linker)
        .arg("-shared")
        .arg("-o")
        .arg(out_path)
        .args(object_paths)
        .status()
        .map_err(|err| format!("failed to run linker {linker:?}: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "linker {linker:?} failed with status {status} while writing {}",
            out_path.display()
        ))
    }
}

fn default_module_cache_dir() -> Result<PathBuf, String> {
    SoacEnvConfig::from_env()?
        .module_cache_root()
        .ok_or_else(|| {
            "SOAC_WORK_DIR must be set when --module-cache-dir is omitted; \
         using SOAC_WORK_DIR/modules for cached BlockPy modules"
                .to_string()
        })
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace crate should live under crates/ in the repo root")
        .to_path_buf()
}

fn default_object_dir(out_path: &Path) -> PathBuf {
    let file_name = out_path
        .file_name()
        .map(|file_name| file_name.to_string_lossy())
        .unwrap_or_else(|| "precompiled-soac".into());
    let dir_name = format!("{file_name}.objects");
    out_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(dir_name)
}

fn object_file_name(module_ref: &CounterModuleRef) -> String {
    let module_name = sanitize_path_component(module_ref.module_name.as_str());
    let module_id = module_ref
        .module_id
        .map(|module_id| module_id.to_string())
        .unwrap_or_else(|| "cached".to_string());
    format!(
        "{module_name}-{:016x}-m{module_id}.o",
        module_ref.source_hash
    )
}

fn sanitize_path_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len().max(1));
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_') {
            out.push(char::from(byte));
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("module");
    }
    out
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let flag = arg
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 argument is unsupported: {arg:?}"))?;
        match flag {
            "--counters" => parsed.counters = Some(next_path(&mut args, flag)?),
            "--module-cache-dir" => parsed.module_cache_dir = Some(next_path(&mut args, flag)?),
            "--build-identity" => parsed.build_identity = Some(next_string(&mut args, flag)?),
            "--out" => parsed.out = Some(next_path(&mut args, flag)?),
            "--object-dir" => parsed.object_dir = Some(next_path(&mut args, flag)?),
            "--linker" => parsed.linker = Some(next_os_string(&mut args, flag)?),
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
    Ok(PathBuf::from(next_os_string(args, flag)?))
}

fn next_string(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<String, String> {
    next_os_string(args, flag)?
        .into_string()
        .map_err(|value| format!("{flag} value must be UTF-8, got {value:?}"))
}

fn next_os_string(
    args: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<OsString, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn print_usage() {
    println!(
        "usage: precompile_blockpy --counters <profile.bin> --out <libsoac_precompiled.so> [--module-cache-dir <dir>] [--build-identity <identity>] [--object-dir <dir>] [--linker <cc>]\n\nBy default, reads cached BlockPy modules from SOAC_WORK_DIR/modules."
    );
}

#[cfg(test)]
mod test {
    use super::*;
    use soac_core::pass_tracker::RecordingPassTracker;
    use soac_core::profile::{CounterDumpRecord, CounterDumpRow, parse_counter_dump_records};
    use soac_driver::codegen_cache::store_codegen_module_cache;
    use soac_driver::{
        CachedModuleOptimizationInput, CodegenPreparationOptions,
        generate_optimization_plans_v3_for_cached_modules, prepare_codegen_module,
    };
    use soac_opt::plan::ProfileEvidenceStore;
    use std::process::Stdio;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn prepare_for_test(
        source: &str,
        module_id: u32,
        options: CodegenPreparationOptions,
    ) -> BlockPyModule<CodegenModuleShape> {
        let mut pass_tracker = RecordingPassTracker::new();
        prepare_codegen_module(
            source,
            ModuleNameGen::new(module_id),
            options,
            &SoacEnvConfig::default(),
            &mut pass_tracker,
        )
        .unwrap()
    }

    fn prepare_with_cache_for_test(
        source: &str,
        module_id: u32,
        cache_root: &Path,
        cache_source: PythonModuleCacheSource,
        module_name: &str,
        runtime_names_as_globals: bool,
    ) -> BlockPyModule<CodegenModuleShape> {
        prepare_for_test(
            source,
            module_id,
            CodegenPreparationOptions::default()
                .with_runtime_names_as_globals(runtime_names_as_globals)
                .with_pre_optimization_cache(
                    cache_root.to_path_buf(),
                    cache_source,
                    module_name,
                    SOAC_BUILD_IDENTITY,
                ),
        )
    }

    #[test]
    fn counter_modules_dedup_by_module_source_and_module_id() {
        let record = counter_record(
            "pkg.mod",
            0x1234,
            Some(RuntimeFunctionId::from_raw_parts(7, 1)),
        );
        let other_record = counter_record(
            "pkg.mod",
            0x1234,
            Some(RuntimeFunctionId::from_raw_parts(7, 2)),
        );
        let bytes = [record.encode().unwrap(), other_record.encode().unwrap()].concat();
        let records = parse_counter_dump_records(bytes.as_slice()).unwrap();

        let modules = counter_modules_from_records(records.as_slice()).unwrap();

        assert_eq!(
            modules,
            vec![CounterModuleRef {
                module_name: "pkg.mod".to_string(),
                source_hash: 0x1234,
                module_id: Some(7),
            }]
        );
    }

    #[test]
    fn counter_modules_reject_mixed_module_ids_in_one_record() {
        let mut record = counter_record(
            "pkg.mod",
            0x1234,
            Some(RuntimeFunctionId::from_raw_parts(7, 1)),
        );
        record
            .rows
            .push(counter_row(Some(RuntimeFunctionId::from_raw_parts(8, 1))));
        let bytes = record.encode().unwrap();
        let records = parse_counter_dump_records(bytes.as_slice()).unwrap();

        let err = counter_modules_from_records(records.as_slice()).unwrap_err();

        assert!(err.contains("mixes function ids from module ids 7 and 8"));
    }

    #[test]
    fn resolves_exact_current_build_identity_cache_path() {
        let root = unique_temp_dir();
        let source = "def f():\n    return 1\n";
        let module_ref = CounterModuleRef {
            module_name: "pkg.mod".to_string(),
            source_hash: hash_module_source(source),
            module_id: Some(7),
        };
        let path = module_cache_path_for_identity(root.as_path(), &module_ref, SOAC_BUILD_IDENTITY)
            .unwrap();
        prepare_with_cache_for_test(
            source,
            7,
            root.as_path(),
            PythonModuleCacheSource::Project,
            module_ref.module_name.as_str(),
            false,
        );

        let resolved = resolve_module_cache_path(root.as_path(), &module_ref, None).unwrap();

        assert_eq!(resolved, path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_resolution_rejects_ambiguous_source_subtrees() {
        let root = unique_temp_dir();
        let source = "def f():\n    return 1\n";
        let lowered = prepare_for_test(source, 7, CodegenPreparationOptions::default());
        let module_ref = CounterModuleRef {
            module_name: "pkg.mod".to_string(),
            source_hash: hash_module_source(source),
            module_id: Some(7),
        };
        for source in [
            PythonModuleCacheSource::Project,
            PythonModuleCacheSource::PythonStdlib,
        ] {
            let path =
                codegen_module_cache_path(root.as_path(), source, module_ref.module_name.as_str())
                    .unwrap();
            store_codegen_module_cache(
                path.as_path(),
                &module_cache_metadata_for_source(source, &module_ref, SOAC_BUILD_IDENTITY),
                &lowered,
            )
            .unwrap();
        }

        let err = resolve_module_cache_path(root.as_path(), &module_ref, None).unwrap_err();

        assert!(err.contains("multiple cached BlockPy modules"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_resolution_ignores_stale_metadata() {
        let root = unique_temp_dir();
        let source = "def f():\n    return 1\n";
        let lowered = prepare_for_test(source, 7, CodegenPreparationOptions::default());
        let module_ref = CounterModuleRef {
            module_name: "pkg.mod".to_string(),
            source_hash: hash_module_source(source),
            module_id: Some(7),
        };
        let stale_ref = CounterModuleRef {
            source_hash: 0x1234,
            ..module_ref.clone()
        };
        let path = codegen_module_cache_path(
            root.as_path(),
            PythonModuleCacheSource::Project,
            module_ref.module_name.as_str(),
        )
        .unwrap();
        store_codegen_module_cache(
            path.as_path(),
            &module_cache_metadata_for_source(
                PythonModuleCacheSource::Project,
                &stale_ref,
                SOAC_BUILD_IDENTITY,
            ),
            &lowered,
        )
        .unwrap();

        let err = resolve_module_cache_path(root.as_path(), &module_ref, None).unwrap_err();

        assert!(err.contains("not found under"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn include_soac_runtime_module_adds_runtime_cache_entry() {
        let root = unique_temp_dir();
        let cache_root = root.join("modules");
        let runtime_source = fs::read_to_string(soac_runtime_source_path()).unwrap();
        let runtime_ref = CounterModuleRef {
            module_name: SOAC_RUNTIME_MODULE_NAME.to_string(),
            source_hash: hash_module_source(runtime_source.as_str()),
            module_id: None,
        };
        prepare_with_cache_for_test(
            runtime_source.as_str(),
            11,
            cache_root.as_path(),
            PythonModuleCacheSource::Project,
            runtime_ref.module_name.as_str(),
            true,
        );
        let mut modules = vec![CounterModuleRef {
            module_name: "pkg.mod".to_string(),
            source_hash: 0x1234,
            module_id: Some(7),
        }];

        include_soac_runtime_module(&mut modules, cache_root.as_path(), None).unwrap();

        assert!(modules.contains(&runtime_ref));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn object_file_name_is_path_safe_and_stable() {
        let module_ref = CounterModuleRef {
            module_name: "pkg/mod:name".to_string(),
            source_hash: 0x1234,
            module_id: Some(7),
        };

        assert_eq!(
            object_file_name(&module_ref),
            "pkg_mod_name-0000000000001234-m7.o"
        );
    }

    #[test]
    fn offline_precompile_links_shared_library_from_counter_dump_and_cache() {
        if !linker_available(OsStr::new("cc")) {
            eprintln!("skipping offline precompile shared-library test: cc is unavailable");
            return;
        }

        let root = unique_temp_dir();
        let source = "def f():\n    return 12345\n";
        let module_name = "pkg.mod";
        let module_id = 7;
        let source_hash = hash_module_source(source);
        let module_ref = CounterModuleRef {
            module_name: module_name.to_string(),
            source_hash,
            module_id: Some(module_id),
        };
        let cache_root = root.join("modules");
        let cache_path =
            module_cache_path_for_identity(cache_root.as_path(), &module_ref, SOAC_BUILD_IDENTITY)
                .unwrap();
        let output = prepare_with_cache_for_test(
            source,
            module_id,
            cache_root.as_path(),
            PythonModuleCacheSource::Project,
            module_ref.module_name.as_str(),
            false,
        );
        assert!(
            cache_path.exists(),
            "lowering should populate the module cache"
        );
        let runtime_source = fs::read_to_string(soac_runtime_source_path()).unwrap();
        let runtime_ref = CounterModuleRef {
            module_name: SOAC_RUNTIME_MODULE_NAME.to_string(),
            source_hash: hash_module_source(runtime_source.as_str()),
            module_id: None,
        };
        let runtime_cache_path =
            module_cache_path_for_identity(cache_root.as_path(), &runtime_ref, SOAC_BUILD_IDENTITY)
                .unwrap();
        prepare_with_cache_for_test(
            runtime_source.as_str(),
            11,
            cache_root.as_path(),
            PythonModuleCacheSource::Project,
            runtime_ref.module_name.as_str(),
            true,
        );
        let function_id = output
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .map(|function| function.function_id)
            .expect("test module should contain f");
        let counters_path = root.join("profile.bin");
        let record = counter_record(module_name, source_hash, Some(function_id));
        let runtime_record =
            counter_record(SOAC_RUNTIME_MODULE_NAME, runtime_ref.source_hash, None);
        fs::write(
            counters_path.as_path(),
            [record.encode().unwrap(), runtime_record.encode().unwrap()].concat(),
        )
        .unwrap();
        let evidence_store = ProfileEvidenceStore::from_counter_dump(counters_path.as_path())
            .expect("test counter dump should load");
        generate_optimization_plans_v3_for_cached_modules(
            &evidence_store,
            [
                CachedModuleOptimizationInput::new(cache_path.clone(), true),
                CachedModuleOptimizationInput::new(runtime_cache_path, true),
            ],
            cache_root.as_path(),
        )
        .expect("test should generate v3 precompile plans");

        let out_path = root.join("libsoac_precompiled_test.so");
        let object_dir = root.join("objects");
        run_with_args([
            OsString::from("--counters"),
            counters_path.into_os_string(),
            OsString::from("--module-cache-dir"),
            cache_root.into_os_string(),
            OsString::from("--out"),
            out_path.clone().into_os_string(),
            OsString::from("--object-dir"),
            object_dir.clone().into_os_string(),
        ])
        .unwrap();

        let object_path = object_dir.join(object_file_name(&module_ref));
        let runtime_object_path = object_dir.join(object_file_name(&runtime_ref));
        assert_elf_file(object_path.as_path());
        assert_elf_file(runtime_object_path.as_path());
        assert_elf_file(out_path.as_path());
        let _ = fs::remove_dir_all(root);
    }

    fn counter_record(
        module_name: &str,
        source_hash: u64,
        function_id: Option<RuntimeFunctionId>,
    ) -> CounterDumpRecord {
        CounterDumpRecord {
            source_hash,
            module_name: module_name.to_string(),
            package_name: None,
            rows: vec![counter_row(function_id)],
            module_keys: Vec::new(),
            type_keys: Vec::new(),
            type_table: Vec::new(),
        }
    }

    fn counter_row(function_id: Option<RuntimeFunctionId>) -> CounterDumpRow {
        CounterDumpRow {
            counter_id: 0,
            scope: "function".to_string(),
            kind: "block_entry".to_string(),
            site_kind: "block_entry".to_string(),
            function_id,
            current_function_id: function_id,
            instr_id: None,
            function_qualname: Some("f".to_string()),
            block_label: Some("bb0".to_string()),
            value: 1,
            branch_values: Vec::new(),
            observed_value: None,
            max_overcount: None,
        }
    }

    fn unique_temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "soac-precompile-blockpy-test-{}-{unique}",
            std::process::id()
        ))
    }

    fn linker_available(linker: &OsStr) -> bool {
        Command::new(linker)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn assert_elf_file(path: &Path) {
        let bytes = fs::read(path).unwrap_or_else(|err| {
            panic!("failed to read emitted ELF file {}: {err}", path.display())
        });
        assert!(
            bytes.starts_with(b"\x7fELF"),
            "{} should be an ELF file",
            path.display()
        );
    }
}
