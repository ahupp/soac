use crate::{passes::ast_to_ast::simplify::flatten, test_util::assert_ast_eq};
use ruff_python_ast::{
    self as ast,
    comparable::{ComparableExpr, ComparableStmt},
    Stmt,
};
use ruff_python_parser::{parse_expression, parse_module};

#[test]
fn inserts_placeholder() {
    let fragment = *parse_expression("2").unwrap().into_syntax().body;
    let expr = py_expr!("1 + {two:expr}", two = fragment);
    let expected = *parse_expression("1 + 2").unwrap().into_syntax().body;
    assert_eq!(ComparableExpr::from(&expr), ComparableExpr::from(&expected));
}

#[test]
fn inserts_identifier() {
    let expr = py_expr!("operator.{func:id}(1)", func = "add");
    let expected = *parse_expression("operator.add(1)")
        .unwrap()
        .into_syntax()
        .body;
    assert_eq!(ComparableExpr::from(&expr), ComparableExpr::from(&expected));
}

#[test]
fn reuses_identifier() {
    let expr = py_expr!("{name:id} + {name:id}", name = "x");
    let expected = *parse_expression("x + x").unwrap().into_syntax().body;
    assert_eq!(ComparableExpr::from(&expr), ComparableExpr::from(&expected));
}

#[test]
fn inserts_literal() {
    let expr = py_expr!("{s:literal}", s = "abc");
    let expected = *parse_expression("\"abc\"").unwrap().into_syntax().body;
    assert_eq!(ComparableExpr::from(&expr), ComparableExpr::from(&expected));
}

#[test]
fn inserts_int_literal() {
    let expr = py_expr!("{n:literal}", n = 5);
    let expected = *parse_expression("5").unwrap().into_syntax().body;
    assert_eq!(ComparableExpr::from(&expr), ComparableExpr::from(&expected));
}

#[test]
fn inserts_bool_literal() {
    let expr = py_expr!("{b:literal}", b = true);
    let expected = *parse_expression("True").unwrap().into_syntax().body;
    assert_eq!(ComparableExpr::from(&expr), ComparableExpr::from(&expected));

    let expr = py_expr!("{b:literal}", b = false);
    let expected = *parse_expression("False").unwrap().into_syntax().body;
    assert_eq!(ComparableExpr::from(&expr), ComparableExpr::from(&expected));
}

#[test]
fn inserts_stmt() {
    let body = parse_module(
        "
a = 1
b = 2
",
    )
    .unwrap()
    .into_syntax()
    .body;

    let actual = py_stmts!("{body:stmt}", body = body.clone());
    let expected = parse_module(
        "
a = 1
b = 2
",
    )
    .unwrap()
    .into_syntax()
    .body;
    let actual_cmp = actual.iter().map(ComparableStmt::from).collect::<Vec<_>>();
    let expected_cmp = expected
        .iter()
        .map(ComparableStmt::from)
        .collect::<Vec<_>>();
    assert_eq!(actual_cmp, expected_cmp);
}

#[test]
fn inserts_boxed_stmt() {
    let mut body = parse_module("a = 1").unwrap().into_syntax().body;
    let stmt = body.pop().unwrap();
    let actual = py_stmt!("{body:stmt}", body = stmt.clone());
    let expected = py_stmt!("{body:stmt}", body = vec![stmt]);
    assert_ast_eq(actual, expected);
}

#[test]
fn inserts_stmt_from_iterator() {
    let body = parse_module(
        "
a = 1
b = 2
",
    )
    .unwrap()
    .into_syntax()
    .body;
    let iter_body = body.iter().cloned().collect::<Vec<_>>();

    let actual = py_stmts!("{body:stmt}", body = iter_body.into_iter());
    let expected = parse_module(
        "
a = 1
b = 2
",
    )
    .unwrap()
    .into_syntax()
    .body;
    let actual_cmp = actual.iter().map(ComparableStmt::from).collect::<Vec<_>>();
    let expected_cmp = expected
        .iter()
        .map(ComparableStmt::from)
        .collect::<Vec<_>>();
    assert_eq!(actual_cmp, expected_cmp);
}

#[test]
fn inserts_pass_from_empty_stmt_iterator() {
    let actual = py_stmts!("{body:stmt}", body = Vec::<Stmt>::new().into_iter(),);
    let expected: Vec<Stmt> = vec![];
    let actual_cmp = actual.iter().map(ComparableStmt::from).collect::<Vec<_>>();
    let expected_cmp = expected
        .iter()
        .map(ComparableStmt::from)
        .collect::<Vec<_>>();
    assert_eq!(actual_cmp, expected_cmp);
}

#[test]
fn wraps_expr_in_stmt() {
    let expr = *parse_expression("a + 1").unwrap().into_syntax().body;
    let actual = py_stmt!(
        "
{expr:stmt}
",
        expr = expr,
    );
    let mut body = vec![actual];
    flatten(&mut body);
    assert_ast_eq(
        body.first()
            .expect("expected single statement after flatten")
            .clone(),
        py_stmt!(
            "
a + 1
",
        ),
    );
}

#[test]
fn inserts_function_parts() {
    let body = parse_module("a = 1").unwrap().into_syntax().body;
    let stmt = py_stmt!(
        "
def {func:id}({param:id}):
    {body:stmt}
",
        func = "foo",
        param = "arg",
        body = body.clone(),
    );
    match stmt {
        ruff_python_ast::Stmt::FunctionDef(ast::StmtFunctionDef {
            name,
            parameters,
            body: mut fn_body,
            ..
        }) => {
            flatten(&mut fn_body);
            assert_eq!(name.id.as_str(), "foo");
            assert_eq!(parameters.args[0].parameter.name.id.as_str(), "arg");
            assert_eq!(
                ComparableStmt::from(&fn_body[0]),
                ComparableStmt::from(&body[0])
            );
        }
        _ => panic!("expected function def"),
    }
}

#[test]
fn inserts_dict_placeholder() {
    let entries = vec![
        ("a".to_string(), py_expr!("1")),
        ("b".to_string(), py_expr!("x")),
    ];
    let expr = py_expr!("{entries:dict}", entries = entries);
    let expected = *parse_expression("{'a': 1, 'b': x}")
        .unwrap()
        .into_syntax()
        .body;
    assert_eq!(ComparableExpr::from(&expr), ComparableExpr::from(&expected));
}

#[test]
fn inserts_boxed_expr() {
    let expr = *parse_expression("a + 1").unwrap().into_syntax().body;
    let actual = py_stmt!("return {expr:expr}", expr = Box::new(expr.clone()));
    let expected = py_stmt!("return {expr:expr}", expr = expr);
    assert_ast_eq(actual, expected);
}

#[test]
fn reports_missing_and_unused_placeholders_together() {
    let result = std::panic::catch_unwind(|| {
        let _ = py_stmt!("{missing:id}", unused = "x");
    });
    let err = result.expect_err("expected template instantiation to panic");
    let msg = err
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| err.downcast_ref::<&str>().copied())
        .unwrap_or("<non-string panic>");
    assert!(msg.contains("expected id or literal for placeholder missing"));
    assert!(msg.contains("unused ids: unused"));
}

#[test]
fn reports_py_stmt_multi_statement_template_context() {
    let expected_file = file!();
    let expected_line = line!() + 2;
    let result = std::panic::catch_unwind(|| {
        let _ = py_stmt!("a = 1\nb = 2");
    });
    let err = result.expect_err("expected multi-statement py_stmt template to panic");
    let msg = err
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| err.downcast_ref::<&str>().copied())
        .unwrap_or("<non-string panic>");

    assert!(msg.contains("must produce exactly one statement, got 2"));
    assert!(msg.contains(&format!("{expected_file}:{expected_line}:")));
    assert!(msg.contains("template:\na = 1\nb = 2"));
    assert!(msg.contains("expanded statements:\na = 1\nb = 2"));
}
