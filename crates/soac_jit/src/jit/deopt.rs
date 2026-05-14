use super::planning::{LocalRefKind, PlannedJitDeoptPointId, PlannedJitDeoptResumeFunction};
use super::runtime_context::FunctionRuntimeDataLayout;
use super::specialized_helpers::ObjPtr;
use super::{BlockPyBlock, blockpy_intrinsics, transient_local_needs_decref};
use crate::module_constants::ModuleConstantId;
use pyo3::{Py, PyAny, ffi};
use soac_core::block_py::{
    BlockArg, BlockEdge, BlockLabel, BlockPyFunction, BlockTerm, CallArgKeyword, CallArgPositional,
    CellLocation, InstrLocationMap, LocalLocation, NameLocation, ParamKind, RuntimeFunctionId,
    StorageLayout, current_instr_locations,
};
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};
use soac_ir_typed::{InstrTyped, TypedBlockPyModuleShape};
use soac_opt::passes::{
    LocalEnvResumeBinding, LocalEnvResumeBindingState, LocalEnvResumePoint,
    LocalEnvResumeStatePrecision, LocalEnvResumeValueSource,
};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::Arc;

pub(super) struct RuntimeJitDeoptTable {
    pub(super) function_id: RuntimeFunctionId,
    pub(super) function: Box<BlockPyFunction<BlockPyModuleShape>>,
    pub(super) module_constant_ptrs: Vec<ObjPtr>,
    #[cfg(not(test))]
    _module_constant_owners: Option<Arc<Vec<Py<PyAny>>>>,
    pub(super) points: Vec<RuntimeJitDeoptRecord>,
}

// The table is immutable after construction. Raw module-constant pointers are copied into the
// table as runtime metadata and are only dereferenced when generated/deopt code is running under
// the Python runtime contract, not during worker-thread codegen.
unsafe impl Send for RuntimeJitDeoptTable {}
unsafe impl Sync for RuntimeJitDeoptTable {}

#[derive(Clone, Debug)]
pub(super) struct RuntimeJitDeoptRecord {
    pub(super) id: PlannedJitDeoptPointId,
    pub(super) resume_point: LocalEnvResumePoint,
    pub(super) precision: LocalEnvResumeStatePrecision,
    pub(super) locals: Vec<LocalEnvResumeBinding>,
    pub(super) continuation: RuntimeJitDeoptContinuation,
}

pub(super) struct RuntimeJitDeoptInvocation<'a> {
    table: &'a RuntimeJitDeoptTable,
    record: &'a RuntimeJitDeoptRecord,
    globals_obj: ObjPtr,
    function_data_obj: ObjPtr,
    live_values: &'a [ObjPtr],
}

pub(super) struct RuntimeJitDeoptLocal<'a> {
    binding: &'a LocalEnvResumeBinding,
    value: ObjPtr,
    release_on_frame_exit: bool,
}

pub(super) struct RuntimeJitDeoptLocals<'a> {
    locals_by_slot: Vec<Option<RuntimeJitDeoptLocal<'a>>>,
    len: usize,
}

pub(super) struct RuntimeFunctionEntryParam {
    name: String,
    kind: ParamKind,
    default_slot: Option<usize>,
}

pub(crate) struct RuntimeFunctionEntryPlan {
    callable_name: String,
    params: Box<[RuntimeFunctionEntryParam]>,
    positional_param_indices: Box<[usize]>,
    param_indices_by_name: HashMap<String, usize>,
    varargs_param: Option<usize>,
    varkw_param: Option<usize>,
    local_bindings: Box<[LocalEnvResumeBinding]>,
    local_param_indices: Box<[Option<usize>]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RuntimeJitDeoptCursor {
    block: BlockLabel,
    body_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RuntimeJitDeoptUnsupportedReason {
    WrongFunction,
    #[allow(dead_code)]
    MissingFunction,
    MissingBlock,
    MissingInstruction,
    MissingPlanRecord,
    UnsupportedBlockTail,
    ReplayUnsafeGuardOperand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum RuntimeJitDeoptContinuation {
    Unsupported {
        reason: RuntimeJitDeoptUnsupportedReason,
    },
    ResumeBlockTail {
        cursor: RuntimeJitDeoptCursor,
    },
}

impl RuntimeJitDeoptCursor {
    pub(super) fn new(block: BlockLabel, body_index: usize) -> Self {
        Self { block, body_index }
    }

    pub(super) fn block(self) -> BlockLabel {
        self.block
    }

    pub(super) fn body_index(self) -> usize {
        self.body_index
    }

    pub(super) fn at_block_entry(block: BlockLabel) -> Self {
        Self::new(block, 0)
    }
}

impl RuntimeJitDeoptContinuation {
    pub(super) fn unsupported(reason: RuntimeJitDeoptUnsupportedReason) -> Self {
        Self::Unsupported { reason }
    }

    pub(super) fn initial_cursor(&self) -> Option<RuntimeJitDeoptCursor> {
        match self {
            RuntimeJitDeoptContinuation::ResumeBlockTail { cursor } => Some(*cursor),
            RuntimeJitDeoptContinuation::Unsupported { .. } => None,
        }
    }

    pub(super) fn unsupported_reason(&self) -> Option<RuntimeJitDeoptUnsupportedReason> {
        match self {
            RuntimeJitDeoptContinuation::Unsupported { reason } => Some(*reason),
            RuntimeJitDeoptContinuation::ResumeBlockTail { .. } => None,
        }
    }
}

impl RuntimeFunctionEntryPlan {
    pub(crate) fn from_function(
        function: &BlockPyFunction<BlockPyModuleShape>,
    ) -> Result<Self, String> {
        let layout = function.public_storage_layout().ok_or_else(|| {
            format!(
                "entry interpreter expected storage layout for function {}",
                function.function_id
            )
        })?;
        let runtime_data_layout = FunctionRuntimeDataLayout::from_function(function);
        let positional_param_indices = function
            .params
            .params
            .iter()
            .enumerate()
            .filter_map(|(index, param)| {
                matches!(param.kind, ParamKind::PosOnly | ParamKind::Any).then_some(index)
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let varargs_param = function.params.vararg_index();
        let varkw_param = function.params.kwarg_index();
        let mut param_indices_by_name = HashMap::with_capacity(function.params.len());
        let params = function
            .params
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                if param_indices_by_name
                    .insert(param.name.clone(), index)
                    .is_some()
                {
                    return Err(format!(
                        "entry interpreter found duplicate param name {:?} in function {}",
                        param.name, function.function_id
                    ));
                }
                let default_slot = match param.kind {
                    ParamKind::PosOnly | ParamKind::Any => {
                        runtime_data_layout.positional_default_slot_for_param_index(index)
                    }
                    ParamKind::KwOnly => runtime_data_layout.kwonly_default_slot(&param.name),
                    ParamKind::VarArg | ParamKind::KwArg => None,
                };
                Ok(RuntimeFunctionEntryParam {
                    name: param.name.clone(),
                    kind: param.kind,
                    default_slot,
                })
            })
            .collect::<Result<Vec<_>, String>>()?
            .into_boxed_slice();

        let mut param_has_stack_slot = vec![false; params.len()];
        let mut local_bindings = Vec::with_capacity(layout.stack_slots().len());
        let mut local_param_indices = Vec::with_capacity(layout.stack_slots().len());
        let mut local_names = HashSet::with_capacity(layout.stack_slots().len());
        for (slot, name) in layout.stack_slots().iter().enumerate() {
            if !local_names.insert(name.as_str()) {
                return Err(format!(
                    "entry interpreter found duplicate stack slot name {name:?} in function {}",
                    function.function_id
                ));
            }
            let location = LocalLocation(u32::try_from(slot).map_err(|_| {
                format!(
                    "entry interpreter cannot address stack slot {slot} for function {}",
                    function.function_id
                )
            })?);
            let param_index = param_indices_by_name.get(name.as_str()).copied();
            if let Some(param_index) = param_index {
                param_has_stack_slot[param_index] = true;
            }
            local_bindings.push(LocalEnvResumeBinding {
                name: name.clone(),
                location,
                binding: if param_index.is_some() {
                    LocalEnvResumeBindingState::Bound
                } else {
                    LocalEnvResumeBindingState::Unbound
                },
                source: if param_index.is_some() {
                    LocalEnvResumeValueSource::Unknown
                } else {
                    LocalEnvResumeValueSource::Unbound
                },
                ownership: if param_index.is_some() {
                    LocalRefKind::Owned
                } else {
                    LocalRefKind::Unbound
                },
                value: None,
            });
            local_param_indices.push(param_index);
        }
        for (param_index, param) in params.iter().enumerate() {
            if !param_has_stack_slot[param_index] {
                return Err(format!(
                    "entry interpreter expected param {:?} in stack slots for function {}",
                    param.name, function.function_id
                ));
            }
        }

        Ok(Self {
            callable_name: function.names.display_name.clone(),
            params,
            positional_param_indices,
            param_indices_by_name,
            varargs_param,
            varkw_param,
            local_bindings: local_bindings.into_boxed_slice(),
            local_param_indices: local_param_indices.into_boxed_slice(),
        })
    }

    pub(super) fn callable_name(&self) -> &str {
        self.callable_name.as_str()
    }

    pub(super) fn params(&self) -> &[RuntimeFunctionEntryParam] {
        &self.params
    }

    pub(super) fn param_index(&self, name: &str) -> Option<usize> {
        self.param_indices_by_name.get(name).copied()
    }

    pub(super) fn positional_param_indices(&self) -> &[usize] {
        &self.positional_param_indices
    }

    pub(super) fn positional_capacity(&self) -> usize {
        self.positional_param_indices.len()
    }

    pub(super) fn varargs_param(&self) -> Option<usize> {
        self.varargs_param
    }

    pub(super) fn varkw_param(&self) -> Option<usize> {
        self.varkw_param
    }

    pub(super) fn local_bindings(&self) -> &[LocalEnvResumeBinding] {
        &self.local_bindings
    }

    pub(super) fn local_param_indices(&self) -> &[Option<usize>] {
        &self.local_param_indices
    }
}

impl RuntimeFunctionEntryParam {
    pub(super) fn name(&self) -> &str {
        self.name.as_str()
    }

    pub(super) fn kind(&self) -> ParamKind {
        self.kind
    }

    pub(super) fn default_slot(&self) -> Option<usize> {
        self.default_slot
    }
}

impl RuntimeJitDeoptRecord {
    #[cfg(test)]
    pub(super) fn id(&self) -> PlannedJitDeoptPointId {
        self.id
    }

    pub(super) fn ordinal(&self) -> usize {
        self.id.ordinal
    }

    pub(super) fn locals(&self) -> &[LocalEnvResumeBinding] {
        &self.locals
    }

    pub(super) fn initial_cursor(&self) -> Option<RuntimeJitDeoptCursor> {
        self.continuation.initial_cursor()
    }

    #[cfg(test)]
    pub(super) fn continuation(&self) -> &RuntimeJitDeoptContinuation {
        &self.continuation
    }

    fn validate_live_value_buffer(
        &self,
        live_values: ObjPtr,
        live_value_count: i64,
    ) -> Result<(), String> {
        let count = usize::try_from(live_value_count).map_err(|_| {
            format!("live value count {live_value_count} is negative or does not fit usize")
        })?;
        if count != self.locals.len() {
            return Err(format!(
                "deopt record {} expected {} live values but got {}",
                self.ordinal(),
                self.locals.len(),
                count
            ));
        }
        if count != 0 && live_values.is_null() {
            return Err(format!(
                "deopt record {} expected a non-null live value buffer for {} values",
                self.ordinal(),
                count
            ));
        }
        Ok(())
    }

    pub(super) fn describe(&self, function_id: RuntimeFunctionId) -> String {
        format!(
            "function {}, record {}, resume_point {:?}, precision {:?}, locals {}, continuation {:?}",
            function_id,
            self.ordinal(),
            self.resume_point,
            self.precision,
            self.locals.len(),
            self.continuation
        )
    }
}

impl RuntimeJitDeoptTable {
    pub(super) fn from_plan(
        function: &BlockPyFunction<BlockPyModuleShape>,
        plan: &PlannedJitDeoptResumeFunction,
        module_constant_ptrs: &[*mut ffi::PyObject],
    ) -> Result<Self, String> {
        Self::from_plan_with_owned_constants(function, plan, module_constant_ptrs, None)
    }

    pub(super) fn from_plan_with_owned_constants(
        function: &BlockPyFunction<BlockPyModuleShape>,
        plan: &PlannedJitDeoptResumeFunction,
        module_constant_ptrs: &[*mut ffi::PyObject],
        #[cfg_attr(test, allow(unused_variables))] module_constant_owners: Option<
            Arc<Vec<Py<PyAny>>>,
        >,
    ) -> Result<Self, String> {
        let mut points = Vec::with_capacity(plan.deopt_points.len());
        let instr_locations = current_instr_locations(function);
        for deopt_point in &plan.deopt_points {
            let entry = plan.entry(deopt_point.resume_point).ok_or_else(|| {
                format!(
                    "planned deopt point {:?} for function {} has no resume entry",
                    deopt_point.point, function.function_id
                )
            })?;
            points.push(RuntimeJitDeoptRecord {
                id: deopt_point.id,
                resume_point: deopt_point.resume_point,
                precision: deopt_point.precision,
                locals: entry.locals.clone(),
                continuation: runtime_jit_deopt_continuation_for_point(
                    function,
                    &instr_locations,
                    deopt_point.resume_point,
                ),
            });
        }
        let table = Self {
            function_id: function.function_id,
            function: Box::new(function.clone()),
            module_constant_ptrs: module_constant_ptrs
                .iter()
                .map(|ptr| ptr.cast::<c_void>())
                .collect(),
            #[cfg(not(test))]
            _module_constant_owners: module_constant_owners,
            points,
        };
        table.validate_against_plan(plan)?;
        Ok(table)
    }

    fn validate_against_plan(&self, plan: &PlannedJitDeoptResumeFunction) -> Result<(), String> {
        if self.points.len() != plan.deopt_points.len() {
            return Err(format!(
                "runtime JIT deopt table for function {} has {} points, expected {}",
                self.function_id,
                self.points.len(),
                plan.deopt_points.len()
            ));
        }
        for (record, planned) in self.points.iter().zip(plan.deopt_points.iter()) {
            if record.id != planned.id
                || record.resume_point != planned.resume_point
                || record.precision != planned.precision
            {
                return Err(format!(
                    "runtime JIT deopt table record {:?} does not match planned point {:?}",
                    record.id, planned.id
                ));
            }
            let Some(entry) = plan.entry(planned.resume_point) else {
                return Err(format!(
                    "runtime JIT deopt table record {:?} references missing resume point {:?}",
                    record.id, planned.resume_point
                ));
            };
            if record.locals != entry.locals {
                return Err(format!(
                    "runtime JIT deopt table record {:?} has stale local materialization",
                    record.id
                ));
            }
        }
        Ok(())
    }

    pub(super) fn function_id(&self) -> RuntimeFunctionId {
        self.function_id
    }

    pub(super) fn supported_resume_points(&self) -> Vec<LocalEnvResumePoint> {
        self.points
            .iter()
            .filter(|record| record.continuation.unsupported_reason().is_none())
            .map(|record| record.resume_point)
            .collect()
    }

    fn function(&self) -> &BlockPyFunction<BlockPyModuleShape> {
        self.function.as_ref()
    }

    fn module_constant_ptr(&self, constant_id: ModuleConstantId) -> Result<ObjPtr, String> {
        self.module_constant_ptrs
            .get(constant_id.0)
            .copied()
            .ok_or_else(|| {
                format!(
                    "deopt table for function {} is missing module constant {}",
                    self.function_id, constant_id.0
                )
            })
    }

    fn record_for_ordinal(&self, record_ordinal: i64) -> Result<&RuntimeJitDeoptRecord, String> {
        let ordinal = usize::try_from(record_ordinal).map_err(|_| {
            format!(
                "deopt record ordinal {record_ordinal} is negative or does not fit usize for function {}",
                self.function_id
            )
        })?;
        let record = self.points.get(ordinal).ok_or_else(|| {
            format!(
                "deopt record ordinal {ordinal} is outside table for function {} with {} records",
                self.function_id,
                self.points.len()
            )
        })?;
        if record.id.ordinal != ordinal {
            return Err(format!(
                "deopt record ordinal {ordinal} resolves to stale record {:?}",
                record.id
            ));
        }
        Ok(record)
    }

    #[cfg(test)]
    pub(super) fn record_for_point(
        &self,
        point: LocalEnvResumePoint,
    ) -> Option<&RuntimeJitDeoptRecord> {
        self.points
            .iter()
            .find(|record| record.resume_point == point)
    }
}

pub(super) fn runtime_jit_deopt_continuation_for_point(
    function: &BlockPyFunction<BlockPyModuleShape>,
    instr_locations: &InstrLocationMap,
    point: LocalEnvResumePoint,
) -> RuntimeJitDeoptContinuation {
    match point {
        LocalEnvResumePoint::BeforeTerm { function_id, block } => {
            if function_id != function.function_id {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::WrongFunction,
                );
            }
            let Some(block) = function
                .blocks
                .iter()
                .find(|candidate| candidate.label == block)
            else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingBlock,
                );
            };
            if runtime_jit_deopt_block_tail_supported(function, block, block.body.len()) {
                RuntimeJitDeoptContinuation::ResumeBlockTail {
                    cursor: RuntimeJitDeoptCursor::new(block.label, block.body.len()),
                }
            } else {
                RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
                )
            }
        }
        LocalEnvResumePoint::BeforeInstr { key } => {
            if key.function_id != function.function_id {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::WrongFunction,
                );
            }
            let Some(location) = instr_locations.get(&key.instr_id).copied() else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingInstruction,
                );
            };
            let block_label = location.block_label();
            let Some(block) = function
                .blocks
                .iter()
                .find(|candidate| candidate.label == block_label)
            else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingBlock,
                );
            };
            let Some(start_body_index) = location.body_index() else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingInstruction,
                );
            };
            if runtime_jit_deopt_block_tail_supported(function, block, start_body_index) {
                RuntimeJitDeoptContinuation::ResumeBlockTail {
                    cursor: RuntimeJitDeoptCursor::new(block_label, start_body_index),
                }
            } else {
                RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
                )
            }
        }
        LocalEnvResumePoint::BlockEntry { function_id, block } => {
            if function_id != function.function_id {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::WrongFunction,
                );
            }
            let Some(block) = function
                .blocks
                .iter()
                .find(|candidate| candidate.label == block)
            else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingBlock,
                );
            };
            if runtime_jit_deopt_block_tail_supported(function, block, 0) {
                RuntimeJitDeoptContinuation::ResumeBlockTail {
                    cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
                }
            } else {
                RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
                )
            }
        }
    }
}

pub(super) fn runtime_jit_typed_deopt_continuation_for_point(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    instr_locations: &InstrLocationMap,
    point: LocalEnvResumePoint,
) -> RuntimeJitDeoptContinuation {
    match point {
        LocalEnvResumePoint::BeforeTerm { function_id, block } => {
            if function_id != function.function_id {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::WrongFunction,
                );
            }
            if let Some(block) = function
                .blocks
                .iter()
                .find(|candidate| candidate.label == block)
            {
                RuntimeJitDeoptContinuation::ResumeBlockTail {
                    cursor: RuntimeJitDeoptCursor::new(block.label, block.body.len()),
                }
            } else {
                RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingBlock,
                )
            }
        }
        LocalEnvResumePoint::BeforeInstr { key } => {
            if key.function_id != function.function_id {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::WrongFunction,
                );
            }
            let Some(location) = instr_locations.get(&key.instr_id).copied() else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingInstruction,
                );
            };
            let Some(body_index) = location.body_index() else {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingInstruction,
                );
            };
            RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(location.block_label(), body_index),
            }
        }
        LocalEnvResumePoint::BlockEntry { function_id, block } => {
            if function_id != function.function_id {
                return RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::WrongFunction,
                );
            }
            if function
                .blocks
                .iter()
                .any(|candidate| candidate.label == block)
            {
                RuntimeJitDeoptContinuation::ResumeBlockTail {
                    cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                }
            } else {
                RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::MissingBlock,
                )
            }
        }
    }
}

pub(super) fn runtime_jit_deopt_guard_operand_replay_safe(expr: &InstrBlockPy) -> bool {
    matches!(
        expr,
        InstrBlockPy::Load(load)
            if matches!(
                load.name.location,
                NameLocation::Local(_) | NameLocation::Cell(_) | NameLocation::Constant(_)
            )
    )
}

pub(super) fn runtime_jit_typed_deopt_guard_operand_replay_safe(expr: &InstrTyped) -> bool {
    matches!(
        expr,
        InstrTyped::Load(load)
            if matches!(
                load.name.location,
                NameLocation::Local(_) | NameLocation::Cell(_) | NameLocation::Constant(_)
            )
    )
}

pub(super) fn typed_nested_guard_misses_can_resume_before_instr(expr: &InstrTyped) -> bool {
    let mut saw_replay_unsafe_effect = false;
    typed_nested_guard_scan_expr(expr, &mut saw_replay_unsafe_effect)
}

fn nested_guard_candidate_seen_before_replay_unsafe_effect(
    has_guard_candidate: bool,
    saw_replay_unsafe_effect: bool,
) -> bool {
    !has_guard_candidate || !saw_replay_unsafe_effect
}

fn typed_nested_guard_scan_expr(expr: &InstrTyped, saw_replay_unsafe_effect: &mut bool) -> bool {
    match expr {
        InstrTyped::Truthy(op) => {
            typed_nested_guard_scan_expr(op.value(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::Load(op) => nested_guard_candidate_seen_before_replay_unsafe_effect(
            matches!(op.name.location, NameLocation::Global(_)),
            *saw_replay_unsafe_effect,
        ),
        InstrTyped::BinOp(op) => {
            typed_nested_guard_scan_expr(op.left.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.right.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::UnaryOp(op) => {
            typed_nested_guard_scan_expr(op.operand.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::Tuple(op) => op
            .values
            .iter()
            .all(|value| typed_nested_guard_scan_expr(value, saw_replay_unsafe_effect)),
        InstrTyped::CalleeFunctionId(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
        }
        InstrTyped::CallTyped(op) => {
            if op.args.is_empty()
                && op.keywords.is_empty()
                && let InstrTyped::GetAttrTyped(getattr) = op.func.as_ref()
            {
                // Direct-method guard code evaluates only the receiver before the guard.
                // Keep this no-arg only until argument guard points carry their own
                // precise resume state.
                return typed_nested_guard_scan_expr(
                    getattr.value.as_ref(),
                    saw_replay_unsafe_effect,
                ) && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                ) && mark_replay_unsafe_effect(saw_replay_unsafe_effect);
            }
            typed_nested_guard_scan_expr(op.func.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_positional_args(
                    op.args.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && typed_nested_guard_scan_keyword_args(
                    op.keywords.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::GuardedCallableCallTyped(op) => {
            typed_nested_guard_scan_expr(op.func.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_positional_args(
                    op.args.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && typed_nested_guard_scan_keyword_args(
                    op.keywords.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::GuardedMethodCallTyped(op) => {
            if op.args.is_empty()
                && op.keywords.is_empty()
                && let InstrTyped::GetAttrTyped(getattr) = op.func.as_ref()
            {
                // Direct-method guard code evaluates only the receiver before the guard.
                // Keep this no-arg only until argument guard points carry their own
                // precise resume state.
                return typed_nested_guard_scan_expr(
                    getattr.value.as_ref(),
                    saw_replay_unsafe_effect,
                ) && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                ) && mark_replay_unsafe_effect(saw_replay_unsafe_effect);
            }
            typed_nested_guard_scan_expr(op.func.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_positional_args(
                    op.args.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && typed_nested_guard_scan_keyword_args(
                    op.keywords.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::DirectCallableCallTyped(op) => {
            typed_nested_guard_scan_expr(op.func.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_positional_args(
                    op.args.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::DirectMethodCallTyped(op) => {
            typed_nested_guard_scan_expr(op.receiver.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_positional_args(
                    op.args.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::DirectCallGuardTest(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
        }
        InstrTyped::CallDirect(op) => {
            typed_nested_guard_scan_expr(op.callable.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_positional_args(
                    op.args.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && typed_nested_guard_scan_keyword_args(
                    op.keywords.as_slice(),
                    saw_replay_unsafe_effect,
                )
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::GetAttrTyped(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.attr.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::SetAttrTyped(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.attr.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.replacement.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    true,
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::GetItem(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.index.as_ref(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::SetItem(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.index.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.replacement.as_ref(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::DelItem(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(op.index.as_ref(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::Store(op) => {
            typed_nested_guard_scan_expr(op.value.as_ref(), saw_replay_unsafe_effect)
                && nested_guard_candidate_seen_before_replay_unsafe_effect(
                    matches!(op.name.location, NameLocation::Global(_)),
                    *saw_replay_unsafe_effect,
                )
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::Del(_) => mark_replay_unsafe_effect(saw_replay_unsafe_effect),
        InstrTyped::MakeCell(op) => {
            op.initial_value.as_ref().map_or(true, |initial_value| {
                typed_nested_guard_scan_expr(initial_value.as_ref(), saw_replay_unsafe_effect)
            }) && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
        InstrTyped::IncrementCounter(_) => mark_replay_unsafe_effect(saw_replay_unsafe_effect),
        InstrTyped::CellRef(_) => true,
        InstrTyped::MakeFunctionWithClosure(op) => {
            typed_nested_guard_scan_expr(op.captures.as_ref(), saw_replay_unsafe_effect)
                && typed_nested_guard_scan_expr(
                    op.param_defaults.as_ref(),
                    saw_replay_unsafe_effect,
                )
                && typed_nested_guard_scan_expr(op.annotate_fn.as_ref(), saw_replay_unsafe_effect)
                && mark_replay_unsafe_effect(saw_replay_unsafe_effect)
        }
    }
}

fn typed_nested_guard_scan_positional_args(
    args: &[CallArgPositional<InstrTyped>],
    saw_replay_unsafe_effect: &mut bool,
) -> bool {
    args.iter().all(|arg| match arg {
        CallArgPositional::Positional(expr) | CallArgPositional::Starred(expr) => {
            typed_nested_guard_scan_expr(expr, saw_replay_unsafe_effect)
        }
    })
}

fn typed_nested_guard_scan_keyword_args(
    keywords: &[CallArgKeyword<InstrTyped>],
    saw_replay_unsafe_effect: &mut bool,
) -> bool {
    keywords.iter().all(|keyword| match keyword {
        CallArgKeyword::Named { value, .. } | CallArgKeyword::Starred(value) => {
            typed_nested_guard_scan_expr(value, saw_replay_unsafe_effect)
        }
    })
}

fn mark_replay_unsafe_effect(saw_replay_unsafe_effect: &mut bool) -> bool {
    *saw_replay_unsafe_effect = true;
    true
}

fn runtime_jit_deopt_block_tail_supported(
    function: &BlockPyFunction<BlockPyModuleShape>,
    block: &BlockPyBlock,
    start_body_index: usize,
) -> bool {
    let Some(body_tail) = block.body.get(start_body_index..) else {
        return false;
    };
    let support = RuntimeJitDeoptSupportCtx::new(function);
    body_tail
        .iter()
        .all(|expr| runtime_jit_deopt_expr_supported(expr, &support))
        && runtime_jit_deopt_term_supported(&block.term, &support)
        && block
            .exc_edge
            .as_ref()
            .is_none_or(runtime_jit_deopt_exception_edge_supported)
}

struct RuntimeJitDeoptSupportCtx<'a> {
    storage_layout: Option<&'a StorageLayout>,
    runtime_layout: FunctionRuntimeDataLayout,
}

impl<'a> RuntimeJitDeoptSupportCtx<'a> {
    fn new(function: &'a BlockPyFunction<BlockPyModuleShape>) -> Self {
        RuntimeJitDeoptSupportCtx {
            storage_layout: function.storage_layout.as_ref(),
            runtime_layout: FunctionRuntimeDataLayout::from_function(function),
        }
    }

    fn owned_cell_supported(&self, slot: u32) -> bool {
        self.storage_layout
            .and_then(|layout| layout.owned_slot(slot))
            .is_some()
    }

    fn closure_cell_supported(&self, slot: u32) -> bool {
        (slot as usize) < self.runtime_layout.closure_len()
    }

    fn preserved_cell_supported(&self, slot: u32) -> bool {
        self.storage_layout
            .and_then(|layout| layout.preserved_slot(slot))
            .is_some_and(|slot| {
                slot.storage == soac_core::block_py::PreservedSlotStorage::PyCellObject
            })
    }
}

fn runtime_jit_deopt_expr_supported(
    expr: &InstrBlockPy,
    support: &RuntimeJitDeoptSupportCtx<'_>,
) -> bool {
    match expr {
        InstrBlockPy::Load(load) => match load.name.location {
            NameLocation::Cell(CellLocation::Owned(slot)) => support.owned_cell_supported(slot),
            NameLocation::Cell(CellLocation::Preserved(slot)) => {
                support.preserved_cell_supported(slot)
            }
            NameLocation::Cell(CellLocation::Closure(slot))
            | NameLocation::Cell(CellLocation::CapturedSource(slot)) => {
                support.closure_cell_supported(slot)
            }
            _ => true,
        },
        InstrBlockPy::BinOp(binop) => {
            runtime_jit_deopt_binop_supported(binop.kind)
                && runtime_jit_deopt_expr_supported(&binop.left, support)
                && runtime_jit_deopt_expr_supported(&binop.right, support)
        }
        InstrBlockPy::UnaryOp(unary) => runtime_jit_deopt_expr_supported(&unary.operand, support),
        InstrBlockPy::Tuple(tuple) => tuple
            .values
            .iter()
            .all(|value| runtime_jit_deopt_expr_supported(value, support)),
        InstrBlockPy::GetAttr(getattr) => {
            runtime_jit_deopt_expr_supported(&getattr.value, support)
                && runtime_jit_deopt_expr_supported(&getattr.attr, support)
        }
        InstrBlockPy::GetItem(getitem) => {
            runtime_jit_deopt_expr_supported(&getitem.value, support)
                && runtime_jit_deopt_expr_supported(&getitem.index, support)
        }
        InstrBlockPy::SetAttr(setattr) => {
            runtime_jit_deopt_expr_supported(&setattr.value, support)
                && runtime_jit_deopt_expr_supported(&setattr.attr, support)
                && runtime_jit_deopt_expr_supported(&setattr.replacement, support)
        }
        InstrBlockPy::SetItem(setitem) => {
            runtime_jit_deopt_expr_supported(&setitem.value, support)
                && runtime_jit_deopt_expr_supported(&setitem.index, support)
                && runtime_jit_deopt_expr_supported(&setitem.replacement, support)
        }
        InstrBlockPy::DelItem(delitem) => {
            runtime_jit_deopt_expr_supported(&delitem.value, support)
                && runtime_jit_deopt_expr_supported(&delitem.index, support)
        }
        InstrBlockPy::Call(call) => {
            runtime_jit_deopt_call_parts_supported(&call.func, &call.args, &call.keywords, support)
        }
        InstrBlockPy::Store(store) => {
            runtime_jit_deopt_name_location_supported(store.name.location, support)
                && runtime_jit_deopt_expr_supported(&store.value, support)
        }
        InstrBlockPy::Del(del) => {
            runtime_jit_deopt_name_location_supported(del.name.location, support)
        }
        InstrBlockPy::IncrementCounter(_) => true,
        InstrBlockPy::MakeCell(make_cell) => make_cell
            .initial_value
            .as_ref()
            .map_or(true, |initial_value| {
                runtime_jit_deopt_expr_supported(initial_value, support)
            }),
        InstrBlockPy::MakeFunctionWithClosure(make_function) => {
            runtime_jit_deopt_expr_supported(&make_function.captures, support)
                && runtime_jit_deopt_expr_supported(&make_function.param_defaults, support)
                && runtime_jit_deopt_expr_supported(&make_function.annotate_fn, support)
        }
        InstrBlockPy::CellRef(cell_ref) => match cell_ref.location {
            CellLocation::Owned(slot) => support.owned_cell_supported(slot),
            CellLocation::Preserved(slot) => support.preserved_cell_supported(slot),
            CellLocation::Closure(slot) | CellLocation::CapturedSource(slot) => {
                support.closure_cell_supported(slot)
            }
        },
    }
}

fn runtime_jit_deopt_name_location_supported(
    location: NameLocation,
    support: &RuntimeJitDeoptSupportCtx<'_>,
) -> bool {
    match location {
        NameLocation::Cell(CellLocation::Owned(slot)) => support.owned_cell_supported(slot),
        NameLocation::Cell(CellLocation::Preserved(slot)) => support.preserved_cell_supported(slot),
        NameLocation::Cell(CellLocation::Closure(slot))
        | NameLocation::Cell(CellLocation::CapturedSource(slot)) => {
            support.closure_cell_supported(slot)
        }
        _ => true,
    }
}

fn runtime_jit_deopt_call_parts_supported(
    callable: &InstrBlockPy,
    args: &[CallArgPositional<InstrBlockPy>],
    keywords: &[CallArgKeyword<InstrBlockPy>],
    support: &RuntimeJitDeoptSupportCtx<'_>,
) -> bool {
    runtime_jit_deopt_expr_supported(callable, support)
        && args.iter().all(|arg| match arg {
            CallArgPositional::Positional(expr) => runtime_jit_deopt_expr_supported(expr, support),
            CallArgPositional::Starred(expr) => runtime_jit_deopt_expr_supported(expr, support),
        })
        && keywords.iter().all(|keyword| match keyword {
            CallArgKeyword::Named { value, .. } => runtime_jit_deopt_expr_supported(value, support),
            CallArgKeyword::Starred(value) => runtime_jit_deopt_expr_supported(value, support),
        })
}

fn runtime_jit_deopt_binop_supported(kind: blockpy_intrinsics::BinOpKind) -> bool {
    matches!(
        kind,
        blockpy_intrinsics::BinOpKind::Add
            | blockpy_intrinsics::BinOpKind::Sub
            | blockpy_intrinsics::BinOpKind::Mul
            | blockpy_intrinsics::BinOpKind::MatMul
            | blockpy_intrinsics::BinOpKind::TrueDiv
            | blockpy_intrinsics::BinOpKind::FloorDiv
            | blockpy_intrinsics::BinOpKind::Mod
            | blockpy_intrinsics::BinOpKind::Pow
            | blockpy_intrinsics::BinOpKind::LShift
            | blockpy_intrinsics::BinOpKind::RShift
            | blockpy_intrinsics::BinOpKind::Or
            | blockpy_intrinsics::BinOpKind::Xor
            | blockpy_intrinsics::BinOpKind::And
            | blockpy_intrinsics::BinOpKind::Eq
            | blockpy_intrinsics::BinOpKind::Ne
            | blockpy_intrinsics::BinOpKind::Lt
            | blockpy_intrinsics::BinOpKind::Le
            | blockpy_intrinsics::BinOpKind::Gt
            | blockpy_intrinsics::BinOpKind::Ge
            | blockpy_intrinsics::BinOpKind::Contains
            | blockpy_intrinsics::BinOpKind::Is
            | blockpy_intrinsics::BinOpKind::InplaceAdd
            | blockpy_intrinsics::BinOpKind::InplaceSub
            | blockpy_intrinsics::BinOpKind::InplaceMul
            | blockpy_intrinsics::BinOpKind::InplaceMatMul
            | blockpy_intrinsics::BinOpKind::InplaceTrueDiv
            | blockpy_intrinsics::BinOpKind::InplaceFloorDiv
            | blockpy_intrinsics::BinOpKind::InplaceMod
            | blockpy_intrinsics::BinOpKind::InplacePow
            | blockpy_intrinsics::BinOpKind::InplaceLShift
            | blockpy_intrinsics::BinOpKind::InplaceRShift
            | blockpy_intrinsics::BinOpKind::InplaceOr
            | blockpy_intrinsics::BinOpKind::InplaceXor
            | blockpy_intrinsics::BinOpKind::InplaceAnd
    )
}

fn runtime_jit_deopt_term_supported(
    term: &BlockTerm<InstrBlockPy>,
    support: &RuntimeJitDeoptSupportCtx<'_>,
) -> bool {
    match term {
        BlockTerm::Return(value) => runtime_jit_deopt_expr_supported(value, support),
        BlockTerm::Jump(edge) => edge.args.iter().all(|arg| {
            matches!(
                arg,
                BlockArg::Name(_)
                    | BlockArg::None
                    | BlockArg::CurrentException
                    | BlockArg::AbruptKind(_)
            )
        }),
        BlockTerm::IfTerm(if_term) => runtime_jit_deopt_expr_supported(&if_term.test, support),
        BlockTerm::BranchTable(branch) => runtime_jit_deopt_expr_supported(&branch.index, support),
        BlockTerm::Raise(raise) => raise
            .exc
            .as_ref()
            .is_none_or(|exc| runtime_jit_deopt_expr_supported(exc, support)),
    }
}

fn runtime_jit_deopt_exception_edge_supported(edge: &BlockEdge) -> bool {
    edge.args.iter().all(|arg| {
        matches!(
            arg,
            BlockArg::Name(_) | BlockArg::None | BlockArg::CurrentException
        )
    })
}

impl RuntimeJitDeoptInvocation<'_> {
    pub(super) unsafe fn from_raw<'a>(
        deopt_table: ObjPtr,
        globals_obj: ObjPtr,
        function_data_obj: ObjPtr,
        record_ordinal: i64,
        live_values: ObjPtr,
        live_value_count: i64,
    ) -> Result<RuntimeJitDeoptInvocation<'a>, String> {
        if deopt_table.is_null() {
            return Err(format!(
                "null deopt table pointer, ordinal {record_ordinal}, live values {live_value_count}"
            ));
        }
        let table = unsafe { &*(deopt_table.cast::<RuntimeJitDeoptTable>()) };
        let record = table.record_for_ordinal(record_ordinal)?;
        record.validate_live_value_buffer(live_values, live_value_count)?;
        let live_value_count = usize::try_from(live_value_count).map_err(|_| {
            format!("live value count {live_value_count} is negative or does not fit usize")
        })?;
        let live_values = if live_value_count == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(live_values.cast::<ObjPtr>(), live_value_count) }
        };
        Ok(RuntimeJitDeoptInvocation {
            table,
            record,
            globals_obj,
            function_data_obj,
            live_values,
        })
    }

    pub(super) fn record(&self) -> &'_ RuntimeJitDeoptRecord {
        self.record
    }

    pub(super) fn function(&self) -> &BlockPyFunction<BlockPyModuleShape> {
        self.table.function()
    }

    pub(super) fn globals_obj(&self) -> ObjPtr {
        self.globals_obj
    }

    pub(super) fn function_data_obj(&self) -> ObjPtr {
        self.function_data_obj
    }

    pub(super) fn module_constant_ptr(&self, constant_index: u32) -> Result<ObjPtr, String> {
        self.table
            .module_constant_ptr(ModuleConstantId(constant_index as usize))
    }

    pub(super) fn live_bindings(
        &self,
    ) -> impl ExactSizeIterator<Item = (&'_ LocalEnvResumeBinding, ObjPtr)> + '_ {
        self.record
            .locals()
            .iter()
            .zip(self.live_values.iter().copied())
    }

    pub(super) fn materialize_locals(&self) -> Result<RuntimeJitDeoptLocals<'_>, String> {
        RuntimeJitDeoptLocals::from_live_bindings(self.live_bindings())
            .map_err(|err| format!("{err}; while materializing locals for {}", self.describe()))
    }

    pub(super) fn describe(&self) -> String {
        format!(
            "{}, live values {}",
            self.record().describe(self.table.function_id()),
            self.live_bindings().len()
        )
    }
}

impl<'a> RuntimeJitDeoptLocals<'a> {
    fn local_slot_index(location: LocalLocation) -> Result<usize, String> {
        usize::try_from(location.slot()).map_err(|_| {
            format!("local location {location:?} does not fit in a runtime local slot index")
        })
    }

    fn insert_local(&mut self, local: RuntimeJitDeoptLocal<'a>) -> Result<(), String> {
        let slot = Self::local_slot_index(local.binding.location)?;
        if slot >= self.locals_by_slot.len() {
            self.locals_by_slot.resize_with(slot + 1, || None);
        }
        if self.locals_by_slot[slot].is_some() {
            return Err(format!(
                "duplicate deopt local location {:?} while reconstructing runtime locals",
                local.binding.location
            ));
        }
        self.locals_by_slot[slot] = Some(local);
        self.len += 1;
        Ok(())
    }

    fn from_live_bindings(
        live_bindings: impl IntoIterator<Item = (&'a LocalEnvResumeBinding, ObjPtr)>,
    ) -> Result<Self, String> {
        let mut names = HashSet::new();
        let mut locations = HashSet::new();
        let mut locals = Self {
            locals_by_slot: Vec::new(),
            len: 0,
        };
        for (binding, value) in live_bindings {
            if !names.insert(binding.name.as_str()) {
                return Err(format!(
                    "duplicate deopt local name {} while reconstructing runtime locals",
                    binding.name
                ));
            }
            if !locations.insert(binding.location) {
                return Err(format!(
                    "duplicate deopt local location {:?} while reconstructing runtime locals",
                    binding.location
                ));
            }
            match binding.binding {
                LocalEnvResumeBindingState::Bound if value.is_null() => {
                    return Err(format!(
                        "deopt local {} at {:?} from {:?} is definitely bound but has a null value",
                        binding.name, binding.location, binding.source
                    ));
                }
                LocalEnvResumeBindingState::Unbound if !value.is_null() => {
                    return Err(format!(
                        "deopt local {} at {:?} from {:?} is unbound but has a non-null value",
                        binding.name, binding.location, binding.source
                    ));
                }
                _ => {}
            }
            locals.insert_local(RuntimeJitDeoptLocal {
                binding,
                value,
                release_on_frame_exit: transient_local_needs_decref(binding.ownership),
            })?;
        }
        Ok(locals)
    }

    pub(super) fn from_prevalidated_live_values(
        bindings: &'a [LocalEnvResumeBinding],
        values: &[ObjPtr],
    ) -> Result<Self, String> {
        if bindings.len() != values.len() {
            return Err(format!(
                "entry interpreter expected {} local values but got {}",
                bindings.len(),
                values.len()
            ));
        }
        let mut locals = Self {
            locals_by_slot: Vec::with_capacity(bindings.len()),
            len: 0,
        };
        for (binding, value) in bindings.iter().zip(values.iter().copied()) {
            match binding.binding {
                LocalEnvResumeBindingState::Bound if value.is_null() => {
                    return Err(format!(
                        "entry local {} at {:?} is definitely bound but has a null value",
                        binding.name, binding.location
                    ));
                }
                LocalEnvResumeBindingState::Unbound if !value.is_null() => {
                    return Err(format!(
                        "entry local {} at {:?} is unbound but has a non-null value",
                        binding.name, binding.location
                    ));
                }
                _ => {}
            }
            locals.insert_local(RuntimeJitDeoptLocal {
                binding,
                value,
                release_on_frame_exit: transient_local_needs_decref(binding.ownership),
            })?;
        }
        Ok(locals)
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn describe(&self) -> String {
        let names = self
            .locals_by_slot
            .iter()
            .filter_map(Option::as_ref)
            .map(|local| {
                format!(
                    "{}@{}={:p}",
                    local.binding.name,
                    local.binding.location.slot(),
                    local.value
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("reconstructed locals {} [{}]", self.len(), names)
    }

    pub(super) fn get_by_name(&self, name: &str) -> Option<&RuntimeJitDeoptLocal<'a>> {
        self.locals_by_slot
            .iter()
            .filter_map(Option::as_ref)
            .find(|local| local.binding.name == name)
    }

    pub(super) fn get_by_name_mut(&mut self, name: &str) -> Option<&mut RuntimeJitDeoptLocal<'a>> {
        self.locals_by_slot
            .iter_mut()
            .filter_map(Option::as_mut)
            .find(|local| local.binding.name == name)
    }

    pub(super) fn get_by_location(
        &self,
        location: LocalLocation,
    ) -> Option<&RuntimeJitDeoptLocal<'a>> {
        let slot = usize::try_from(location.slot()).ok()?;
        self.locals_by_slot.get(slot)?.as_ref()
    }

    pub(super) fn get_by_location_mut(
        &mut self,
        location: LocalLocation,
    ) -> Option<&mut RuntimeJitDeoptLocal<'a>> {
        let slot = usize::try_from(location.slot()).ok()?;
        self.locals_by_slot.get_mut(slot)?.as_mut()
    }

    pub(super) unsafe fn release_frame_owned_values(&mut self) {
        for local in self.locals_by_slot.iter_mut().filter_map(Option::as_mut) {
            unsafe {
                local.release_frame_owned_value();
            }
        }
    }
}

impl RuntimeJitDeoptLocal<'_> {
    pub(super) fn binding(&self) -> &'_ LocalEnvResumeBinding {
        self.binding
    }

    pub(super) fn value(&self) -> ObjPtr {
        self.value
    }

    pub(super) unsafe fn replace_with_owned_value(&mut self, value: ObjPtr) {
        unsafe {
            self.release_frame_owned_value();
        }
        self.value = value;
        self.release_on_frame_exit = true;
    }

    pub(super) unsafe fn delete_value(&mut self) {
        unsafe {
            self.release_frame_owned_value();
        }
    }

    unsafe fn release_frame_owned_value(&mut self) {
        if self.release_on_frame_exit && !self.value.is_null() {
            unsafe {
                ffi::Py_DECREF(self.value.cast::<ffi::PyObject>());
            }
        }
        self.value = std::ptr::null_mut();
        self.release_on_frame_exit = false;
    }
}
