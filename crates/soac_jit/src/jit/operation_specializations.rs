use super::counters::{
    CounterRef, emit_record_top_value_counter_slot, top_value_counter_slot_for_id,
};
use super::inspection::RefcountFamily;
use super::intrinsics::{OperationEmitState, increment_counter_with_state};
use super::symbols::{
    CpythonTypeSymbol, RelocTypeRef, register_runtime_type_for_key,
    reloc_type_ref_from_typed_attr_owner_ref, typed_attr_owner_ref_from_reloc_type_ref,
};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use pyo3::{Bound, Py, PyAny, Python, ffi};
use soac_core::block_py::{
    CounterId, GetItem, HasSemanticInstrId, Instr, InstrId, RuntimeFunctionId, SetItem,
};
use soac_core::profile::CounterDumpTypeKey;
use soac_ir_blockpy::InstrBlockPy;
use soac_ir_typed::plan_v3::{
    EXACT_LIST_EXACT_INT_ITEM_SHAPE_TAG, EXACT_TUPLE_EXACT_INT_ITEM_SHAPE_TAG,
    ExactListItemAccessKind, ExactListItemShape,
    IndexedFieldAccessKind as PlanV3IndexedFieldAccessKind,
};
use soac_ir_typed::{
    PyObjFacts, TypedExactListItemAccessPlan, TypedIndexedFieldGuard, TypedIndexedFieldPlanSource,
};
use soac_opt::access_emission_v3::{
    IndexedFieldRuntimeAccessRequest as OptV3IndexedFieldRuntimeAccessRequest,
    ResolvedIndexedFieldAccess as OptV3ResolvedIndexedFieldAccessFromOpt,
};
use std::mem::offset_of;

const PYLONG_COMPACT_TAG_LIMIT: i64 = 2 << 3;
const PYLONG_SIGN_MASK: i64 = 3;

unsafe extern "C" {
    fn PyUnstable_Type_AssignVersionTag(type_obj: *mut ffi::PyTypeObject) -> i32;
}

#[repr(C)]
struct RawPyLongValue {
    lv_tag: usize,
    ob_digit: [u32; 1],
}

#[repr(C)]
struct RawPyLongObject {
    ob_base: ffi::PyObject,
    long_value: RawPyLongValue,
}

#[repr(C)]
struct RawPyTupleObject {
    ob_base: ffi::PyVarObject,
    ob_hash: isize,
    ob_item: [*mut ffi::PyObject; 1],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExactListItemLoweringPlan {
    access: ExactListItemAccessKind,
    shape: ExactListItemShape,
    counter_source: Option<(RuntimeFunctionId, InstrId)>,
}

impl ExactListItemLoweringPlan {
    fn receiver_type(self) -> CpythonTypeSymbol {
        match self.shape {
            ExactListItemShape::ExactListExactInt => CpythonTypeSymbol::List,
            ExactListItemShape::ExactTupleExactInt => CpythonTypeSymbol::Tuple,
        }
    }

    fn expect_exact_list_exact_int(self, expected_access: ExactListItemAccessKind) {
        assert_eq!(
            self.access, expected_access,
            "exact-list item plan {:?} reached {:?} lowering",
            self.access, expected_access
        );
        assert!(
            matches!(
                (self.shape, expected_access),
                (ExactListItemShape::ExactListExactInt, _)
                    | (
                        ExactListItemShape::ExactTupleExactInt,
                        ExactListItemAccessKind::Get
                    )
            ),
            "exact item plan {:?} does not support {:?} lowering",
            self.shape,
            expected_access,
        );
    }
}

pub(super) fn lowering_plan_from_typed_exact_list_item(
    plan: &TypedExactListItemAccessPlan,
    expected_access: ExactListItemAccessKind,
) -> ExactListItemLoweringPlan {
    debug_assert_eq!(plan.access, expected_access);
    ExactListItemLoweringPlan {
        access: plan.access,
        shape: plan.shape,
        counter_source: plan
            .counter_source
            .map(|source| (source.function_id, source.instr_id)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FieldIndexSpecialization {
    pub(super) expected_index: u32,
    pub(super) owner_type_ref: RelocTypeRef,
    pub(super) type_version: u32,
}

impl FieldIndexSpecialization {
    pub(super) fn to_typed_guard(&self) -> TypedIndexedFieldGuard {
        TypedIndexedFieldGuard {
            expected_index: self.expected_index,
            owner_type_ref: typed_attr_owner_ref_from_reloc_type_ref(&self.owner_type_ref),
            type_version: self.type_version,
        }
    }
}

pub(super) type OptV3ResolvedIndexedFieldAccess =
    OptV3ResolvedIndexedFieldAccessFromOpt<FieldIndexSpecialization>;

// A field profile is not permission to execute namespace or allocation hooks.
// Scan exact Unicode keys without hashing, rich comparison, or a UTF-8 cache
// allocation. An unsupported key anywhere declines the speculative lookup.
unsafe fn field_dictionary_value(
    dictionary: *mut ffi::PyObject,
    name: &str,
) -> Result<Option<*mut ffi::PyObject>, ()> {
    if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
        return Err(());
    }
    let mut position = 0;
    let mut key = std::ptr::null_mut();
    let mut value = std::ptr::null_mut();
    let mut found = None;
    while unsafe { ffi::PyDict_Next(dictionary, &mut position, &mut key, &mut value) } != 0 {
        if unsafe { ffi::PyUnicode_CheckExact(key) } == 0 {
            return Err(());
        }
        if unsafe { ffi::PyUnicode_GetLength(key) } == name.chars().count() as ffi::Py_ssize_t
            && name.chars().enumerate().all(|(index, character)| {
                (unsafe { ffi::PyUnicode_ReadChar(key, index as ffi::Py_ssize_t) })
                    == u32::from(character)
            })
        {
            found = Some(value);
        }
    }
    Ok(found)
}

fn indexed_field_owner_type_for_type_key(type_key: &CounterDumpTypeKey) -> Option<Py<PyAny>> {
    if type_key.module_name.is_empty()
        || type_key.qualname.is_empty()
        || type_key
            .qualname
            .split('.')
            .any(|part| part.is_empty() || part == "<locals>")
        || unsafe { super::PyThreadState_GetUnchecked() }.is_null()
    {
        return None;
    }
    let py = unsafe { Python::assume_attached() };
    let modules = unsafe { ffi::PyImport_GetModuleDict() };
    let module = unsafe { field_dictionary_value(modules, &type_key.module_name) }.ok()??;
    if unsafe { ffi::PyModule_Check(module) } == 0 {
        return None;
    }
    let module = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, module) };
    let mut parts = type_key.qualname.split('.');
    let current =
        unsafe { field_dictionary_value(ffi::PyModule_GetDict(module.as_ptr()), parts.next()?) }
            .ok()??;
    let mut current = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, current) };
    for part in parts {
        if unsafe { ffi::PyType_Check(current.as_ptr()) } == 0
            || unsafe { ffi::Py_TYPE(current.as_ptr()) } != std::ptr::addr_of_mut!(ffi::PyType_Type)
        {
            return None;
        }
        let dictionary = unsafe { ffi::PyType_GetDict(current.as_ptr().cast()) };
        let next = unsafe { field_dictionary_value(dictionary, part) };
        unsafe { ffi::Py_XDECREF(dictionary) };
        let next = next.ok()??;
        // Own the child before releasing the preceding namespace owner.
        current = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, next) };
    }
    if unsafe { ffi::PyType_Check(current.as_ptr()) } == 0
        || unsafe { ffi::Py_TYPE(current.as_ptr()) } != std::ptr::addr_of_mut!(ffi::PyType_Type)
    {
        return None;
    }
    // Retain the independently resolved type through guard materialization.
    // Never dereference an unverified raw registry address or synthesize a
    // missing binding through module/metaclass __getattr__.
    Some(current.unbind())
}

fn owner_type_has_no_class_binding_for_attr(
    owner_type: *mut ffi::PyTypeObject,
    attr_name: &str,
) -> bool {
    let mro = unsafe { (*owner_type).tp_mro };
    if mro.is_null() || unsafe { ffi::PyTuple_CheckExact(mro) } == 0 {
        return false;
    }
    for index in 0..unsafe { ffi::PyTuple_GET_SIZE(mro) } {
        let base = unsafe { ffi::PyTuple_GET_ITEM(mro, index) };
        if unsafe { ffi::PyType_Check(base) } == 0 {
            return false;
        }
        // Static builtin dictionaries are per-interpreter, not in tp_dict.
        // The accessor returns a new reference to an already type-owned dict.
        let dictionary = unsafe { ffi::PyType_GetDict(base.cast()) };
        let binding = unsafe { field_dictionary_value(dictionary, attr_name) };
        unsafe { ffi::Py_XDECREF(dictionary) };
        if !matches!(binding, Ok(None)) {
            return false;
        }
    }
    true
}

fn field_index_specialization_for_type(
    owner_type: *mut ffi::PyTypeObject,
    type_key: &CounterDumpTypeKey,
    attr_name: &str,
    expected_index: u32,
) -> Result<Option<FieldIndexSpecialization>, String> {
    if owner_type.is_null() {
        return Ok(None);
    }
    // A generic get/set slot and a matching shared-key index do not authorize
    // skipping the actual native ordinary-dictionary write policy. This pure
    // query includes inherited and terminal policies; keep generic access.
    if unsafe { crate::_PySOAC_HasOrdinaryInstanceWrites(owner_type) } != 0 {
        return Ok(None);
    }
    const PY_TPFLAGS_MANAGED_DICT_SOAC: u64 = 1 << 4;
    const PY_TPFLAGS_INLINE_VALUES_SOAC: u64 = 1 << 2;
    let required_flags =
        ffi::Py_TPFLAGS_HEAPTYPE | PY_TPFLAGS_MANAGED_DICT_SOAC | PY_TPFLAGS_INLINE_VALUES_SOAC;
    if unsafe { (*owner_type).tp_flags } & required_flags != required_flags {
        return Ok(None);
    }
    if attr_name.contains('\0') {
        return Err(format!(
            "field specialization attr contains NUL: {attr_name:?}"
        ));
    }
    let has_generic_getattr = unsafe { (*owner_type).tp_getattro }.is_some_and(|getattr| {
        std::ptr::fn_addr_eq(
            getattr,
            ffi::PyObject_GenericGetAttr
                as unsafe extern "C" fn(
                    *mut ffi::PyObject,
                    *mut ffi::PyObject,
                ) -> *mut ffi::PyObject,
        )
    });
    let has_generic_setattr = unsafe { (*owner_type).tp_setattro }.is_some_and(|setattr| {
        std::ptr::fn_addr_eq(
            setattr,
            ffi::PyObject_GenericSetAttr
                as unsafe extern "C" fn(
                    *mut ffi::PyObject,
                    *mut ffi::PyObject,
                    *mut ffi::PyObject,
                ) -> i32,
        )
    });
    if !has_generic_getattr
        || !has_generic_setattr
        || !owner_type_has_no_class_binding_for_attr(owner_type, attr_name)
    {
        return Ok(None);
    }

    if unsafe { (*owner_type).tp_version_tag } == 0 {
        let _ = unsafe { PyUnstable_Type_AssignVersionTag(owner_type) };
    }
    let type_version = unsafe { (*owner_type).tp_version_tag };
    if type_version == 0 {
        return Ok(None);
    }
    // The raw namespace walk already resolved this exact profile key; do not
    // re-read __module__/__qualname__ through Python attribute dispatch.
    let owner_type_ref = RelocTypeRef::TypeKey(type_key.clone());
    register_runtime_type_for_key(type_key, owner_type);

    Ok(Some(FieldIndexSpecialization {
        expected_index,
        owner_type_ref,
        type_version,
    }))
}

pub(super) fn field_index_specialization_from_opt_v3(
    request: &OptV3IndexedFieldRuntimeAccessRequest,
) -> Result<Option<FieldIndexSpecialization>, String> {
    let Some(owner_type) = indexed_field_owner_type_for_type_key(&request.type_key) else {
        return Ok(None);
    };
    field_index_specialization_for_type(
        owner_type.as_ptr().cast(),
        &request.type_key,
        request.attr_name.as_str(),
        request.expected_index,
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct IndexedFieldLoweringPlan {
    pub(super) source: TypedIndexedFieldPlanSource,
    pub(super) access: PlanV3IndexedFieldAccessKind,
    pub(super) specializations: Vec<FieldIndexSpecialization>,
}

impl IndexedFieldLoweringPlan {
    pub(super) fn for_access(
        instr_id: InstrId,
        source: TypedIndexedFieldPlanSource,
        guards: &[TypedIndexedFieldGuard],
        expected_access: PlanV3IndexedFieldAccessKind,
    ) -> Result<Option<Self>, String> {
        match source {
            TypedIndexedFieldPlanSource::OptimizationPlanV3 => {
                Self::from_typed_guards(instr_id, source, guards, expected_access)
            }
        }
    }

    fn from_typed_guards(
        instr_id: InstrId,
        source: TypedIndexedFieldPlanSource,
        guards: &[TypedIndexedFieldGuard],
        expected_access: PlanV3IndexedFieldAccessKind,
    ) -> Result<Option<Self>, String> {
        if guards.is_empty() {
            if source == TypedIndexedFieldPlanSource::OptimizationPlanV3 {
                return Err(format!(
                    "optimizer v3 indexed-field {:?} for {instr_id} lost all typed codegen guards",
                    expected_access
                ));
            }
            return Ok(None);
        }

        let mut specializations = Vec::with_capacity(guards.len());
        for guard in guards {
            let Some(specialization) = field_index_specialization_from_typed_guard(guard) else {
                continue;
            };
            push_unique_specialization(&mut specializations, specialization);
        }

        if specializations.is_empty() {
            if source == TypedIndexedFieldPlanSource::OptimizationPlanV3 {
                return Err(format!(
                    "optimizer v3 indexed-field {:?} for {instr_id} has no resolvable typed codegen guards",
                    expected_access
                ));
            }
            return Ok(None);
        }

        Ok(Some(Self {
            source,
            access: expected_access,
            specializations,
        }))
    }

    pub(super) fn require_type_ptr(
        &self,
        instr_id: InstrId,
        specialization: &FieldIndexSpecialization,
        owner_type: Option<ir::Value>,
    ) -> Result<Option<ir::Value>, String> {
        match owner_type {
            Some(owner_type) => Ok(Some(owner_type)),
            None if self.source == TypedIndexedFieldPlanSource::OptimizationPlanV3 => Err(format!(
                "prevalidated optimizer v3 indexed-field {:?} for {instr_id} could not bind runtime owner type reference {:?}",
                self.access, specialization.owner_type_ref
            )),
            None => Ok(None),
        }
    }
}

fn field_index_specialization_from_typed_guard(
    guard: &TypedIndexedFieldGuard,
) -> Option<FieldIndexSpecialization> {
    Some(FieldIndexSpecialization {
        expected_index: guard.expected_index,
        owner_type_ref: reloc_type_ref_from_typed_attr_owner_ref(&guard.owner_type_ref)?,
        type_version: guard.type_version,
    })
}

fn push_unique_specialization(
    specializations: &mut Vec<FieldIndexSpecialization>,
    specialization: FieldIndexSpecialization,
) {
    if !specializations.contains(&specialization) {
        specializations.push(specialization);
    }
}

pub(super) fn emit_getitem<'fb>(
    op: &GetItem<InstrBlockPy>,
    state: &mut impl OperationEmitState<'fb, InstrBlockPy>,
) -> ir::Value {
    emit_getitem_with_plan(op, state, None)
}

pub(super) fn emit_getitem_with_plan<'fb, E: Instr>(
    op: &GetItem<E>,
    state: &mut impl OperationEmitState<'fb, E>,
    lowering_plan: Option<ExactListItemLoweringPlan>,
) -> ir::Value {
    let instr_id = op.semantic_instr_id();
    let shape_counter_id = state
        .ctx()
        .getitem_shape_counter_ids
        .get(&instr_id)
        .copied();
    let counter_source = lowering_plan.and_then(|plan| plan.counter_source);
    let specialized_hit_counter_id = counter_source
        .and_then(|source| {
            state
                .ctx()
                .getitem_specialized_hit_counter_ids_by_source
                .get(&source)
                .copied()
        })
        .or_else(|| {
            state
                .ctx()
                .getitem_specialized_hit_counter_ids
                .get(&instr_id)
                .copied()
        });
    let specialized_fallback_counter_id = counter_source
        .and_then(|source| {
            state
                .ctx()
                .getitem_specialized_fallback_counter_ids_by_source
                .get(&source)
                .copied()
        })
        .or_else(|| {
            state
                .ctx()
                .getitem_specialized_fallback_counter_ids
                .get(&instr_id)
                .copied()
        });
    if shape_counter_id.is_none() && lowering_plan.is_none() {
        return emit_generic_getitem_from_exprs(op, state);
    }

    if shape_counter_id.is_none()
        && let Some(plan) = lowering_plan
        && state.can_emit_guarded_i64_index_arg(op.index.as_ref())
    {
        return emit_exact_list_item_getitem_from_guarded_i64_index(
            op,
            state,
            plan,
            specialized_hit_counter_id,
            specialized_fallback_counter_id,
        );
    }

    let arg_values = state.emit_arg_values(&[op.value.as_ref(), op.index.as_ref()]);
    if let Some(counter_id) = shape_counter_id {
        let shape = emit_item_dispatch_shape_from_arg_values(state, &arg_values);
        emit_record_item_shape_counter(state, counter_id, shape);
    }

    if let Some(plan) = lowering_plan {
        return emit_exact_list_item_getitem_from_plan(
            state,
            &arg_values,
            plan,
            specialized_hit_counter_id,
            specialized_fallback_counter_id,
        );
    }

    let result = emit_generic_getitem_from_arg_values(state, &arg_values);
    state.release_arg_values(&arg_values);
    state.finish_owned_result(result)
}

pub(super) fn emit_setitem<'fb>(
    op: &SetItem<InstrBlockPy>,
    state: &mut impl OperationEmitState<'fb, InstrBlockPy>,
) -> ir::Value {
    emit_setitem_with_plan(op, state, None)
}

pub(super) fn emit_setitem_with_plan<'fb, E: Instr>(
    op: &SetItem<E>,
    state: &mut impl OperationEmitState<'fb, E>,
    lowering_plan: Option<ExactListItemLoweringPlan>,
) -> ir::Value {
    let instr_id = op.semantic_instr_id();
    let shape_counter_id = state
        .ctx()
        .setitem_shape_counter_ids
        .get(&instr_id)
        .copied();
    let counter_source = lowering_plan.and_then(|plan| plan.counter_source);
    let specialized_hit_counter_id = counter_source
        .and_then(|source| {
            state
                .ctx()
                .setitem_specialized_hit_counter_ids_by_source
                .get(&source)
                .copied()
        })
        .or_else(|| {
            state
                .ctx()
                .setitem_specialized_hit_counter_ids
                .get(&instr_id)
                .copied()
        });
    let specialized_fallback_counter_id = counter_source
        .and_then(|source| {
            state
                .ctx()
                .setitem_specialized_fallback_counter_ids_by_source
                .get(&source)
                .copied()
        })
        .or_else(|| {
            state
                .ctx()
                .setitem_specialized_fallback_counter_ids
                .get(&instr_id)
                .copied()
        });
    if shape_counter_id.is_none() && lowering_plan.is_none() {
        return emit_generic_setitem_from_exprs(op, state);
    }

    // This emitter generates the replacement separately in both guard arms.
    // Consuming/effectful replacements use the boxed-index path below: it
    // evaluates all inputs once and guards the receiver after that evaluation.
    if shape_counter_id.is_none()
        && let Some(plan) = lowering_plan
        && state.can_emit_guarded_i64_index_arg(op.index.as_ref())
        && state.can_replay_setitem_replacement_after_guard(op.replacement.as_ref())
    {
        return emit_exact_list_item_setitem_from_guarded_i64_index(
            op,
            state,
            plan,
            specialized_hit_counter_id,
            specialized_fallback_counter_id,
        );
    }

    let arg_values = state.emit_arg_values(&[
        op.value.as_ref(),
        op.index.as_ref(),
        op.replacement.as_ref(),
    ]);
    if let Some(counter_id) = shape_counter_id {
        let shape = emit_item_dispatch_shape_from_arg_values(state, &arg_values[..2]);
        emit_record_item_shape_counter(state, counter_id, shape);
    }

    if let Some(plan) = lowering_plan {
        return emit_exact_list_item_setitem_from_plan(
            state,
            &arg_values,
            plan,
            specialized_hit_counter_id,
            specialized_fallback_counter_id,
        );
    }

    let result = emit_generic_setitem_from_arg_values(state, &arg_values);
    release_setitem_inputs(state, &arg_values);
    state.finish_owned_result(result)
}

fn emit_generic_getitem_from_exprs<'fb, E: Instr>(
    op: &GetItem<E>,
    state: &mut impl OperationEmitState<'fb, E>,
) -> ir::Value {
    let arg_values = state.emit_arg_values(&[op.value.as_ref(), op.index.as_ref()]);
    let result = emit_generic_getitem_from_arg_values(state, &arg_values);
    state.release_arg_values(&arg_values);
    state.finish_owned_result(result)
}

fn release_setitem_inputs<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    inputs: &[(ir::Value, bool)],
) {
    assert_eq!(inputs.len(), 3);
    let ordered = SetItem::<InstrBlockPy>::INPUT_RELEASE_ORDER.map(|index| inputs[index]);
    state.release_arg_values(&ordered);
}

fn emit_generic_setitem_from_exprs<'fb, E: Instr>(
    op: &SetItem<E>,
    state: &mut impl OperationEmitState<'fb, E>,
) -> ir::Value {
    let arg_values = state.emit_arg_values(&[
        op.value.as_ref(),
        op.index.as_ref(),
        op.replacement.as_ref(),
    ]);
    let result = emit_generic_setitem_from_arg_values(state, &arg_values);
    release_setitem_inputs(state, &arg_values);
    state.finish_owned_result(result)
}

fn emit_record_item_shape_counter<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    counter_id: CounterId,
    shape: ir::Value,
) {
    let counter_slot = top_value_counter_slot_for_id(state.ctx().counter_slots_by_id, counter_id)
        .unwrap_or_else(|err| panic!("{err}"));
    let top_value_counter_base_value = state
        .ctx()
        .consts
        .top_value_counter_base_value
        .unwrap_or_else(|| {
            panic!(
                "missing top-value counter base for counter id {}",
                counter_id.0
            )
        });
    let record_top_value_sample_ref =
        state.ctx().record_top_value_sample_ref.unwrap_or_else(|| {
            panic!(
                "missing top-value counter helper import for counter id {}",
                counter_id.0
            )
        });
    emit_record_top_value_counter_slot(
        state.fb(),
        top_value_counter_base_value,
        counter_slot,
        shape,
        record_top_value_sample_ref,
    );
}

fn emit_item_dispatch_shape_from_arg_values<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    debug_assert_eq!(arg_values.len(), 2);
    let i64_ty = state.ctx().consts.i64_ty;
    let Some(list_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::List))
    else {
        return state.fb().ins().iconst(i64_ty, 0);
    };
    let Some(tuple_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Tuple))
    else {
        return state.fb().ins().iconst(i64_ty, 0);
    };
    let Some(long_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Long))
    else {
        return state.fb().ins().iconst(i64_ty, 0);
    };

    let ptr_ty = state.ctx().consts.ptr_ty;
    let zero_shape = state.fb().ins().iconst(i64_ty, 0);
    let exact_list_exact_int_shape = state
        .fb()
        .ins()
        .iconst(i64_ty, EXACT_LIST_EXACT_INT_ITEM_SHAPE_TAG as i64);
    let exact_tuple_exact_int_shape = state
        .fb()
        .ins()
        .iconst(i64_ty, EXACT_TUPLE_EXACT_INT_ITEM_SHAPE_TAG as i64);
    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, i64_ty);
    let obj = arg_values[0].0;
    let key = arg_values[1].0;

    let obj_not_null_block = state.fb().create_block();
    let obj_is_null = state
        .fb()
        .ins()
        .icmp_imm(ir::condcodes::IntCC::Equal, obj, 0);
    state.fb().ins().brif(
        obj_is_null,
        result_block,
        &[ir::BlockArg::Value(zero_shape)],
        obj_not_null_block,
        &[],
    );

    state.fb().switch_to_block(obj_not_null_block);
    let obj_type = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyObject, ob_type) as i32,
    );
    let is_exact_list = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_type, list_type);
    let tuple_guard_block = state.fb().create_block();
    let key_guard_block = state.fb().create_block();
    state.fb().append_block_param(key_guard_block, i64_ty);
    state.fb().ins().brif(
        is_exact_list,
        key_guard_block,
        &[ir::BlockArg::Value(exact_list_exact_int_shape)],
        tuple_guard_block,
        &[],
    );

    state.fb().switch_to_block(tuple_guard_block);
    let is_exact_tuple = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_type, tuple_type);
    state.fb().ins().brif(
        is_exact_tuple,
        key_guard_block,
        &[ir::BlockArg::Value(exact_tuple_exact_int_shape)],
        result_block,
        &[ir::BlockArg::Value(zero_shape)],
    );

    state.fb().switch_to_block(key_guard_block);
    let exact_item_shape = state.fb().block_params(key_guard_block)[0];
    let key_is_null = state
        .fb()
        .ins()
        .icmp_imm(ir::condcodes::IntCC::Equal, key, 0);
    let key_not_null_block = state.fb().create_block();
    state.fb().ins().brif(
        key_is_null,
        result_block,
        &[ir::BlockArg::Value(zero_shape)],
        key_not_null_block,
        &[],
    );

    state.fb().switch_to_block(key_not_null_block);
    let key_type = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        key,
        offset_of!(ffi::PyObject, ob_type) as i32,
    );
    let key_is_exact_long = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, key_type, long_type);
    state.fb().ins().brif(
        key_is_exact_long,
        result_block,
        &[ir::BlockArg::Value(exact_item_shape)],
        result_block,
        &[ir::BlockArg::Value(zero_shape)],
    );

    state.fb().switch_to_block(result_block);
    state.fb().block_params(result_block)[0]
}

fn emit_generic_getitem_from_arg_values<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    debug_assert_eq!(arg_values.len(), 2);
    let pyobject_getitem_ref = state.ctx().pyobject_getitem_ref;
    let call_inst = state
        .fb()
        .ins()
        .call(pyobject_getitem_ref, &[arg_values[0].0, arg_values[1].0]);
    state.fb().inst_results(call_inst)[0]
}

fn emit_generic_setitem_from_arg_values<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    debug_assert_eq!(arg_values.len(), 3);
    let pyobject_setitem_ref = state.ctx().pyobject_setitem_ref;
    let call_inst = state.fb().ins().call(
        pyobject_setitem_ref,
        &[arg_values[0].0, arg_values[1].0, arg_values[2].0],
    );
    state.fb().inst_results(call_inst)[0]
}

fn emit_exact_list_item_getitem_from_plan<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<CounterRef>,
    specialized_fallback_counter_id: Option<CounterRef>,
) -> ir::Value {
    plan.expect_exact_list_exact_int(ExactListItemAccessKind::Get);
    emit_exact_list_exact_int_getitem(
        state,
        arg_values,
        plan,
        specialized_hit_counter_id,
        specialized_fallback_counter_id,
    )
}

fn emit_exact_list_item_setitem_from_plan<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<CounterRef>,
    specialized_fallback_counter_id: Option<CounterRef>,
) -> ir::Value {
    plan.expect_exact_list_exact_int(ExactListItemAccessKind::Set);
    emit_exact_list_exact_int_setitem(
        state,
        arg_values,
        plan,
        specialized_hit_counter_id,
        specialized_fallback_counter_id,
    )
}

fn emit_exact_list_exact_compact_int_in_bounds_guard<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    plan: ExactListItemLoweringPlan,
    expected_access: ExactListItemAccessKind,
    obj: ir::Value,
    key: ir::Value,
    sequence_type: ir::Value,
    long_type: ir::Value,
    guard_miss_block: ir::Block,
) -> ir::Value {
    plan.expect_exact_list_exact_int(expected_access);

    let ptr_ty = state.ctx().consts.ptr_ty;
    let i64_ty = state.ctx().consts.i64_ty;
    let i32_ty = state.ctx().consts.i32_ty;

    let obj_not_null_block = state.fb().create_block();
    let obj_is_null = state
        .fb()
        .ins()
        .icmp_imm(ir::condcodes::IntCC::Equal, obj, 0);
    state
        .fb()
        .ins()
        .brif(obj_is_null, guard_miss_block, &[], obj_not_null_block, &[]);

    state.fb().switch_to_block(obj_not_null_block);
    let obj_type = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyObject, ob_type) as i32,
    );
    let is_exact_sequence =
        state
            .fb()
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, obj_type, sequence_type);
    let key_guard_block = state.fb().create_block();
    state.fb().ins().brif(
        is_exact_sequence,
        key_guard_block,
        &[],
        guard_miss_block,
        &[],
    );

    state.fb().switch_to_block(key_guard_block);
    let key_is_null = state
        .fb()
        .ins()
        .icmp_imm(ir::condcodes::IntCC::Equal, key, 0);
    let key_not_null_block = state.fb().create_block();
    state
        .fb()
        .ins()
        .brif(key_is_null, guard_miss_block, &[], key_not_null_block, &[]);

    state.fb().switch_to_block(key_not_null_block);
    let key_type = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        key,
        offset_of!(ffi::PyObject, ob_type) as i32,
    );
    let key_is_exact_long = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, key_type, long_type);
    let compact_index_block = state.fb().create_block();
    state.fb().ins().brif(
        key_is_exact_long,
        compact_index_block,
        &[],
        guard_miss_block,
        &[],
    );

    state.fb().switch_to_block(compact_index_block);
    let lv_tag_offset =
        offset_of!(RawPyLongObject, long_value) as i32 + offset_of!(RawPyLongValue, lv_tag) as i32;
    let digit_offset = offset_of!(RawPyLongObject, long_value) as i32
        + offset_of!(RawPyLongValue, ob_digit) as i32;
    let lv_tag = state
        .fb()
        .ins()
        .load(i64_ty, ir::MemFlags::trusted(), key, lv_tag_offset);
    let is_compact_long = state.fb().ins().icmp_imm(
        ir::condcodes::IntCC::UnsignedLessThan,
        lv_tag,
        PYLONG_COMPACT_TAG_LIMIT,
    );
    let digit_i32 = state
        .fb()
        .ins()
        .load(i32_ty, ir::MemFlags::trusted(), key, digit_offset);
    let digit_i64 = state.fb().ins().uextend(i64_ty, digit_i32);
    let sign_mask = state.fb().ins().iconst(i64_ty, PYLONG_SIGN_MASK);
    let sign_bits = state.fb().ins().band(lv_tag, sign_mask);
    let one = state.fb().ins().iconst(i64_ty, 1);
    let sign = state.fb().ins().isub(one, sign_bits);
    let raw_index = state.fb().ins().imul(sign, digit_i64);
    let index_block = state.fb().create_block();
    state.fb().append_block_param(index_block, i64_ty);
    state.fb().ins().brif(
        is_compact_long,
        index_block,
        &[ir::BlockArg::Value(raw_index)],
        guard_miss_block,
        &[],
    );

    state.fb().switch_to_block(index_block);
    let raw_index = state.fb().block_params(index_block)[0];
    let sequence_len = state.fb().ins().load(
        i64_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyVarObject, ob_size) as i32,
    );
    let negative_index_block = state.fb().create_block();
    let nonnegative_index_block = state.fb().create_block();
    let normalized_index_block = state.fb().create_block();
    state
        .fb()
        .append_block_param(normalized_index_block, i64_ty);
    let is_negative_index =
        state
            .fb()
            .ins()
            .icmp_imm(ir::condcodes::IntCC::SignedLessThan, raw_index, 0);
    state.fb().ins().brif(
        is_negative_index,
        negative_index_block,
        &[],
        nonnegative_index_block,
        &[],
    );

    state.fb().switch_to_block(negative_index_block);
    let adjusted_index = state.fb().ins().iadd(raw_index, sequence_len);
    state.fb().ins().jump(
        normalized_index_block,
        &[ir::BlockArg::Value(adjusted_index)],
    );

    state.fb().switch_to_block(nonnegative_index_block);
    state
        .fb()
        .ins()
        .jump(normalized_index_block, &[ir::BlockArg::Value(raw_index)]);

    state.fb().switch_to_block(normalized_index_block);
    let normalized_index = state.fb().block_params(normalized_index_block)[0];
    let index_ge_zero = state.fb().ins().icmp_imm(
        ir::condcodes::IntCC::SignedGreaterThanOrEqual,
        normalized_index,
        0,
    );
    let index_lt_len = state.fb().ins().icmp(
        ir::condcodes::IntCC::SignedLessThan,
        normalized_index,
        sequence_len,
    );
    let index_in_bounds = state.fb().ins().band(index_ge_zero, index_lt_len);
    let direct_access_block = state.fb().create_block();
    state.fb().ins().brif(
        index_in_bounds,
        direct_access_block,
        &[],
        guard_miss_block,
        &[],
    );

    state.fb().switch_to_block(direct_access_block);
    normalized_index
}

fn emit_exact_list_i64_index_in_bounds_guard<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    plan: ExactListItemLoweringPlan,
    expected_access: ExactListItemAccessKind,
    obj: ir::Value,
    raw_index: ir::Value,
    sequence_type: ir::Value,
    guard_miss_block: ir::Block,
) -> ir::Value {
    plan.expect_exact_list_exact_int(expected_access);

    let ptr_ty = state.ctx().consts.ptr_ty;
    let i64_ty = state.ctx().consts.i64_ty;

    let obj_not_null_block = state.fb().create_block();
    let obj_is_null = state
        .fb()
        .ins()
        .icmp_imm(ir::condcodes::IntCC::Equal, obj, 0);
    state
        .fb()
        .ins()
        .brif(obj_is_null, guard_miss_block, &[], obj_not_null_block, &[]);

    state.fb().switch_to_block(obj_not_null_block);
    let obj_type = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyObject, ob_type) as i32,
    );
    let is_exact_sequence =
        state
            .fb()
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, obj_type, sequence_type);
    let index_block = state.fb().create_block();
    state
        .fb()
        .ins()
        .brif(is_exact_sequence, index_block, &[], guard_miss_block, &[]);

    state.fb().switch_to_block(index_block);
    let sequence_len = state.fb().ins().load(
        i64_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyVarObject, ob_size) as i32,
    );
    let negative_index_block = state.fb().create_block();
    let nonnegative_index_block = state.fb().create_block();
    let normalized_index_block = state.fb().create_block();
    state
        .fb()
        .append_block_param(normalized_index_block, i64_ty);
    let is_negative_index =
        state
            .fb()
            .ins()
            .icmp_imm(ir::condcodes::IntCC::SignedLessThan, raw_index, 0);
    state.fb().ins().brif(
        is_negative_index,
        negative_index_block,
        &[],
        nonnegative_index_block,
        &[],
    );

    state.fb().switch_to_block(negative_index_block);
    let adjusted_index = state.fb().ins().iadd(raw_index, sequence_len);
    state.fb().ins().jump(
        normalized_index_block,
        &[ir::BlockArg::Value(adjusted_index)],
    );

    state.fb().switch_to_block(nonnegative_index_block);
    state
        .fb()
        .ins()
        .jump(normalized_index_block, &[ir::BlockArg::Value(raw_index)]);

    state.fb().switch_to_block(normalized_index_block);
    let normalized_index = state.fb().block_params(normalized_index_block)[0];
    let index_ge_zero = state.fb().ins().icmp_imm(
        ir::condcodes::IntCC::SignedGreaterThanOrEqual,
        normalized_index,
        0,
    );
    let index_lt_len = state.fb().ins().icmp(
        ir::condcodes::IntCC::SignedLessThan,
        normalized_index,
        sequence_len,
    );
    let index_in_bounds = state.fb().ins().band(index_ge_zero, index_lt_len);
    let direct_access_block = state.fb().create_block();
    state.fb().ins().brif(
        index_in_bounds,
        direct_access_block,
        &[],
        guard_miss_block,
        &[],
    );

    state.fb().switch_to_block(direct_access_block);
    normalized_index
}

fn emit_exact_sequence_item_address<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    obj: ir::Value,
    normalized_index: ir::Value,
    shape: ExactListItemShape,
) -> ir::Value {
    let ptr_ty = state.ctx().consts.ptr_ty;
    let items = match shape {
        ExactListItemShape::ExactListExactInt => state.fb().ins().load(
            ptr_ty,
            ir::MemFlags::trusted(),
            obj,
            offset_of!(ffi::PyListObject, ob_item) as i32,
        ),
        ExactListItemShape::ExactTupleExactInt => state.fb().ins().iadd_imm(
            obj,
            i64::try_from(offset_of!(RawPyTupleObject, ob_item))
                .expect("raw tuple item offset should fit in i64"),
        ),
    };
    let item_offset = state.fb().ins().ishl_imm(normalized_index, 3);
    state.fb().ins().iadd(items, item_offset)
}

fn emit_exact_list_exact_int_getitem<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<CounterRef>,
    specialized_fallback_counter_id: Option<CounterRef>,
) -> ir::Value {
    let Some(sequence_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(plan.receiver_type()))
    else {
        let result = emit_generic_getitem_from_arg_values(state, arg_values);
        state.release_arg_values(arg_values);
        return state.finish_owned_result(result);
    };
    let Some(long_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Long))
    else {
        let result = emit_generic_getitem_from_arg_values(state, arg_values);
        state.release_arg_values(arg_values);
        return state.finish_owned_result(result);
    };

    let ptr_ty = state.ctx().consts.ptr_ty;

    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);
    let guard_miss_block = fallback_block;

    let obj = arg_values[0].0;
    let key = arg_values[1].0;
    let normalized_index = emit_exact_list_exact_compact_int_in_bounds_guard(
        state,
        plan,
        ExactListItemAccessKind::Get,
        obj,
        key,
        sequence_type,
        long_type,
        guard_miss_block,
    );
    increment_counter_with_state(state, specialized_hit_counter_id);
    let item_addr = emit_exact_sequence_item_address(state, obj, normalized_index, plan.shape);
    let item = state
        .fb()
        .ins()
        .load(ptr_ty, ir::MemFlags::trusted(), item_addr, 0);
    state.emit_incref_for_family(
        item,
        Some(PyObjFacts::unknown().with_non_null_ref()),
        RefcountFamily::BorrowedResultClone,
    );
    state.release_arg_values(arg_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(item)]);

    state.fb().switch_to_block(fallback_block);
    increment_counter_with_state(state, specialized_fallback_counter_id);
    let fallback_value = emit_generic_getitem_from_arg_values(state, arg_values);
    state.release_arg_values(arg_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);

    state.fb().switch_to_block(result_block);
    let result = state.fb().block_params(result_block)[0];
    state.finish_owned_result(result)
}

fn emit_exact_list_item_getitem_from_guarded_i64_index<'fb, E: Instr>(
    op: &GetItem<E>,
    state: &mut impl OperationEmitState<'fb, E>,
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<CounterRef>,
    specialized_fallback_counter_id: Option<CounterRef>,
) -> ir::Value {
    plan.expect_exact_list_exact_int(ExactListItemAccessKind::Get);
    let Some(sequence_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(plan.receiver_type()))
    else {
        return emit_generic_getitem_from_exprs(op, state);
    };

    let ptr_ty = state.ctx().consts.ptr_ty;

    let obj_values = state.emit_arg_values(&[op.value.as_ref()]);
    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);

    let obj = obj_values[0].0;
    let raw_index = state
        .emit_guarded_i64_index_arg(op.index.as_ref(), fallback_block)
        .expect("guarded item index should emit after can_emit_guarded_i64_index_arg");
    let normalized_index = emit_exact_list_i64_index_in_bounds_guard(
        state,
        plan,
        ExactListItemAccessKind::Get,
        obj,
        raw_index,
        sequence_type,
        fallback_block,
    );
    increment_counter_with_state(state, specialized_hit_counter_id);
    let item_addr = emit_exact_sequence_item_address(state, obj, normalized_index, plan.shape);
    let item = state
        .fb()
        .ins()
        .load(ptr_ty, ir::MemFlags::trusted(), item_addr, 0);
    state.emit_incref_for_family(
        item,
        Some(PyObjFacts::unknown().with_non_null_ref()),
        RefcountFamily::BorrowedResultClone,
    );
    state.release_arg_values(&obj_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(item)]);

    state.fb().switch_to_block(fallback_block);
    increment_counter_with_state(state, specialized_fallback_counter_id);
    let key_values = state.emit_arg_values(&[op.index.as_ref()]);
    let arg_values = [obj_values[0], key_values[0]];
    let fallback_value = emit_generic_getitem_from_arg_values(state, &arg_values);
    state.release_arg_values(&arg_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);

    state.fb().switch_to_block(result_block);
    let result = state.fb().block_params(result_block)[0];
    state.finish_owned_result(result)
}

fn emit_exact_list_exact_int_setitem<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<CounterRef>,
    specialized_fallback_counter_id: Option<CounterRef>,
) -> ir::Value {
    debug_assert_eq!(arg_values.len(), 3);
    let Some(list_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::List))
    else {
        let result = emit_generic_setitem_from_arg_values(state, arg_values);
        release_setitem_inputs(state, arg_values);
        return state.finish_owned_result(result);
    };
    let Some(long_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Long))
    else {
        let result = emit_generic_setitem_from_arg_values(state, arg_values);
        release_setitem_inputs(state, arg_values);
        return state.finish_owned_result(result);
    };

    let ptr_ty = state.ctx().consts.ptr_ty;

    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);
    let guard_miss_block = fallback_block;

    let obj = arg_values[0].0;
    let key = arg_values[1].0;
    let replacement = arg_values[2].0;
    let normalized_index = emit_exact_list_exact_compact_int_in_bounds_guard(
        state,
        plan,
        ExactListItemAccessKind::Set,
        obj,
        key,
        list_type,
        long_type,
        guard_miss_block,
    );
    let replacement_is_null =
        state
            .fb()
            .ins()
            .icmp_imm(ir::condcodes::IntCC::Equal, replacement, 0);
    let replacement_not_null_block = state.fb().create_block();
    state.fb().ins().brif(
        replacement_is_null,
        guard_miss_block,
        &[],
        replacement_not_null_block,
        &[],
    );

    state.fb().switch_to_block(replacement_not_null_block);
    increment_counter_with_state(state, specialized_hit_counter_id);
    let items = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyListObject, ob_item) as i32,
    );
    let item_offset = state.fb().ins().ishl_imm(normalized_index, 3);
    let item_addr = state.fb().ins().iadd(items, item_offset);
    let old_item = state
        .fb()
        .ins()
        .load(ptr_ty, ir::MemFlags::trusted(), item_addr, 0);
    if arg_values[2].1 {
        state.emit_incref_for_family(
            replacement,
            Some(PyObjFacts::unknown().with_non_null_ref()),
            RefcountFamily::ContainerStoreClone,
        );
    }
    state
        .fb()
        .ins()
        .store(ir::MemFlags::trusted(), replacement, item_addr, 0);
    state.emit_decref_for_family(
        old_item,
        Some(PyObjFacts::unknown().with_non_null_ref()),
        RefcountFamily::ContainerOverwriteRelease,
    );
    // The replacement's owned input was transferred into the list above.
    release_setitem_inputs(state, &[arg_values[0], arg_values[1], (replacement, true)]);
    let none = state.emit_owned_module_constant(state.ctx().consts.none_constant_id);
    state.emit_incref_for_family(
        none,
        Some(PyObjFacts::none_singleton()),
        RefcountFamily::ConstantClone,
    );
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(none)]);

    state.fb().switch_to_block(fallback_block);
    increment_counter_with_state(state, specialized_fallback_counter_id);
    let fallback_value = emit_generic_setitem_from_arg_values(state, arg_values);
    release_setitem_inputs(state, arg_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);

    state.fb().switch_to_block(result_block);
    let result = state.fb().block_params(result_block)[0];
    state.finish_owned_result(result)
}

fn emit_exact_list_item_setitem_from_guarded_i64_index<'fb, E: Instr>(
    op: &SetItem<E>,
    state: &mut impl OperationEmitState<'fb, E>,
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<CounterRef>,
    specialized_fallback_counter_id: Option<CounterRef>,
) -> ir::Value {
    plan.expect_exact_list_exact_int(ExactListItemAccessKind::Set);
    let Some(list_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::List))
    else {
        return emit_generic_setitem_from_exprs(op, state);
    };

    let ptr_ty = state.ctx().consts.ptr_ty;

    let obj_values = state.emit_arg_values(&[op.value.as_ref()]);
    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);

    let obj = obj_values[0].0;
    let raw_index = state
        .emit_guarded_i64_index_arg(op.index.as_ref(), fallback_block)
        .expect("guarded item index should emit after can_emit_guarded_i64_index_arg");
    let normalized_index = emit_exact_list_i64_index_in_bounds_guard(
        state,
        plan,
        ExactListItemAccessKind::Set,
        obj,
        raw_index,
        list_type,
        fallback_block,
    );
    let replacement_values = state.emit_arg_values(&[op.replacement.as_ref()]);
    let replacement = replacement_values[0].0;
    let replacement_is_null =
        state
            .fb()
            .ins()
            .icmp_imm(ir::condcodes::IntCC::Equal, replacement, 0);
    let replacement_not_null_block = state.fb().create_block();
    let replacement_null_block = state.fb().create_block();
    state.fb().ins().brif(
        replacement_is_null,
        replacement_null_block,
        &[],
        replacement_not_null_block,
        &[],
    );

    state.fb().switch_to_block(replacement_null_block);
    state.release_arg_values(&obj_values);
    state.release_arg_values(&replacement_values);
    let step_null_block = state.ctx().consts.step_null_block;
    let step_null_args = super::step_null_block_args(state.ctx());
    state.fb().ins().jump(step_null_block, &step_null_args);

    state.fb().switch_to_block(replacement_not_null_block);
    increment_counter_with_state(state, specialized_hit_counter_id);
    let items = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyListObject, ob_item) as i32,
    );
    let item_offset = state.fb().ins().ishl_imm(normalized_index, 3);
    let item_addr = state.fb().ins().iadd(items, item_offset);
    let old_item = state
        .fb()
        .ins()
        .load(ptr_ty, ir::MemFlags::trusted(), item_addr, 0);
    if replacement_values[0].1 {
        state.emit_incref_for_family(
            replacement,
            Some(PyObjFacts::unknown().with_non_null_ref()),
            RefcountFamily::ContainerStoreClone,
        );
    }
    state
        .fb()
        .ins()
        .store(ir::MemFlags::trusted(), replacement, item_addr, 0);
    state.emit_decref_for_family(
        old_item,
        Some(PyObjFacts::unknown().with_non_null_ref()),
        RefcountFamily::ContainerOverwriteRelease,
    );
    state.release_arg_values(&obj_values);
    let none = state.emit_owned_module_constant(state.ctx().consts.none_constant_id);
    state.emit_incref_for_family(
        none,
        Some(PyObjFacts::none_singleton()),
        RefcountFamily::ConstantClone,
    );
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(none)]);

    state.fb().switch_to_block(fallback_block);
    increment_counter_with_state(state, specialized_fallback_counter_id);
    let key_values = state.emit_arg_values(&[op.index.as_ref()]);
    let replacement_values = state.emit_arg_values(&[op.replacement.as_ref()]);
    let arg_values = [obj_values[0], key_values[0], replacement_values[0]];
    let fallback_value = emit_generic_setitem_from_arg_values(state, &arg_values);
    release_setitem_inputs(state, &arg_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);

    state.fb().switch_to_block(result_block);
    let result = state.fb().block_params(result_block)[0];
    state.finish_owned_result(result)
}
