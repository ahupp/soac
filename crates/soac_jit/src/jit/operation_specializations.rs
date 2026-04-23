use super::intrinsics::{OperationEmitState, increment_counter_with_state};
use super::{CpythonTypeSymbol, OptV3ExactListItemAccessPlan, RelocTypeRef};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use pyo3::ffi;
use soac_core::block_py::{CounterId, GetItem, HasSemanticInstrId, InstrId, SetItem};
use soac_lowering::passes::InstrCodegen;
use soac_opt::plan_v3::{EXACT_LIST_EXACT_INT_ITEM_SHAPE_TAG, ExactListItemAccessKind};
use std::mem::offset_of;

const PYLONG_COMPACT_TAG_LIMIT: i64 = 2 << 3;
const PYLONG_SIGN_MASK: i64 = 3;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ExactListItemLoweringPlan {
    access: ExactListItemAccessKind,
}

impl ExactListItemLoweringPlan {
    fn from_v3(
        plan: &OptV3ExactListItemAccessPlan,
        expected_access: ExactListItemAccessKind,
    ) -> Self {
        debug_assert_eq!(plan.access, expected_access);
        Self {
            access: plan.access,
        }
    }

    fn expect_exact_list_exact_int(self, expected_access: ExactListItemAccessKind) {
        assert_eq!(
            self.access, expected_access,
            "exact-list item plan {:?} reached {:?} lowering",
            self.access, expected_access
        );
    }
}

fn selected_v3_getitem_lowering_plan<'fb>(
    state: &impl OperationEmitState<'fb, InstrCodegen>,
    instr_id: InstrId,
) -> Option<ExactListItemLoweringPlan> {
    state
        .ctx()
        .opt_v3_exact_list_items_by_instr
        .get(&instr_id)
        .filter(|plan| plan.access == ExactListItemAccessKind::Get)
        .map(|plan| ExactListItemLoweringPlan::from_v3(plan, ExactListItemAccessKind::Get))
}

fn selected_v3_setitem_lowering_plan<'fb>(
    state: &impl OperationEmitState<'fb, InstrCodegen>,
    instr_id: InstrId,
) -> Option<ExactListItemLoweringPlan> {
    state
        .ctx()
        .opt_v3_exact_list_items_by_instr
        .get(&instr_id)
        .filter(|plan| plan.access == ExactListItemAccessKind::Set)
        .map(|plan| ExactListItemLoweringPlan::from_v3(plan, ExactListItemAccessKind::Set))
}

pub(super) fn emit_getitem<'fb>(
    op: &GetItem<InstrCodegen>,
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
) -> ir::Value {
    let instr_id = op.semantic_instr_id();
    let shape_counter_id = state
        .ctx()
        .getitem_shape_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_hit_counter_id = state
        .ctx()
        .getitem_specialized_hit_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_fallback_counter_id = state
        .ctx()
        .getitem_specialized_fallback_counter_ids
        .get(&instr_id)
        .copied();
    let lowering_plan = selected_v3_getitem_lowering_plan(state, instr_id);
    if shape_counter_id.is_none() && lowering_plan.is_none() {
        return emit_generic_getitem_from_exprs(op, state);
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
    op: &SetItem<InstrCodegen>,
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
) -> ir::Value {
    let instr_id = op.semantic_instr_id();
    let shape_counter_id = state
        .ctx()
        .setitem_shape_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_hit_counter_id = state
        .ctx()
        .setitem_specialized_hit_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_fallback_counter_id = state
        .ctx()
        .setitem_specialized_fallback_counter_ids
        .get(&instr_id)
        .copied();
    let lowering_plan = selected_v3_setitem_lowering_plan(state, instr_id);
    if shape_counter_id.is_none() && lowering_plan.is_none() {
        return emit_generic_setitem_from_exprs(op, state);
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
    state.release_arg_values(&arg_values);
    state.finish_owned_result(result)
}

fn emit_generic_getitem_from_exprs<'fb>(
    op: &GetItem<InstrCodegen>,
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
) -> ir::Value {
    let arg_values = state.emit_arg_values(&[op.value.as_ref(), op.index.as_ref()]);
    let result = emit_generic_getitem_from_arg_values(state, &arg_values);
    state.release_arg_values(&arg_values);
    state.finish_owned_result(result)
}

fn emit_generic_setitem_from_exprs<'fb>(
    op: &SetItem<InstrCodegen>,
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
) -> ir::Value {
    let arg_values = state.emit_arg_values(&[
        op.value.as_ref(),
        op.index.as_ref(),
        op.replacement.as_ref(),
    ]);
    let result = emit_generic_setitem_from_arg_values(state, &arg_values);
    state.release_arg_values(&arg_values);
    state.finish_owned_result(result)
}

fn emit_record_item_shape_counter<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    counter_id: CounterId,
    shape: ir::Value,
) {
    let counter_slot =
        super::top_value_counter_slot_for_id(state.ctx().counter_slots_by_id, counter_id)
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
    super::emit_record_top_value_counter_slot(
        state.fb(),
        top_value_counter_base_value,
        counter_slot,
        shape,
        record_top_value_sample_ref,
    );
}

fn emit_item_dispatch_shape_from_arg_values<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    debug_assert_eq!(arg_values.len(), 2);
    let i64_ty = state.ctx().consts.i64_ty;
    let Some(list_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::List))
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
    let key_guard_block = state.fb().create_block();
    state.fb().ins().brif(
        is_exact_list,
        key_guard_block,
        &[],
        result_block,
        &[ir::BlockArg::Value(zero_shape)],
    );

    state.fb().switch_to_block(key_guard_block);
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
        &[ir::BlockArg::Value(exact_list_exact_int_shape)],
        result_block,
        &[ir::BlockArg::Value(zero_shape)],
    );

    state.fb().switch_to_block(result_block);
    state.fb().block_params(result_block)[0]
}

fn emit_generic_getitem_from_arg_values<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
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

fn emit_generic_setitem_from_arg_values<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
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

fn emit_exact_list_item_getitem_from_plan<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    arg_values: &[(ir::Value, bool)],
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<super::CounterRef>,
    specialized_fallback_counter_id: Option<super::CounterRef>,
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

fn emit_exact_list_item_setitem_from_plan<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    arg_values: &[(ir::Value, bool)],
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<super::CounterRef>,
    specialized_fallback_counter_id: Option<super::CounterRef>,
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

fn emit_exact_list_exact_compact_int_in_bounds_guard<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    plan: ExactListItemLoweringPlan,
    expected_access: ExactListItemAccessKind,
    obj: ir::Value,
    key: ir::Value,
    list_type: ir::Value,
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
    let is_exact_list = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, obj_type, list_type);
    let key_guard_block = state.fb().create_block();
    state
        .fb()
        .ins()
        .brif(is_exact_list, key_guard_block, &[], guard_miss_block, &[]);

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
    let list_len = state.fb().ins().load(
        i64_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyListObject, ob_base) as i32
            + offset_of!(ffi::PyVarObject, ob_size) as i32,
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
    let adjusted_index = state.fb().ins().iadd(raw_index, list_len);
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
        list_len,
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

fn emit_exact_list_exact_int_getitem<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    arg_values: &[(ir::Value, bool)],
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<super::CounterRef>,
    specialized_fallback_counter_id: Option<super::CounterRef>,
) -> ir::Value {
    let Some(list_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::List))
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
    let incref_ref = state.ctx().incref_ref;

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
        list_type,
        long_type,
        guard_miss_block,
    );
    increment_counter_with_state(state, specialized_hit_counter_id);
    let items = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        obj,
        offset_of!(ffi::PyListObject, ob_item) as i32,
    );
    let item_offset = state.fb().ins().ishl_imm(normalized_index, 3);
    let item_addr = state.fb().ins().iadd(items, item_offset);
    let item = state
        .fb()
        .ins()
        .load(ptr_ty, ir::MemFlags::trusted(), item_addr, 0);
    state.fb().ins().call(incref_ref, &[item]);
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

fn emit_exact_list_exact_int_setitem<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    arg_values: &[(ir::Value, bool)],
    plan: ExactListItemLoweringPlan,
    specialized_hit_counter_id: Option<super::CounterRef>,
    specialized_fallback_counter_id: Option<super::CounterRef>,
) -> ir::Value {
    debug_assert_eq!(arg_values.len(), 3);
    let Some(list_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::List))
    else {
        let result = emit_generic_setitem_from_arg_values(state, arg_values);
        state.release_arg_values(arg_values);
        return state.finish_owned_result(result);
    };
    let Some(long_type) =
        state.emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Long))
    else {
        let result = emit_generic_setitem_from_arg_values(state, arg_values);
        state.release_arg_values(arg_values);
        return state.finish_owned_result(result);
    };

    let ptr_ty = state.ctx().consts.ptr_ty;
    let thread_state_value = state.ctx().consts.thread_state_value;
    let incref_ref = state.ctx().incref_ref;
    let decref_ref = state.ctx().decref_ref;

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
    state.fb().ins().call(incref_ref, &[replacement]);
    state
        .fb()
        .ins()
        .store(ir::MemFlags::trusted(), replacement, item_addr, 0);
    state
        .fb()
        .ins()
        .call(decref_ref, &[thread_state_value, old_item]);
    state.release_arg_values(arg_values);
    let none = state.emit_owned_module_constant(state.ctx().consts.none_constant_id);
    state.fb().ins().call(incref_ref, &[none]);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(none)]);

    state.fb().switch_to_block(fallback_block);
    increment_counter_with_state(state, specialized_fallback_counter_id);
    let fallback_value = emit_generic_setitem_from_arg_values(state, arg_values);
    state.release_arg_values(arg_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);

    state.fb().switch_to_block(result_block);
    let result = state.fb().block_params(result_block)[0];
    state.finish_owned_result(result)
}
