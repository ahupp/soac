use super::{
    walk_block, walk_expr, Block, BlockPyFunction, Call, CallArgPositional, CellCaptureBinding,
    ChildVisitable, ClosureInit, ClosureSlot, Instr, InstrLow, InstrResolved, InstrRuff,
    InstrWithAwaitAndYield, InstrWithYield, Literal, ModuleShape, NameLike, StorageLayout,
};
use ruff_python_ast::{self as ast};
use std::collections::HashSet;

pub(crate) trait ScopeExprNode: Instr + ChildVisitable<Self> {
    fn root_name_id(&self) -> Option<&str> {
        None
    }

    fn root_string_literal_value(&self) -> Option<String> {
        None
    }

    fn walk_root_loaded_names(&self, _f: &mut impl FnMut(&str)) {}

    fn walk_root_defined_names(&self, _f: &mut impl FnMut(&str)) {}

    fn walk_root_deleted_names(&self, _f: &mut impl FnMut(&str)) {}

    fn walk_root_cell_ref_logical_names(&self, _f: &mut impl FnMut(&str)) {}
}

fn call_root_cell_ref_logical_name<E>(call: &Call<E>) -> Option<String>
where
    E: ScopeExprNode,
{
    let helper_name = call.func.as_ref().root_name_id()?;
    if helper_name != "cell_ref" {
        return None;
    }
    let CallArgPositional::Positional(arg) = call.args.first()? else {
        return None;
    };
    arg.root_string_literal_value()
}

fn walk_assigned_name_targets_in_instr_ruff(target: &InstrRuff, f: &mut impl FnMut(&str)) {
    match target {
        InstrRuff::ExprName(name) => {
            if matches!(name.ctx, ast::ExprContext::Store | ast::ExprContext::Del) {
                f(name.id.as_str());
            }
        }
        InstrRuff::ExprTuple(tuple) => {
            for elt in &tuple.elts {
                walk_assigned_name_targets_in_instr_ruff(elt, f);
            }
        }
        InstrRuff::ExprList(list) => {
            for elt in &list.elts {
                walk_assigned_name_targets_in_instr_ruff(elt, f);
            }
        }
        InstrRuff::ExprStarred(starred) => {
            walk_assigned_name_targets_in_instr_ruff(starred.value.as_ref(), f)
        }
        InstrRuff::ExprNamed(named) => {
            walk_assigned_name_targets_in_instr_ruff(named.target.as_ref(), f)
        }
        InstrRuff::StmtAssign(stmt) => {
            for target in &stmt.targets {
                walk_assigned_name_targets_in_instr_ruff(target, f);
            }
        }
        InstrRuff::StmtDelete(stmt) => {
            for target in &stmt.targets {
                walk_assigned_name_targets_in_instr_ruff(target, f);
            }
        }
        _ => {}
    }
}

impl ScopeExprNode for InstrRuff {
    fn root_name_id(&self) -> Option<&str> {
        match self {
            Self::ExprName(name) => Some(name.id.as_str()),
            Self::ExprAttribute(attr) if matches!(attr.value.as_ref(), Self::ExprName(name) if name.id.as_str() == "__soac__") => {
                Some(attr.attr.as_str())
            }
            Self::Call(call) => call.func.as_ref().root_name_id(),
            Self::StmtFunctionDef(stmt) => Some(stmt.name.as_str()),
            Self::StmtClassDef(stmt) => Some(stmt.name.as_str()),
            Self::StmtExpr(stmt) => stmt.value.as_ref().root_name_id(),
            _ => None,
        }
    }

    fn root_string_literal_value(&self) -> Option<String> {
        match self {
            Self::ExprStringLiteral(literal) => Some(literal.value.to_str().to_string()),
            Self::StmtExpr(stmt) => stmt.value.as_ref().root_string_literal_value(),
            _ => None,
        }
    }

    fn walk_root_loaded_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::ExprName(name) => {
                if matches!(name.ctx, ast::ExprContext::Load) {
                    f(name.id.as_str());
                }
            }
            Self::Call(call) => {
                if let Some(name) = call.func.as_ref().root_name_id() {
                    f(name);
                }
            }
            Self::StmtExpr(stmt) => stmt.value.as_ref().walk_root_loaded_names(f),
            _ => {}
        }
    }

    fn walk_root_defined_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::ExprNamed(named) => {
                walk_assigned_name_targets_in_instr_ruff(named.target.as_ref(), f);
            }
            Self::ExprName(name) => {
                if matches!(name.ctx, ast::ExprContext::Store) {
                    f(name.id.as_str());
                }
            }
            Self::StmtAssign(stmt) => {
                for target in &stmt.targets {
                    walk_assigned_name_targets_in_instr_ruff(target, f);
                }
            }
            Self::StmtFunctionDef(stmt) => f(stmt.name.as_str()),
            Self::StmtClassDef(stmt) => f(stmt.name.as_str()),
            Self::StmtExpr(stmt) => stmt.value.as_ref().walk_root_defined_names(f),
            _ => {}
        }
    }

    fn walk_root_deleted_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::ExprName(name) => {
                if matches!(name.ctx, ast::ExprContext::Del) {
                    f(name.id.as_str());
                }
            }
            Self::StmtDelete(stmt) => {
                for target in &stmt.targets {
                    walk_assigned_name_targets_in_instr_ruff(target, f);
                }
            }
            Self::StmtExpr(stmt) => stmt.value.as_ref().walk_root_deleted_names(f),
            _ => {}
        }
    }

    fn walk_root_cell_ref_logical_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call_root_cell_ref_logical_name(call) {
                    f(name.as_str());
                }
            }
            Self::StmtExpr(stmt) => stmt.value.as_ref().walk_root_cell_ref_logical_names(f),
            _ => {}
        }
    }
}

impl ScopeExprNode for InstrWithAwaitAndYield {
    fn root_name_id(&self) -> Option<&str> {
        match self {
            Self::Call(call) => call.func.as_ref().root_name_id(),
            Self::Load(op) => Some(op.name.id_str()),
            _ => None,
        }
    }

    fn root_string_literal_value(&self) -> Option<String> {
        match self {
            Self::Literal(literal) => match literal.as_literal() {
                Literal::StringLiteral(literal) => Some(literal.value.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn walk_root_loaded_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call.func.as_ref().root_name_id() {
                    f(name);
                }
            }
            Self::Load(op) => f(op.name.id_str()),
            _ => {}
        }
    }

    fn walk_root_defined_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Store(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_deleted_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Del(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_cell_ref_logical_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call_root_cell_ref_logical_name(call) {
                    f(name.as_str());
                }
            }
            Self::CellRefForName(op) => f(op.logical_name.as_str()),
            _ => {}
        }
    }
}

impl ScopeExprNode for InstrWithYield {
    fn root_name_id(&self) -> Option<&str> {
        match self {
            Self::Call(call) => call.func.as_ref().root_name_id(),
            Self::Load(op) => Some(op.name.id_str()),
            _ => None,
        }
    }

    fn root_string_literal_value(&self) -> Option<String> {
        match self {
            Self::Literal(literal) => match literal.as_literal() {
                Literal::StringLiteral(literal) => Some(literal.value.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn walk_root_loaded_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call.func.as_ref().root_name_id() {
                    f(name);
                }
            }
            Self::Load(op) => f(op.name.id_str()),
            _ => {}
        }
    }

    fn walk_root_defined_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Store(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_deleted_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Del(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_cell_ref_logical_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call_root_cell_ref_logical_name(call) {
                    f(name.as_str());
                }
            }
            Self::CellRefForName(op) => f(op.logical_name.as_str()),
            _ => {}
        }
    }
}

impl<N> ScopeExprNode for InstrLow<N>
where
    N: NameLike,
{
    fn root_name_id(&self) -> Option<&str> {
        match self {
            Self::Call(call) => call.func.as_ref().root_name_id(),
            Self::Load(op) => Some(op.name.id_str()),
            _ => None,
        }
    }

    fn root_string_literal_value(&self) -> Option<String> {
        match self {
            Self::Literal(literal) => match literal.as_literal() {
                Literal::StringLiteral(literal) => Some(literal.value.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn walk_root_loaded_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call.func.as_ref().root_name_id() {
                    f(name);
                }
            }
            Self::Load(op) => f(op.name.id_str()),
            _ => {}
        }
    }

    fn walk_root_defined_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Store(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_deleted_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Del(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_cell_ref_logical_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call_root_cell_ref_logical_name(call) {
                    f(name.as_str());
                }
            }
            Self::CellRefForName(op) => f(op.logical_name.as_str()),
            _ => {}
        }
    }
}

impl ScopeExprNode for InstrResolved {
    fn root_name_id(&self) -> Option<&str> {
        match self {
            Self::Call(call) => call.func.as_ref().root_name_id(),
            Self::Load(op) => Some(op.name.id_str()),
            _ => None,
        }
    }

    fn root_string_literal_value(&self) -> Option<String> {
        match self {
            Self::Literal(literal) => match literal.as_literal() {
                Literal::StringLiteral(literal) => Some(literal.value.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    fn walk_root_loaded_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call.func.as_ref().root_name_id() {
                    f(name);
                }
            }
            Self::Load(op) => f(op.name.id_str()),
            _ => {}
        }
    }

    fn walk_root_defined_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Store(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_deleted_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Del(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_cell_ref_logical_names(&self, _f: &mut impl FnMut(&str)) {}
}

impl ScopeExprNode for super::InstrCodegen {
    fn root_name_id(&self) -> Option<&str> {
        match self {
            Self::Call(call) => call.func.as_ref().root_name_id(),
            Self::Load(op) => Some(op.name.id_str()),
            _ => None,
        }
    }

    fn walk_root_loaded_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call.func.as_ref().root_name_id() {
                    f(name);
                }
            }
            Self::Load(op) => f(op.name.id_str()),
            _ => {}
        }
    }

    fn walk_root_defined_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Store(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_deleted_names(&self, f: &mut impl FnMut(&str)) {
        if let Self::Del(op) = self {
            f(op.name.id_str());
        }
    }

    fn walk_root_cell_ref_logical_names(&self, f: &mut impl FnMut(&str)) {
        match self {
            Self::Call(call) => {
                if let Some(name) = call_root_cell_ref_logical_name(call) {
                    f(name.as_str());
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct StorageLayoutScopeCollector {
    used_names: HashSet<String>,
    defined_names: HashSet<String>,
    deleted_names: HashSet<String>,
    cell_ref_logical_names: HashSet<String>,
}

impl<I> crate::block_py::Visit<I> for StorageLayoutScopeCollector
where
    I: ScopeExprNode,
{
    fn visit_instr(&mut self, expr: &I) {
        expr.walk_root_loaded_names(&mut |name| {
            self.used_names.insert(name.to_string());
        });
        expr.walk_root_defined_names(&mut |name| {
            self.defined_names.insert(name.to_string());
        });
        expr.walk_root_cell_ref_logical_names(&mut |name| {
            self.cell_ref_logical_names.insert(name.to_string());
        });
        walk_expr::<Self, I>(self, expr);
    }

    fn visit_block(&mut self, block: &Block<I>) {
        if let Some(exc_param) = block.exception_param() {
            self.used_names.insert(exc_param.to_string());
        }
        walk_block::<Self, I>(self, block);
    }

    fn visit_stmt(&mut self, stmt: &I) {
        stmt.walk_root_deleted_names(&mut |name| {
            let name = name.to_string();
            self.used_names.insert(name.clone());
            self.deleted_names.insert(name);
        });
        self.visit_instr(stmt);
    }
}

pub(crate) fn is_runtime_closure_name(name: &str) -> bool {
    matches!(name, "_dp_pc" | "_dp_yieldfrom" | "_dp_throw_context")
        || name.starts_with("_dp_try_abrupt_kind_")
}

pub(crate) fn compute_make_function_capture_bindings_from_scope<P>(
    callable_def: &BlockPyFunction<P>,
) -> Vec<CellCaptureBinding>
where
    P: ModuleShape,
    P::Instr: ScopeExprNode,
{
    let normalize_capture_name = |name: &str| {
        callable_def
            .scope
            .logical_name_for_cell_capture_source(name)
            .or_else(|| callable_def.scope.logical_name_for_cell_storage(name))
            .unwrap_or_else(|| name.to_string())
    };

    let param_names = callable_def.params.names();
    let owned_cell_slot_names = callable_def.scope.owned_cell_storage_names();
    let param_name_set = param_names.iter().cloned().collect::<HashSet<_>>();

    let mut collector = StorageLayoutScopeCollector::default();
    crate::block_py::walk_fn::<StorageLayoutScopeCollector, P>(&mut collector, callable_def);

    let mut capture_bindings = callable_def
        .scope
        .captured_cell_bindings()
        .into_iter()
        .map(|binding| {
            let logical_name = normalize_capture_name(binding.logical_name.as_str());
            CellCaptureBinding {
                source_name: callable_def
                    .scope
                    .cell_capture_source_name(logical_name.as_str()),
                logical_name,
            }
        })
        .collect::<Vec<_>>();
    capture_bindings.extend(
        collector
            .cell_ref_logical_names
            .iter()
            .map(|name| normalize_capture_name(name.as_str()))
            .filter(|logical_name| !is_runtime_closure_name(logical_name.as_str()))
            .filter(|logical_name| !param_name_set.contains(logical_name.as_str()))
            .filter(|logical_name| {
                let source_name = callable_def
                    .scope
                    .cell_capture_source_name(logical_name.as_str());
                if owned_cell_slot_names.contains("_dp_classcell")
                    && (logical_name == "__class__" || source_name == "_dp_classcell")
                {
                    return false;
                }
                !owned_cell_slot_names.contains(source_name.as_str())
            })
            .map(|logical_name| CellCaptureBinding {
                source_name: callable_def
                    .scope
                    .cell_capture_source_name(logical_name.as_str()),
                logical_name,
            }),
    );
    capture_bindings.sort_by(|left, right| {
        left.logical_name
            .cmp(&right.logical_name)
            .then_with(|| left.source_name.cmp(&right.source_name))
    });
    capture_bindings.dedup_by(|left, right| left.logical_name == right.logical_name);

    capture_bindings
}

pub(crate) fn compute_storage_layout_from_scope<P>(
    callable_def: &BlockPyFunction<P>,
) -> Option<StorageLayout>
where
    P: ModuleShape,
    P::Instr: ScopeExprNode,
{
    let owned_cell_slot_names = callable_def.scope.owned_cell_storage_names();
    let mut local_cell_slots = owned_cell_slot_names.iter().cloned().collect::<Vec<_>>();
    local_cell_slots.sort();
    let param_name_set = callable_def
        .params
        .names()
        .into_iter()
        .collect::<HashSet<_>>();

    let capture_names = compute_make_function_capture_bindings_from_scope(callable_def)
        .into_iter()
        .map(|binding| binding.logical_name)
        .collect::<Vec<_>>();

    build_storage_layout_from_capture_names(
        callable_def,
        capture_names,
        &param_name_set,
        &local_cell_slots,
    )
}

pub(crate) fn build_storage_layout_from_capture_names<P>(
    callable_def: &BlockPyFunction<P>,
    mut capture_names: Vec<String>,
    param_name_set: &HashSet<String>,
    local_cell_slots: &[String],
) -> Option<StorageLayout>
where
    P: ModuleShape,
{
    capture_names.sort();
    capture_names.dedup();
    let local_cell_slots = local_cell_slots
        .iter()
        .filter(|storage_name| {
            let logical_name = callable_def
                .scope
                .logical_name_for_cell_storage(storage_name.as_str())
                .unwrap_or_else(|| (*storage_name).clone());
            !is_runtime_closure_name(logical_name.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();

    if capture_names.is_empty() && local_cell_slots.is_empty() {
        return None;
    }

    let freevars = capture_names
        .iter()
        .map(|logical_name| ClosureSlot {
            logical_name: logical_name.clone(),
            storage_name: callable_def.scope.cell_storage_name(logical_name.as_str()),
            init: ClosureInit::InheritedCapture,
        })
        .collect::<Vec<_>>();
    let cellvars = local_cell_slots
        .into_iter()
        .map(|storage_name| {
            let logical_name = callable_def
                .scope
                .logical_name_for_cell_storage(storage_name.as_str())
                .unwrap_or_else(|| storage_name.clone());
            let init = if param_name_set.contains(logical_name.as_str()) {
                ClosureInit::Parameter
            } else {
                ClosureInit::EmptyCell
            };
            ClosureSlot {
                logical_name,
                storage_name,
                init,
            }
        })
        .collect::<Vec<_>>();

    Some(StorageLayout {
        freevars,
        cellvars,
        runtime_cells: Vec::new(),
        stack_slots: Vec::new(),
    })
}
