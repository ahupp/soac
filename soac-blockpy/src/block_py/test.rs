use super::*;
use crate::passes::{CoreModuleShape, InstrRuff};
use crate::py_expr;

#[test]
fn cfg_block_new_sets_explicit_term() {
    let block = Block::new(
        BlockLabel::from_index(0),
        vec![crate::passes::ast_to_instr::from_ast_expr(py_expr!("x"))],
        BlockTerm::<InstrRuff>::Jump(crate::block_py::BlockEdge::new(BlockLabel::from_index(1))),
        Vec::new(),
        None,
    );

    assert_eq!(block.body.len(), 1);
    assert!(matches!(block.body[0], InstrRuff::ExprName(_)));
    assert!(matches!(block.term, BlockTerm::Jump(_)));
}

#[test]
fn cfg_block_from_fragment_without_term_uses_implicit_none_return_value() {
    let block = Block::from_builder(
        BlockLabel::from_index(0),
        BlockBuilder::from_stmts(vec![crate::passes::ast_to_instr::from_ast_expr(py_expr!(
            "x"
        ))]),
        Vec::new(),
        None,
        None,
    );

    assert_eq!(block.body.len(), 1);
    assert!(matches!(
        &block.term,
        BlockTerm::Return(InstrRuff::ExprNoneLiteral(_))
    ));
}

#[test]
fn block_label_fallthrough_is_distinct() {
    assert!(BlockLabel::fallthrough().is_fallthrough());
    assert_eq!(BlockLabel::fallthrough().to_string(), "<fallthrough>");
    assert_ne!(BlockLabel::fallthrough(), BlockLabel::from_index(0));
}

#[test]
fn cfg_block_can_replace_fallthrough_target() {
    let mut block = Block::new(
        BlockLabel::from_index(0),
        Vec::<InstrRuff>::new(),
        BlockTerm::<InstrRuff>::Jump(crate::block_py::BlockEdge::new(BlockLabel::fallthrough())),
        Vec::new(),
        None,
    );

    assert!(block.replace_fallthrough_target(BlockLabel::from_index(7)));
    let BlockTerm::Jump(edge) = &block.term else {
        panic!("expected jump term");
    };
    assert_eq!(edge.target, BlockLabel::from_index(7));
}

#[test]
fn stmt_fragment_can_carry_optional_term() {
    let fragment: BlockBuilder<InstrRuff> = BlockBuilder::with_term(
        vec![crate::passes::ast_to_instr::from_ast_expr(py_expr!("x"))],
        Some(BlockTerm::Return(
            crate::passes::ast_to_instr::from_ast_expr(py_expr!("None")),
        )),
    );

    assert_eq!(fragment.body.len(), 1);
    assert!(matches!(fragment.body[0], InstrRuff::ExprName(_)));
    assert!(matches!(fragment.term, Some(BlockTerm::Return(_))));
}

#[test]
fn core_blockpy_expr_wraps_name_expr() {
    let expr = InstrWithAwaitAndYield::from_ast_expr(py_expr!("y"));

    assert!(matches!(
        expr,
        InstrWithAwaitAndYield::Load(op)
            if op.name.id_str() == "y"
    ));
}

#[test]
fn call_and_keyword_arg_expr_helpers_preserve_shape() {
    let mut positional = CallArgPositional::Positional(py_expr!("x"));
    *positional.expr_mut() = py_expr!("y");
    assert!(matches!(
        positional,
        CallArgPositional::Positional(Expr::Name(name)) if name.id.as_str() == "y"
    ));

    let starred = CallArgPositional::Starred(py_expr!("z")).map_instr(|expr| {
        let Expr::Name(name) = expr else {
            panic!("expected name expr");
        };
        Expr::Name(name)
    });
    assert!(matches!(starred, CallArgPositional::Starred(_)));

    let keyword = CallArgKeyword::Named {
        arg: ast::Identifier::new("value", ruff_text_size::TextRange::default()).into(),
        value: py_expr!("a"),
    }
    .try_map_instr(|expr| -> Result<Expr, &'static str> {
        let Expr::Name(name) = expr else {
            return Err("expected name expr");
        };
        Ok(Expr::Name(name))
    })
    .expect("keyword arg mapping should succeed");
    assert!(matches!(
        keyword,
        CallArgKeyword::Named { arg, value: Expr::Name(_), .. } if arg.as_str() == "value"
    ));
}

#[test]
fn ast_call_arg_helpers_preserve_star_shapes() {
    let positional = CallArgPositional::from_ast_expr_with(py_expr!("x"), |expr| expr);
    assert!(matches!(
        positional,
        CallArgPositional::Positional(Expr::Name(name)) if name.id.as_str() == "x"
    ));

    let starred = CallArgPositional::from_ast_expr_with(py_expr!("*xs"), |expr| expr);
    assert!(matches!(
        starred,
        CallArgPositional::Starred(Expr::Name(name)) if name.id.as_str() == "xs"
    ));

    let named = CallArgKeyword::from_ast_keyword_with(
        ast::Keyword {
            node_index: ast::AtomicNodeIndex::default(),
            range: ruff_text_size::TextRange::default(),
            arg: Some(ast::Identifier::new(
                "value",
                ruff_text_size::TextRange::default(),
            )),
            value: py_expr!("y"),
        },
        |expr| expr,
    );
    assert!(matches!(
        named,
        CallArgKeyword::Named { arg, value: Expr::Name(_), .. } if arg.as_str() == "value"
    ));

    let starred_keyword = CallArgKeyword::from_ast_keyword_with(
        ast::Keyword {
            node_index: ast::AtomicNodeIndex::default(),
            range: ruff_text_size::TextRange::default(),
            arg: None,
            value: py_expr!("kw"),
        },
        |expr| expr,
    );
    assert!(matches!(
        starred_keyword,
        CallArgKeyword::Starred(Expr::Name(name)) if name.id.as_str() == "kw"
    ));
}

#[test]
fn ast_operator_kind_helpers_cover_python_ops() {
    assert_eq!(
        BinOpKind::from_ast_operator(ast::Operator::Div),
        BinOpKind::TrueDiv
    );
    assert_eq!(
        BinOpKind::from_ast_inplace_operator(ast::Operator::Div),
        BinOpKind::InplaceTrueDiv
    );
    assert_eq!(
        UnaryOpKind::from_ast_unary_op(ast::UnaryOp::USub),
        UnaryOpKind::Neg
    );
}

fn test_name_gen() -> FunctionNameGen {
    let module_name_gen = ModuleNameGen::new(0);
    module_name_gen.next_function_name_gen()
}

#[test]
fn storage_layout_semantics_collects_structured_cell_ref_logical_names() {
    let function = BlockPyFunction::<CoreModuleShape> {
        function_id: FunctionId::new(0, 1),
        name_gen: test_name_gen(),
        names: FunctionName::new("f", "f", "f", "f"),
        kind: FunctionKind::Function,
        execution_mode: Default::default(),
        params: ParamSpec::default(),
        blocks: vec![Block {
            label: BlockLabel::from_index(0),
            body: vec![CellRefForName::new("captured".to_string()).into()],
            term: BlockTerm::Return(InstrUnresolved::constant_none()),
            params: Vec::new(),
            exc_edge: None,
        }],
        doc: None,
        storage_layout: None,
        scope: CallableScopeInfo::default(),
    };

    let layout =
        compute_storage_layout_from_scope(&function).expect("structured cell ref should capture");

    assert_eq!(
        layout.freevars,
        vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }]
    );
}
