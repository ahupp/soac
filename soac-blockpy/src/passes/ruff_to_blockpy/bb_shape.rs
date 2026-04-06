use crate::block_py::{
    BlockArg, BlockEdge, BlockTerm, ChildVisitable, FunctionNameGen, Instr, InstrLow,
    InstrWithAwaitAndYield, Load, Meta, NameLike, UnresolvedName, WithMeta,
};
use ruff_python_ast::{self as ast};
use ruff_text_size::TextRange;
use std::collections::{HashMap, HashSet};

pub(crate) fn lower_structured_blocks_to_bb_blocks<E, N>(
    _name_gen: &FunctionNameGen,
    blocks: &[crate::block_py::Block<E, E>],
) -> Vec<crate::block_py::Block<E, E>>
where
    E: Clone + Instr<Name = N>,
    N: NameLike,
{
    let mut bb_blocks = blocks
        .iter()
        .map(|block| crate::block_py::Block {
            label: block.label.clone(),
            body: block.body.clone(),
            term: block.term.clone(),
            params: block.bb_params().cloned().collect(),
            exc_edge: block.exc_edge.clone(),
        })
        .collect::<Vec<_>>();
    populate_exception_edge_args(&mut bb_blocks);
    bb_blocks
}

pub(crate) trait CurrentExceptionExpr:
    Instr + ChildVisitable<Self> + Clone + From<Load<Self>>
{
    fn is_current_exception_call(&self) -> bool;
}

impl<N> CurrentExceptionExpr for InstrLow<N>
where
    N: NameLike,
{
    fn is_current_exception_call(&self) -> bool {
        let InstrLow::Call(call) = self else {
            return false;
        };
        let Some(func_name) = (match call.func.as_ref() {
            InstrLow::Load(op) => Some(op.name.id_str()),
            _ => None,
        }) else {
            return false;
        };
        call.args.is_empty() && call.keywords.is_empty() && func_name == "current_exception"
    }
}

impl CurrentExceptionExpr for InstrWithAwaitAndYield {
    fn is_current_exception_call(&self) -> bool {
        let InstrWithAwaitAndYield::Call(call) = self else {
            return false;
        };
        let Some(func_name) = (match call.func.as_ref() {
            InstrWithAwaitAndYield::Load(op) => Some(op.name.id_str()),
            _ => None,
        }) else {
            return false;
        };
        call.args.is_empty() && call.keywords.is_empty() && func_name == "current_exception"
    }
}

pub(crate) fn rewrite_current_exception_in_core_blocks<E>(
    blocks: &mut [crate::block_py::Block<E, E>],
) where
    E: CurrentExceptionExpr + Instr<Name = UnresolvedName>,
{
    for block in blocks {
        let Some(exc_name) = block.exception_param().map(ToString::to_string) else {
            continue;
        };
        for stmt in &mut block.body {
            rewrite_current_exception_in_expr(stmt, exc_name.as_str());
        }
        rewrite_current_exception_in_term(&mut block.term, exc_name.as_str());
    }
}

pub(crate) fn rewrite_current_exception_in_core_blocks_with_await_and_yield(
    blocks: &mut [crate::block_py::Block<InstrWithAwaitAndYield, InstrWithAwaitAndYield>],
) {
    rewrite_current_exception_in_core_blocks(blocks);
}

fn rewrite_current_exception_in_term<E>(term: &mut BlockTerm<E>, exc_name: &str)
where
    E: CurrentExceptionExpr + Instr<Name = UnresolvedName>,
{
    struct RewriteTermVisitor<'a, E> {
        exc_name: &'a str,
        _marker: std::marker::PhantomData<fn(E)>,
    }

    impl<E> crate::block_py::VisitMut<E> for RewriteTermVisitor<'_, E>
    where
        E: CurrentExceptionExpr + Instr<Name = UnresolvedName>,
    {
        fn visit_instr_mut(&mut self, expr: &mut E) {
            rewrite_current_exception_in_expr(expr, self.exc_name);
        }

        fn visit_raise_term_mut(&mut self, raise_term: &mut crate::block_py::TermRaise<E>) {
            if let Some(exc) = raise_term.exc.as_mut() {
                rewrite_current_exception_in_expr(exc, self.exc_name);
            } else {
                raise_term.exc = Some(current_exception_name_expr(self.exc_name));
            }
        }
    }

    crate::block_py::walk_term_mut(
        &mut RewriteTermVisitor {
            exc_name,
            _marker: std::marker::PhantomData,
        },
        term,
    );
}

fn rewrite_current_exception_in_expr<E>(expr: &mut E, exc_name: &str)
where
    E: CurrentExceptionExpr + Instr<Name = UnresolvedName>,
{
    struct RewriteVisitor<'a> {
        exc_name: &'a str,
    }

    impl<E> crate::block_py::VisitMut<E> for RewriteVisitor<'_>
    where
        E: CurrentExceptionExpr + Instr<Name = UnresolvedName>,
    {
        fn visit_instr_mut(&mut self, expr: &mut E) {
            rewrite_current_exception_in_expr(expr, self.exc_name);
        }
    }

    expr.visit_children_mut(&mut RewriteVisitor { exc_name });
    if expr.is_current_exception_call() {
        *expr = current_exception_name_expr(exc_name);
    }
}

pub(crate) fn populate_exception_edge_args<E, N>(blocks: &mut [crate::block_py::Block<E, E>])
where
    E: Instr<Name = N>,
    N: NameLike,
{
    let label_to_index = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label.clone(), index))
        .collect::<HashMap<_, _>>();
    for block_index in 0..blocks.len() {
        let Some(exc_target_label) = blocks[block_index]
            .exc_edge
            .as_ref()
            .map(|edge| edge.target.clone())
        else {
            continue;
        };
        let Some(target_index) = label_to_index.get(&exc_target_label).copied() else {
            continue;
        };
        let source_params = blocks[block_index].param_name_vec();
        let source_has_owner = source_params
            .iter()
            .any(|param| param == "_dp_self" || param == "_dp_state");
        let target_params = blocks[target_index].param_name_vec();
        let exc_name = blocks[target_index]
            .exception_param()
            .map(ToString::to_string);
        let current_exception_aliases = match &blocks[target_index].term {
            BlockTerm::Jump(edge) => edge
                .args
                .iter()
                .filter_map(|arg| match arg {
                    BlockArg::Name(name) if name.starts_with("_dp_try_exc_") => Some(name.as_str()),
                    _ => None,
                })
                .collect::<HashSet<_>>(),
            _ => HashSet::new(),
        };
        let args = target_params
            .into_iter()
            .map(|target_param| {
                if exc_name.as_deref() == Some(target_param.as_str()) {
                    BlockArg::CurrentException
                } else if current_exception_aliases.contains(target_param.as_str()) {
                    BlockArg::CurrentException
                } else if source_params.iter().any(|param| param == &target_param)
                    || source_has_owner
                {
                    BlockArg::Name(target_param)
                } else {
                    BlockArg::None
                }
            })
            .collect();
        blocks[block_index].exc_edge = Some(BlockEdge::with_args(exc_target_label, args));
    }
}

pub(crate) fn lowered_exception_edges<S, T: crate::block_py::Instr>(
    blocks: &[crate::block_py::Block<S, T>],
) -> HashMap<crate::block_py::BlockLabel, Option<crate::block_py::BlockLabel>> {
    blocks
        .iter()
        .map(|block| {
            (
                block.label.clone(),
                block.exc_edge.as_ref().map(|edge| edge.target.clone()),
            )
        })
        .collect()
}

fn current_exception_name_expr<E>(exc_name: &str) -> E
where
    E: CurrentExceptionExpr + Instr<Name = UnresolvedName>,
{
    let range = compat_range();
    let node_index = compat_node_index();
    E::from(Load::<E>::new(ast::name::Name::new(exc_name)).with_meta(Meta::new(node_index, range)))
}

fn compat_node_index() -> ast::AtomicNodeIndex {
    ast::AtomicNodeIndex::default()
}

fn compat_range() -> TextRange {
    TextRange::default()
}

#[cfg(test)]
pub(crate) use tests::lower_structured_located_blocks_to_bb_blocks;
#[cfg(test)]
mod tests;
