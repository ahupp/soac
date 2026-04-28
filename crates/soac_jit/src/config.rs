use cranelift_codegen::isa::TargetIsa;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use soac_config::SoacEnvConfig;
pub(crate) use soac_config::SpecializationMode;
pub use soac_driver::blockpy_cache::CachedBlockPyModuleMetadata;
use soac_driver::blockpy_cache::{
    pre_optimization_module_cache_identity as blockpy_pre_optimization_module_cache_identity,
    pre_optimization_module_cache_metadata as blockpy_pre_optimization_module_cache_metadata,
    pre_optimization_module_cache_path as blockpy_pre_optimization_module_cache_path,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct CraneliftTargetConfig {
    opt_level: String,
    is_pic: bool,
    preserve_frame_pointers: bool,
    machine_code_cfg_info: bool,
}

impl CraneliftTargetConfig {
    pub(crate) fn runtime(config: &SoacEnvConfig) -> Self {
        Self::from_config_with_pic(config, false)
    }

    pub(crate) fn object(config: &SoacEnvConfig) -> Self {
        Self::from_config_with_pic(config, true)
    }

    fn from_config_with_pic(config: &SoacEnvConfig, is_pic: bool) -> Self {
        Self {
            opt_level: config.cranelift_opt_level().to_string(),
            is_pic,
            preserve_frame_pointers: true,
            machine_code_cfg_info: true,
        }
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
) -> CachedBlockPyModuleMetadata {
    blockpy_pre_optimization_module_cache_metadata(
        source,
        module_name,
        source_hash,
        build_identity,
        runtime_names_as_globals,
    )
}

pub(crate) fn precompiled_library_path() -> Result<Option<PathBuf>, String> {
    Ok(SoacEnvConfig::from_env()?
        .precompiled_library_path()
        .map(Path::to_path_buf))
}
pub use soac_driver::blockpy_cache::PythonModuleCacheSource;
