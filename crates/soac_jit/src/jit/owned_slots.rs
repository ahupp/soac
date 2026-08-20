//! Mechanical transfers of boxed local or suspended owners. Explicit
//! expression-operand roles select the physical storage; source
//! names, cell contents, and speculative type facts never authorize a move.

use super::*;
use soac_core::block_py::{Instr, OperandLocation, TakeOperand};

struct RawOwner {
    value: ir::Value,
    facts: Option<PyObjFacts>,
    binding: ParamBindingFacts,
    unbound: bool,
}

fn physical_slot<'a>(
    location: LocalLocation,
    ctx: &'a JitEmitCtx<'_>,
) -> Result<(&'a str, ir::StackSlot), String> {
    let name = ctx
        .storage_layout
        .as_ref()
        .and_then(|layout| layout.stack_slots().get(location.slot() as usize))
        .ok_or("owned operand slot has no physical local")?;
    let slot = ctx
        .stack_slots
        .slot_for_name(name)
        .ok_or("owned operand slot has no planned boxed owner storage")?;
    Ok((name, slot))
}

/// Read a nullable raw owner without applying Python's checked-local load or
/// dereferencing a cell. LocalOnly and StackMirror are different owning edges.
fn read_owner(
    fb: &mut FunctionBuilder<'_>,
    location: LocalLocation,
    locals: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Result<RawOwner, String> {
    let (_, slot) = physical_slot(location, ctx)?;
    if let Some(index) = locals.entry_index_for_location(location) {
        let entry = &locals.entries[index];
        let value = match entry.binding {
            LocalBindingValue::PyObject { value, .. } | LocalBindingValue::Unbound { value } => {
                value
            }
            LocalBindingValue::ExactI64 { .. } | LocalBindingValue::I32Bool01 { .. } => {
                return Err("owned operand slot lost its boxed owner".into());
            }
        };
        if entry.storage == LocalEnvStorage::LocalOnly && entry.ref_kind() == LocalRefKind::Borrowed
        {
            return Err("owned operand slot cannot transfer a borrowed-only value".into());
        }
        return Ok(RawOwner {
            value: if entry.storage == LocalEnvStorage::StackMirror {
                fb.ins().stack_load(ctx.consts.ptr_ty, slot, 0)
            } else {
                value
            },
            facts: entry.py_facts(),
            binding: entry.binding_facts,
            unbound: entry.ref_kind() == LocalRefKind::Unbound,
        });
    }
    Ok(RawOwner {
        value: fb.ins().stack_load(ctx.consts.ptr_ty, slot, 0),
        facts: None,
        binding: ParamBindingFacts::MaybeUnbound,
        unbound: false,
    })
}

/// Publish an already-owned reference, with no INCREF or release of the old
/// edge. The caller performs every slot publication before any DECREF that
/// can invoke Python. Moving a LocalOnly owner also changes its SSA mirror.
fn publish_owner(
    fb: &mut FunctionBuilder<'_>,
    location: LocalLocation,
    owner: &RawOwner,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    let (name, slot) = physical_slot(location, ctx)?;
    fb.ins().stack_store(owner.value, slot, 0);
    let previous = locals
        .entry_index_for_location(location)
        .map(|index| locals.entries.remove(index));
    locals.entries.push(LocalEnvEntry::new(
        Some(location),
        name.to_owned(),
        previous.map(|entry| entry.aliases).unwrap_or_default(),
        if owner.unbound {
            LocalBindingValue::unbound(owner.value)
        } else {
            LocalBindingValue::pyobject(owner.value, LocalRefKind::Borrowed, owner.facts)
        },
        LocalEnvStorage::StackMirror,
        owner.binding,
    ));
    Ok(())
}

fn operand_name<'a>(location: OperandLocation, ctx: &'a JitEmitCtx<'_>) -> Result<&'a str, String> {
    match location {
        OperandLocation::Local(location) => physical_slot(location, ctx).map(|(name, _)| name),
        OperandLocation::Preserved(location) => ctx
            .storage_layout
            .as_ref()
            .and_then(|layout| layout.preserved_slots.get(location.slot() as usize))
            .map(|slot| slot.storage_name.as_str())
            .ok_or_else(|| "expression operand has no physical preserved slot".into()),
    }
}

fn read_operand_owner(
    fb: &mut FunctionBuilder<'_>,
    location: OperandLocation,
    locals: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Result<RawOwner, String> {
    match location {
        OperandLocation::Local(location) => read_owner(fb, location, locals, ctx),
        OperandLocation::Preserved(location) => {
            let values = preserved_values_base_value(ctx);
            let offset = preserved_values_slot_offset(location.slot())?;
            Ok(RawOwner {
                value: fb
                    .ins()
                    .load(ctx.consts.ptr_ty, ir::MemFlags::trusted(), values, offset),
                facts: None,
                binding: ParamBindingFacts::MaybeUnbound,
                unbound: false,
            })
        }
    }
}

fn publish_operand_owner(
    fb: &mut FunctionBuilder<'_>,
    location: OperandLocation,
    owner: &RawOwner,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Result<(), String> {
    match location {
        OperandLocation::Local(location) => publish_owner(fb, location, owner, locals, ctx),
        OperandLocation::Preserved(location) => {
            let values = preserved_values_base_value(ctx);
            let offset = preserved_values_slot_offset(location.slot())?;
            fb.ins()
                .store(ir::MemFlags::trusted(), owner.value, values, offset);
            Ok(())
        }
    }
}

/// Replace a validated operand's nullable owning edge. No reference is cloned
/// or released here: the caller retires the returned old edge only after the
/// new owner is published, including while preserving a pending exception.
pub(super) fn publish_operand_owned(
    fb: &mut FunctionBuilder<'_>,
    location: OperandLocation,
    new_owned: Option<ir::Value>,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Result<ir::Value, String> {
    let old = read_operand_owner(fb, location, locals, ctx)?;
    let owner = RawOwner {
        value: new_owned.unwrap_or_else(|| fb.ins().iconst(ctx.consts.ptr_ty, 0)),
        facts: new_owned.map(|_| PyObjFacts::unknown().with_non_null_ref()),
        binding: if new_owned.is_some() {
            ParamBindingFacts::DefinitelyBound
        } else {
            ParamBindingFacts::MaybeUnbound
        },
        unbound: new_owned.is_none(),
    };
    publish_operand_owner(fb, location, &owner, locals, ctx)?;
    Ok(old.value)
}

pub(super) fn local_diagnostic_name<'a>(
    scope: &'a soac_core::block_py::CallableScopeInfo,
    layout: Option<&StorageLayout>,
    location: LocalLocation,
    fallback: &'a str,
) -> &'a str {
    layout
        .and_then(|layout| {
            layout.class_bindings.as_ref()?.source_name_at(
                scope.class_bindings.as_ref()?,
                layout,
                location,
            )
        })
        .unwrap_or(fallback)
}

/// The caller has validated the expression-operand role. The existing owner
/// stays live through element evaluation and the native consuming insertion.
pub(super) fn borrow_operand(
    fb: &mut FunctionBuilder<'_>,
    location: OperandLocation,
    locals: &LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Result<ir::Value, String> {
    let owner = read_operand_owner(fb, location, locals, ctx)?;
    let name = operand_name(location, ctx)?;
    Ok(emit_checked_local_value_or_unbound(
        fb,
        name,
        Some(location.name_location()),
        owner.value,
        LocalRefKind::Borrowed,
        ctx,
        true,
    ))
}

pub(super) fn take_operand<I: Instr<Name = ResolvedName>>(
    fb: &mut FunctionBuilder<'_>,
    op: &TakeOperand<I>,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
) -> Result<SoacValue, String> {
    let layout = ctx
        .storage_layout
        .as_ref()
        .ok_or("operand take has no physical layout")?;
    let location = op.validate_resolved(layout)?;
    let owner = read_operand_owner(fb, location, locals, ctx)?;
    let name = operand_name(location, ctx)?;
    let value = emit_checked_local_value_or_unbound(
        fb,
        name,
        Some(location.name_location()),
        owner.value,
        LocalRefKind::Borrowed,
        ctx,
        true,
    );
    let empty = RawOwner {
        value: fb.ins().iconst(ctx.consts.ptr_ty, 0),
        facts: None,
        binding: ParamBindingFacts::MaybeUnbound,
        unbound: true,
    };
    publish_operand_owner(fb, location, &empty, locals, ctx)?;
    Ok(SoacValue::owned_pyobject(
        value,
        owner
            .facts
            .unwrap_or_else(PyObjFacts::unknown)
            .with_non_null_ref(),
    ))
}
