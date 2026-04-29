use crate::block_py::{
    core_runtime_positional_call_expr_with_meta, literal_expr, BlockLabel, BlockTerm, Del,
    FunctionKind, FunctionNameGen, HasMeta, InstrWithAwaitAndYield, InstrWithConstantNone, Meta,
    RuntimeFunctionId, Store, StringLiteral, TermIf, UnresolvedName, WithMeta,
};
use crate::namegen::fresh_name;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::string_templates::lower_string_templates_in_instr_ruff;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::LoopContext;
use crate::passes::InstrRuff;
use ruff_python_ast::{self as ast, CmpOp};
use ruff_text_size::TextRange;
use std::collections::HashSet;

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
    fn can_forward_name_value(&self, _name: &str) -> bool {
        false
    }

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

pub(crate) struct ScopedSetupExprLowerer {
    value_forwarding_locals: HashSet<String>,
}

impl ScopedSetupExprLowerer {
    pub(crate) fn new(value_forwarding_locals: HashSet<String>) -> Self {
        Self {
            value_forwarding_locals,
        }
    }
}

impl BlockPySetupExprLowerer for ScopedSetupExprLowerer {
    fn can_forward_name_value(&self, name: &str) -> bool {
        self.value_forwarding_locals.contains(name)
    }
}

pub(crate) use boolop_compare::{
    try_lower_boolop_assign_direct, try_lower_boolop_raise_direct, try_lower_boolop_return_direct,
    try_lower_branching_expr_branch_direct, try_lower_branching_expr_direct,
};
pub(crate) use if_expr::{
    try_lower_if_expr_branch_direct, try_lower_if_expr_direct, try_lower_if_expr_raise_direct,
    try_lower_if_expr_return_direct,
};

fn store_name(name: &str) -> ast::name::Name {
    name.into()
}

fn load_name(name: &str) -> InstrRuff {
    crate::passes::ast_to_instr::from_ast_expr(crate::template::py_expr!("{name:id}", name = name))
}

fn assign_name<E>(target: &str, value: InstrRuff) -> E
where
    E: RuffToBlockPyExpr,
{
    let target = store_name(target);
    Store::new(target, E::from_lowered_expr(value))
        .with_meta(Meta::synthetic())
        .into()
}

fn compare_expr(op: CmpOp, left: InstrRuff, right: InstrRuff) -> InstrRuff {
    InstrRuff::ExprCompare(
        crate::block_py::ExprCompare::new(left, vec![op], vec![right]).with_meta(Meta::default()),
    )
}

fn effect_fragment_from_builders<E>(
    fragments: Vec<(
        crate::passes::ruff_to_blockpy::InlineBlockRef,
        Vec<crate::block_py::Block<E>>,
    )>,
    expect: &str,
) -> crate::passes::ruff_to_blockpy::InlineFragment<E>
where
    E: RuffToBlockPyExpr,
{
    let mut fragments = fragments.into_iter();
    let (setup_entry_ref, mut setup_blocks) = fragments
        .next()
        .unwrap_or_else(|| panic!("{expect} should produce at least one fragment"));
    for (_, mut blocks) in fragments {
        setup_blocks.append(&mut blocks);
    }
    let setup_entry_index = setup_blocks
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .unwrap_or_else(|| panic!("{expect} entry label should be present in assembled blocks"));
    let setup_entry = setup_blocks.remove(setup_entry_index);
    crate::passes::ruff_to_blockpy::InlineFragment::new(setup_entry, setup_blocks)
}

fn lower_effect_only_expr_into<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<(), String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    if let Some(lowered) =
        try_lower_effect_only_expr_direct_with_lowerer(lowerer, name_gen, expr.clone(), loop_ctx)
    {
        out.append_fragment(lowered?);
        return Ok(());
    }
    let value = lowerer.lower_expr_instr_into(expr, out, loop_ctx)?;
    out.push_stmt(E::from_lowered_expr(value));
    Ok(())
}

fn try_lower_if_expr_effect_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    if_expr: crate::block_py::ExprIf<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Result<crate::passes::ruff_to_blockpy::InlineFragment<E>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprIf {
        test, body, orelse, ..
    } = if_expr;
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let (mut entry, test) = bridge
        .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
            lowerer.lower_expr_instr_into(*test.clone(), structured, loop_ctx)
        })
        .transpose()?
        .ok_or_else(|| {
            "if-expression effect setup still requires structured lowering".to_string()
        })?;

    let (mut body_entry, ()) = bridge
        .try_lower_inline_value::<E, ()>(name_gen, |structured| {
            lower_effect_only_expr_into(lowerer, name_gen, *body.clone(), structured, loop_ctx)
        })
        .transpose()?
        .ok_or_else(|| {
            "if-expression body effect still requires structured lowering".to_string()
        })?;
    body_entry.ensure_fallthrough_term();

    let (mut orelse_entry, ()) = bridge
        .try_lower_inline_value::<E, ()>(name_gen, |structured| {
            lower_effect_only_expr_into(lowerer, name_gen, *orelse.clone(), structured, loop_ctx)
        })
        .transpose()?
        .ok_or_else(|| {
            "if-expression orelse effect still requires structured lowering".to_string()
        })?;
    orelse_entry.ensure_fallthrough_term();

    entry.set_term(BlockTerm::IfTerm(TermIf {
        test: E::from_lowered_expr(test),
        then_label: body_entry.entry_ref().label(),
        else_label: orelse_entry.entry_ref().label(),
    }));

    Ok(effect_fragment_from_builders(
        vec![
            entry.finish_blocks(),
            body_entry.finish_blocks(),
            orelse_entry.finish_blocks(),
        ],
        "if-expression effect setup",
    ))
}

fn try_lower_boolop_effect_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Result<crate::passes::ruff_to_blockpy::InlineFragment<E>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprBoolOp { op, values, .. } = bool_op;
    let value_count = values.len();
    assert!(value_count > 0, "bool op expects at least one value");

    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let mut fragments = Vec::new();
    let mut entries = Vec::with_capacity(value_count);
    let mut lowered_non_final = Vec::new();
    let mut final_fragment = None;
    let mut values = values.into_iter().peekable();
    while let Some(value) = values.next() {
        if values.peek().is_none() {
            let (mut final_entry, ()) = bridge
                .try_lower_inline_value::<E, ()>(name_gen, |structured| {
                    lower_effect_only_expr_into(
                        lowerer,
                        name_gen,
                        value.clone(),
                        structured,
                        loop_ctx,
                    )
                })
                .transpose()?
                .ok_or_else(|| {
                    "boolop final effect still requires structured lowering".to_string()
                })?;
            final_entry.ensure_fallthrough_term();
            entries.push(final_entry.entry_ref().label());
            final_fragment = Some(final_entry.finish_blocks());
            break;
        }

        let (entry, test) = bridge
            .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
                lowerer.lower_expr_instr_into(value.clone(), structured, loop_ctx)
            })
            .transpose()?
            .ok_or_else(|| "boolop effect setup still requires structured lowering".to_string())?;
        entries.push(entry.entry_ref().label());
        lowered_non_final.push((entry, test));
    }

    for (index, (mut builder, test)) in lowered_non_final.into_iter().enumerate() {
        let next_label = entries
            .get(index + 1)
            .copied()
            .expect("non-final boolop effect value should have a successor");
        let (truthy_label, falsey_label) = match op {
            ast::BoolOp::And => (next_label, BlockLabel::fallthrough()),
            ast::BoolOp::Or => (BlockLabel::fallthrough(), next_label),
        };
        builder.set_term(BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(test),
            then_label: truthy_label,
            else_label: falsey_label,
        }));
        fragments.push(builder.finish_blocks());
    }
    fragments.push(final_fragment.expect("boolop effect should have a final expression fragment"));

    Ok(effect_fragment_from_builders(
        fragments,
        "boolop effect setup",
    ))
}

fn try_lower_compare_chain_effect_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    compare: crate::block_py::ExprCompare<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Result<crate::passes::ruff_to_blockpy::InlineFragment<E>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprCompare {
        left,
        ops,
        comparators,
        ..
    } = compare;
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let mut steps = ops.into_iter().zip(comparators.into_iter()).peekable();
    let Some((first_op, first_comparator_expr)) = steps.next() else {
        unreachable!("compare chain should contain at least one step");
    };
    let first_has_more = steps.peek().is_some();

    let compare_name = fresh_setup_name("compare");
    let (entry, (initial_left, first_comparator)) = bridge
        .try_lower_inline_value::<E, (InstrRuff, InstrRuff)>(name_gen, |structured| {
            let initial_left =
                lowerer.lower_expr_instr_into((*left).clone(), structured, loop_ctx)?;
            let mut first_comparator = lowerer.lower_expr_instr_into(
                first_comparator_expr.clone(),
                structured,
                loop_ctx,
            )?;
            if first_has_more {
                structured.push_stmt(assign_name(&compare_name, first_comparator.clone()));
                first_comparator = load_name(&compare_name);
            }
            Ok((initial_left, first_comparator))
        })
        .transpose()?
        .ok_or_else(|| {
            "compare-chain effect setup still requires structured lowering".to_string()
        })?;

    let mut fragments = Vec::new();
    let mut current_builder = entry;
    let mut current_left = first_comparator.clone();
    let mut current_test = compare_expr(first_op, initial_left, first_comparator);

    while let Some((op, comparator)) = steps.next() {
        let has_more = steps.peek().is_some();
        let current_left_for_step = current_left.clone();
        let (mut next_entry, comparator_expr) = bridge
            .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
                let mut comparator_expr =
                    lowerer.lower_expr_instr_into(comparator.clone(), structured, loop_ctx)?;
                if has_more {
                    let tmp_name = fresh_setup_name("compare");
                    structured.push_stmt(assign_name(&tmp_name, comparator_expr.clone()));
                    comparator_expr = load_name(&tmp_name);
                } else {
                    structured.push_stmt(E::from_lowered_expr(compare_expr(
                        op,
                        current_left_for_step.clone(),
                        comparator_expr.clone(),
                    )));
                }
                Ok(comparator_expr)
            })
            .transpose()?
            .ok_or_else(|| {
                "compare-chain effect step still requires structured lowering".to_string()
            })?;

        current_builder.set_term(BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(current_test.clone()),
            then_label: next_entry.entry_ref().label(),
            else_label: BlockLabel::fallthrough(),
        }));
        fragments.push(current_builder.finish_blocks());
        if has_more {
            current_left = comparator_expr.clone();
            current_test = compare_expr(op, current_left_for_step, comparator_expr);
        } else {
            next_entry.ensure_fallthrough_term();
        }
        current_builder = next_entry;
    }
    fragments.push(current_builder.finish_blocks());

    Ok(effect_fragment_from_builders(
        fragments,
        "compare-chain effect setup",
    ))
}

fn try_lower_effect_only_expr_direct_with_lowerer<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<crate::passes::ruff_to_blockpy::InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    match expr {
        InstrRuff::UnaryOp(unary) if unary.kind == crate::block_py::UnaryOpKind::Not => {
            try_lower_truthiness_effect_direct_with_lowerer(
                lowerer,
                name_gen,
                *unary.operand,
                loop_ctx,
            )
        }
        InstrRuff::ExprIf(if_expr) => Some(try_lower_if_expr_effect_direct(
            lowerer, name_gen, if_expr, loop_ctx,
        )),
        InstrRuff::ExprBoolOp(bool_op) => Some(try_lower_boolop_effect_direct(
            lowerer, name_gen, bool_op, loop_ctx,
        )),
        InstrRuff::ExprCompare(compare) if compare.ops.len() > 1 => Some(
            try_lower_compare_chain_effect_direct(lowerer, name_gen, compare, loop_ctx),
        ),
        _ => None,
    }
}

fn try_lower_truthiness_effect_direct_with_lowerer<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<crate::passes::ruff_to_blockpy::InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    match expr {
        InstrRuff::ExprIf(if_expr) => try_lower_if_expr_branch_direct::<_, E>(
            lowerer,
            name_gen,
            if_expr,
            BlockLabel::fallthrough(),
            BlockLabel::fallthrough(),
            loop_ctx,
        ),
        other => try_lower_branching_expr_branch_direct::<_, E>(
            lowerer,
            name_gen,
            other,
            BlockLabel::fallthrough(),
            BlockLabel::fallthrough(),
            loop_ctx,
        ),
    }
}

pub(crate) fn try_lower_effect_only_expr_direct_with_context<E>(
    context: &Context,
    expr: InstrRuff,
    name_gen: &FunctionNameGen,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<crate::passes::ruff_to_blockpy::InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr,
{
    let lowerer = ScopedSetupExprLowerer::new(context.current_value_forwarding_locals());
    try_lower_effect_only_expr_direct_with_lowerer(&lowerer, name_gen, expr, loop_ctx)
}

pub(crate) fn lower_expr_head_ast_for_blockpy(expr: InstrRuff) -> InstrRuff {
    expr
}

#[allow(dead_code)]
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

pub(crate) fn lower_expr_into_with_context<E>(
    context: &Context,
    expr: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<E, String>
where
    E: RuffToBlockPyExpr,
{
    ScopedSetupExprLowerer::new(context.current_value_forwarding_locals())
        .lower_expr_into(expr, out, loop_ctx)
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
