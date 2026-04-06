use super::{
    lower_structured_blocks_to_bb_blocks, rewrite_current_exception_in_core_blocks,
    CurrentExceptionExpr,
};
use crate::block_py::{
    Block, BlockLabel, BlockParam, BlockParamRole, BlockTerm, CallArgPositional, ChildVisitable,
    InstrLow, InstrResolved, InstrUnresolved, Meta, ModuleNameGen, NameLike, NameLocation,
    ResolvedName, ResolvedStorageBlock, TermIf, WithMeta,
};
use ruff_python_ast::{self as ast};
use ruff_text_size::TextRange;

pub(crate) fn lower_structured_core_blocks_to_bb_blocks<N>(
    blocks: &[Block<InstrLow<N>>],
) -> Vec<Block<InstrLow<N>>>
where
    N: NameLike,
{
    let module_name_gen = ModuleNameGen::new(0);
    let name_gen = module_name_gen.next_function_name_gen();
    let normalized_blocks = blocks.to_vec();
    if let Some(max_label) = normalized_blocks
        .iter()
        .map(|block| block.label.index())
        .max()
    {
        while name_gen.next_block_name().index() <= max_label {}
    }
    lower_structured_blocks_to_bb_blocks(&name_gen, &normalized_blocks)
}

pub(crate) fn lower_structured_unresolved_core_blocks_to_bb_blocks(
    blocks: &[Block<InstrUnresolved>],
) -> Vec<Block<InstrUnresolved>> {
    let module_name_gen = ModuleNameGen::new(0);
    let name_gen = module_name_gen.next_function_name_gen();
    let mut normalized_blocks = blocks.to_vec();
    if let Some(max_label) = normalized_blocks
        .iter()
        .map(|block| block.label.index())
        .max()
    {
        while name_gen.next_block_name().index() <= max_label {}
    }
    rewrite_current_exception_in_core_blocks(&mut normalized_blocks);
    lower_structured_blocks_to_bb_blocks(&name_gen, &normalized_blocks)
}

pub(crate) fn lower_structured_located_blocks_to_bb_blocks(
    blocks: &[Block<InstrResolved>],
) -> Vec<ResolvedStorageBlock> {
    let mut lowered = lower_structured_core_blocks_to_bb_blocks(blocks);
    rewrite_current_exception_in_located_core_blocks(&mut lowered);
    lowered
}

fn rewrite_current_exception_in_located_core_blocks(blocks: &mut [Block<InstrResolved>]) {
    for block in blocks {
        let Some(exc_name) = block.exception_param().map(ToString::to_string) else {
            continue;
        };
        for stmt in &mut block.body {
            rewrite_current_exception_in_located_expr(stmt, exc_name.as_str());
        }
        rewrite_current_exception_in_located_term(&mut block.term, exc_name.as_str());
    }
}

fn rewrite_current_exception_in_located_term(term: &mut BlockTerm<InstrResolved>, exc_name: &str) {
    struct RewriteTermVisitor<'a> {
        exc_name: &'a str,
    }

    impl crate::block_py::VisitMut<InstrResolved> for RewriteTermVisitor<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrResolved) {
            rewrite_current_exception_in_located_expr(expr, self.exc_name);
        }

        fn visit_raise_term_mut(
            &mut self,
            raise_term: &mut crate::block_py::TermRaise<InstrResolved>,
        ) {
            if let Some(exc) = raise_term.exc.as_mut() {
                rewrite_current_exception_in_located_expr(exc, self.exc_name);
            } else {
                raise_term.exc = Some(current_exception_name_expr_located(self.exc_name));
            }
        }
    }

    crate::block_py::walk_term_mut(&mut RewriteTermVisitor { exc_name }, term);
}

fn rewrite_current_exception_in_located_expr(expr: &mut InstrResolved, exc_name: &str) {
    struct RewriteVisitor<'a> {
        exc_name: &'a str,
    }

    impl crate::block_py::VisitMut<InstrResolved> for RewriteVisitor<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrResolved) {
            rewrite_current_exception_in_located_expr(expr, self.exc_name);
        }
    }

    expr.visit_children_mut(&mut RewriteVisitor { exc_name });
    if expr.is_current_exception_call() {
        *expr = current_exception_name_expr_located(exc_name);
    }
}

fn current_exception_name_expr_located(exc_name: &str) -> InstrResolved {
    crate::block_py::Load::new(ResolvedName {
        id: exc_name.into(),
        location: NameLocation::global(0),
    })
    .with_meta(Meta::synthetic())
    .into()
}

fn expr_name(name: &str, ctx: ast::ExprContext) -> ast::ExprName {
    ast::ExprName {
        id: name.into(),
        ctx,
        range: TextRange::default(),
        node_index: ast::AtomicNodeIndex::default(),
    }
}

fn core_name_expr(name: &str) -> InstrUnresolved {
    let name = expr_name(name, ast::ExprContext::Load);
    crate::block_py::Load::new(name.id.clone())
        .with_meta(crate::block_py::Meta::synthetic())
        .into()
}

#[test]
fn lower_structured_core_blocks_to_bb_blocks_handles_unlocated_names() {
    let blocks = vec![
        Block {
            label: BlockLabel::from_index(0),
            body: Vec::new(),
            term: BlockTerm::IfTerm(TermIf {
                test: crate::block_py::core_call_expr_with_meta(
                    core_name_expr("current_exception"),
                    ast::AtomicNodeIndex::default(),
                    TextRange::default(),
                    Vec::<CallArgPositional<InstrUnresolved>>::new(),
                    Vec::new(),
                ),
                then_label: BlockLabel::from_index(1),
                else_label: BlockLabel::from_index(2),
            }),
            params: vec![BlockParam {
                name: "_dp_try_exc_0".to_string(),
                role: BlockParamRole::Exception,
            }],
            exc_edge: None,
        },
        Block {
            label: BlockLabel::from_index(1),
            body: vec![crate::block_py::Store::new(
                expr_name("x", ast::ExprContext::Store).id,
                core_name_expr("a"),
            )
            .into()],
            term: BlockTerm::Return(core_name_expr("__dp_NONE")),
            params: Vec::new(),
            exc_edge: None,
        },
        Block {
            label: BlockLabel::from_index(2),
            body: vec![crate::block_py::Store::new(
                expr_name("x", ast::ExprContext::Store).id,
                core_name_expr("b"),
            )
            .into()],
            term: BlockTerm::Return(core_name_expr("__dp_NONE")),
            params: Vec::new(),
            exc_edge: None,
        },
    ];

    let lowered = lower_structured_unresolved_core_blocks_to_bb_blocks(&blocks);

    assert_eq!(lowered.len(), 3, "{lowered:?}");
    let BlockTerm::IfTerm(TermIf {
        test: InstrUnresolved::Load(load),
        ..
    }) = &lowered[0].term
    else {
        panic!("expected rewritten current-exception test");
    };
    assert_eq!(load.name.id_str(), "_dp_try_exc_0");
}
