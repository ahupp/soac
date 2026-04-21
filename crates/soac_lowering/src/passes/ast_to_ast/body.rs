use ruff_python_ast::{self as ast, Stmt};

pub type Suite = ast::Suite;

pub fn empty_suite() -> Suite {
    vec![]
}

pub fn split_docstring(body: &Suite) -> (Option<String>, Suite) {
    let mut rest = body.clone();
    let Some(docstring) = rest.first().and_then(|first| match first {
        Stmt::Expr(ast::StmtExpr { value, .. }) => match value.as_ref() {
            ast::Expr::StringLiteral(ast::ExprStringLiteral { value, .. }) => {
                Some(value.to_string())
            }
            _ => None,
        },
        _ => None,
    }) else {
        return (None, rest);
    };
    rest.remove(0);
    (Some(docstring), rest)
}
