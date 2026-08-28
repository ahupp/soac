//! Offline type analysis. No part of this executable is linked into SOAC's JIT.

mod inputs;
mod policy;
mod publish;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use ruff_db::files::{File, system_path_to_file, vendored_path_to_file};
use ruff_db::system::{System, SystemPath};
use ruff_db::vendored::VendoredPath;
use ruff_python_ast::PythonVersion as TyPythonVersion;
use ruff_ranged_value::ValueSource;
use serde::Serialize;
use soac_contracts::{
    AnalysisDependency, AnalysisDependencySource, AnalysisDirectoryFilter,
    AnalysisFileConfiguration, ArtifactEnvironment, ArtifactSigningKey, ConservativeAnalysis,
    DEPLOYMENT_SCHEMA_VERSION, DeployedModule, Fingerprint, InterpreterIdentity,
    ModuleArtifactIndex, PythonVersion, SourceDialect, StrictArtifactDeployment,
    TypeArtifactManifest, encode_module_shard, verify_analysis_inputs,
};
use ty_project::{ProjectDatabase, ProjectMetadata};
use ty_python_core::AnalysisDialect;
use ty_python_semantic::{
    Db as _, SoacDependencyPath, SoacSourcePolicies, effective_analysis_settings,
    export_soac_module,
};

use inputs::AnalysisSystem;
use policy::ProjectPolicy;

const TY_REVISION: &str = env!("SOAC_TY_RUFF_REVISION");
const CHECKER_SOURCE: &str = env!("SOAC_TY_CHECKER_FINGERPRINT");
const EXPORTER_SOURCE: &str = env!("SOAC_TY_EXPORTER_FINGERPRINT");

#[derive(Parser)]
#[command(
    name = "soac-ty",
    version,
    about = "Authenticated offline ty contracts for strict SOAC modules"
)]
struct Cli {
    #[command(subcommand)]
    command: Operation,
}

#[derive(Subcommand)]
enum Operation {
    /// Generate a private build-side signing seed. Never put it in an artifact directory.
    Keygen {
        #[arg(long)]
        signing_key: PathBuf,
    },
    /// Analyze source without importing it, then atomically publish a complete signed generation.
    Check(Check),
}

#[derive(clap::Args)]
struct Check {
    #[arg(long, default_value = ".")]
    project: PathBuf,
    /// Import root used to derive canonical dotted names (defaults to project).
    #[arg(long)]
    source_root: Option<PathBuf>,
    /// Explicit driver name=path mapping, for example __main__=bench.py.
    #[arg(long = "module")]
    modules: Vec<String>,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    signing_key: PathBuf,
    /// Out-of-band startup authority, separate from the writable artifact root.
    #[arg(long)]
    deployment: PathBuf,
    /// Exact target CPython executable, queried with -I -S (no project imports).
    #[arg(long)]
    python: PathBuf,
    #[arg(long, default_value = "3.15")]
    python_version: String,
}

fn fingerprint(value: &impl Serialize) -> Result<Fingerprint> {
    // All unordered maps in these owned records are BTreeMaps or serde's
    // canonical sorted map. No Debug/type rendering or Salsa identity is hashed.
    Ok(Fingerprint::digest(serde_json::to_vec(value)?))
}

fn absolute(path: &Path, root: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn system_path(path: &Path) -> Result<&SystemPath> {
    Ok(SystemPath::new(
        path.to_str()
            .context("offline analysis requires UTF-8 paths")?,
    ))
}

fn file_configuration(database: &ProjectDatabase, file: File) -> Result<AnalysisFileConfiguration> {
    use ruff_db::diagnostic::Severity;

    let program_file = database.program_file(file);
    let analysis_policy = program_file.analysis_policy(database);
    let analysis = effective_analysis_settings(database, file);
    ensure!(
        analysis_policy.dialect == AnalysisDialect::SoacStrictV1
            && analysis_policy.python_version == TyPythonVersion::PY315
            && analysis.strict_equality_semantics
            && analysis.strict_generic_narrowing,
        "consumed source does not have the authenticated conservative Python 3.15 analysis policy"
    );
    let program = program_file.program(database);
    Ok(AnalysisFileConfiguration {
        python_version: PythonVersion {
            major: 3,
            minor: 15,
        },
        python_platform: program.python_platform(database).to_string(),
        analysis: ConservativeAnalysis {
            strict_equality_semantics: analysis.strict_equality_semantics,
            strict_generic_narrowing: analysis.strict_generic_narrowing,
        },
        respect_type_ignore_comments: analysis.respect_type_ignore_comments,
        import_search_paths: ty_module_resolver::system_module_search_paths(
            database,
            program.resolver_environment(database),
        )
        .map(|path| path.to_string())
        .collect(),
        enabled_diagnostics: database
            .rule_selection(file)
            .iter()
            .map(|(lint, severity)| {
                let level = match severity {
                    Severity::Info => "info",
                    Severity::Warning => "warning",
                    Severity::Error => "error",
                    Severity::Fatal => "fatal",
                };
                (lint.name().to_string(), level.to_owned())
            })
            .collect(),
    })
}

fn dependency_source(path: &SoacDependencyPath) -> AnalysisDependencySource {
    match path {
        SoacDependencyPath::System(path) => AnalysisDependencySource::System { path: path.into() },
        SoacDependencyPath::Vendored(path) => {
            AnalysisDependencySource::Vendored { path: path.clone() }
        }
    }
}

fn dependency_key(path: &SoacDependencyPath) -> String {
    match path {
        SoacDependencyPath::System(path) => format!("system:{path}"),
        SoacDependencyPath::Vendored(path) => format!("vendored:{path}"),
    }
}

fn interpreter_identity(python: &Path) -> Result<InterpreterIdentity> {
    // Invoke the selected spelling: resolving a venv's executable symlink
    // before launching Python discards its prefix and package search paths.
    let mut command = Command::new(python);
    #[cfg(target_os = "linux")]
    if let Some(parent) = python.canonicalize()?.parent() {
        // Source-build CPython keeps its matching shared library beside the
        // real executable, not necessarily beside a venv's symlink. The probe
        // reports the actual loaded library below.
        let mut paths = vec![parent.to_path_buf()];
        if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        command.env("LD_LIBRARY_PATH", std::env::join_paths(paths)?);
    }
    let output = command.args(["-I", "-S", "-B", "-c", r#"
import json, os, sys, sysconfig
keys = ('SOABI', 'MULTIARCH', 'Py_DEBUG', 'Py_GIL_DISABLED', 'SIZEOF_VOID_P',
        'SIZEOF_LONG', 'LDLIBRARY', 'LIBDIR', 'Py_ENABLE_SHARED', 'ABIFLAGS')
configuration = {key: sysconfig.get_config_var(key) for key in keys}
abi_files = [os.path.realpath(sys.executable)]
if sys.platform == 'linux' and configuration['Py_ENABLE_SHARED']:
    with open('/proc/self/maps', encoding='utf-8') as maps:
        abi_files.extend(line.split()[-1] for line in maps
                         if '/libpython' in line and '.so' in line and line.split()[-1].startswith('/'))
    if len(set(abi_files)) == 1:
        raise RuntimeError('cannot identify the selected interpreter loaded libpython')
elif configuration['Py_ENABLE_SHARED']:
    raise RuntimeError('shared interpreter identity is currently supported on Linux only')
configuration_files = [sysconfig.__file__, sysconfig.get_config_h_filename(), sysconfig.get_makefile_filename()]
data = sys.modules.get(sysconfig._get_sysconfigdata_name())
if data is not None and getattr(data, '__file__', None): configuration_files.append(data.__file__)
configuration_files.append(os.path.join(sys.prefix, 'pyvenv.cfg'))
for executable_path in (sys.executable, os.path.realpath(sys.executable)):
    configuration_files.extend((executable_path + '._pth', os.path.join(os.path.dirname(executable_path), 'pybuilddir.txt')))
print(json.dumps(dict(version=list(sys.version_info[:2]), platform=sys.platform,
    prefix=sys.prefix, executable=os.path.realpath(sys.executable),
    build_directory=sysconfig.get_config_var('abs_builddir') or sys.prefix,
    site_packages=sorted(set((sysconfig.get_path('purelib'), sysconfig.get_path('platlib')))),
    real_stdlib=getattr(sys, '_stdlib_dir', None) or sysconfig.get_path('stdlib'),
    abi_files=sorted(set(abi_files)), configuration_files=sorted(set(configuration_files)),
    configuration=configuration)))
"#]).output().context("query isolated target CPython ABI")?;
    ensure!(
        output.status.success(),
        "target CPython identity query failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)
        .context("target CPython returned an invalid identity")?)
}

fn keygen(path: &Path) -> Result<()> {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).map_err(|error| anyhow::anyhow!("signing key entropy: {error}"))?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .context("create a new private signing key")?;
    file.write_all(&seed)?;
    file.sync_all()?;
    println!(
        "Created private signing key {}; its bytes are not part of the artifact.",
        path.display()
    );
    Ok(())
}

fn load_key(path: &Path) -> Result<ArtifactSigningKey> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            fs::metadata(path)?.permissions().mode() & 0o077 == 0,
            "signing key must not be accessible to group or other users"
        );
    }
    let bytes = fs::read(path)?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing key must contain exactly 32 bytes"))?;
    Ok(ArtifactSigningKey::from_bytes(&seed))
}

fn module_name(path: &Path, source_root: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(source_root)
        .context("source is outside its import root")?;
    let mut parts = relative
        .with_extension("")
        .components()
        .map(|part| {
            part.as_os_str()
                .to_str()
                .map(str::to_owned)
                .context("module name is not UTF-8")
        })
        .collect::<Result<Vec<_>>>()?;
    if parts.last().is_some_and(|part| part == "__init__") {
        parts.pop();
    }
    ensure!(
        !parts.is_empty(),
        "root __init__.py needs an explicit --module name=path"
    );
    Ok(parts.join("."))
}

fn sources(
    options: &Check,
    project: &Path,
    source_root: &Path,
    policy: &mut ProjectPolicy,
    system: &AnalysisSystem,
    output: &Path,
) -> Result<BTreeMap<String, (PathBuf, soac_contracts::ResolvedStrictPolicy)>> {
    let paths = if options.modules.is_empty() {
        let mut paths = Vec::new();
        let mut pending = vec![source_root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let mut excluded_names = [
                ".git",
                ".jj",
                ".venv",
                "vendor",
                "work",
                "target",
                "__pycache__",
            ]
            .map(str::to_owned)
            .to_vec();
            if output.parent() == Some(directory.as_path()) {
                excluded_names.push(
                    output
                        .file_name()
                        .context("output directory name")?
                        .to_str()
                        .context("UTF-8 output directory")?
                        .to_owned(),
                );
            }
            excluded_names.sort();
            excluded_names.dedup();
            for entry in system.read_directory_filtered(
                system_path(&directory)?,
                AnalysisDirectoryFilter::SourceSelection { excluded_names },
            )? {
                let path = entry.path().as_std_path();
                if entry.file_type().is_directory() {
                    pending.push(path.to_path_buf());
                } else if entry.file_type().is_file()
                    && path.extension().is_some_and(|ext| ext == "py")
                {
                    let path = path.canonicalize()?;
                    paths.push((module_name(&path, source_root)?, path));
                }
            }
        }
        paths
    } else {
        options
            .modules
            .iter()
            .map(|entry| {
                let (name, path) = entry
                    .split_once('=')
                    .context("--module must be dotted.name=path")?;
                let path = absolute(Path::new(path), project);
                system.observe_path(system_path(&path)?)?;
                Ok((name.to_owned(), path.canonicalize()?))
            })
            .collect::<Result<Vec<_>>>()?
    };
    let mut modules = BTreeMap::new();
    for (name, path) in paths {
        path.strip_prefix(project)
            .context("selected source is outside the project")?;
        let resolved = policy.for_path(&path, system)?;
        ensure!(
            modules.insert(name, (path, resolved)).is_none(),
            "duplicate canonical module name"
        );
    }
    ensure!(
        !modules.is_empty(),
        "no Python source files selected for analysis"
    );
    Ok(modules)
}

fn check(options: Check) -> Result<publish::Publication> {
    let project_selection = absolute(&options.project, &std::env::current_dir()?);
    let project = project_selection.canonicalize()?;
    let source_selection = options
        .source_root
        .as_ref()
        .map_or_else(|| project.clone(), |path| absolute(path, &project));
    let source_root = source_selection.canonicalize()?;
    let output = absolute(&options.output, &project);
    fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    let signing_path = absolute(&options.signing_key, &project).canonicalize()?;
    let deployment_path = absolute(&options.deployment, &project);
    if let Some(parent) = deployment_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let deployment_path = deployment_path
        .parent()
        .context("deployment parent")?
        .canonicalize()?
        .join(deployment_path.file_name().context("deployment filename")?);
    ensure!(
        deployment_path != signing_path,
        "startup authority must not overwrite the private signing key"
    );
    ensure!(
        !signing_path.starts_with(&output),
        "signing key must be outside the writable artifact root"
    );
    ensure!(
        !deployment_path
            .parent()
            .context("deployment parent")?
            .canonicalize()?
            .starts_with(&output),
        "startup authority must be outside the writable artifact root"
    );
    let signing_key = load_key(&signing_path)?;
    let system = AnalysisSystem::new(system_path(&project)?);
    system.observe_path(system_path(&project_selection)?)?;
    system.observe_path(system_path(&source_selection)?)?;
    let config_path = project.join("pyproject.toml");
    match system.read_to_string(system_path(&config_path)?) {
        Ok(source) => policy::reject_config_policy(&source)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut policy = ProjectPolicy::new(source_root.clone());
    ensure!(
        output != source_root,
        "artifact output must not replace the source root"
    );
    let modules = sources(
        &options,
        &project,
        &source_root,
        &mut policy,
        &system,
        &output,
    )?;
    let source_policies = modules
        .values()
        .map(|(path, policy)| Ok((system_path(path)?.to_path_buf(), policy.clone())))
        .collect::<Result<SoacSourcePolicies>>()?;
    let python_selection = absolute(&options.python, &project);
    system.observe_path(system_path(&python_selection)?)?;
    let python = python_selection.canonicalize()?;
    system.observe_path(system_path(&python)?)?;
    let _ = system.env_var("LD_LIBRARY_PATH");
    let interpreter = interpreter_identity(&python_selection)?;
    ensure!(
        options.python_version == "3.15" && interpreter.version == [3, 15],
        "strict contract v1 requires the selected CPython 3.15 executable"
    );
    for path in interpreter
        .abi_files
        .iter()
        .chain(&interpreter.configuration_files)
    {
        system.observe_path(system_path(Path::new(path))?)?;
    }
    system.observe_path(system_path(Path::new(&interpreter.real_stdlib))?)?;
    // Distribution metadata is part of validity even when ty only reads a stub.
    for site in &interpreter.site_packages {
        let site = Path::new(site);
        system.observe_path(system_path(site)?)?;
        if site.is_dir() {
            for entry in system.read_directory_filtered(
                system_path(site)?,
                AnalysisDirectoryFilter::Suffix {
                    suffix: ".dist-info".into(),
                },
            )? {
                let path = entry.into_path();
                for name in ["METADATA", "WHEEL", "RECORD", "direct_url.json"] {
                    system.observe_path(&path.join(name))?;
                }
            }
        }
    }
    let mut metadata = ProjectMetadata::discover_without_uv(system_path(&project)?, &system)?;
    // ty may select an ancestor's configuration rather than --project itself.
    // Check that actual project too, without scanning unrelated ancestors.
    if metadata.root() != system_path(&project)? {
        let discovered_config = metadata.root().join("pyproject.toml");
        match system.read_to_string(&discovered_config) {
            Ok(source) => policy::reject_config_policy(&source)
                .with_context(|| format!("invalid SOAC configuration in {discovered_config}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    metadata.apply_configuration_files(&system)?;
    // ty's ranged configuration values need their real option provenance.
    // Use its configuration parser rather than deserializing detached JSON,
    // which loses that provenance and panics inside RangedValue.
    metadata.apply_override_options(ty_project::metadata::Options::from_toml_str(
        &toml::to_string(&serde_json::json!({
            "environment": {
                "python-version": options.python_version,
                "python-platform": interpreter.platform,
                "root": [source_root],
            },
        "analysis": { "strict-equality-semantics": true, "strict-generic-narrowing": true },
        "src": { "respect-ignore-files": false }
        }))?,
        ValueSource::Cli,
    )?);
    let configured_options = serde_json::to_value(metadata.to_merged_options().options())?;
    let mut database = ProjectDatabase::fallible_with_python_environment(
        metadata,
        system.clone(),
        AnalysisDialect::SoacStrictV1,
        ty_project::metadata::PythonEnvironmentPaths {
            site_packages: interpreter
                .site_packages
                .iter()
                .map(|path| ruff_db::system::SystemPathBuf::from(path.as_str()))
                .collect(),
            real_stdlib: Some(ruff_db::system::SystemPathBuf::from(
                interpreter.real_stdlib.as_str(),
            )),
        },
    )?;
    let selected_source_owners = modules
        .iter()
        .filter(|(_, (_, policy))| policy.is_selected())
        .map(|(name, (path, _))| (name, path))
        .collect::<Vec<_>>();
    // Discovery also finds ordinary dependencies. Their normal
    // source/stub resolution must not be replaced by strict source ownership.
    let selected_source_modules = selected_source_owners
        .iter()
        .map(|&(name, path)| {
            let name = ty_module_resolver::ModuleName::new(name.as_str())
                .context("selected source has an invalid logical module name")?;
            Ok((name, system_path(path)?.to_path_buf()))
        })
        .collect::<Result<Vec<_>>>()?;
    let project = ty_project::Db::project(&database);
    let mut program_settings = project.program_settings(&database).clone();
    program_settings.search_paths = program_settings
        .search_paths
        .with_selected_source_modules(&system, selected_source_modules)
        .map_err(anyhow::Error::msg)?;
    project.update_program(&mut database, program_settings);

    ty_project::Db::project(&database).set_included_paths(
        &mut database,
        modules
            .values()
            .map(|(path, _)| system_path(path).map(SystemPath::to_path_buf))
            .collect::<Result<Vec<_>>>()?,
    );
    let mut exports = Vec::new();
    let mut deployed_modules = Vec::new();
    let mut per_file_settings = BTreeMap::new();
    let mut search_paths = BTreeSet::new();
    for (name, (path, module_policy)) in &modules {
        let file = system_path_to_file(&database, system_path(path)?)?;
        let configuration = file_configuration(&database, file)?;
        ensure!(
            configuration.python_platform == interpreter.platform,
            "file {name} has a different target platform"
        );
        search_paths.extend(configuration.import_search_paths.iter().cloned());
        per_file_settings.insert(format!("system:{}", path.display()), configuration);
        let export = export_soac_module(&database, file, name, &source_policies)?;
        for diagnostic in &export.facts.diagnostics {
            eprintln!(
                "{name}:{}..{}: {:?}: {}{}",
                diagnostic.source_range.start,
                diagnostic.source_range.end,
                diagnostic.code,
                diagnostic.message,
                if diagnostic.suppressed {
                    " (suppressed; facts withheld)"
                } else {
                    ""
                }
            );
        }
        if export.facts.source_dialect == SourceDialect::OrdinaryPython {
            // This command publishes selected contracts, not a normal ty check.
            // Ordinary diagnostics are informative; no contract is emitted for
            // them. Consumed inputs still participate in authentication.
            continue;
        }
        deployed_modules.push(DeployedModule {
            module_name: name.clone(),
            source_path: path.clone(),
            policy: module_policy.clone(),
        });
        exports.push(export);
    }
    // Every consumed source gets its actual ProgramFile policy, including
    // imported source/stubs that were not selected as strict output modules.
    for export in &exports {
        for dependency in &export.dependencies {
            let key = dependency_key(&dependency.path);
            if per_file_settings.contains_key(&key) {
                continue;
            }
            let file = match &dependency.path {
                SoacDependencyPath::System(path) => {
                    system_path_to_file(&database, SystemPath::new(path))?
                }
                SoacDependencyPath::Vendored(path) => {
                    vendored_path_to_file(&database, VendoredPath::new(path))?
                }
            };
            let configuration = file_configuration(&database, file)?;
            ensure!(
                configuration.python_platform == interpreter.platform,
                "dependency has a different target platform"
            );
            search_paths.extend(configuration.import_search_paths.iter().cloned());
            per_file_settings.insert(key, configuration);
        }
    }
    let (inputs, analysis_environment) = system.snapshot()?;
    ensure!(
        interpreter_identity(&python_selection)? == interpreter,
        "selected interpreter changed during analysis"
    );
    let stub_inputs = inputs
        .iter()
        .filter(|input| {
            input
                .path
                .extension()
                .is_some_and(|extension| extension == "pyi")
        })
        .collect::<Vec<_>>();
    let configuration = fingerprint(&(
        configured_options,
        &per_file_settings,
        &selected_source_owners,
    ))?;
    let environment = ArtifactEnvironment {
        ty_revision: TY_REVISION.into(),
        checker_source_fingerprint: Fingerprint::from_hex(CHECKER_SOURCE)?,
        exporter_revision: EXPORTER_SOURCE.into(),
        python_version: PythonVersion {
            major: 3,
            minor: 15,
        },
        python_platform: interpreter.platform.clone(),
        cpython_abi_fingerprint: interpreter.abi_fingerprint(&inputs)?,
        normalized_project_policy: fingerprint(&source_policies)?,
        resolved_typechecker_configuration: configuration,
        import_search_path: fingerprint(&(search_paths, &analysis_environment))?,
        typeshed_fingerprint: Fingerprint::from_hex(CHECKER_SOURCE)?,
        installed_stub_fingerprint: fingerprint(&stub_inputs)?,
        installed_dependency_fingerprint: fingerprint(&inputs)?,
        analysis: ConservativeAnalysis::default(),
    };
    let mut shards = Vec::new();
    let mut analysis_dependencies = Vec::new();
    for mut export in exports {
        for dependency in export.dependencies {
            let observed = AnalysisDependency {
                importer_module: export.facts.module.module_name.clone(),
                module: dependency.module,
                source: dependency_source(&dependency.path),
                source_digest: dependency.source_digest,
                source_size: dependency.source_size,
                configuration: per_file_settings
                    .get(&dependency_key(&dependency.path))
                    .context("dependency has no resolved per-file policy")?
                    .clone(),
            };
            export
                .facts
                .consumed_dependencies
                .push(observed.fingerprint(&environment, &deployed_modules, &inputs)?);
            analysis_dependencies.push(observed);
        }
        shards.push(encode_module_shard(&export.facts)?);
    }
    let indices = shards
        .iter()
        .map(ModuleArtifactIndex::from_shard)
        .collect::<Result<Vec<_>, _>>()?;
    let manifest = TypeArtifactManifest::new(environment.clone(), indices)?;
    publish::check_output_boundary(&inputs, &output, &deployment_path)?;
    verify_analysis_inputs(&inputs)?;
    let publication = publish::publish(&output, &manifest, &shards, &signing_key)?;
    verify_analysis_inputs(&inputs)?;
    let deployment = StrictArtifactDeployment {
        schema_version: DEPLOYMENT_SCHEMA_VERSION,
        artifact_directory: publication.artifact_directory.clone(),
        generation: manifest.generation,
        environment,
        target_interpreter: interpreter,
        trust_anchor: signing_key.trust_anchor().to_bytes(),
        modules: deployed_modules,
        analysis_dependencies,
        analysis_inputs: inputs,
        analysis_environment,
    };
    publish::write_deployment(&deployment_path, &deployment)?;
    Ok(publication)
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Operation::Keygen { signing_key } => keygen(&signing_key),
        Operation::Check(options) => {
            let publication = check(options)?;
            println!("{}", serde_json::to_string(&publication)?);
            Ok(())
        }
    }
}
