use crate::block_py::{
    core_runtime_positional_call_expr_with_meta, literal_expr, operation, BlockPyStmtBuilder,
    InstrWithAwaitAndYield, CoreStringLiteral, Del, FunctionId, FunctionKind, Instr,
    Meta, Store, UnresolvedName, WithMeta,
};
use crate::namegen::fresh_name;
use crate::passes::ast_to_ast::string_templates::lower_string_templates_in_expr;
use crate::passes::ruff_to_blockpy::LoopContext;
use crate::py_expr;
use ruff_python_ast::{self as ast, Expr};
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
    literal_expr(CoreStringLiteral { value }, Meta::new(node_index, range))
}

pub(crate) trait RuffToBlockPyExpr:
    From<Expr>
    + From<Store<Self>>
    + From<Del<Self>>
    + Instr<Name = UnresolvedName>
    + std::fmt::Debug
    + Clone
    + Sized
{
    fn from_lowered_expr(expr: Expr) -> Self {
        expr.into()
    }

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

    fn load_deleted_name(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        name: String,
        value: Self,
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

fn inplace_kind(op: ast::Operator) -> Option<operation::BinOpKind> {
    Some(match op {
        ast::Operator::Add => operation::BinOpKind::InplaceAdd,
        ast::Operator::Sub => operation::BinOpKind::InplaceSub,
        ast::Operator::Mult => operation::BinOpKind::InplaceMul,
        ast::Operator::MatMult => operation::BinOpKind::InplaceMatMul,
        ast::Operator::Div => operation::BinOpKind::InplaceTrueDiv,
        ast::Operator::Mod => operation::BinOpKind::InplaceMod,
        ast::Operator::Pow => operation::BinOpKind::InplacePow,
        ast::Operator::LShift => operation::BinOpKind::InplaceLShift,
        ast::Operator::RShift => operation::BinOpKind::InplaceRShift,
        ast::Operator::BitOr => operation::BinOpKind::InplaceOr,
        ast::Operator::BitXor => operation::BinOpKind::InplaceXor,
        ast::Operator::BitAnd => operation::BinOpKind::InplaceAnd,
        ast::Operator::FloorDiv => operation::BinOpKind::InplaceFloorDiv,
    })
}

impl RuffToBlockPyExpr for InstrWithAwaitAndYield {
    fn from_lowered_expr(expr: Expr) -> Self {
        lower_direct_core_helper_expr(&expr).unwrap_or_else(|| expr.into())
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
        operation::BinOp::new(kind, Box::new(left), Box::new(right))
            .with_meta(meta)
            .into()
    }

    fn load_deleted_name(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        name: String,
        value: Self,
    ) -> Self {
        Self::helper_call(
            node_index,
            range,
            "load_deleted_name",
            vec![
                InstrWithAwaitAndYield::from(py_expr!("{name:literal}", name = name)),
                value,
            ],
        )
    }

    fn get_attr(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        attr: String,
    ) -> Self {
        let attr_expr = string_literal_expr(node_index.clone(), range, attr);
        operation::GetAttr::new(Box::new(value), Box::new(attr_expr))
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
        operation::SetAttr::new(Box::new(value), Box::new(attr_expr), Box::new(replacement))
            .with_meta(Meta::new(node_index, range))
            .into()
    }

    fn get_item(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        index: Self,
    ) -> Self {
        operation::GetItem::new(Box::new(value), Box::new(index))
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
        operation::SetItem::new(Box::new(value), Box::new(index), Box::new(replacement))
            .with_meta(Meta::new(node_index, range))
            .into()
    }

    fn del_item(
        node_index: ast::AtomicNodeIndex,
        range: TextRange,
        value: Self,
        index: Self,
    ) -> Self {
        operation::DelItem::new(Box::new(value), Box::new(index))
            .with_meta(Meta::new(node_index, range))
            .into()
    }
}

pub(crate) trait BlockPySetupExprLowerer {
    fn lower_expr_ast_into<E>(
        &self,
        expr: Expr,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
        next_label_id: &mut usize,
    ) -> Result<Expr, String>
    where
        E: RuffToBlockPyExpr,
    {
        let mut expr = expr;
        lower_string_templates_in_expr(&mut expr);
        recursive::lower_expr_ast_recursive(self, expr, out, loop_ctx, next_label_id)
    }

    fn lower_expr_into<E>(
        &self,
        expr: Expr,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
        next_label_id: &mut usize,
    ) -> Result<E, String>
    where
        E: RuffToBlockPyExpr,
    {
        Ok(E::from_lowered_expr(self.lower_expr_ast_into(
            expr,
            out,
            loop_ctx,
            next_label_id,
        )?))
    }
}

pub(crate) struct AstSetupExprLowerer;

impl BlockPySetupExprLowerer for AstSetupExprLowerer {}

pub(crate) fn lower_expr_head_ast_for_blockpy(expr: Expr) -> Expr {
    expr
}

pub(crate) fn lower_expr_into_with_setup<E>(
    expr: Expr,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<E, String>
where
    E: RuffToBlockPyExpr,
{
    AstSetupExprLowerer.lower_expr_into(expr, out, loop_ctx, next_label_id)
}

fn make_function_kind_from_literal(expr: &Expr) -> Option<FunctionKind> {
    let Expr::StringLiteral(string) = expr else {
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

fn make_function_id_from_literal(expr: &Expr) -> Option<FunctionId> {
    let Expr::NumberLiteral(number) = expr else {
        return None;
    };
    let ast::Number::Int(value) = &number.value else {
        return None;
    };
    value.to_string().parse().ok().map(FunctionId::from_packed)
}

fn string_literal_value(expr: &Expr) -> Option<String> {
    let Expr::StringLiteral(string) = expr else {
        return None;
    };
    Some(string.value.to_str().to_string())
}

fn lowered_helper_call<'a>(
    expr: &'a Expr,
    expected_name: &str,
    arity: usize,
) -> Option<&'a ast::ExprCall> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if !call.arguments.keywords.is_empty() || call.arguments.args.len() != arity {
        return None;
    }
    if !matches!(
        call.func.as_ref(),
        Expr::Attribute(ast::ExprAttribute { value, attr, .. })
            if matches!(value.as_ref(), Expr::Name(name) if name.id.as_str() == "__soac__")
                && attr.id.as_str() == expected_name
    ) {
        return None;
    }
    Some(call)
}

fn lower_direct_core_helper_expr(expr: &Expr) -> Option<InstrWithAwaitAndYield> {
    fn lowered(expr: Expr) -> InstrWithAwaitAndYield {
        <InstrWithAwaitAndYield as RuffToBlockPyExpr>::from_lowered_expr(expr)
    }

    if let Some(call) = lowered_helper_call(expr, "make_function", 5) {
        let function_id = make_function_id_from_literal(&call.arguments.args[0])?;
        let kind = make_function_kind_from_literal(&call.arguments.args[1])?;
        return Some(
            operation::MakeFunction::new(
                function_id,
                kind,
                Box::new(lowered(call.arguments.args[3].clone())),
                Box::new(lowered(call.arguments.args[4].clone())),
            )
            .with_meta(Meta::new(call.node_index.clone(), call.range))
            .into(),
        );
    }

    if let Some(call) = lowered_helper_call(expr, "store_global", 3) {
        return Some(
            operation::Store::new(
                ast::ExprName {
                    id: string_literal_value(&call.arguments.args[1])?.into(),
                    ctx: ast::ExprContext::Store,
                    node_index: call.node_index.clone(),
                    range: call.range,
                },
                Box::new(lowered(call.arguments.args[2].clone())),
            )
            .with_meta(Meta::new(call.node_index.clone(), call.range))
            .into(),
        );
    }

    if let Some(call) = lowered_helper_call(expr, "cell_ref", 1) {
        return Some(
            operation::CellRefForName::new(string_literal_value(&call.arguments.args[0])?)
                .with_meta(Meta::new(call.node_index.clone(), call.range))
                .into(),
        );
    }

    None
}

pub(crate) fn fresh_setup_name(prefix: &str) -> String {
    fresh_name(prefix)
}
