use crate::block_py::{
    core_runtime_positional_call_expr_with_meta, literal_expr, Del, FunctionKind, HasMeta,
    InstrWithAwaitAndYield, InstrWithConstantNone, Meta, RuntimeFunctionId, Store, StringLiteral,
    UnresolvedName, WithMeta,
};
use crate::namegen::fresh_name;
use crate::passes::ast_to_ast::string_templates::lower_string_templates_in_instr_ruff;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::LoopContext;
use crate::passes::InstrRuff;
use ruff_python_ast::{self as ast};
use ruff_text_size::TextRange;

mod boolop_compare;
mod if_expr;
mod named_expr;
mod recursive;

fn string_literal_expr(
    node_index: ast::AtomicNodeIndex,
    range: TextRange,
    value: String,
) -> InstrWithAwaitAndYield {
    literal_expr(StringLiteral { value }, Meta::new(node_index, range))
}

pub(crate) trait RuffToBlockPyExpr:
    From<Store<Self>>
    + From<Del<Self>>
    + InstrWithConstantNone<Name = UnresolvedName>
    + std::fmt::Debug
    + Clone
    + Sized
{
    fn from_lowered_expr(expr: InstrRuff) -> Self;

    fn helper_call(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        name: &'static str,
        args: Vec<Self>,
    ) -> Self;

    fn lower_augassign_value(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        op: ast::Operator,
        left: Self,
        right: Self,
    ) -> Self;

    fn get_attr(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        attr: String,
    ) -> Self;

    fn set_attr(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        attr: String,
        replacement: Self,
    ) -> Self;

    fn get_item(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        index: Self,
    ) -> Self;

    fn set_item(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        index: Self,
        replacement: Self,
    ) -> Self;

    fn del_item(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        index: Self,
    ) -> Self;
}

fn inplace_kind(op: ast::Operator) -> Option<crate::block_py::BinOpKind> {
    Some(crate::block_py::BinOpKind::from_ast_inplace_operator(op))
}

impl RuffToBlockPyExpr for InstrWithAwaitAndYield {
    fn from_lowered_expr(expr: InstrRuff) -> Self {
        lower_direct_core_helper_expr(&expr)
            .unwrap_or_else(|| InstrWithAwaitAndYield::from_ruff_expr(expr))
    }

    fn helper_call(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        name: &'static str,
        args: Vec<Self>,
    ) -> Self {
        core_runtime_positional_call_expr_with_meta(name, node_index, range, args)
    }

    fn lower_augassign_value(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        op: ast::Operator,
        left: Self,
        right: Self,
    ) -> Self {
        let meta = Meta::new(node_index.clone(), range);
        let kind = inplace_kind(op)
            .expect("direct augassign lowering should support every Python inplace operator");
        crate::block_py::BinOp::new(kind, Box::new(left), Box::new(right))
            .with_meta(meta)
            .into()
    }

    fn get_attr(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        attr: String,
    ) -> Self {
        let attr_expr = string_literal_expr(node_index.clone(), range, attr);
        crate::block_py::GetAttr::new(Box::new(value), Box::new(attr_expr))
            .with_meta(Meta::new(node_index, range))
            .into()
    }

    fn set_attr(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        attr: String,
        replacement: Self,
    ) -> Self {
        let attr_expr = string_literal_expr(node_index.clone(), range, attr);
        crate::block_py::SetAttr::new(Box::new(value), Box::new(attr_expr), Box::new(replacement))
            .with_meta(Meta::new(node_index, range))
            .into()
    }

    fn get_item(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        index: Self,
    ) -> Self {
        crate::block_py::GetItem::new(Box::new(value), Box::new(index))
            .with_meta(Meta::new(node_index, range))
            .into()
    }

    fn set_item(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        index: Self,
        replacement: Self,
    ) -> Self {
        crate::block_py::SetItem::new(Box::new(value), Box::new(index), Box::new(replacement))
            .with_meta(Meta::new(node_index, range))
            .into()
    }

    fn del_item(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        index: Self,
    ) -> Self {
        crate::block_py::DelItem::new(Box::new(value), Box::new(index))
            .with_meta(Meta::new(node_index, range))
            .into()
    }
}

pub(crate) trait BlockPySetupExprLowerer {
    fn lower_expr_instr_into<E>(
        &self,
        expr: InstrRuff,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
    ) -> Result<InstrRuff, String>
    where
        E: RuffToBlockPyExpr,
    {
        let expr = lower_string_templates_in_instr_ruff(expr);
        recursive::lower_expr_ast_recursive(self, expr, out, loop_ctx)
    }

    fn lower_expr_into<E>(
        &self,
        expr: InstrRuff,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
    ) -> Result<E, String>
    where
        E: RuffToBlockPyExpr,
    {
        Ok(E::from_lowered_expr(
            self.lower_expr_instr_into(expr, out, loop_ctx)?,
        ))
    }
}

pub(crate) struct AstSetupExprLowerer;

impl BlockPySetupExprLowerer for AstSetupExprLowerer {}

pub(crate) use boolop_compare::try_lower_branching_expr_direct;
pub(crate) use if_expr::{try_lower_if_expr_direct, try_lower_if_expr_return_direct};

pub(crate) fn lower_expr_head_ast_for_blockpy(expr: InstrRuff) -> InstrRuff {
    expr
}

pub(crate) fn lower_expr_into_with_setup<E>(
    expr: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<E, String>
where
    E: RuffToBlockPyExpr,
{
    AstSetupExprLowerer.lower_expr_into(expr, out, loop_ctx)
}

fn make_function_kind_from_literal(expr: &InstrRuff) -> Option<FunctionKind> {
    let InstrRuff::ExprStringLiteral(string) = expr else {
        return None;
    };
    Some(match string.value.to_str() {
        "function" => FunctionKind::Function,
        "coroutine" => FunctionKind::Coroutine,
        "generator" => FunctionKind::Generator,
        "async_generator" => FunctionKind::AsyncGenerator,
        _ => return None,
    })
}

fn make_function_id_from_literal(expr: &InstrRuff) -> Option<RuntimeFunctionId> {
    let InstrRuff::ExprNumberLiteral(number) = expr else {
        return None;
    };
    let ast::Number::Int(value) = &number.value else {
        return None;
    };
    value
        .to_string()
        .parse()
        .ok()
        .map(RuntimeFunctionId::from_packed_runtime_u64)
}

fn string_literal_value(expr: &InstrRuff) -> Option<String> {
    let InstrRuff::ExprStringLiteral(string) = expr else {
        return None;
    };
    Some(string.value.to_str().to_string())
}

fn lowered_helper_call<'a>(
    expr: &'a InstrRuff,
    expected_name: &str,
    arity: usize,
) -> Option<&'a crate::block_py::Call<InstrRuff>> {
    let InstrRuff::Call(call) = expr else {
        return None;
    };
    if !call.keywords.is_empty() || call.args.len() != arity {
        return None;
    }
    if !matches!(
        call.func.as_ref(),
        InstrRuff::ExprAttribute(attr_expr)
            if matches!(attr_expr.value.as_ref(), InstrRuff::ExprName(name) if name.id.as_str() == "__soac__")
                && attr_expr.attr.id.as_str() == expected_name
    ) {
        return None;
    }
    Some(call)
}

fn lower_direct_core_helper_expr(expr: &InstrRuff) -> Option<InstrWithAwaitAndYield> {
    fn lowered(expr: InstrRuff) -> InstrWithAwaitAndYield {
        <InstrWithAwaitAndYield as RuffToBlockPyExpr>::from_lowered_expr(expr)
    }

    if let Some(call) = lowered_helper_call(expr, "make_function", 5) {
        let crate::block_py::CallArgPositional::Positional(function_id_expr) = &call.args[0] else {
            return None;
        };
        let crate::block_py::CallArgPositional::Positional(kind_expr) = &call.args[1] else {
            return None;
        };
        let crate::block_py::CallArgPositional::Positional(param_defaults_expr) = &call.args[3]
        else {
            return None;
        };
        let crate::block_py::CallArgPositional::Positional(annotate_fn_expr) = &call.args[4] else {
            return None;
        };
        let function_id = make_function_id_from_literal(function_id_expr)?;
        let kind = make_function_kind_from_literal(kind_expr)?;
        return Some(
            crate::block_py::MakeFunction::new(
                function_id,
                kind,
                Box::new(lowered(param_defaults_expr.clone())),
                Box::new(lowered(annotate_fn_expr.clone())),
            )
            .with_meta(call.meta())
            .into(),
        );
    }

    if let Some(call) = lowered_helper_call(expr, "store_global", 3) {
        let crate::block_py::CallArgPositional::Positional(name_expr) = &call.args[1] else {
            return None;
        };
        let crate::block_py::CallArgPositional::Positional(value_expr) = &call.args[2] else {
            return None;
        };
        return Some(
            crate::block_py::Store::new(
                ast::name::Name::new(string_literal_value(name_expr)?),
                Box::new(lowered(value_expr.clone())),
            )
            .with_meta(call.meta())
            .into(),
        );
    }

    if let Some(call) = lowered_helper_call(expr, "cell_ref", 1) {
        let crate::block_py::CallArgPositional::Positional(name_expr) = &call.args[0] else {
            return None;
        };
        return Some(
            crate::block_py::CellRefForName::new(string_literal_value(name_expr)?)
                .with_meta(call.meta())
                .into(),
        );
    }

    None
}

pub(crate) fn fresh_setup_name(prefix: &str) -> String {
    fresh_name(prefix)
}
