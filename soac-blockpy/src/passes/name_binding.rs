use crate::block_py::{
    build_storage_layout_from_capture_names, compute_make_function_capture_bindings_from_scope,
    compute_storage_layout_from_scope, core_runtime_positional_call_expr_with_meta, literal_expr,
    runtime_symbol, BindingKind, BindingPurpose, BindingTarget, BlockArg, BlockPyFunction,
    BlockPyModule, BlockPyNameLike, BlockTerm, Call, CallArgPositional, CallableScopeInfo,
    CallableScopeKind, CellBindingKind, CellCaptureBinding, CellLocation, CellRef, CellRefForName,
    ChildVisitable, ClassBodyFallback, ClosureInit, ClosureSlot, InstrLow, InstrUnresolved,
    CoreNumberLiteral, CoreNumberLiteralValue, CoreStringLiteral, Del, DelItem, EffectiveBinding,
    FunctionId, FunctionKind, HasMeta, Load, LocalLocation, InstrResolved, ResolvedName,
    MakeCell, MakeFunction, MapFunction, MapInstr, Mappable, NameLocation, SetItem, StorageLayout,
    Store, UnresolvedName, WithMeta,
};
use crate::passes::ruff_to_blockpy::{
    populate_exception_edge_args, rewrite_current_exception_in_core_blocks,
};
use crate::passes::{CoreBlockPyPass, ResolvedStorageBlockPyPass};
use ruff_python_ast::{self as ast};
use soac_macros::match_default;
use std::collections::{HashMap, HashSet};

fn is_internal_symbol(name: &str) -> bool {
    name.starts_with("_dp_") || name == "__soac__"
}

fn should_late_bind_name(name: &str, scope: &CallableScopeInfo) -> bool {
    !is_internal_symbol(name) || scope.honors_internal_binding(name)
}

fn core_string_expr(
    value: String,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrUnresolved {
    literal_expr(
        CoreStringLiteral { value },
        crate::block_py::Meta::new(node_index, range),
    )
}

fn core_int_expr(
    value: u64,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrUnresolved {
    let text = value.to_string();
    literal_expr(
        CoreNumberLiteral {
            value: CoreNumberLiteralValue::Int(
                ast::Int::from_str_radix(text.as_str(), 10, text.as_str())
                    .expect("function id should round-trip through Int"),
            ),
        },
        crate::block_py::Meta::new(node_index, range),
    )
}

fn globals_expr(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrUnresolved {
    core_runtime_positional_call_expr_with_meta("globals", node_index, range, Vec::new())
}

fn op_expr(operation: impl Into<InstrUnresolved>) -> InstrUnresolved {
    operation.into()
}

type CoreStmt = InstrUnresolved;

fn op_stmt(operation: impl Into<InstrUnresolved>) -> CoreStmt {
    op_expr(operation)
}

fn constant_location_expr(meta: crate::block_py::Meta, index: u32) -> InstrResolved {
    let name = ResolvedName {
        id: "__dp_constant".into(),
        location: NameLocation::Constant(index),
    };
    Load::new(name).with_meta(meta).into()
}
fn rewrite_global_name_load(name: ast::name::Name, meta: crate::block_py::Meta) -> InstrUnresolved {
    op_expr(Load::new(name).with_meta(meta))
}

fn rewrite_local_name_load(
    name: ast::name::Name,
    meta: crate::block_py::Meta,
    resolver: &NameBindingMapper<'_>,
) -> InstrUnresolved {
    let _ = resolver;
    rewrite_global_name_load(name, meta)
}

fn cell_expr_for_name(
    name: &str,
    scope: &CallableScopeInfo,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrUnresolved {
    let _ = scope;
    CellRefForName::new(name.to_string())
        .with_meta(crate::block_py::Meta::new(node_index, range))
        .into()
}

fn rewrite_cell_name_load(
    name: ast::name::Name,
    meta: crate::block_py::Meta,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) -> InstrUnresolved {
    let _ = (scope, resolver);
    rewrite_global_name_load(name, meta)
}

fn rewrite_raw_cell_storage_name_load(
    name: ast::name::Name,
    meta: crate::block_py::Meta,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) -> Option<InstrUnresolved> {
    let _ = (scope, resolver);
    resolve_cell_storage_name(scope, name.as_str())?;
    Some(rewrite_global_name_load(name, meta))
}

fn raw_load_name<N>(expr: &InstrLow<N>) -> Option<String>
where
    N: BlockPyNameLike,
{
    match expr {
        InstrLow::Load(op) => Some(op.name.id_str().to_string()),
        _ => None,
    }
}

fn rewrite_name_load(
    name: ast::name::Name,
    meta: crate::block_py::Meta,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) -> InstrUnresolved {
    if is_internal_symbol(name.as_str()) && !scope.honors_internal_binding(name.as_str()) {
        return Load::new(name).with_meta(meta).into();
    }

    if scope.scope_kind == CallableScopeKind::Class {
        return match scope.effective_binding(name.as_str(), BindingPurpose::Load) {
            Some(EffectiveBinding::ClassBody(ClassBodyFallback::Cell)) => {
                rewrite_class_name_load_cell(name, meta, scope)
            }
            Some(EffectiveBinding::Cell(_)) => rewrite_cell_name_load(name, meta, scope, resolver),
            Some(EffectiveBinding::Global) => rewrite_global_name_load(name, meta),
            Some(EffectiveBinding::Local) => rewrite_local_name_load(name, meta, resolver),
            Some(EffectiveBinding::ClassBody(ClassBodyFallback::Global)) | None => {
                rewrite_class_name_load_global(name, meta)
            }
        };
    }

    match scope.resolved_load_binding_kind(name.as_str()) {
        BindingKind::Cell(_) => rewrite_cell_name_load(name, meta, scope, resolver),
        BindingKind::Global => rewrite_global_name_load(name, meta),
        BindingKind::Local => rewrite_local_name_load(name, meta, resolver),
    }
}

fn should_rewrite_raw_name_load(name: &str, scope: &CallableScopeInfo) -> bool {
    if should_late_bind_name(name, scope) {
        return true;
    }

    matches!(
        scope.effective_binding(name, BindingPurpose::Load),
        Some(EffectiveBinding::Global | EffectiveBinding::Cell(_) | EffectiveBinding::ClassBody(_))
    )
}

fn rewrite_cell_ref_expr(
    logical_name: &str,
    _semantic: &CallableScopeInfo,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrUnresolved {
    op_expr(
        CellRefForName::new(logical_name.to_string())
            .with_meta(crate::block_py::Meta::new(node_index.clone(), range)),
    )
}

fn rewrite_global_binding_assign(
    target: UnresolvedName,
    value: InstrUnresolved,
    meta: crate::block_py::Meta,
) -> CoreStmt {
    op_stmt(Store::new(target, Box::new(value)).with_meta(meta))
}

fn rewrite_class_namespace_binding_assign(
    target: UnresolvedName,
    value: InstrUnresolved,
    meta: crate::block_py::Meta,
) -> CoreStmt {
    let bind_name = target.id_str().to_string();
    op_stmt(
        SetItem::new(
            Box::new(class_namespace_expr(meta.node_index.clone(), meta.range)),
            Box::new(core_string_expr(
                bind_name,
                meta.node_index.clone(),
                meta.range,
            )),
            Box::new(value),
        )
        .with_meta(meta),
    )
}

fn rewrite_cell_binding_assign(
    target: UnresolvedName,
    value: InstrUnresolved,
    meta: crate::block_py::Meta,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) -> CoreStmt {
    let _ = (scope, resolver);
    rewrite_global_binding_assign(target, value, meta)
}

fn rewrite_global_binding_delete_by_name(
    bind_name: ast::name::Name,
    meta: crate::block_py::Meta,
) -> CoreStmt {
    op_stmt(Del::new(bind_name, false).with_meta(meta))
}

fn rewrite_binding_delete(
    target: ast::name::Name,
    meta: crate::block_py::Meta,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) -> CoreStmt {
    let bind_name = target.to_string();
    if scope.is_cell_binding(bind_name.as_str()) {
        let _ = resolver;
        return op_stmt(Del::new(target, false).with_meta(meta));
    }
    match scope.binding_target_for_name(bind_name.as_str(), BindingPurpose::Store) {
        BindingTarget::Local => op_stmt(
            Store::new(
                target,
                Box::new(deleted_sentinel_expr(meta.node_index.clone(), meta.range)),
            )
            .with_meta(meta),
        ),
        BindingTarget::ModuleGlobal => {
            rewrite_global_binding_delete_by_name(bind_name.into(), meta)
        }
        BindingTarget::ClassNamespace => op_stmt(
            DelItem::new(
                Box::new(class_namespace_expr(meta.node_index.clone(), meta.range)),
                Box::new(core_string_expr(
                    bind_name,
                    meta.node_index.clone(),
                    meta.range,
                )),
            )
            .with_meta(meta),
        ),
    }
}

fn wrap_deleted_name_load_expr(
    logical_name: String,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    value: InstrUnresolved,
) -> InstrUnresolved {
    core_runtime_positional_call_expr_with_meta(
        "load_deleted_name",
        node_index.clone(),
        range,
        vec![
            core_string_expr(logical_name, node_index.clone(), range),
            value,
        ],
    )
}

fn with_helper_arg_mut<N: BlockPyNameLike + Clone>(
    expr: &mut InstrLow<N>,
    index: usize,
    f: &mut impl FnMut(&mut InstrLow<N>),
) -> bool {
    match expr {
        InstrLow::BinOp(operation) => with_helper_arg_mut_in_operation(operation, index, f),
        InstrLow::UnaryOp(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        InstrLow::Call(operation) => with_helper_arg_mut_in_operation(operation, index, f),
        InstrLow::GetAttr(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        InstrLow::SetAttr(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        InstrLow::GetItem(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        InstrLow::SetItem(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        InstrLow::DelItem(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        InstrLow::Load(operation) => with_helper_arg_mut_in_operation(operation, index, f),
        InstrLow::Store(operation) => with_helper_arg_mut_in_operation(operation, index, f),
        InstrLow::Del(operation) => with_helper_arg_mut_in_operation(operation, index, f),
        InstrLow::MakeCell(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        InstrLow::CellRefForName(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        InstrLow::CellRef(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        InstrLow::MakeFunction(operation) => {
            with_helper_arg_mut_in_operation(operation, index, f)
        }
        _ => false,
    }
}

fn with_helper_arg_mut_in_operation<N: BlockPyNameLike + Clone, T>(
    operation: &mut T,
    index: usize,
    f: &mut impl FnMut(&mut InstrLow<N>),
) -> bool
where
    T: crate::block_py::Mappable<InstrLow<N>, Mapped<InstrLow<N>> = T>
        + crate::block_py::ChildVisitable<InstrLow<N>>,
{
    let current = 0;
    let applied = false;
    struct IndexedArgMutVisitor<'a, N: BlockPyNameLike, F> {
        current: usize,
        index: usize,
        applied: bool,
        f: &'a mut F,
        _marker: std::marker::PhantomData<fn(InstrLow<N>)>,
    }

    impl<N, F> crate::block_py::VisitMut<InstrLow<N>> for IndexedArgMutVisitor<'_, N, F>
    where
        N: BlockPyNameLike + Clone,
        F: FnMut(&mut InstrLow<N>),
    {
        fn visit_instr_mut(&mut self, expr: &mut InstrLow<N>) {
            if self.current == self.index && !self.applied {
                (self.f)(expr);
                self.applied = true;
            }
            self.current += 1;
        }
    }

    let mut visitor = IndexedArgMutVisitor {
        current,
        index,
        applied,
        f,
        _marker: std::marker::PhantomData,
    };
    operation.visit_children_mut(&mut visitor);
    visitor.applied
}

fn rewrite_deleted_name_loads_in_expr(
    expr: &mut InstrUnresolved,
    scope: &CallableScopeInfo,
    storage_layout: &StorageLayout,
    resolver: &NameBindingMapper<'_>,
    deleted_names: &HashSet<String>,
    always_unbound_names: &HashSet<String>,
) {
    if let Some(logical_name) = cell_load_logical_name(expr, scope, storage_layout) {
        if deleted_names.contains(logical_name.as_str())
            || always_unbound_names.contains(logical_name.as_str())
        {
            let meta = expr.meta();
            *expr = core_runtime_positional_call_expr_with_meta(
                "load_deleted_name",
                meta.node_index.clone(),
                meta.range,
                vec![
                    core_string_expr(logical_name, meta.node_index.clone(), meta.range),
                    expr.clone(),
                ],
            );
            return;
        }
    }
    match expr {
        InstrUnresolved::Load(op) => {
            let meta = op.meta();
            let always_unbound = always_unbound_names.contains(op.name.id_str());
            let deleted = deleted_names.contains(op.name.id_str());
            if always_unbound || deleted {
                *expr = wrap_deleted_name_load_expr(
                    op.name.id_str().to_string(),
                    meta.node_index.clone(),
                    meta.range,
                    if always_unbound {
                        deleted_sentinel_expr(meta.node_index, meta.range)
                    } else {
                        expr.clone()
                    },
                );
                return;
            }
            if let UnresolvedName::SourceName(_) = &op.name {
                if let Some(location) = resolver
                    .local_slots
                    .get(op.name.id_str())
                    .copied()
                    .map(LocalLocation)
                {
                    if let Some(logical_name) =
                        logical_name_for_local_location(storage_layout, location)
                    {
                        let always_unbound = always_unbound_names.contains(logical_name.as_str());
                        let deleted = deleted_names.contains(logical_name.as_str());
                        if always_unbound || deleted {
                            *expr = wrap_deleted_name_load_expr(
                                logical_name,
                                meta.node_index.clone(),
                                meta.range,
                                if always_unbound {
                                    deleted_sentinel_expr(meta.node_index, meta.range)
                                } else {
                                    expr.clone()
                                },
                            );
                        }
                    }
                }
            }
        }
        InstrUnresolved::BinOp(_)
        | InstrUnresolved::UnaryOp(_)
        | InstrUnresolved::Call(_)
        | InstrUnresolved::GetAttr(_)
        | InstrUnresolved::SetAttr(_)
        | InstrUnresolved::GetItem(_)
        | InstrUnresolved::SetItem(_)
        | InstrUnresolved::DelItem(_)
        | InstrUnresolved::MakeCell(_)
        | InstrUnresolved::MakeFunction(_) => {
            struct RewriteVisitor<'a> {
                scope: &'a CallableScopeInfo,
                storage_layout: &'a StorageLayout,
                resolver: &'a NameBindingMapper<'a>,
                deleted_names: &'a HashSet<String>,
                always_unbound_names: &'a HashSet<String>,
            }

            impl crate::block_py::VisitMut<InstrUnresolved> for RewriteVisitor<'_> {
                fn visit_instr_mut(&mut self, expr: &mut InstrUnresolved) {
                    rewrite_deleted_name_loads_in_expr(
                        expr,
                        self.scope,
                        self.storage_layout,
                        self.resolver,
                        self.deleted_names,
                        self.always_unbound_names,
                    );
                }
            }

            expr.visit_children_mut(&mut RewriteVisitor {
                scope,
                storage_layout,
                resolver,
                deleted_names,
                always_unbound_names,
            });
        }
        InstrUnresolved::Store(_) => {
            with_helper_arg_mut(expr, 1, &mut |value_expr| {
                rewrite_deleted_name_loads_in_expr(
                    value_expr,
                    scope,
                    storage_layout,
                    resolver,
                    deleted_names,
                    always_unbound_names,
                );
            });
        }
        InstrUnresolved::Del(_)
        | InstrUnresolved::CellRefForName(_)
        | InstrUnresolved::CellRef(_) => {}
        InstrUnresolved::Literal(_) => {}
    }
}

fn core_name_expr(
    id: &str,
    ctx: ast::ExprContext,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrUnresolved {
    assert!(
        matches!(ctx, ast::ExprContext::Load),
        "core_name_expr should only produce load expressions"
    );
    if matches!(ctx, ast::ExprContext::Load)
        && matches!(
            id,
            "DELETED"
                | "NONE"
                | "TRUE"
                | "FALSE"
                | "ELLIPSIS"
                | "globals"
                | "load_deleted_name"
                | "class_lookup_global"
                | "class_lookup_cell"
                | "tuple"
                | "make_function"
        )
    {
        return Load::new(runtime_symbol(id))
            .with_meta(crate::block_py::Meta::new(node_index, range))
            .into();
    }
    let meta = crate::block_py::Meta::new(node_index.clone(), range);
    let _ = ctx;
    Load::new(ast::name::Name::new(id)).with_meta(meta).into()
}

fn compat_node_index() -> ast::AtomicNodeIndex {
    ast::AtomicNodeIndex::default()
}

fn compat_range() -> ruff_text_size::TextRange {
    ruff_text_size::TextRange::default()
}

fn class_namespace_expr(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrUnresolved {
    core_name_expr("_dp_class_ns", ast::ExprContext::Load, node_index, range)
}

fn deleted_sentinel_expr(
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> InstrUnresolved {
    core_name_expr("DELETED", ast::ExprContext::Load, node_index, range)
}

fn rewrite_class_name_load_global(
    name: ast::name::Name,
    meta: crate::block_py::Meta,
) -> InstrUnresolved {
    let bind_name = name.to_string();
    core_runtime_positional_call_expr_with_meta(
        "class_lookup_global",
        meta.node_index.clone(),
        meta.range,
        vec![
            class_namespace_expr(meta.node_index.clone(), meta.range),
            core_string_expr(bind_name, meta.node_index.clone(), meta.range),
            globals_expr(meta.node_index, meta.range),
        ],
    )
}

fn rewrite_class_name_load_cell(
    name: ast::name::Name,
    meta: crate::block_py::Meta,
    scope: &CallableScopeInfo,
) -> InstrUnresolved {
    let bind_name = name.to_string();
    core_runtime_positional_call_expr_with_meta(
        "class_lookup_cell",
        meta.node_index.clone(),
        meta.range,
        vec![
            class_namespace_expr(meta.node_index.clone(), meta.range),
            core_string_expr(bind_name, meta.node_index.clone(), meta.range),
            cell_expr_for_name(name.as_str(), scope, meta.node_index, meta.range),
        ],
    )
}

fn rewrite_quiet_delete_marker(
    name: ast::name::Name,
    meta: crate::block_py::Meta,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) -> CoreStmt {
    match scope.binding_kind(name.as_str()) {
        Some(BindingKind::Cell(_)) => {
            let _ = resolver;
            op_stmt(Del::new(name, true).with_meta(meta))
        }
        _ => match scope.binding_target_for_name(name.as_str(), BindingPurpose::Store) {
            BindingTarget::Local => op_stmt(
                Store::new(
                    name,
                    Box::new(deleted_sentinel_expr(meta.node_index.clone(), meta.range)),
                )
                .with_meta(meta),
            ),
            BindingTarget::ModuleGlobal => op_stmt(Del::new(name, true).with_meta(meta)),
            BindingTarget::ClassNamespace => op_stmt(
                DelItem::new(
                    Box::new(class_namespace_expr(meta.node_index.clone(), meta.range)),
                    Box::new(core_string_expr(
                        name.to_string(),
                        meta.node_index.clone(),
                        meta.range,
                    )),
                )
                .with_meta(meta),
            ),
        },
    }
}

fn quiet_delete_marker_target(expr: &InstrUnresolved) -> Option<ast::name::Name> {
    let InstrUnresolved::Call(call) = expr else {
        return None;
    };
    let Call {
        func,
        args,
        keywords,
        ..
    } = call;
    if !keywords.is_empty() || args.len() != 1 {
        return None;
    }
    if raw_load_name(func.as_ref()).as_deref() != Some("del_quietly") {
        return None;
    }
    match &args[0] {
        CallArgPositional::Positional(expr) => {
            let InstrUnresolved::Call(nested_call) = expr else {
                return raw_load_name(expr).map(ast::name::Name::new);
            };
            if !nested_call.keywords.is_empty()
                || nested_call.args.len() != 2
                || !raw_load_name(nested_call.func.as_ref())
                    .as_ref()
                    .is_some_and(|name| name == "load_deleted_name")
            {
                return raw_load_name(expr).map(ast::name::Name::new);
            }
            match &nested_call.args[1] {
                CallArgPositional::Positional(expr) => {
                    raw_load_name(expr).map(ast::name::Name::new)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn is_deleted_sentinel_expr(expr: &InstrUnresolved) -> bool {
    matches!(expr, InstrUnresolved::Load(op) if op.name.is_runtime_symbol("DELETED"))
}

fn cell_ref_marker_target(expr: &InstrUnresolved) -> Option<String> {
    let InstrUnresolved::CellRefForName(CellRefForName { logical_name, .. }) = expr else {
        return None;
    };
    Some(logical_name.clone())
}

fn make_function_kind_name(kind: FunctionKind) -> &'static str {
    match kind {
        FunctionKind::Function => "function",
        FunctionKind::Coroutine => "coroutine",
        FunctionKind::Generator => "generator",
        FunctionKind::AsyncGenerator => "async_generator",
    }
}

fn cell_load_logical_name(
    expr: &InstrUnresolved,
    scope: &CallableScopeInfo,
    _storage_layout: &StorageLayout,
) -> Option<String> {
    let InstrUnresolved::Load(op) = expr else {
        return None;
    };
    logical_name_for_cell_bound_name(scope, &op.name)
}

fn build_local_cell_init_assign(
    storage_name: &str,
    logical_name: &str,
    is_parameter: bool,
) -> CoreStmt {
    let node_index = compat_node_index();
    let range = compat_range();
    let init_expr = if is_parameter {
        core_name_expr(
            logical_name,
            ast::ExprContext::Load,
            node_index.clone(),
            range,
        )
    } else {
        deleted_sentinel_expr(node_index.clone(), range)
    };
    op_stmt(
        Store::new(
            ast::name::Name::new(storage_name),
            Box::new(op_expr(MakeCell::new(Box::new(init_expr)).with_meta(
                crate::block_py::Meta::new(node_index.clone(), range),
            ))),
        )
        .with_meta(crate::block_py::Meta::new(node_index, range)),
    )
}

fn closure_slot_init_expr(slot: &ClosureSlot) -> InstrUnresolved {
    let node_index = compat_node_index();
    let range = compat_range();
    match slot.init {
        ClosureInit::InheritedCapture => {
            panic!("inherited captures do not allocate new cells in outer callables")
        }
        ClosureInit::Parameter => core_name_expr(
            slot.logical_name.as_str(),
            ast::ExprContext::Load,
            node_index,
            range,
        ),
        ClosureInit::DeletedSentinel => deleted_sentinel_expr(node_index, range),
        ClosureInit::RuntimePcUnstarted => literal_expr(
            CoreNumberLiteral {
                value: CoreNumberLiteralValue::Int(ast::Int::ONE),
            },
            crate::block_py::Meta::new(node_index, range),
        ),
        ClosureInit::RuntimeAbruptKindFallthrough => literal_expr(
            CoreNumberLiteral {
                value: CoreNumberLiteralValue::Int(
                    ast::Int::from_str_radix("0", 10, "0")
                        .expect("zero should parse as an integer literal"),
                ),
            },
            crate::block_py::Meta::new(node_index, range),
        ),
        ClosureInit::RuntimeNone | ClosureInit::Deferred => {
            core_name_expr("NONE", ast::ExprContext::Load, node_index, range)
        }
    }
}

fn build_closure_slot_cell_init_assign(slot: &ClosureSlot) -> CoreStmt {
    let node_index = compat_node_index();
    let range = compat_range();
    op_stmt(
        Store::new(
            ast::name::Name::new(slot.storage_name.as_str()),
            Box::new(op_expr(
                MakeCell::new(Box::new(closure_slot_init_expr(slot)))
                    .with_meta(crate::block_py::Meta::new(node_index.clone(), range)),
            )),
        )
        .with_meta(crate::block_py::Meta::new(node_index, range)),
    )
}

fn prepend_owned_cell_init_preamble(callable: &mut BlockPyFunction<CoreBlockPyPass>) {
    let init_stmts = match callable.kind {
        FunctionKind::Function => {
            let mut storage_names = callable
                .scope
                .owned_cell_storage_names()
                .into_iter()
                .collect::<Vec<_>>();
            if storage_names.is_empty() {
                return;
            }
            storage_names.sort();
            let param_names = callable.params.names().into_iter().collect::<HashSet<_>>();
            storage_names
                .into_iter()
                .map(|storage_name| {
                    let logical_name = callable
                        .scope
                        .logical_name_for_cell_storage(storage_name.as_str())
                        .unwrap_or_else(|| storage_name.clone());
                    build_local_cell_init_assign(
                        storage_name.as_str(),
                        logical_name.as_str(),
                        param_names.contains(logical_name.as_str()),
                    )
                })
                .collect::<Vec<_>>()
        }
        FunctionKind::Generator | FunctionKind::Coroutine | FunctionKind::AsyncGenerator => {
            let layout = callable
                .storage_layout
                .as_ref()
                .expect("generator-like visible function should have closure layout");
            layout
                .cellvars
                .iter()
                .chain(layout.runtime_cells.iter())
                .map(build_closure_slot_cell_init_assign)
                .collect::<Vec<_>>()
        }
    };
    callable
        .blocks
        .first_mut()
        .expect("BlockPyFunction should have at least one block")
        .body
        .splice(0..0, init_stmts.into_iter().map(Into::into));
}

fn logical_name_for_local_location(
    layout: &StorageLayout,
    location: LocalLocation,
) -> Option<String> {
    layout.stack_slots().get(location.slot() as usize).cloned()
}

fn logical_name_for_cell_bound_name(
    scope: &CallableScopeInfo,
    name: &UnresolvedName,
) -> Option<String> {
    let name = name.id_str();
    if scope.is_cell_binding(name) {
        return Some(name.to_string());
    }
    let storage_name = resolve_cell_storage_name(scope, name)?;
    scope.logical_name_for_cell_storage(storage_name.as_str())
}

fn store_cell_deleted_logical_name(
    expr: &InstrUnresolved,
    scope: &CallableScopeInfo,
    _storage_layout: &StorageLayout,
) -> Option<String> {
    let InstrUnresolved::Store(op) = expr else {
        return None;
    };
    if !is_deleted_sentinel_expr(&op.value) {
        return None;
    }
    logical_name_for_cell_bound_name(scope, &op.name)
}

fn del_deref_logical_name(
    expr: &InstrUnresolved,
    scope: &CallableScopeInfo,
    _storage_layout: &StorageLayout,
) -> Option<String> {
    let InstrUnresolved::Del(op) = expr else {
        return None;
    };
    if op.quietly {
        return None;
    }
    logical_name_for_cell_bound_name(scope, &op.name)
}

fn store_cell_runtime_logical_name(
    expr: &InstrUnresolved,
    scope: &CallableScopeInfo,
    _storage_layout: &StorageLayout,
) -> Option<String> {
    let InstrUnresolved::Store(op) = expr else {
        return None;
    };
    if is_deleted_sentinel_expr(&op.value) {
        return None;
    }
    logical_name_for_cell_bound_name(scope, &op.name)
}

struct NameBindingMapper<'a> {
    scope: &'a CallableScopeInfo,
    callee_make_function_captures:
        &'a HashMap<crate::block_py::FunctionId, Vec<CellCaptureBinding>>,
    local_slots: HashMap<String, u32>,
}

impl NameBindingMapper<'_> {
    fn materialize_make_function_expr(
        &mut self,
        meta: crate::block_py::Meta,
        op: MakeFunction<InstrUnresolved>,
    ) -> InstrUnresolved {
        let captures = self
            .callee_make_function_captures
            .get(&op.function_id)
            .into_iter()
            .flat_map(|captures| captures.iter())
            .map(|capture| {
                core_runtime_positional_call_expr_with_meta(
                    "tuple_values",
                    meta.node_index.clone(),
                    meta.range,
                    vec![
                        core_string_expr(
                            capture.logical_name.clone(),
                            meta.node_index.clone(),
                            meta.range,
                        ),
                        rewrite_cell_ref_expr(
                            capture.source_name.as_str(),
                            self.scope,
                            meta.node_index.clone(),
                            meta.range,
                        ),
                    ],
                )
            })
            .collect::<Vec<_>>();
        let captures_expr = core_runtime_positional_call_expr_with_meta(
            "tuple_values",
            meta.node_index.clone(),
            meta.range,
            captures,
        );
        core_runtime_positional_call_expr_with_meta(
            "make_function",
            meta.node_index.clone(),
            meta.range,
            vec![
                core_int_expr(op.function_id.packed(), meta.node_index.clone(), meta.range),
                core_string_expr(
                    make_function_kind_name(op.kind).to_string(),
                    meta.node_index.clone(),
                    meta.range,
                ),
                captures_expr,
                self.map_instr(*op.param_defaults),
                self.map_instr(*op.annotate_fn),
            ],
        )
    }
}

fn rewrite_binding_assign_by_name(
    name: String,
    value: InstrUnresolved,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
    node_index: ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> CoreStmt {
    let meta = crate::block_py::Meta::new(node_index.clone(), range);
    let target: UnresolvedName = ast::name::Name::new(name.clone()).into();
    if scope.is_cell_binding(name.as_str()) {
        if is_deleted_sentinel_expr(&value) {
            let _ = resolver;
            return op_stmt(Del::new(target.clone(), false).with_meta(meta));
        }
        return rewrite_cell_binding_assign(target, value, meta, scope, resolver);
    }
    match scope.binding_target_for_name(name.as_str(), BindingPurpose::Store) {
        BindingTarget::ModuleGlobal => {
            if is_deleted_sentinel_expr(&value) {
                return rewrite_global_binding_delete_by_name(ast::name::Name::new(name), meta);
            }
            rewrite_global_binding_assign(target, value, meta)
        }
        BindingTarget::ClassNamespace => {
            if is_deleted_sentinel_expr(&value) {
                return op_stmt(
                    DelItem::new(
                        Box::new(class_namespace_expr(node_index.clone(), range)),
                        Box::new(core_string_expr(name, node_index, range)),
                    )
                    .with_meta(meta),
                );
            }
            rewrite_class_namespace_binding_assign(target, value, meta)
        }
        BindingTarget::Local => op_stmt(Store::new(target, Box::new(value)).with_meta(meta)),
    }
}

impl MapInstr<InstrUnresolved, InstrUnresolved> for NameBindingMapper<'_> {
    fn map_instr(&mut self, expr: InstrUnresolved) -> InstrUnresolved {
        if let Some(name) = quiet_delete_marker_target(&expr) {
            return rewrite_quiet_delete_marker(name, expr.meta(), self.scope, self);
        }
        if let Some((name, value, node_index, range)) = unresolved_semantic_store_parts(&expr) {
            return rewrite_binding_assign_by_name(
                name,
                self.map_instr(value),
                self.scope,
                self,
                node_index,
                range,
            );
        }
        if let Some((target, meta)) = unresolved_semantic_delete_target(&expr) {
            return rewrite_binding_delete(target, meta, self.scope, self);
        }
        if let Some(target_name) = cell_ref_marker_target(&expr) {
            let meta = expr.meta();
            return rewrite_cell_ref_expr(
                target_name.as_str(),
                self.scope,
                meta.node_index,
                meta.range,
            );
        }
        match_default!(expr: crate::passes::InstrLow<UnresolvedName> {
            InstrUnresolved::Load(op) => {
                let meta = op.meta();
                if op.name.is_runtime_name() {
                    Load::new(op.name).with_meta(meta).into()
                } else if let UnresolvedName::SourceName(name) = op.name {
                    if resolve_cell_storage_name(self.scope, name.as_str()).is_some() {
                        rewrite_raw_cell_storage_name_load(
                            name.clone(),
                            meta.clone(),
                            self.scope,
                            self,
                        )
                        .expect("raw cell-storage load guard should ensure rewrite target")
                    } else if should_rewrite_raw_name_load(name.as_str(), self.scope) {
                        rewrite_name_load(name, meta, self.scope, self)
                    } else {
                        Load::new(name).with_meta(meta).into()
                    }
                } else {
                    rewrite_name_load(op.name.name(), meta, self.scope, self)
                }
            },
            InstrUnresolved::Literal(literal) => InstrUnresolved::Literal(literal),
            InstrUnresolved::MakeFunction(op) => self.materialize_make_function_expr(op.meta(), op),
            InstrUnresolved::Call(call)
                if call.args.is_empty()
                    && call.keywords.is_empty()
                    && raw_load_name(call.func.as_ref())
                        .as_ref()
                        .is_some_and(|name| {
                            name == "globals"
                                && self.scope.resolved_load_binding_kind("globals")
                                    == BindingKind::Global
                        }) =>
            {
                let meta = call.meta();
                globals_expr(meta.node_index, meta.range)
            },
            InstrUnresolved::Call(call)
                if call.keywords.is_empty()
                    && call.args.len() == 3
                    && raw_load_name(call.func.as_ref())
                        .as_ref()
                        .is_some_and(|name| name == "class_lookup_cell") =>
            {
                let meta = call.meta();
                let mut mapped_args = Vec::with_capacity(3);
                for (index, arg) in call.args.into_iter().enumerate() {
                    match (index, arg) {
                        (2, arg) => mapped_args.push(arg),
                        (_, CallArgPositional::Positional(expr)) => {
                            mapped_args.push(CallArgPositional::Positional(self.map_instr(expr)))
                        }
                        (_, CallArgPositional::Starred(expr)) => {
                            mapped_args.push(CallArgPositional::Starred(self.map_instr(expr)))
                        }
                    }
                }
                Call::new(self.map_instr(*call.func), mapped_args, call.keywords)
                    .with_meta(meta)
                    .into()
            },
            InstrUnresolved::Call(call) => call.map_same_children(self).into(),
            rest => rest.map_children(self).into(),
        })
    }

    fn map_name(&mut self, name: UnresolvedName) -> UnresolvedName {
        name
    }
}

fn unresolved_semantic_store_parts(
    expr: &InstrUnresolved,
) -> Option<(
    String,
    InstrUnresolved,
    ast::AtomicNodeIndex,
    ruff_text_size::TextRange,
)> {
    let InstrUnresolved::Store(op) = expr else {
        return None;
    };
    if op.name.is_runtime_name() || is_internal_symbol(op.name.id_str()) {
        return None;
    }
    let meta = op.meta();
    Some((
        op.name.id_str().to_string(),
        op.value.as_ref().clone(),
        meta.node_index,
        meta.range,
    ))
}

fn unresolved_semantic_delete_target(
    expr: &InstrUnresolved,
) -> Option<(ast::name::Name, crate::block_py::Meta)> {
    let InstrUnresolved::Del(op) = expr else {
        return None;
    };
    if op.quietly || op.name.is_runtime_name() || is_internal_symbol(op.name.id_str()) {
        return None;
    }
    Some((op.name.clone().name(), op.meta()))
}

fn collect_deleted_names_in_stmt(
    stmt: &CoreStmt,
    scope: &CallableScopeInfo,
    storage_layout: &StorageLayout,
    names: &mut HashSet<String>,
) {
    if let Some((name, value, _, _)) = unresolved_semantic_store_parts(stmt) {
        if scope.has_local_def(name.as_str()) && is_deleted_sentinel_expr(&value) {
            names.insert(name);
        }
    }
    if let Some((target, _meta)) = unresolved_semantic_delete_target(stmt) {
        if scope.has_local_def(target.as_str()) {
            names.insert(target.to_string());
        }
    }
    if let Some(name) = store_cell_deleted_logical_name(stmt, scope, storage_layout) {
        names.insert(name);
    }
    if let Some(name) = del_deref_logical_name(stmt, scope, storage_layout) {
        names.insert(name);
    }
}

fn rewrite_deleted_name_loads_in_stmt(
    stmt: &mut CoreStmt,
    scope: &CallableScopeInfo,
    storage_layout: &StorageLayout,
    resolver: &NameBindingMapper<'_>,
    deleted_names: &HashSet<String>,
    always_unbound_names: &HashSet<String>,
) {
    rewrite_deleted_name_loads_in_expr(
        stmt,
        scope,
        storage_layout,
        resolver,
        deleted_names,
        always_unbound_names,
    )
}

fn rewrite_deleted_name_loads_in_term(
    term: &mut BlockTerm<InstrUnresolved>,
    scope: &CallableScopeInfo,
    storage_layout: &StorageLayout,
    resolver: &NameBindingMapper<'_>,
    deleted_names: &HashSet<String>,
    always_unbound_names: &HashSet<String>,
) {
    struct RewriteTermVisitor<'a> {
        scope: &'a CallableScopeInfo,
        storage_layout: &'a StorageLayout,
        resolver: &'a NameBindingMapper<'a>,
        deleted_names: &'a HashSet<String>,
        always_unbound_names: &'a HashSet<String>,
    }

    impl crate::block_py::VisitMut<InstrUnresolved> for RewriteTermVisitor<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrUnresolved) {
            rewrite_deleted_name_loads_in_expr(
                expr,
                self.scope,
                self.storage_layout,
                self.resolver,
                self.deleted_names,
                self.always_unbound_names,
            );
        }
    }

    crate::block_py::walk_term_mut(
        &mut RewriteTermVisitor {
            scope,
            storage_layout,
            resolver,
            deleted_names,
            always_unbound_names,
        },
        term,
    );
}

fn rewrite_raw_cell_loads_in_expr(
    expr: &mut InstrUnresolved,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) {
    match expr {
        InstrUnresolved::Call(call) => {
            if call.keywords.is_empty()
                && call.args.len() == 3
                && raw_load_name(call.func.as_ref())
                    .as_ref()
                    .is_some_and(|name| name == "class_lookup_cell")
            {
                rewrite_raw_cell_loads_in_expr(call.func.as_mut(), scope, resolver);
                if let Some(arg) = call.args.get_mut(0) {
                    rewrite_raw_cell_loads_in_expr(arg.expr_mut(), scope, resolver);
                }
                if let Some(arg) = call.args.get_mut(1) {
                    rewrite_raw_cell_loads_in_expr(arg.expr_mut(), scope, resolver);
                }
                return;
            }
            struct RewriteVisitor<'a> {
                scope: &'a CallableScopeInfo,
                resolver: &'a NameBindingMapper<'a>,
            }

            impl crate::block_py::VisitMut<InstrUnresolved> for RewriteVisitor<'_> {
                fn visit_instr_mut(&mut self, expr: &mut InstrUnresolved) {
                    rewrite_raw_cell_loads_in_expr(expr, self.scope, self.resolver);
                }
            }

            call.visit_children_mut(&mut RewriteVisitor { scope, resolver });
        }
        InstrUnresolved::BinOp(_)
        | InstrUnresolved::UnaryOp(_)
        | InstrUnresolved::GetAttr(_)
        | InstrUnresolved::SetAttr(_)
        | InstrUnresolved::GetItem(_)
        | InstrUnresolved::SetItem(_)
        | InstrUnresolved::DelItem(_)
        | InstrUnresolved::Load(_)
        | InstrUnresolved::Store(_)
        | InstrUnresolved::Del(_)
        | InstrUnresolved::MakeCell(_)
        | InstrUnresolved::CellRefForName(_)
        | InstrUnresolved::CellRef(_)
        | InstrUnresolved::MakeFunction(_) => {
            if let InstrUnresolved::Load(op) = expr {
                if let UnresolvedName::SourceName(name) = &op.name {
                    if matches!(
                        scope.binding_kind(name.as_str()),
                        Some(BindingKind::Cell(_))
                    ) {
                        *expr = rewrite_cell_name_load(name.clone(), op.meta(), scope, resolver);
                        return;
                    }
                }
            }
            struct RewriteVisitor<'a> {
                scope: &'a CallableScopeInfo,
                resolver: &'a NameBindingMapper<'a>,
            }

            impl crate::block_py::VisitMut<InstrUnresolved> for RewriteVisitor<'_> {
                fn visit_instr_mut(&mut self, expr: &mut InstrUnresolved) {
                    rewrite_raw_cell_loads_in_expr(expr, self.scope, self.resolver);
                }
            }

            expr.visit_children_mut(&mut RewriteVisitor { scope, resolver });
        }
        InstrUnresolved::Literal(_) => {}
    }
}

fn is_local_cell_init_store(expr: &InstrUnresolved) -> bool {
    let InstrUnresolved::Store(Store { name, value, .. }) = expr else {
        return false;
    };
    name.id_str().starts_with("_dp_cell_") && matches!(value.as_ref(), InstrUnresolved::MakeCell(_))
}

fn rewrite_raw_cell_loads_in_stmt(
    stmt: &mut CoreStmt,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) {
    if is_local_cell_init_store(stmt) {
        return;
    }
    rewrite_raw_cell_loads_in_expr(stmt, scope, resolver)
}

fn rewrite_raw_cell_loads_in_term(
    term: &mut BlockTerm<InstrUnresolved>,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) {
    struct RewriteTermVisitor<'a> {
        scope: &'a CallableScopeInfo,
        resolver: &'a NameBindingMapper<'a>,
    }

    impl crate::block_py::VisitMut<InstrUnresolved> for RewriteTermVisitor<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrUnresolved) {
            rewrite_raw_cell_loads_in_expr(expr, self.scope, self.resolver);
        }
    }

    crate::block_py::walk_term_mut(&mut RewriteTermVisitor { scope, resolver }, term);
}

fn normal_successor_labels(term: &BlockTerm<InstrUnresolved>) -> Vec<&crate::block_py::BlockLabel> {
    match term {
        BlockTerm::Jump(edge) => vec![&edge.target],
        BlockTerm::IfTerm(if_term) => vec![&if_term.then_label, &if_term.else_label],
        BlockTerm::BranchTable(branch) => {
            let mut targets = branch.targets.iter().collect::<Vec<_>>();
            targets.push(&branch.default_label);
            targets
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => Vec::new(),
    }
}

fn normal_predecessor_exc_param_names(
    blocks: &[crate::block_py::Block<InstrUnresolved, InstrUnresolved>],
) -> HashMap<crate::block_py::BlockLabel, Vec<Option<String>>> {
    let mut predecessors = HashMap::new();
    for block in blocks {
        let exc_name = block.exception_param().map(ToString::to_string);
        for target in normal_successor_labels(&block.term) {
            predecessors
                .entry(target.clone())
                .or_insert_with(Vec::new)
                .push(exc_name.clone());
        }
    }
    predecessors
}

fn sync_exception_param_cell_in_block(
    block: &mut crate::block_py::Block<InstrUnresolved, InstrUnresolved>,
    normal_predecessor_exc_names: &[Option<String>],
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) {
    let Some(exc_name) = block.exception_param() else {
        return;
    };
    if !matches!(scope.binding_kind(exc_name), Some(BindingKind::Cell(_))) {
        return;
    }
    if normal_predecessor_exc_names.iter().any(|pred_exc_name| {
        pred_exc_name
            .as_deref()
            .is_some_and(|pred_exc_name| pred_exc_name != exc_name)
    }) {
        return;
    }

    let node_index = compat_node_index();
    let range = compat_range();
    let exc_load = ast::name::Name::new(exc_name);
    let meta = crate::block_py::Meta::new(node_index.clone(), range);
    let sync_stmt = op_stmt(
        Store::new(
            ast::name::Name::new(exc_name),
            Box::new(rewrite_local_name_load(exc_load, meta.clone(), resolver)),
        )
        .with_meta(crate::block_py::Meta::new(node_index, range)),
    );
    block.body.insert(0, sync_stmt);
}

fn collect_deleted_names_in_blocks(
    blocks: &[crate::block_py::Block<InstrUnresolved, InstrUnresolved>],
    scope: &CallableScopeInfo,
    storage_layout: &StorageLayout,
) -> HashSet<String> {
    let mut names = HashSet::new();
    for block in blocks {
        for stmt in &block.body {
            collect_deleted_names_in_stmt(stmt, scope, storage_layout, &mut names);
        }
    }
    names
}

fn collect_runtime_bound_local_names_in_stmt(
    stmt: &CoreStmt,
    scope: &CallableScopeInfo,
    storage_layout: &StorageLayout,
    names: &mut HashSet<String>,
) {
    if let Some((name, value, _, _)) = unresolved_semantic_store_parts(stmt) {
        if scope.has_local_def(name.as_str()) && !is_deleted_sentinel_expr(&value) {
            names.insert(name);
        }
    }
    if let Some(name) = store_cell_runtime_logical_name(stmt, scope, storage_layout) {
        names.insert(name);
    }
}

fn collect_runtime_bound_local_names(
    blocks: &[crate::block_py::Block<InstrUnresolved, InstrUnresolved>],
    scope: &CallableScopeInfo,
    storage_layout: &StorageLayout,
) -> HashSet<String> {
    let mut names = HashSet::new();
    for block in blocks {
        for stmt in &block.body {
            collect_runtime_bound_local_names_in_stmt(stmt, scope, storage_layout, &mut names);
        }
    }
    names
}

fn collect_always_unbound_local_names(
    callable: &BlockPyFunction<CoreBlockPyPass>,
) -> HashSet<String> {
    let scope = &callable.scope;
    let storage_layout = callable
        .storage_layout
        .as_ref()
        .expect("name binding should have storage layout before local-name analysis");
    let param_names = callable.params.names().into_iter().collect::<HashSet<_>>();
    let runtime_bound_names =
        collect_runtime_bound_local_names(&callable.blocks, scope, storage_layout);
    scope
        .local_defs
        .iter()
        .filter(|name| !param_names.contains(*name))
        .filter(|name| !is_internal_symbol(name.as_str()))
        .filter(|name| !runtime_bound_names.contains(*name))
        .filter(|name| {
            matches!(
                scope.effective_binding(name.as_str(), BindingPurpose::Load),
                Some(EffectiveBinding::Local | EffectiveBinding::Cell(_))
            )
        })
        .cloned()
        .collect()
}

fn collect_remaining_names_in_expr(expr: &InstrUnresolved, names: &mut HashSet<String>) {
    match expr {
        InstrUnresolved::Load(op) => {
            names.insert(op.name.id_str().to_string());
        }
        InstrUnresolved::Store(op) => {
            names.insert(op.name.id_str().to_string());
        }
        InstrUnresolved::Del(op) => {
            names.insert(op.name.id_str().to_string());
        }
        InstrUnresolved::Literal(_)
        | InstrUnresolved::BinOp(_)
        | InstrUnresolved::UnaryOp(_)
        | InstrUnresolved::Call(_)
        | InstrUnresolved::GetAttr(_)
        | InstrUnresolved::SetAttr(_)
        | InstrUnresolved::GetItem(_)
        | InstrUnresolved::SetItem(_)
        | InstrUnresolved::DelItem(_)
        | InstrUnresolved::MakeCell(_)
        | InstrUnresolved::CellRefForName(_)
        | InstrUnresolved::CellRef(_)
        | InstrUnresolved::MakeFunction(_) => {}
    }

    struct RemainingNamesVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl crate::block_py::Visit<InstrUnresolved> for RemainingNamesVisitor<'_> {
        fn visit_instr(&mut self, expr: &InstrUnresolved) {
            collect_remaining_names_in_expr(expr, self.names);
        }
    }

    expr.visit_children(&mut RemainingNamesVisitor { names });
}

fn collect_remaining_names_in_stmt(stmt: &CoreStmt, names: &mut HashSet<String>) {
    collect_remaining_names_in_expr(stmt, names)
}

fn collect_remaining_names_in_term(term: &BlockTerm<InstrUnresolved>, names: &mut HashSet<String>) {
    struct RemainingNamesVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl crate::block_py::Visit<InstrUnresolved> for RemainingNamesVisitor<'_> {
        fn visit_instr(&mut self, expr: &InstrUnresolved) {
            collect_remaining_names_in_expr(expr, self.names);
        }

        fn visit_block_arg(&mut self, arg: &BlockArg) {
            if let BlockArg::Name(name) = arg {
                self.names.insert(name.clone());
            }
        }
    }

    crate::block_py::walk_term(&mut RemainingNamesVisitor { names }, term);
}

fn resolve_cell_storage_name(scope: &CallableScopeInfo, name: &str) -> Option<String> {
    scope
        .logical_name_for_cell_capture_source(name)
        .map(|logical_name| scope.cell_storage_name(logical_name.as_str()))
}

fn resolve_captured_cell_source_storage_name(
    scope: &CallableScopeInfo,
    name: &str,
) -> Option<String> {
    let logical_name = scope.logical_name_for_cell_capture_source(name)?;
    if scope.binding_kind(logical_name.as_str())
        != Some(BindingKind::Cell(CellBindingKind::Capture))
    {
        return None;
    }
    let capture_source_name = scope.cell_capture_source_name(logical_name.as_str());
    let storage_name = scope.cell_storage_name(logical_name.as_str());
    (capture_source_name == name && capture_source_name != storage_name).then_some(storage_name)
}

fn collect_captured_cell_slot_locations(
    callable: &BlockPyFunction<CoreBlockPyPass>,
) -> HashMap<String, u32> {
    let mut slots = HashMap::new();
    if let Some(layout) = callable.storage_layout.as_ref() {
        for (slot, closure_slot) in layout.freevars.iter().enumerate() {
            slots.insert(closure_slot.storage_name.clone(), slot as u32);
            slots.insert(closure_slot.logical_name.clone(), slot as u32);
        }
    }
    slots
}

fn collect_owned_cell_storage_bindings(
    callable: &BlockPyFunction<CoreBlockPyPass>,
) -> Vec<(String, String)> {
    if let Some(layout) = callable
        .storage_layout
        .as_ref()
        .filter(|layout| !layout.cellvars.is_empty() || !layout.runtime_cells.is_empty())
    {
        return layout
            .cellvars
            .iter()
            .chain(layout.runtime_cells.iter())
            .map(|slot| (slot.logical_name.clone(), slot.storage_name.clone()))
            .collect();
    }

    let mut storage_names = callable
        .scope
        .owned_cell_storage_names()
        .into_iter()
        .collect::<Vec<_>>();
    storage_names.sort();
    storage_names
        .into_iter()
        .map(|storage_name| {
            let logical_name = callable
                .scope
                .logical_name_for_cell_storage(storage_name.as_str())
                .unwrap_or_else(|| storage_name.clone());
            (logical_name, storage_name)
        })
        .collect()
}

fn collect_owned_cell_slot_locations(
    callable: &BlockPyFunction<CoreBlockPyPass>,
) -> HashMap<String, u32> {
    let mut slots = HashMap::new();
    for (slot, (logical_name, storage_name)) in collect_owned_cell_storage_bindings(callable)
        .into_iter()
        .enumerate()
    {
        slots.insert(storage_name, slot as u32);
        slots.insert(logical_name, slot as u32);
    }
    slots
}

fn collect_cell_bindings(
    callable: &BlockPyFunction<CoreBlockPyPass>,
) -> HashMap<String, (String, CellBindingKind)> {
    let mut bindings = HashMap::new();
    let Some(layout) = callable.storage_layout.as_ref() else {
        return bindings;
    };

    let mut add_binding = |name: &str, storage_name: &str, binding_kind: CellBindingKind| {
        bindings.insert(name.to_string(), (storage_name.to_string(), binding_kind));
    };

    for slot in &layout.freevars {
        add_binding(
            slot.logical_name.as_str(),
            slot.storage_name.as_str(),
            CellBindingKind::Capture,
        );
        add_binding(
            slot.storage_name.as_str(),
            slot.storage_name.as_str(),
            CellBindingKind::Capture,
        );
        let capture_source_name = callable
            .scope
            .cell_capture_source_name(slot.logical_name.as_str());
        add_binding(
            capture_source_name.as_str(),
            slot.storage_name.as_str(),
            CellBindingKind::Capture,
        );
    }

    for (logical_name, storage_name) in collect_owned_cell_storage_bindings(callable) {
        add_binding(
            logical_name.as_str(),
            storage_name.as_str(),
            CellBindingKind::Owner,
        );
        add_binding(
            storage_name.as_str(),
            storage_name.as_str(),
            CellBindingKind::Owner,
        );
    }

    bindings
}

fn is_remaining_local_name(
    name: &str,
    scope: &CallableScopeInfo,
    has_explicit_store: bool,
) -> bool {
    if resolve_cell_storage_name(scope, name).is_some() {
        return false;
    }
    if has_explicit_store {
        return !matches!(
            scope.binding_kind(name),
            Some(BindingKind::Cell(_)) | Some(BindingKind::Global)
        ) && matches!(
            scope.binding_target_for_name(name, BindingPurpose::Store),
            BindingTarget::Local
        );
    }
    match scope.binding_kind(name) {
        Some(BindingKind::Local) => scope.honors_internal_binding(name),
        Some(BindingKind::Cell(_)) | Some(BindingKind::Global) => false,
        None => scope.has_local_def(name),
    }
}

fn compute_local_slot_locations_from_analysis(
    callable: &BlockPyFunction<CoreBlockPyPass>,
) -> HashMap<String, u32> {
    let mut slots = HashMap::new();
    for (slot, param_name) in callable.params.names().into_iter().enumerate() {
        slots.insert(param_name, slot as u32);
    }
    let mut next_slot = slots.len() as u32;
    let mut owned_cell_storage_names = callable
        .scope
        .owned_cell_storage_names()
        .into_iter()
        .collect::<Vec<_>>();
    owned_cell_storage_names.sort();
    for storage_name in owned_cell_storage_names {
        if slots.contains_key(storage_name.as_str()) {
            continue;
        }
        slots.insert(storage_name, next_slot);
        next_slot += 1;
    }
    for block in &callable.blocks {
        for param_name in block.param_names() {
            if slots.contains_key(param_name) {
                continue;
            }
            slots.insert(param_name.to_string(), next_slot);
            next_slot += 1;
        }
    }

    let mut remaining = HashSet::new();
    let mut explicitly_stored = HashSet::new();
    for block in &callable.blocks {
        for stmt in &block.body {
            collect_remaining_names_in_stmt(stmt, &mut remaining);
            match stmt {
                InstrUnresolved::Store(op) => {
                    explicitly_stored.insert(op.name.id_str().to_string());
                }
                InstrUnresolved::Del(op) => {
                    explicitly_stored.insert(op.name.id_str().to_string());
                }
                _ => {}
            }
        }
        collect_remaining_names_in_term(&block.term, &mut remaining);
    }

    let mut non_param_locals = remaining
        .into_iter()
        .filter(|name| !slots.contains_key(name))
        .filter(|name| {
            is_remaining_local_name(
                name,
                &callable.scope,
                explicitly_stored.contains(name.as_str()),
            )
        })
        .collect::<Vec<_>>();
    non_param_locals.sort();

    for name in non_param_locals {
        slots.insert(name, next_slot);
        next_slot += 1;
    }
    slots
}

fn ordered_slot_names_from_local_slots(local_slots: HashMap<String, u32>) -> Vec<String> {
    let mut slots = local_slots.into_iter().collect::<Vec<_>>();
    slots.sort_by_key(|(_, slot)| *slot);
    slots.into_iter().map(|(name, _)| name).collect()
}

fn collect_local_slot_locations(
    callable: &BlockPyFunction<CoreBlockPyPass>,
) -> HashMap<String, u32> {
    if let Some(layout) = callable
        .storage_layout
        .as_ref()
        .filter(|layout| !layout.stack_slots().is_empty())
    {
        return layout
            .stack_slots()
            .iter()
            .enumerate()
            .map(|(slot, name)| (name.clone(), slot as u32))
            .collect();
    }

    compute_local_slot_locations_from_analysis(callable)
}

fn populate_stack_slots_in_storage_layout<P: crate::block_py::BlockPyPass>(
    callable: &mut BlockPyFunction<P>,
    local_slots: HashMap<String, u32>,
) {
    let stack_slots = ordered_slot_names_from_local_slots(local_slots);
    callable
        .storage_layout
        .get_or_insert_with(StorageLayout::default)
        .set_stack_slots(stack_slots);
}

fn ensure_storage_layout_covers_block_params<P: crate::block_py::BlockPyPass>(
    callable: &mut BlockPyFunction<P>,
) {
    let Some(layout) = callable.storage_layout.as_mut() else {
        return;
    };
    for block in &callable.blocks {
        for name in block.param_names() {
            layout.ensure_stack_slot(name.to_string());
        }
    }
}

struct NameLocator<'a> {
    scope: &'a CallableScopeInfo,
    exception_param_names: HashSet<String>,
    local_slots: HashMap<String, u32>,
    captured_cell_slots: HashMap<String, u32>,
    owned_cell_slots: HashMap<String, u32>,
    cell_bindings: HashMap<String, (String, CellBindingKind)>,
    global_slots: &'a mut ModuleGlobalSlots,
}

#[derive(Default)]
struct ModuleGlobalSlots {
    slot_by_name: HashMap<String, u32>,
    names: Vec<String>,
}

impl ModuleGlobalSlots {
    fn slot_for(&mut self, name: &str) -> u32 {
        if let Some(slot) = self.slot_by_name.get(name).copied() {
            return slot;
        }
        let slot =
            u32::try_from(self.names.len()).expect("module global slot count should fit in u32");
        self.slot_by_name.insert(name.to_string(), slot);
        self.names.push(name.to_string());
        slot
    }

    fn into_names(self) -> Vec<String> {
        self.names
    }
}

impl NameLocator<'_> {
    fn resolve_raw_cell_location(&self, name_text: &str) -> CellLocation {
        if let Some(storage_name) = resolve_captured_cell_source_storage_name(self.scope, name_text)
        {
            let slot = self
                .captured_cell_slots
                .get(storage_name.as_str())
                .copied()
                .unwrap_or_else(|| {
                    panic!(
                        "missing closure slot for captured raw cell source {name_text} via storage name {storage_name}"
                    )
                });
            return CellLocation::CapturedSource(slot);
        }

        if let Some((storage_name, binding_kind)) = self.cell_bindings.get(name_text) {
            return match binding_kind {
                CellBindingKind::Owner => {
                    let slot = self
                        .owned_cell_slots
                        .get(storage_name.as_str())
                        .copied()
                        .unwrap_or_else(|| {
                            panic!(
                                "missing owned cell slot for raw cell target {name_text} via storage name {storage_name}"
                            )
                        });
                    CellLocation::Owned(slot)
                }
                CellBindingKind::Capture => {
                    let slot = self
                        .captured_cell_slots
                        .get(storage_name.as_str())
                        .copied()
                        .unwrap_or_else(|| {
                            panic!(
                                "missing closure slot for raw captured cell target {name_text} via storage name {storage_name}"
                            )
                        });
                    CellLocation::CapturedSource(slot)
                }
            };
        }

        panic!("raw cell target {name_text} did not resolve to a cell-backed location");
    }

    fn resolve_cell_ref_location(&self, logical_name: &str) -> CellLocation {
        if self.cell_bindings.contains_key(logical_name)
            || resolve_captured_cell_source_storage_name(self.scope, logical_name).is_some()
        {
            return self.resolve_raw_cell_location(logical_name);
        }
        let source_name = self.scope.cell_ref_source_name(logical_name);
        self.resolve_raw_cell_location(source_name.as_str())
    }

    fn locate_name(&mut self, name: ast::name::Name) -> ResolvedName {
        let name_text = name.to_string();
        let location = if self.exception_param_names.contains(name_text.as_str()) {
            let slot = self
                .local_slots
                .get(name_text.as_str())
                .copied()
                .unwrap_or_else(|| {
                    panic!("missing local slot for exception param {name_text}");
                });
            NameLocation::local(slot)
        } else if let Some(storage_name) =
            resolve_captured_cell_source_storage_name(self.scope, name_text.as_str())
        {
            let slot = self
                .captured_cell_slots
                .get(storage_name.as_str())
                .copied()
                .unwrap_or_else(|| {
                panic!(
                    "missing closure slot for captured cell source {name_text} via storage name {storage_name}"
                )
            });
            NameLocation::captured_source_cell(slot)
        } else if let Some((storage_name, binding_kind)) =
            self.cell_bindings.get(name_text.as_str()).cloned()
        {
            match binding_kind {
                CellBindingKind::Owner => {
                    if name_text != storage_name {
                        if let Some(slot) = self.local_slots.get(name_text.as_str()).copied() {
                            NameLocation::local(slot)
                        } else {
                            let slot = self
                                .owned_cell_slots
                                .get(storage_name.as_str())
                                .copied()
                                .unwrap_or_else(|| {
                                    panic!(
                                        "missing owned cell slot for storage name {storage_name} while locating {name_text}"
                                    )
                                });
                            NameLocation::owned_cell(slot)
                        }
                    } else {
                        let slot = self
                            .owned_cell_slots
                            .get(storage_name.as_str())
                            .copied()
                            .unwrap_or_else(|| {
                                panic!(
                                    "missing owned cell slot for storage name {storage_name} while locating {name_text}"
                                )
                            });
                        NameLocation::owned_cell(slot)
                    }
                }
                CellBindingKind::Capture => {
                    let slot = self
                        .captured_cell_slots
                        .get(storage_name.as_str())
                        .copied()
                        .unwrap_or_else(|| {
                            panic!(
                                "missing closure slot for storage name {storage_name} while locating {name_text}"
                            )
                        });
                    NameLocation::closure_cell(slot)
                }
            }
        } else if let Some(slot) = self.local_slots.get(name_text.as_str()).copied() {
            NameLocation::local(slot)
        } else {
            NameLocation::global(self.global_slots.slot_for(name_text.as_str()))
        };
        ResolvedName { id: name, location }
    }

    fn locate_unresolved_name(&mut self, name: UnresolvedName) -> ResolvedName {
        match name {
            UnresolvedName::SourceName(name) => self.locate_name(name),
            UnresolvedName::RuntimeName(name) => ResolvedName {
                id: name,
                location: NameLocation::RuntimeName,
            },
        }
    }

    fn mark_raw_cell_store_name(&self, name: ResolvedName) -> ResolvedName {
        let name_text = name.id.to_string();
        if resolve_cell_storage_name(self.scope, name_text.as_str()).is_some() {
            if let Some(slot) = self.local_slots.get(name_text.as_str()).copied() {
                return name.with_location(NameLocation::local(slot));
            }
        }
        self.mark_raw_cell_name(name)
    }

    fn mark_raw_cell_name(&self, name: ResolvedName) -> ResolvedName {
        let name_text = name.id.to_string();
        if name.location.is_global() {
            if resolve_captured_cell_source_storage_name(self.scope, name_text.as_str()).is_some()
                || self.cell_bindings.contains_key(name_text.as_str())
            {
                let location = self.resolve_raw_cell_location(name_text.as_str());
                return name.with_location(NameLocation::Cell(location));
            }
            return name;
        }

        match name.cell_location() {
            Some(location) if location.is_closure() => {
                name.with_location(NameLocation::captured_source_cell(location.slot()))
            }
            _ => name,
        }
    }

fn mark_raw_cell_expr(
    &self,
    expr: InstrLow<ResolvedName>,
) -> InstrLow<ResolvedName> {
    match expr {
            InstrLow::Load(op) => {
                let meta = op.meta();
                let name = op.name;
                let marked = self.mark_raw_cell_name(name);
                if let Some(location) = marked.cell_location() {
                    return CellRef::new(location).with_meta(meta).into();
                }
                Load::new(marked).with_meta(meta).into()
            }
            other => other,
        }
    }
}

impl MapInstr<InstrUnresolved, InstrLow<ResolvedName>> for NameLocator<'_> {
    fn map_instr(&mut self, expr: InstrUnresolved) -> InstrLow<ResolvedName> {
        match_default!(expr: crate::passes::InstrLow<UnresolvedName> {
            InstrLow::Literal(literal) => InstrResolved::Literal(literal),
            InstrLow::Load(op) => {
                let meta = op.meta();
                let name = self.locate_unresolved_name(op.name);
                let name = if name.is_runtime_name() {
                    name
                } else {
                    self.mark_raw_cell_name(name)
                };
                Load::new(name).with_meta(meta).into()
            },
            InstrLow::Store(op) => {
                let meta = op.meta();
                let name = self.locate_unresolved_name(op.name);
                let name = if name.is_runtime_name() {
                    name
                } else {
                    self.mark_raw_cell_store_name(name)
                };
                let value = self.map_instr(*op.value);
                Store::new(name, Box::new(value)).with_meta(meta).into()
            },
            InstrLow::Del(op) => {
                let meta = op.meta();
                let name = self.locate_unresolved_name(op.name);
                let name = if name.is_runtime_name() {
                    name
                } else {
                    self.mark_raw_cell_store_name(name)
                };
                Del::new(name, op.quietly).with_meta(meta).into()
            },
            InstrLow::CellRefForName(op) => {
                let meta = op.meta();
                let location = self.resolve_cell_ref_location(op.logical_name.as_str());
                CellRef::new(location).with_meta(meta).into()
            },
            InstrLow::Call(call) => {
                let meta = call.meta();
                let call = call.map_children(self);
                if raw_load_name(call.func.as_ref())
                    .as_ref()
                    .is_some_and(|name| name == "class_lookup_cell")
                    && call.args.len() == 3
                {
                    if let Some(CallArgPositional::Positional(expr)) = call.args.get(2) {
                        let mut marked = expr.clone();
                        marked = self.mark_raw_cell_expr(marked);
                        let mut call = call;
                        if let Some(CallArgPositional::Positional(target)) = call.args.get_mut(2) {
                            *target = marked;
                        }
                        return call.with_meta(meta).into();
                    }
                }
                call.with_meta(meta).into()
            },
            InstrLow::CellRef(node) => node.into(),
            rest => rest.map_children(self).into(),
        })
    }

    fn map_name(&mut self, name: UnresolvedName) -> ResolvedName {
        self.locate_unresolved_name(name)
    }
}

fn locate_names_in_callable(
    callable: BlockPyFunction<CoreBlockPyPass>,
    global_slots: &mut ModuleGlobalSlots,
) -> BlockPyFunction<ResolvedStorageBlockPyPass> {
    let scope = callable.scope.clone();
    let exception_param_names = callable
        .blocks
        .iter()
        .filter_map(|block| block.exception_param().map(ToString::to_string))
        .collect::<HashSet<_>>();
    let local_slots = collect_local_slot_locations(&callable);
    let captured_cell_slots = collect_captured_cell_slot_locations(&callable);
    let owned_cell_slots = collect_owned_cell_slot_locations(&callable);
    let cell_bindings = collect_cell_bindings(&callable);
    let mut mapper = NameLocator {
        scope: &scope,
        exception_param_names,
        local_slots,
        captured_cell_slots,
        owned_cell_slots,
        cell_bindings,
        global_slots,
    };
    mapper.map_fn(callable)
}

fn collect_make_function_callee_ids_in_expr(expr: &InstrUnresolved, out: &mut Vec<FunctionId>) {
    match expr {
        InstrUnresolved::Literal(_) => {}
        InstrUnresolved::MakeFunction(op) => {
            out.push(op.function_id);
        }
        _ => {
            struct CalleeVisitor<'a> {
                out: &'a mut Vec<FunctionId>,
            }

            impl crate::block_py::Visit<InstrUnresolved> for CalleeVisitor<'_> {
                fn visit_instr(&mut self, expr: &InstrUnresolved) {
                    collect_make_function_callee_ids_in_expr(expr, self.out);
                }
            }

            expr.visit_children(&mut CalleeVisitor { out });
        }
    }
}

fn collect_make_function_callee_ids(
    callable: &BlockPyFunction<CoreBlockPyPass>,
) -> Vec<FunctionId> {
    let mut out = Vec::new();
    for block in &callable.blocks {
        for stmt in &block.body {
            collect_make_function_callee_ids_in_stmt(stmt, &mut out);
        }
        collect_make_function_callee_ids_in_term(&block.term, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

fn collect_make_function_callee_ids_in_stmt(stmt: &CoreStmt, out: &mut Vec<FunctionId>) {
    collect_make_function_callee_ids_in_expr(stmt, out)
}

fn collect_make_function_callee_ids_in_term(
    term: &BlockTerm<InstrUnresolved>,
    out: &mut Vec<FunctionId>,
) {
    struct CalleeVisitor<'a> {
        out: &'a mut Vec<FunctionId>,
    }

    impl crate::block_py::Visit<InstrUnresolved> for CalleeVisitor<'_> {
        fn visit_instr(&mut self, expr: &InstrUnresolved) {
            collect_make_function_callee_ids_in_expr(expr, self.out);
        }
    }

    crate::block_py::walk_term(&mut CalleeVisitor { out }, term);
}

fn compute_callable_storage_layout_for_name_binding(
    function_id: FunctionId,
    callable_by_id: &HashMap<FunctionId, &BlockPyFunction<CoreBlockPyPass>>,
    make_function_callees: &HashMap<FunctionId, Vec<FunctionId>>,
    memo: &mut HashMap<FunctionId, Option<StorageLayout>>,
    visiting: &mut HashSet<FunctionId>,
) -> Option<StorageLayout> {
    if let Some(layout) = memo.get(&function_id) {
        return layout.clone();
    }
    let callable = callable_by_id
        .get(&function_id)
        .unwrap_or_else(|| panic!("missing callable for function id {:?}", function_id));
    if let Some(layout) = callable.storage_layout.clone() {
        memo.insert(function_id, Some(layout.clone()));
        return Some(layout);
    }
    if !visiting.insert(function_id) {
        return compute_storage_layout_from_scope(callable);
    }

    let base_layout = compute_storage_layout_from_scope(callable);
    let mut capture_names = base_layout
        .as_ref()
        .map(|layout| {
            layout
                .freevars
                .iter()
                .map(|slot| slot.logical_name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let base_cellvar_names = base_layout
        .as_ref()
        .map(|layout| {
            layout
                .cellvars
                .iter()
                .map(|slot| slot.logical_name.clone())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let base_cellvar_storage_names = base_layout
        .as_ref()
        .map(|layout| {
            layout
                .cellvars
                .iter()
                .map(|slot| slot.storage_name.clone())
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    if let Some(callee_ids) = make_function_callees.get(&function_id) {
        for callee_id in callee_ids {
            let Some(callee_layout) = compute_callable_storage_layout_for_name_binding(
                *callee_id,
                callable_by_id,
                make_function_callees,
                memo,
                visiting,
            ) else {
                continue;
            };
            for slot in &callee_layout.freevars {
                let capture_source_name = callable
                    .scope
                    .cell_capture_source_name(slot.logical_name.as_str());
                if base_cellvar_names.contains(slot.logical_name.as_str())
                    || base_cellvar_storage_names.contains(capture_source_name.as_str())
                {
                    continue;
                }
                capture_names.push(slot.logical_name.clone());
            }
        }
    }
    visiting.remove(&function_id);

    let param_name_set = callable.params.names().into_iter().collect::<HashSet<_>>();
    let mut local_cell_slots = callable
        .scope
        .owned_cell_storage_names()
        .into_iter()
        .collect::<Vec<_>>();
    local_cell_slots.sort();
    let layout = build_storage_layout_from_capture_names(
        callable,
        capture_names,
        &param_name_set,
        &local_cell_slots,
    );
    memo.insert(function_id, layout.clone());
    layout
}

fn ensure_module_storage_layouts(
    callable_defs: Vec<BlockPyFunction<CoreBlockPyPass>>,
) -> Vec<BlockPyFunction<CoreBlockPyPass>> {
    let computed_layouts = {
        let callable_by_id = callable_defs
            .iter()
            .map(|callable| (callable.function_id, callable))
            .collect::<HashMap<_, _>>();
        let make_function_callees = callable_defs
            .iter()
            .map(|callable| {
                (
                    callable.function_id,
                    collect_make_function_callee_ids(callable),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut memo = HashMap::new();
        let mut visiting = HashSet::new();
        for function_id in callable_by_id.keys().copied().collect::<Vec<_>>() {
            compute_callable_storage_layout_for_name_binding(
                function_id,
                &callable_by_id,
                &make_function_callees,
                &mut memo,
                &mut visiting,
            );
        }
        memo
    };

    callable_defs
        .into_iter()
        .map(|mut callable| {
            if callable.storage_layout.is_none() {
                callable.storage_layout = computed_layouts
                    .get(&callable.function_id)
                    .cloned()
                    .flatten();
            }
            callable
        })
        .collect()
}

fn compute_module_make_function_capture_names(
    callable_defs: &[BlockPyFunction<CoreBlockPyPass>],
) -> HashMap<FunctionId, Vec<CellCaptureBinding>> {
    fn compute_callable_make_function_capture_bindings_for_name_binding(
        function_id: FunctionId,
        callable_by_id: &HashMap<FunctionId, &BlockPyFunction<CoreBlockPyPass>>,
        make_function_callees: &HashMap<FunctionId, Vec<FunctionId>>,
        memo: &mut HashMap<FunctionId, Vec<CellCaptureBinding>>,
        visiting: &mut HashSet<FunctionId>,
    ) -> Vec<CellCaptureBinding> {
        if let Some(captures) = memo.get(&function_id) {
            return captures.clone();
        }

        let callable = callable_by_id
            .get(&function_id)
            .unwrap_or_else(|| panic!("missing callable for function id {:?}", function_id));
        let layout_capture_bindings = || {
            callable
                .storage_layout
                .as_ref()
                .map(|layout| {
                    layout
                        .freevars
                        .iter()
                        .map(|slot| CellCaptureBinding {
                            logical_name: slot.logical_name.clone(),
                            source_name: callable
                                .scope
                                .cell_capture_source_name(slot.logical_name.as_str()),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| compute_make_function_capture_bindings_from_scope(callable))
        };
        if matches!(
            callable.kind,
            FunctionKind::Generator | FunctionKind::Coroutine | FunctionKind::AsyncGenerator
        ) {
            let captures = layout_capture_bindings();
            memo.insert(function_id, captures.clone());
            return captures;
        }
        if !visiting.insert(function_id) {
            let captures = compute_make_function_capture_bindings_from_scope(callable);
            memo.insert(function_id, captures.clone());
            return captures;
        }

        let mut captures = compute_make_function_capture_bindings_from_scope(callable);
        let base_owned_logical_names = callable
            .scope
            .bindings
            .iter()
            .filter_map(|(name, binding)| {
                matches!(binding, BindingKind::Cell(CellBindingKind::Owner)).then(|| name.clone())
            })
            .collect::<HashSet<_>>();
        let base_owned_storage_names = callable.scope.owned_cell_storage_names();

        if let Some(callee_ids) = make_function_callees.get(&function_id) {
            for callee_id in callee_ids {
                let callee_captures =
                    compute_callable_make_function_capture_bindings_for_name_binding(
                        *callee_id,
                        callable_by_id,
                        make_function_callees,
                        memo,
                        visiting,
                    );
                for capture in callee_captures {
                    let mut logical_name = capture.logical_name;
                    loop {
                        let next = callable
                            .scope
                            .logical_name_for_cell_capture_source(logical_name.as_str())
                            .or_else(|| {
                                callable
                                    .scope
                                    .logical_name_for_cell_storage(logical_name.as_str())
                            })
                            .unwrap_or_else(|| logical_name.clone());
                        if next == logical_name {
                            break;
                        }
                        logical_name = next;
                    }
                    let source_name = callable
                        .scope
                        .cell_capture_source_name(logical_name.as_str());
                    if base_owned_logical_names.contains(logical_name.as_str())
                        || base_owned_storage_names.contains(source_name.as_str())
                    {
                        continue;
                    }
                    captures.push(CellCaptureBinding {
                        logical_name,
                        source_name,
                    });
                }
            }
        }

        visiting.remove(&function_id);
        captures.sort_by(|left, right| {
            left.logical_name
                .cmp(&right.logical_name)
                .then_with(|| left.source_name.cmp(&right.source_name))
        });
        captures.dedup_by(|left, right| left.logical_name == right.logical_name);
        memo.insert(function_id, captures.clone());
        captures
    }

    let callable_by_id = callable_defs
        .iter()
        .map(|callable| (callable.function_id, callable))
        .collect::<HashMap<_, _>>();
    let make_function_callees = callable_defs
        .iter()
        .map(|callable| {
            (
                callable.function_id,
                collect_make_function_callee_ids(callable),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut memo = HashMap::new();
    let mut visiting = HashSet::new();

    callable_defs
        .iter()
        .map(|callable| {
            let captures = if callable.scope.scope_kind == CallableScopeKind::Class {
                compute_callable_make_function_capture_bindings_for_name_binding(
                    callable.function_id,
                    &callable_by_id,
                    &make_function_callees,
                    &mut memo,
                    &mut visiting,
                )
            } else {
                callable
                    .storage_layout
                    .as_ref()
                    .map(|layout| {
                        layout
                            .freevars
                            .iter()
                            .map(|slot| CellCaptureBinding {
                                logical_name: slot.logical_name.clone(),
                                source_name: callable
                                    .scope
                                    .cell_capture_source_name(slot.logical_name.as_str()),
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_else(|| compute_make_function_capture_bindings_from_scope(callable))
            };
            (callable.function_id, captures)
        })
        .collect()
}

fn refresh_bb_callable_block_params(
    callable: BlockPyFunction<ResolvedStorageBlockPyPass>,
) -> BlockPyFunction<ResolvedStorageBlockPyPass> {
    let BlockPyFunction {
        function_id,
        name_gen,
        names,
        kind,
        params,
        blocks,
        doc,
        storage_layout,
        scope,
    } = callable;
    let mut blocks = blocks
        .into_iter()
        .map(|block| {
            let params = block.bb_params().cloned().collect();
            crate::block_py::Block {
                label: block.label,
                body: block.body,
                term: block.term,
                params,
                exc_edge: block.exc_edge,
            }
        })
        .collect::<Vec<_>>();
    populate_exception_edge_args(&mut blocks);
    populate_jump_edge_args(&mut blocks);
    BlockPyFunction {
        function_id,
        name_gen,
        names,
        kind,
        params,
        blocks,
        doc,
        storage_layout,
        scope,
    }
}

fn populate_jump_edge_args(blocks: &mut [crate::block_py::ResolvedStorageBlock]) {
    let label_to_index = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect::<HashMap<_, _>>();
    for block_index in 0..blocks.len() {
        let BlockTerm::Jump(edge) = &blocks[block_index].term else {
            continue;
        };
        let Some(target_index) = label_to_index.get(&edge.target).copied() else {
            continue;
        };
        let target_params = blocks[target_index].params.clone();
        if target_params.is_empty() {
            continue;
        }
        let source_params = blocks[block_index].params.clone();
        let explicit_args = edge.args.clone();
        let explicit_start = target_params.len().saturating_sub(explicit_args.len());
        let new_args = target_params
            .iter()
            .enumerate()
            .map(|(param_index, target_param)| {
                if param_index >= explicit_start {
                    return explicit_args[param_index - explicit_start].clone();
                }
                if source_params
                    .iter()
                    .any(|source_param| source_param.name == target_param.name)
                {
                    return BlockArg::Name(target_param.name.clone());
                }
                if let Some(source_same_role) = source_params
                    .iter()
                    .find(|source_param| source_param.role == target_param.role)
                {
                    return BlockArg::Name(source_same_role.name.clone());
                }
                BlockArg::None
            })
            .collect::<Vec<_>>();
        if let BlockTerm::Jump(edge) = &mut blocks[block_index].term {
            edge.args = new_args;
        }
    }
}

fn lower_name_binding_callable(
    callable: BlockPyFunction<CoreBlockPyPass>,
    callee_make_function_captures: &HashMap<crate::block_py::FunctionId, Vec<CellCaptureBinding>>,
    global_slots: &mut ModuleGlobalSlots,
) -> BlockPyFunction<ResolvedStorageBlockPyPass> {
    let scope = callable.scope.clone();
    let local_slots = collect_local_slot_locations(&callable);
    let mut mapper = NameBindingMapper {
        scope: &scope,
        callee_make_function_captures,
        local_slots: local_slots.clone(),
    };
    let mut lowered = mapper.map_fn(callable);
    prepend_owned_cell_init_preamble(&mut lowered);
    populate_stack_slots_in_storage_layout(&mut lowered, local_slots);
    let storage_layout = lowered
        .storage_layout
        .as_ref()
        .expect("name binding should have storage layout before cell-location analysis");
    let deleted_names = collect_deleted_names_in_blocks(&lowered.blocks, &scope, storage_layout);
    let always_unbound_names = collect_always_unbound_local_names(&lowered);
    if !deleted_names.is_empty() || !always_unbound_names.is_empty() {
        for block in &mut lowered.blocks {
            for stmt in &mut block.body {
                rewrite_deleted_name_loads_in_stmt(
                    stmt,
                    &scope,
                    storage_layout,
                    &mapper,
                    &deleted_names,
                    &always_unbound_names,
                );
            }
            rewrite_deleted_name_loads_in_term(
                &mut block.term,
                &scope,
                storage_layout,
                &mapper,
                &deleted_names,
                &always_unbound_names,
            );
        }
    }
    rewrite_current_exception_in_core_blocks(&mut lowered.blocks);
    let normal_predecessors = normal_predecessor_exc_param_names(&lowered.blocks);
    for block in &mut lowered.blocks {
        let predecessor_exc_names = normal_predecessors
            .get(&block.label)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        sync_exception_param_cell_in_block(block, predecessor_exc_names, &scope, &mapper);
        for stmt in &mut block.body {
            rewrite_raw_cell_loads_in_stmt(stmt, &scope, &mapper);
        }
        rewrite_raw_cell_loads_in_term(&mut block.term, &scope, &mapper);
    }
    let mut lowered = normalize_stmt_ops_in_resolved_callable(refresh_bb_callable_block_params(
        locate_names_in_callable(lowered, global_slots),
    ));
    ensure_storage_layout_covers_block_params(&mut lowered);
    lowered
}

fn normalize_stmt_ops_in_resolved_callable(
    callable: BlockPyFunction<ResolvedStorageBlockPyPass>,
) -> BlockPyFunction<ResolvedStorageBlockPyPass> {
    callable
}

#[derive(Default)]
struct ModuleConstantExtractor {
    constants: Vec<InstrResolved>,
}

impl ModuleConstantExtractor {
    fn extract_module(
        mut self,
        mut module: BlockPyModule<ResolvedStorageBlockPyPass>,
    ) -> BlockPyModule<ResolvedStorageBlockPyPass> {
        debug_assert!(
            module.module_constants.is_empty(),
            "name binding should be the first pass to populate module constants"
        );
        for callable in &mut module.callable_defs {
            self.extract_function(callable);
        }
        module.module_constants = self.constants;
        module
    }

    fn extract_function(&mut self, function: &mut BlockPyFunction<ResolvedStorageBlockPyPass>) {
        for block in &mut function.blocks {
            for stmt in &mut block.body {
                self.extract_stmt(stmt);
            }
            self.extract_term(&mut block.term);
        }
    }

    fn extract_stmt(&mut self, stmt: &mut InstrResolved) {
        self.extract_expr(stmt);
    }

    fn extract_term(&mut self, term: &mut BlockTerm<InstrResolved>) {
        crate::block_py::walk_term_mut(self, term);
    }

    fn extract_expr(&mut self, expr: &mut InstrResolved) {
        if matches!(expr, InstrResolved::Literal(_))
            || matches!(expr, InstrResolved::Load(op) if op.name.is_runtime_name())
        {
            let meta = expr.meta();
            let index = u32::try_from(self.constants.len())
                .expect("module constant count should fit in NameLocation::Constant");
            let constant = std::mem::replace(expr, constant_location_expr(meta, index));
            self.constants.push(constant);
            return;
        }
        if !matches!(expr, InstrResolved::Literal(_)) {
            expr.visit_children_mut(self);
        }
    }
}

impl crate::block_py::VisitMut<InstrResolved> for ModuleConstantExtractor {
    fn visit_instr_mut(&mut self, expr: &mut InstrResolved) {
        self.extract_expr(expr);
    }
}

pub(crate) fn lower_name_binding_in_core_blockpy_module(
    module: BlockPyModule<CoreBlockPyPass>,
) -> BlockPyModule<ResolvedStorageBlockPyPass> {
    let callable_defs = ensure_module_storage_layouts(module.callable_defs);
    let callee_make_function_capture_names =
        compute_module_make_function_capture_names(&callable_defs);
    let mut global_slots = ModuleGlobalSlots::default();
    let mut lowered = ModuleConstantExtractor::default().extract_module(BlockPyModule {
        module_name_gen: module.module_name_gen,
        global_names: Vec::new(),
        callable_defs: callable_defs
            .into_iter()
            .map(|callable| {
                lower_name_binding_callable(
                    callable,
                    &callee_make_function_capture_names,
                    &mut global_slots,
                )
            })
            .collect(),
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    });
    lowered.global_names = global_slots.into_names();
    lowered
}
