//! Actual class construction and lexical cells use shared owned storage.

use crate::block_py::{
    BindingKind, CallableScopeInfo, CallableSourceRole, CellBindingKind, ClassBindingInitialValue,
    ClassBindingPhase, ClassBodyFallback, EffectiveBinding, HasMeta, MakeCell, Meta, Store,
    UnresolvedName, WithMeta,
};
use crate::passes::ast_to_ast::context::{
    Context, NativeClassBodyBoundary, NativeClassLoweringPlan,
};
use crate::passes::InstrRuff;
use ruff_python_ast::{self as ast, HasNodeIndex};

pub(super) fn apply_native_class_scope(
    context: &Context,
    func: &ast::StmtFunctionDef,
    scope: &mut CallableScopeInfo,
) {
    if context.strict_source().is_none()
        || !scope
            .source_origin
            .as_ref()
            .is_some_and(|origin| origin.role == CallableSourceRole::ClassNamespace)
    {
        return;
    }
    let plan = context
        .native_class_plan(func.range)
        .expect("strict class namespace has its exact original native recipe");
    let class = &plan.scope;
    // Rebind inferred class-cell aliases to the actual construction cells,
    // without allocating a parallel cell for an outlined helper capture.
    let old_owners = scope
        .bindings
        .iter()
        .filter_map(|(name, binding)| {
            matches!(binding, BindingKind::Cell(CellBindingKind::Owner)).then_some(name.clone())
        })
        .collect::<Vec<_>>();
    let inferred_owners = old_owners
        .iter()
        .map(|name| {
            (
                name.clone(),
                scope.cell_storage_name(name),
                scope.cell_capture_source_name(name),
                scope.effective_load_bindings.get(name).cloned(),
                scope.effective_store_bindings.get(name).cloned(),
            )
        })
        .collect::<Vec<_>>();
    for name in old_owners {
        scope.bindings.remove(&name);
        scope.cell_storage_names.remove(&name);
        scope.effective_load_bindings.remove(&name);
        scope.effective_store_bindings.remove(&name);
    }
    scope.owned_cell_source_names.clear();
    // Incoming FREE cells retain their actual closure ordinals. Helpers that
    // capture an outer binding still use these cells, not class namespace keys.
    for ordinal in 0..class.node.freevar_count {
        let native = class
            .node
            .freevar_slot(ordinal)
            .expect("native class free ordinal");
        let source = scope
            .cell_capture_source_names
            .get(&native.name)
            .cloned()
            .unwrap_or_else(|| native.name.clone());
        let source_load = scope.effective_load_bindings.get(&native.name).cloned();
        let source_store = scope.effective_store_bindings.get(&native.name).cloned();
        scope.insert_binding_with_cell_names(
            &native.name,
            BindingKind::Cell(CellBindingKind::Capture),
            true,
            Some(native.name.clone()),
            Some(source),
        );
        // A class namespace key and an incoming FREE slot may have the same
        // spelling. Register the closure owner without changing the original
        // namespace/global decision for source operations without a slot row.
        if let Some(binding) = source_load {
            scope
                .effective_load_bindings
                .insert(native.name.clone(), binding);
        }
        if let Some(binding) = source_store {
            scope
                .effective_store_bindings
                .insert(native.name.clone(), binding);
        }
    }
    for name in [&class.namespace_binding, &plan.execution_binding] {
        scope.local_defs.insert(name.clone());
        scope.bindings.insert(name.clone(), BindingKind::Local);
        scope
            .effective_load_bindings
            .insert(name.clone(), EffectiveBinding::Local);
        scope
            .effective_store_bindings
            .insert(name.clone(), EffectiveBinding::Local);
    }
    for row in &class.slots {
        let native = &class.node.slots[row.slot.index as usize];
        scope.local_defs.insert(row.binding.clone());
        scope
            .bindings
            .insert(row.binding.clone(), BindingKind::Local);
        scope
            .effective_load_bindings
            .insert(row.binding.clone(), EffectiveBinding::Local);
        scope
            .effective_store_bindings
            .insert(row.binding.clone(), EffectiveBinding::Local);
        if native.kind.is_cell() {
            scope.owned_cell_source_names.insert(row.binding.clone());
            scope
                .cell_storage_names
                .insert(row.binding.clone(), row.binding.clone());
            let value = plan.value_binding(row.slot).to_owned();
            scope
                .cell_value_aliases
                .insert(value.clone(), row.binding.clone());
            // The value name is only an alias onto the registered raw cell.
            // Do not register another owned cell or allocate a second primary.
            scope.bindings.insert(value.clone(), BindingKind::Local);
            scope.effective_load_bindings.insert(
                value.clone(),
                EffectiveBinding::Cell(CellBindingKind::Owner),
            );
            scope
                .effective_store_bindings
                .insert(value, EffectiveBinding::Cell(CellBindingKind::Owner));
        }
    }
    for (logical, storage, capture_source, load, store) in inferred_owners {
        let mut cells = class.slots.iter().filter(|row| {
            let native = &class.node.slots[row.slot.index as usize];
            native.kind.is_cell() && !native.kind.is_free() && native.name == logical
        });
        let Some(cell) = cells.next() else {
            continue;
        };
        assert!(cells.next().is_none(), "one lexical class-owned cell");
        // These were already parser-resolved class-owned aliases. Preserve the
        // original namespace/global lookup decision for source Name operations.
        for alias in [storage, capture_source] {
            scope.cell_value_aliases.insert(alias, cell.binding.clone());
        }
        if !scope
            .bindings
            .get(&logical)
            .is_some_and(|binding| matches!(binding, BindingKind::Cell(CellBindingKind::Capture)))
        {
            scope
                .cell_value_aliases
                .insert(logical.clone(), cell.binding.clone());
        }
        if let Some(load) = load {
            scope.effective_load_bindings.insert(logical.clone(), load);
        }
        if let Some(store) = store {
            scope.effective_store_bindings.insert(logical, store);
        }
    }
    if let Some(classcell) = class
        .recipe
        .exports
        .iter()
        .find(|export| export.kind == crate::block_py::ClassBindingExportKind::ClassCell)
    {
        let cell = class
            .slot_binding(classcell.source)
            .expect("classcell storage");
        // Strict namespace functions receive the construction handle, not the
        // ordinary `_dp_classcell_arg`. Their outlined helpers still use the
        // parser's synthetic class-cell aliases; bind those to the actual
        // authenticated export without changing a same-spelled outer FREE.
        for alias in ["_dp_classcell", "_dp_cell__dp_classcell"] {
            scope
                .cell_value_aliases
                .insert(alias.into(), cell.to_owned());
        }
    }
    if let Some(cell) = context.class_annotation_cell(func.range) {
        let initializer = class
            .recipe
            .initializers
            .iter()
            .find(|init| init.value == ClassBindingInitialValue::ConditionalSetStore)
            .expect("conditional annotation owner requires its actual native initializer");
        let raw = class
            .slot_binding(initializer.slot)
            .expect("conditional current carrier");
        scope
            .cell_value_aliases
            .insert(cell.owner_binding.clone(), raw.to_owned());
        scope
            .bindings
            .insert(cell.owner_binding.clone(), BindingKind::Local);
        scope.effective_load_bindings.insert(
            cell.owner_binding.clone(),
            EffectiveBinding::Cell(CellBindingKind::Owner),
        );
        scope.effective_store_bindings.insert(
            cell.owner_binding,
            EffectiveBinding::Cell(CellBindingKind::Owner),
        );
    }
    scope.class_bindings = Some(class.clone());
}

fn name(name: &str) -> InstrRuff {
    crate::passes::ast_to_instr::from_ast_expr(crate::template::py_expr!("{name:id}", name = name))
}

fn expression(value: InstrRuff, meta: Meta) -> InstrRuff {
    crate::block_py::StmtExpr::new(Box::new(value))
        .with_meta(meta)
        .into()
}

fn lower_phase(
    plan: &NativeClassLoweringPlan,
    phase: ClassBindingPhase,
    meta: Meta,
) -> Vec<InstrRuff> {
    let mut output = Vec::new();
    for initializer in plan
        .scope
        .recipe
        .initializers
        .iter()
        .filter(|init| init.phase == phase)
    {
        let current = plan
            .scope
            .slot_binding(initializer.slot)
            .expect("native current carrier");
        let (target, value): (&str, InstrRuff) = match initializer.value {
            ClassBindingInitialValue::EmptyCell => (
                current,
                MakeCell::<InstrRuff>::empty()
                    .with_meta(meta.clone())
                    .into(),
            ),
            ClassBindingInitialValue::IncomingFree { ordinal } => {
                let native = plan
                    .scope
                    .node
                    .freevar_slot(ordinal)
                    .expect("native free ordinal");
                let incoming = crate::block_py::CellRefForName::new(native.name.clone(), None)
                    .with_meta(meta.clone())
                    .into();
                (current, incoming)
            }
            ClassBindingInitialValue::NamespaceStore => (
                plan.value_binding(initializer.slot),
                name(&plan.scope.namespace_binding),
            ),
            ClassBindingInitialValue::ConditionalSetStore => (
                plan.value_binding(initializer.slot),
                crate::block_py::BuildCollection::new(
                    crate::block_py::BuildCollectionKind::Set,
                    Vec::<InstrRuff>::new(),
                )
                .with_meta(meta.clone())
                .into(),
            ),
        };
        let store: InstrRuff = Store::new(UnresolvedName::from(target), Box::new(value))
            .with_meta(meta.clone())
            .into();
        output.push(expression(store, meta.clone()));
    }
    output
}

pub(super) fn lower_native_class_body(
    context: &Context,
    scope: &CallableScopeInfo,
    body: &[ast::Stmt],
    name_gen: &crate::block_py::FunctionNameGen,
) -> Vec<InstrRuff> {
    let mut output = Vec::new();
    for statement in body {
        if let Some((code, boundary)) = context.native_class_boundary(statement.node_index().load())
        {
            let class = scope
                .class_bindings
                .as_ref()
                .expect("class phase only belongs to its namespace");
            assert_eq!(
                code, class.node.id,
                "class phase cannot cross an activation"
            );
            let marker = crate::passes::ast_to_instr::from_ast_stmt(statement.clone());
            let plan = context.native_class_plan_by_code(code);
            output.extend(match boundary {
                NativeClassBodyBoundary::Initialize(phase) => {
                    lower_phase(&plan, phase, marker.meta())
                }
                NativeClassBodyBoundary::Complete => {
                    lower_completion(&plan, name_gen, marker.meta())
                }
            });
        } else {
            output.push(crate::passes::ast_to_instr::from_ast_stmt(
                statement.clone(),
            ));
        }
    }
    output
}

pub(super) fn apply_native_class_captures(
    context: &Context,
    function: &ast::StmtFunctionDef,
    parent: &CallableScopeInfo,
    child: &mut CallableScopeInfo,
) {
    use crate::block_py::CellCaptureProjection;
    let Some(class) = &parent.class_bindings else {
        return;
    };
    let Some(node) = context.native_class_child(class.node.id, function, child) else {
        // An ordinary compiler helper is not an original native direct child.
        // Its parser-resolved captured raw alias may nevertheless name a real
        // class construction cell (notably the delayed __class__ cell).
        let captures = child
            .bindings
            .iter()
            .filter_map(|(name, binding)| {
                if !matches!(binding, BindingKind::Cell(CellBindingKind::Capture)) {
                    return None;
                }
                let source = child.cell_capture_source_name(name);
                let current = parent.cell_value_aliases.get(&source)?;
                class
                    .slots
                    .iter()
                    .any(|row| row.binding == *current)
                    .then(|| (name.clone(), current.clone()))
            })
            .collect::<Vec<_>>();
        for (name, current) in captures {
            child
                .cell_capture_source_names
                .insert(name.clone(), current);
            child
                .cell_capture_projections
                .insert(name, CellCaptureProjection::CellObject);
        }
        return;
    };
    // An implicit annotation scope can see a class-local name even when an
    // outer function defines the same spelling. The original native unit's
    // complete freevar inventory decides whether its dictionary-first lookup
    // falls back to a cell or globals; transformed lexical inference cannot
    // add a capture that the original function never had.
    let globals = child
        .effective_load_bindings
        .iter()
        .filter_map(|(name, binding)| {
            (matches!(
                binding,
                EffectiveBinding::ClassBody(ClassBodyFallback::Cell)
            ) && child.binding_kind(name) == Some(BindingKind::Cell(CellBindingKind::Capture))
                && !(0..node.freevar_count)
                    .any(|ordinal| node.freevar_slot(ordinal).unwrap().name == *name))
            .then(|| name.clone())
        })
        .collect::<Vec<_>>();
    for name in globals {
        assert!(
            !node.slots.iter().any(|slot| slot.name == name)
                && !child
                    .cell_value_aliases
                    .values()
                    .any(|logical| logical == &name),
            "dictionary fallback cannot replace a native local or compiler cell projection"
        );
        child.cell_storage_names.remove(&name);
        child.cell_capture_source_names.remove(&name);
        child.cell_capture_projections.remove(&name);
        child.insert_binding(&name, BindingKind::Global, true, None);
        child
            .effective_load_bindings
            .insert(name, EffectiveBinding::ClassBody(ClassBodyFallback::Global));
    }
    for ordinal in 0..node.freevar_count {
        let native = node
            .freevar_slot(ordinal)
            .expect("native child freevar ordinal");
        let mut captures = class
            .recipe
            .captures
            .iter()
            .filter(|capture| capture.child == node.id && capture.freevar_ordinal == ordinal);
        let capture = captures
            .next()
            .expect("validated native child capture coverage");
        assert!(
            captures.all(|other| other.source == capture.source),
            "one native child freevar reads the same current slot at all duplicated creation sites"
        );
        let source = class
            .slot_binding(capture.source)
            .expect("native capture current slot belongs to parent class");
        assert!(class.node.slots[capture.source.index as usize]
            .kind
            .is_cell());
        // The native recipe selects the raw capture transport. It must not
        // replace an annotation/type-parameter scope's already selected
        // dictionary-first source lookup with a plain LOAD_DEREF equivalent.
        let source_lookup = child.effective_load_bindings.get(&native.name).cloned();
        child.insert_binding_with_cell_names(
            &native.name,
            BindingKind::Cell(CellBindingKind::Capture),
            true,
            Some(native.name.clone()),
            Some(source.to_owned()),
        );
        if let Some(binding @ EffectiveBinding::ClassBody(_)) = source_lookup {
            child
                .effective_load_bindings
                .insert(native.name.clone(), binding);
        }
        child
            .cell_capture_projections
            .insert(native.name.clone(), CellCaptureProjection::CellObject);
    }
}

fn lower_completion(
    plan: &NativeClassLoweringPlan,
    name_gen: &crate::block_py::FunctionNameGen,
    meta: Meta,
) -> Vec<InstrRuff> {
    use crate::block_py::{ClassBindingExportKind, StoreLifetime, TakeOperand};
    let mut output = Vec::new();
    let mut returned_cell = false;
    for export in &plan.scope.recipe.exports {
        let current = plan
            .scope
            .slot_binding(export.source)
            .expect("native export current slot");
        let key = match export.kind {
            ClassBindingExportKind::ClassDictCell => "__classdictcell__",
            ClassBindingExportKind::ClassCell => {
                assert!(!returned_cell, "one classcell export");
                // Native COPY keeps the return-cell operand live across STORE_NAME.
                let retain: InstrRuff =
                    Store::new(plan.return_binding.as_str(), Box::new(name(current)))
                        .with_lifetime(StoreLifetime::Operand {
                            unwind_order: name_gen.next_temporary_sequence(),
                        })
                        .with_meta(meta.clone())
                        .into();
                output.push(expression(retain, meta.clone()));
                returned_cell = true;
                "__classcell__"
            }
        };
        output.push(crate::passes::ast_to_instr::from_ast_stmt(
            crate::template::py_stmt!(
                "{namespace:id}[{key:literal}] = {current:id}",
                namespace = plan.scope.namespace_binding.as_str(),
                key = key,
                current = current,
            ),
        ));
    }
    let value = if returned_cell {
        TakeOperand::new(plan.return_binding.as_str())
            .with_meta(meta.clone())
            .into()
    } else {
        crate::passes::ast_to_instr::from_ast_expr(crate::template::py_expr!("None"))
    };
    output.push(
        crate::block_py::StmtReturn::new(Box::new(value))
            .with_meta(meta)
            .into(),
    );
    output
}
