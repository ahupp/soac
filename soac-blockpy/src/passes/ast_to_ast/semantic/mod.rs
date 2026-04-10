use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use ruff_python_ast::{
    self as ast, ExprContext, HasNodeIndex, NodeIndex, StmtClassDef, StmtFunctionDef,
};
use ruff_python_semantic::{
    BindingFlags as RuffBindingFlags, BindingKind as RuffBindingKind, Module as RuffModule,
    ModuleKind as RuffModuleKind, ModuleSource as RuffModuleSource, ScopeId as RuffScopeId,
    ScopeKind as RuffScopeKind, SemanticModel as RuffSemanticModel,
};
use ruff_text_size::{Ranged, TextRange};

use crate::passes::ast_symbol_analysis::CurrentScopeNameTraversal;
use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::scope_helpers::is_internal_symbol;
use crate::transformer::Transformer;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SemanticBindingKind {
    Local,
    Nonlocal,
    Global,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SemanticBindingUse {
    Load,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SemanticScopeKind {
    Function,
    Class,
    Module,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct SemanticScopeId(usize);

#[derive(Clone, Debug)]
struct SemanticScopeData {
    kind: SemanticScopeKind,
    bindings: HashMap<String, SemanticBindingKind>,
    local_defs: HashSet<String>,
    type_param_names: HashSet<String>,
    local_cell_bindings: HashSet<String>,
    cell_storage_names: HashMap<String, String>,
    parent: Option<SemanticScopeId>,
    qualname: String,
    reuses_child_scopes: bool,
    function_children: HashMap<NodeIndex, SemanticScopeId>,
    class_children: HashMap<NodeIndex, SemanticScopeId>,
}

#[derive(Clone, Debug)]
struct SemanticSnapshot {
    scopes: Vec<SemanticScopeData>,
}

impl SemanticSnapshot {
    fn scope(&self, scope_id: SemanticScopeId) -> &SemanticScopeData {
        &self.scopes[scope_id.0]
    }

    fn scope_mut(&mut self, scope_id: SemanticScopeId) -> &mut SemanticScopeData {
        &mut self.scopes[scope_id.0]
    }
}

#[derive(Debug, Default)]
struct SemanticProvenance {
    function_scope_overrides: HashMap<NodeIndex, SemanticScopeId>,
    next_node_index: u32,
}

impl SemanticProvenance {
    fn ensure_node_index<T: HasNodeIndex>(&mut self, node: &T) -> NodeIndex {
        let node_index = node.node_index().load();
        if node_index != NodeIndex::NONE {
            if let Some(value) = node_index.as_u32() {
                self.next_node_index = self.next_node_index.max(value + 1);
            }
            return node_index;
        }

        let index = NodeIndex::from(self.next_node_index);
        self.next_node_index += 1;
        node.node_index().set(index);
        index
    }

    fn function_scope_override(&self, func_def: &StmtFunctionDef) -> Option<SemanticScopeId> {
        self.function_scope_overrides
            .get(&func_def.node_index().load())
            .copied()
    }
}

#[derive(Default)]
struct MaxNodeIndexCollector {
    max: u32,
}

impl Transformer for MaxNodeIndexCollector {
    fn visit_stmt(&mut self, stmt: &mut ast::Stmt) {
        if let Some(value) = stmt.node_index().load().as_u32() {
            self.max = self.max.max(value);
        }
        crate::transformer::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &mut ast::Expr) {
        if let Some(value) = expr.node_index().load().as_u32() {
            self.max = self.max.max(value);
        }
        crate::transformer::walk_expr(self, expr);
    }
}

fn next_node_index_for_suite(module: &mut Suite) -> u32 {
    let mut cloned = module.clone();
    let mut collector = MaxNodeIndexCollector::default();
    collector.visit_body(&mut cloned);
    collector.max.saturating_add(1)
}

struct MissingNodeIndexAssigner {
    next: u32,
}

impl Transformer for MissingNodeIndexAssigner {
    fn visit_stmt(&mut self, stmt: &mut ast::Stmt) {
        if stmt.node_index().load() == NodeIndex::NONE {
            stmt.node_index().set(NodeIndex::from(self.next));
            self.next += 1;
        }
        crate::transformer::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &mut ast::Expr) {
        if expr.node_index().load() == NodeIndex::NONE {
            expr.node_index().set(NodeIndex::from(self.next));
            self.next += 1;
        }
        crate::transformer::walk_expr(self, expr);
    }
}

fn ensure_node_indices_for_suite(module: &mut Suite) -> u32 {
    let next = next_node_index_for_suite(module);
    MissingNodeIndexAssigner { next }.visit_body(module);
    next_node_index_for_suite(module)
}

#[derive(Clone, Debug)]
struct SemanticStateInner {
    snapshot: SemanticSnapshot,
}

#[derive(Clone, Debug)]
pub(crate) struct SemanticAstState {
    inner: Arc<SemanticStateInner>,
    provenance: Arc<Mutex<SemanticProvenance>>,
}

#[derive(Clone, Debug)]
pub(crate) struct SemanticScope {
    state: SemanticAstState,
    scope_id: SemanticScopeId,
}

impl SemanticScope {
    fn new(state: SemanticAstState, scope_id: SemanticScopeId) -> Self {
        Self { state, scope_id }
    }

    fn data(&self) -> &SemanticScopeData {
        self.state.inner.snapshot.scope(self.scope_id)
    }

    pub(crate) fn kind(&self) -> SemanticScopeKind {
        self.data().kind
    }

    #[cfg(test)]
    pub(crate) fn binding_in_scope(
        &self,
        name: &str,
        use_kind: SemanticBindingUse,
    ) -> SemanticBindingKind {
        match self.binding_in_current_scope(name) {
            Some(binding) => binding,
            None => match use_kind {
                SemanticBindingUse::Load => SemanticBindingKind::Local,
            },
        }
    }

    pub(crate) fn binding_in_current_scope(&self, name: &str) -> Option<SemanticBindingKind> {
        self.data().bindings.get(name).copied()
    }

    pub(crate) fn resolved_load_binding(&self, name: &str) -> SemanticBindingKind {
        if let Some(binding) = self.binding_in_current_scope(name) {
            return binding;
        }
        if is_internal_symbol(name) {
            return SemanticBindingKind::Local;
        }
        let mut current = self.data().parent;
        while let Some(scope_id) = current {
            let scope = self.state.inner.snapshot.scope(scope_id);
            match scope.kind {
                SemanticScopeKind::Function => {
                    if scope.local_defs.contains(name)
                        || matches!(
                            scope.bindings.get(name),
                            Some(SemanticBindingKind::Nonlocal)
                        )
                    {
                        return SemanticBindingKind::Nonlocal;
                    }
                }
                SemanticScopeKind::Module => return SemanticBindingKind::Global,
                SemanticScopeKind::Class => {}
            }
            current = scope.parent;
        }
        SemanticBindingKind::Global
    }

    pub(crate) fn bindings(&self) -> HashMap<String, SemanticBindingKind> {
        self.data().bindings.clone()
    }

    #[cfg(test)]
    pub(crate) fn local_binding_names(&self) -> HashSet<String> {
        self.data()
            .bindings
            .iter()
            .filter_map(|(name, kind)| {
                matches!(kind, SemanticBindingKind::Local).then(|| name.clone())
            })
            .collect()
    }

    pub(crate) fn local_def_names(&self) -> HashSet<String> {
        self.data().local_defs.clone()
    }

    pub(crate) fn has_local_def(&self, name: &str) -> bool {
        self.data().local_defs.contains(name)
    }

    pub(crate) fn type_param_names(&self) -> HashSet<String> {
        self.data().type_param_names.clone()
    }

    pub(crate) fn child_scope_for_function(
        &self,
        func_def: &StmtFunctionDef,
    ) -> Option<SemanticScope> {
        if let Some(scope_id) = self.state.function_scope_id(func_def) {
            let child = self.state.inner.snapshot.scope(scope_id);
            if child.parent == Some(self.scope_id) || self.data().reuses_child_scopes {
                return Some(self.state.scope(scope_id));
            }
        }
        self.data()
            .function_children
            .get(&func_def.node_index().load())
            .copied()
            .filter(|scope_id| {
                self.data().reuses_child_scopes
                    || self.state.inner.snapshot.scope(*scope_id).parent == Some(self.scope_id)
            })
            .map(|scope_id| self.state.scope(scope_id))
    }

    pub(crate) fn child_scope_for_class(&self, class_def: &StmtClassDef) -> Option<SemanticScope> {
        self.data()
            .class_children
            .get(&class_def.node_index().load())
            .copied()
            .filter(|scope_id| {
                self.data().reuses_child_scopes
                    || self.state.inner.snapshot.scope(*scope_id).parent == Some(self.scope_id)
            })
            .map(|scope_id| self.state.scope(scope_id))
    }

    pub(crate) fn local_cell_bindings(&self) -> HashSet<String> {
        self.data().local_cell_bindings.clone()
    }

    pub(crate) fn qualname(&self) -> &str {
        self.data().qualname.as_str()
    }

    #[cfg(test)]
    pub(crate) fn cell_storage_name(&self, name: &str) -> Option<String> {
        self.data().cell_storage_names.get(name).cloned()
    }

    pub(crate) fn cell_storage_names(&self) -> HashMap<String, String> {
        self.data().cell_storage_names.clone()
    }

    pub(crate) fn child_function_qualname(&self, name: &str) -> String {
        child_qualname(self.data(), name)
    }
}

fn child_qualname(parent: &SemanticScopeData, name: &str) -> String {
    if matches!(parent.bindings.get(name), Some(SemanticBindingKind::Global)) {
        return name.to_string();
    }
    match parent.kind {
        SemanticScopeKind::Module => name.to_string(),
        SemanticScopeKind::Function => format!("{}.<locals>.{name}", parent.qualname),
        SemanticScopeKind::Class => format!("{}.{}", parent.qualname, name),
    }
}

#[derive(Default)]
struct RuffScopeBindingCollector {
    bound_names: HashSet<String>,
    type_param_names: HashSet<String>,
    explicit_globals: Vec<(String, TextRange)>,
    explicit_nonlocals: Vec<(String, TextRange)>,
    load_names: HashSet<String>,
}

#[derive(Default)]
struct ImplicitClassCellUseDetector {
    uses_class_cell: bool,
}

fn is_super_call(expr: &ast::Expr) -> bool {
    let ast::Expr::Call(ast::ExprCall { func, .. }) = expr else {
        return false;
    };
    matches!(
        func.as_ref(),
        ast::Expr::Name(ast::ExprName { id, .. }) if id.as_str() == "super"
    )
}

impl Transformer for ImplicitClassCellUseDetector {
    fn visit_stmt(&mut self, stmt: &mut ast::Stmt) {
        match stmt {
            ast::Stmt::ClassDef(class_def) => {
                for decorator in &mut class_def.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(type_params) = class_def.type_params.as_mut() {
                    self.visit_type_params(type_params);
                }
                if let Some(arguments) = class_def.arguments.as_mut() {
                    self.visit_arguments(arguments);
                }
                for stmt in &mut class_def.body {
                    if matches!(stmt, ast::Stmt::FunctionDef(_)) {
                        continue;
                    }
                    self.visit_stmt(stmt);
                }
                return;
            }
            ast::Stmt::Delete(ast::StmtDelete { targets, .. }) => {
                if targets.iter().any(|target| {
                    matches!(
                        target,
                        ast::Expr::Name(ast::ExprName { id, .. }) if id.as_str() == "__class__"
                    )
                }) {
                    self.uses_class_cell = true;
                    return;
                }
            }
            ast::Stmt::Nonlocal(ast::StmtNonlocal { names, .. }) => {
                if names.iter().any(|name| name.id.as_str() == "__class__") {
                    self.uses_class_cell = true;
                    return;
                }
            }
            _ => {}
        }
        crate::transformer::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &mut ast::Expr) {
        match expr {
            ast::Expr::Name(ast::ExprName {
                id,
                ctx: ExprContext::Load,
                ..
            }) if id.as_str() == "__class__" => {
                self.uses_class_cell = true;
                return;
            }
            ast::Expr::Call(_) if is_super_call(expr) => {
                self.uses_class_cell = true;
                return;
            }
            _ => {}
        }
        crate::transformer::walk_expr(self, expr);
    }
}

fn uses_implicit_class_cell(body: &mut Suite) -> bool {
    let mut detector = ImplicitClassCellUseDetector::default();
    detector.visit_body(body);
    detector.uses_class_cell
}

fn expr_uses_implicit_class_cell(expr: &mut ast::Expr) -> bool {
    let mut detector = ImplicitClassCellUseDetector::default();
    detector.visit_expr(expr);
    detector.uses_class_cell
}

impl Transformer for RuffScopeBindingCollector {
    fn visit_stmt(&mut self, stmt: &mut ast::Stmt) {
        match stmt {
            ast::Stmt::Global(global_stmt) => {
                for name in &global_stmt.names {
                    self.explicit_globals
                        .push((name.id.to_string(), name.range()));
                }
            }
            ast::Stmt::Nonlocal(nonlocal_stmt) => {
                for name in &nonlocal_stmt.names {
                    self.explicit_nonlocals
                        .push((name.id.to_string(), name.range()));
                }
            }
            _ => self.visit_current_scope_stmt_impl(stmt),
        }
    }

    fn visit_expr(&mut self, expr: &mut ast::Expr) {
        self.visit_current_scope_expr_impl(expr);
    }

    fn visit_type_param(&mut self, type_param: &mut ast::TypeParam) {
        match type_param {
            ast::TypeParam::TypeVar(ast::TypeParamTypeVar { name, .. })
            | ast::TypeParam::TypeVarTuple(ast::TypeParamTypeVarTuple { name, .. })
            | ast::TypeParam::ParamSpec(ast::TypeParamParamSpec { name, .. }) => {
                self.bound_names.insert(name.id.to_string());
                self.type_param_names.insert(name.id.to_string());
            }
        }
        crate::transformer::walk_type_param(self, type_param);
    }
}

impl CurrentScopeNameTraversal for RuffScopeBindingCollector {
    fn bound_names_mut(&mut self) -> &mut HashSet<String> {
        &mut self.bound_names
    }

    fn loaded_names_mut(&mut self) -> &mut HashSet<String> {
        &mut self.load_names
    }

    fn record_loaded_name(&mut self, name: &str) {
        if !is_internal_symbol(name) {
            self.load_names.insert(name.to_string());
        }
    }
}

fn collect_scope_bindings(
    body: &mut Suite,
    type_params: Option<&mut ast::TypeParams>,
) -> RuffScopeBindingCollector {
    let mut collector = RuffScopeBindingCollector::default();
    if let Some(type_params) = type_params {
        collector.visit_type_params(type_params);
    }
    collector.visit_body(body);
    collector
}

fn collect_scope_expr_bindings(expr: &mut ast::Expr) -> RuffScopeBindingCollector {
    let mut collector = RuffScopeBindingCollector::default();
    collector.visit_expr(expr);
    collector
}

fn merge_semantic_binding(
    existing: SemanticBindingKind,
    incoming: SemanticBindingKind,
) -> SemanticBindingKind {
    match (existing, incoming) {
        (
            SemanticBindingKind::Global | SemanticBindingKind::Nonlocal,
            SemanticBindingKind::Local,
        ) => existing,
        (
            SemanticBindingKind::Local,
            SemanticBindingKind::Global | SemanticBindingKind::Nonlocal,
        ) => incoming,
        _ => existing,
    }
}

fn set_semantic_binding(
    bindings: &mut HashMap<String, SemanticBindingKind>,
    name: &str,
    binding: SemanticBindingKind,
) {
    let binding = if is_internal_symbol(name) {
        SemanticBindingKind::Local
    } else {
        binding
    };
    if let Some(existing) = bindings.get(name).copied() {
        let merged = merge_semantic_binding(existing, binding);
        if merged != existing {
            bindings.insert(name.to_string(), merged);
        }
    } else {
        bindings.insert(name.to_string(), binding);
    }
}

struct ScopePreparation {
    bindings: HashMap<String, SemanticBindingKind>,
    local_defs: HashSet<String>,
    type_param_names: HashSet<String>,
    cell_storage_names: HashMap<String, String>,
}

struct RuffSemanticSnapshotBuilder {
    semantic: RuffSemanticModel<'static>,
    snapshot: SemanticSnapshot,
    scope_stack: Vec<(SemanticScopeId, RuffScopeId)>,
    implicit_nonlocals_by_scope: HashMap<SemanticScopeId, HashSet<String>>,
    next_node_index: u32,
}

impl RuffSemanticSnapshotBuilder {
    fn build(module: &mut Suite) -> SemanticStateInner {
        let module_for_model = Box::leak(Box::new(module.clone()));
        let module_for_build = Box::leak(Box::new(module.clone()));
        let path = Path::new("<semantic-state>");
        let python_ast: &'static [ast::Stmt] = &*module_for_model;
        let module_info = RuffModule {
            kind: RuffModuleKind::Module,
            source: RuffModuleSource::File(path),
            python_ast,
            name: Some("<semantic-state>"),
        };
        let typing_modules: &[String] = &[];
        let semantic = RuffSemanticModel::new(typing_modules, path, module_info);
        let mut builder = Self {
            semantic,
            snapshot: SemanticSnapshot {
                scopes: vec![SemanticScopeData {
                    kind: SemanticScopeKind::Module,
                    bindings: HashMap::new(),
                    local_defs: HashSet::new(),
                    type_param_names: HashSet::new(),
                    local_cell_bindings: HashSet::new(),
                    cell_storage_names: HashMap::new(),
                    parent: None,
                    qualname: String::new(),
                    reuses_child_scopes: false,
                    function_children: HashMap::new(),
                    class_children: HashMap::new(),
                }],
            },
            scope_stack: vec![(SemanticScopeId(0), RuffScopeId::global())],
            implicit_nonlocals_by_scope: HashMap::new(),
            next_node_index: 1,
        };

        let module_preparation = builder.prepare_current_scope(module_for_build, None, &[]);
        {
            let module_scope = builder.snapshot.scope_mut(SemanticScopeId(0));
            module_scope.bindings = module_preparation.bindings;
            module_scope.local_defs = module_preparation.local_defs;
            module_scope.type_param_names = module_preparation.type_param_names;
            module_scope.cell_storage_names = module_preparation.cell_storage_names;
        }
        builder.visit_body(module_for_build);
        builder.propagate_nonlocal_roots();
        builder.compute_local_cell_bindings();

        SemanticStateInner {
            snapshot: builder.snapshot,
        }
    }

    fn current_ids(&self) -> (SemanticScopeId, RuffScopeId) {
        *self
            .scope_stack
            .last()
            .expect("missing current semantic scope")
    }

    fn ensure_node_index<T: HasNodeIndex>(&mut self, node: &T) -> NodeIndex {
        let node_index = node.node_index().load();
        if node_index != NodeIndex::NONE {
            if let Some(value) = node_index.as_u32() {
                self.next_node_index = self.next_node_index.max(value + 1);
            }
            return node_index;
        }

        let index = NodeIndex::from(self.next_node_index);
        self.next_node_index += 1;
        node.node_index().set(index);
        index
    }

    fn prepare_current_scope(
        &mut self,
        body: &mut Suite,
        type_params: Option<&mut ast::TypeParams>,
        parameters: &[(String, TextRange)],
    ) -> ScopePreparation {
        let collector = collect_scope_bindings(body, type_params);
        let uses_class_cell = uses_implicit_class_cell(body);
        self.prepare_scope_from_collector(collector, uses_class_cell, parameters)
    }

    fn prepare_current_expr_scope(
        &mut self,
        expr: &mut ast::Expr,
        parameters: &[(String, TextRange)],
    ) -> ScopePreparation {
        let collector = collect_scope_expr_bindings(expr);
        let uses_class_cell = expr_uses_implicit_class_cell(expr);
        self.prepare_scope_from_collector(collector, uses_class_cell, parameters)
    }

    fn visit_function_definition_exprs(&mut self, func_def: &mut ast::StmtFunctionDef) {
        for decorator in &mut func_def.decorator_list {
            self.visit_decorator(decorator);
        }
        for param in &mut func_def.parameters.posonlyargs {
            if let Some(default) = &mut param.default {
                self.visit_expr(default);
            }
        }
        for param in &mut func_def.parameters.args {
            if let Some(default) = &mut param.default {
                self.visit_expr(default);
            }
        }
        for param in &mut func_def.parameters.kwonlyargs {
            if let Some(default) = &mut param.default {
                self.visit_expr(default);
            }
        }
    }

    fn prepare_scope_from_collector(
        &mut self,
        collector: RuffScopeBindingCollector,
        uses_class_cell: bool,
        parameters: &[(String, TextRange)],
    ) -> ScopePreparation {
        let explicit_globals = collector
            .explicit_globals
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<HashSet<_>>();
        let explicit_nonlocals = collector
            .explicit_nonlocals
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<HashSet<_>>();

        for (name, range) in collector.explicit_globals {
            if !self.semantic.scope_id.is_global() {
                let global_binding = self.semantic.global_scope().get(name.as_str());
                let binding_id = self.semantic.push_binding(
                    range,
                    RuffBindingKind::Global(global_binding),
                    RuffBindingFlags::GLOBAL,
                );
                let leaked_name = Box::leak(name.into_boxed_str());
                self.semantic
                    .current_scope_mut()
                    .add(leaked_name, binding_id);
            }
        }
        for (name, range) in collector.explicit_nonlocals {
            if let Some((scope_id, binding_id)) = self.semantic.nonlocal(name.as_str()) {
                let binding_id = self.semantic.push_binding(
                    range,
                    RuffBindingKind::Nonlocal(binding_id, scope_id),
                    RuffBindingFlags::NONLOCAL,
                );
                let leaked_name = Box::leak(name.into_boxed_str());
                self.semantic
                    .current_scope_mut()
                    .add(leaked_name, binding_id);
            }
        }
        for (name, range) in parameters {
            let binding_id = self.semantic.push_binding(
                *range,
                RuffBindingKind::Argument,
                RuffBindingFlags::empty(),
            );
            let leaked_name = Box::leak(name.clone().into_boxed_str());
            self.semantic
                .current_scope_mut()
                .add(leaked_name, binding_id);
        }
        for name in collector.bound_names.iter() {
            if self.semantic.current_scope().has(name.as_str()) {
                continue;
            }
            let binding_id = self.semantic.push_binding(
                TextRange::default(),
                RuffBindingKind::Assignment,
                RuffBindingFlags::empty(),
            );
            let leaked_name = Box::leak(name.clone().into_boxed_str());
            self.semantic
                .current_scope_mut()
                .add(leaked_name, binding_id);
        }

        let mut bindings = HashMap::new();
        let mut local_defs = HashSet::new();
        let mut cell_storage_names = HashMap::new();
        for name in &explicit_globals {
            set_semantic_binding(&mut bindings, name, SemanticBindingKind::Global);
        }
        for name in &explicit_nonlocals {
            set_semantic_binding(&mut bindings, name, SemanticBindingKind::Nonlocal);
        }
        for (name, _) in parameters {
            set_semantic_binding(&mut bindings, name.as_str(), SemanticBindingKind::Local);
            if !explicit_globals.contains(name) && !explicit_nonlocals.contains(name) {
                local_defs.insert(name.clone());
            }
        }
        for name in &collector.bound_names {
            set_semantic_binding(&mut bindings, name.as_str(), SemanticBindingKind::Local);
            if !explicit_globals.contains(name) && !explicit_nonlocals.contains(name) {
                local_defs.insert(name.clone());
            }
        }
        for name in collector.load_names {
            if bindings.contains_key(name.as_str()) {
                continue;
            }
            if let Some(storage_name) = self.enclosing_function_capture_storage_name(name.as_str())
            {
                set_semantic_binding(&mut bindings, name.as_str(), SemanticBindingKind::Nonlocal);
                if let Some(storage_name) = storage_name {
                    cell_storage_names.insert(name.clone(), storage_name);
                }
                self.implicit_nonlocals_by_scope
                    .entry(self.current_ids().0)
                    .or_default()
                    .insert(name);
            }
        }

        let should_synthesize_class_cell = self.current_scope_is_function()
            && uses_class_cell
            && self.current_scope_has_class_ancestor();
        if should_synthesize_class_cell && !bindings.contains_key("__class__") {
            set_semantic_binding(&mut bindings, "__class__", SemanticBindingKind::Nonlocal);
            cell_storage_names.insert("__class__".to_string(), "_dp_classcell".to_string());
        } else if should_synthesize_class_cell
            && matches!(
                bindings.get("__class__"),
                Some(SemanticBindingKind::Nonlocal)
            )
        {
            cell_storage_names.insert("__class__".to_string(), "_dp_classcell".to_string());
        }

        ScopePreparation {
            bindings,
            local_defs,
            type_param_names: collector.type_param_names,
            cell_storage_names,
        }
    }

    fn current_scope_is_function(&self) -> bool {
        matches!(
            self.semantic.current_scope().kind,
            RuffScopeKind::Function(_) | RuffScopeKind::Lambda(_)
        )
    }

    fn current_scope_has_class_ancestor(&self) -> bool {
        let mut current = Some(self.current_ids().0);
        while let Some(scope_id) = current {
            let scope = self.snapshot.scope(scope_id);
            match scope.kind {
                SemanticScopeKind::Class => return true,
                SemanticScopeKind::Module => return false,
                SemanticScopeKind::Function => current = scope.parent,
            }
        }
        false
    }

    fn enclosing_function_capture_storage_name(&self, name: &str) -> Option<Option<String>> {
        let mut current = Some(self.current_ids().0);
        while let Some(scope_id) = current {
            let scope = self.snapshot.scope(scope_id);
            match scope.kind {
                SemanticScopeKind::Function => {
                    if scope.local_defs.contains(name)
                        || matches!(
                            scope.bindings.get(name),
                            Some(SemanticBindingKind::Nonlocal)
                        )
                    {
                        return Some(scope.cell_storage_names.get(name).cloned());
                    }
                }
                SemanticScopeKind::Module => return None,
                SemanticScopeKind::Class => {}
            }
            current = scope.parent;
        }
        None
    }

    fn push_snapshot_scope(
        &mut self,
        kind: SemanticScopeKind,
        name: &str,
        node_index: NodeIndex,
        preparation: ScopePreparation,
    ) -> SemanticScopeId {
        let parent_id = self.current_ids().0;
        let qualname = child_qualname(self.snapshot.scope(parent_id), name);
        let scope_id = SemanticScopeId(self.snapshot.scopes.len());
        self.snapshot.scopes.push(SemanticScopeData {
            kind,
            bindings: preparation.bindings,
            local_defs: preparation.local_defs,
            type_param_names: preparation.type_param_names,
            local_cell_bindings: HashSet::new(),
            cell_storage_names: preparation.cell_storage_names,
            parent: Some(parent_id),
            qualname,
            reuses_child_scopes: false,
            function_children: HashMap::new(),
            class_children: HashMap::new(),
        });
        match kind {
            SemanticScopeKind::Function => {
                self.snapshot
                    .scope_mut(parent_id)
                    .function_children
                    .insert(node_index, scope_id);
            }
            SemanticScopeKind::Class => {
                self.snapshot
                    .scope_mut(parent_id)
                    .class_children
                    .insert(node_index, scope_id);
            }
            SemanticScopeKind::Module => {}
        }
        scope_id
    }

    fn propagate_nonlocal_roots(&mut self) {
        let scope_ids = (0..self.snapshot.scopes.len())
            .map(SemanticScopeId)
            .collect::<Vec<_>>();
        for scope_id in scope_ids {
            let nonlocals = self
                .snapshot
                .scope(scope_id)
                .bindings
                .iter()
                .filter_map(|(name, kind)| {
                    if matches!(kind, SemanticBindingKind::Nonlocal) {
                        Some(name.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            for name in nonlocals {
                let mut current = self.snapshot.scope(scope_id).parent;
                while let Some(parent_id) = current {
                    let parent = self.snapshot.scope(parent_id);
                    if matches!(parent.kind, SemanticScopeKind::Function)
                        && matches!(
                            parent.bindings.get(name.as_str()),
                            Some(SemanticBindingKind::Local)
                        )
                    {
                        set_semantic_binding(
                            &mut self.snapshot.scope_mut(parent_id).bindings,
                            name.as_str(),
                            SemanticBindingKind::Nonlocal,
                        );
                        break;
                    }
                    current = parent.parent;
                }
            }
        }
    }

    fn descendant_uses_nonlocal(&self, scope_id: SemanticScopeId, name: &str) -> bool {
        let scope = self.snapshot.scope(scope_id);
        let child_ids = scope
            .function_children
            .values()
            .chain(scope.class_children.values())
            .copied()
            .collect::<Vec<_>>();
        for child_id in child_ids {
            if matches!(
                self.snapshot.scope(child_id).bindings.get(name),
                Some(SemanticBindingKind::Nonlocal)
            ) {
                return true;
            }
            if self.descendant_uses_nonlocal(child_id, name) {
                return true;
            }
        }
        false
    }

    fn compute_local_cell_bindings(&mut self) {
        let scope_ids = (0..self.snapshot.scopes.len())
            .map(SemanticScopeId)
            .collect::<Vec<_>>();
        for scope_id in scope_ids {
            let local_defs = self.snapshot.scope(scope_id).local_defs.clone();
            let local_cell_bindings = local_defs
                .into_iter()
                .filter(|name| self.descendant_uses_nonlocal(scope_id, name.as_str()))
                .collect::<HashSet<_>>();
            self.snapshot.scope_mut(scope_id).local_cell_bindings = local_cell_bindings;
        }
    }
}

impl Transformer for RuffSemanticSnapshotBuilder {
    fn visit_stmt(&mut self, stmt: &mut ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(func_def) => {
                self.visit_function_definition_exprs(func_def);
                let node_index = self.ensure_node_index(func_def);
                let leaked_func = Box::leak(Box::new(func_def.clone()));
                self.semantic
                    .push_scope(RuffScopeKind::Function(leaked_func));
                let parameters = parameter_refs(&func_def.parameters);
                let preparation = self.prepare_current_scope(
                    &mut func_def.body,
                    func_def.type_params.as_deref_mut(),
                    &parameters,
                );
                let scope_id = self.push_snapshot_scope(
                    SemanticScopeKind::Function,
                    func_def.name.id.as_str(),
                    node_index,
                    preparation,
                );
                let ruff_scope_id = self.semantic.scope_id;
                self.scope_stack.push((scope_id, ruff_scope_id));
                self.visit_body(&mut func_def.body);
                self.scope_stack.pop();
                self.semantic.pop_scope();
            }
            ast::Stmt::ClassDef(class_def) => {
                let node_index = self.ensure_node_index(class_def);
                let leaked_class = Box::leak(Box::new(class_def.clone()));
                self.semantic.push_scope(RuffScopeKind::Class(leaked_class));
                let preparation = self.prepare_current_scope(
                    &mut class_def.body,
                    class_def.type_params.as_deref_mut(),
                    &[],
                );
                let scope_id = self.push_snapshot_scope(
                    SemanticScopeKind::Class,
                    class_def.name.id.as_str(),
                    node_index,
                    preparation,
                );
                let ruff_scope_id = self.semantic.scope_id;
                self.scope_stack.push((scope_id, ruff_scope_id));
                self.visit_body(&mut class_def.body);
                self.scope_stack.pop();
                self.semantic.pop_scope();
            }
            _ => crate::transformer::walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &mut ast::Expr) {
        match expr {
            ast::Expr::Lambda(lambda) => {
                let node_index = self.ensure_node_index(lambda);
                let leaked_lambda = Box::leak(Box::new(lambda.clone()));
                self.semantic
                    .push_scope(RuffScopeKind::Lambda(leaked_lambda));
                let parameters = lambda
                    .parameters
                    .as_deref()
                    .map(parameter_refs)
                    .unwrap_or_default();
                let preparation =
                    self.prepare_current_expr_scope(lambda.body.as_mut(), &parameters);
                let scope_id = self.push_snapshot_scope(
                    SemanticScopeKind::Function,
                    "<lambda>",
                    node_index,
                    preparation,
                );
                let ruff_scope_id = self.semantic.scope_id;
                self.scope_stack.push((scope_id, ruff_scope_id));
                self.visit_expr(lambda.body.as_mut());
                self.scope_stack.pop();
                self.semantic.pop_scope();
            }
            _ => crate::transformer::walk_expr(self, expr),
        }
    }
}

fn parameter_refs(parameters: &ast::Parameters) -> Vec<(String, TextRange)> {
    let mut refs = Vec::new();
    for parameter in &parameters.posonlyargs {
        refs.push((
            parameter.parameter.name.id.to_string(),
            parameter.parameter.range(),
        ));
    }
    for parameter in &parameters.args {
        refs.push((
            parameter.parameter.name.id.to_string(),
            parameter.parameter.range(),
        ));
    }
    if let Some(vararg) = parameters.vararg.as_ref() {
        refs.push((vararg.name.id.to_string(), vararg.range()));
    }
    for parameter in &parameters.kwonlyargs {
        refs.push((
            parameter.parameter.name.id.to_string(),
            parameter.parameter.range(),
        ));
    }
    if let Some(kwarg) = parameters.kwarg.as_ref() {
        refs.push((kwarg.name.id.to_string(), kwarg.range()));
    }
    refs
}

impl SemanticAstState {
    pub(crate) fn from_ruff(module: &mut Suite) -> Self {
        let next_node_index = ensure_node_indices_for_suite(module);
        let inner = Arc::new(RuffSemanticSnapshotBuilder::build(module));
        let mut provenance = SemanticProvenance::default();
        provenance.next_node_index = next_node_index;
        Self {
            inner,
            provenance: Arc::new(Mutex::new(provenance)),
        }
    }

    fn scope(&self, scope_id: SemanticScopeId) -> SemanticScope {
        SemanticScope::new(self.clone(), scope_id)
    }

    fn function_scope_id(&self, func_def: &StmtFunctionDef) -> Option<SemanticScopeId> {
        if let Some(scope_id) = self
            .provenance
            .lock()
            .expect("semantic provenance mutex poisoned")
            .function_scope_override(func_def)
        {
            return Some(scope_id);
        }
        self.inner
            .snapshot
            .scope(SemanticScopeId(0))
            .function_children
            .get(&func_def.node_index().load())
            .copied()
            .or_else(|| {
                self.inner.snapshot.scopes.iter().find_map(|scope| {
                    scope
                        .function_children
                        .get(&func_def.node_index().load())
                        .copied()
                })
            })
    }

    fn lambda_scope_id(&self, lambda: &ast::ExprLambda) -> Option<SemanticScopeId> {
        self.inner
            .snapshot
            .scope(SemanticScopeId(0))
            .function_children
            .get(&lambda.node_index().load())
            .copied()
            .or_else(|| {
                self.inner.snapshot.scopes.iter().find_map(|scope| {
                    scope
                        .function_children
                        .get(&lambda.node_index().load())
                        .copied()
                })
            })
    }

    pub(crate) fn module_scope(&self) -> SemanticScope {
        self.scope(SemanticScopeId(0))
    }

    pub(crate) fn synthesize_module_init_scope(
        &mut self,
        func_def: &StmtFunctionDef,
    ) -> SemanticScope {
        let module_scope = self.module_scope();
        let module_data = module_scope.data().clone();
        let translated_bindings = module_data
            .bindings
            .into_iter()
            .map(|(name, binding)| {
                let translated = if is_internal_symbol(name.as_str()) {
                    SemanticBindingKind::Local
                } else {
                    match binding {
                        SemanticBindingKind::Local => SemanticBindingKind::Global,
                        SemanticBindingKind::Nonlocal => SemanticBindingKind::Nonlocal,
                        SemanticBindingKind::Global => SemanticBindingKind::Global,
                    }
                };
                (name, translated)
            })
            .collect::<HashMap<_, _>>();

        let scope_id = {
            let inner = Arc::make_mut(&mut self.inner);
            let scope_id = SemanticScopeId(inner.snapshot.scopes.len());
            inner.snapshot.scopes.push(SemanticScopeData {
                kind: SemanticScopeKind::Function,
                bindings: translated_bindings,
                local_defs: HashSet::new(),
                type_param_names: HashSet::new(),
                local_cell_bindings: HashSet::new(),
                cell_storage_names: HashMap::new(),
                parent: Some(module_scope.scope_id),
                qualname: "_dp_module_init".to_string(),
                reuses_child_scopes: true,
                function_children: module_scope.data().function_children.clone(),
                class_children: module_scope.data().class_children.clone(),
            });
            scope_id
        };

        let scope = self.scope(scope_id);
        self.register_function_scope_override(func_def, scope.clone());
        scope
    }

    pub(crate) fn register_function_scope_override(
        &mut self,
        func_def: &StmtFunctionDef,
        scope: SemanticScope,
    ) {
        let mut provenance = self
            .provenance
            .lock()
            .expect("semantic provenance mutex poisoned");
        let node_index = provenance.ensure_node_index(func_def);
        provenance
            .function_scope_overrides
            .insert(node_index, scope.scope_id);
    }

    pub(crate) fn function_scope(&self, func_def: &StmtFunctionDef) -> Option<SemanticScope> {
        self.function_scope_id(func_def)
            .map(|scope_id| self.scope(scope_id))
    }

    pub(crate) fn lambda_scope(&self, lambda: &ast::ExprLambda) -> Option<SemanticScope> {
        self.lambda_scope_id(lambda)
            .map(|scope_id| self.scope(scope_id))
    }

    pub(crate) fn has_function_scope_override(&self, func_def: &StmtFunctionDef) -> bool {
        self.provenance
            .lock()
            .expect("semantic provenance mutex poisoned")
            .function_scope_override(func_def)
            .is_some()
    }
}

#[cfg(test)]
mod test;
