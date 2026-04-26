use crate::counter::TopValueCounter;
use crate::module_type::CounterRuntimeSlot;
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::JITModule;
use cranelift_module::{DataId, FuncId};
use soac_config::SoacEnvConfig;
use soac_core::block_py::ModuleShape;
use soac_core::block_py::{
    BlockPyFunction, CounterBranchId, CounterDef, CounterId, CounterScope, CounterSite,
    DeoptEntrySource, InstrId, RuntimeFunctionId,
};
use soac_opt::passes::LocalEnvResumePoint;
use std::collections::HashMap;

use super::backend::define_prepared_function;
use super::codegen_env::{
    FuncBuildImports, JitCodegenEnv, declare_local_fn, lower_static_signature,
};
use super::imports::{
    DP_JIT_DECREF_IMPORT, DP_JIT_INCREF_IMPORT, ImportSpec, ModuleFuncImports,
    SOAC_RUNTIME_DECREF_APPLIED_IMPORT, SOAC_RUNTIME_INCREF_APPLIED_IMPORT,
};
use super::planning::PlannedJitDeoptResumeFunction;
use super::symbols::scoped_jit_symbol;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct CounterRef {
    pub(super) counter_id: CounterId,
    branch_id: Option<CounterBranchId>,
}

impl CounterRef {
    pub(super) const fn branch(counter_id: CounterId, branch_id: CounterBranchId) -> Self {
        Self {
            counter_id,
            branch_id: Some(branch_id),
        }
    }
}

pub(super) fn scalar_counter_slot_for_id(
    counter_slots_by_id: &[CounterRuntimeSlot],
    counter_id: CounterId,
) -> Result<usize, String> {
    match counter_slots_by_id.get(counter_id.0).copied() {
        Some(CounterRuntimeSlot::Scalar(slot)) => Ok(slot),
        Some(CounterRuntimeSlot::Branches { .. }) => Err(format!(
            "counter id {} uses branch storage where a scalar counter was required",
            counter_id.0
        )),
        Some(CounterRuntimeSlot::TopValues(_)) => Err(format!(
            "counter id {} uses top-value storage where a scalar counter was required",
            counter_id.0
        )),
        None => Err(format!(
            "missing scalar counter slot for counter id {}",
            counter_id.0
        )),
    }
}

pub(super) fn scalar_counter_slot_for_ref(
    counter_slots_by_id: &[CounterRuntimeSlot],
    counter_ref: CounterRef,
) -> Result<usize, String> {
    match (
        counter_slots_by_id.get(counter_ref.counter_id.0).copied(),
        counter_ref.branch_id,
    ) {
        (Some(CounterRuntimeSlot::Scalar(slot)), None) => Ok(slot),
        (Some(CounterRuntimeSlot::Branches { start, len }), Some(branch_id))
            if branch_id.0 < len =>
        {
            Ok(start + branch_id.0)
        }
        (Some(CounterRuntimeSlot::Branches { .. }), None) => Err(format!(
            "counter id {} uses branch storage but no branch was selected",
            counter_ref.counter_id.0
        )),
        (Some(CounterRuntimeSlot::Scalar(_)), Some(branch_id)) => Err(format!(
            "counter id {} uses scalar storage but branch {} was selected",
            counter_ref.counter_id.0, branch_id.0
        )),
        (Some(CounterRuntimeSlot::TopValues(_)), _) => Err(format!(
            "counter id {} uses top-value storage where a scalar counter was required",
            counter_ref.counter_id.0
        )),
        (Some(CounterRuntimeSlot::Branches { len, .. }), Some(branch_id)) => Err(format!(
            "counter id {} branch {} is out of range for {} branches",
            counter_ref.counter_id.0, branch_id.0, len
        )),
        (None, _) => Err(format!(
            "missing scalar counter slot for counter id {}",
            counter_ref.counter_id.0
        )),
    }
}

pub(super) fn top_value_counter_slot_for_id(
    counter_slots_by_id: &[CounterRuntimeSlot],
    counter_id: CounterId,
) -> Result<usize, String> {
    match counter_slots_by_id.get(counter_id.0).copied() {
        Some(CounterRuntimeSlot::TopValues(slot)) => Ok(slot),
        Some(CounterRuntimeSlot::Scalar(_)) => Err(format!(
            "counter id {} uses scalar storage where a top-value counter was required",
            counter_id.0
        )),
        Some(CounterRuntimeSlot::Branches { .. }) => Err(format!(
            "counter id {} uses branch storage where a top-value counter was required",
            counter_id.0
        )),
        None => Err(format!(
            "missing top-value counter slot for counter id {}",
            counter_id.0
        )),
    }
}

fn scalar_counter_byte_offset(counter_slot: usize) -> i64 {
    counter_slot
        .checked_mul(std::mem::size_of::<u64>())
        .and_then(|offset| i64::try_from(offset).ok())
        .unwrap_or_else(|| panic!("scalar counter byte offset overflow for slot {counter_slot}"))
}

pub(super) fn scalar_counter_addr(
    fb: &mut FunctionBuilder<'_>,
    scalar_counter_base_value: ir::Value,
    counter_slot: usize,
) -> (ir::Value, i32) {
    let byte_offset = scalar_counter_byte_offset(counter_slot);
    if let Ok(offset) = i32::try_from(byte_offset) {
        (scalar_counter_base_value, offset)
    } else {
        (fb.ins().iadd_imm(scalar_counter_base_value, byte_offset), 0)
    }
}

pub(super) fn emit_increment_counter_slot(
    fb: &mut FunctionBuilder<'_>,
    scalar_counter_base_value: ir::Value,
    counter_slot: usize,
) {
    let (counter_addr, counter_offset) =
        scalar_counter_addr(fb, scalar_counter_base_value, counter_slot);
    let old_value = fb.ins().load(
        ir::types::I64,
        ir::MemFlags::trusted(),
        counter_addr,
        counter_offset,
    );
    let new_value = fb.ins().iadd_imm(old_value, 1);
    fb.ins().store(
        ir::MemFlags::trusted(),
        new_value,
        counter_addr,
        counter_offset,
    );
}

fn top_value_counter_byte_offset(counter_slot: usize) -> i64 {
    counter_slot
        .checked_mul(std::mem::size_of::<TopValueCounter>())
        .and_then(|offset| i64::try_from(offset).ok())
        .unwrap_or_else(|| panic!("top-value counter byte offset overflow for slot {counter_slot}"))
}

pub(super) fn emit_record_top_value_counter_slot(
    fb: &mut FunctionBuilder<'_>,
    top_value_counter_base_value: ir::Value,
    counter_slot: usize,
    observed_value: ir::Value,
    record_top_value_sample_ref: ir::FuncRef,
) {
    let counter_addr = fb.ins().iadd_imm(
        top_value_counter_base_value,
        top_value_counter_byte_offset(counter_slot),
    );
    fb.ins()
        .call(record_top_value_sample_ref, &[counter_addr, observed_value]);
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CountedRefcountHelpers {
    pub(super) incref_func_id: Option<FuncId>,
    pub(super) decref_func_id: Option<FuncId>,
}

fn lookup_counter_id(
    counter_defs: &[CounterDef],
    scope: CounterScope,
    kind: &str,
    site: &CounterSite,
) -> Option<CounterId> {
    counter_defs.iter().find_map(|counter| {
        (counter.scope == scope && counter.kind == kind && &counter.site == site)
            .then_some(counter.id)
    })
}

fn lookup_runtime_counter_id(
    counter_defs: &[CounterDef],
    function_id: RuntimeFunctionId,
    kind: &str,
) -> Option<CounterId> {
    lookup_counter_id(
        counter_defs,
        CounterScope::Function,
        kind,
        &CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: None,
        },
    )
    .or_else(|| {
        lookup_counter_id(
            counter_defs,
            CounterScope::Global,
            kind,
            &CounterSite::Runtime {
                function_id: None,
                instr_id: None,
            },
        )
    })
}

pub(super) fn collect_runtime_counter_ids_by_kind(
    counter_defs: &[CounterDef],
    function_id: RuntimeFunctionId,
    kind: &str,
) -> HashMap<InstrId, CounterId> {
    counter_defs
        .iter()
        .filter_map(|counter| match &counter.site {
            CounterSite::Runtime {
                function_id: Some(counter_function_id),
                instr_id: Some(instr_id),
            } if counter.kind == kind && *counter_function_id == function_id => {
                Some((*instr_id, counter.id))
            }
            _ => None,
        })
        .collect()
}

pub(super) fn collect_runtime_counter_refs_by_kind_branch(
    counter_defs: &[CounterDef],
    function_id: RuntimeFunctionId,
    kind: &str,
    branch: &str,
) -> HashMap<InstrId, CounterRef> {
    counter_defs
        .iter()
        .filter_map(|counter| match &counter.site {
            CounterSite::Runtime {
                function_id: Some(counter_function_id),
                instr_id: Some(instr_id),
            } if counter.kind == kind && *counter_function_id == function_id => {
                let branch_id = counter.branch_id(branch)?;
                Some((*instr_id, CounterRef::branch(counter.id, branch_id)))
            }
            _ => None,
        })
        .collect()
}

fn deopt_entry_source_for_resume_point(point: LocalEnvResumePoint) -> DeoptEntrySource {
    match point {
        LocalEnvResumePoint::BlockEntry { block, .. } => {
            DeoptEntrySource::BlockEntry { block_label: block }
        }
        LocalEnvResumePoint::BeforeInstr { key } => DeoptEntrySource::BeforeInstr {
            instr_id: key.instr_id,
        },
        LocalEnvResumePoint::BeforeTerm { block, .. } => {
            DeoptEntrySource::BeforeTerm { block_label: block }
        }
    }
}

pub(super) fn collect_deopt_entry_counter_ids_by_kind(
    counter_defs: &[CounterDef],
    function_id: RuntimeFunctionId,
    kind: &str,
    deopt_resume_plan: &PlannedJitDeoptResumeFunction,
) -> HashMap<usize, CounterId> {
    counter_defs
        .iter()
        .filter_map(|counter| match &counter.site {
            CounterSite::DeoptEntry {
                function_id: counter_function_id,
                source,
            } if counter.kind == kind && *counter_function_id == function_id => {
                let ordinal = deopt_resume_plan
                    .deopt_points
                    .iter()
                    .find(|point| deopt_entry_source_for_resume_point(point.point) == *source)?
                    .id
                    .ordinal;
                Some((ordinal, counter.id))
            }
            _ => None,
        })
        .collect()
}

pub(super) fn build_counted_runtime_refcount_helper(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    symbol_name: &str,
    function_name: &str,
    wrapper_import: &'static ImportSpec,
    applied_import: &'static ImportSpec,
    scalar_counter_data_id: DataId,
    counter_slot: usize,
) -> Result<FuncId, String> {
    let ptr_ty = jit_module.codegen_target_config().pointer_type();
    let sig = lower_static_signature(jit_module, wrapper_import.signature);
    let helper_id = declare_local_fn(jit_module, symbol_name, &sig)?;

    let mut ctx = jit_module.codegen_make_context();
    ctx.func.signature = sig;
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = fb.create_block();
        fb.append_block_params_for_function_params(entry_block);
        fb.switch_to_block(entry_block);
        let call_args = fb.block_params(entry_block).to_vec();
        let mut module_imports = ModuleFuncImports::new();
        let mut func_imports = FuncBuildImports::new(&mut module_imports);
        let runtime_ref = func_imports.get_or_panic(jit_module, &mut fb.func, applied_import);
        let runtime_call = fb.ins().call(runtime_ref, &call_args);
        let applied = fb.inst_results(runtime_call)[0];
        let counter_data =
            jit_module.codegen_declare_data_in_func(scalar_counter_data_id, &mut fb.func)?;
        let scalar_counter_base_value = fb.ins().global_value(ptr_ty, counter_data);
        let (counter_addr, counter_offset) =
            scalar_counter_addr(&mut fb, scalar_counter_base_value, counter_slot);
        let old_value = fb.ins().load(
            ir::types::I64,
            ir::MemFlags::trusted(),
            counter_addr,
            counter_offset,
        );
        let applied_i64 = fb.ins().uextend(ir::types::I64, applied);
        let new_value = fb.ins().iadd(old_value, applied_i64);
        fb.ins().store(
            ir::MemFlags::trusted(),
            new_value,
            counter_addr,
            counter_offset,
        );
        fb.ins().return_(&[]);
        fb.seal_all_blocks();
        fb.finalize();
    }

    let _ = define_prepared_function(
        jit_module,
        env_config,
        helper_id,
        &mut ctx,
        function_name,
        "failed to define counted runtime refcount helper",
    )?;
    jit_module.codegen_clear_context(&mut ctx);
    Ok(helper_id)
}

pub(super) fn build_counted_runtime_refcount_helpers(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    function: &BlockPyFunction<impl ModuleShape>,
    counter_defs: &[CounterDef],
    counter_slots_by_id: &[CounterRuntimeSlot],
    scalar_counter_data_id: Option<DataId>,
    symbol_scope: Option<&str>,
) -> Result<CountedRefcountHelpers, String> {
    if !env_config.jit_refcount_emission_enabled() {
        return Ok(CountedRefcountHelpers::default());
    }

    let incref_func_id =
        lookup_runtime_counter_id(counter_defs, function.function_id, "runtime_incref")
            .map(|counter_id| {
                let counter_slot = scalar_counter_slot_for_id(counter_slots_by_id, counter_id)?;
                let scalar_counter_data_id = scalar_counter_data_id.ok_or_else(|| {
                    format!(
                        "missing scalar counter storage for runtime incref counter {}",
                        counter_id.0
                    )
                })?;
                let helper_name = scoped_jit_symbol(
                    format!("py:rc:incref:{}", function.names.qualname).as_str(),
                    symbol_scope,
                );
                build_counted_runtime_refcount_helper(
                    jit_module,
                    env_config,
                    &helper_name,
                    &helper_name,
                    &DP_JIT_INCREF_IMPORT,
                    &SOAC_RUNTIME_INCREF_APPLIED_IMPORT,
                    scalar_counter_data_id,
                    counter_slot,
                )
            })
            .transpose()?;

    let decref_func_id =
        lookup_runtime_counter_id(counter_defs, function.function_id, "runtime_decref")
            .map(|counter_id| {
                let counter_slot = scalar_counter_slot_for_id(counter_slots_by_id, counter_id)?;
                let scalar_counter_data_id = scalar_counter_data_id.ok_or_else(|| {
                    format!(
                        "missing scalar counter storage for runtime decref counter {}",
                        counter_id.0
                    )
                })?;
                let helper_name = scoped_jit_symbol(
                    format!("py:rc:decref:{}", function.names.qualname).as_str(),
                    symbol_scope,
                );
                build_counted_runtime_refcount_helper(
                    jit_module,
                    env_config,
                    &helper_name,
                    &helper_name,
                    &DP_JIT_DECREF_IMPORT,
                    &SOAC_RUNTIME_DECREF_APPLIED_IMPORT,
                    scalar_counter_data_id,
                    counter_slot,
                )
            })
            .transpose()?;

    Ok(CountedRefcountHelpers {
        incref_func_id,
        decref_func_id,
    })
}
