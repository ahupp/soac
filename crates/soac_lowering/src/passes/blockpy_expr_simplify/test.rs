use super::*;

use crate::block_py::{BinOpKind, BlockTerm, Literal, NameLike, UnaryOpKind};
use crate::pass_tracker::LoweringPassTrackerInternalExt;

fn lower_semantic_expr_without_setup(expr: &Expr) -> InstrWithAwaitAndYield {
    InstrWithAwaitAndYield::from_ast_expr(expr.clone())
}

use crate::block_py::{CallArgKeyword, CallArgPositional, InstrWithAwaitAndYield};
use crate::lower_python_to_blockpy_for_testing;
use crate::template::py_expr;
use ruff_python_parser::parse_expression;

type OperandMove = (String, crate::block_py::InstrId, ruff_text_size::TextRange);

fn ruff_expr_with_operand_moves(source: &str, names: &[&str]) -> (InstrRuff, Vec<OperandMove>) {
    use crate::block_py::{ChildVisitable, InstrId, TakeOperand, VisitMut};

    struct MoveNames<'a> {
        names: &'a [&'a str],
        moves: Vec<OperandMove>,
    }
    impl VisitMut<InstrRuff> for MoveNames<'_> {
        fn visit_instr_mut(&mut self, instr: &mut InstrRuff) {
            if let InstrRuff::ExprName(node) = instr {
                if self.names.contains(&node.id.as_str()) {
                    let id = InstrId::new(u32::try_from(self.moves.len()).unwrap() + 1);
                    let meta = Meta {
                        instr_id: Some(id),
                        ..node.meta()
                    };
                    self.moves.push((node.id.to_string(), id, meta.range));
                    *instr =
                        InstrRuff::TakeOperand(TakeOperand::new(node.id.clone()).with_meta(meta));
                    return;
                }
            }
            instr.visit_children_mut(self);
        }
    }

    let mut expr = crate::passes::ast_to_instr::from_ast_expr(
        *parse_expression(source).unwrap().into_syntax().body,
    );
    let mut visitor = MoveNames {
        names,
        moves: vec![],
    };
    visitor.visit_instr_mut(&mut expr);
    let meta = Meta {
        instr_id: Some(InstrId::new(1000)),
        ..expr.meta()
    };
    (expr.with_meta(meta), visitor.moves)
}

fn core_operand_moves(expr: &InstrWithAwaitAndYield) -> Vec<OperandMove> {
    use crate::block_py::{ChildVisitable, Visit};

    struct Collect(Vec<OperandMove>);
    impl Visit<InstrWithAwaitAndYield> for Collect {
        fn visit_instr(&mut self, instr: &InstrWithAwaitAndYield) {
            if let InstrWithAwaitAndYield::TakeOperand(node) = instr {
                let meta = node.meta();
                self.0.push((
                    node.name.id_str().to_owned(),
                    meta.instr_id
                        .expect("operand move metadata must survive the bridge"),
                    meta.range,
                ));
            }
            instr.visit_children(self);
        }
    }
    let mut collector = Collect(vec![]);
    collector.visit_instr(expr);
    collector.0
}

fn assert_operand_moves(expr: &InstrWithAwaitAndYield, expected: &[OperandMove], names: &[&str]) {
    assert_eq!(core_operand_moves(expr), expected);
    assert_eq!(
        expected
            .iter()
            .map(|(name, _, _)| name.as_str())
            .collect::<Vec<_>>(),
        names,
    );
}

fn runtime_call<'a>(
    expr: &'a InstrWithAwaitAndYield,
    name: &str,
) -> &'a crate::block_py::Call<InstrWithAwaitAndYield> {
    let InstrWithAwaitAndYield::Call(call) = expr else {
        panic!("expected the existing runtime constructor call");
    };
    assert!(
        matches!(call.func.as_ref(), InstrWithAwaitAndYield::Load(load)
        if load.name.is_runtime_symbol(name))
    );
    call
}

#[test]
fn ruff_core_bridge_preserves_take_call_children_and_frame_namespace() {
    use crate::block_py::{FrameNamespace, InstrId};

    let names = ["callee", "pos", "spread", "named_value", "mapping"];
    for module_namespace in [false, true] {
        let (expr, expected) = ruff_expr_with_operand_moves(
            "callee(pos, *spread, named=named_value, **mapping)",
            &names,
        );
        let InstrRuff::Call(mut call) = expr else {
            panic!("expected call input")
        };
        let range = call.meta().range;
        call.frame_namespace = Some(if module_namespace {
            FrameNamespace::ModuleGlobals
        } else {
            FrameNamespace::Mapping(Box::new(crate::passes::ast_to_instr::from_ast_expr(
                py_expr!("namespace"),
            )))
        });
        let lowered = InstrWithAwaitAndYield::from_ruff_expr(InstrRuff::Call(call));
        assert_operand_moves(&lowered, &expected, &names);
        let InstrWithAwaitAndYield::Call(call) = lowered else {
            panic!("call shape must survive")
        };
        assert_eq!(call.meta().instr_id, Some(InstrId::new(1000)));
        assert_eq!(call.meta().range, range);
        assert!(matches!(
            call.args.as_slice(),
            [
                CallArgPositional::Positional(_),
                CallArgPositional::Starred(_)
            ]
        ));
        assert!(
            matches!(call.keywords.as_slice(), [CallArgKeyword::Named { arg, .. }, CallArgKeyword::Starred(_)]
            if arg.as_str() == "named")
        );
        match call.frame_namespace.unwrap() {
            FrameNamespace::ModuleGlobals => assert!(module_namespace),
            FrameNamespace::Mapping(value) => {
                assert!(!module_namespace);
                assert!(is_raw_load_name_expr(&value, "namespace"));
            }
        }
    }
}

#[test]
fn ruff_core_bridge_preserves_take_container_and_splat_shapes() {
    use crate::block_py::InstrId;

    for (source, constructor, splatted) in [
        ("(head, tail)", None, false),
        ("[head, tail]", Some("list"), false),
        ("{head, tail}", Some("set"), false),
        ("(head, *spread, tail)", None, true),
        ("[head, *spread, tail]", Some("list"), true),
        ("{head, *spread, tail}", Some("set"), true),
    ] {
        let names: &[&str] = if splatted {
            &["head", "spread", "tail"]
        } else {
            &["head", "tail"]
        };
        let (expr, expected) = ruff_expr_with_operand_moves(source, names);
        let range = expr.meta().range;
        let lowered = InstrWithAwaitAndYield::from_ruff_expr(expr);
        assert_operand_moves(&lowered, &expected, names);
        assert_eq!(lowered.meta().instr_id, Some(InstrId::new(1000)));
        assert_eq!(lowered.meta().range, range);
        let tuple = if let Some(constructor) = constructor {
            let call = runtime_call(&lowered, constructor);
            let [CallArgPositional::Positional(tuple)] = call.args.as_slice() else {
                panic!("list/set still receive exactly one tuple-shaped operand");
            };
            tuple
        } else {
            &lowered
        };
        if splatted {
            let InstrWithAwaitAndYield::BinOp(tail) = tuple else {
                panic!("expected segment concatenation")
            };
            assert_eq!(tail.kind, BinOpKind::Add);
            let InstrWithAwaitAndYield::BinOp(head) = tail.left.as_ref() else {
                panic!("expected left-associated segments")
            };
            assert_eq!(head.kind, BinOpKind::Add);
            assert!(
                matches!(head.left.as_ref(), InstrWithAwaitAndYield::Tuple(tuple) if tuple.values.len() == 1)
            );
            let spread = runtime_call(&head.right, "tuple_from_iter");
            assert!(matches!(
                spread.args.as_slice(),
                [CallArgPositional::Positional(
                    InstrWithAwaitAndYield::TakeOperand(_)
                )]
            ));
            assert!(
                matches!(tail.right.as_ref(), InstrWithAwaitAndYield::Tuple(tuple) if tuple.values.len() == 1)
            );
        } else {
            assert!(
                matches!(tuple, InstrWithAwaitAndYield::Tuple(tuple) if tuple.values.len() == 2)
            );
        }
    }
}

#[test]
fn ruff_core_bridge_preserves_take_slice_bounds_and_missing_defaults() {
    use crate::block_py::InstrId;

    for (source, names) in [
        ("owner[lower:upper:step]", &["lower", "upper", "step"][..]),
        ("owner[:upper:]", &["upper"][..]),
    ] {
        let (expr, expected) = ruff_expr_with_operand_moves(source, names);
        let InstrRuff::ExprSubscript(node) = expr else {
            panic!("expected subscript input")
        };
        let slice = *node.slice;
        let meta = Meta {
            instr_id: Some(InstrId::new(1000)),
            ..slice.meta()
        };
        let lowered = InstrWithAwaitAndYield::from_ruff_expr(slice.with_meta(meta.clone()));
        assert_operand_moves(&lowered, &expected, names);
        let call = runtime_call(&lowered, "slice");
        assert_eq!(call.meta().instr_id, meta.instr_id);
        assert_eq!(call.meta().range, meta.range);
        assert_eq!(call.args.len(), 3);
        if names.len() == 1 {
            for bound in [&call.args[0], &call.args[2]] {
                assert!(matches!(bound.expr(), InstrWithAwaitAndYield::Load(load)
                    if load.name.is_runtime_symbol("NONE")));
            }
        }
    }
}

#[test]
fn ruff_core_bridge_preserves_take_dict_pairs_and_unpack_segments() {
    let names = ["key", "value", "mapping", "tail_key", "tail_value"];
    let (expr, expected) =
        ruff_expr_with_operand_moves("{key: value, **mapping, tail_key: tail_value}", &names);
    let meta = expr.meta();
    let lowered = InstrWithAwaitAndYield::from_ruff_expr(expr);
    assert_operand_moves(&lowered, &expected, &names);
    assert_eq!(lowered.meta().instr_id, meta.instr_id);
    assert_eq!(lowered.meta().range, meta.range);
    let InstrWithAwaitAndYield::BinOp(tail) = &lowered else {
        panic!("expected dictionary segment union")
    };
    assert_eq!(tail.kind, BinOpKind::Or);
    let InstrWithAwaitAndYield::BinOp(head) = tail.left.as_ref() else {
        panic!("expected left-associated dictionary segments")
    };
    assert_eq!(head.kind, BinOpKind::Or);
    for segment in [&head.left, &tail.right] {
        let call = runtime_call(segment, "dict");
        let [CallArgPositional::Positional(InstrWithAwaitAndYield::Tuple(pairs))] =
            call.args.as_slice()
        else {
            panic!("keyed dictionaries still construct from a tuple of pairs, not MAP_ADD");
        };
        assert!(
            matches!(pairs.values.as_slice(), [InstrWithAwaitAndYield::Tuple(pair)] if pair.values.len() == 2)
        );
    }
    let mapping = runtime_call(&head.right, "dict");
    assert!(matches!(
        mapping.args.as_slice(),
        [CallArgPositional::Positional(
            InstrWithAwaitAndYield::TakeOperand(_)
        )]
    ));

    for (source, names, keyed_pairs) in [
        ("{}", &[][..], Some(0)),
        (
            "{first: second, third: fourth}",
            &["first", "second", "third", "fourth"][..],
            Some(2),
        ),
        ("{**mapping}", &["mapping"][..], None),
    ] {
        let (expr, expected) = ruff_expr_with_operand_moves(source, names);
        let lowered = InstrWithAwaitAndYield::from_ruff_expr(expr);
        assert_operand_moves(&lowered, &expected, names);
        let call = runtime_call(&lowered, "dict");
        match keyed_pairs {
            Some(0) => assert!(call.args.is_empty()),
            Some(count) => {
                let [CallArgPositional::Positional(InstrWithAwaitAndYield::Tuple(pairs))] =
                    call.args.as_slice()
                else {
                    panic!("expected the existing tuple-pairs constructor");
                };
                assert_eq!(pairs.values.len(), count);
                assert!(pairs.values.iter().all(|pair| matches!(pair, InstrWithAwaitAndYield::Tuple(pair) if pair.values.len() == 2)));
            }
            None => assert!(matches!(
                call.args.as_slice(),
                [CallArgPositional::Positional(
                    InstrWithAwaitAndYield::TakeOperand(_)
                )]
            )),
        }
    }
}

#[test]
fn ruff_core_bridge_keeps_helper_extraction_and_callable_admission_boundaries() {
    use crate::block_py::FrameNamespace;

    let cell_ref =
        crate::passes::ast_to_instr::from_ast_expr(py_expr!("__soac__.cell_ref('cell')"));
    assert!(matches!(
        InstrWithAwaitAndYield::from_ruff_expr(cell_ref),
        InstrWithAwaitAndYield::CellRefForName(_)
    ));

    let (expr, expected) = ruff_expr_with_operand_moves(
        "__soac__.store_global(globals, 'target', value)",
        &["value"],
    );
    let lowered = InstrWithAwaitAndYield::from_ruff_expr(expr.clone());
    assert_operand_moves(&lowered, &expected, &["value"]);
    assert!(matches!(lowered, InstrWithAwaitAndYield::Store(store)
        if store.name.id_str() == "target" && matches!(store.value.as_ref(), InstrWithAwaitAndYield::TakeOperand(_))));

    let InstrRuff::Call(mut call) = expr else {
        panic!("expected helper call input")
    };
    call.frame_namespace = Some(FrameNamespace::ModuleGlobals);
    let lowered = InstrWithAwaitAndYield::from_ruff_expr(InstrRuff::Call(call));
    assert_operand_moves(&lowered, &expected, &["value"]);
    let call = runtime_call(&lowered, "store_global");
    assert_eq!(
        call.args.len(),
        3,
        "explicit namespace metadata cannot disappear during extraction"
    );
    assert!(matches!(
        call.frame_namespace,
        Some(FrameNamespace::ModuleGlobals)
    ));

    let names = ["captures", "defaults", "annotations"];
    let (expr, expected) = ruff_expr_with_operand_moves(
        "__soac__.make_function(7, 'function', captures, defaults, annotations)",
        &names,
    );
    let lowered = InstrWithAwaitAndYield::from_ruff_expr(expr);
    assert_operand_moves(&lowered, &expected, &names);
    let call = runtime_call(&lowered, "make_function");
    assert_eq!(
        call.args.len(),
        5,
        "source helper spelling cannot mint callable authority"
    );
}

fn is_raw_load_name_expr(expr: &InstrWithAwaitAndYield, expected: &str) -> bool {
    matches!(
        expr,
        InstrWithAwaitAndYield::Load(op) if op.name.id_str() == expected
    )
}

fn contains_runtime_call(expr: &InstrWithAwaitAndYield, expected: &str) -> bool {
    match expr {
        InstrWithAwaitAndYield::Load(op) => op.name.is_runtime_symbol(expected),
        InstrWithAwaitAndYield::Call(call) => {
            contains_runtime_call(call.func.as_ref(), expected)
                || call
                    .args
                    .iter()
                    .any(|arg| contains_runtime_call(arg.expr(), expected))
                || call
                    .keywords
                    .iter()
                    .any(|keyword| contains_runtime_call(keyword.expr(), expected))
        }
        InstrWithAwaitAndYield::BinOp(op) => {
            contains_runtime_call(op.left.as_ref(), expected)
                || contains_runtime_call(op.right.as_ref(), expected)
        }
        _ => false,
    }
}

#[test]
fn raw_make_function_call_cannot_mint_an_intrinsic_or_discard_operands() {
    let parsed = *parse_expression(
        "__soac__.make_function(7, 'function', captures(), defaults(), annotations())",
    )
    .unwrap()
    .into_syntax()
    .body;
    let InstrWithAwaitAndYield::Call(call) = InstrWithAwaitAndYield::from_ast_expr(parsed) else {
        panic!("source spelling cannot authorize MakeFunction");
    };
    assert_eq!(call.args.len(), 5);
    for (operand, name) in call.args[2..]
        .iter()
        .zip(["captures", "defaults", "annotations"])
    {
        let InstrWithAwaitAndYield::Call(operand) = operand.expr() else {
            panic!("ordinary helper arguments must still be evaluated");
        };
        assert!(
            matches!(operand.func.as_ref(), InstrWithAwaitAndYield::Load(load)
            if load.name.id_str() == name)
        );
    }
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
    let function = core
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "f")
        .expect("missing lowered f callable");

    assert!(function
        .blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::IfTerm(_))));
    assert!(function
        .blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::Return(_))));
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
fn core_blockpy_expr_reduces_complex_literal_to_runtime_constructor() {
    let lowered = InstrWithAwaitAndYield::from_ast_expr(py_expr!("1j"));

    assert!(contains_runtime_call(&lowered, "complex_from_parts"));
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
fn membership_ir_preserves_source_operand_evaluation_order() {
    for (source, negated) in [
        ("needle() in container()", false),
        ("needle() not in container()", true),
    ] {
        let parsed = *parse_expression(source).unwrap().into_syntax().body;
        let mut lowered = InstrWithAwaitAndYield::from_ast_expr(parsed);
        if negated {
            let InstrWithAwaitAndYield::UnaryOp(op) = lowered else {
                panic!("not-in negates the membership result");
            };
            assert_eq!(op.kind, UnaryOpKind::Not);
            lowered = *op.operand;
        }
        let InstrWithAwaitAndYield::BinOp(op) = lowered else {
            panic!("membership lowers to a binary operation");
        };
        assert_eq!(op.kind, BinOpKind::Contains);
        for (operand, target) in [(&op.left, "needle"), (&op.right, "container")] {
            let InstrWithAwaitAndYield::Call(call) = operand.as_ref() else {
                panic!("source operand retains its single callback");
            };
            assert!(is_raw_load_name_expr(call.func.as_ref(), target));
        }
    }
}

#[test]
fn core_blockpy_expr_keeps_non_intrinsic_helper_families_as_named_calls() {
    for (expr, helper_name) in [("[x, y]", "list"), ("{x, y}", "set"), ("{x: y}", "dict")] {
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
fn core_blockpy_expr_reduces_tuple_literal_to_tuple_instruction() {
    let parsed = *parse_expression("(x, y)").unwrap().into_syntax().body;
    let InstrWithAwaitAndYield::Tuple(tuple) = InstrWithAwaitAndYield::from_ast_expr(parsed) else {
        panic!("expected tuple instruction for tuple literal");
    };
    assert_eq!(tuple.values.len(), 2);
}

#[test]
fn core_blockpy_expr_reuses_shared_tuple_splat_intrinsic_shape() {
    let parsed = *parse_expression("(x, *xs, y)").unwrap().into_syntax().body;
    let lowered = InstrWithAwaitAndYield::from_ast_expr(parsed);
    let InstrWithAwaitAndYield::BinOp(op) = &lowered else {
        panic!("expected operation-shaped reduced tuple expr");
    };
    assert_eq!(op.kind, BinOpKind::Add);
    assert!(contains_runtime_call(&lowered, "tuple_from_iter"));
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
        assert!(contains_runtime_call(tupleish, "tuple_from_iter"));
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
    let function = blockpy
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "f")
        .expect("missing lowered f callable");
    let module_init = blockpy
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "_dp_module_init")
        .expect("missing lowered module-init callable");

    assert_eq!(
        function.params.names(),
        vec!["d".to_string(), "kw".to_string()]
    );
    assert_eq!(function.params.default_count(), 1);
    assert!(module_init.blocks.iter().any(|block| {
        block.body.iter().any(|stmt| {
            matches!(
                stmt,
                InstrWithAwaitAndYield::Store(store)
                    if matches!(store.value.as_ref(), InstrWithAwaitAndYield::MakeFunction(_))
            )
        })
    }));
}
