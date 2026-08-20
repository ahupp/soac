use super::{
    TypedInlineUnsupportedReason, TypedTempLocal, append_typed_cleanup_dels_to_body,
    try_allocate_typed_stack_temp, typed_store_temp,
};
use soac_core::block_py::{
    BlockPyFunction, BlockTerm, ChildVisitable, HasSemanticInstrId, InstrId, Load, Mappable, Meta,
    ResolvedName, RuntimeFunctionId, RuntimeName, TakeOperand, TryMapInstr, Visit, WithMeta,
};
use soac_ir_typed::plan_v3::{
    IndexedFieldOwnerType, LateBoundOwnerFieldStorage, RegionInputSource, RegionPlan, Rep,
};
use soac_ir_typed::{
    InstrTyped, TypedAttrAccessPlan, TypedAttrOwnerRef, TypedBlockPyModuleShape,
    TypedCallAccessPlan, TypedInstrExtra, ValueFacts,
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
            BlockTerm::GeneratorReturn(value) => {
                BlockTerm::GeneratorReturn(linearizer.linearize_root(value)?)
            }
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
        operand_temps: &[TypedTempLocal],
    ) -> Result<InstrTyped, TypedInlineUnsupportedReason> {
        let facts = expr.result_facts();
        let moves_operand = matches!(&expr, InstrTyped::TakeOperand(_));
        let temp = try_allocate_typed_stack_temp(self.function, "typed_linearized_expr")?;
        self.function
            .storage_layout
            .as_mut()
            .expect("temporary allocation requires a storage layout")
            .mark_expression_temporary(temp.location);
        let temp_name = temp.resolved_name();
        self.prefix.push(typed_store_temp(temp_name.clone(), expr));
        // These values model operands of this operation, not frame locals.
        // Release them before evaluating another sibling or assigning the
        // operation's result to a source binding. A failing operation instead
        // unwinds the still-live expression temporaries through its error edge.
        append_typed_cleanup_dels_to_body(&mut self.prefix, operand_temps);
        self.temps.push(temp);
        self.lifted_nested_exprs += 1;
        Ok(if moves_operand {
            // Capturing a move must remain a move at its consumer. Replacing
            // it with Load would keep a second owner through later callbacks.
            typed_take_linearized_temp(&temp_name, facts)
        } else {
            typed_load_linearized_temp(&temp_name, facts)
        })
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
        let first_operand_temp = self.temps.len();
        self.depth += 1;
        let owned_call = match &instr {
            InstrTyped::CallTyped(call) => {
                let layout = self
                    .function
                    .storage_layout
                    .as_ref()
                    .ok_or(TypedInlineUnsupportedReason::MissingCallerStorageLayout)?;
                call.has_owned_operand_inputs(layout)
                    .map_err(TypedInlineUnsupportedReason::InvalidOperandCall)?
            }
            _ => false,
        };
        let rewritten = if owned_call
            || instr.native_iterator_pipeline_plan().is_some()
            || matches!(
                &instr,
                InstrTyped::ComprehensionInsert(_)
                    | InstrTyped::BuildCollection(_)
                    | InstrTyped::CallArgumentOp(_)
                    | InstrTyped::PreparedCall(_)
            )
            || matches!(&instr, InstrTyped::CallTyped(call) if call.extra.source_call.is_some() || matches!(call.access, TypedCallAccessPlan::GuardedSealedMethod(_)))
            || matches!(&instr, InstrTyped::GuardedMethodCallTyped(call) if call.method_guards.is_empty())
            || instr.typed_extra().is_some_and(|extra| {
                extra.exact_int_branch_plan().is_some_and(|plan| {
                    !plan
                        .hot_plan
                        .inputs
                        .iter()
                        .any(|input| matches!(input.source, RegionInputSource::IndexedField { .. }))
                })
            })
            || typed_exact_int_field_scalar_expression_is_atomic(&instr, self.function.function_id)
        {
            // Complete owned calls retain their child evaluation and consuming
            // cleanup as one operation. Hoisting a fresh call result into Load
            // would add a second owner at the final native call.
            // Collection insertion, complete native iterator templates, resolved lookup/call and
            // complete scalar branch regions already
            // own operand evaluation and cleanup. Hoisting their children
            // would discard the selected operation or detach it from its
            // branch. Borrowed indexed-field regions additionally require the
            // matching live field guard checked below.
            instr
        } else {
            let mut children = OrderedChildLinearizer {
                last_lifted_child: last_child_with_lifted_expression(&instr),
                next_child: 0,
                linearizer: self,
            };
            match instr {
                InstrTyped::Truthy(op) => {
                    InstrTyped::Truthy(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::Load(op) => InstrTyped::Load(op.try_map_same_children(&mut children)?),
                InstrTyped::BinOp(op) if op.extra().exact_float_expression_plan().is_some() => {
                    InstrTyped::BinOp(op)
                }
                InstrTyped::BinOp(op) => {
                    InstrTyped::BinOp(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::Tuple(op) => {
                    InstrTyped::Tuple(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::UnaryOp(op) => {
                    InstrTyped::UnaryOp(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::CalleeFunctionId(op) => {
                    InstrTyped::CalleeFunctionId(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::CallTyped(op) => {
                    let mut op = op.try_map_same_children(&mut children)?;
                    if matches!(op.access, TypedCallAccessPlan::GuardedMethod { .. })
                        && !matches!(op.func.as_ref(), InstrTyped::GetAttrTyped(_))
                    {
                        op.access = TypedCallAccessPlan::Generic;
                    }
                    InstrTyped::CallTyped(op)
                }
                InstrTyped::GuardedCallableCallTyped(op) => {
                    InstrTyped::GuardedCallableCallTyped(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::GuardedMethodCallTyped(op) => {
                    let op = op.try_map_same_children(&mut children)?;
                    if matches!(op.func.as_ref(), InstrTyped::GetAttrTyped(_)) {
                        InstrTyped::GuardedMethodCallTyped(op)
                    } else {
                        let mut call = op.into_typed_call();
                        call.access = TypedCallAccessPlan::Generic;
                        InstrTyped::CallTyped(call)
                    }
                }
                InstrTyped::DirectCallableCallTyped(op) => {
                    InstrTyped::DirectCallableCallTyped(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::DirectMethodCallTyped(op) => {
                    InstrTyped::DirectMethodCallTyped(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::DirectCallGuardTest(op) => {
                    InstrTyped::DirectCallGuardTest(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::CallDirect(op) => {
                    InstrTyped::CallDirect(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::GetAttrTyped(op) => {
                    InstrTyped::GetAttrTyped(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::SetAttrTyped(op) => {
                    InstrTyped::SetAttrTyped(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::GetItem(op) => {
                    InstrTyped::GetItem(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::SetItem(op) => {
                    InstrTyped::SetItem(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::DelItem(op) => {
                    InstrTyped::DelItem(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::Store(op) => {
                    InstrTyped::Store(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::Del(op) => InstrTyped::Del(op.try_map_same_children(&mut children)?),
                InstrTyped::MakeCell(op) => {
                    InstrTyped::MakeCell(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::NewAnnotationSet(op) => {
                    InstrTyped::NewAnnotationSet(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::SetupAnnotations(op) => {
                    InstrTyped::SetupAnnotations(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::ConstructTypeParameterScope(op) => {
                    InstrTyped::ConstructTypeParameterScope(
                        op.try_map_same_children(&mut children)?,
                    )
                }
                InstrTyped::SubscriptGeneric(op) => {
                    InstrTyped::SubscriptGeneric(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::SetFunctionTypeParameters(op) => {
                    InstrTyped::SetFunctionTypeParameters(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::CreateTypeAlias(op) => {
                    InstrTyped::CreateTypeAlias(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::CreateTypeParameter(op) => {
                    InstrTyped::CreateTypeParameter(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::SetTypeParameterDefault(op) => {
                    InstrTyped::SetTypeParameterDefault(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::CheckAnnotationFormat(op) => {
                    InstrTyped::CheckAnnotationFormat(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::RecordAnnotation(op) => {
                    InstrTyped::RecordAnnotation(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::IncrementCounter(op) => InstrTyped::IncrementCounter(op),
                InstrTyped::CellRef(op) => InstrTyped::CellRef(op),
                InstrTyped::MakeFunctionWithClosure(op) => {
                    InstrTyped::MakeFunctionWithClosure(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::ConstructClass(op) => {
                    InstrTyped::ConstructClass(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::PrepareClassDecorator(op) => {
                    InstrTyped::PrepareClassDecorator(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::ApplyClassDecorator(op) => {
                    InstrTyped::ApplyClassDecorator(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::DiscardClassDecorator(op) => {
                    InstrTyped::DiscardClassDecorator(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::TakeOperand(op) => InstrTyped::TakeOperand(op),
                InstrTyped::ComprehensionInsert(op) => InstrTyped::ComprehensionInsert(op),
                InstrTyped::BuildCollection(op) => InstrTyped::BuildCollection(op),
                InstrTyped::CallArgumentOp(op) => InstrTyped::CallArgumentOp(op),
                InstrTyped::PreparedCall(op) => InstrTyped::PreparedCall(op),
                InstrTyped::IteratorStep(op) => InstrTyped::IteratorStep(op),
                InstrTyped::DiscardClassConstructionCaptures(op) => {
                    InstrTyped::DiscardClassConstructionCaptures(
                        op.try_map_same_children(&mut children)?,
                    )
                }
                InstrTyped::CompleteFunctionDefinition(op) => {
                    InstrTyped::CompleteFunctionDefinition(op.try_map_same_children(&mut children)?)
                }
                InstrTyped::ApplyFunctionDescriptor(op) => {
                    InstrTyped::ApplyFunctionDescriptor(op.try_map_same_children(&mut children)?)
                }
            }
        };
        self.depth -= 1;

        if typed_nested_expr_requires_temp(&rewritten)
            && (!is_root || self.temps.len() != first_operand_temp)
        {
            let operand_temps = self.temps.split_off(first_operand_temp);
            self.lift_nested_expr(rewritten, &operand_temps)
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

/// A load is not evaluation-order independent just because it has no children.
/// Hoisting a later sibling can call Python, change a global/closure binding,
/// or raise before an earlier name lookup. Capture those earlier reads first.
/// Compiler language intrinsics have no callable lookup to capture: replacing
/// one with a local would turn the operation into a mutable runtime helper.
/// Already-linearized children do not cause another capture on a later pass.
struct OrderedChildLinearizer<'a, 'function> {
    linearizer: &'a mut TypedExpressionLinearizer<'function>,
    next_child: usize,
    last_lifted_child: Option<usize>,
}

impl TryMapInstr<InstrTyped, InstrTyped, TypedInlineUnsupportedReason>
    for OrderedChildLinearizer<'_, '_>
{
    fn try_map_instr(
        &mut self,
        instr: InstrTyped,
    ) -> Result<InstrTyped, TypedInlineUnsupportedReason> {
        let capture_read = self
            .last_lifted_child
            .is_some_and(|last| self.next_child < last)
            && (matches!(&instr, InstrTyped::Load(load)
                if load.name.location.as_constant().is_none()
                    && !load.name.runtime_name_id().is_some_and(RuntimeName::is_language_intrinsic))
                || matches!(&instr, InstrTyped::TakeOperand(_)));
        self.next_child += 1;
        let rewritten = self.linearizer.try_map_instr(instr)?;
        if capture_read {
            self.linearizer.lift_nested_expr(rewritten, &[])
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

fn last_child_with_lifted_expression(instr: &InstrTyped) -> Option<usize> {
    struct NestedExpressionFinder {
        found: bool,
    }

    impl Visit<InstrTyped> for NestedExpressionFinder {
        fn visit_instr(&mut self, instr: &InstrTyped) {
            self.found |= typed_nested_expr_requires_temp(instr);
            if !self.found {
                instr.visit_children(self);
            }
        }
    }

    #[derive(Default)]
    struct ChildFinder {
        next: usize,
        last_lifted: Option<usize>,
    }

    impl Visit<InstrTyped> for ChildFinder {
        fn visit_instr(&mut self, instr: &InstrTyped) {
            let mut nested = NestedExpressionFinder { found: false };
            nested.visit_instr(instr);
            if nested.found {
                self.last_lifted = Some(self.next);
            }
            self.next += 1;
        }
    }

    let mut children = ChildFinder::default();
    instr.visit_children(&mut children);
    children.last_lifted
}

/// Match an existing resolved field access inside the selected expression.
/// This only preserves a selected region through rewriting; it creates no
/// guard or unchecked fact. Final region validation still checks every input.
pub fn typed_exact_int_region_matches_field_expression(
    expr: &InstrTyped,
    region: &RegionPlan,
    function_id: RuntimeFunctionId,
) -> bool {
    struct MatchingField<'a> {
        source: InstrId,
        owner_type: &'a IndexedFieldOwnerType,
        attr_name: &'a str,
        expected_index: u32,
        function_id: RuntimeFunctionId,
        found: bool,
    }

    impl Visit<InstrTyped> for MatchingField<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }
            if let InstrTyped::GetAttrTyped(op) = expr
                && op.semantic_instr_id() == self.source
            {
                self.found = match &op.access {
                    TypedAttrAccessPlan::IndexedField { guards, .. } => {
                        guards.iter().any(|guard| {
                            guard.expected_index == self.expected_index
                                && matches!(
                                    &guard.owner_type_ref,
                                    TypedAttrOwnerRef::TypeKey { module_name, qualname }
                                        if module_name == &self.owner_type.module_name
                                            && qualname == &self.owner_type.qualname
                                )
                        })
                    }
                    TypedAttrAccessPlan::LateBoundOwnerField(plan) => {
                        plan.counter_source.function_id.runtime_module_id()
                            == self.function_id.runtime_module_id()
                            && &plan.owner_type == self.owner_type
                            && plan.attr_name == self.attr_name
                            && matches!(
                                plan.storage,
                                LateBoundOwnerFieldStorage::SplitDict { expected_index }
                                    if expected_index == self.expected_index
                            )
                    }
                    _ => false,
                };
                if self.found {
                    return;
                }
            }
            expr.visit_children(self);
        }
    }

    region.inputs.iter().any(|input| {
        if input.value.rep != Rep::PyObjectBorrowed {
            return false;
        }
        let RegionInputSource::IndexedField {
            source,
            owner_type,
            attr_name,
            expected_index,
            ..
        } = &input.source
        else {
            return false;
        };
        let mut matcher = MatchingField {
            source: *source,
            owner_type,
            attr_name,
            expected_index: *expected_index,
            function_id,
            found: false,
        };
        matcher.visit_instr(expr);
        matcher.found
    })
}

fn typed_exact_int_field_scalar_expression_is_atomic(
    expr: &InstrTyped,
    function_id: RuntimeFunctionId,
) -> bool {
    let Some(extra) = expr.typed_extra() else {
        return false;
    };

    extra.exact_int_branch_plan().is_some_and(|plan| {
        typed_exact_int_region_matches_field_expression(expr, &plan.hot_plan, function_id)
    }) || extra.exact_int_return_plan().is_some_and(|plan| {
        typed_exact_int_region_matches_field_expression(expr, &plan.hot_plan, function_id)
    })
}

fn typed_nested_expr_requires_temp(expr: &InstrTyped) -> bool {
    // TakeOperand is already an effectful atom. It stays at its evaluation
    // point unless a later sibling actually lifts; OrderedChildLinearizer
    // then captures and re-takes it. Treating every take as another nested
    // expression would create a new move temporary on every linearizer pass.
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
            | InstrTyped::ConstructClass(_)
            | InstrTyped::PrepareClassDecorator(_)
            | InstrTyped::DiscardClassDecorator(_)
            | InstrTyped::DiscardClassConstructionCaptures(_)
            | InstrTyped::ApplyClassDecorator(_)
            | InstrTyped::CompleteFunctionDefinition(_)
            | InstrTyped::ApplyFunctionDescriptor(_)
            | InstrTyped::NewAnnotationSet(_)
            | InstrTyped::SetupAnnotations(_)
            | InstrTyped::ConstructTypeParameterScope(_)
            | InstrTyped::SubscriptGeneric(_)
            | InstrTyped::SetFunctionTypeParameters(_)
            | InstrTyped::CreateTypeAlias(_)
            | InstrTyped::CreateTypeParameter(_)
            | InstrTyped::SetTypeParameterDefault(_)
            | InstrTyped::CheckAnnotationFormat(_)
            | InstrTyped::RecordAnnotation(_)
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

fn typed_take_linearized_temp(temp_name: &ResolvedName, facts: Option<ValueFacts>) -> InstrTyped {
    let mut extra = TypedInstrExtra::default();
    if let Some(facts) = facts {
        extra.refine_result_facts(facts);
    }
    InstrTyped::TakeOperand(
        TakeOperand::new(temp_name.clone())
            .with_extra(extra)
            .with_meta(Meta::synthetic()),
    )
}
