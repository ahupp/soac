mod suspend_order;

use self::suspend_order::make_suspend_order_explicit_in_core_callable_def;
use crate::block_py::cfg::RelabelBlockTargets;
use crate::block_py::{
    compute_storage_layout_from_scope, core_call_expr_with_meta, core_runtime_name_expr_with_meta,
    core_runtime_positional_call_expr_with_meta, literal_expr, map_module_functions, BindingKind,
    BindingPurpose, BindingTarget, Block, BlockArg, BlockBuilder, BlockEdge, BlockLabel,
    BlockParam, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm, CallArgKeyword,
    CallArgPositional, CallableScopeInfo, CellBindingKind, ClosureInit, ClosureSlot, FunctionKind,
    FunctionName, FunctionNameGen, GetAttr, GetItem, Instr, InstrUnresolved, InstrWithConstantNone,
    InstrWithYield, Load, MakeFunction, Mappable, ModuleNameGen, NameLike, NumberLiteral,
    NumberLiteralValue, RuntimeFunctionId, ScopeExprNode, SetItem, StorageLayout, Store,
    StringLiteral, TermBranchTable, TermIf, TermRaise, TryMapFunction, TryMapInstr, TryMapTerm,
    Tuple, UnaryOp, UnaryOpKind, UnresolvedName, WithMeta,
};
use crate::block_py::{Param, ParamKind, ParamSpec};
use crate::passes::ast_to_ast::scope_helpers::is_internal_symbol;
use crate::passes::ruff_to_blockpy::{attach_exception_edges_to_blocks, lowered_exception_edges};
use crate::passes::{CoreModuleShape, CoreModuleShapeWithYield};
use ruff_python_ast::{self as ast};
use soac_macros::match_default;
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ResumeAbiParam {
    SelfValue,
    SendValue,
    ResumeExc,
    TransportSent,
}

impl ResumeAbiParam {
    fn name(self) -> &'static str {
        match self {
            Self::SelfValue => "_dp_self",
            Self::SendValue => "_dp_send_value",
            Self::ResumeExc => "_dp_resume_exc",
            Self::TransportSent => "_dp_transport_sent",
        }
    }
}

const GENERATOR_RESUME_ABI_PARAMS: [ResumeAbiParam; 3] = [
    ResumeAbiParam::SelfValue,
    ResumeAbiParam::SendValue,
    ResumeAbiParam::ResumeExc,
];

const ASYNC_GENERATOR_RESUME_ABI_PARAMS: [ResumeAbiParam; 4] = [
    ResumeAbiParam::SelfValue,
    ResumeAbiParam::SendValue,
    ResumeAbiParam::ResumeExc,
    ResumeAbiParam::TransportSent,
];

type LinearYieldStmt = InstrWithYield;
type LinearCoreStmt = InstrUnresolved;
type LinearYieldBlock = Block<LinearYieldStmt>;
type LinearCoreBlock = Block<LinearCoreStmt>;
type BlockPyBlock = LinearCoreBlock;

struct ErrOnYield;

fn try_lower_core_expr_without_yield_with_mapper<M>(
    expr: InstrWithYield,
    map: &mut M,
) -> Result<InstrUnresolved, InstrWithYield>
where
    M: TryMapInstr<InstrWithYield, InstrUnresolved, InstrWithYield>,
{
    match_default!(expr: crate::passes::InstrWithYield {
        InstrWithYield::Yield(node) => Err(node.into()),
        InstrWithYield::YieldFrom(node) => Err(node.into()),
        rest => Ok(rest.try_map_children(map)?.into()),
    })
}

impl TryMapInstr<InstrWithYield, InstrUnresolved, InstrWithYield> for ErrOnYield {
    fn try_map_instr(&mut self, expr: InstrWithYield) -> Result<InstrUnresolved, InstrWithYield> {
        try_lower_core_expr_without_yield_with_mapper(expr, self)
    }

    fn try_map_name(&mut self, name: UnresolvedName) -> Result<UnresolvedName, InstrWithYield> {
        Ok(name)
    }
}

fn resume_abi_params(kind: FunctionKind) -> &'static [ResumeAbiParam] {
    match kind {
        FunctionKind::Function => &[],
        FunctionKind::Coroutine | FunctionKind::Generator => &GENERATOR_RESUME_ABI_PARAMS,
        FunctionKind::AsyncGenerator => &ASYNC_GENERATOR_RESUME_ABI_PARAMS,
    }
}

fn generator_state_logical_name(scope: &CallableScopeInfo, name: &str) -> String {
    scope
        .logical_name_for_cell_storage(name)
        .unwrap_or_else(|| name.to_string())
}

fn generator_state_storage_name(scope: &CallableScopeInfo, name: &str) -> String {
    let logical_name = generator_state_logical_name(scope, name);
    scope.cell_storage_name(logical_name.as_str())
}

fn runtime_init(name: &str) -> Option<ClosureInit> {
    match name {
        "_dp_pc" => Some(ClosureInit::RuntimePcUnstarted),
        name if name.starts_with("_dp_try_abrupt_kind_") => {
            Some(ClosureInit::RuntimeAbruptKindFallthrough)
        }
        "_dp_yieldfrom" => Some(ClosureInit::RuntimeNone),
        "_dp_throw_context" => Some(ClosureInit::RuntimeNone),
        _ => None,
    }
}

pub(crate) fn build_blockpy_storage_layout(
    scope: &CallableScopeInfo,
    param_names: &[String],
    state_vars: &[String],
    capture_names: &[String],
    semantic_cell_storage_names: &HashSet<String>,
    injected_exception_names: &HashSet<String>,
) -> StorageLayout {
    let capture_names = capture_names.iter().cloned().collect::<HashSet<_>>();
    let mut seen_storage_names = HashSet::new();

    let mut freevars = Vec::new();
    let mut cellvars = Vec::new();
    let mut preserved_slots = Vec::new();

    for name in state_vars {
        let logical_name = generator_state_logical_name(scope, name.as_str());
        let storage_name = generator_state_storage_name(scope, name.as_str());
        if !seen_storage_names.insert(storage_name.clone()) {
            continue;
        }
        if let Some(init) = runtime_init(logical_name.as_str()) {
            preserved_slots.push(ClosureSlot {
                logical_name,
                storage_name,
                init,
            });
            continue;
        }
        if capture_names.contains(name.as_str())
            || capture_names.contains(logical_name.as_str())
            || capture_names.contains(storage_name.as_str())
        {
            freevars.push(ClosureSlot {
                logical_name,
                storage_name,
                init: ClosureInit::InheritedCapture,
            });
            continue;
        }
        let init = if injected_exception_names.contains(logical_name.as_str()) {
            ClosureInit::RuntimeNone
        } else if param_names.iter().any(|param| param == &logical_name) {
            ClosureInit::Parameter
        } else {
            ClosureInit::Deferred
        };
        let slots = if semantic_cell_storage_names.contains(storage_name.as_str()) {
            &mut cellvars
        } else {
            &mut preserved_slots
        };
        slots.push(ClosureSlot {
            logical_name,
            storage_name,
            init,
        });
    }

    StorageLayout {
        freevars,
        cellvars,
        preserved_slots,
        stack_slots: Vec::new(),
    }
}

fn unresolved_name(id: &str) -> UnresolvedName {
    ast::name::Name::new(id).into()
}

fn core_name(name: &str) -> InstrUnresolved {
    unresolved_load_expr(unresolved_name(name))
}

fn internal_store_stmt<E>(target: &str, value: E) -> E
where
    E: Instr<Name = UnresolvedName> + From<Store<E>>,
{
    unresolved_store_stmt(unresolved_name(target), value)
}

fn unresolved_store_stmt<E>(target: UnresolvedName, value: E) -> E
where
    E: Instr<Name = UnresolvedName> + From<Store<E>>,
{
    Store::new(target, Box::new(value)).into()
}

fn unresolved_load_expr<E>(name: UnresolvedName) -> E
where
    E: Instr<Name = UnresolvedName> + From<Load<E>>,
{
    Load::new(name).into()
}

fn collect_state_vars<E>(
    scope: &CallableScopeInfo,
    param_names: &[String],
    blocks: &[Block<E>],
) -> Vec<String>
where
    E: ScopeExprNode + Instr,
{
    let mut state = param_names.to_vec();
    for block in blocks {
        for param_name in block
            .exception_param()
            .into_iter()
            .chain(block.param_names())
        {
            if !state.iter().any(|existing| existing == param_name) {
                state.push(param_name.to_string());
            }
        }
        for stmt in &block.body {
            for name in assigned_names_in_linear_stmt(stmt) {
                if scope.binding_target_for_name(name.as_str(), BindingPurpose::Store)
                    != BindingTarget::Local
                {
                    continue;
                }
                if !state.iter().any(|existing| existing == &name) {
                    state.push(name);
                }
            }
        }
        for name in assigned_names_in_term(&block.term) {
            if scope.binding_target_for_name(name.as_str(), BindingPurpose::Store)
                != BindingTarget::Local
            {
                continue;
            }
            if !state.iter().any(|existing| existing == &name) {
                state.push(name);
            }
        }
    }
    state
}

fn assigned_names_in_linear_stmt<E>(stmt: &E) -> HashSet<String>
where
    E: ScopeExprNode + Instr,
{
    let mut names = HashSet::new();
    collect_named_expr_target_names(stmt, &mut names);
    names
}

fn assigned_names_in_term<E>(term: &BlockTerm<E>) -> HashSet<String>
where
    E: ScopeExprNode + Instr,
{
    struct AssignedNamesVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl<E> crate::block_py::Visit<E> for AssignedNamesVisitor<'_>
    where
        E: ScopeExprNode + Instr,
    {
        fn visit_instr(&mut self, expr: &E) {
            collect_named_expr_target_names(expr, self.names);
        }
    }
    let mut names = HashSet::new();
    crate::block_py::walk_term(&mut AssignedNamesVisitor { names: &mut names }, term);
    names
}

fn collect_named_expr_target_names<E>(expr: &E, names: &mut HashSet<String>)
where
    E: ScopeExprNode + Instr,
{
    struct NamedExprTargetVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl<E> crate::block_py::Visit<E> for NamedExprTargetVisitor<'_>
    where
        E: ScopeExprNode + Instr,
    {
        fn visit_instr(&mut self, expr: &E) {
            collect_named_expr_target_names(expr, self.names);
        }
    }

    expr.walk_root_defined_names(&mut |name| {
        names.insert(name.to_string());
    });
    expr.visit_children(&mut NamedExprTargetVisitor { names });
}

fn collect_deleted_names<E>(blocks: &[Block<E>]) -> HashSet<String>
where
    E: ScopeExprNode + Instr,
{
    struct DeletedNamesVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl<E> crate::block_py::Visit<E> for DeletedNamesVisitor<'_>
    where
        E: ScopeExprNode + Instr,
    {
        fn visit_instr(&mut self, expr: &E) {
            collect_deleted_expr_names(expr, self.names);
        }
    }

    let mut names = HashSet::new();
    for block in blocks {
        for stmt in &block.body {
            collect_deleted_expr_names(stmt, &mut names);
        }
        crate::block_py::walk_term(&mut DeletedNamesVisitor { names: &mut names }, &block.term);
    }
    names
}

fn collect_deleted_expr_names<E>(expr: &E, names: &mut HashSet<String>)
where
    E: ScopeExprNode + Instr,
{
    struct DeletedNamesVisitor<'a> {
        names: &'a mut HashSet<String>,
    }

    impl<E> crate::block_py::Visit<E> for DeletedNamesVisitor<'_>
    where
        E: ScopeExprNode + Instr,
    {
        fn visit_instr(&mut self, expr: &E) {
            collect_deleted_expr_names(expr, self.names);
        }
    }

    expr.walk_root_deleted_names(&mut |name| {
        names.insert(name.to_string());
    });
    expr.visit_children(&mut DeletedNamesVisitor { names });
}

fn core_literal_int(value: usize) -> InstrUnresolved {
    let text = value.to_string();
    literal_expr(
        NumberLiteral {
            value: NumberLiteralValue::Int(crate::block_py::IntLiteral::from_decimal(text)),
        },
        Default::default(),
    )
}

fn core_none() -> InstrUnresolved {
    InstrUnresolved::constant_none()
}

fn core_string_literal(value: &str) -> InstrUnresolved {
    literal_expr(
        StringLiteral {
            value: value.to_string(),
        },
        Default::default(),
    )
}

fn core_call(func_name: &str, args: Vec<InstrUnresolved>) -> InstrUnresolved {
    core_runtime_positional_call_expr_with_meta(
        func_name,
        ast::AtomicNodeIndex::default(),
        Default::default(),
        args,
    )
}

fn core_tuple(values: Vec<InstrUnresolved>) -> InstrUnresolved {
    Tuple::new(values).with_meta(Default::default()).into()
}

fn core_call_expr(
    func: InstrUnresolved,
    args: Vec<InstrUnresolved>,
    keywords: Vec<(&str, InstrUnresolved)>,
) -> InstrUnresolved {
    core_call_expr_with_meta(
        func,
        ast::AtomicNodeIndex::default(),
        Default::default(),
        args.into_iter()
            .map(CallArgPositional::Positional)
            .collect(),
        keywords
            .into_iter()
            .map(|(arg, value)| CallArgKeyword::Named {
                arg: arg.into(),
                value,
            })
            .collect(),
    )
}

fn core_runtime_attr(attr: &str) -> InstrUnresolved {
    core_runtime_name_expr_with_meta(attr, ast::AtomicNodeIndex::default(), Default::default())
}

fn core_get_attr(value: InstrUnresolved, attr: &str) -> InstrUnresolved {
    GetAttr::new(Box::new(value), Box::new(core_string_literal(attr))).into()
}

fn core_get_item(value: InstrUnresolved, index: InstrUnresolved) -> InstrUnresolved {
    GetItem::new(Box::new(value), Box::new(index)).into()
}

fn core_set_item_stmt(
    value: InstrUnresolved,
    index: InstrUnresolved,
    replacement: InstrUnresolved,
) -> InstrUnresolved {
    SetItem::new(Box::new(value), Box::new(index), Box::new(replacement)).into()
}

fn core_generator_code(async_gen: bool, name: &str, qualname: &str) -> InstrUnresolved {
    let template_attr = if async_gen {
        "code_template_async_gen"
    } else {
        "code_template_gen"
    };
    let replace = core_get_attr(
        core_get_attr(core_runtime_attr(template_attr), "__code__"),
        "replace",
    );
    core_call_expr(
        replace,
        Vec::new(),
        vec![
            ("co_name", core_string_literal(name)),
            ("co_qualname", core_string_literal(qualname)),
        ],
    )
}

fn core_make_function(
    function_id: RuntimeFunctionId,
    kind: FunctionKind,
    param_defaults: InstrUnresolved,
    annotate_fn: InstrUnresolved,
) -> InstrUnresolved {
    MakeFunction::new(
        function_id,
        kind,
        Box::new(param_defaults),
        Box::new(annotate_fn),
    )
    .into()
}

fn is_generator_like(kind: FunctionKind) -> bool {
    matches!(
        kind,
        FunctionKind::Generator | FunctionKind::Coroutine | FunctionKind::AsyncGenerator
    )
}

fn injected_exception_names<I: Instr>(blocks: &[Block<I>]) -> HashSet<String> {
    let mut names = HashSet::new();
    for block in blocks {
        if let Some(exc_param) = block.exception_param() {
            names.insert(exc_param.to_string());
        }
    }
    names
}

fn build_generator_storage_layout(
    callable: &BlockPyFunction<CoreModuleShapeWithYield>,
) -> StorageLayout {
    let param_names = callable.params.names();
    let semantic_layout = compute_storage_layout_from_scope(callable).unwrap_or(StorageLayout {
        freevars: Vec::new(),
        cellvars: Vec::new(),
        preserved_slots: Vec::new(),
        stack_slots: Vec::new(),
    });
    let capture_names = semantic_layout
        .freevars
        .iter()
        .map(|slot| slot.logical_name.clone())
        .collect::<Vec<_>>();
    let local_cell_slots = semantic_layout
        .cellvars
        .iter()
        .map(|slot| slot.storage_name.clone())
        .collect::<Vec<_>>();
    let mut semantic_cell_storage_names = local_cell_slots.iter().cloned().collect::<HashSet<_>>();
    for deleted_name in collect_deleted_names(&callable.blocks) {
        if callable
            .scope
            .binding_target_for_name(deleted_name.as_str(), BindingPurpose::Store)
            == BindingTarget::Local
        {
            // Preserved value tuples currently carry only Python objects. A local
            // that can be deleted needs to preserve the unbound state too, so keep
            // it cell-backed until preserved activation state can encode that.
            semantic_cell_storage_names.insert(generator_state_storage_name(
                &callable.scope,
                deleted_name.as_str(),
            ));
        }
    }

    let mut state_vars = collect_state_vars(&callable.scope, &param_names, &callable.blocks);
    for capture_name in &capture_names {
        if !state_vars.iter().any(|existing| existing == capture_name) {
            state_vars.push(capture_name.clone());
        }
    }
    for block in &callable.blocks {
        if let Some(exc_param) = block.exception_param() {
            if !state_vars.iter().any(|existing| existing == exc_param) {
                state_vars.push(exc_param.to_string());
            }
        }
    }
    for slot in local_cell_slots {
        let logical_name = callable
            .scope
            .logical_name_for_cell_storage(slot.as_str())
            .unwrap_or_else(|| slot.clone());
        if !state_vars.iter().any(|existing| existing == &logical_name) {
            state_vars.push(logical_name);
        }
    }
    for runtime_name in ["_dp_pc", "_dp_yieldfrom", "_dp_throw_context"] {
        if !state_vars.iter().any(|existing| existing == runtime_name) {
            state_vars.push(runtime_name.to_string());
        }
    }

    build_blockpy_storage_layout(
        &callable.scope,
        &param_names,
        &state_vars,
        &capture_names,
        &semantic_cell_storage_names,
        &injected_exception_names(&callable.blocks),
    )
}

fn resume_closure_state_order(layout: &StorageLayout) -> Vec<String> {
    let mut order = Vec::new();
    order.extend(layout.freevars.iter().map(|slot| slot.logical_name.clone()));
    order.extend(layout.cellvars.iter().map(|slot| slot.logical_name.clone()));
    order
}

fn preserved_slot_init_expr(slot: &ClosureSlot) -> InstrUnresolved {
    match slot.init {
        ClosureInit::InheritedCapture | ClosureInit::EmptyCell => {
            panic!("preserved slots should not use closure-only init {slot:?}")
        }
        ClosureInit::Parameter => core_name(slot.logical_name.as_str()),
        ClosureInit::RuntimePcUnstarted => core_literal_int(1),
        ClosureInit::RuntimeAbruptKindFallthrough => core_literal_int(0),
        ClosureInit::RuntimeNone | ClosureInit::Deferred => core_none(),
    }
}

fn preserved_slot_index(layout: &StorageLayout, logical_name: &str) -> usize {
    layout
        .preserved_slots
        .iter()
        .position(|slot| slot.logical_name == logical_name)
        .unwrap_or_else(|| panic!("missing preserved slot {logical_name} in {layout:?}"))
}

fn preserved_values_expr() -> InstrUnresolved {
    core_get_attr(core_name("_dp_self"), "_preserved_values")
}

fn preserved_slot_reload_stmts(preserved_slots: &[ClosureSlot]) -> Vec<LinearCoreStmt> {
    preserved_slots
        .iter()
        .enumerate()
        .map(|(slot, preserved)| {
            internal_store_stmt(
                preserved.logical_name.as_str(),
                core_get_item(preserved_values_expr(), core_literal_int(slot)),
            )
        })
        .collect()
}

fn preserved_slot_spill_stmts(preserved_slots: &[ClosureSlot]) -> Vec<LinearCoreStmt> {
    preserved_slots
        .iter()
        .enumerate()
        .map(|(slot, preserved)| {
            core_set_item_stmt(
                preserved_values_expr(),
                core_literal_int(slot),
                core_name(preserved.logical_name.as_str()),
            )
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResumeClosureBindings {
    runtime_state_bindings: Vec<(String, String)>,
}

impl ResumeClosureBindings {}

fn resume_state_uses_standard_name_binding(name: &str) -> bool {
    !name.starts_with("_dp_cell_")
}

fn augment_resume_semantic_for_standard_name_binding(
    scope: &mut CallableScopeInfo,
    closure_bindings: &ResumeClosureBindings,
) {
    for (name, source_name) in &closure_bindings.runtime_state_bindings {
        if resume_state_uses_standard_name_binding(name.as_str()) {
            scope.insert_binding_with_cell_names(
                name.clone(),
                BindingKind::Cell(CellBindingKind::Capture),
                is_internal_symbol(name.as_str()),
                Some(name.clone()),
                Some(source_name.clone()),
            );
        }
    }
}

fn resume_closure_bindings(
    scope: &CallableScopeInfo,
    persistent_logical_names: &[String],
) -> ResumeClosureBindings {
    let runtime_state_bindings = persistent_logical_names
        .iter()
        .map(|logical_name| {
            (
                logical_name.clone(),
                scope.cell_capture_source_name(logical_name.as_str()),
            )
        })
        .collect::<Vec<_>>();
    ResumeClosureBindings {
        runtime_state_bindings,
    }
}

fn generator_resume_declared_params(kind: FunctionKind, params: &[BlockParam]) -> Vec<BlockParam> {
    let kept_indices = generator_resume_declared_param_indices(kind, params);
    params
        .iter()
        .enumerate()
        .filter(|(index, _)| kept_indices.contains(index))
        .map(|(_, param)| param.clone())
        .collect()
}

fn generator_resume_declared_param_indices(
    kind: FunctionKind,
    params: &[BlockParam],
) -> Vec<usize> {
    let resume_abi_names = resume_abi_params(kind)
        .iter()
        .map(|param| param.name())
        .collect::<HashSet<_>>();
    params
        .iter()
        .enumerate()
        .filter(|(_, param)| {
            param.role == BlockParamRole::Exception
                || param.role == BlockParamRole::AbruptKind
                || param.role == BlockParamRole::AbruptPayload
                || resume_abi_names.contains(param.name.as_str())
        })
        .map(|(index, _)| index)
        .collect()
}

fn build_factory_block(
    visible_names: &FunctionName,
    resume_function_id: RuntimeFunctionId,
    kind: FunctionKind,
    storage_layout: &StorageLayout,
) -> LinearCoreBlock {
    let resume_entry = core_make_function(
        resume_function_id,
        FunctionKind::Function,
        core_tuple(Vec::new()),
        core_none(),
    );
    let preserved_values = core_tuple(
        storage_layout
            .preserved_slots
            .iter()
            .map(preserved_slot_init_expr)
            .collect(),
    );
    let yieldfrom_slot = core_literal_int(preserved_slot_index(storage_layout, "_dp_yieldfrom"));
    let throw_context_slot =
        core_literal_int(preserved_slot_index(storage_layout, "_dp_throw_context"));
    let generator = match kind {
        FunctionKind::Generator | FunctionKind::Coroutine => core_call_expr(
            core_runtime_attr("ClosureGenerator"),
            vec![
                resume_entry,
                core_string_literal(visible_names.display_name.as_str()),
                core_string_literal(visible_names.qualname.as_str()),
                core_generator_code(
                    false,
                    visible_names.display_name.as_str(),
                    visible_names.qualname.as_str(),
                ),
                preserved_values.clone(),
                yieldfrom_slot.clone(),
                throw_context_slot.clone(),
            ],
            Vec::new(),
        ),
        FunctionKind::AsyncGenerator => core_call_expr(
            core_runtime_attr("ClosureAsyncGenerator"),
            vec![
                resume_entry,
                core_string_literal(visible_names.display_name.as_str()),
                core_string_literal(visible_names.qualname.as_str()),
                core_generator_code(
                    true,
                    visible_names.display_name.as_str(),
                    visible_names.qualname.as_str(),
                ),
                preserved_values,
                yieldfrom_slot,
                throw_context_slot,
            ],
            Vec::new(),
        ),
        FunctionKind::Function => {
            unreachable!("plain functions do not use generator factories")
        }
    };
    let factory_value = match kind {
        FunctionKind::Coroutine => {
            core_call_expr(core_runtime_attr("Coroutine"), vec![generator], Vec::new())
        }
        FunctionKind::Generator | FunctionKind::AsyncGenerator => generator,
        FunctionKind::Function => {
            unreachable!("plain functions do not use generator factories")
        }
    };

    Block::from_builder(
        BlockLabel::from_index(0),
        BlockBuilder::with_term(Vec::new(), Some(BlockTerm::Return(factory_value))),
        Vec::new(),
        None,
        None,
    )
}

fn resume_param_spec(kind: FunctionKind) -> ParamSpec {
    ParamSpec {
        params: resume_abi_params(kind)
            .iter()
            .map(|param| Param {
                name: param.name().to_string(),
                kind: ParamKind::PosOnly,
                has_default: false,
            })
            .collect(),
    }
}

#[derive(Clone)]
enum YieldSite {
    ExprYield(InstrWithYield),
    AssignYield {
        target: UnresolvedName,
        value: InstrWithYield,
    },
    ReturnYield(InstrWithYield),
    ExprYieldFrom(InstrWithYield),
    AssignYieldFrom {
        target: UnresolvedName,
        value: InstrWithYield,
    },
    ReturnYieldFrom(InstrWithYield),
}

fn stmt_yield_site(stmt: &LinearYieldStmt) -> Option<YieldSite> {
    match stmt {
        InstrWithYield::Yield(yield_expr) => {
            Some(YieldSite::ExprYield(yield_expr.value.as_ref().clone()))
        }
        InstrWithYield::YieldFrom(yield_from) => {
            Some(YieldSite::ExprYieldFrom((*yield_from.value).clone()))
        }
        InstrWithYield::Store(store) => match store.value.as_ref() {
            InstrWithYield::Yield(yield_expr) => Some(YieldSite::AssignYield {
                target: store.name.clone(),
                value: yield_expr.value.as_ref().clone(),
            }),
            InstrWithYield::YieldFrom(yield_from) => Some(YieldSite::AssignYieldFrom {
                target: store.name.clone(),
                value: (*yield_from.value).clone(),
            }),
            _ => None,
        },
        _ => None,
    }
}

fn term_yield_site(term: &BlockTerm<InstrWithYield>) -> Option<YieldSite> {
    match term {
        BlockTerm::Return(InstrWithYield::Yield(yield_expr)) => {
            Some(YieldSite::ReturnYield(yield_expr.value.as_ref().clone()))
        }
        BlockTerm::Return(InstrWithYield::YieldFrom(yield_from)) => {
            Some(YieldSite::ReturnYieldFrom((*yield_from.value).clone()))
        }
        _ => None,
    }
}

fn lower_stmt_no_yield(stmt: LinearYieldStmt) -> LinearCoreStmt {
    let mut mapper = ErrOnYield;
    mapper.try_map_instr(stmt.clone()).unwrap_or_else(|_| {
            panic!(
                "generator lowering expected yield-like sites to be split before stmt conversion: {stmt:?}"
            )
        })
}

fn lower_term_no_yield(term: BlockTerm<InstrWithYield>) -> BlockTerm<InstrUnresolved> {
    let mut mapper = ErrOnYield;
    mapper.try_map_term(term.clone()).unwrap_or_else(|_| {
        panic!(
            "generator lowering expected yield-like sites to be split before term conversion: {term:?}"
        )
    })
}

fn yield_value_expr(value: InstrWithYield) -> InstrUnresolved {
    ErrOnYield
        .try_map_instr(value)
        .unwrap_or_else(|_| panic!("yield payload unexpectedly contained nested yield"))
}

fn completion_raise(
    kind: FunctionKind,
    value: Option<InstrUnresolved>,
) -> BlockTerm<InstrUnresolved> {
    match kind {
        FunctionKind::Generator | FunctionKind::Coroutine => {
            let exc = if let Some(value) = value {
                core_call("StopIteration", vec![value])
            } else {
                core_call("StopIteration", Vec::new())
            };
            BlockTerm::Raise(TermRaise { exc: Some(exc) })
        }
        FunctionKind::AsyncGenerator => BlockTerm::Raise(TermRaise {
            exc: Some(core_call("AsyncGenComplete", Vec::new())),
        }),
        FunctionKind::Function => unreachable!(),
    }
}

fn push_completion_raise_block(
    state: &mut ResumeLoweringState,
    label: BlockLabel,
    body: Vec<LinearCoreStmt>,
    value: Option<InstrUnresolved>,
    params: Vec<BlockParam>,
    exc_target: Option<BlockLabel>,
) {
    let completion_label = state.fresh_label("resume_complete");
    state.push_block(
        BlockPyBlock {
            label,
            body,
            term: BlockTerm::Jump(BlockEdge::new(completion_label.clone())),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target,
    );
    state.push_block(
        BlockPyBlock {
            label: completion_label,
            body: Vec::new(),
            term: completion_raise(state.kind, value),
            params,
            exc_edge: None,
            extra: Default::default(),
        },
        None,
    );
}

fn explicit_jump_args_for_params(params: &[BlockParam]) -> Vec<BlockArg> {
    params
        .iter()
        .map(|param| BlockArg::Name(param.name.clone()))
        .collect()
}

fn resume_dispatch_jump_args_for_params(params: &[BlockParam]) -> Vec<BlockArg> {
    params
        .iter()
        .map(|param| match param.role {
            BlockParamRole::Exception => BlockArg::None,
            BlockParamRole::Value | BlockParamRole::AbruptKind | BlockParamRole::AbruptPayload => {
                BlockArg::Name(param.name.clone())
            }
        })
        .collect()
}

fn is_resume_exc_test() -> InstrUnresolved {
    crate::block_py::UnaryOp::new(
        crate::block_py::UnaryOpKind::Not,
        Box::new(
            crate::block_py::BinOp::new(
                crate::block_py::BinOpKind::Is,
                Box::new(core_name("_dp_resume_exc")),
                Box::new(core_runtime_name_expr_with_meta(
                    "NO_DEFAULT",
                    ast::AtomicNodeIndex::default(),
                    Default::default(),
                )),
            )
            .into(),
        ),
    )
    .into()
}

fn is_send_none_test() -> InstrUnresolved {
    crate::block_py::BinOp::new(
        crate::block_py::BinOpKind::Is,
        Box::new(core_name("_dp_send_value")),
        Box::new(core_none()),
    )
    .into()
}

fn is_name_none_test(name: &str) -> InstrUnresolved {
    crate::block_py::BinOp::new(
        crate::block_py::BinOpKind::Is,
        Box::new(core_name(name)),
        Box::new(core_none()),
    )
    .into()
}

fn is_name_not_none_test(name: &str) -> InstrUnresolved {
    UnaryOp::new(UnaryOpKind::Not, Box::new(is_name_none_test(name))).into()
}

fn is_resume_generator_exit_test() -> InstrUnresolved {
    core_call(
        "isinstance",
        vec![
            core_name("_dp_resume_exc"),
            core_runtime_attr("GeneratorExit"),
        ],
    )
}

fn resume_exc_raise_term() -> BlockTerm<InstrUnresolved> {
    BlockTerm::Raise(TermRaise {
        exc: Some(core_name("_dp_resume_exc")),
    })
}

fn stop_iteration_match_test(exc_name: &str) -> InstrUnresolved {
    core_call_expr(
        core_runtime_attr("exception_matches"),
        vec![core_name(exc_name), core_runtime_attr("StopIteration")],
        Vec::new(),
    )
}

fn current_exception_value_expr(exc_name: &str) -> InstrUnresolved {
    core_get_attr(core_name(exc_name), "value")
}

fn yield_from_next_expr() -> InstrUnresolved {
    core_call("next", vec![core_name("_dp_yieldfrom")])
}

fn yield_from_send_expr() -> InstrUnresolved {
    core_call_expr(
        core_get_attr(core_name("_dp_yieldfrom"), "send"),
        vec![core_name("_dp_send_value")],
        Vec::new(),
    )
}

fn yield_from_method_lookup_expr(method: &str) -> InstrUnresolved {
    core_call(
        "getattr",
        vec![
            core_name("_dp_yieldfrom"),
            core_string_literal(method),
            core_none(),
        ],
    )
}

fn no_arg_name_call_expr(name: &str) -> InstrUnresolved {
    core_call_expr(core_name(name), Vec::new(), Vec::new())
}

fn single_arg_name_call_expr(name: &str, arg: InstrUnresolved) -> InstrUnresolved {
    core_call_expr(core_name(name), vec![arg], Vec::new())
}

struct ResumeLoweringState {
    kind: FunctionKind,
    name_gen: FunctionNameGen,
    next_resume_pc: usize,
    blocks: Vec<LinearCoreBlock>,
    exception_edges: HashMap<BlockLabel, Option<BlockLabel>>,
    target_arg_indices: HashMap<BlockLabel, Vec<usize>>,
    resume_targets: Vec<(usize, BlockLabel)>,
    exhausted_label: BlockLabel,
}

impl ResumeLoweringState {
    fn new(
        name_gen: FunctionNameGen,
        kind: FunctionKind,
        target_arg_indices: HashMap<BlockLabel, Vec<usize>>,
    ) -> Self {
        let exhausted_label = name_gen.next_block_name();
        Self {
            kind,
            name_gen,
            next_resume_pc: 2,
            blocks: Vec::new(),
            exception_edges: HashMap::new(),
            target_arg_indices,
            resume_targets: Vec::new(),
            exhausted_label,
        }
    }

    fn fresh_label(&mut self, base: &str) -> BlockLabel {
        let _ = base;
        self.name_gen.next_block_name()
    }

    fn fresh_resume_target(&mut self, base: &str) -> (usize, BlockLabel) {
        let pc = self.next_resume_pc;
        self.next_resume_pc += 1;
        let label = self.fresh_label(base);
        self.resume_targets.push((pc, label.clone()));
        (pc, label)
    }

    fn fresh_temp(&mut self, base: &str) -> String {
        self.name_gen.next_tmp_name(base).to_string()
    }

    fn push_block(&mut self, mut block: LinearCoreBlock, exc_target: Option<BlockLabel>) {
        let active_exception = block
            .params
            .iter()
            .find(|param| param.role == BlockParamRole::Exception)
            .map(|param| core_name(param.name.as_str()))
            .unwrap_or_else(core_none);
        block.body.insert(
            0,
            internal_store_stmt("_dp_throw_context", active_exception),
        );
        self.exception_edges.insert(block.label.clone(), exc_target);
        self.blocks.push(block);
    }

    fn prune_term_target_args(&self, term: &mut BlockTerm<InstrUnresolved>) {
        let BlockTerm::Jump(edge) = term else {
            return;
        };
        let Some(indices) = self.target_arg_indices.get(&edge.target) else {
            return;
        };
        if edge.args.is_empty() {
            return;
        }
        edge.args = indices
            .iter()
            .filter_map(|index| edge.args.get(*index).cloned())
            .collect();
    }
}

fn lower_resume_fragment(
    state: &mut ResumeLoweringState,
    label: BlockLabel,
    body: Vec<LinearYieldStmt>,
    term: BlockTerm<InstrWithYield>,
    params: Vec<BlockParam>,
    exc_target: Option<BlockLabel>,
) {
    for (index, stmt) in body.iter().enumerate() {
        if let Some(site) = stmt_yield_site(stmt) {
            let mut prefix = body[..index]
                .iter()
                .cloned()
                .map(lower_stmt_no_yield)
                .collect::<Vec<_>>();
            emit_yield_site(
                state,
                label,
                &mut prefix,
                site,
                body[index + 1..].to_vec(),
                term,
                params,
                exc_target,
            );
            return;
        }
    }
    if let Some(site) = term_yield_site(&term) {
        let mut prefix = body
            .into_iter()
            .map(lower_stmt_no_yield)
            .collect::<Vec<_>>();
        emit_yield_site(
            state,
            label,
            &mut prefix,
            site,
            Vec::new(),
            BlockTerm::Return(InstrWithYield::constant_none()),
            params,
            exc_target,
        );
        return;
    }

    let lowered_body = body
        .into_iter()
        .map(lower_stmt_no_yield)
        .collect::<Vec<_>>();
    match term {
        BlockTerm::Return(value) => {
            push_completion_raise_block(
                state,
                label,
                lowered_body,
                Some(ErrOnYield.try_map_instr(value).unwrap_or_else(|_| {
                    panic!("generator lowering expected yield-free final return value")
                })),
                params,
                exc_target,
            );
        }
        other => {
            let mut lowered_term = lower_term_no_yield(other);
            state.prune_term_target_args(&mut lowered_term);
            state.push_block(
                BlockPyBlock {
                    label,
                    body: lowered_body,
                    term: lowered_term,
                    params,
                    exc_edge: None,
                    extra: Default::default(),
                },
                exc_target,
            );
        }
    }
}

fn emit_yield_site(
    state: &mut ResumeLoweringState,
    label: BlockLabel,
    prefix: &mut Vec<LinearCoreStmt>,
    site: YieldSite,
    tail_body: Vec<LinearYieldStmt>,
    tail_term: BlockTerm<InstrWithYield>,
    params: Vec<BlockParam>,
    exc_target: Option<BlockLabel>,
) {
    match site {
        YieldSite::ExprYield(value) => {
            let (resume_pc, resume_label) = state.fresh_resume_target("yield_resume");
            prefix.push(internal_store_stmt("_dp_pc", core_literal_int(resume_pc)));
            prefix.push(internal_store_stmt("_dp_yieldfrom", core_none()));
            state.push_block(
                BlockPyBlock {
                    label,
                    body: std::mem::take(prefix),
                    term: BlockTerm::Return(yield_value_expr(value)),
                    params: params.clone(),
                    exc_edge: None,
                    extra: Default::default(),
                },
                exc_target.clone(),
            );
            emit_resume_after_yield(
                state,
                resume_label,
                None,
                tail_body,
                tail_term,
                params,
                exc_target,
            );
        }
        YieldSite::AssignYield { target, value } => {
            let (resume_pc, resume_label) = state.fresh_resume_target("yield_resume");
            prefix.push(internal_store_stmt("_dp_pc", core_literal_int(resume_pc)));
            prefix.push(internal_store_stmt("_dp_yieldfrom", core_none()));
            state.push_block(
                BlockPyBlock {
                    label,
                    body: std::mem::take(prefix),
                    term: BlockTerm::Return(yield_value_expr(value)),
                    params: params.clone(),
                    exc_edge: None,
                    extra: Default::default(),
                },
                exc_target.clone(),
            );
            emit_resume_after_yield(
                state,
                resume_label,
                Some(target),
                tail_body,
                tail_term,
                params,
                exc_target,
            );
        }
        YieldSite::ReturnYield(value) => {
            let (resume_pc, resume_label) = state.fresh_resume_target("yield_return_resume");
            prefix.push(internal_store_stmt("_dp_pc", core_literal_int(resume_pc)));
            prefix.push(internal_store_stmt("_dp_yieldfrom", core_none()));
            state.push_block(
                BlockPyBlock {
                    label,
                    body: std::mem::take(prefix),
                    term: BlockTerm::Return(yield_value_expr(value)),
                    params: params.clone(),
                    exc_edge: None,
                    extra: Default::default(),
                },
                exc_target.clone(),
            );
            emit_resume_after_yield(
                state,
                resume_label,
                None,
                Vec::new(),
                BlockTerm::Return(unresolved_load_expr(unresolved_name("_dp_send_value"))),
                params,
                exc_target,
            );
        }
        YieldSite::ExprYieldFrom(value) => emit_yield_from_site(
            state, label, prefix, value, None, tail_body, tail_term, params, exc_target,
        ),
        YieldSite::AssignYieldFrom { target, value } => emit_yield_from_site(
            state,
            label,
            prefix,
            value,
            Some(target),
            tail_body,
            tail_term,
            params,
            exc_target,
        ),
        YieldSite::ReturnYieldFrom(value) => emit_yield_from_site(
            state,
            label,
            prefix,
            value,
            None,
            Vec::new(),
            BlockTerm::Return(unresolved_load_expr(unresolved_name(
                "_dp_yield_from_value",
            ))),
            params,
            exc_target,
        ),
    }
}

fn emit_resume_after_yield(
    state: &mut ResumeLoweringState,
    resume_label: BlockLabel,
    assign_target: Option<UnresolvedName>,
    mut tail_body: Vec<LinearYieldStmt>,
    tail_term: BlockTerm<InstrWithYield>,
    params: Vec<BlockParam>,
    exc_target: Option<BlockLabel>,
) {
    let raise_label = state.fresh_label("yield_throw");
    let continue_label = state.fresh_label("yield_continue");
    state.push_block(
        BlockPyBlock {
            label: resume_label,
            body: Vec::new(),
            term: BlockTerm::IfTerm(TermIf {
                test: is_resume_exc_test(),
                then_label: raise_label.clone(),
                else_label: continue_label.clone(),
            }),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: raise_label,
            body: Vec::new(),
            term: resume_exc_raise_term(),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    if let Some(target) = assign_target {
        tail_body.insert(
            0,
            unresolved_store_stmt(
                target,
                unresolved_load_expr(unresolved_name("_dp_send_value")),
            ),
        );
    }
    lower_resume_fragment(
        state,
        continue_label,
        tail_body,
        tail_term,
        params,
        exc_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_yield_from_site(
    state: &mut ResumeLoweringState,
    label: BlockLabel,
    prefix: &mut Vec<LinearCoreStmt>,
    value: InstrWithYield,
    assign_target: Option<UnresolvedName>,
    mut tail_body: Vec<LinearYieldStmt>,
    tail_term: BlockTerm<InstrWithYield>,
    params: Vec<BlockParam>,
    exc_target: Option<BlockLabel>,
) {
    let (delegate_pc, delegate_label) = state.fresh_resume_target("yield_from");
    let send_dispatch_label = state.fresh_label("yield_from_send_dispatch");
    let exc_dispatch_label = state.fresh_label("yield_from_exc_dispatch");
    let next_call_label = state.fresh_label("yield_from_next");
    let send_call_label = state.fresh_label("yield_from_send");
    let throw_lookup_label = state.fresh_label("yield_from_throw_lookup");
    let throw_call_label = state.fresh_label("yield_from_throw");
    let close_lookup_label = state.fresh_label("yield_from_close_lookup");
    let close_call_label = state.fresh_label("yield_from_close");
    let raise_resume_exc_label = state.fresh_label("yield_from_reraise");
    let call_except_label = state.fresh_label("yield_from_except");
    let stopiter_label = state.fresh_label("yield_from_stopiter");
    let non_stopiter_label = state.fresh_label("yield_from_non_stopiter");
    let value_expr = ErrOnYield
        .try_map_instr(value)
        .unwrap_or_else(|_| panic!("yield from payload unexpectedly contained nested yield"));
    let yielded_value_name = state.fresh_temp("yield_from_value");
    let throw_name = state.fresh_temp("yield_from_throw");
    let close_name = state.fresh_temp("yield_from_close");
    let caught_exc_name = state.fresh_temp("yield_from_exc");
    prefix.push(internal_store_stmt(
        "_dp_yieldfrom",
        core_call("iter", vec![value_expr]),
    ));
    prefix.push(internal_store_stmt("_dp_pc", core_literal_int(delegate_pc)));
    state.push_block(
        BlockPyBlock {
            label,
            body: std::mem::take(prefix),
            term: BlockTerm::Jump(BlockEdge::new(delegate_label.clone())),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );

    let yielded_label = state.fresh_label("yield_from_yielded");
    let done_label = state.fresh_label("yield_from_done");
    state.push_block(
        BlockPyBlock {
            label: delegate_label.clone(),
            body: Vec::new(),
            term: BlockTerm::IfTerm(TermIf {
                test: is_resume_exc_test(),
                then_label: exc_dispatch_label.clone(),
                else_label: send_dispatch_label.clone(),
            }),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: send_dispatch_label,
            body: Vec::new(),
            term: BlockTerm::IfTerm(TermIf {
                test: is_send_none_test(),
                then_label: next_call_label.clone(),
                else_label: send_call_label.clone(),
            }),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: next_call_label,
            body: vec![internal_store_stmt(
                yielded_value_name.as_str(),
                yield_from_next_expr(),
            )],
            term: BlockTerm::Jump(BlockEdge::new(yielded_label.clone())),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        Some(call_except_label.clone()),
    );
    state.push_block(
        BlockPyBlock {
            label: send_call_label,
            body: vec![internal_store_stmt(
                yielded_value_name.as_str(),
                yield_from_send_expr(),
            )],
            term: BlockTerm::Jump(BlockEdge::new(yielded_label.clone())),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        Some(call_except_label.clone()),
    );
    state.push_block(
        BlockPyBlock {
            label: exc_dispatch_label,
            body: Vec::new(),
            term: BlockTerm::IfTerm(TermIf {
                test: is_resume_generator_exit_test(),
                then_label: close_lookup_label.clone(),
                else_label: throw_lookup_label.clone(),
            }),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: close_lookup_label,
            body: vec![internal_store_stmt(
                close_name.as_str(),
                yield_from_method_lookup_expr("close"),
            )],
            term: BlockTerm::IfTerm(TermIf {
                test: is_name_not_none_test(close_name.as_str()),
                then_label: close_call_label.clone(),
                else_label: raise_resume_exc_label.clone(),
            }),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: close_call_label,
            body: vec![no_arg_name_call_expr(close_name.as_str())],
            term: BlockTerm::Jump(BlockEdge::new(raise_resume_exc_label.clone())),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: throw_lookup_label,
            body: vec![internal_store_stmt(
                throw_name.as_str(),
                yield_from_method_lookup_expr("throw"),
            )],
            term: BlockTerm::IfTerm(TermIf {
                test: is_name_none_test(throw_name.as_str()),
                then_label: raise_resume_exc_label.clone(),
                else_label: throw_call_label.clone(),
            }),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: throw_call_label,
            body: vec![internal_store_stmt(
                yielded_value_name.as_str(),
                single_arg_name_call_expr(throw_name.as_str(), core_name("_dp_resume_exc")),
            )],
            term: BlockTerm::Jump(BlockEdge::new(yielded_label.clone())),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        Some(call_except_label.clone()),
    );
    let mut except_params = params.clone();
    except_params
        .retain(|param| param.role != BlockParamRole::Exception || param.name == caught_exc_name);
    if let Some(existing) = except_params
        .iter_mut()
        .find(|param| param.name == caught_exc_name)
    {
        existing.role = BlockParamRole::Exception;
    } else {
        except_params.push(BlockParam {
            name: caught_exc_name.clone(),
            role: BlockParamRole::Exception,
        });
    }
    state.push_block(
        BlockPyBlock {
            label: call_except_label.clone(),
            body: Vec::new(),
            term: BlockTerm::IfTerm(TermIf {
                test: stop_iteration_match_test(caught_exc_name.as_str()),
                then_label: stopiter_label.clone(),
                else_label: non_stopiter_label.clone(),
            }),
            params: except_params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: stopiter_label,
            body: vec![internal_store_stmt(
                yielded_value_name.as_str(),
                current_exception_value_expr(caught_exc_name.as_str()),
            )],
            term: BlockTerm::Jump(BlockEdge::with_args(
                done_label.clone(),
                explicit_jump_args_for_params(&params),
            )),
            params: except_params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: non_stopiter_label,
            body: vec![internal_store_stmt(
                "_dp_yieldfrom",
                InstrUnresolved::constant_none(),
            )],
            term: BlockTerm::Raise(TermRaise {
                exc: Some(core_name(caught_exc_name.as_str())),
            }),
            params: except_params,
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: yielded_label,
            body: vec![internal_store_stmt("_dp_pc", core_literal_int(delegate_pc))],
            term: BlockTerm::Return(core_name(yielded_value_name.as_str())),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );
    state.push_block(
        BlockPyBlock {
            label: raise_resume_exc_label,
            body: Vec::new(),
            term: resume_exc_raise_term(),
            params: params.clone(),
            exc_edge: None,
            extra: Default::default(),
        },
        exc_target.clone(),
    );

    tail_body.insert(
        0,
        internal_store_stmt("_dp_yieldfrom", InstrWithYield::constant_none()),
    );
    if let Some(target) = assign_target {
        tail_body.insert(
            1,
            unresolved_store_stmt(
                target,
                unresolved_load_expr(unresolved_name(yielded_value_name.as_str())),
            ),
        );
    } else if matches!(tail_term, BlockTerm::Return(InstrWithYield::Load(ref op)) if op.name.id_str() == "_dp_yield_from_value")
    {
        tail_body.insert(
            1,
            internal_store_stmt(
                "_dp_yield_from_value",
                unresolved_load_expr(unresolved_name(yielded_value_name.as_str())),
            ),
        );
    }
    lower_resume_fragment(state, done_label, tail_body, tail_term, params, exc_target);
}

fn lower_resume_blocks(
    callable: &BlockPyFunction<CoreModuleShapeWithYield>,
    resume_name_gen: FunctionNameGen,
    preserved_slots: &[ClosureSlot],
) -> (
    Vec<LinearCoreBlock>,
    HashMap<BlockLabel, Option<BlockLabel>>,
    BlockLabel,
) {
    let labels = callable
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<std::collections::HashSet<_>>();
    for block in &callable.blocks {
        let check = |target: BlockLabel, kind: &str| {
            assert!(
                target.is_fallthrough() || labels.contains(&target),
                "dangling {} in resume source {} from {} to {}",
                kind,
                callable.names.qualname,
                block.label,
                target,
            );
        };
        match &block.term {
            BlockTerm::Jump(edge) => check(edge.target, "jump"),
            BlockTerm::IfTerm(if_term) => {
                check(if_term.then_label, "then");
                check(if_term.else_label, "else");
            }
            BlockTerm::BranchTable(branch) => {
                for target in &branch.targets {
                    check(*target, "branch");
                }
                check(branch.default_label, "branch default");
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
        }
        if let Some(edge) = &block.exc_edge {
            check(edge.target, "exception");
        }
    }
    let relabel = callable
        .blocks
        .iter()
        .map(|block| (block.label, resume_name_gen.next_block_name()))
        .collect::<HashMap<_, _>>();
    let linear_exception_edges = lowered_exception_edges(&callable.blocks);
    let declared_param_indices_by_label = callable
        .blocks
        .iter()
        .map(|block| {
            (
                relabel
                    .get(&block.label)
                    .expect("resume relabel should cover every block")
                    .clone(),
                generator_resume_declared_param_indices(callable.kind, &block.params),
            )
        })
        .collect::<HashMap<_, _>>();
    let linear_blocks = callable
        .blocks
        .iter()
        .cloned()
        .map(|block| {
            let mut term = block.term;
            term.relabel_targets(&relabel);
            LinearYieldBlock {
                label: relabel
                    .get(&block.label)
                    .expect("resume relabel should cover every block")
                    .clone(),
                body: block.body,
                term,
                params: block.params,
                exc_edge: None,
                extra: Default::default(),
            }
        })
        .collect::<Vec<_>>();
    let remapped_exception_edges = linear_exception_edges
        .into_iter()
        .map(|(label, exc_target)| {
            (
                relabel
                    .get(&label)
                    .expect("resume relabel should cover every exception source")
                    .clone(),
                exc_target.map(|target| {
                    relabel
                        .get(&target)
                        .expect("resume relabel should cover every exception target")
                        .clone()
                }),
            )
        })
        .collect::<HashMap<_, _>>();
    let resume_entry_target = relabel
        .get(&callable.entry_block().label)
        .expect("resume relabel should cover entry block")
        .clone();

    let mut state = ResumeLoweringState::new(
        resume_name_gen,
        callable.kind,
        declared_param_indices_by_label,
    );
    state.resume_targets.push((1, resume_entry_target));

    let mut queue = linear_blocks
        .into_iter()
        .map(|block| {
            (
                block.label.clone(),
                block.body,
                block.term,
                generator_resume_declared_params(callable.kind, &block.params),
                remapped_exception_edges
                    .get(&block.label)
                    .cloned()
                    .unwrap_or(None),
            )
        })
        .collect::<VecDeque<_>>();
    while let Some((label, body, term, params, exc_target)) = queue.pop_front() {
        lower_resume_fragment(&mut state, label, body, term, params, exc_target);
    }

    let dispatch_label = state.fresh_label("resume_dispatch");
    let targets_len = state
        .resume_targets
        .iter()
        .map(|(pc, _)| *pc)
        .max()
        .unwrap_or(1)
        + 1;
    let mut targets = vec![state.exhausted_label.clone(); targets_len];
    let mut dispatch_wrappers = Vec::new();
    let params_by_label = state
        .blocks
        .iter()
        .map(|block| (block.label.clone(), block.params.clone()))
        .collect::<HashMap<_, _>>();
    let resume_targets = state.resume_targets.clone();
    for (pc, label) in resume_targets {
        let declared_params = params_by_label.get(&label).cloned().unwrap_or_default();
        if declared_params.is_empty() {
            targets[pc] = label.clone();
        } else {
            let wrapper_label = state.fresh_label("resume_dispatch_target");
            targets[pc] = wrapper_label.clone();
            dispatch_wrappers.push(LinearCoreBlock {
                label: wrapper_label.clone(),
                body: Vec::new(),
                term: BlockTerm::Jump(BlockEdge::with_args(
                    label.clone(),
                    resume_dispatch_jump_args_for_params(&declared_params),
                )),
                params: Vec::new(),
                exc_edge: None,
                extra: Default::default(),
            });
            state.exception_edges.insert(wrapper_label, None);
        }
    }

    let mut blocks = vec![LinearCoreBlock {
        label: dispatch_label.clone(),
        body: preserved_slot_reload_stmts(preserved_slots),
        term: BlockTerm::BranchTable(TermBranchTable {
            index: core_name("_dp_pc"),
            targets,
            default_label: state.exhausted_label.clone(),
        }),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    }];
    blocks.append(&mut dispatch_wrappers);
    blocks.append(&mut state.blocks);
    blocks.push(LinearCoreBlock {
        label: state.exhausted_label.clone(),
        body: Vec::new(),
        term: completion_raise(state.kind, None),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    });
    for block in &mut blocks {
        if matches!(block.term, BlockTerm::Return(_)) {
            block
                .body
                .extend(preserved_slot_spill_stmts(preserved_slots));
        }
    }
    state.exception_edges.insert(dispatch_label.clone(), None);
    state
        .exception_edges
        .insert(state.exhausted_label.clone(), None);
    (
        attach_exception_edges_to_blocks(blocks, &state.exception_edges),
        state.exception_edges,
        dispatch_label,
    )
}

fn ordered_resume_binding_logical_names(
    _callable: &BlockPyFunction<CoreModuleShapeWithYield>,
    persistent_state_order: &[String],
) -> Vec<String> {
    let mut seen = HashSet::new();
    persistent_state_order
        .iter()
        .cloned()
        .filter(|name| seen.insert(name.clone()))
        .collect()
}

pub(crate) fn lower_generator_like_function(
    callable: BlockPyFunction<CoreModuleShapeWithYield>,
    module_name_gen: &ModuleNameGen,
) -> Vec<BlockPyFunction<CoreModuleShape>> {
    assert!(
        is_generator_like(callable.kind),
        "generator lowering only applies to generator-like callables"
    );
    let resume_name_gen = module_name_gen.next_function_name_gen();
    let resume_function_id = resume_name_gen.function_id();
    let storage_layout = build_generator_storage_layout(&callable);
    let resume_closure_state_order = resume_closure_state_order(&storage_layout);
    let resume_binding_logical_names =
        ordered_resume_binding_logical_names(&callable, &resume_closure_state_order);
    let (resume_blocks, _resume_exception_edges, _resume_entry_label) = lower_resume_blocks(
        &callable,
        resume_name_gen.share(),
        &storage_layout.preserved_slots,
    );
    let closure_bindings = resume_closure_bindings(&callable.scope, &resume_binding_logical_names);

    let BlockPyFunction {
        function_id,
        name_gen,
        names,
        kind,
        execution_mode,
        params,
        doc,
        scope,
        ..
    } = callable;

    let factory_block = build_factory_block(&names, resume_function_id, kind, &storage_layout);

    let mut resume_semantic = scope.clone();
    augment_resume_semantic_for_standard_name_binding(&mut resume_semantic, &closure_bindings);

    let resume_params = resume_param_spec(kind);
    let resume_names = FunctionName::new(
        format!("{}_resume", names.bind_name),
        "_dp_resume",
        names.display_name.clone(),
        names.qualname.clone(),
    );
    let resume_function = BlockPyFunction {
        function_id: resume_function_id,
        name_gen: resume_name_gen,
        names: resume_names,
        kind: FunctionKind::Function,
        execution_mode: Default::default(),
        params: resume_params.clone(),
        blocks: resume_blocks.clone(),
        doc: None,
        storage_layout: None,
        scope: resume_semantic,
    };
    let resume_function = BlockPyFunction {
        storage_layout: compute_storage_layout_from_scope(&resume_function),
        ..resume_function
    };

    let visible_function = BlockPyFunction {
        function_id,
        name_gen,
        names: names.clone(),
        kind,
        execution_mode,
        params: params.clone(),
        blocks: attach_exception_edges_to_blocks(
            vec![factory_block.clone()],
            &HashMap::from([(factory_block.label.clone(), None)]),
        ),
        doc,
        storage_layout: Some(storage_layout.clone()),
        scope: scope.clone(),
    };

    vec![visible_function, resume_function]
}

pub(crate) fn lower_yield_in_lowered_core_blockpy_module_bundle(
    module: BlockPyModule<CoreModuleShapeWithYield>,
) -> BlockPyModule<CoreModuleShape> {
    let module = map_module_functions(module, make_suspend_order_explicit_in_core_callable_def);
    let module_name_gen = module.module_name_gen.clone();
    let mut callable_defs = Vec::new();
    for callable in module.callable_defs {
        match callable.kind {
            FunctionKind::Function => {
                let qualname = callable.names.qualname.clone();
                let mut mapper = ErrOnYield;
                callable_defs.push(mapper.try_map_fn(callable).unwrap_or_else(|_| {
                    panic!(
                        "core BlockPy yield lowering is not explicit yet: yield-family expr reached the core no-yield boundary for {}",
                        qualname
                    )
                }));
            }
            FunctionKind::Generator | FunctionKind::Coroutine | FunctionKind::AsyncGenerator => {
                callable_defs.extend(lower_generator_like_function(callable, &module_name_gen));
            }
        }
    }
    BlockPyModule {
        module_name_gen,
        global_names: Vec::new(),
        callable_defs,
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    }
}

#[cfg(test)]
mod test;
