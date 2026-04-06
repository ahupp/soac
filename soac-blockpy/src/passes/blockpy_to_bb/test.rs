use crate::block_py::{
    literal_expr, Block, BlockLabel, BlockPyLiteral, BlockTerm,
    CallArgPositional, CoreStringLiteral, GetAttr, InstrResolved,
    ResolvedName, NameLocation, WithMeta,
};
use crate::passes::ruff_to_blockpy::{
    lower_structured_located_blocks_to_bb_blocks, populate_exception_edge_args,
};
use ruff_python_ast::{self as ast};
use ruff_text_size::TextRange;

fn core_name_expr(name: &str) -> InstrResolved {
    let name = ResolvedName {
        id: name.into(),
        location: NameLocation::global(0),
    };
    crate::block_py::Load::new(name.clone())
        .with_meta(crate::block_py::Meta::synthetic())
        .into()
}

fn core_call_expr(name: &str, args: Vec<InstrResolved>) -> InstrResolved {
    crate::block_py::core_call_expr_with_meta(
        core_name_expr(name),
        ast::AtomicNodeIndex::default(),
        TextRange::default(),
        args.into_iter()
            .map(CallArgPositional::Positional)
            .collect(),
        Vec::new(),
    )
}

#[test]
fn rewrites_current_exception_placeholders_in_final_core_blocks() {
    let block: Block<InstrResolved, InstrResolved> = Block {
        label: BlockLabel::from_index(0),
        body: vec![core_call_expr("current_exception", Vec::new())],
        term: BlockTerm::Return(core_call_expr("current_exception", Vec::new())),
        params: vec![crate::block_py::BlockParam {
            name: "_dp_try_exc_0".to_string(),
            role: crate::block_py::BlockParamRole::Exception,
        }],
        exc_edge: None,
    };

    let lowered = lower_structured_located_blocks_to_bb_blocks(&[Block {
        label: block.label,
        body: block.body,
        term: block.term,
        params: block.params,
        exc_edge: None,
    }]);
    let block = &lowered[0];

    let body_expr = &block.body[0];
    assert!(matches!(
        body_expr,
        InstrResolved::Load(load) if load.name.id.as_str() == "_dp_try_exc_0"
    ));

    let BlockTerm::Return(InstrResolved::Load(load)) = &block.term else {
        panic!("expected rewritten return expr");
    };
    assert_eq!(load.name.id.as_str(), "_dp_try_exc_0");
}

#[test]
fn rewrites_current_exception_inside_intrinsic_helper_args() {
    let block: Block<InstrResolved, InstrResolved> = Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Return(
            GetAttr::new(
                core_call_expr("current_exception", Vec::new()),
                literal_expr::<InstrResolved>(
                    CoreStringLiteral {
                        value: "value".to_string(),
                    },
                    crate::block_py::Meta::synthetic(),
                ),
            )
            .with_meta(crate::block_py::Meta::new(
                ast::AtomicNodeIndex::default(),
                TextRange::default(),
            ))
            .into(),
        ),
        params: vec![crate::block_py::BlockParam {
            name: "_dp_try_exc_0".to_string(),
            role: crate::block_py::BlockParamRole::Exception,
        }],
        exc_edge: None,
    };

    let lowered = lower_structured_located_blocks_to_bb_blocks(&[Block {
        label: block.label,
        body: block.body,
        term: block.term,
        params: block.params,
        exc_edge: None,
    }]);
    let block = &lowered[0];

    let BlockTerm::Return(InstrResolved::GetAttr(GetAttr { value, attr, .. })) = &block.term
    else {
        panic!("expected getattr operation");
    };
    assert!(matches!(
        value.as_ref(),
        InstrResolved::Load(load) if load.name.id.as_str() == "_dp_try_exc_0"
    ));
    assert!(matches!(
        attr.as_ref(),
        InstrResolved::Literal(literal)
            if matches!(
                literal.as_literal(),
                BlockPyLiteral::StringLiteral(CoreStringLiteral { value }) if value == "value"
            )
    ));
}

#[test]
fn exception_edges_seed_hidden_try_exception_locals_from_current_exception() {
    let mut blocks: Vec<crate::block_py::Block<InstrResolved>> = vec![
        crate::block_py::Block {
            label: BlockLabel::from_index(0),
            body: Vec::new(),
            term: BlockTerm::<InstrResolved>::Return(
                <InstrResolved as crate::block_py::ImplicitNoneExpr>::implicit_none_expr(),
            ),
            params: vec![crate::block_py::BlockParam {
                name: "_dp_outer_exc".to_string(),
                role: crate::block_py::BlockParamRole::Exception,
            }],
            exc_edge: Some(crate::block_py::BlockEdge::new(BlockLabel::from_index(1))),
        },
        crate::block_py::Block {
            label: BlockLabel::from_index(1),
            body: Vec::new(),
            term: BlockTerm::<InstrResolved>::Jump(crate::block_py::BlockEdge::with_args(
                BlockLabel::from_index(2),
                vec![
                    crate::block_py::BlockArg::AbruptKind(crate::block_py::AbruptKind::Exception),
                    crate::block_py::BlockArg::Name("_dp_try_exc_payload".to_string()),
                ],
            )),
            params: vec![
                crate::block_py::BlockParam {
                    name: "_dp_inner_exc".to_string(),
                    role: crate::block_py::BlockParamRole::Exception,
                },
                crate::block_py::BlockParam {
                    name: "_dp_try_exc_payload".to_string(),
                    role: crate::block_py::BlockParamRole::AbruptPayload,
                },
            ],
            exc_edge: None,
        },
        crate::block_py::Block {
            label: BlockLabel::from_index(2),
            body: Vec::new(),
            term: BlockTerm::<InstrResolved>::Return(
                <InstrResolved as crate::block_py::ImplicitNoneExpr>::implicit_none_expr(),
            ),
            params: Vec::new(),
            exc_edge: None,
        },
    ];

    populate_exception_edge_args(&mut blocks);

    let edge = blocks[0]
        .exc_edge
        .as_ref()
        .expect("source block must keep exception edge");
    assert!(matches!(
        edge.args.as_slice(),
        [
            crate::block_py::BlockArg::CurrentException,
            crate::block_py::BlockArg::CurrentException
        ]
    ));
}
