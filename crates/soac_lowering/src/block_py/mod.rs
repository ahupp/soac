pub(crate) use soac_core::block_py::*;

pub(crate) mod cfg;
pub mod counters;
pub mod literal;
mod pretty;
mod scope_impls;
pub mod validate;

pub(crate) use crate::passes::{
    InstrCodegen, InstrCodegenOp, InstrLow, InstrResolved, InstrRuff, InstrUnresolved,
    InstrWithAwaitAndYield, InstrWithYield,
};
pub(crate) use counters::IncrementCounter;
pub(crate) use literal::literal_expr;
pub(crate) use literal::{
    BytesLiteral, IntLiteral, Literal, LiteralValue, NumberLiteral, NumberLiteralValue,
    StringLiteral,
};
pub(crate) use scope_impls::{
    build_storage_layout_from_capture_names, compute_make_function_capture_bindings_from_scope,
    compute_storage_layout_from_scope, is_runtime_closure_name, ScopeExprNode,
};
#[cfg(test)]
pub(crate) use validate::validate_module;

pub(crate) type ResolvedStorageBlock = Block<InstrResolved>;
#[cfg(test)]
pub(crate) type CodegenBlock = Block<InstrCodegen>;

pub(crate) fn core_call_expr_with_meta<E>(
    func: E,
    node_index: ruff_python_ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    args: Vec<CallArgPositional<E>>,
    keywords: Vec<CallArgKeyword<E>>,
) -> E
where
    E: Instr + From<Call<E>>,
{
    Call::new(func, args, keywords)
        .with_meta(Meta::new(node_index, range))
        .into()
}

pub(crate) fn core_runtime_name_expr_with_meta<E>(
    name: &str,
    node_index: ruff_python_ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
) -> E
where
    E: Instr + From<Load<E>>,
{
    Load::new(E::Name::runtime_name(name))
        .with_meta(Meta::new(node_index, range))
        .into()
}

pub(crate) fn runtime_name_load<E>(name: &str) -> E
where
    E: Instr + From<Load<E>>,
{
    Load::new(E::Name::runtime_name(name)).into()
}

pub(crate) fn core_runtime_named_call_expr_with_meta<E>(
    func_name: &str,
    node_index: ruff_python_ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    args: Vec<CallArgPositional<E>>,
    keywords: Vec<CallArgKeyword<E>>,
) -> E
where
    E: Instr + From<Call<E>> + From<Load<E>>,
{
    let func = core_runtime_name_expr_with_meta(func_name, node_index.clone(), range);
    core_call_expr_with_meta(func, node_index, range, args, keywords)
}

pub(crate) fn core_runtime_positional_call_expr_with_meta<E>(
    func_name: &str,
    node_index: ruff_python_ast::AtomicNodeIndex,
    range: ruff_text_size::TextRange,
    args: Vec<E>,
) -> E
where
    E: Instr + From<Call<E>> + From<Load<E>>,
{
    core_runtime_named_call_expr_with_meta(
        func_name,
        node_index,
        range,
        args.into_iter()
            .map(CallArgPositional::Positional)
            .collect(),
        Vec::new(),
    )
}

#[cfg(test)]
mod test;
