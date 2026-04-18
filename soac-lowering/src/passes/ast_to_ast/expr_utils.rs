use ruff_python_ast::{self as ast, Expr};

pub(crate) fn make_tuple(items: Vec<Expr>) -> Expr {
    Expr::Tuple(ast::ExprTuple {
        range: Default::default(),
        node_index: ast::AtomicNodeIndex::default(),
        elts: items.into(),
        ctx: ast::ExprContext::Load,
        parenthesized: true,
    })
}

pub(crate) fn make_dp_tuple(items: Vec<Expr>) -> Expr {
    make_tuple(items)
}
