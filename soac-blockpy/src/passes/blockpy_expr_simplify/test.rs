use super::*;

use crate::block_py::{BinOpKind, Literal, NameLike, UnaryOpKind};

fn lower_semantic_expr_without_setup(expr: &Expr) -> InstrWithAwaitAndYield {
    InstrWithAwaitAndYield::from_ast_expr(expr.clone())
}

use crate::block_py::{CallArgKeyword, CallArgPositional, InstrWithAwaitAndYield};
use crate::lower_python_to_blockpy_for_testing;
use crate::py_expr;
use ruff_python_parser::parse_expression;

fn is_raw_load_name_expr(expr: &InstrWithAwaitAndYield, expected: &str) -> bool {
    matches!(
        expr,
        InstrWithAwaitAndYield::Load(op) if op.name.id_str() == expected
    )
}

#[test]
fn expr_simplify_preserves_control_flow_but_reduces_exprs() {
    let source = r#"
def f(x):
    if x:
        return 1
    return 2
"#;
    let core = lower_python_to_blockpy_for_testing(source)
        .unwrap()
        .pass_tracker
        .pass_core_blockpy_with_await_and_yield()
        .cloned()
        .expect("expected lowered core BlockPy module");
    let core_rendered = crate::block_py::pretty::blockpy_module_to_string(&core);

    assert!(core_rendered.contains("function f(x):"));
    assert!(core_rendered.contains("return 1"));
}

#[test]
fn expr_simplify_recurses_bottom_up_for_operator_family() {
    let expr = Expr::from(py_expr!("-(x + 1)"));
    let lowered = lower_semantic_expr_without_setup(&expr);

    let InstrWithAwaitAndYield::UnaryOp(op) = lowered else {
        panic!("expected unary-op core expr");
    };
    assert_eq!(op.kind, UnaryOpKind::Neg);
    let InstrWithAwaitAndYield::BinOp(inner) = op.operand.as_ref() else {
        panic!("expected __dp_neg to receive one lowered op arg");
    };
    assert_eq!(inner.kind, BinOpKind::Add);
}

#[test]
fn core_blockpy_expr_uses_reduced_variants_for_simple_shapes() {
    assert!(is_raw_load_name_expr(
        &InstrWithAwaitAndYield::from_ast_expr(py_expr!("x")),
        "x"
    ));
    assert!(matches!(
        InstrWithAwaitAndYield::from_ast_expr(py_expr!("1")),
        InstrWithAwaitAndYield::Literal(literal)
            if matches!(literal.as_literal(), Literal::NumberLiteral(_))
    ));
    assert!(matches!(
        InstrWithAwaitAndYield::from_ast_expr(py_expr!("f(x)")),
        InstrWithAwaitAndYield::Call(_)
    ));
    assert!(matches!(
        InstrWithAwaitAndYield::from_ast_expr(py_expr!("await f(x)")),
        InstrWithAwaitAndYield::Await(_)
    ));
    assert!(matches!(
        InstrWithAwaitAndYield::from_ast_expr(py_expr!("yield x")),
        InstrWithAwaitAndYield::Yield(_)
    ));
    assert!(matches!(
        InstrWithAwaitAndYield::from_ast_expr(py_expr!("yield from xs")),
        InstrWithAwaitAndYield::YieldFrom(_)
    ));
}

#[test]
fn core_blockpy_call_supports_star_args_and_kwargs() {
    let InstrWithAwaitAndYield::Call(call) =
        InstrWithAwaitAndYield::from_ast_expr(py_expr!("f(x, *args, y=z, **kw)"))
    else {
        panic!("expected reduced call expr");
    };
    assert!(is_raw_load_name_expr(call.func.as_ref(), "f"));
    assert_eq!(call.args.len(), 2);
    assert!(matches!(call.args[0], CallArgPositional::Positional(_)));
    assert!(matches!(call.args[1], CallArgPositional::Starred(_)));
    assert_eq!(call.keywords.len(), 2);
    assert!(matches!(
        &call.keywords[0],
        CallArgKeyword::Named { arg, .. } if arg.as_str() == "y"
    ));
    assert!(matches!(call.keywords[1], CallArgKeyword::Starred(_)));
}

#[test]
fn core_blockpy_expr_reduces_add_to_structured_intrinsic() {
    let parsed = *parse_expression("x + y").unwrap().into_syntax().body;
    let InstrWithAwaitAndYield::BinOp(op) = InstrWithAwaitAndYield::from_ast_expr(parsed) else {
        panic!("expected operation-shaped reduced expr for x + y");
    };
    assert_eq!(op.kind, BinOpKind::Add);
}

#[test]
fn core_blockpy_expr_reduces_operator_helper_families_to_intrinsics() {
    for expr in ["obj.attr", "obj[idx]", "-x", "x < y", "x in y", "x is y"] {
        let parsed = *parse_expression(expr).unwrap().into_syntax().body;
        let lowered = InstrWithAwaitAndYield::from_ast_expr(parsed);
        let matches_expected = match (&*expr, &lowered) {
            ("obj.attr", InstrWithAwaitAndYield::GetAttr(_)) => true,
            ("obj[idx]", InstrWithAwaitAndYield::GetItem(_)) => true,
            ("-x", InstrWithAwaitAndYield::UnaryOp(op)) if op.kind == UnaryOpKind::Neg => true,
            ("x < y", InstrWithAwaitAndYield::BinOp(op)) if op.kind == BinOpKind::Lt => true,
            ("x in y", InstrWithAwaitAndYield::BinOp(op)) if op.kind == BinOpKind::Contains => true,
            ("x is y", InstrWithAwaitAndYield::BinOp(op)) if op.kind == BinOpKind::Is => true,
            _ => false,
        };
        assert!(matches_expected, "{lowered:?}");
    }
}

#[test]
fn core_blockpy_expr_keeps_non_intrinsic_helper_families_as_named_calls() {
    for (expr, helper_name) in [
        ("(x, y)", "tuple_values"),
        ("[x, y]", "list"),
        ("{x, y}", "set"),
        ("{x: y}", "dict"),
    ] {
        let parsed = *parse_expression(expr).unwrap().into_syntax().body;
        let InstrWithAwaitAndYield::Call(call) = InstrWithAwaitAndYield::from_ast_expr(parsed)
        else {
            panic!("expected call-shaped reduced expr for {expr}");
        };
        assert!(
            matches!(
                &*call.func,
                InstrWithAwaitAndYield::Load(op)
                    if op.name.is_runtime_symbol(helper_name)
            ),
            "{call:?}",
        );
    }
}

#[test]
fn core_blockpy_expr_reuses_shared_tuple_splat_intrinsic_shape() {
    let parsed = *parse_expression("(x, *xs, y)").unwrap().into_syntax().body;
    let lowered = InstrWithAwaitAndYield::from_ast_expr(parsed);
    let InstrWithAwaitAndYield::BinOp(op) = &lowered else {
        panic!("expected operation-shaped reduced tuple expr");
    };
    assert_eq!(op.kind, BinOpKind::Add);
    let rendered = format!("{lowered:?}");
    assert!(rendered.contains("tuple_from_iter(xs)"), "{rendered}");
}

#[test]
fn core_blockpy_expr_reuses_shared_tuple_splat_for_list_and_set() {
    for (expr, intrinsic) in [("[x, *xs, y]", "list"), ("{x, *xs, y}", "set")] {
        let parsed = *parse_expression(expr).unwrap().into_syntax().body;
        let InstrWithAwaitAndYield::Call(call) = InstrWithAwaitAndYield::from_ast_expr(parsed)
        else {
            panic!("expected call-shaped reduced expr for {expr}");
        };
        assert!(matches!(
            &*call.func,
            InstrWithAwaitAndYield::Load(op)
                if op.name.is_runtime_symbol(intrinsic)
        ));
        let [CallArgPositional::Positional(tupleish)] = &call.args[..] else {
            panic!("expected one positional arg for {expr}");
        };
        let rendered = format!("{tupleish:?}");
        assert!(rendered.contains("tuple_from_iter(xs)"), "{rendered}");
    }
}

#[test]
fn helper_scoped_families_do_not_reach_core_blockpy_boundary() {
    for expr in [
        "(lambda x: x + 1)",
        "[x for x in xs]",
        "{x for x in xs}",
        "{x: y for x, y in pairs}",
        "(x for x in xs)",
    ] {
        let parsed = *parse_expression(expr).unwrap().into_syntax().body;
        let panic = std::panic::catch_unwind(|| InstrWithAwaitAndYield::from_ast_expr(parsed));
        assert!(
            panic.is_err(),
            "{expr} should be lowered before the core boundary"
        );
    }
}

#[test]
#[should_panic(expected = "IpyEscapeCommand should not reach late core BlockPy boundary")]
fn ipy_escape_command_does_not_reach_core_blockpy_boundary() {
    let expr = Expr::IpyEscapeCommand(ast::ExprIpyEscapeCommand {
        node_index: ast::AtomicNodeIndex::default(),
        range: ruff_text_size::TextRange::default(),
        kind: ast::IpyEscapeKind::Shell,
        value: "ls".into(),
    });
    let _ = InstrWithAwaitAndYield::from_ast_expr(expr);
}

#[test]
fn core_blockpy_keeps_function_defaults_out_of_blockpy_ir() {
    let source = r#"
def f(*, d={"metaclass": Meta}, **kw):
    return d
"#;
    let blockpy = lower_python_to_blockpy_for_testing(source)
        .unwrap()
        .pass_tracker
        .pass_core_blockpy_with_await_and_yield()
        .cloned()
        .expect("expected lowered core BlockPy module");
    let rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);

    assert!(rendered.contains("function f(*, d, **kw):"), "{rendered}");
    assert!(!rendered.contains("function f(*, d={"), "{rendered}");
    assert!(rendered.contains("MakeFunction("), "{rendered}");
}
