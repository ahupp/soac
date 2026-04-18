use crate::transformer::{walk_expr, walk_stmt, Transformer};
use ruff_python_ast::{self as ast, Expr, Stmt};
use std::collections::HashSet;

pub(crate) trait CurrentScopeNameTraversal: Transformer {
    fn bound_names_mut(&mut self) -> &mut HashSet<String>;
    fn loaded_names_mut(&mut self) -> &mut HashSet<String>;

    fn record_bound_name(&mut self, name: &str) {
        self.bound_names_mut().insert(name.to_string());
    }

    fn record_loaded_name(&mut self, name: &str) {
        self.loaded_names_mut().insert(name.to_string());
    }

    fn visit_current_scope_stmt_impl(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    collect_assigned_names(target, self.bound_names_mut());
                }
                walk_stmt(self, stmt);
            }
            Stmt::AugAssign(aug) => {
                collect_assigned_names(aug.target.as_ref(), self.bound_names_mut());
                walk_stmt(self, stmt);
            }
            Stmt::AnnAssign(ann) => {
                collect_assigned_names(ann.target.as_ref(), self.bound_names_mut());
                walk_stmt(self, stmt);
            }
            Stmt::For(for_stmt) => {
                collect_assigned_names(for_stmt.target.as_ref(), self.bound_names_mut());
                walk_stmt(self, stmt);
            }
            Stmt::With(with_stmt) => {
                for item in &with_stmt.items {
                    if let Some(optional_vars) = item.optional_vars.as_ref() {
                        collect_assigned_names(optional_vars.as_ref(), self.bound_names_mut());
                    }
                }
                walk_stmt(self, stmt);
            }
            Stmt::Delete(delete_stmt) => {
                for target in &delete_stmt.targets {
                    collect_assigned_names(target, self.bound_names_mut());
                }
                walk_stmt(self, stmt);
            }
            Stmt::Try(try_stmt) => {
                for handler in &try_stmt.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    if let Some(name) = handler.name.as_ref() {
                        self.record_bound_name(name.id.as_str());
                    }
                }
                walk_stmt(self, stmt);
            }
            Stmt::Import(import_stmt) => {
                for alias in &import_stmt.names {
                    self.record_bound_name(import_binding_name(alias));
                }
            }
            Stmt::ImportFrom(import_stmt) => {
                for alias in &import_stmt.names {
                    if alias.name.as_str() == "*" {
                        continue;
                    }
                    self.record_bound_name(alias.asname.as_ref().unwrap_or(&alias.name).as_str());
                }
            }
            Stmt::FunctionDef(func_def) => {
                self.record_bound_name(func_def.name.id.as_str());
                for decorator in &mut func_def.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(type_params) = func_def.type_params.as_mut() {
                    self.visit_type_params(type_params);
                }
                self.visit_parameters(&mut func_def.parameters);
                if let Some(returns) = func_def.returns.as_mut() {
                    self.visit_annotation(returns);
                }
            }
            Stmt::ClassDef(class_def) => {
                self.record_bound_name(class_def.name.id.as_str());
                for decorator in &mut class_def.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(type_params) = class_def.type_params.as_mut() {
                    self.visit_type_params(type_params);
                }
                if let Some(arguments) = class_def.arguments.as_mut() {
                    self.visit_arguments(arguments);
                }
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_current_scope_expr_impl(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Name(name) => {
                match name.ctx {
                    ast::ExprContext::Load => self.record_loaded_name(name.id.as_str()),
                    ast::ExprContext::Store => self.record_bound_name(name.id.as_str()),
                    _ => {}
                }
                walk_expr(self, expr);
            }
            Expr::Named(named) => {
                collect_assigned_names(named.target.as_ref(), self.bound_names_mut());
                self.visit_expr(named.value.as_mut());
            }
            Expr::Lambda(_)
            | Expr::Generator(_)
            | Expr::ListComp(_)
            | Expr::SetComp(_)
            | Expr::DictComp(_) => {}
            _ => walk_expr(self, expr),
        }
    }
}

#[derive(Default)]
struct CurrentScopeNameCollector {
    bound_names: HashSet<String>,
    loaded_names: HashSet<String>,
}

impl CurrentScopeNameTraversal for CurrentScopeNameCollector {
    fn bound_names_mut(&mut self) -> &mut HashSet<String> {
        &mut self.bound_names
    }

    fn loaded_names_mut(&mut self) -> &mut HashSet<String> {
        &mut self.loaded_names
    }
}

impl Transformer for CurrentScopeNameCollector {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        self.visit_current_scope_stmt_impl(stmt);
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        self.visit_current_scope_expr_impl(expr);
    }
}

pub(crate) fn collect_loaded_names(stmts: &[Stmt]) -> HashSet<String> {
    let mut body = stmts.to_vec();
    let mut collector = CurrentScopeNameCollector::default();
    collector.visit_body(&mut body);
    collector.loaded_names
}

pub(crate) fn collect_bound_names(stmts: &[Stmt]) -> HashSet<String> {
    let mut body = stmts.to_vec();
    let mut collector = CurrentScopeNameCollector::default();
    collector.visit_body(&mut body);
    collector.bound_names
}

#[cfg(test)]
#[derive(Default)]
struct ExplicitGlobalOrNonlocalCollector {
    names: HashSet<String>,
}

#[cfg(test)]
impl Transformer for ExplicitGlobalOrNonlocalCollector {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::Global(global_stmt) => {
                for name in &global_stmt.names {
                    self.names.insert(name.id.to_string());
                }
            }
            Stmt::Nonlocal(nonlocal_stmt) => {
                for name in &nonlocal_stmt.names {
                    self.names.insert(name.id.to_string());
                }
            }
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            _ => walk_stmt(self, stmt),
        }
    }
}

#[cfg(test)]
pub(crate) fn collect_explicit_global_or_nonlocal_names(stmts: &[Stmt]) -> HashSet<String> {
    let mut body = stmts.to_vec();
    let mut collector = ExplicitGlobalOrNonlocalCollector::default();
    collector.visit_body(&mut body);
    collector.names
}

fn collect_assigned_names(target: &Expr, names: &mut HashSet<String>) {
    match target {
        Expr::Name(name) => {
            names.insert(name.id.to_string());
        }
        Expr::Tuple(tuple) => {
            for elt in &tuple.elts {
                collect_assigned_names(elt, names);
            }
        }
        Expr::List(list) => {
            for elt in &list.elts {
                collect_assigned_names(elt, names);
            }
        }
        Expr::Starred(starred) => collect_assigned_names(starred.value.as_ref(), names),
        _ => {}
    }
}

fn import_binding_name(alias: &ast::Alias) -> &str {
    alias.asname.as_ref().map_or_else(
        || alias.name.as_str().split('.').next().unwrap(),
        |asname| asname.as_str(),
    )
}

#[cfg(test)]
mod test;
