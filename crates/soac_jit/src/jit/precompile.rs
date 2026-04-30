use super::backend::{compile_prepared_function_bytes_with_isa, new_jit_builder};
use super::codegen_env::JitCodegenEnv;
use super::direct_function::{
    build_default_resolving_direct_adapter, declare_direct_function,
    declare_imported_direct_function,
};
use super::function_targets::collect_typed_call_direct_targets;
use super::module_data::{
    ModuleConstantAccess, ModuleConstantAccessTable,
    declare_module_constant_object_data_for_prefix, define_scalar_counter_storage_data,
    define_top_value_counter_storage_data, module_constant_object_symbol,
    module_constant_symbol_prefix_for_module_identity, persistent_function_id_for_module_function,
    precompiled_direct_function_symbol_scope_for_persistent, scalar_counter_storage_symbol,
    top_value_counter_storage_symbol,
};
use super::precompiled_object::{
    ElfSymbolBinding, ElfSymbolKind, ObjectDataDefinition, ObjectDataRelocation,
    ObjectFunctionDefinition, R_X86_64_64, write_precompiled_object,
};
use super::runtime_support::compile_runtime_support_clif_for_object;
use super::symbols::direct_function_backend_name;
use super::typed_pipeline::{
    apply_profile_call_emission_plans_to_typed_function, optimize_blockpy,
};
use super::{
    BuildSpecializedFunctionOptions, SpecializationProfile,
    build_cranelift_run_bb_specialized_function, placeholder_module_constant_ptrs,
};
use crate::config::{CraneliftTargetConfig, pre_optimization_module_cache_identity};
use crate::counter::TopValueCounter;
use crate::module_constants::{ModuleCodegenConstants, ModuleConstantId};
use crate::module_type::build_counter_storage_layout;
use cranelift_jit::JITModule;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, PersistentFunctionId, RuntimeFunctionId, RuntimeModuleId,
};
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::{TypedBlockPyModuleShape, lower_blockpy_function_to_typed};
use soac_opt::passes::lower_typed_function_call_access_plan_instrs;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PrecompileObjectSummary {
    pub output_path: PathBuf,
    pub function_count: usize,
    pub data_object_count: usize,
    pub object_size_bytes: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct PrecompileModuleIndexEntry<'a> {
    pub module_name: &'a str,
    pub source_hash: u64,
    pub module: &'a BlockPyModule<BlockPyModuleShape>,
}

#[derive(Debug, Clone)]
struct PrecompileIndexedFunction {
    module_name: String,
    source_hash: u64,
    persistent_id: PersistentFunctionId,
    function: BlockPyFunction<BlockPyModuleShape>,
}

#[derive(Debug, Clone)]
struct PrecompileIndexedModule {
    module_id: RuntimeModuleId,
}

#[derive(Debug, Clone, Default)]
pub struct PrecompileModuleIndex {
    modules_by_identity: HashMap<(String, u64), PrecompileIndexedModule>,
    functions_by_id: HashMap<RuntimeFunctionId, PrecompileIndexedFunction>,
    ambiguous_function_ids: HashSet<RuntimeFunctionId>,
}

impl PrecompileModuleIndex {
    pub fn from_entries<'a>(
        entries: impl IntoIterator<Item = PrecompileModuleIndexEntry<'a>>,
    ) -> Result<Self, String> {
        let mut index = Self::default();
        for entry in entries {
            index.insert(entry)?;
        }
        Ok(index)
    }

    fn insert(&mut self, entry: PrecompileModuleIndexEntry<'_>) -> Result<(), String> {
        let identity = (entry.module_name.to_string(), entry.source_hash);
        let module_id = entry.module.module_name_gen.runtime_module_id();
        if self
            .modules_by_identity
            .insert(identity.clone(), PrecompileIndexedModule { module_id })
            .is_some()
        {
            return Err(format!(
                "duplicate precompile module identity: module={} source_hash=0x{:016x}",
                entry.module_name, entry.source_hash
            ));
        }
        for function in &entry.module.callable_defs {
            let indexed = PrecompileIndexedFunction {
                module_name: entry.module_name.to_string(),
                source_hash: entry.source_hash,
                persistent_id: persistent_function_id_for_module_function(
                    entry.module_name,
                    entry.source_hash,
                    function.function_id.local_function_id(),
                ),
                function: function.clone(),
            };
            if let Some(previous) = self.functions_by_id.insert(function.function_id, indexed)
                && (previous.module_name != entry.module_name
                    || previous.source_hash != entry.source_hash)
            {
                self.functions_by_id.remove(&function.function_id);
                self.ambiguous_function_ids.insert(function.function_id);
            }
        }
        Ok(())
    }

    pub(super) fn function_id_for_target(
        &self,
        target: &PersistentFunctionId,
    ) -> Option<RuntimeFunctionId> {
        let module = self
            .modules_by_identity
            .get(&(target.module.module_name.clone(), target.module.source_hash))?;
        let function_id = RuntimeFunctionId::new(module.module_id, target.local);
        self.function(function_id).map(|_| function_id)
    }

    fn function(&self, function_id: RuntimeFunctionId) -> Option<&PrecompileIndexedFunction> {
        if self.ambiguous_function_ids.contains(&function_id) {
            return None;
        }
        self.functions_by_id.get(&function_id)
    }

    fn precompiled_symbol_scope_for_function(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Option<String> {
        let function = self.function(function_id)?;
        Some(precompiled_direct_function_symbol_scope_for_persistent(
            &function.persistent_id,
        ))
    }
}

fn precompile_external_direct_call_target_functions(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    module_index: Option<&PrecompileModuleIndex>,
) -> Result<HashMap<RuntimeFunctionId, BlockPyFunction<TypedBlockPyModuleShape>>, String> {
    let Some(module_index) = module_index else {
        return Ok(HashMap::new());
    };
    let current_module_id = module.module_name_gen.module_id();
    let mut target_ids = HashSet::new();
    for function in &module.callable_defs {
        let mut typed_function = function.clone();
        apply_profile_call_emission_plans_to_typed_function(&mut typed_function, profile)?;
        lower_typed_function_call_access_plan_instrs(&mut typed_function);
        target_ids.extend(collect_typed_call_direct_targets(&typed_function));
    }
    Ok(target_ids
        .into_iter()
        .filter(|function_id| function_id.runtime_module_id().as_u32() != current_module_id)
        .filter_map(|function_id| {
            module_index.function(function_id).map(|target| {
                (
                    function_id,
                    lower_blockpy_function_to_typed(target.function.clone()),
                )
            })
        })
        .collect())
}

pub fn precompile_codegen_module_to_object_file(
    module_name: &str,
    source_hash: u64,
    module: &BlockPyModule<BlockPyModuleShape>,
    counter_dump_path: Option<&Path>,
    cache_identity: Option<&str>,
    module_index: Option<&PrecompileModuleIndex>,
    output_path: &Path,
) -> Result<PrecompileObjectSummary, String> {
    let bytes = precompile_codegen_module_to_object_bytes(
        module_name,
        source_hash,
        module,
        counter_dump_path,
        cache_identity,
        module_index,
    )?;
    if let Some(parent) = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create object output dir {}: {err}",
                parent.display()
            )
        })?;
    }
    fs::write(output_path, bytes.object.as_slice()).map_err(|err| {
        format!(
            "failed to write object file {}: {err}",
            output_path.display()
        )
    })?;
    Ok(PrecompileObjectSummary {
        output_path: output_path.to_path_buf(),
        function_count: bytes.function_count,
        data_object_count: bytes.data_object_count,
        object_size_bytes: bytes.object.len(),
    })
}

pub(super) struct PrecompiledObjectBytes {
    pub(super) object: Vec<u8>,
    pub(super) function_count: usize,
    pub(super) data_object_count: usize,
    #[cfg(test)]
    pub(super) function_symbols: Vec<String>,
    #[cfg(test)]
    pub(super) data_symbols: Vec<String>,
    #[cfg(test)]
    pub(super) data_symbol_writable: Vec<(String, bool)>,
}

pub(super) fn precompile_codegen_module_to_object_bytes(
    module_name: &str,
    source_hash: u64,
    module: &BlockPyModule<BlockPyModuleShape>,
    counter_dump_path: Option<&Path>,
    cache_identity: Option<&str>,
    module_index: Option<&PrecompileModuleIndex>,
) -> Result<PrecompiledObjectBytes, String> {
    let compile_session = crate::session::CompileSession::new();
    let env_config = compile_session.env_config()?;
    let object_isa = CraneliftTargetConfig::object(env_config).build_isa()?;
    let builder = new_jit_builder(env_config)?;
    let mut jit_module = JITModule::new(builder);
    let mut function_definitions =
        compile_runtime_support_clif_for_object(&mut jit_module, env_config, object_isa.as_ref())?;

    let module_constants = ModuleCodegenConstants::collect_from_module(module);
    let module_constant_ptrs = placeholder_module_constant_ptrs(module_constants.len());
    let module_constant_symbol_prefix =
        module_constant_symbol_prefix_for_module_identity(module_name, source_hash);
    let module_constant_object_data_ids = declare_module_constant_object_data_for_prefix(
        &mut jit_module,
        module_constant_symbol_prefix.as_str(),
        module_constant_ptrs.as_slice(),
    )?;

    let (counter_slots_by_id, scalar_counter_count, top_value_counter_count) =
        build_counter_storage_layout(module.counter_defs.as_slice())?;
    let scalar_counter_data_id = if scalar_counter_count == 0 {
        None
    } else {
        Some(define_scalar_counter_storage_data(
            &mut jit_module,
            module,
            scalar_counter_count,
        )?)
    };
    let top_value_counter_data_id = if top_value_counter_count == 0 {
        None
    } else {
        Some(define_top_value_counter_storage_data(
            &mut jit_module,
            module,
            top_value_counter_count,
        )?)
    };

    let mut data_definitions = Vec::new();
    let mut module_constant_accesses = Vec::with_capacity(module_constants.len());
    for (index, data_id) in module_constant_object_data_ids.iter().copied().enumerate() {
        let constant_id = ModuleConstantId(index);
        let symbol =
            module_constant_object_symbol(module_constant_symbol_prefix.as_str(), constant_id);
        if let Some(image) = module_constants.static_pyobject_image(constant_id) {
            module_constant_accesses.push(ModuleConstantAccess::SymbolAddress);
            data_definitions.push(ObjectDataDefinition {
                data_id,
                symbol,
                binding: ElfSymbolBinding::Global,
                bytes: image.bytes,
                align: image.align,
                writable: image.writable,
                relocations: image
                    .relocations
                    .into_iter()
                    .map(|relocation| ObjectDataRelocation {
                        offset: relocation.offset,
                        symbol: relocation.symbol.to_string(),
                        kind: ElfSymbolKind::Object,
                        reloc_type: R_X86_64_64,
                        addend: 0,
                    })
                    .collect(),
            });
        } else {
            module_constant_accesses.push(ModuleConstantAccess::PointerSlot);
            data_definitions.push(ObjectDataDefinition {
                data_id,
                symbol,
                binding: ElfSymbolBinding::Global,
                bytes: vec![0; std::mem::size_of::<usize>()],
                align: std::mem::align_of::<usize>() as u64,
                writable: true,
                relocations: Vec::new(),
            });
        }
    }
    let module_constant_access_table =
        ModuleConstantAccessTable::from_entries(module_constant_accesses);
    if let Some(data_id) = scalar_counter_data_id {
        data_definitions.push(ObjectDataDefinition {
            data_id,
            symbol: scalar_counter_storage_symbol(module),
            binding: ElfSymbolBinding::Global,
            bytes: vec![
                0;
                scalar_counter_count
                    .checked_mul(std::mem::size_of::<u64>())
                    .ok_or_else(|| format!(
                        "scalar counter storage size overflow: {scalar_counter_count}"
                    ))?
            ],
            align: std::mem::align_of::<u64>() as u64,
            writable: true,
            relocations: Vec::new(),
        });
    }
    if let Some(data_id) = top_value_counter_data_id {
        data_definitions.push(ObjectDataDefinition {
            data_id,
            symbol: top_value_counter_storage_symbol(module),
            binding: ElfSymbolBinding::Global,
            bytes: vec![
                0;
                top_value_counter_count
                    .checked_mul(std::mem::size_of::<TopValueCounter>())
                    .ok_or_else(|| format!(
                        "top-value counter storage size overflow: {top_value_counter_count}"
                    ))?
            ],
            align: std::mem::align_of::<TopValueCounter>() as u64,
            writable: true,
            relocations: Vec::new(),
        });
    }

    let default_cache_identity;
    let cache_identity = match cache_identity {
        Some(cache_identity) => cache_identity,
        None => {
            default_cache_identity = pre_optimization_module_cache_identity(
                env!("SOAC_BUILD_IDENTITY"),
                module_name == "soac.runtime",
            );
            default_cache_identity.as_str()
        }
    };
    let specialization_profile = SpecializationProfile::from_precompile(
        env_config,
        module_name,
        source_hash,
        cache_identity,
        module,
        module_index,
        counter_dump_path,
    )?;
    let jit_module_plan = optimize_blockpy(module, Some(&specialization_profile), env_config)?;
    let planned_module = jit_module_plan.module.as_ref();
    let external_direct_call_target_functions = precompile_external_direct_call_target_functions(
        planned_module,
        &specialization_profile,
        module_index,
    )?;
    let mut predeclared = HashMap::new();
    let mut symbol_scopes = HashMap::new();
    for function in &planned_module.callable_defs {
        let persistent_id = persistent_function_id_for_module_function(
            module_name,
            source_hash,
            function.function_id.local_function_id(),
        );
        let symbol_scope = precompiled_direct_function_symbol_scope_for_persistent(&persistent_id);
        let (_sig, declared) =
            declare_direct_function(&mut jit_module, function, Some(symbol_scope.as_str()))?;
        predeclared.insert(function.function_id, declared);
        symbol_scopes.insert(function.function_id, symbol_scope);
    }
    if let Some(module_index) = module_index {
        for (function_id, function) in &external_direct_call_target_functions {
            if predeclared.contains_key(function_id) {
                continue;
            }
            let Some(symbol_scope) =
                module_index.precompiled_symbol_scope_for_function(*function_id)
            else {
                continue;
            };
            let declared =
                declare_imported_direct_function(&mut jit_module, function, symbol_scope.as_str())?;
            predeclared.insert(*function_id, declared);
        }
    }
    for function in &planned_module.callable_defs {
        let placeholder_blocks =
            vec![std::ptr::null_mut::<std::ffi::c_void>(); function.blocks.len()];
        let jit_local_plan = jit_module_plan
            .locals
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT local plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let jit_deopt_resume_plan = jit_module_plan
            .deopt_resume
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT deopt resume plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let built = build_cranelift_run_bb_specialized_function(
            &mut jit_module,
            placeholder_blocks.as_slice(),
            planned_module,
            function,
            &jit_module_plan.value_facts,
            jit_local_plan,
            jit_deopt_resume_plan,
            &module_constants,
            planned_module.counter_defs.as_slice(),
            module_constant_object_data_ids.as_slice(),
            counter_slots_by_id.as_ref(),
            scalar_counter_data_id,
            top_value_counter_data_id,
            &compile_session,
            None,
            symbol_scopes.get(&function.function_id).map(String::as_str),
            Some(&predeclared),
            BuildSpecializedFunctionOptions {
                module_constant_accesses: module_constant_access_table.clone(),
                external_direct_call_target_functions: external_direct_call_target_functions
                    .clone(),
                ..BuildSpecializedFunctionOptions::default()
            },
        )
        .map_err(|err| {
            format!(
                "{err} [function={} id={}]",
                function.names.qualname, function.function_id
            )
        })?;
        let mut ctx = built.ctx;
        let compiled = compile_prepared_function_bytes_with_isa(
            &mut jit_module,
            env_config,
            object_isa.as_ref(),
            built.main_id,
            &mut ctx,
            direct_function_backend_name(function, None).as_str(),
            "failed to compile specialized jit run_bb function to object",
        )
        .map_err(|err| {
            format!(
                "{err} [function={} id={}]",
                function.names.qualname, function.function_id
            )
        })?;
        jit_module.codegen_clear_context(&mut ctx);
        function_definitions.push(ObjectFunctionDefinition {
            func_id: built.main_id,
            symbol: built.main_symbol,
            binding: ElfSymbolBinding::Global,
            bytes: compiled.bytes,
            systemv_unwind_info: compiled.artifact.systemv_unwind_info,
        });
        match (
            built.default_adapter_id,
            built.default_adapter_symbol.as_ref(),
        ) {
            (Some(default_adapter_id), Some(default_adapter_symbol)) => {
                let mut default_ctx = build_default_resolving_direct_adapter(
                    &mut jit_module,
                    function,
                    built.main_id,
                    default_adapter_id,
                )
                .map_err(|err| {
                    format!(
                        "{err} [default-adapter function={} id={}]",
                        function.names.qualname, function.function_id
                    )
                })?;
                let compiled = compile_prepared_function_bytes_with_isa(
                    &mut jit_module,
                    env_config,
                    object_isa.as_ref(),
                    default_adapter_id,
                    &mut default_ctx,
                    default_adapter_symbol.as_str(),
                    "failed to compile default-resolving direct adapter to object",
                )
                .map_err(|err| {
                    format!(
                        "{err} [default-adapter function={} id={}]",
                        function.names.qualname, function.function_id
                    )
                })?;
                jit_module.codegen_clear_context(&mut default_ctx);
                function_definitions.push(ObjectFunctionDefinition {
                    func_id: default_adapter_id,
                    symbol: default_adapter_symbol.clone(),
                    binding: ElfSymbolBinding::Global,
                    bytes: compiled.bytes,
                    systemv_unwind_info: compiled.artifact.systemv_unwind_info,
                });
            }
            (None, None) => {}
            _ => {
                return Err(format!(
                    "default direct adapter declaration is inconsistent for function {} id={}",
                    function.names.qualname, function.function_id
                ));
            }
        }
    }

    let object = write_precompiled_object(
        &jit_module,
        object_isa.as_ref(),
        &function_definitions,
        &data_definitions,
    )?;
    Ok(PrecompiledObjectBytes {
        object,
        function_count: function_definitions.len(),
        data_object_count: data_definitions.len(),
        #[cfg(test)]
        function_symbols: function_definitions
            .iter()
            .map(|definition| definition.symbol.clone())
            .collect(),
        #[cfg(test)]
        data_symbols: data_definitions
            .iter()
            .map(|definition| definition.symbol.clone())
            .collect(),
        #[cfg(test)]
        data_symbol_writable: data_definitions
            .iter()
            .map(|definition| (definition.symbol.clone(), definition.writable))
            .collect(),
    })
}
