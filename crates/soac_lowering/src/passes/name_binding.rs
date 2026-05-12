use crate::block_py::{
    build_storage_layout_from_capture_names, compute_make_function_capture_bindings_from_scope,
    compute_storage_layout_from_scope, core_runtime_positional_call_expr_with_meta,
    is_runtime_closure_name, literal_expr, BindingKind, BindingPurpose, BindingTarget, Block,
    BlockArg, BlockPyFunction, BlockPyModule, BlockTerm, Call, CallArgPositional,
    CallableScopeInfo, CallableScopeKind, CellBindingKind, CellCaptureBinding, CellLocation,
    CellRef, CellRefForName, ChildVisitable, ClassBodyFallback, ClosureInit, ClosureSlot, Del,
    DelItem, EffectiveBinding, FunctionKind, HasMeta, InstrLow, InstrResolved, InstrUnresolved,
    IntLiteral, Load, MakeCell, MakeFunction, MakeFunctionWithClosure, MapFunction, MapInstr,
    MapTerm, Mappable, NameLike, NameLocation, NumberLiteral, NumberLiteralValue,
    PreservedSlotStorage, ResolvedName, RuntimeFunctionId, RuntimeName, SetItem, StorageLayout,
    Store, StringLiteral, Tuple, UnresolvedName, Visit, VisitMut, WithMeta,
};
use crate::passes::ruff_to_blockpy::{
    populate_exception_edge_args, rewrite_current_exception_in_core_blocks,
};
use crate::passes::{CoreModuleShape, ResolvedStorageModuleShape};
use ruff_python_ast::{self as ast};
use soac_macros::match_default;
use std::collections::{HashMap, HashSet};

fn is_internal_symbol(name: &str) -> bool {
    name.starts_with("_dp_") || name == "__soac__"
}

fn is_unsound_runtime_builtin_candidate(name: &str) -> bool {
    // Builtins that are expected to live in the builtin namespace. Rewriting
    // these as RuntimeName is intentionally unsound because it skips module
    // globals and snapshots the value in a module constant slot.
    matches!(
        name,
        "ArithmeticError"
            | "AssertionError"
            | "AttributeError"
            | "BaseException"
            | "BaseExceptionGroup"
            | "BlockingIOError"
            | "BrokenPipeError"
            | "BufferError"
            | "BytesWarning"
            | "ChildProcessError"
            | "ConnectionAbortedError"
            | "ConnectionError"
            | "ConnectionRefusedError"
            | "ConnectionResetError"
            | "DeprecationWarning"
            | "EOFError"
            | "EncodingWarning"
            | "EnvironmentError"
            | "Exception"
            | "ExceptionGroup"
            | "FileExistsError"
            | "FileNotFoundError"
            | "FloatingPointError"
            | "FutureWarning"
            | "GeneratorExit"
            | "IOError"
            | "ImportError"
            | "ImportWarning"
            | "IndentationError"
            | "IndexError"
            | "InterruptedError"
            | "IsADirectoryError"
            | "KeyError"
            | "KeyboardInterrupt"
            | "LookupError"
            | "MemoryError"
            | "ModuleNotFoundError"
            | "NameError"
            | "NotADirectoryError"
            | "NotImplemented"
            | "NotImplementedError"
            | "OSError"
            | "OverflowError"
            | "PendingDeprecationWarning"
            | "PermissionError"
            | "ProcessLookupError"
            | "RecursionError"
            | "ReferenceError"
            | "ResourceWarning"
            | "RuntimeError"
            | "RuntimeWarning"
            | "StopAsyncIteration"
            | "StopIteration"
            | "SyntaxError"
            | "SyntaxWarning"
            | "SystemError"
            | "SystemExit"
            | "TabError"
            | "TimeoutError"
            | "TypeError"
            | "UnboundLocalError"
            | "UnicodeDecodeError"
            | "UnicodeEncodeError"
            | "UnicodeError"
            | "UnicodeTranslateError"
            | "UnicodeWarning"
            | "UserWarning"
            | "ValueError"
            | "Warning"
            | "ZeroDivisionError"
            | "__build_class__"
            | "__import__"
            | "abs"
            | "aiter"
            | "all"
            | "anext"
            | "any"
            | "ascii"
            | "bin"
            | "bool"
            | "breakpoint"
            | "bytearray"
            | "bytes"
            | "callable"
            | "chr"
            | "classmethod"
            | "compile"
            | "complex"
            | "copyright"
            | "credits"
            | "delattr"
            | "dict"
            | "dir"
            | "divmod"
            | "enumerate"
            | "eval"
            | "exec"
            | "exit"
            | "filter"
            | "float"
            | "format"
            | "frozenset"
            | "getattr"
            | "globals"
            | "hasattr"
            | "hash"
            | "help"
            | "hex"
            | "id"
            | "input"
            | "int"
            | "isinstance"
            | "issubclass"
            | "iter"
            | "len"
            | "license"
            | "list"
            | "locals"
            | "map"
            | "max"
            | "memoryview"
            | "min"
            | "next"
            | "object"
            | "oct"
            | "open"
            | "ord"
            | "pow"
            | "print"
            | "property"
            | "quit"
            | "range"
            | "repr"
            | "reversed"
            | "round"
            | "set"
            | "setattr"
            | "slice"
            | "sorted"
            | "staticmethod"
            | "str"
            | "sum"
            | "tuple"
            | "type"
            | "vars"
            | "zip"
    )
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
        StringLiteral { value },
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
    N: NameLike,
{
    match expr {
        InstrLow::Load(op) => Some(op.name.id_str().to_string()),
        _ => None,
    }
}

fn raw_resolved_load_name(expr: &InstrResolved) -> Option<String> {
    match expr {
        InstrResolved::Load(op) => Some(op.name.id_str().to_string()),
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
        BindingTarget::Local => op_stmt(Del::new(target, false).with_meta(meta)),
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
            "NONE"
                | "TRUE"
                | "FALSE"
                | "ELLIPSIS"
                | "globals"
                | "class_lookup_global"
                | "class_lookup_cell"
                | "tuple"
        )
    {
        return Load::new(<UnresolvedName as NameLike>::runtime_name(id))
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
            BindingTarget::Local => op_stmt(Del::new(name, true).with_meta(meta)),
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
        CallArgPositional::Positional(expr) => raw_load_name(expr).map(ast::name::Name::new),
        _ => None,
    }
}

fn cell_ref_marker_target(expr: &InstrUnresolved) -> Option<String> {
    let InstrUnresolved::CellRefForName(CellRefForName { logical_name, .. }) = expr else {
        return None;
    };
    Some(logical_name.clone())
}

fn build_local_cell_init_assign(
    storage_name: &str,
    logical_name: &str,
    is_parameter: bool,
) -> CoreStmt {
    let node_index = compat_node_index();
    let range = compat_range();
    let make_cell = if is_parameter {
        MakeCell::with_initial_value(core_name_expr(
            logical_name,
            ast::ExprContext::Load,
            node_index.clone(),
            range,
        ))
    } else {
        MakeCell::empty()
    };
    op_stmt(
        Store::new(
            ast::name::Name::new(storage_name),
            Box::new(op_expr(make_cell.with_meta(crate::block_py::Meta::new(
                node_index.clone(),
                range,
            )))),
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
        ClosureInit::EmptyCell => {
            panic!("empty cells should lower through MakeCell::empty, not an expression")
        }
        ClosureInit::RuntimePcUnstarted => literal_expr(
            NumberLiteral {
                value: NumberLiteralValue::Int(IntLiteral::from_i64(1)),
            },
            crate::block_py::Meta::new(node_index, range),
        ),
        ClosureInit::RuntimeAbruptKindFallthrough => literal_expr(
            NumberLiteral {
                value: NumberLiteralValue::Int(IntLiteral::from_i64(0)),
            },
            crate::block_py::Meta::new(node_index, range),
        ),
        ClosureInit::RuntimeZero => literal_expr(
            NumberLiteral {
                value: NumberLiteralValue::Int(IntLiteral::from_i64(0)),
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
            Box::new(op_expr({
                let make_cell = match slot.init {
                    ClosureInit::EmptyCell => MakeCell::empty(),
                    _ => MakeCell::with_initial_value(closure_slot_init_expr(slot)),
                };
                make_cell.with_meta(crate::block_py::Meta::new(node_index.clone(), range))
            })),
        )
        .with_meta(crate::block_py::Meta::new(node_index, range)),
    )
}

fn prepend_owned_cell_init_preamble(callable: &mut BlockPyFunction<CoreModuleShape>) {
    let init_stmts = match callable.kind {
        FunctionKind::Function => {
            let mut storage_bindings = collect_owned_cell_storage_bindings(callable);
            if storage_bindings.is_empty() {
                return;
            }
            storage_bindings
                .sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
            let param_names = callable.params.names().into_iter().collect::<HashSet<_>>();
            storage_bindings
                .into_iter()
                .map(|(logical_name, storage_name)| {
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

struct NameBindingMapper<'a> {
    scope: &'a CallableScopeInfo,
    callee_make_function_captures:
        &'a HashMap<crate::block_py::RuntimeFunctionId, Vec<CellCaptureBinding>>,
}

impl NameBindingMapper<'_> {
    fn materialize_capture_tuple(
        &mut self,
        function_id: RuntimeFunctionId,
        meta: crate::block_py::Meta,
    ) -> InstrUnresolved {
        let captures: Vec<InstrUnresolved> = self
            .callee_make_function_captures
            .get(&function_id)
            .into_iter()
            .flat_map(|captures| captures.iter())
            .map(|capture| {
                Tuple::new(vec![
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
                ])
                .with_meta(meta.clone())
                .into()
            })
            .collect::<Vec<_>>();
        Tuple::new(captures).with_meta(meta).into()
    }

    fn materialize_make_function_expr(
        &mut self,
        meta: crate::block_py::Meta,
        op: MakeFunction<InstrUnresolved>,
    ) -> InstrUnresolved {
        let captures_expr = self.materialize_capture_tuple(op.function_id, meta.clone());
        MakeFunctionWithClosure::new(
            op.function_id,
            op.kind,
            captures_expr,
            self.map_instr(*op.param_defaults),
            self.map_instr(*op.annotate_fn),
        )
        .with_meta(meta)
        .into()
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
        return rewrite_cell_binding_assign(target, value, meta, scope, resolver);
    }
    match scope.binding_target_for_name(name.as_str(), BindingPurpose::Store) {
        BindingTarget::ModuleGlobal => rewrite_global_binding_assign(target, value, meta),
        BindingTarget::ClassNamespace => {
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
                            name.clone().into_ast_name(),
                            meta.clone(),
                            self.scope,
                            self,
                        )
                        .expect("raw cell-storage load guard should ensure rewrite target")
                    } else if should_rewrite_raw_name_load(name.as_str(), self.scope) {
                        rewrite_name_load(name.into_ast_name(), meta, self.scope, self)
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
        | InstrUnresolved::Tuple(_)
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
        | InstrUnresolved::MakeFunction(_)
        | InstrUnresolved::MakeFunctionWithClosure(_) => {
            if let InstrUnresolved::Load(op) = expr {
                if let UnresolvedName::SourceName(name) = &op.name {
                    if matches!(
                        scope.binding_kind(name.as_str()),
                        Some(BindingKind::Cell(_))
                    ) {
                        *expr = rewrite_cell_name_load(
                            name.clone().into_ast_name(),
                            op.meta(),
                            scope,
                            resolver,
                        );
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
    blocks: &[crate::block_py::Block<InstrUnresolved>],
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

fn sync_block_param_cell_in_block(
    param: &crate::block_py::BlockParam,
    should_sync: bool,
    scope: &CallableScopeInfo,
    resolver: &NameBindingMapper<'_>,
) -> Option<CoreStmt> {
    if !should_sync {
        return None;
    }
    if !matches!(
        param.role,
        crate::block_py::BlockParamRole::Exception
            | crate::block_py::BlockParamRole::AbruptKind
            | crate::block_py::BlockParamRole::AbruptPayload
    ) {
        return None;
    }
    if !matches!(
        scope.binding_kind(param.name.as_str()),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    ) {
        return None;
    }

    let node_index = compat_node_index();
    let range = compat_range();
    let target_name = scope.cell_capture_source_name(param.name.as_str());
    let param_load = ast::name::Name::new(param.name.clone());
    let meta = crate::block_py::Meta::new(node_index.clone(), range);
    Some(op_stmt(
        Store::new(
            ast::name::Name::new(target_name),
            Box::new(rewrite_local_name_load(param_load, meta.clone(), resolver)),
        )
        .with_meta(crate::block_py::Meta::new(node_index, range)),
    ))
}

fn sync_block_param_preserved_slot_in_block(
    param: &crate::block_py::BlockParam,
    should_sync: bool,
    storage_layout: Option<&crate::block_py::StorageLayout>,
    resolver: &NameBindingMapper<'_>,
) -> Option<CoreStmt> {
    if !should_sync {
        return None;
    }
    let preserved_slot = storage_layout?
        .preserved_slots
        .iter()
        .find(|slot| slot.logical_name == param.name)
        .cloned()?;
    let target_name = match preserved_slot.storage {
        PreservedSlotStorage::PyCellObject => preserved_slot.logical_name,
        PreservedSlotStorage::PyObjectOrNull | PreservedSlotStorage::I64 => {
            preserved_slot.storage_name
        }
    };

    let node_index = compat_node_index();
    let range = compat_range();
    let param_load = ast::name::Name::new(param.name.clone());
    let meta = crate::block_py::Meta::new(node_index.clone(), range);
    Some(op_stmt(
        Store::new(
            ast::name::Name::new(target_name),
            Box::new(rewrite_local_name_load(param_load, meta.clone(), resolver)),
        )
        .with_meta(crate::block_py::Meta::new(node_index, range)),
    ))
}

fn sync_backed_block_params_in_block(
    block: &mut crate::block_py::Block<InstrUnresolved>,
    normal_predecessor_exc_names: &[Option<String>],
    scope: &CallableScopeInfo,
    storage_layout: Option<&crate::block_py::StorageLayout>,
    resolver: &NameBindingMapper<'_>,
) {
    let active_exception_name = block.exception_param().map(ToString::to_string);
    let should_sync_exception = active_exception_name.as_deref().is_some_and(|exc_name| {
        !normal_predecessor_exc_names.iter().any(|pred_exc_name| {
            pred_exc_name
                .as_deref()
                .is_some_and(|pred_exc_name| pred_exc_name != exc_name)
        })
    });
    let sync_stmts = block
        .params
        .iter()
        .flat_map(|param| {
            let should_sync =
                param.role != crate::block_py::BlockParamRole::Exception || should_sync_exception;
            [
                sync_block_param_cell_in_block(param, should_sync, scope, resolver),
                sync_block_param_preserved_slot_in_block(
                    param,
                    should_sync,
                    storage_layout,
                    resolver,
                ),
            ]
            .into_iter()
            .flatten()
        })
        .collect::<Vec<_>>();
    if sync_stmts.is_empty() {
        return;
    }
    block.body.splice(0..0, sync_stmts);
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
        | InstrUnresolved::Tuple(_)
        | InstrUnresolved::Call(_)
        | InstrUnresolved::GetAttr(_)
        | InstrUnresolved::SetAttr(_)
        | InstrUnresolved::GetItem(_)
        | InstrUnresolved::SetItem(_)
        | InstrUnresolved::DelItem(_)
        | InstrUnresolved::MakeCell(_)
        | InstrUnresolved::CellRefForName(_)
        | InstrUnresolved::CellRef(_)
        | InstrUnresolved::MakeFunction(_)
        | InstrUnresolved::MakeFunctionWithClosure(_) => {}
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
    callable: &BlockPyFunction<CoreModuleShape>,
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
    callable: &BlockPyFunction<CoreModuleShape>,
) -> Vec<(String, String)> {
    if let Some(layout) = callable.storage_layout.as_ref() {
        return layout
            .cellvars
            .iter()
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

fn collect_preserved_cell_slot_locations(
    callable: &BlockPyFunction<CoreModuleShape>,
) -> HashMap<String, u32> {
    let mut slots = HashMap::new();
    if let Some(layout) = callable.storage_layout.as_ref() {
        for (slot, preserved_slot) in layout.preserved_slots.iter().enumerate() {
            if preserved_slot.storage != PreservedSlotStorage::PyCellObject {
                continue;
            }
            slots.insert(preserved_slot.storage_name.clone(), slot as u32);
            slots.insert(preserved_slot.logical_name.clone(), slot as u32);
        }
    }
    slots
}

fn collect_owned_cell_slot_locations(
    callable: &BlockPyFunction<CoreModuleShape>,
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

fn collect_preserved_slot_locations(
    callable: &BlockPyFunction<CoreModuleShape>,
) -> HashMap<String, u32> {
    let mut slots = HashMap::new();
    if let Some(layout) = callable.storage_layout.as_ref() {
        for (slot, preserved_slot) in layout.preserved_slots.iter().enumerate() {
            slots.insert(preserved_slot.storage_name.clone(), slot as u32);
            slots.insert(preserved_slot.logical_name.clone(), slot as u32);
        }
    }
    slots
}

fn materialize_preserved_block_arg_sources(callable: &mut BlockPyFunction<CoreModuleShape>) {
    let preserved_slots = collect_preserved_slot_locations(callable);
    if preserved_slots.is_empty() {
        return;
    }
    let name_gen = callable.name_gen.share();
    for block in &mut callable.blocks {
        let current_param_names = block
            .param_names()
            .map(ToString::to_string)
            .collect::<HashSet<_>>();
        let BlockTerm::Jump(edge) = &mut block.term else {
            continue;
        };
        for arg in &mut edge.args {
            let BlockArg::Name(source_name) = arg else {
                continue;
            };
            if !preserved_slots.contains_key(source_name.as_str())
                || current_param_names.contains(source_name)
            {
                continue;
            }
            let local_name = name_gen.next_tmp_name("preserved_arg").to_string();
            let meta = crate::block_py::Meta::new(compat_node_index(), compat_range());
            block.body.push(op_stmt(
                Store::new(
                    ast::name::Name::new(local_name.clone()),
                    Box::new(rewrite_global_name_load(
                        ast::name::Name::new(source_name.clone()),
                        meta.clone(),
                    )),
                )
                .with_meta(meta),
            ));
            *source_name = local_name;
        }
    }
}

fn collect_cell_bindings(
    callable: &BlockPyFunction<CoreModuleShape>,
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

    for slot in &layout.preserved_slots {
        if slot.storage != PreservedSlotStorage::PyCellObject {
            continue;
        }
        add_binding(
            slot.logical_name.as_str(),
            slot.storage_name.as_str(),
            CellBindingKind::Owner,
        );
        add_binding(
            slot.storage_name.as_str(),
            slot.storage_name.as_str(),
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
    callable: &BlockPyFunction<CoreModuleShape>,
) -> HashMap<String, u32> {
    let mut slots = HashMap::new();
    for (slot, param_name) in callable.body_params().names().into_iter().enumerate() {
        slots.insert(param_name, slot as u32);
    }
    let mut next_slot = slots.len() as u32;
    let mut owned_cell_storage_names = callable
        .storage_layout
        .as_ref()
        .map(|layout| {
            layout
                .cellvars
                .iter()
                .map(|slot| slot.storage_name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            callable
                .scope
                .owned_cell_storage_names()
                .into_iter()
                .collect::<Vec<_>>()
        });
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

    let preserved_slot_names = collect_preserved_slot_locations(callable);
    let mut non_param_locals = remaining
        .into_iter()
        .filter(|name| !slots.contains_key(name))
        .filter(|name| !preserved_slot_names.contains_key(name))
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
    callable: &BlockPyFunction<CoreModuleShape>,
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

fn populate_stack_slots_in_storage_layout<P: crate::block_py::ModuleShape>(
    callable: &mut BlockPyFunction<P>,
    local_slots: HashMap<String, u32>,
) {
    let stack_slots = ordered_slot_names_from_local_slots(local_slots);
    callable
        .storage_layout
        .get_or_insert_with(StorageLayout::default)
        .set_stack_slots(stack_slots);
}

fn ensure_storage_layout_covers_block_params<P: crate::block_py::ModuleShape>(
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
    current_block_param_names: HashSet<String>,
    local_slots: HashMap<String, u32>,
    captured_cell_slots: HashMap<String, u32>,
    owned_cell_slots: HashMap<String, u32>,
    preserved_cell_slots: HashMap<String, u32>,
    preserved_slots: HashMap<String, u32>,
    cell_bindings: HashMap<String, (String, CellBindingKind)>,
}

impl NameLocator<'_> {
    fn map_block_with_current_params(
        &mut self,
        block: Block<InstrUnresolved>,
    ) -> Block<InstrResolved> {
        let previous_block_param_names = std::mem::replace(
            &mut self.current_block_param_names,
            block.param_names().map(ToString::to_string).collect(),
        );
        let block = Block {
            label: block.label,
            body: block
                .body
                .into_iter()
                .map(|stmt| self.map_instr(stmt))
                .collect(),
            term: self.map_term(block.term),
            params: block.params,
            exc_edge: block.exc_edge,
            extra: Default::default(),
        };
        self.current_block_param_names = previous_block_param_names;
        block
    }

    fn map_callable(
        &mut self,
        func: BlockPyFunction<CoreModuleShape>,
    ) -> BlockPyFunction<ResolvedStorageModuleShape> {
        BlockPyFunction {
            function_id: func.function_id,
            name_gen: func.name_gen,
            names: func.names,
            kind: func.kind,
            execution_mode: func.execution_mode,
            params: func.params,
            body_params: func.body_params,
            public_scope: func.public_scope,
            blocks: func
                .blocks
                .into_iter()
                .map(|block| self.map_block_with_current_params(block))
                .collect(),
            doc: func.doc,
            public_storage_layout: func.public_storage_layout,
            storage_layout: func.storage_layout,
            scope: func.scope,
        }
    }

    fn resolve_raw_cell_location(&self, name_text: &str) -> CellLocation {
        if let Some(slot) = self.preserved_cell_slots.get(name_text).copied() {
            return CellLocation::Preserved(slot);
        }
        if let Some(slot) = self.owned_cell_slots.get(name_text).copied() {
            return CellLocation::Owned(slot);
        }
        if let Some(storage_name) = name_text.strip_prefix("_dp_cell_") {
            if let Some(slot) = self.preserved_cell_slots.get(storage_name).copied() {
                return CellLocation::Preserved(slot);
            }
            if let Some(slot) = self.owned_cell_slots.get(storage_name).copied() {
                return CellLocation::Owned(slot);
            }
        }

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
                    if let Some(slot) = self
                        .preserved_cell_slots
                        .get(storage_name.as_str())
                        .copied()
                    {
                        return CellLocation::Preserved(slot);
                    }
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

        panic!(
            "raw cell target {name_text} did not resolve to a cell-backed location in {}; owned={:?}; captured={:?}; bindings={:?}",
            self.scope.names.qualname,
            self.owned_cell_slots,
            self.captured_cell_slots,
            self.cell_bindings
        );
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

    fn locate_name(&mut self, name: crate::block_py::BlockPyName) -> ResolvedName {
        let name_text = name.to_string();
        let location = if self.current_block_param_names.contains(name_text.as_str()) {
            let slot = self
                .local_slots
                .get(name_text.as_str())
                .copied()
                .unwrap_or_else(|| {
                    panic!("missing local slot for block param {name_text}");
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
                    if let Some(slot) = self
                        .preserved_cell_slots
                        .get(storage_name.as_str())
                        .copied()
                    {
                        return ResolvedName {
                            id: name,
                            location: NameLocation::preserved_cell(slot),
                        };
                    }
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
        } else if let Some(slot) = self.preserved_slots.get(name_text.as_str()).copied() {
            NameLocation::preserved(slot)
        } else if let Some(slot) = self.local_slots.get(name_text.as_str()).copied() {
            NameLocation::local(slot)
        } else {
            NameLocation::global_name()
        };
        ResolvedName { id: name, location }
    }

    fn locate_make_cell_initializer_name(
        &mut self,
        name: crate::block_py::BlockPyName,
    ) -> ResolvedName {
        let name_text = name.to_string();
        if let Some((storage_name, CellBindingKind::Owner)) =
            self.cell_bindings.get(name_text.as_str())
        {
            if name_text != *storage_name {
                if let Some(slot) = self.local_slots.get(name_text.as_str()).copied() {
                    return ResolvedName {
                        id: name,
                        location: NameLocation::local(slot),
                    };
                }
            }
        }
        self.locate_name(name)
    }

    fn locate_unresolved_name(&mut self, name: UnresolvedName) -> ResolvedName {
        match name {
            UnresolvedName::SourceName(name) => self.locate_name(name),
            UnresolvedName::RuntimeName(name) => ResolvedName {
                id: name.name().into(),
                location: NameLocation::RuntimeName(name),
            },
        }
    }

    fn mark_raw_cell_store_name(&self, name: ResolvedName) -> ResolvedName {
        let name_text = name.id.to_string();
        if self
            .scope
            .logical_name_for_cell_storage(name_text.as_str())
            .is_some()
        {
            if let Some(slot) = self.local_slots.get(name_text.as_str()).copied() {
                return name.with_location(NameLocation::local(slot));
            }
        }
        self.mark_raw_cell_name(name)
    }

    fn map_make_cell_initial_value(&mut self, expr: InstrUnresolved) -> InstrResolved {
        let InstrUnresolved::Load(op) = expr else {
            return self.map_instr(expr);
        };
        let meta = op.meta();
        let name = match op.name {
            UnresolvedName::SourceName(name) => self.locate_make_cell_initializer_name(name),
            UnresolvedName::RuntimeName(name) => ResolvedName {
                id: name.name().into(),
                location: NameLocation::RuntimeName(name),
            },
        };
        Load::new(name).with_meta(meta).into()
    }

    fn mark_raw_cell_name(&self, name: ResolvedName) -> ResolvedName {
        let name_text = name.id.to_string();
        if name.location.is_global() || name.location.is_global_name() {
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

    fn mark_raw_cell_expr(&self, expr: InstrResolved) -> InstrResolved {
        match expr {
            InstrResolved::Load(op) => {
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

impl MapInstr<InstrUnresolved, InstrResolved> for NameLocator<'_> {
    fn map_instr(&mut self, expr: InstrUnresolved) -> InstrResolved {
        if let InstrLow::MakeCell(op) = expr {
            let meta = op.meta();
            let initial_value = op
                .initial_value
                .map(|initial_value| Box::new(self.map_make_cell_initial_value(*initial_value)));
            return MakeCell::new(initial_value).with_meta(meta).into();
        }
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
                if raw_resolved_load_name(call.func.as_ref())
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
            InstrLow::MakeFunction(_) => {
                panic!("MakeFunction should lower to MakeFunctionWithClosure before name location")
            },
            rest => rest.map_children(self).into(),
        })
    }

    fn map_name(&mut self, name: UnresolvedName) -> ResolvedName {
        self.locate_unresolved_name(name)
    }
}

fn locate_names_in_callable(
    callable: BlockPyFunction<CoreModuleShape>,
) -> BlockPyFunction<ResolvedStorageModuleShape> {
    let scope = callable.scope.clone();
    let local_slots = collect_local_slot_locations(&callable);
    let captured_cell_slots = collect_captured_cell_slot_locations(&callable);
    let owned_cell_slots = collect_owned_cell_slot_locations(&callable);
    let preserved_cell_slots = collect_preserved_cell_slot_locations(&callable);
    let preserved_slots = collect_preserved_slot_locations(&callable);
    let cell_bindings = collect_cell_bindings(&callable);
    let mut mapper = NameLocator {
        scope: &scope,
        current_block_param_names: HashSet::new(),
        local_slots,
        captured_cell_slots,
        owned_cell_slots,
        preserved_cell_slots,
        preserved_slots,
        cell_bindings,
    };
    mapper.map_callable(callable)
}

fn collect_make_function_callee_ids_in_expr(
    expr: &InstrUnresolved,
    out: &mut Vec<RuntimeFunctionId>,
) {
    match expr {
        InstrUnresolved::Literal(_) => {}
        InstrUnresolved::MakeFunction(op) => out.push(op.function_id),
        _ => {
            struct CalleeVisitor<'a> {
                out: &'a mut Vec<RuntimeFunctionId>,
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
    callable: &BlockPyFunction<CoreModuleShape>,
) -> Vec<RuntimeFunctionId> {
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

fn collect_make_function_callee_ids_in_stmt(stmt: &CoreStmt, out: &mut Vec<RuntimeFunctionId>) {
    collect_make_function_callee_ids_in_expr(stmt, out)
}

fn collect_make_function_callee_ids_in_term(
    term: &BlockTerm<InstrUnresolved>,
    out: &mut Vec<RuntimeFunctionId>,
) {
    struct CalleeVisitor<'a> {
        out: &'a mut Vec<RuntimeFunctionId>,
    }

    impl crate::block_py::Visit<InstrUnresolved> for CalleeVisitor<'_> {
        fn visit_instr(&mut self, expr: &InstrUnresolved) {
            collect_make_function_callee_ids_in_expr(expr, self.out);
        }
    }

    crate::block_py::walk_term(&mut CalleeVisitor { out }, term);
}

fn compute_callable_storage_layout_for_name_binding(
    function_id: RuntimeFunctionId,
    callable_by_id: &HashMap<RuntimeFunctionId, &BlockPyFunction<CoreModuleShape>>,
    make_function_callees: &HashMap<RuntimeFunctionId, Vec<RuntimeFunctionId>>,
    memo: &mut HashMap<RuntimeFunctionId, Option<StorageLayout>>,
    visiting: &mut HashSet<RuntimeFunctionId>,
) -> Option<StorageLayout> {
    if let Some(layout) = memo.get(&function_id) {
        return layout.clone();
    }
    let callable = callable_by_id
        .get(&function_id)
        .unwrap_or_else(|| panic!("missing callable for function id {:?}", function_id));
    let explicit_layout = callable.storage_layout.clone();
    if !visiting.insert(function_id) {
        return compute_storage_layout_from_scope(callable);
    }

    let base_layout = explicit_layout
        .clone()
        .or_else(|| compute_storage_layout_from_scope(callable));
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
    let param_name_set = callable.params.names().into_iter().collect::<HashSet<_>>();
    let mut local_cell_slots = base_layout
        .as_ref()
        .map(|layout| {
            layout
                .cellvars
                .iter()
                .map(|slot| slot.storage_name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            let mut storage_names = callable
                .scope
                .owned_cell_storage_names()
                .into_iter()
                .collect::<Vec<_>>();
            storage_names.sort();
            storage_names
        });
    let mut local_cell_slot_names = local_cell_slots.iter().cloned().collect::<HashSet<_>>();
    let mut base_cellvar_names = base_cellvar_names;
    let mut base_cellvar_storage_names = base_cellvar_storage_names;
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
                let capture_storage_name =
                    callable.scope.cell_storage_name(slot.logical_name.as_str());
                if (slot.logical_name == "__class__"
                    && base_cellvar_storage_names.contains("_dp_classcell"))
                    || base_cellvar_names.contains(slot.logical_name.as_str())
                    || base_cellvar_storage_names.contains(capture_source_name.as_str())
                {
                    continue;
                }
                let local_def_provides_capture = callable.scope.scope_kind
                    == CallableScopeKind::Function
                    && callable.scope.has_local_def(slot.logical_name.as_str());
                let type_param_provides_capture = callable
                    .scope
                    .type_param_names
                    .contains(slot.logical_name.as_str());
                if local_def_provides_capture
                    || type_param_provides_capture
                    || param_name_set.contains(slot.logical_name.as_str())
                {
                    let local_storage_name = if callable.scope.scope_kind
                        == CallableScopeKind::Function
                        && callable.scope.has_local_def(slot.logical_name.as_str())
                    {
                        capture_storage_name
                    } else {
                        capture_source_name
                    };
                    base_cellvar_names.insert(slot.logical_name.clone());
                    base_cellvar_storage_names.insert(local_storage_name.clone());
                    if local_cell_slot_names.insert(local_storage_name.clone()) {
                        local_cell_slots.push(local_storage_name);
                    }
                    continue;
                }
                capture_names.push(slot.logical_name.clone());
            }
        }
    }
    visiting.remove(&function_id);

    local_cell_slots.sort();
    local_cell_slots.dedup();
    let layout = if let Some(mut layout) = explicit_layout {
        for storage_name in local_cell_slots {
            if layout.has_storage_name(storage_name.as_str()) {
                continue;
            }
            let logical_name = callable
                .scope
                .logical_name_for_cell_storage(storage_name.as_str())
                .unwrap_or_else(|| storage_name.clone());
            let init = if param_name_set.contains(logical_name.as_str()) {
                ClosureInit::Parameter
            } else {
                ClosureInit::EmptyCell
            };
            layout.cellvars.push(ClosureSlot {
                logical_name,
                storage_name,
                init,
            });
        }
        capture_names.sort();
        capture_names.dedup();
        for logical_name in capture_names {
            if is_runtime_closure_name(logical_name.as_str()) {
                continue;
            }
            let storage_name = callable.scope.cell_storage_name(logical_name.as_str());
            if layout.has_storage_name(storage_name.as_str()) {
                continue;
            }
            layout.freevars.push(ClosureSlot {
                logical_name,
                storage_name,
                init: ClosureInit::InheritedCapture,
            });
        }
        Some(layout)
    } else {
        build_storage_layout_from_capture_names(
            callable,
            capture_names,
            &param_name_set,
            &local_cell_slots,
        )
    };
    memo.insert(function_id, layout.clone());
    layout
}

fn ensure_module_storage_layouts(
    callable_defs: Vec<BlockPyFunction<CoreModuleShape>>,
) -> Vec<BlockPyFunction<CoreModuleShape>> {
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
            if let Some(layout) = computed_layouts
                .get(&callable.function_id)
                .cloned()
                .flatten()
            {
                synchronize_generator_public_storage_layout(&mut callable, &layout);
                callable.storage_layout = Some(layout);
            }
            callable
        })
        .collect()
}

fn synchronize_generator_public_storage_layout(
    callable: &mut BlockPyFunction<CoreModuleShape>,
    body_layout: &StorageLayout,
) {
    if !matches!(
        callable.kind,
        FunctionKind::Generator | FunctionKind::Coroutine | FunctionKind::AsyncGenerator
    ) {
        return;
    }
    let Some(public_layout) = callable.public_storage_layout.as_mut() else {
        return;
    };
    let mut public_freevars = public_layout
        .freevars
        .iter()
        .map(|slot| slot.logical_name.clone())
        .collect::<HashSet<_>>();
    for slot in &body_layout.freevars {
        if public_freevars.insert(slot.logical_name.clone()) {
            public_layout.freevars.push(slot.clone());
        }
    }
}

fn compute_module_make_function_capture_names(
    callable_defs: &[BlockPyFunction<CoreModuleShape>],
) -> HashMap<RuntimeFunctionId, Vec<CellCaptureBinding>> {
    fn make_function_capture_source_name(scope: &CallableScopeInfo, logical_name: &str) -> String {
        if scope.binding_kind(logical_name).is_some()
            || scope
                .logical_name_for_cell_capture_source(logical_name)
                .is_some()
            || scope.logical_name_for_cell_storage(logical_name).is_some()
        {
            return scope.cell_capture_source_name(logical_name);
        }
        logical_name.to_string()
    }

    fn owned_cell_logical_names(callable: &BlockPyFunction<CoreModuleShape>) -> HashSet<String> {
        if let Some(layout) = callable.storage_layout.as_ref() {
            return layout
                .cellvars
                .iter()
                .map(|slot| slot.logical_name.clone())
                .collect();
        }
        callable
            .scope
            .bindings
            .iter()
            .filter_map(|(name, binding)| {
                matches!(binding, BindingKind::Cell(CellBindingKind::Owner)).then(|| name.clone())
            })
            .collect()
    }

    fn owned_cell_storage_names(callable: &BlockPyFunction<CoreModuleShape>) -> HashSet<String> {
        if let Some(layout) = callable.storage_layout.as_ref() {
            return layout
                .cellvars
                .iter()
                .map(|slot| slot.storage_name.clone())
                .collect();
        }
        callable.scope.owned_cell_storage_names()
    }

    fn compute_callable_make_function_capture_bindings_for_name_binding(
        function_id: RuntimeFunctionId,
        callable_by_id: &HashMap<RuntimeFunctionId, &BlockPyFunction<CoreModuleShape>>,
        make_function_callees: &HashMap<RuntimeFunctionId, Vec<RuntimeFunctionId>>,
        memo: &mut HashMap<RuntimeFunctionId, Vec<CellCaptureBinding>>,
        visiting: &mut HashSet<RuntimeFunctionId>,
    ) -> Vec<CellCaptureBinding> {
        if let Some(captures) = memo.get(&function_id) {
            return captures.clone();
        }

        let callable = callable_by_id
            .get(&function_id)
            .unwrap_or_else(|| panic!("missing callable for function id {:?}", function_id));
        let layout_capture_bindings = || {
            callable
                .public_storage_layout()
                .map(|layout| {
                    layout
                        .freevars
                        .iter()
                        .map(|slot| CellCaptureBinding {
                            logical_name: slot.logical_name.clone(),
                            source_name: make_function_capture_source_name(
                                callable.public_scope(),
                                slot.logical_name.as_str(),
                            ),
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
        let base_owned_logical_names = owned_cell_logical_names(callable);
        let base_owned_storage_names = owned_cell_storage_names(callable);

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
                    let requested_source_name = capture.source_name;
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
                    let source_name =
                        make_function_capture_source_name(&callable.scope, logical_name.as_str());
                    if (logical_name == "__class__"
                        && base_owned_storage_names.contains("_dp_classcell"))
                        || base_owned_logical_names.contains(logical_name.as_str())
                        || base_owned_storage_names.contains(source_name.as_str())
                        || (callable.scope.scope_kind == CallableScopeKind::Function
                            && callable.scope.has_local_def(logical_name.as_str()))
                        || callable
                            .scope
                            .type_param_names
                            .contains(logical_name.as_str())
                        || base_owned_storage_names.contains(requested_source_name.as_str())
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
                    .public_storage_layout()
                    .map(|layout| {
                        layout
                            .freevars
                            .iter()
                            .map(|slot| CellCaptureBinding {
                                logical_name: slot.logical_name.clone(),
                                source_name: make_function_capture_source_name(
                                    callable.public_scope(),
                                    slot.logical_name.as_str(),
                                ),
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
    callable: BlockPyFunction<ResolvedStorageModuleShape>,
) -> BlockPyFunction<ResolvedStorageModuleShape> {
    let BlockPyFunction {
        function_id,
        name_gen,
        names,
        kind,
        execution_mode,
        params,
        body_params,
        public_scope,
        blocks,
        doc,
        public_storage_layout,
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
                extra: Default::default(),
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
        execution_mode,
        params,
        body_params,
        public_scope,
        blocks,
        doc,
        public_storage_layout,
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
    mut callable: BlockPyFunction<CoreModuleShape>,
    callee_make_function_captures: &HashMap<
        crate::block_py::RuntimeFunctionId,
        Vec<CellCaptureBinding>,
    >,
) -> BlockPyFunction<ResolvedStorageModuleShape> {
    materialize_preserved_block_arg_sources(&mut callable);
    let scope = callable.scope.clone();
    let local_slots = collect_local_slot_locations(&callable);
    let mut mapper = NameBindingMapper {
        scope: &scope,
        callee_make_function_captures,
    };
    let mut lowered = mapper.map_fn(callable);
    prepend_owned_cell_init_preamble(&mut lowered);
    populate_stack_slots_in_storage_layout(&mut lowered, local_slots);
    rewrite_current_exception_in_core_blocks(&mut lowered.blocks);
    let normal_predecessors = normal_predecessor_exc_param_names(&lowered.blocks);
    let storage_layout = lowered.storage_layout.clone();
    for block in &mut lowered.blocks {
        let predecessor_exc_names = normal_predecessors
            .get(&block.label)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        sync_backed_block_params_in_block(
            block,
            predecessor_exc_names,
            &scope,
            storage_layout.as_ref(),
            &mapper,
        );
        for stmt in &mut block.body {
            rewrite_raw_cell_loads_in_stmt(stmt, &scope, &mapper);
        }
        rewrite_raw_cell_loads_in_term(&mut block.term, &scope, &mapper);
    }
    let mut lowered = normalize_stmt_ops_in_resolved_callable(refresh_bb_callable_block_params(
        locate_names_in_callable(lowered),
    ));
    ensure_storage_layout_covers_block_params(&mut lowered);
    lowered
}

fn normalize_stmt_ops_in_resolved_callable(
    callable: BlockPyFunction<ResolvedStorageModuleShape>,
) -> BlockPyFunction<ResolvedStorageModuleShape> {
    callable
}

#[derive(Default)]
struct ModuleConstantExtractor {
    constants: Vec<InstrResolved>,
}

impl ModuleConstantExtractor {
    fn extract_module(
        mut self,
        mut module: BlockPyModule<ResolvedStorageModuleShape>,
    ) -> BlockPyModule<ResolvedStorageModuleShape> {
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

    fn extract_function(&mut self, function: &mut BlockPyFunction<ResolvedStorageModuleShape>) {
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
            || matches!(
                expr,
                InstrResolved::Load(op)
                    if op.name.is_runtime_name()
                        && op.name.runtime_name_id() != Some(RuntimeName::Globals)
            )
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

#[derive(Default)]
struct DeclaredGlobalNameCollector {
    names: HashSet<String>,
}

impl DeclaredGlobalNameCollector {
    fn collect_module(
        mut self,
        module: &BlockPyModule<ResolvedStorageModuleShape>,
    ) -> HashSet<String> {
        for callable in &module.callable_defs {
            self.collect_callable_body_declarations(callable);
        }
        self.names
    }

    fn collect_callable_body_declarations(
        &mut self,
        callable: &BlockPyFunction<ResolvedStorageModuleShape>,
    ) {
        for block in &callable.blocks {
            for stmt in &block.body {
                self.visit_instr(stmt);
            }
            crate::block_py::walk_term(self, &block.term);
        }
    }

    fn collect_declared_name(&mut self, name: &ResolvedName) {
        if name.location.is_global_name() {
            self.names.insert(name.id.to_string());
        }
    }
}

impl Visit<InstrResolved> for DeclaredGlobalNameCollector {
    fn visit_instr(&mut self, expr: &InstrResolved) {
        match expr {
            InstrResolved::Store(op) => {
                self.collect_declared_name(&op.name);
                op.visit_children(self);
            }
            InstrResolved::Del(op) => {
                self.collect_declared_name(&op.name);
                op.visit_children(self);
            }
            _ => expr.visit_children(self),
        }
    }
}

struct UnsoundBuiltinRuntimeNameRewriter<'a> {
    declared_global_names: &'a HashSet<String>,
}

impl UnsoundBuiltinRuntimeNameRewriter<'_> {
    fn maybe_rewrite_name(&self, name: &mut ResolvedName) {
        if !name.location.is_global_name()
            || self.declared_global_names.contains(name.id.as_str())
            || !is_unsound_runtime_builtin_candidate(name.id.as_str())
        {
            return;
        }
        // BEHAVIOR_CHANGE: runtime builtins are loaded from SOAC runtime constants,
        // not by re-checking module globals for a later shadowing store.
        name.location = NameLocation::RuntimeName(
            RuntimeName::from_name(name.id.as_str())
                .expect("runtime builtin candidate should have a RuntimeName id"),
        );
    }
}

impl crate::block_py::VisitMut<InstrResolved> for UnsoundBuiltinRuntimeNameRewriter<'_> {
    fn visit_instr_mut(&mut self, expr: &mut InstrResolved) {
        match expr {
            InstrResolved::Load(op) => {
                self.maybe_rewrite_name(&mut op.name);
                op.visit_children_mut(self);
            }
            _ => expr.visit_children_mut(self),
        }
    }
}

fn rewrite_unsound_builtin_loads_as_runtime_names(
    mut module: BlockPyModule<ResolvedStorageModuleShape>,
) -> BlockPyModule<ResolvedStorageModuleShape> {
    let declared_global_names = DeclaredGlobalNameCollector::default().collect_module(&module);
    let mut rewriter = UnsoundBuiltinRuntimeNameRewriter {
        declared_global_names: &declared_global_names,
    };
    for callable in &mut module.callable_defs {
        for block in &mut callable.blocks {
            for stmt in &mut block.body {
                rewriter.visit_instr_mut(stmt);
            }
            crate::block_py::walk_term_mut(&mut rewriter, &mut block.term);
        }
    }
    module
}

struct RuntimeNameGlobalNameRewriter;

impl crate::block_py::VisitMut<InstrResolved> for RuntimeNameGlobalNameRewriter {
    fn visit_instr_mut(&mut self, expr: &mut InstrResolved) {
        if let InstrResolved::Load(op) = expr {
            if op.name.location.is_runtime_name()
                && op.name.runtime_name_id() != Some(RuntimeName::Globals)
            {
                op.name.location = NameLocation::global_name();
            }
        }
        expr.visit_children_mut(self);
    }
}

fn rewrite_runtime_name_loads_as_global_names(
    mut module: BlockPyModule<ResolvedStorageModuleShape>,
) -> BlockPyModule<ResolvedStorageModuleShape> {
    let mut rewriter = RuntimeNameGlobalNameRewriter;
    crate::block_py::walk_module_mut(&mut rewriter, &mut module);
    module
}

pub(crate) fn lower_name_binding_in_core_blockpy_module_with_unsound_runtime_builtins(
    module: BlockPyModule<CoreModuleShape>,
    unsound_runtime_builtin_names: bool,
    runtime_names_as_globals: bool,
) -> BlockPyModule<ResolvedStorageModuleShape> {
    let callable_defs = ensure_module_storage_layouts(module.callable_defs);
    let callee_make_function_capture_names =
        compute_module_make_function_capture_names(&callable_defs);
    let mut module = BlockPyModule {
        module_name_gen: module.module_name_gen,
        global_names: Vec::new(),
        callable_defs: callable_defs
            .into_iter()
            .map(|callable| {
                lower_name_binding_callable(callable, &callee_make_function_capture_names)
            })
            .collect(),
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    };
    if unsound_runtime_builtin_names {
        module = rewrite_unsound_builtin_loads_as_runtime_names(module);
    }
    if runtime_names_as_globals {
        module = rewrite_runtime_name_loads_as_global_names(module);
    }
    ModuleConstantExtractor::default().extract_module(module)
}

pub(crate) fn lower_name_binding_in_core_blockpy_module_with_options(
    module: BlockPyModule<CoreModuleShape>,
    runtime_names_as_globals: bool,
) -> BlockPyModule<ResolvedStorageModuleShape> {
    lower_name_binding_in_core_blockpy_module_with_unsound_runtime_builtins(
        module,
        true,
        runtime_names_as_globals,
    )
}
