use ruff_python_ast::{self as ast, Expr};

use crate::py_expr;

pub fn collect_exprs(decorators: &[ast::Decorator]) -> Vec<Expr> {
    decorators
        .iter()
        .map(|decorator| decorator.expression.clone())
        .collect()
}

pub fn into_exprs(decorators: Vec<ast::Decorator>) -> Vec<Expr> {
    decorators
        .into_iter()
        .map(|decorator| decorator.expression)
        .collect()
}

pub fn rewrite_exprs(decorators: Vec<Expr>, mut to_decorate: Expr) -> Expr {
    for decorator in decorators.into_iter().rev() {
        to_decorate = py_expr!(
            "({decorator:expr})({to_decorate:expr})",
            decorator = decorator,
            to_decorate = to_decorate
        );
    }

    to_decorate
}

/// Rewrite decorated functions and classes into explicit decorator applications.
pub fn rewrite(decorators: Vec<ast::Decorator>, to_decorate: Expr) -> Expr {
    rewrite_exprs(into_exprs(decorators), to_decorate)
}

#[cfg(test)]
mod test;
