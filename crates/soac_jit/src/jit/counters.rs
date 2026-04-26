use crate::counter::TopValueCounter;
use crate::module_type::CounterRuntimeSlot;
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::FunctionBuilder;
use soac_core::block_py::{CounterBranchId, CounterId};

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
