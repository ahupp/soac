use super::{
    TypedInlineUnsupportedReason, TypedTempLocal, append_typed_cleanup_dels_to_body,
    try_allocate_typed_stack_temp, typed_store_temp,
};
use soac_core::block_py::{
    BlockPyFunction, BlockTerm, Load, Mappable, Meta, ResolvedName, TryMapInstr, WithMeta,
};
use soac_ir_typed::{
    InstrTyped, TypedBlockPyModuleShape, TypedCallAccessPlan, TypedInstrExtra, ValueFacts,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypedExpressionLinearizationStats {
    pub rewritten_body_roots: usize,
    pub rewritten_terms: usize,
    pub lifted_nested_exprs: usize,
}

pub fn linearize_typed_function_expressions(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
) -> Result<TypedExpressionLinearizationStats, TypedInlineUnsupportedReason> {
    let original_blocks = std::mem::take(&mut function.blocks);
    let mut rewritten_blocks = Vec::with_capacity(original_blocks.len());
    let mut stats = TypedExpressionLinearizationStats::default();

    for mut block in original_blocks {
        let original_body = std::mem::take(&mut block.body);
        let mut rewritten_body = Vec::with_capacity(original_body.len());
        for instr in original_body {
            let mut linearizer = TypedExpressionLinearizer::new(function);
            let rewritten = match instr {
                InstrTyped::Store(mut store) => {
                    let value = linearizer.linearize_root(*store.value)?;
                    store.value = Box::new(value);
                    InstrTyped::Store(store)
                }
                instr => linearizer.linearize_root(instr)?,
            };
            stats.lifted_nested_exprs += linearizer.lifted_nested_exprs;
            if !linearizer.prefix.is_empty() {
                stats.rewritten_body_roots += 1;
            }
            rewritten_body.append(&mut linearizer.prefix);
            rewritten_body.push(rewritten);
            append_typed_cleanup_dels_to_body(&mut rewritten_body, &linearizer.temps);
        }

        let term = block.term;
        let mut linearizer = TypedExpressionLinearizer::new(function);
        let rewritten_term = match term {
            BlockTerm::Jump(edge) => BlockTerm::Jump(edge),
            BlockTerm::IfTerm(mut if_term) => {
                if_term.test = linearizer.linearize_root(if_term.test)?;
                BlockTerm::IfTerm(if_term)
            }
            BlockTerm::BranchTable(mut branch) => {
                branch.index = linearizer.linearize_root(branch.index)?;
                BlockTerm::BranchTable(branch)
            }
            BlockTerm::Raise(mut raise_stmt) => {
                if let Some(exc) = raise_stmt.exc.take() {
                    raise_stmt.exc = Some(linearizer.linearize_root(exc)?);
                }
                BlockTerm::Raise(raise_stmt)
            }
            BlockTerm::Return(value) => BlockTerm::Return(linearizer.linearize_root(value)?),
        };
        stats.lifted_nested_exprs += linearizer.lifted_nested_exprs;
        if !linearizer.prefix.is_empty() {
            stats.rewritten_terms += 1;
        }
        rewritten_body.append(&mut linearizer.prefix);
        block.body = rewritten_body;
        block.term = rewritten_term;
        rewritten_blocks.push(block);
    }

    function.blocks = rewritten_blocks;
    Ok(stats)
}

struct TypedExpressionLinearizer<'a> {
    function: &'a mut BlockPyFunction<TypedBlockPyModuleShape>,
    prefix: Vec<InstrTyped>,
    temps: Vec<TypedTempLocal>,
    lifted_nested_exprs: usize,
    depth: usize,
}

impl<'a> TypedExpressionLinearizer<'a> {
    fn new(function: &'a mut BlockPyFunction<TypedBlockPyModuleShape>) -> Self {
        Self {
            function,
            prefix: Vec::new(),
            temps: Vec::new(),
            lifted_nested_exprs: 0,
            depth: 0,
        }
    }

    fn linearize_root(
        &mut self,
        expr: InstrTyped,
    ) -> Result<InstrTyped, TypedInlineUnsupportedReason> {
        self.try_map_instr(expr)
    }

    fn lift_nested_expr(
        &mut self,
        expr: InstrTyped,
    ) -> Result<InstrTyped, TypedInlineUnsupportedReason> {
        let facts = expr.result_facts();
        let temp = try_allocate_typed_stack_temp(self.function, "typed_linearized_expr")?;
        let temp_name = temp.resolved_name();
        self.prefix.push(typed_store_temp(temp_name.clone(), expr));
        self.temps.push(temp);
        self.lifted_nested_exprs += 1;
        Ok(typed_load_linearized_temp(&temp_name, facts))
    }
}

impl TryMapInstr<InstrTyped, InstrTyped, TypedInlineUnsupportedReason>
    for TypedExpressionLinearizer<'_>
{
    fn try_map_instr(
        &mut self,
        instr: InstrTyped,
    ) -> Result<InstrTyped, TypedInlineUnsupportedReason> {
        let is_root = self.depth == 0;
        self.depth += 1;
        let rewritten = match instr {
            InstrTyped::Truthy(op) => InstrTyped::Truthy(op.try_map_same_children(self)?),
            InstrTyped::Load(op) => InstrTyped::Load(op.try_map_same_children(self)?),
            InstrTyped::BinOp(op) => InstrTyped::BinOp(op.try_map_same_children(self)?),
            InstrTyped::Tuple(op) => InstrTyped::Tuple(op.try_map_same_children(self)?),
            InstrTyped::UnaryOp(op) => InstrTyped::UnaryOp(op.try_map_same_children(self)?),
            InstrTyped::CalleeFunctionId(op) => {
                InstrTyped::CalleeFunctionId(op.try_map_same_children(self)?)
            }
            InstrTyped::CallTyped(op) => {
                let mut op = op.try_map_same_children(self)?;
                if matches!(op.access, TypedCallAccessPlan::GuardedMethod { .. })
                    && !matches!(op.func.as_ref(), InstrTyped::GetAttrTyped(_))
                {
                    op.access = TypedCallAccessPlan::Generic;
                }
                InstrTyped::CallTyped(op)
            }
            InstrTyped::GuardedCallableCallTyped(op) => {
                InstrTyped::GuardedCallableCallTyped(op.try_map_same_children(self)?)
            }
            InstrTyped::GuardedMethodCallTyped(op) => {
                let op = op.try_map_same_children(self)?;
                if matches!(op.func.as_ref(), InstrTyped::GetAttrTyped(_)) {
                    InstrTyped::GuardedMethodCallTyped(op)
                } else {
                    let mut call = op.into_typed_call();
                    call.access = TypedCallAccessPlan::Generic;
                    InstrTyped::CallTyped(call)
                }
            }
            InstrTyped::DirectCallableCallTyped(op) => {
                InstrTyped::DirectCallableCallTyped(op.try_map_same_children(self)?)
            }
            InstrTyped::DirectMethodCallTyped(op) => {
                InstrTyped::DirectMethodCallTyped(op.try_map_same_children(self)?)
            }
            InstrTyped::DirectCallGuardTest(op) => {
                InstrTyped::DirectCallGuardTest(op.try_map_same_children(self)?)
            }
            InstrTyped::CallDirect(op) => InstrTyped::CallDirect(op.try_map_same_children(self)?),
            InstrTyped::GetAttrTyped(op) => {
                InstrTyped::GetAttrTyped(op.try_map_same_children(self)?)
            }
            InstrTyped::SetAttrTyped(op) => {
                InstrTyped::SetAttrTyped(op.try_map_same_children(self)?)
            }
            InstrTyped::GetItem(op) => InstrTyped::GetItem(op.try_map_same_children(self)?),
            InstrTyped::SetItem(op) => InstrTyped::SetItem(op.try_map_same_children(self)?),
            InstrTyped::DelItem(op) => InstrTyped::DelItem(op.try_map_same_children(self)?),
            InstrTyped::Store(op) => InstrTyped::Store(op.try_map_same_children(self)?),
            InstrTyped::Del(op) => InstrTyped::Del(op.try_map_same_children(self)?),
            InstrTyped::MakeCell(op) => InstrTyped::MakeCell(op.try_map_same_children(self)?),
            InstrTyped::IncrementCounter(op) => InstrTyped::IncrementCounter(op),
            InstrTyped::CellRef(op) => InstrTyped::CellRef(op),
            InstrTyped::MakeFunctionWithClosure(op) => {
                InstrTyped::MakeFunctionWithClosure(op.try_map_same_children(self)?)
            }
        };
        self.depth -= 1;

        if !is_root && typed_nested_expr_requires_temp(&rewritten) {
            self.lift_nested_expr(rewritten)
        } else {
            Ok(rewritten)
        }
    }

    fn try_map_name(
        &mut self,
        name: ResolvedName,
    ) -> Result<ResolvedName, TypedInlineUnsupportedReason> {
        Ok(name)
    }
}

fn typed_nested_expr_requires_temp(expr: &InstrTyped) -> bool {
    matches!(
        expr,
        InstrTyped::Truthy(_)
            | InstrTyped::BinOp(_)
            | InstrTyped::Tuple(_)
            | InstrTyped::UnaryOp(_)
            | InstrTyped::CalleeFunctionId(_)
            | InstrTyped::CallTyped(_)
            | InstrTyped::GuardedCallableCallTyped(_)
            | InstrTyped::GuardedMethodCallTyped(_)
            | InstrTyped::DirectCallableCallTyped(_)
            | InstrTyped::DirectMethodCallTyped(_)
            | InstrTyped::DirectCallGuardTest(_)
            | InstrTyped::CallDirect(_)
            | InstrTyped::GetAttrTyped(_)
            | InstrTyped::GetItem(_)
            | InstrTyped::MakeFunctionWithClosure(_)
    )
}

fn typed_load_linearized_temp(
    temp_name: &soac_core::block_py::ResolvedName,
    facts: Option<ValueFacts>,
) -> InstrTyped {
    let mut extra = TypedInstrExtra::default();
    if let Some(facts) = facts {
        extra.refine_result_facts(facts);
    }
    InstrTyped::Load(
        Load::new(temp_name.clone())
            .with_extra(extra)
            .with_meta(Meta::synthetic()),
    )
}
