use super::rewrite_exprs;
use ruff_python_ast::Expr;

fn assert_call_name<'a>(expr: &'a Expr, expected: &str) -> &'a Expr {
    let Expr::Call(call) = expr else {
        panic!("expected call to {expected}, got {expr:?}");
    };
    assert!(
        matches!(call.func.as_ref(), Expr::Name(name) if name.id.as_str() == expected),
        "expected call to {expected}, got {expr:?}",
    );
    call.arguments
        .args
        .first()
        .expect("decorator call should pass the decorated object")
}

#[test]
fn rewrite_exprs_applies_decorators_inside_out() {
    let decorated = rewrite_exprs(
        vec![
            crate::template::py_expr!("d1"),
            crate::template::py_expr!("d2"),
        ],
        crate::template::py_expr!("f"),
    );
    let inner = assert_call_name(&decorated, "d1");
    let original = assert_call_name(inner, "d2");
    assert!(matches!(original, Expr::Name(name) if name.id.as_str() == "f"));
}
