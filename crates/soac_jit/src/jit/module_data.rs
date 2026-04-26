use super::codegen_env::JitCodegenEnv;
use super::symbols::{push_symbol_component_hex, register_jit_data_symbol};
use crate::counter::TopValueCounter;
use crate::module_constants::ModuleConstantId;
use crate::module_type::SharedModuleState;
use cranelift_jit::JITModule;
use cranelift_module::{DataDescription, DataId, Linkage, Module};
use pyo3::ffi;
use soac_core::block_py::{
    BlockPyModule, LocalFunctionId, ModuleContentId, ModuleShape, PersistentFunctionId,
    RuntimeFunctionId,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ModuleConstantAccess {
    #[default]
    SymbolAddress,
    PointerSlot,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ModuleConstantAccessTable {
    entries: Option<Arc<[ModuleConstantAccess]>>,
}

impl ModuleConstantAccessTable {
    pub(super) fn from_entries(entries: Vec<ModuleConstantAccess>) -> Self {
        Self {
            entries: Some(Arc::from(entries)),
        }
    }

    pub(super) fn access(&self, constant_id: ModuleConstantId) -> ModuleConstantAccess {
        self.entries
            .as_ref()
            .and_then(|entries| entries.get(constant_id.0).copied())
            .unwrap_or_default()
    }
}

fn module_constant_symbol_prefix<P: ModuleShape>(module: &BlockPyModule<P>) -> String {
    format!(
        "__soac_module_constant_{}",
        module.module_name_gen.module_id()
    )
}

pub(super) fn module_constant_symbol_prefix_for_instance<P: ModuleShape>(
    module: &BlockPyModule<P>,
    instance_key: usize,
) -> String {
    format!("{}_{}", module_constant_symbol_prefix(module), instance_key)
}

pub(super) fn scalar_counter_storage_symbol<P: ModuleShape>(module: &BlockPyModule<P>) -> String {
    format!(
        "__soac_scalar_counters_{}",
        module.module_name_gen.module_id()
    )
}

pub(super) fn scalar_counter_storage_symbol_for_instance<P: ModuleShape>(
    module: &BlockPyModule<P>,
    instance_key: usize,
) -> String {
    format!("{}_{}", scalar_counter_storage_symbol(module), instance_key)
}

pub(super) fn top_value_counter_storage_symbol<P: ModuleShape>(
    module: &BlockPyModule<P>,
) -> String {
    format!(
        "__soac_top_value_counters_{}",
        module.module_name_gen.module_id()
    )
}

pub(super) fn top_value_counter_storage_symbol_for_instance<P: ModuleShape>(
    module: &BlockPyModule<P>,
    instance_key: usize,
) -> String {
    format!(
        "{}_{}",
        top_value_counter_storage_symbol(module),
        instance_key
    )
}

pub(super) fn push_shared_module_symbol_identity(
    out: &mut String,
    module_name: &str,
    source_hash: u64,
    fallback_instance_key: Option<usize>,
) {
    push_symbol_component_hex(out, module_name);
    out.push('_');
    out.push_str(format!("{source_hash:016x}").as_str());
    if source_hash == 0 {
        if let Some(instance_key) = fallback_instance_key {
            out.push_str("_inst_");
            out.push_str(instance_key.to_string().as_str());
        }
    }
}

fn push_shared_module_symbol_identity_for_shared_state(
    out: &mut String,
    shared_state: &SharedModuleState,
) {
    push_shared_module_symbol_identity(
        out,
        shared_state.module_name.as_str(),
        shared_state.source_hash(),
        Some(shared_state.storage_instance_key()),
    );
}

pub(super) fn module_constant_symbol_prefix_for_shared_state(
    shared_state: &SharedModuleState,
) -> String {
    let mut symbol = String::from("__soac_module_constant_shared_");
    push_shared_module_symbol_identity_for_shared_state(&mut symbol, shared_state);
    symbol
}

pub(super) fn module_constant_symbol_prefix_for_module_identity(
    module_name: &str,
    source_hash: u64,
) -> String {
    let mut symbol = String::from("__soac_module_constant_shared_");
    push_shared_module_symbol_identity(&mut symbol, module_name, source_hash, None);
    symbol
}

pub(super) fn scalar_counter_storage_symbol_for_shared_state(
    shared_state: &SharedModuleState,
) -> String {
    let mut symbol = String::from("__soac_scalar_counters_shared_");
    push_shared_module_symbol_identity_for_shared_state(&mut symbol, shared_state);
    symbol
}

pub(super) fn top_value_counter_storage_symbol_for_shared_state(
    shared_state: &SharedModuleState,
) -> String {
    let mut symbol = String::from("__soac_top_value_counters_shared_");
    push_shared_module_symbol_identity_for_shared_state(&mut symbol, shared_state);
    symbol
}

pub(super) fn direct_function_symbol_scope_for_shared_state(
    shared_state: &SharedModuleState,
    function_id: RuntimeFunctionId,
) -> String {
    let mut scope = String::from("shared_");
    push_shared_module_symbol_identity_for_shared_state(&mut scope, shared_state);
    scope.push_str("_fn_");
    scope.push_str(function_id.to_packed_runtime_u64().to_string().as_str());
    scope
}

pub(super) fn precompiled_direct_function_symbol_scope_for_shared_state(
    shared_state: &SharedModuleState,
    function_id: RuntimeFunctionId,
) -> String {
    let persistent = persistent_function_id_for_module_function(
        shared_state.module_name.as_str(),
        shared_state.source_hash(),
        function_id.local_function_id(),
    );
    precompiled_direct_function_symbol_scope_for_persistent(&persistent)
}

fn module_content_id_for_module_identity(module_name: &str, source_hash: u64) -> ModuleContentId {
    ModuleContentId::new(module_name, source_hash)
}

pub(super) fn persistent_function_id_for_module_function(
    module_name: &str,
    source_hash: u64,
    local_function_id: LocalFunctionId,
) -> PersistentFunctionId {
    PersistentFunctionId::new(
        module_content_id_for_module_identity(module_name, source_hash),
        local_function_id,
    )
}

pub(super) fn precompiled_direct_function_symbol_scope_for_persistent(
    function: &PersistentFunctionId,
) -> String {
    let mut scope = String::from("shared_");
    push_shared_module_symbol_identity(
        &mut scope,
        function.module.module_name.as_str(),
        function.module.source_hash,
        None,
    );
    scope.push_str("_fn_");
    scope.push_str(function.local.as_u32().to_string().as_str());
    scope
}

pub(super) fn module_constant_object_symbol(
    symbol_prefix: &str,
    constant_id: ModuleConstantId,
) -> String {
    format!("{symbol_prefix}_object_{}", constant_id.0)
}

fn declare_module_constant_object_data_for_symbol(
    jit_module: &mut JITModule,
    symbol_prefix: &str,
    constant_id: ModuleConstantId,
    module_constant_ptr: *mut ffi::PyObject,
) -> Result<DataId, String> {
    let symbol = module_constant_object_symbol(symbol_prefix, constant_id);
    register_jit_data_symbol(symbol.as_str(), module_constant_ptr.cast::<u8>());
    jit_module
        .codegen_declare_data(symbol.as_str(), Linkage::Import, true, false)
        .map_err(|err| format!("failed to declare module constant object {symbol}: {err}"))
}

pub(super) fn declare_module_constant_object_data(
    jit_module: &mut JITModule,
    module: &BlockPyModule<impl ModuleShape>,
    module_constant_ptrs: &[*mut ffi::PyObject],
) -> Result<Vec<DataId>, String> {
    let instance_key = std::ptr::from_ref(module).cast::<()>() as usize;
    let symbol_prefix = module_constant_symbol_prefix_for_instance(module, instance_key);
    declare_module_constant_object_data_for_prefix(
        jit_module,
        symbol_prefix.as_str(),
        module_constant_ptrs,
    )
}

pub(super) fn declare_module_constant_object_data_for_prefix(
    jit_module: &mut JITModule,
    symbol_prefix: &str,
    module_constant_ptrs: &[*mut ffi::PyObject],
) -> Result<Vec<DataId>, String> {
    module_constant_ptrs
        .iter()
        .copied()
        .enumerate()
        .map(|(index, ptr)| {
            declare_module_constant_object_data_for_symbol(
                jit_module,
                symbol_prefix,
                ModuleConstantId(index),
                ptr,
            )
        })
        .collect()
}

pub(super) fn define_scalar_counter_storage_data_for_symbol(
    jit_module: &mut JITModule,
    symbol: &str,
    scalar_counter_count: usize,
) -> Result<DataId, String> {
    let data_id = jit_module
        .codegen_declare_data(symbol, Linkage::Local, true, false)
        .map_err(|err| format!("failed to declare scalar counter storage {symbol}: {err}"))?;
    let mut data = DataDescription::new();
    data.define_zeroinit(
        scalar_counter_count
            .checked_mul(std::mem::size_of::<u64>())
            .ok_or_else(|| {
                format!("scalar counter storage size overflow for {symbol}: {scalar_counter_count}")
            })?,
    );
    data.set_align(std::mem::align_of::<u64>() as u64);
    jit_module
        .define_data(data_id, &data)
        .map_err(|err| format!("failed to define scalar counter storage {symbol}: {err}"))?;
    Ok(data_id)
}

pub(super) fn define_scalar_counter_storage_data(
    jit_module: &mut JITModule,
    module: &BlockPyModule<impl ModuleShape>,
    scalar_counter_count: usize,
) -> Result<DataId, String> {
    define_scalar_counter_storage_data_for_symbol(
        jit_module,
        scalar_counter_storage_symbol(module).as_str(),
        scalar_counter_count,
    )
}

pub(super) fn declare_scalar_counter_storage_import(
    jit_module: &mut JITModule,
    symbol: &str,
) -> Result<DataId, String> {
    jit_module
        .codegen_declare_data(symbol, Linkage::Import, true, false)
        .map_err(|err| format!("failed to declare imported scalar counter storage {symbol}: {err}"))
}

pub(super) fn define_top_value_counter_storage_data_for_symbol(
    jit_module: &mut JITModule,
    symbol: &str,
    top_value_counter_count: usize,
) -> Result<DataId, String> {
    let data_id = jit_module
        .codegen_declare_data(symbol, Linkage::Local, true, false)
        .map_err(|err| format!("failed to declare top-value counter storage {symbol}: {err}"))?;
    let mut data = DataDescription::new();
    data.define_zeroinit(
        top_value_counter_count
            .checked_mul(std::mem::size_of::<TopValueCounter>())
            .ok_or_else(|| {
                format!(
                    "top-value counter storage size overflow for {symbol}: {top_value_counter_count}"
                )
            })?,
    );
    data.set_align(std::mem::align_of::<TopValueCounter>() as u64);
    jit_module
        .define_data(data_id, &data)
        .map_err(|err| format!("failed to define top-value counter storage {symbol}: {err}"))?;
    Ok(data_id)
}

pub(super) fn define_top_value_counter_storage_data(
    jit_module: &mut JITModule,
    module: &BlockPyModule<impl ModuleShape>,
    top_value_counter_count: usize,
) -> Result<DataId, String> {
    define_top_value_counter_storage_data_for_symbol(
        jit_module,
        top_value_counter_storage_symbol(module).as_str(),
        top_value_counter_count,
    )
}

pub(super) fn declare_top_value_counter_storage_import(
    jit_module: &mut JITModule,
    symbol: &str,
) -> Result<DataId, String> {
    jit_module
        .codegen_declare_data(symbol, Linkage::Import, true, false)
        .map_err(|err| {
            format!("failed to declare imported top-value counter storage {symbol}: {err}")
        })
}

pub(super) fn declare_type_ptr_import(
    codegen_env: &mut impl JitCodegenEnv,
    symbol: &str,
) -> Result<DataId, String> {
    codegen_env
        .codegen_declare_data(symbol, Linkage::Import, true, false)
        .map_err(|err| format!("failed to declare imported type symbol {symbol}: {err}"))
}
