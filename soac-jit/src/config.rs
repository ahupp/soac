use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
#[cfg(test)]
pub(crate) use soac_config::SOAC_JIT_EMIT_REFCOUNTS_ENV;
pub(crate) use soac_config::SpecializationMode;
use soac_config::{
    background_jit_enabled_from_env,
    counter_dump_input_path_from_env as config_counter_dump_input_path_from_env,
    counter_dump_output_path_from_env as config_counter_dump_output_path_from_env,
    cranelift_opt_level_from_env, eager_clif_compile_requested_from_env,
    jit_compile_workers_from_env, jit_perf_helper_frames_enabled_from_env,
    jit_refcount_emission_enabled_from_env,
    module_cache_root_from_env_or_repo as config_module_cache_root_from_env_or_repo,
    precompiled_library_path_from_env as config_precompiled_library_path_from_env,
    profiled_cold_blocks_enabled_from_env, soac_work_dir_from_env as config_soac_work_dir_from_env,
    specialization_mode_from_env as config_specialization_mode_from_env,
};
pub use soac_lowering::codegen_cache::CachedCodegenModuleMetadata;
use soac_lowering::codegen_cache::{
    module_optimization_plan_path as blockpy_module_optimization_plan_path,
    pre_optimization_module_cache_identity as blockpy_pre_optimization_module_cache_identity,
    pre_optimization_module_cache_metadata as blockpy_pre_optimization_module_cache_metadata,
    pre_optimization_module_cache_path as blockpy_pre_optimization_module_cache_path,
};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct CraneliftTargetConfig {
    opt_level: String,
    is_pic: bool,
    preserve_frame_pointers: bool,
    machine_code_cfg_info: bool,
}

impl CraneliftTargetConfig {
    pub(crate) fn runtime_from_env() -> Result<Self, String> {
        Self::from_env_with_pic(false)
    }

    pub(crate) fn object_from_env() -> Result<Self, String> {
        Self::from_env_with_pic(true)
    }

    fn from_env_with_pic(is_pic: bool) -> Result<Self, String> {
        Ok(Self {
            opt_level: cranelift_opt_level_from_env()?,
            is_pic,
            preserve_frame_pointers: true,
            machine_code_cfg_info: true,
        })
    }

    pub(crate) fn build_isa(&self) -> Result<Arc<dyn TargetIsa>, String> {
        let mut flag_builder = settings::builder();
        self.set_flag(&mut flag_builder, "opt_level", self.opt_level.as_str())?;
        self.set_bool_flag(&mut flag_builder, "is_pic", self.is_pic)?;
        self.set_bool_flag(
            &mut flag_builder,
            "preserve_frame_pointers",
            self.preserve_frame_pointers,
        )?;
        self.set_bool_flag(
            &mut flag_builder,
            "machine_code_cfg_info",
            self.machine_code_cfg_info,
        )?;
        let isa_builder = cranelift_native::builder().map_err(|err| format!("{err}"))?;
        isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|err| format!("failed to finish ISA: {err}"))
    }

    fn set_bool_flag(
        &self,
        flag_builder: &mut settings::Builder,
        name: &str,
        enabled: bool,
    ) -> Result<(), String> {
        self.set_flag(flag_builder, name, if enabled { "true" } else { "false" })
    }

    fn set_flag(
        &self,
        flag_builder: &mut settings::Builder,
        name: &str,
        value: &str,
    ) -> Result<(), String> {
        flag_builder
            .set(name, value)
            .map_err(|err| format!("failed to configure Cranelift flags: {err}"))
    }
}

pub fn soac_work_dir_from_env() -> Result<Option<PathBuf>, String> {
    config_soac_work_dir_from_env()
}

pub fn counter_dump_input_path_from_env() -> Result<Option<PathBuf>, String> {
    config_counter_dump_input_path_from_env()
}

pub(crate) fn counter_dump_output_path_from_env() -> Result<Option<PathBuf>, String> {
    config_counter_dump_output_path_from_env()
}

pub(crate) fn profiled_cold_blocks_enabled() -> Result<bool, String> {
    profiled_cold_blocks_enabled_from_env()
}

pub(crate) fn jit_refcount_emission_enabled() -> Result<bool, String> {
    jit_refcount_emission_enabled_from_env()
}

pub fn module_cache_root_from_env_or_repo(
    repo_root: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    config_module_cache_root_from_env_or_repo(repo_root)
}

pub fn pre_optimization_module_cache_identity(
    build_identity: &str,
    runtime_names_as_globals: bool,
) -> String {
    blockpy_pre_optimization_module_cache_identity(build_identity, runtime_names_as_globals)
}

pub fn pre_optimization_module_cache_path(
    cache_root: &Path,
    source: PythonModuleCacheSource,
    module_name: &str,
    source_hash: u64,
    build_identity: &str,
    runtime_names_as_globals: bool,
) -> Result<PathBuf, String> {
    blockpy_pre_optimization_module_cache_path(
        cache_root,
        source,
        module_name,
        source_hash,
        build_identity,
        runtime_names_as_globals,
    )
}

pub fn pre_optimization_module_cache_metadata(
    source: PythonModuleCacheSource,
    module_name: &str,
    source_hash: u64,
    build_identity: &str,
    runtime_names_as_globals: bool,
) -> CachedCodegenModuleMetadata {
    blockpy_pre_optimization_module_cache_metadata(
        source,
        module_name,
        source_hash,
        build_identity,
        runtime_names_as_globals,
    )
}

pub fn module_optimization_plan_path(
    cache_root: &Path,
    source: PythonModuleCacheSource,
    module_name: &str,
) -> Result<PathBuf, String> {
    blockpy_module_optimization_plan_path(cache_root, source, module_name)
        .map_err(|err| err.to_string())
}

pub(crate) fn precompiled_library_path_from_env() -> Result<Option<PathBuf>, String> {
    config_precompiled_library_path_from_env()
}

pub fn eager_clif_compile_requested() -> Result<bool, String> {
    eager_clif_compile_requested_from_env()
}

pub(crate) fn jit_compile_workers() -> Result<Option<usize>, String> {
    jit_compile_workers_from_env()
}

pub(crate) fn background_jit_enabled() -> Result<bool, String> {
    background_jit_enabled_from_env()
}

pub(crate) fn jit_perf_helper_frames_enabled() -> Result<bool, String> {
    jit_perf_helper_frames_enabled_from_env()
}

#[cfg(test)]
pub(crate) fn specialization_mode_is_profile() -> Result<bool, String> {
    Ok(specialization_mode_from_env()? == Some(SpecializationMode::Profile))
}

pub(crate) fn behavior_change_indexed_stores_enabled() -> Result<bool, String> {
    Ok(specialization_mode_from_env()?
        .is_some_and(SpecializationMode::behavior_change_indexed_stores_enabled))
}

pub(crate) fn specialization_mode_from_env() -> Result<Option<SpecializationMode>, String> {
    config_specialization_mode_from_env()
}
pub use soac_lowering::codegen_cache::PythonModuleCacheSource;
