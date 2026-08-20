//! Original identifier occurrences which retain Python binding semantics.
//!
//! The spelling alone is never sufficient: compiler-generated names remain
//! private even when another scope contains a same-spelled source name.

use std::collections::HashSet;

use ruff_python_ast::{self as ast, Expr, HasNodeIndex, NodeIndex, Stmt};
use ruff_text_size::{Ranged, TextRange};

use crate::passes::ast_to_ast::scope_helpers::is_internal_symbol;
use crate::transformer::{walk_expr, walk_stmt, Transformer};

#[derive(Clone, Debug, Default)]
pub(crate) struct SourceNameCatalog {
    occurrences: HashSet<(NodeIndex, TextRange, String)>,
}

impl SourceNameCatalog {
    pub(crate) fn from_original(body: &mut ast::Suite) -> Self {
        super::super::semantic::ensure_node_indices_for_suite(body);
        #[derive(Default)]
        struct Collector(SourceNameCatalog);
        impl Collector {
            fn record(&mut self, name: &str, range: TextRange, node: NodeIndex) {
                if is_internal_symbol(name) {
                    assert!(
                        node.as_u32().is_some(),
                        "original source occurrence has a stable node identity"
                    );
                    self.0.occurrences.insert((node, range, name.to_owned()));
                }
            }
        }
        impl Transformer for Collector {
            fn visit_stmt(&mut self, stmt: &mut Stmt) {
                statement_names(stmt, |name, range, node| self.record(name, range, node));
                walk_stmt(self, stmt);
            }

            fn visit_expr(&mut self, expr: &mut Expr) {
                if let Expr::Name(name) = expr {
                    self.record(name.id.as_str(), name.range, name.node_index.load());
                }
                walk_expr(self, expr);
            }

            fn visit_parameter(&mut self, parameter: &mut ast::Parameter) {
                self.record(
                    parameter.name.as_str(),
                    parameter.range,
                    parameter.node_index.load(),
                );
                crate::transformer::walk_parameter(self, parameter);
            }

            fn visit_except_handler(&mut self, handler: &mut ast::ExceptHandler) {
                let ast::ExceptHandler::ExceptHandler(value) = handler;
                if let Some(name) = &value.name {
                    self.record(name.as_str(), name.range(), value.node_index.load());
                }
                crate::transformer::walk_except_handler(self, handler);
            }

            fn visit_type_param(&mut self, parameter: &mut ast::TypeParam) {
                let name = type_parameter_name(parameter);
                self.record(name.as_str(), name.range(), parameter.node_index().load());
                crate::transformer::walk_type_param(self, parameter);
            }

            fn visit_pattern(&mut self, pattern: &mut ast::Pattern) {
                if let Some(name) = pattern_name(pattern) {
                    self.record(name.as_str(), name.range(), pattern.node_index().load());
                }
                crate::transformer::walk_pattern(self, pattern);
            }
        }
        let mut collector = Collector::default();
        collector.visit_body(&mut body.clone());
        collector.0
    }

    pub(crate) fn contains(&self, name: &str, range: TextRange, node: NodeIndex) -> bool {
        is_internal_symbol(name) && self.occurrences.contains(&(node, range, name.to_owned()))
    }
}

pub(crate) fn statement_names(stmt: &Stmt, mut record: impl FnMut(&str, TextRange, NodeIndex)) {
    let node = stmt.node_index().load();
    match stmt {
        Stmt::FunctionDef(function) => record(function.name.as_str(), function.name.range(), node),
        Stmt::ClassDef(class) => record(class.name.as_str(), class.name.range(), node),
        Stmt::Global(global) => {
            for name in &global.names {
                record(name.as_str(), name.range(), node);
            }
        }
        Stmt::Nonlocal(nonlocal) => {
            for name in &nonlocal.names {
                record(name.as_str(), name.range(), node);
            }
        }
        Stmt::Import(import) => {
            for alias in &import.names {
                let name = alias.asname.as_ref().map_or_else(
                    || {
                        alias
                            .name
                            .as_str()
                            .split('.')
                            .next()
                            .expect("nonempty import name")
                    },
                    |name| name.as_str(),
                );
                record(name, alias.range, node);
            }
        }
        Stmt::ImportFrom(import) => {
            for alias in &import.names {
                record(
                    alias.asname.as_ref().unwrap_or(&alias.name).as_str(),
                    alias.range,
                    node,
                );
            }
        }
        _ => {}
    }
}

pub(crate) fn type_parameter_name(parameter: &ast::TypeParam) -> &ast::Identifier {
    match parameter {
        ast::TypeParam::TypeVar(parameter) => &parameter.name,
        ast::TypeParam::TypeVarTuple(parameter) => &parameter.name,
        ast::TypeParam::ParamSpec(parameter) => &parameter.name,
    }
}

pub(crate) fn pattern_name(pattern: &ast::Pattern) -> Option<&ast::Identifier> {
    match pattern {
        ast::Pattern::MatchMapping(pattern) => pattern.rest.as_ref(),
        ast::Pattern::MatchStar(pattern) => pattern.name.as_ref(),
        ast::Pattern::MatchAs(pattern) => pattern.name.as_ref(),
        _ => None,
    }
}
