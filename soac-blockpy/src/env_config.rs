use crate::codegen_cache::{
    codegen_module_cache_path, CachedCodegenModuleMetadata, PythonModuleCacheSource,
};
use std::env::{self, VarError};
use std::path::{Path, PathBuf};

pub const SOAC_OPT_MODE_ENV: &str = "SOAC_OPT_MODE";
pub const SOAC_WORK_DIR_ENV: &str = "SOAC_WORK_DIR";
pub const SOAC_CRANELIFT_OPT_LEVEL_ENV: &str = "SOAC_CRANELIFT_OPT_LEVEL";
pub const SOAC_ENABLE_PROFILED_COLD_BLOCKS_ENV: &str = "SOAC_ENABLE_PROFILED_COLD_BLOCKS";
pub const SOAC_JIT_EMIT_REFCOUNTS_ENV: &str = "SOAC_JIT_EMIT_REFCOUNTS";
pub const SOAC_JIT_COMPILE_WORKERS_ENV: &str = "SOAC_JIT_COMPILE_WORKERS";
pub const SOAC_BACKGROUND_JIT_ENV: &str = "SOAC_BACKGROUND_JIT";
pub const SOAC_MODULE_CACHE_DIR_ENV: &str = "SOAC_MODULE_CACHE_DIR";
pub const SOAC_PRECOMPILED_LIBRARY_ENV: &str = "SOAC_PRECOMPILED_LIBRARY";
pub const SOAC_COMPILE_MODE_ENV: &str = "SOAC_COMPILE_MODE";
pub const SOAC_JIT_PERF_HELPER_FRAMES_ENV: &str = "SOAC_JIT_PERF_HELPER_FRAMES";
pub const SOAC_LOG_ENV: &str = "SOAC_LOG";
pub const SOAC_EXEC_TRACE_ENV: &str = "SOAC_EXEC_TRACE";

pub const DEFAULT_SOAC_JSON_LOG_FILTER: &str =
    "soac_jit=info,soac_module_load=info,soac_jit_codegen=info,soac_specialization_runtime=info,soac_blockpy_module_cache=info";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecializationMode {
    Profile,
    Verify,
    Apply,
}

impl SpecializationMode {
    pub fn from_str(mode: &str) -> Result<Option<Self>, String> {
        match mode.trim() {
            "none" => Ok(None),
            "profile" => Ok(Some(Self::Profile)),
            "verify" => Ok(Some(Self::Verify)),
            "apply" => Ok(Some(Self::Apply)),
            value => Err(format!(
                "unrecognized specialization mode {value:?}; expected one of: none, profile, verify, apply"
            )),
        }
    }

    pub fn records_counters(self) -> bool {
        matches!(self, Self::Profile | Self::Verify)
    }

    pub fn behavior_change_indexed_stores_enabled(self) -> bool {
        matches!(self, Self::Verify | Self::Apply)
    }

    fn output_counter_dump_filename(self) -> Option<&'static str> {
        match self {
            Self::Profile => Some("profile.bin"),
            Self::Verify => Some("verify.bin"),
            Self::Apply => None,
        }
    }

    fn input_counter_dump_filename(self) -> Option<&'static str> {
        match self {
            Self::Verify | Self::Apply => Some("profile.bin"),
            Self::Profile => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileMode {
    Lazy,
    Eager,
}

impl CompileMode {
    pub fn from_str(mode: &str) -> Result<Self, String> {
        match mode.trim().to_ascii_lowercase().as_str() {
            "lazy" => Ok(Self::Lazy),
            "eager" => Ok(Self::Eager),
            value => Err(format!(
                "unrecognized compile mode {value:?}; expected one of: lazy, eager"
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SoacLogConfig {
    pub filter: String,
    pub json_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SoacEnvConfig {
    cranelift_opt_level: String,
    specialization_mode: Option<SpecializationMode>,
    soac_work_dir: Option<PathBuf>,
    profiled_cold_blocks_enabled: bool,
    jit_refcount_emission_enabled: bool,
    module_cache_dir: Option<PathBuf>,
    compile_mode: CompileMode,
    jit_compile_workers: Option<usize>,
    background_jit_enabled: bool,
    jit_perf_helper_frames_enabled: bool,
    soac_exec_trace: Option<String>,
    soac_log: SoacLogConfig,
    soac_log_explicit: bool,
}

impl SoacEnvConfig {
    pub fn from_env() -> Result<Self, String> {
        let cranelift_opt_level = parse_optional_cranelift_opt_level(
            env_string(SOAC_CRANELIFT_OPT_LEVEL_ENV)?.as_deref(),
        )?;
        let specialization_mode =
            parse_optional_specialization_mode(env_string(SOAC_OPT_MODE_ENV)?.as_deref())?;
        let soac_work_dir = env_path(SOAC_WORK_DIR_ENV)?;
        let profiled_cold_blocks_enabled = env_bool(SOAC_ENABLE_PROFILED_COLD_BLOCKS_ENV, false)?;
        let jit_refcount_emission_enabled = env_bool(SOAC_JIT_EMIT_REFCOUNTS_ENV, true)?;
        let module_cache_dir = env_path(SOAC_MODULE_CACHE_DIR_ENV)?;
        let compile_mode =
            parse_optional_compile_mode(env_string(SOAC_COMPILE_MODE_ENV)?.as_deref())?;
        let jit_compile_workers = parse_optional_positive_usize(
            SOAC_JIT_COMPILE_WORKERS_ENV,
            env_string(SOAC_JIT_COMPILE_WORKERS_ENV)?.as_deref(),
        )?;
        let background_jit_enabled = env_bool(SOAC_BACKGROUND_JIT_ENV, true)?;
        let jit_perf_helper_frames_enabled = env_bool(SOAC_JIT_PERF_HELPER_FRAMES_ENV, false)?;
        let soac_exec_trace = env_string(SOAC_EXEC_TRACE_ENV)?;
        let soac_log_raw = env_string(SOAC_LOG_ENV)?;
        let soac_log_explicit = soac_log_raw
            .as_ref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        let soac_log = parse_soac_log_config(soac_log_raw.as_deref(), soac_work_dir.as_deref())?;
        Ok(Self {
            cranelift_opt_level,
            specialization_mode,
            soac_work_dir,
            profiled_cold_blocks_enabled,
            jit_refcount_emission_enabled,
            module_cache_dir,
            compile_mode,
            jit_compile_workers,
            background_jit_enabled,
            jit_perf_helper_frames_enabled,
            soac_exec_trace,
            soac_log,
            soac_log_explicit,
        })
    }

    pub fn cranelift_opt_level(&self) -> &str {
        self.cranelift_opt_level.as_str()
    }

    pub fn specialization_mode(&self) -> Option<SpecializationMode> {
        self.specialization_mode
    }

    pub fn soac_work_dir(&self) -> Option<&Path> {
        self.soac_work_dir.as_deref()
    }

    pub fn counter_dump_input_path(&self) -> Option<PathBuf> {
        let mode = self.specialization_mode?;
        let filename = mode.input_counter_dump_filename()?;
        self.soac_work_dir.as_ref().map(|dir| dir.join(filename))
    }

    pub fn counter_dump_output_path(&self) -> Option<PathBuf> {
        let mode = self.specialization_mode?;
        let filename = mode.output_counter_dump_filename()?;
        self.soac_work_dir.as_ref().map(|dir| dir.join(filename))
    }

    pub fn profiled_cold_blocks_enabled(&self) -> bool {
        self.profiled_cold_blocks_enabled
    }

    pub fn jit_refcount_emission_enabled(&self) -> bool {
        self.jit_refcount_emission_enabled
    }

    pub fn module_cache_root_or_repo(&self, repo_root: Option<&Path>) -> Option<PathBuf> {
        self.module_cache_dir
            .clone()
            .or_else(|| self.soac_work_dir.as_ref().map(|root| root.join("modules")))
            .or_else(|| repo_root.map(|root| root.join("soac-module-cache")))
    }

    pub fn compile_mode(&self) -> CompileMode {
        self.compile_mode
    }

    pub fn eager_clif_compile_requested(&self) -> bool {
        self.compile_mode == CompileMode::Eager
    }

    pub fn jit_compile_workers(&self) -> Option<usize> {
        self.jit_compile_workers
    }

    pub fn background_jit_enabled(&self) -> bool {
        self.background_jit_enabled
    }

    pub fn jit_perf_helper_frames_enabled(&self) -> bool {
        self.jit_perf_helper_frames_enabled
    }

    pub fn soac_exec_trace(&self) -> Option<&str> {
        self.soac_exec_trace.as_deref()
    }

    pub fn soac_log(&self) -> &SoacLogConfig {
        &self.soac_log
    }

    pub fn specialization_runtime_logging_enabled(&self) -> bool {
        self.specialization_mode == Some(SpecializationMode::Apply)
            && (self.soac_log_has_explicit_value() || self.soac_work_dir.is_some())
    }

    fn soac_log_has_explicit_value(&self) -> bool {
        self.soac_log_explicit
    }
}

fn invalid_env_value(name: &str, raw: &str, detail: impl AsRef<str>) -> String {
    format!("{name}={raw:?} is invalid: {}", detail.as_ref())
}

fn env_string(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(format!("{name} must be valid Unicode")),
    }
}

fn env_path(name: &str) -> Result<Option<PathBuf>, String> {
    let Some(raw) = env::var_os(name) else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Err(format!("{name} must not be empty when set"));
    }
    Ok(Some(PathBuf::from(raw)))
}

fn parse_bool_env_value(name: &str, raw: &str) -> Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(invalid_env_value(
            name,
            raw,
            "expected one of: 1, 0, true, false, yes, no, on, off",
        )),
    }
}

fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    let Some(raw) = env_string(name)? else {
        return Ok(default);
    };
    parse_bool_env_value(name, raw.as_str())
}

fn parse_cranelift_opt_level(raw: &str) -> Result<String, String> {
    match raw.trim() {
        "none" | "speed" | "speed_and_size" => Ok(raw.trim().to_string()),
        value => Err(format!(
            "unrecognized Cranelift opt level {value:?}; expected one of: none, speed, speed_and_size"
        )),
    }
}

pub fn cranelift_opt_level_from_env() -> Result<String, String> {
    Ok(SoacEnvConfig::from_env()?.cranelift_opt_level().to_string())
}

fn parse_optional_cranelift_opt_level(raw: Option<&str>) -> Result<String, String> {
    let Some(raw) = raw else {
        return Ok("speed".to_string());
    };
    parse_cranelift_opt_level(raw)
        .map_err(|err| invalid_env_value(SOAC_CRANELIFT_OPT_LEVEL_ENV, raw, err))
}

fn parse_optional_specialization_mode(
    mode: Option<&str>,
) -> Result<Option<SpecializationMode>, String> {
    let Some(mode) = mode else {
        return Ok(None);
    };
    SpecializationMode::from_str(mode)
        .map_err(|err| invalid_env_value(SOAC_OPT_MODE_ENV, mode, err))
}

fn parse_optional_compile_mode(raw: Option<&str>) -> Result<CompileMode, String> {
    let Some(raw) = raw else {
        return Ok(CompileMode::Lazy);
    };
    CompileMode::from_str(raw).map_err(|err| invalid_env_value(SOAC_COMPILE_MODE_ENV, raw, err))
}

fn parse_optional_positive_usize(name: &str, raw: Option<&str>) -> Result<Option<usize>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid_env_value(name, raw, "expected a positive integer"));
    }
    let value = trimmed
        .parse::<usize>()
        .map_err(|_| invalid_env_value(name, raw, "expected a positive integer"))?;
    if value == 0 {
        return Err(invalid_env_value(name, raw, "expected a positive integer"));
    }
    Ok(Some(value))
}

pub fn specialization_mode_from_env() -> Result<Option<SpecializationMode>, String> {
    Ok(SoacEnvConfig::from_env()?.specialization_mode())
}

pub fn soac_work_dir_from_env() -> Result<Option<PathBuf>, String> {
    Ok(SoacEnvConfig::from_env()?
        .soac_work_dir()
        .map(Path::to_path_buf))
}

pub fn counter_dump_input_path_from_env() -> Result<Option<PathBuf>, String> {
    Ok(SoacEnvConfig::from_env()?.counter_dump_input_path())
}

pub fn counter_dump_output_path_from_env() -> Result<Option<PathBuf>, String> {
    Ok(SoacEnvConfig::from_env()?.counter_dump_output_path())
}

pub fn profiled_cold_blocks_enabled_from_env() -> Result<bool, String> {
    Ok(SoacEnvConfig::from_env()?.profiled_cold_blocks_enabled())
}

pub fn jit_refcount_emission_enabled_from_env() -> Result<bool, String> {
    Ok(SoacEnvConfig::from_env()?.jit_refcount_emission_enabled())
}

pub fn module_cache_root_from_env_or_repo(
    repo_root: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    Ok(SoacEnvConfig::from_env()?.module_cache_root_or_repo(repo_root))
}

pub fn precompiled_library_path_from_env() -> Result<Option<PathBuf>, String> {
    env_path(SOAC_PRECOMPILED_LIBRARY_ENV)
}

pub fn pre_optimization_module_cache_identity(
    build_identity: &str,
    runtime_names_as_globals: bool,
) -> String {
    format!("{build_identity};runtime_names_as_globals={runtime_names_as_globals}")
}

pub fn pre_optimization_module_cache_metadata(
    source: PythonModuleCacheSource,
    module_name: &str,
    source_hash: u64,
    build_identity: &str,
    runtime_names_as_globals: bool,
) -> CachedCodegenModuleMetadata {
    CachedCodegenModuleMetadata {
        source,
        module_name: module_name.to_string(),
        source_hash,
        cache_identity: pre_optimization_module_cache_identity(
            build_identity,
            runtime_names_as_globals,
        ),
    }
}

pub fn pre_optimization_module_cache_path(
    cache_root: &Path,
    source: PythonModuleCacheSource,
    module_name: &str,
    _source_hash: u64,
    _build_identity: &str,
    _runtime_names_as_globals: bool,
) -> Result<PathBuf, String> {
    codegen_module_cache_path(cache_root, source, module_name).map_err(|err| err.to_string())
}

pub fn compile_mode_from_env() -> Result<CompileMode, String> {
    Ok(SoacEnvConfig::from_env()?.compile_mode())
}

pub fn eager_clif_compile_requested_from_env() -> Result<bool, String> {
    Ok(SoacEnvConfig::from_env()?.eager_clif_compile_requested())
}

pub fn background_jit_enabled_from_env() -> Result<bool, String> {
    Ok(SoacEnvConfig::from_env()?.background_jit_enabled())
}

pub fn jit_compile_workers_from_env() -> Result<Option<usize>, String> {
    Ok(SoacEnvConfig::from_env()?.jit_compile_workers())
}

pub fn jit_perf_helper_frames_enabled_from_env() -> Result<bool, String> {
    Ok(SoacEnvConfig::from_env()?.jit_perf_helper_frames_enabled())
}

pub fn soac_exec_trace_from_env() -> Result<Option<String>, String> {
    Ok(SoacEnvConfig::from_env()?
        .soac_exec_trace()
        .map(ToString::to_string))
}

pub fn soac_log_config_from_env() -> Result<SoacLogConfig, String> {
    Ok(SoacEnvConfig::from_env()?.soac_log().clone())
}

fn parse_soac_log_config(
    raw: Option<&str>,
    soac_work_dir: Option<&Path>,
) -> Result<SoacLogConfig, String> {
    let raw = raw.unwrap_or_default();
    let mut filter_segments = Vec::new();
    let mut json_path = None;
    for segment in raw.split(';') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        if let Some(path) = segment.strip_prefix("json=") {
            let path = path.trim();
            if path.is_empty() {
                return Err(invalid_env_value(
                    SOAC_LOG_ENV,
                    raw,
                    "json= segment must include a non-empty path",
                ));
            }
            json_path = Some(PathBuf::from(path));
        } else {
            filter_segments.push(segment);
        }
    }
    if raw.trim().is_empty() {
        if let Some(work_dir) = soac_work_dir {
            json_path = Some(work_dir.join("events.jsonl"));
            filter_segments.push(DEFAULT_SOAC_JSON_LOG_FILTER);
        }
    }
    Ok(SoacLogConfig {
        filter: filter_segments.join(","),
        json_path,
    })
}

pub fn specialization_runtime_logging_enabled_from_env() -> Result<bool, String> {
    Ok(SoacEnvConfig::from_env()?.specialization_runtime_logging_enabled())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let previous = env::var_os(name);
            env::set_var(name, value);
            Self { name, previous }
        }

        fn remove(name: &'static str) -> Self {
            let previous = env::var_os(name);
            env::remove_var(name);
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => env::set_var(self.name, value),
                None => env::remove_var(self.name),
            }
        }
    }

    fn clear_soac_config_env() -> Vec<EnvVarGuard> {
        vec![
            EnvVarGuard::remove(SOAC_OPT_MODE_ENV),
            EnvVarGuard::remove(SOAC_WORK_DIR_ENV),
            EnvVarGuard::remove(SOAC_CRANELIFT_OPT_LEVEL_ENV),
            EnvVarGuard::remove(SOAC_ENABLE_PROFILED_COLD_BLOCKS_ENV),
            EnvVarGuard::remove(SOAC_JIT_EMIT_REFCOUNTS_ENV),
            EnvVarGuard::remove(SOAC_MODULE_CACHE_DIR_ENV),
            EnvVarGuard::remove(SOAC_PRECOMPILED_LIBRARY_ENV),
            EnvVarGuard::remove(SOAC_COMPILE_MODE_ENV),
            EnvVarGuard::remove(SOAC_JIT_COMPILE_WORKERS_ENV),
            EnvVarGuard::remove(SOAC_BACKGROUND_JIT_ENV),
            EnvVarGuard::remove(SOAC_JIT_PERF_HELPER_FRAMES_ENV),
            EnvVarGuard::remove(SOAC_LOG_ENV),
            EnvVarGuard::remove(SOAC_EXEC_TRACE_ENV),
        ]
    }

    #[test]
    fn specialization_mode_from_str_rejects_unknown_values() {
        assert_eq!(
            SpecializationMode::from_str("profile").unwrap(),
            Some(SpecializationMode::Profile)
        );
        assert_eq!(SpecializationMode::from_str("none").unwrap(), None);
        assert!(SpecializationMode::from_str("").is_err());
        assert!(SpecializationMode::from_str("bogus").is_err());
    }

    #[test]
    fn env_config_defaults_only_when_vars_are_absent() {
        let _lock = env_lock().lock().unwrap();
        let _guards = clear_soac_config_env();

        let config = SoacEnvConfig::from_env().unwrap();

        assert_eq!(config.specialization_mode(), None);
        assert_eq!(config.cranelift_opt_level(), "speed");
        assert_eq!(config.compile_mode(), CompileMode::Lazy);
        assert_eq!(config.jit_compile_workers(), None);
        assert!(config.background_jit_enabled());
        assert!(config.jit_refcount_emission_enabled());
        assert!(!config.jit_perf_helper_frames_enabled());
    }

    #[test]
    fn env_config_accepts_jit_compile_worker_count() {
        let _lock = env_lock().lock().unwrap();
        let _guards = clear_soac_config_env();
        let _workers = EnvVarGuard::set(SOAC_JIT_COMPILE_WORKERS_ENV, "3");

        let config = SoacEnvConfig::from_env().unwrap();

        assert_eq!(config.jit_compile_workers(), Some(3));
    }

    #[test]
    fn env_config_rejects_present_unknown_values() {
        let _lock = env_lock().lock().unwrap();

        for (name, value) in [
            (SOAC_OPT_MODE_ENV, "bogus"),
            (SOAC_CRANELIFT_OPT_LEVEL_ENV, "fastest"),
            (SOAC_ENABLE_PROFILED_COLD_BLOCKS_ENV, "maybe"),
            (SOAC_JIT_EMIT_REFCOUNTS_ENV, ""),
            (SOAC_COMPILE_MODE_ENV, "always"),
            (SOAC_JIT_COMPILE_WORKERS_ENV, "0"),
            (SOAC_BACKGROUND_JIT_ENV, "sometimes"),
            (SOAC_JIT_PERF_HELPER_FRAMES_ENV, "2"),
        ] {
            let _guards = clear_soac_config_env();
            let _invalid = EnvVarGuard::set(name, value);
            let err = SoacEnvConfig::from_env().unwrap_err();
            assert!(
                err.contains(name),
                "error {err:?} should identify invalid env var {name}"
            );
        }
    }
}
