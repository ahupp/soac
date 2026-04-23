use crate::passes::{
    CodegenModuleShape, ConstructorFieldValue, InlinePlanModule, InstrCodegen, InstrCodegenOp,
    InstrResolved, allocate_codegen_stack_temp, plan_module_inlining,
    reassign_codegen_function_instr_ids, summarize_module_escapes, try_allocate_codegen_stack_temp,
};
use soac_core::block_py::literal::Literal;
use soac_core::block_py::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm,
    CallArgPositional, CallDirect, HasMeta, LocalLocation, MapInstr, Mappable, NameLocation,
    ParamKind, ResolvedName, RuntimeFunctionId, RuntimeName, Store, TryMapInstr, TryMapTerm,
    WithMeta, instr_any,
};
use std::collections::{HashMap, HashSet};

pub const DEFAULT_INLINE_SCALAR_FIXED_POINT_ITERATIONS: usize = 4;

#[derive(Debug, Clone)]
pub struct InlineFragment {
    pub entry_label: BlockLabel,
    pub blocks: Vec<Block<InstrCodegen>>,
    pub locals: HashMap<LocalLocation, InlineLocal>,
    pub return_local: Option<InlineLocal>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct InlineLocal {
    pub name: String,
    pub location: LocalLocation,
}

pub type InlineValueBindings = HashMap<LocalLocation, InstrCodegen>;

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct InlineRewriteStats {
    pub rewritten_stores: usize,
    pub skipped_candidates: usize,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct ScalarReplacementStats {
    pub candidate_allocations: usize,
    pub planned_allocations: usize,
    pub replaced_allocations: usize,
    pub skipped_allocations: usize,
    pub skipped_unbuildable_allocations: usize,
    pub skipped_live_alias_control_flow_allocations: usize,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct InlineScalarRewriteStats {
    pub iterations: usize,
    pub inline_rewrite: InlineRewriteStats,
    pub scalar_replacement: ScalarReplacementStats,
    pub hit_iteration_limit: bool,
}

impl InlineScalarRewriteStats {
    fn record_iteration(
        &mut self,
        inline_rewrite: InlineRewriteStats,
        scalar_replacement: ScalarReplacementStats,
    ) {
        self.iterations += 1;
        self.inline_rewrite.rewritten_stores += inline_rewrite.rewritten_stores;
        self.inline_rewrite.skipped_candidates += inline_rewrite.skipped_candidates;
        self.scalar_replacement.candidate_allocations += scalar_replacement.candidate_allocations;
        self.scalar_replacement.planned_allocations += scalar_replacement.planned_allocations;
        self.scalar_replacement.replaced_allocations += scalar_replacement.replaced_allocations;
        self.scalar_replacement.skipped_allocations += scalar_replacement.skipped_allocations;
        self.scalar_replacement.skipped_unbuildable_allocations +=
            scalar_replacement.skipped_unbuildable_allocations;
        self.scalar_replacement
            .skipped_live_alias_control_flow_allocations +=
            scalar_replacement.skipped_live_alias_control_flow_allocations;
    }
}

#[derive(Debug, Clone)]
pub struct InlineCallee {
    function: BlockPyFunction<CodegenModuleShape>,
    module_constants: Option<Vec<InstrResolved>>,
}

impl InlineCallee {
    pub fn same_module(function: BlockPyFunction<CodegenModuleShape>) -> Self {
        Self {
            function,
            module_constants: None,
        }
    }

    pub fn cross_module(
        function: BlockPyFunction<CodegenModuleShape>,
        module_constants: Vec<InstrResolved>,
    ) -> Self {
        Self {
            function,
            module_constants: Some(module_constants),
        }
    }

    pub fn function(&self) -> &BlockPyFunction<CodegenModuleShape> {
        &self.function
    }

    pub fn module_constants(&self) -> Option<&[InstrResolved]> {
        self.module_constants.as_deref()
    }

    pub fn with_function(&self, function: BlockPyFunction<CodegenModuleShape>) -> Self {
        Self {
            function,
            module_constants: self.module_constants.clone(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum InlineUnsupportedReason {
    MissingCallerStorageLayout,
    MissingCalleeStorageLayout,
    MissingCalleeLocal(LocalLocation),
    MissingParameterLocal(String),
    RebindsBoundLocal(LocalLocation),
    ArityMismatch { expected: usize, actual: usize },
    KeywordArguments,
    StarredArguments,
    UnsupportedParameterKind { name: String, kind: ParamKind },
    TooManyBlocks { count: usize, max: usize },
    MultipleBlocks { count: usize },
    UnknownLabel(BlockLabel),
    BlockParams,
    JumpArgs,
    ExceptionEdge,
    NonReturnTerm,
    MissingCalleeConstant(u32),
    TooManyCallerConstants,
    CrossModuleGlobalName(String),
}

const MAX_INLINE_DIRECT_CALL_BLOCKS: usize = 16;

pub fn inline_simple_direct_call_stores(
    module: &mut BlockPyModule<CodegenModuleShape>,
    inline_plan: &InlinePlanModule,
) -> InlineRewriteStats {
    let callees = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                InlineCallee::same_module(function.clone()),
            )
        })
        .collect::<HashMap<_, _>>();
    inline_direct_call_stores_with_callees(module, inline_plan, &callees)
}

pub fn inline_and_scalar_replace_until_fixed_point(
    module: &mut BlockPyModule<CodegenModuleShape>,
) -> InlineScalarRewriteStats {
    inline_and_scalar_replace_with_callees_until_fixed_point(
        module,
        &InlinePlanModule::default(),
        &HashMap::new(),
    )
}

pub fn inline_and_scalar_replace_with_callees_until_fixed_point(
    module: &mut BlockPyModule<CodegenModuleShape>,
    external_inline_plan: &InlinePlanModule,
    extra_callees: &HashMap<RuntimeFunctionId, InlineCallee>,
) -> InlineScalarRewriteStats {
    let mut stats = InlineScalarRewriteStats::default();
    for iteration in 0..DEFAULT_INLINE_SCALAR_FIXED_POINT_ITERATIONS {
        let mut inline_plan = plan_module_inlining(&summarize_module_escapes(module));
        inline_plan
            .functions
            .extend(external_inline_plan.functions.clone());
        let mut callees = extra_callees.clone();
        for function in &module.callable_defs {
            callees.insert(
                function.function_id,
                InlineCallee::same_module(function.clone()),
            );
        }

        let inline_rewrite = inline_direct_call_stores_with_callees(module, &inline_plan, &callees);
        let scalar_replacement =
            scalar_replace_non_escaping_constructor_allocations(module, &inline_plan);
        let changed =
            inline_rewrite.rewritten_stores != 0 || scalar_replacement.replaced_allocations != 0;
        stats.record_iteration(inline_rewrite, scalar_replacement);
        if !changed {
            return stats;
        }
        if iteration + 1 == DEFAULT_INLINE_SCALAR_FIXED_POINT_ITERATIONS {
            stats.hit_iteration_limit = true;
        }
    }
    stats
}

pub fn inline_direct_call_stores_with_callees(
    module: &mut BlockPyModule<CodegenModuleShape>,
    inline_plan: &InlinePlanModule,
    callees: &HashMap<RuntimeFunctionId, InlineCallee>,
) -> InlineRewriteStats {
    let mut stats = InlineRewriteStats::default();
    let module_constants = &mut module.module_constants;
    for function in &mut module.callable_defs {
        if inline_simple_direct_call_stores_in_function(
            function,
            module_constants,
            inline_plan,
            callees,
            &mut stats,
        ) {
            reassign_codegen_function_instr_ids(function);
        }
    }
    stats
}

pub fn rewrite_static_runtime_constructor_call_stores(
    blocks: &mut [Block<InstrCodegen>],
    module_constants: &[InstrResolved],
    mut constructor_for_runtime_name: impl FnMut(RuntimeName) -> Option<RuntimeFunctionId>,
) -> usize {
    let mut rewritten = 0;
    for block in blocks {
        for instr in &mut block.body {
            let InstrCodegen::Store(store) = instr else {
                continue;
            };
            let InstrCodegen::Call(call) = store.value.as_ref() else {
                continue;
            };
            let Some(runtime_name) =
                static_runtime_name_from_codegen_expr(call.func.as_ref(), module_constants)
            else {
                continue;
            };
            let Some(function_id) = constructor_for_runtime_name(runtime_name) else {
                continue;
            };
            let meta = call.meta();
            *store.value = InstrCodegen::CallDirect(
                CallDirect::new(
                    call.func.clone(),
                    function_id,
                    call.args.clone(),
                    call.keywords.clone(),
                )
                .with_meta(meta),
            );
            rewritten += 1;
        }
    }
    rewritten
}

fn static_runtime_name_from_codegen_expr(
    expr: &InstrCodegen,
    module_constants: &[InstrResolved],
) -> Option<RuntimeName> {
    match expr {
        InstrCodegen::Load(load) if load.name.location.is_runtime_name() => {
            load.name.location.runtime_name_id()
        }
        InstrCodegen::Load(load) => load
            .name
            .location
            .as_constant()
            .and_then(|index| module_constants.get(index as usize))
            .and_then(|expr| {
                RuntimeName::ALL
                    .iter()
                    .copied()
                    .find(|runtime_name| is_resolved_runtime_name_expr(expr, *runtime_name))
            }),
        _ => None,
    }
}

pub fn scalar_replace_non_escaping_constructor_allocations(
    module: &mut BlockPyModule<CodegenModuleShape>,
    inline_plan: &InlinePlanModule,
) -> ScalarReplacementStats {
    let constants = module.module_constants.clone();
    let straightline_constructor_ids = inline_plan
        .functions
        .iter()
        .filter_map(|(function_id, plan)| {
            plan.straightline_constructor.as_ref().map(|_| *function_id)
        })
        .collect::<HashSet<_>>();
    let mut stats = ScalarReplacementStats::default();
    for function in &mut module.callable_defs {
        let candidate_allocations =
            count_scalar_replacement_candidate_allocations(function, &straightline_constructor_ids);
        stats.candidate_allocations += candidate_allocations;
        stats.planned_allocations += candidate_allocations;
        if candidate_allocations == 0 {
            continue;
        }
        if scalar_replace_non_escaping_constructor_allocations_in_function(
            function,
            inline_plan,
            &straightline_constructor_ids,
            constants.as_slice(),
            &mut stats,
        ) {
            reassign_codegen_function_instr_ids(function);
        }
    }
    stats.skipped_allocations = stats
        .planned_allocations
        .saturating_sub(stats.replaced_allocations);
    stats
}

fn count_scalar_replacement_candidate_allocations(
    function: &BlockPyFunction<CodegenModuleShape>,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
) -> usize {
    function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter(|instr| {
            let InstrCodegen::Store(store) = instr else {
                return false;
            };
            if store.name.location.as_local().is_none() {
                return false;
            }
            let InstrCodegen::CallDirect(call) = store.value.as_ref() else {
                return false;
            };
            straightline_constructor_ids.contains(&call.function_id)
        })
        .count()
}

fn scalar_replace_non_escaping_constructor_allocations_in_function(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    inline_plan: &InlinePlanModule,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
    constants: &[InstrResolved],
    stats: &mut ScalarReplacementStats,
) -> bool {
    let mut changed = false;
    let mut blocks = std::mem::take(&mut function.blocks);
    loop {
        let block_index_by_label = block_index_by_label(&blocks);
        let normal_predecessor_counts = normal_predecessor_counts(&blocks);
        let mut replaced_in_iteration = false;
        for block_index in 0..blocks.len() {
            match try_scalar_replace_block_chain(
                function,
                &mut blocks,
                block_index,
                inline_plan,
                straightline_constructor_ids,
                constants,
                &block_index_by_label,
                &normal_predecessor_counts,
            ) {
                ScalarReplacementAttempt::Replaced => {
                    stats.replaced_allocations += 1;
                    changed = true;
                    replaced_in_iteration = true;
                    break;
                }
                ScalarReplacementAttempt::SkippedUnbuildableAllocation => {
                    stats.skipped_unbuildable_allocations += 1;
                }
                ScalarReplacementAttempt::SkippedLiveAliasControlFlow => {
                    stats.skipped_live_alias_control_flow_allocations += 1;
                }
                ScalarReplacementAttempt::NoCandidate
                | ScalarReplacementAttempt::SkippedUnsupported => {}
            }
        }
        if !replaced_in_iteration {
            break;
        }
    }
    function.blocks = blocks;
    changed
}

fn inline_simple_direct_call_stores_in_function(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    module_constants: &mut Vec<InstrResolved>,
    inline_plan: &InlinePlanModule,
    callees: &HashMap<RuntimeFunctionId, InlineCallee>,
    stats: &mut InlineRewriteStats,
) -> bool {
    let mut changed = false;
    let original_blocks = std::mem::take(&mut function.blocks);
    let original_block_by_label = original_blocks
        .iter()
        .cloned()
        .map(|block| (block.label, block))
        .collect::<HashMap<_, _>>();
    let mut rewritten_blocks = Vec::with_capacity(original_blocks.len());
    for block in original_blocks {
        match build_direct_store_rewrite(
            function,
            module_constants,
            block,
            inline_plan,
            callees,
            &original_block_by_label,
            stats,
        ) {
            InlineBlockRewrite::Rewritten(blocks) => {
                rewritten_blocks.extend(blocks);
                changed = true;
            }
            InlineBlockRewrite::Unchanged(block) => rewritten_blocks.push(block),
        }
    }
    function.blocks = rewritten_blocks;
    changed
}

fn build_direct_store_rewrite(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    caller_constants: &mut Vec<InstrResolved>,
    block: Block<InstrCodegen>,
    inline_plan: &InlinePlanModule,
    callees: &HashMap<RuntimeFunctionId, InlineCallee>,
    original_block_by_label: &HashMap<BlockLabel, Block<InstrCodegen>>,
    stats: &mut InlineRewriteStats,
) -> InlineBlockRewrite {
    let Some(candidate) =
        find_inline_store_candidate(&block, caller.function_id, inline_plan, callees)
    else {
        return InlineBlockRewrite::Unchanged(block);
    };
    let callee = callees
        .get(&candidate.callee_id)
        .expect("inline store candidate should have an existing callee");
    let Ok(bindings) = bind_simple_direct_call_inline_args(callee.function(), &candidate.call)
    else {
        stats.skipped_candidates += 1;
        return InlineBlockRewrite::Unchanged(block);
    };

    let continuation = caller.name_gen.next_block_name();
    let fragment_result = match callee.module_constants() {
        Some(callee_constants) => build_cross_module_direct_call_inline_fragment_to_target(
            caller,
            caller_constants,
            callee.function(),
            callee_constants,
            continuation,
            &bindings,
            candidate.target,
        ),
        None => build_direct_call_inline_fragment_to_target(
            caller,
            callee.function(),
            continuation,
            &bindings,
            candidate.target,
        ),
    };
    let Ok(mut fragment) = fragment_result else {
        stats.skipped_candidates += 1;
        return InlineBlockRewrite::Unchanged(block);
    };

    let exc_edge = block.exc_edge.clone();
    for fragment_block in &mut fragment.blocks {
        fragment_block.exc_edge = exc_edge.clone();
    }
    if let Some(stop_iteration_target) = stop_iteration_handler_target(
        exc_edge.as_ref(),
        original_block_by_label,
        caller_constants.as_slice(),
    ) {
        rewrite_inlined_stop_iteration_raises(
            &mut fragment.blocks,
            stop_iteration_target,
            caller_constants.as_slice(),
        );
    }

    let mut before = block.body;
    let after = before.split_off(candidate.instr_index + 1);
    before.truncate(candidate.instr_index);
    let prelude = Block::new(
        block.label,
        before,
        BlockTerm::Jump(BlockEdge::new(fragment.entry_label)),
        block.params,
        exc_edge.clone(),
    );
    let continuation_block = Block::new(continuation, after, block.term, Vec::new(), exc_edge);

    let mut blocks = Vec::with_capacity(fragment.blocks.len() + 2);
    blocks.push(prelude);
    blocks.extend(fragment.blocks);
    blocks.push(continuation_block);
    stats.rewritten_stores += 1;
    InlineBlockRewrite::Rewritten(blocks)
}

fn stop_iteration_handler_target(
    exc_edge: Option<&BlockEdge>,
    original_block_by_label: &HashMap<BlockLabel, Block<InstrCodegen>>,
    module_constants: &[InstrResolved],
) -> Option<BlockLabel> {
    let exc_edge = exc_edge?;
    if !matches!(exc_edge.args.as_slice(), [BlockArg::CurrentException]) {
        return None;
    }
    let handler = original_block_by_label.get(&exc_edge.target)?;
    let BlockTerm::IfTerm(if_term) = &handler.term else {
        return None;
    };
    is_exception_matches_stop_iteration_test(&if_term.test, module_constants)
        .then_some(if_term.then_label)
}

fn rewrite_inlined_stop_iteration_raises(
    blocks: &mut [Block<InstrCodegen>],
    stop_iteration_target: BlockLabel,
    module_constants: &[InstrResolved],
) {
    for block in blocks {
        if let BlockTerm::Raise(raise) = &block.term {
            if raise
                .exc
                .as_ref()
                .is_some_and(|expr| is_runtime_stop_iteration_expr(expr, module_constants))
            {
                block.term = BlockTerm::Jump(BlockEdge::new(stop_iteration_target));
            }
        }
    }
}

fn is_exception_matches_stop_iteration_test(
    expr: &InstrCodegen,
    module_constants: &[InstrResolved],
) -> bool {
    match expr {
        InstrCodegenOp::Call(call) => {
            call.keywords.is_empty()
                && is_runtime_name_expr(
                    call.func.as_ref(),
                    RuntimeName::ExceptionMatches,
                    module_constants,
                )
                && matches!(
                    call.args.as_slice(),
                    [
                        CallArgPositional::Positional(_),
                        CallArgPositional::Positional(stop_iteration),
                    ] if is_runtime_stop_iteration_expr(stop_iteration, module_constants)
                )
        }
        InstrCodegenOp::CallDirect(call) => {
            call.keywords.is_empty()
                && is_runtime_name_expr(
                    call.callable.as_ref(),
                    RuntimeName::ExceptionMatches,
                    module_constants,
                )
                && matches!(
                    call.args.as_slice(),
                    [
                        CallArgPositional::Positional(_),
                        CallArgPositional::Positional(stop_iteration),
                    ] if is_runtime_stop_iteration_expr(stop_iteration, module_constants)
                )
        }
        _ => false,
    }
}

fn is_runtime_stop_iteration_expr(expr: &InstrCodegen, module_constants: &[InstrResolved]) -> bool {
    is_runtime_name_expr(expr, RuntimeName::StopIteration, module_constants)
}

fn is_runtime_name_expr(
    expr: &InstrCodegen,
    runtime_name: RuntimeName,
    module_constants: &[InstrResolved],
) -> bool {
    let InstrCodegenOp::Load(load) = expr else {
        return false;
    };
    if load.name.location == NameLocation::RuntimeName(runtime_name)
        || load.name.id.as_str() == runtime_name.name()
    {
        return true;
    }
    let NameLocation::Constant(constant_index) = load.name.location else {
        return false;
    };
    module_constants
        .get(constant_index as usize)
        .is_some_and(|constant| is_resolved_runtime_name_expr(constant, runtime_name))
}

fn is_resolved_runtime_name_expr(expr: &InstrResolved, runtime_name: RuntimeName) -> bool {
    matches!(
        expr,
        InstrResolved::Load(load)
            if load.name.location == NameLocation::RuntimeName(runtime_name)
                || load.name.id.as_str() == runtime_name.name()
    )
}

enum InlineBlockRewrite {
    Rewritten(Vec<Block<InstrCodegen>>),
    Unchanged(Block<InstrCodegen>),
}

struct ScalarizedAllocation {
    object_location: LocalLocation,
    aliases: HashSet<LocalLocation>,
    fields: HashMap<String, InlineLocal>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ScalarReplacementAttempt {
    NoCandidate,
    Replaced,
    SkippedUnbuildableAllocation,
    SkippedLiveAliasControlFlow,
    SkippedUnsupported,
}

fn try_scalar_replace_block_chain(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    blocks: &mut Vec<Block<InstrCodegen>>,
    start_block_index: usize,
    inline_plan: &InlinePlanModule,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
    constants: &[InstrResolved],
    block_index_by_label: &HashMap<BlockLabel, usize>,
    normal_predecessor_counts: &HashMap<BlockLabel, usize>,
) -> ScalarReplacementAttempt {
    let original_blocks = blocks.to_vec();
    let original_storage_layout = function.storage_layout.clone();
    let attempt = try_scalar_replace_block_chain_in_place(
        function,
        blocks,
        start_block_index,
        inline_plan,
        straightline_constructor_ids,
        constants,
        block_index_by_label,
        normal_predecessor_counts,
    );
    if attempt != ScalarReplacementAttempt::Replaced {
        *blocks = original_blocks;
        function.storage_layout = original_storage_layout;
    }
    if attempt == ScalarReplacementAttempt::SkippedLiveAliasControlFlow {
        return try_scalar_replace_reachable_control_flow(
            function,
            blocks,
            start_block_index,
            inline_plan,
            straightline_constructor_ids,
            constants,
            block_index_by_label,
        );
    }
    attempt
}

fn try_scalar_replace_block_chain_in_place(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    blocks: &mut Vec<Block<InstrCodegen>>,
    start_block_index: usize,
    inline_plan: &InlinePlanModule,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
    constants: &[InstrResolved],
    block_index_by_label: &HashMap<BlockLabel, usize>,
    normal_predecessor_counts: &HashMap<BlockLabel, usize>,
) -> ScalarReplacementAttempt {
    let Some(candidate) = find_scalarizable_allocation_candidate(
        &blocks[start_block_index],
        straightline_constructor_ids,
    ) else {
        return ScalarReplacementAttempt::NoCandidate;
    };
    let Some(scalarized) = build_scalarized_allocation(
        function,
        inline_plan,
        candidate.local_location,
        &candidate.call,
        constants,
    ) else {
        return ScalarReplacementAttempt::SkippedUnbuildableAllocation;
    };
    let mut allocation = scalarized.allocation;
    let mut next_block_index = Some(start_block_index);
    let mut first_block = true;
    let mut visited = HashSet::new();

    while let Some(block_index) = next_block_index {
        if !visited.insert(blocks[block_index].label) {
            return ScalarReplacementAttempt::SkippedUnsupported;
        }
        let prefix_end = if first_block {
            candidate.instr_index
        } else {
            0
        };
        let suffix_start = if first_block {
            candidate.instr_index + 1
        } else {
            0
        };
        let body = std::mem::take(&mut blocks[block_index].body);
        let mut rewritten = Vec::with_capacity(body.len() + scalarized.initializers.len());
        rewritten.extend(body[..prefix_end].iter().cloned());
        if first_block {
            rewritten.extend(scalarized.initializers.iter().cloned());
        }
        let Some(status) = rewrite_scalarized_body_suffix(
            function,
            body[suffix_start..].iter().cloned(),
            &mut allocation,
            constants,
            &mut rewritten,
        ) else {
            blocks[block_index].body = body;
            return ScalarReplacementAttempt::SkippedUnsupported;
        };
        blocks[block_index].body = rewritten;
        if matches!(status, ScalarizedBodyStatus::Deactivated) {
            return ScalarReplacementAttempt::Replaced;
        }

        if let Some(rewritten_term) =
            rewrite_scalarized_term(blocks[block_index].term.clone(), &mut allocation, constants)
        {
            blocks[block_index].term = rewritten_term;
            return ScalarReplacementAttempt::Replaced;
        }
        match &blocks[block_index].term {
            BlockTerm::Jump(edge)
                if edge.args.is_empty()
                    && normal_predecessor_counts.get(&edge.target).copied() == Some(1) =>
            {
                let Some(target_index) = block_index_by_label.get(&edge.target).copied() else {
                    return ScalarReplacementAttempt::SkippedUnsupported;
                };
                if !blocks[target_index].params.is_empty() {
                    return ScalarReplacementAttempt::SkippedUnsupported;
                }
                next_block_index = Some(target_index);
            }
            BlockTerm::Return(term) if !instr_references_any_alias(term, &allocation.aliases) => {
                return ScalarReplacementAttempt::Replaced;
            }
            BlockTerm::Raise(term)
                if term
                    .exc
                    .as_ref()
                    .is_none_or(|exc| !instr_references_any_alias(exc, &allocation.aliases)) =>
            {
                return ScalarReplacementAttempt::Replaced;
            }
            BlockTerm::IfTerm(_) | BlockTerm::BranchTable(_) | BlockTerm::Jump(_)
                if allocation.aliases.is_empty()
                    && !term_references_any_alias(
                        &blocks[block_index].term,
                        &allocation.aliases,
                    ) =>
            {
                return ScalarReplacementAttempt::Replaced;
            }
            BlockTerm::IfTerm(_) | BlockTerm::BranchTable(_) | BlockTerm::Jump(_)
                if !allocation.aliases.is_empty()
                    && !term_references_any_alias(
                        &blocks[block_index].term,
                        &allocation.aliases,
                    ) =>
            {
                return ScalarReplacementAttempt::SkippedLiveAliasControlFlow;
            }
            _ => return ScalarReplacementAttempt::SkippedUnsupported,
        }
        first_block = false;
    }
    ScalarReplacementAttempt::Replaced
}

fn try_scalar_replace_reachable_control_flow(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    blocks: &mut Vec<Block<InstrCodegen>>,
    start_block_index: usize,
    inline_plan: &InlinePlanModule,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
    constants: &[InstrResolved],
    block_index_by_label: &HashMap<BlockLabel, usize>,
) -> ScalarReplacementAttempt {
    let original_blocks = blocks.to_vec();
    let original_storage_layout = function.storage_layout.clone();
    let attempt = try_scalar_replace_reachable_control_flow_in_place(
        function,
        blocks,
        start_block_index,
        inline_plan,
        straightline_constructor_ids,
        constants,
        block_index_by_label,
    );
    if attempt != ScalarReplacementAttempt::Replaced {
        *blocks = original_blocks;
        function.storage_layout = original_storage_layout;
    }
    attempt
}

fn try_scalar_replace_reachable_control_flow_in_place(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    blocks: &mut Vec<Block<InstrCodegen>>,
    start_block_index: usize,
    inline_plan: &InlinePlanModule,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
    constants: &[InstrResolved],
    block_index_by_label: &HashMap<BlockLabel, usize>,
) -> ScalarReplacementAttempt {
    let Some(candidate) = find_scalarizable_allocation_candidate(
        &blocks[start_block_index],
        straightline_constructor_ids,
    ) else {
        return ScalarReplacementAttempt::NoCandidate;
    };
    let Some(scalarized) = build_scalarized_allocation(
        function,
        inline_plan,
        candidate.local_location,
        &candidate.call,
        constants,
    ) else {
        return ScalarReplacementAttempt::SkippedUnbuildableAllocation;
    };

    let start_label = blocks[start_block_index].label;
    let mut allocation = scalarized.allocation;
    let mut region = reachable_scalar_alias_region(
        function,
        blocks,
        start_block_index,
        block_index_by_label,
        &allocation.aliases,
    );
    let entry_label = blocks
        .first()
        .expect("BlockPyFunction should have at least one block")
        .label;
    isolate_scalar_region_external_predecessors(
        function,
        blocks,
        &mut region,
        start_block_index,
        entry_label,
    );
    if !region_is_closed_to_normal_predecessors(blocks, &region, start_label, entry_label) {
        return scalar_control_flow_skip("region_not_closed");
    }

    if !remove_scalarized_alias_block_params(
        function,
        blocks,
        &region,
        start_label,
        &allocation.aliases,
    ) {
        return scalar_control_flow_skip("alias_block_params");
    }
    for block_index in 0..blocks.len() {
        if !region.contains(&block_index) {
            continue;
        }
        let active_region_labels = reachable_region_labels_from(blocks, &region, start_label);
        if !active_region_labels.contains(&blocks[block_index].label) {
            continue;
        }
        let is_start_block = block_index == start_block_index;
        let prefix_end = if is_start_block {
            candidate.instr_index
        } else {
            0
        };
        let suffix_start = if is_start_block {
            candidate.instr_index + 1
        } else {
            0
        };
        let body = std::mem::take(&mut blocks[block_index].body);
        let mut rewritten = Vec::with_capacity(body.len() + scalarized.initializers.len());
        rewritten.extend(body[..prefix_end].iter().cloned());
        if is_start_block {
            rewritten.extend(scalarized.initializers.iter().cloned());
        }
        let aliases_before_block = allocation.aliases.clone();
        let Some(status) = rewrite_scalarized_body_suffix(
            function,
            body[suffix_start..].iter().cloned(),
            &mut allocation,
            constants,
            &mut rewritten,
        ) else {
            blocks[block_index].body = body;
            return scalar_control_flow_skip("body_rewrite_failed");
        };
        if matches!(status, ScalarizedBodyStatus::Deactivated) {
            blocks[block_index].body = body;
            return scalar_control_flow_skip("alias_deactivated");
        }
        blocks[block_index].body = rewritten;

        if let Some(rewritten_term) =
            rewrite_scalarized_term(blocks[block_index].term.clone(), &mut allocation, constants)
        {
            blocks[block_index].term = rewritten_term;
        }
        if !is_start_block
            && !reconcile_block_local_aliases(
                function,
                blocks,
                &region,
                start_label,
                blocks[block_index].label,
                &aliases_before_block,
                &mut allocation,
            )
        {
            return scalar_control_flow_skip("alias_mutated_after_start");
        }
        if term_references_any_alias(&blocks[block_index].term, &allocation.aliases)
            || block_term_edge_args_reference_any_alias(
                &blocks[block_index].term,
                function,
                &allocation.aliases,
            )
            || block_exc_edge_references_any_alias(
                blocks[block_index].exc_edge.as_ref(),
                function,
                &allocation.aliases,
            )
        {
            return scalar_control_flow_skip("term_or_exception_edge_references_alias");
        }
    }

    ScalarReplacementAttempt::Replaced
}

fn reconcile_block_local_aliases(
    function: &BlockPyFunction<CodegenModuleShape>,
    blocks: &[Block<InstrCodegen>],
    region: &HashSet<usize>,
    start_label: BlockLabel,
    current_label: BlockLabel,
    aliases_before_block: &HashSet<LocalLocation>,
    allocation: &mut ScalarizedAllocation,
) -> bool {
    if allocation.aliases == *aliases_before_block {
        return true;
    }
    let removed_aliases = aliases_before_block
        .difference(&allocation.aliases)
        .copied()
        .collect::<HashSet<_>>();
    if !removed_aliases.is_empty() {
        let references_after_remove = active_region_references_aliases(
            function,
            blocks,
            region,
            start_label,
            &removed_aliases,
        );
        if !references_after_remove.is_empty() {
            tracing::debug!(
                target: "soac_blockpy_scalar_replace",
                removed_aliases = ?removed_aliases,
                referencing_blocks = ?references_after_remove,
                "scalar block removed aliases that remain live"
            );
            return false;
        }
    }

    let added_aliases = allocation
        .aliases
        .difference(aliases_before_block)
        .copied()
        .collect::<HashSet<_>>();
    if added_aliases.is_empty() {
        return true;
    }
    let referencing_blocks =
        active_region_references_aliases(function, blocks, region, start_label, &added_aliases);
    if referencing_blocks.is_empty() {
        allocation
            .aliases
            .retain(|alias| !added_aliases.contains(alias));
        return true;
    }
    if referencing_blocks.iter().all(|target_label| {
        region_label_dominates(blocks, region, start_label, current_label, *target_label)
    }) {
        return true;
    }
    let referencing_block_debug = region
        .iter()
        .filter_map(|block_index| {
            let label = blocks[*block_index].label;
            referencing_blocks
                .contains(&label)
                .then_some(format!("{:?}", blocks[*block_index]))
        })
        .collect::<Vec<_>>()
        .join("\n");
    tracing::debug!(
        target: "soac_blockpy_scalar_replace",
        added_aliases = ?added_aliases,
        referencing_blocks = ?referencing_blocks,
        referencing_block_debug,
        "scalar block added aliases without dominating all active uses"
    );
    false
}

fn active_region_references_aliases(
    function: &BlockPyFunction<CodegenModuleShape>,
    blocks: &[Block<InstrCodegen>],
    region: &HashSet<usize>,
    start_label: BlockLabel,
    aliases: &HashSet<LocalLocation>,
) -> Vec<BlockLabel> {
    let active_labels = reachable_region_labels_from(blocks, region, start_label);
    region
        .iter()
        .filter(|block_index| {
            let block = &blocks[**block_index];
            active_labels.contains(&block.label)
                && block_references_scalar_alias(function, block, aliases)
        })
        .map(|block_index| blocks[*block_index].label)
        .collect()
}

fn region_label_dominates(
    blocks: &[Block<InstrCodegen>],
    region: &HashSet<usize>,
    start_label: BlockLabel,
    dominator_label: BlockLabel,
    target_label: BlockLabel,
) -> bool {
    if dominator_label == target_label {
        return true;
    }
    let region_labels = region
        .iter()
        .map(|block_index| blocks[*block_index].label)
        .collect::<HashSet<_>>();
    let block_index_by_label = block_index_by_label(blocks);
    let mut reachable_without_dominator = HashSet::new();
    let mut stack = vec![start_label];
    while let Some(label) = stack.pop() {
        if label == dominator_label || !region_labels.contains(&label) {
            continue;
        }
        if !reachable_without_dominator.insert(label) {
            continue;
        }
        if label == target_label {
            return false;
        }
        if let Some(block_index) = block_index_by_label.get(&label).copied() {
            stack.extend(block_successors_including_exceptions(&blocks[block_index]));
        }
    }
    true
}

fn reachable_region_labels_from(
    blocks: &[Block<InstrCodegen>],
    region: &HashSet<usize>,
    start_label: BlockLabel,
) -> HashSet<BlockLabel> {
    let region_labels = region
        .iter()
        .map(|block_index| blocks[*block_index].label)
        .collect::<HashSet<_>>();
    let block_index_by_label = block_index_by_label(blocks);
    let mut reachable = HashSet::new();
    let mut stack = vec![start_label];
    while let Some(label) = stack.pop() {
        if !region_labels.contains(&label) || !reachable.insert(label) {
            continue;
        }
        let Some(block_index) = block_index_by_label.get(&label).copied() else {
            continue;
        };
        stack.extend(block_successors_including_exceptions(&blocks[block_index]));
    }
    reachable
}

fn scalar_control_flow_skip(reason: &'static str) -> ScalarReplacementAttempt {
    tracing::debug!(
        target: "soac_blockpy_scalar_replace",
        reason,
        "skipped scalar replacement across control flow"
    );
    ScalarReplacementAttempt::SkippedLiveAliasControlFlow
}

fn isolate_scalar_region_external_predecessors(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    blocks: &mut Vec<Block<InstrCodegen>>,
    region: &mut HashSet<usize>,
    start_block_index: usize,
    entry_label: BlockLabel,
) {
    let start_label = blocks[start_block_index].label;
    let labels_in_region = region
        .iter()
        .map(|index| blocks[*index].label)
        .collect::<HashSet<_>>();
    let predecessors = predecessor_labels_including_exceptions(blocks);
    let entry_reachable = reachable_block_labels_from(blocks, entry_label);
    let has_external_predecessor = region.iter().any(|block_index| {
        let label = blocks[*block_index].label;
        label != start_label
            && predecessors.get(&label).is_some_and(|block_predecessors| {
                block_predecessors.iter().any(|predecessor| {
                    !labels_in_region.contains(predecessor) && entry_reachable.contains(predecessor)
                })
            })
    });
    if !has_external_predecessor {
        return;
    }

    let mut clone_sources = region
        .iter()
        .copied()
        .filter(|block_index| *block_index != start_block_index)
        .collect::<Vec<_>>();
    clone_sources.sort_by_key(|block_index| blocks[*block_index].label.as_u32());
    if clone_sources.is_empty() {
        return;
    }

    let mut label_map = HashMap::new();
    for block_index in &clone_sources {
        label_map.insert(
            blocks[*block_index].label,
            function.name_gen.next_block_name(),
        );
    }

    let mut clone_indices = Vec::with_capacity(clone_sources.len());
    for block_index in clone_sources {
        let mut duplicate = blocks[block_index].clone();
        duplicate.label = *label_map
            .get(&duplicate.label)
            .expect("clone source should have a replacement label");
        rewrite_block_targets_with_label_map(&mut duplicate, &label_map);
        blocks.push(duplicate);
        clone_indices.push(blocks.len() - 1);
    }
    rewrite_block_targets_with_label_map(&mut blocks[start_block_index], &label_map);

    region.clear();
    region.insert(start_block_index);
    region.extend(clone_indices);
    tracing::debug!(
        target: "soac_blockpy_scalar_replace",
        cloned_blocks = label_map.len(),
        "isolated scalar control-flow region from external predecessors"
    );
}

fn rewrite_block_targets_with_label_map(
    block: &mut Block<InstrCodegen>,
    label_map: &HashMap<BlockLabel, BlockLabel>,
) {
    for (old_label, new_label) in label_map {
        block.term.replace_target(*old_label, *new_label);
        if let Some(edge) = &mut block.exc_edge {
            if edge.target == *old_label {
                edge.target = *new_label;
            }
        }
    }
}

fn remove_scalarized_alias_block_params(
    function: &BlockPyFunction<CodegenModuleShape>,
    blocks: &mut [Block<InstrCodegen>],
    region: &HashSet<usize>,
    start_label: BlockLabel,
    aliases: &HashSet<LocalLocation>,
) -> bool {
    let alias_names = aliases
        .iter()
        .filter_map(|location| local_name_for_location(function, *location))
        .map(str::to_string)
        .collect::<HashSet<_>>();
    if alias_names.is_empty() {
        return true;
    }

    let labels_in_region = region
        .iter()
        .map(|index| blocks[*index].label)
        .collect::<HashSet<_>>();
    let mut removals_by_target = HashMap::<BlockLabel, Vec<usize>>::new();
    for block_index in region {
        let block = &blocks[*block_index];
        let remove_indices = block
            .params
            .iter()
            .enumerate()
            .filter_map(|(index, param)| alias_names.contains(&param.name).then_some(index))
            .collect::<Vec<_>>();
        if remove_indices.is_empty() {
            continue;
        }
        if block.label == start_label {
            return false;
        }
        removals_by_target.insert(block.label, remove_indices);
    }
    if removals_by_target.is_empty() {
        return true;
    }

    for block_index in region {
        let block = &blocks[*block_index];
        for target in block_term_successors(&block.term) {
            let Some(remove_indices) = removals_by_target.get(&target) else {
                continue;
            };
            if !labels_in_region.contains(&target) {
                return false;
            }
            let BlockTerm::Jump(edge) = &block.term else {
                return false;
            };
            if edge.target != target {
                return false;
            }
            for remove_index in remove_indices {
                let Some(arg) = edge.args.get(*remove_index) else {
                    return false;
                };
                if !block_arg_references_alias_name(arg, &alias_names) {
                    return false;
                }
            }
        }
    }

    for block_index in 0..blocks.len() {
        let block_label = blocks[block_index].label;
        if let Some(remove_indices) = removals_by_target.get(&block_label) {
            remove_indices_descending(remove_indices, &mut blocks[block_index].params);
        }
        let BlockTerm::Jump(edge) = &mut blocks[block_index].term else {
            continue;
        };
        if let Some(remove_indices) = removals_by_target.get(&edge.target) {
            remove_indices_descending(remove_indices, &mut edge.args);
        }
    }
    true
}

fn block_arg_references_alias_name(arg: &BlockArg, alias_names: &HashSet<String>) -> bool {
    match arg {
        BlockArg::Name(name) => alias_names.contains(name),
        BlockArg::None | BlockArg::CurrentException | BlockArg::AbruptKind(_) => false,
    }
}

fn remove_indices_descending<T>(indices: &[usize], values: &mut Vec<T>) {
    for index in indices.iter().rev() {
        values.remove(*index);
    }
}

fn reachable_scalar_alias_region(
    function: &BlockPyFunction<CodegenModuleShape>,
    blocks: &[Block<InstrCodegen>],
    start_block_index: usize,
    block_index_by_label: &HashMap<BlockLabel, usize>,
    aliases: &HashSet<LocalLocation>,
) -> HashSet<usize> {
    let mut reachable = HashSet::new();
    let mut stack = vec![start_block_index];
    while let Some(block_index) = stack.pop() {
        if !reachable.insert(block_index) {
            continue;
        }
        for target in block_successors_including_exceptions(&blocks[block_index]) {
            if let Some(target_index) = block_index_by_label.get(&target).copied() {
                stack.push(target_index);
            }
        }
    }

    let mut predecessors = HashMap::<BlockLabel, Vec<BlockLabel>>::new();
    for block_index in &reachable {
        let block = &blocks[*block_index];
        for target in block_successors_including_exceptions(block) {
            predecessors.entry(target).or_default().push(block.label);
        }
    }

    let mut region = HashSet::from([start_block_index]);
    let mut worklist = reachable
        .iter()
        .copied()
        .filter(|block_index| {
            block_references_scalar_alias(function, &blocks[*block_index], aliases)
        })
        .collect::<Vec<_>>();
    while let Some(block_index) = worklist.pop() {
        if !region.insert(block_index) {
            continue;
        }
        if block_index == start_block_index {
            continue;
        }
        let Some(block_predecessors) = predecessors.get(&blocks[block_index].label) else {
            continue;
        };
        for predecessor in block_predecessors {
            if let Some(predecessor_index) = block_index_by_label.get(predecessor).copied() {
                if reachable.contains(&predecessor_index) {
                    worklist.push(predecessor_index);
                }
            }
        }
    }
    region
}

fn block_references_scalar_alias(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &Block<InstrCodegen>,
    aliases: &HashSet<LocalLocation>,
) -> bool {
    block
        .body
        .iter()
        .any(|instr| instr_references_any_alias(instr, aliases))
        || term_references_any_alias(&block.term, aliases)
        || block_term_edge_args_reference_any_alias(&block.term, function, aliases)
        || block_exc_edge_references_any_alias(block.exc_edge.as_ref(), function, aliases)
        || block.params.iter().any(|param| {
            aliases.iter().any(|location| {
                local_name_for_location(function, *location).is_some_and(|name| name == param.name)
            })
        })
}

fn region_is_closed_to_normal_predecessors(
    blocks: &[Block<InstrCodegen>],
    region: &HashSet<usize>,
    start_label: BlockLabel,
    entry_label: BlockLabel,
) -> bool {
    let labels_in_region = region
        .iter()
        .map(|index| blocks[*index].label)
        .collect::<HashSet<_>>();
    let predecessors = predecessor_labels_including_exceptions(blocks);
    let entry_reachable = reachable_block_labels_from(blocks, entry_label);
    for block_index in region {
        let label = blocks[*block_index].label;
        if label == start_label {
            continue;
        }
        let Some(block_predecessors) = predecessors.get(&label) else {
            continue;
        };
        let reachable_outside_predecessors = block_predecessors
            .iter()
            .filter(|predecessor| {
                !labels_in_region.contains(predecessor) && entry_reachable.contains(predecessor)
            })
            .collect::<Vec<_>>();
        if !reachable_outside_predecessors.is_empty() {
            let outside_predecessors = reachable_outside_predecessors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let outside_predecessor_blocks = reachable_outside_predecessors
                .iter()
                .filter_map(|predecessor| blocks.iter().find(|block| block.label == **predecessor))
                .map(|block| format!("{block:?}"))
                .collect::<Vec<_>>()
                .join("\n");
            let region_labels = labels_in_region
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            tracing::debug!(
                target: "soac_blockpy_scalar_replace",
                block = %label,
                outside_predecessors,
                block_debug = ?blocks[*block_index],
                outside_predecessor_blocks,
                region_labels,
                "scalar control-flow region is not closed"
            );
            return false;
        }
    }
    true
}

fn reachable_block_labels_from(
    blocks: &[Block<InstrCodegen>],
    start_label: BlockLabel,
) -> HashSet<BlockLabel> {
    let block_index_by_label = block_index_by_label(blocks);
    let mut reachable = HashSet::new();
    let mut stack = vec![start_label];
    while let Some(label) = stack.pop() {
        if !reachable.insert(label) {
            continue;
        }
        let Some(block_index) = block_index_by_label.get(&label).copied() else {
            continue;
        };
        stack.extend(block_successors_including_exceptions(&blocks[block_index]));
    }
    reachable
}

fn predecessor_labels_including_exceptions(
    blocks: &[Block<InstrCodegen>],
) -> HashMap<BlockLabel, HashSet<BlockLabel>> {
    let mut predecessors: HashMap<BlockLabel, HashSet<BlockLabel>> = HashMap::new();
    for block in blocks {
        for target in block_successors_including_exceptions(block) {
            predecessors.entry(target).or_default().insert(block.label);
        }
    }
    predecessors
}

fn block_successors_including_exceptions(block: &Block<InstrCodegen>) -> Vec<BlockLabel> {
    let mut targets = block_term_successors(&block.term);
    if let Some(edge) = &block.exc_edge {
        targets.push(edge.target);
    }
    targets
}

fn block_term_successors(term: &BlockTerm<InstrCodegen>) -> Vec<BlockLabel> {
    match term {
        BlockTerm::Jump(edge) => vec![edge.target],
        BlockTerm::IfTerm(term) => vec![term.then_label, term.else_label],
        BlockTerm::BranchTable(term) => {
            let mut targets = term.targets.clone();
            targets.push(term.default_label);
            targets
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => Vec::new(),
    }
}

struct ScalarizedAllocationCandidate {
    instr_index: usize,
    local_location: LocalLocation,
    call: CallDirect<InstrCodegen>,
}

fn find_scalarizable_allocation_candidate(
    block: &Block<InstrCodegen>,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
) -> Option<ScalarizedAllocationCandidate> {
    block
        .body
        .iter()
        .enumerate()
        .find_map(|(instr_index, instr)| {
            let InstrCodegenOp::Store(store) = instr else {
                return None;
            };
            let local_location = store.name.location.as_local()?;
            let InstrCodegenOp::CallDirect(call) = store.value.as_ref() else {
                return None;
            };
            if !straightline_constructor_ids.contains(&call.function_id) {
                return None;
            }
            Some(ScalarizedAllocationCandidate {
                instr_index,
                local_location,
                call: call.clone(),
            })
        })
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum ScalarizedBodyStatus {
    Active,
    Deactivated,
}

fn rewrite_scalarized_body_suffix(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    instrs: impl IntoIterator<Item = InstrCodegen>,
    allocation: &mut ScalarizedAllocation,
    constants: &[InstrResolved],
    rewritten: &mut Vec<InstrCodegen>,
) -> Option<ScalarizedBodyStatus> {
    for instr in instrs {
        if record_or_remove_scalarized_alias(&instr, allocation) {
            continue;
        }
        if let Some(rewritten_instr) =
            rewrite_scalarized_instr_root(function, instr.clone(), allocation, constants)
        {
            rewritten.push(rewritten_instr);
            continue;
        }
        if is_store_to_any_alias(&instr, &allocation.aliases) {
            rewritten.push(instr);
            return Some(ScalarizedBodyStatus::Deactivated);
        }
        if instr_references_any_alias(&instr, &allocation.aliases) {
            tracing::debug!(
                target: "soac_blockpy_scalar_replace",
                instr = ?instr,
                aliases = ?allocation.aliases,
                "scalar body rewrite found unsupported alias reference"
            );
            return None;
        }
        rewritten.push(instr);
    }
    Some(ScalarizedBodyStatus::Active)
}

fn block_index_by_label(blocks: &[Block<InstrCodegen>]) -> HashMap<BlockLabel, usize> {
    blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect()
}

fn normal_predecessor_counts(blocks: &[Block<InstrCodegen>]) -> HashMap<BlockLabel, usize> {
    let mut counts = HashMap::new();
    for block in blocks {
        match &block.term {
            BlockTerm::Jump(edge) => {
                *counts.entry(edge.target).or_insert(0) += 1;
            }
            BlockTerm::IfTerm(term) => {
                *counts.entry(term.then_label).or_insert(0) += 1;
                *counts.entry(term.else_label).or_insert(0) += 1;
            }
            BlockTerm::BranchTable(term) => {
                for target in &term.targets {
                    *counts.entry(*target).or_insert(0) += 1;
                }
                *counts.entry(term.default_label).or_insert(0) += 1;
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
        }
    }
    counts
}

struct ScalarizedAllocationBuild {
    allocation: ScalarizedAllocation,
    initializers: Vec<InstrCodegen>,
}

fn build_scalarized_allocation(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    inline_plan: &InlinePlanModule,
    object_location: LocalLocation,
    call: &CallDirect<InstrCodegen>,
    constants: &[InstrResolved],
) -> Option<ScalarizedAllocationBuild> {
    let constructor = inline_plan.straightline_constructor(call.function_id)?;
    let mut fields = HashMap::new();
    let mut initializers = Vec::new();
    let arg_locals = scalar_argument_locals(function, call, constants)?;
    initializers.extend(arg_locals.iter().map(|arg| {
        Store::new(arg.local.resolved_name(), Box::new(arg.value.clone()))
            .with_meta(call.meta())
            .into()
    }));
    for field_store in &constructor.field_stores {
        let value = scalar_initializer_for_field(constructor, &field_store.value, &arg_locals)?;
        let field_local = allocate_scalar_field_local(function);
        initializers.push(
            Store::new(field_local.resolved_name(), Box::new(value))
                .with_meta(call.meta())
                .into(),
        );
        fields.insert(field_store.field_name.clone(), field_local);
    }
    Some(ScalarizedAllocationBuild {
        allocation: ScalarizedAllocation {
            object_location,
            aliases: HashSet::from([object_location]),
            fields,
        },
        initializers,
    })
}

struct ScalarArgumentLocal {
    local: InlineLocal,
    value: InstrCodegen,
}

fn scalar_argument_locals(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    call: &CallDirect<InstrCodegen>,
    constants: &[InstrResolved],
) -> Option<Vec<ScalarArgumentLocal>> {
    if !call.keywords.is_empty() {
        return None;
    }
    call.args
        .iter()
        .map(|arg| {
            let CallArgPositional::Positional(value) = arg else {
                return None;
            };
            if !is_scalar_evaluatable_argument_expr(value, constants) {
                return None;
            }
            Some(ScalarArgumentLocal {
                local: allocate_scalar_arg_local(function),
                value: clear_codegen_instr_ids(value.clone()),
            })
        })
        .collect()
}

fn scalar_initializer_for_field(
    constructor: &crate::passes::StraightlineConstructorInlinePlan,
    value: &ConstructorFieldValue,
    arg_locals: &[ScalarArgumentLocal],
) -> Option<InstrCodegen> {
    match value {
        ConstructorFieldValue::Param {
            index, location, ..
        } => {
            if *location == constructor.self_location {
                return None;
            }
            let call_arg_index = if constructor.self_location.is_some() {
                index.checked_sub(1)?
            } else {
                *index
            };
            let arg = arg_locals.get(call_arg_index)?;
            Some(clear_codegen_instr_id(
                soac_core::block_py::Load::new(arg.local.resolved_name()).into(),
            ))
        }
        ConstructorFieldValue::Local { .. }
        | ConstructorFieldValue::Constant { .. }
        | ConstructorFieldValue::Other => None,
    }
}

fn is_scalar_evaluatable_argument_expr(instr: &InstrCodegen, constants: &[InstrResolved]) -> bool {
    match instr {
        InstrCodegenOp::Load(load) => {
            load.name.location.as_local().is_some()
                || constant_string_from_constants(constants, instr).is_some()
                || matches!(load.name.location, NameLocation::RuntimeName(_))
        }
        InstrCodegenOp::GetAttr(getattr) => {
            is_scalar_evaluatable_argument_expr(&getattr.value, constants)
                && constant_string_from_constants(constants, &getattr.attr).is_some()
        }
        _ => false,
    }
}

fn allocate_scalar_arg_local(function: &mut BlockPyFunction<CodegenModuleShape>) -> InlineLocal {
    let temp = allocate_codegen_stack_temp(function, "scalar_arg");
    InlineLocal {
        name: temp.name,
        location: temp.location,
    }
}

fn allocate_scalar_field_local(function: &mut BlockPyFunction<CodegenModuleShape>) -> InlineLocal {
    let temp = allocate_codegen_stack_temp(function, "scalar_field");
    InlineLocal {
        name: temp.name,
        location: temp.location,
    }
}

fn rewrite_scalarized_instr_root(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    instr: InstrCodegen,
    allocation: &mut ScalarizedAllocation,
    constants: &[InstrResolved],
) -> Option<InstrCodegen> {
    match instr {
        InstrCodegenOp::GetAttr(getattr)
            if is_local_alias_load(&getattr.value, &allocation.aliases) =>
        {
            let field_name = constant_string_from_constants(constants, &getattr.attr)?;
            let field = allocation.fields.get(field_name.as_str())?;
            Some(clear_codegen_instr_id(
                soac_core::block_py::Load::new(field.resolved_name()).into(),
            ))
        }
        InstrCodegenOp::SetAttr(setattr)
            if is_local_alias_load(&setattr.value, &allocation.aliases) =>
        {
            let field_name = constant_string_from_constants(constants, &setattr.attr)?;
            let field = allocation
                .fields
                .entry(field_name)
                .or_insert_with(|| allocate_scalar_field_local(function));
            Some(clear_codegen_instr_id(
                Store::new(field.resolved_name(), setattr.replacement).into(),
            ))
        }
        InstrCodegenOp::Store(store) => {
            let InstrCodegenOp::GetAttr(getattr) = store.value.as_ref() else {
                return None;
            };
            if !is_local_alias_load(&getattr.value, &allocation.aliases) {
                return None;
            }
            let field_name = constant_string_from_constants(constants, &getattr.attr)?;
            let field = allocation.fields.get(field_name.as_str())?;
            let field_load: InstrCodegen =
                soac_core::block_py::Load::new(field.resolved_name()).into();
            let meta = store.meta();
            Some(clear_codegen_instr_id(
                Store::new(store.name, field_load).with_meta(meta).into(),
            ))
        }
        _ => None,
    }
}

fn record_or_remove_scalarized_alias(
    instr: &InstrCodegen,
    allocation: &mut ScalarizedAllocation,
) -> bool {
    match instr {
        InstrCodegenOp::Store(store) => {
            let Some(target_location) = store.name.location.as_local() else {
                return false;
            };
            if is_local_alias_load(&store.value, &allocation.aliases) {
                allocation.aliases.insert(target_location);
                return true;
            }
            false
        }
        InstrCodegenOp::Del(del) => {
            let Some(target_location) = del.name.location.as_local() else {
                return false;
            };
            if target_location != allocation.object_location
                && allocation.aliases.remove(&target_location)
            {
                return true;
            }
            false
        }
        _ => false,
    }
}

fn rewrite_scalarized_term(
    term: BlockTerm<InstrCodegen>,
    allocation: &mut ScalarizedAllocation,
    constants: &[InstrResolved],
) -> Option<BlockTerm<InstrCodegen>> {
    match term {
        BlockTerm::Return(value) => {
            let InstrCodegenOp::GetAttr(getattr) = value else {
                return None;
            };
            if !is_local_alias_load(&getattr.value, &allocation.aliases) {
                return None;
            }
            let field_name = constant_string_from_constants(constants, &getattr.attr)?;
            let field = allocation.fields.get(field_name.as_str())?;
            Some(BlockTerm::Return(clear_codegen_instr_id(
                soac_core::block_py::Load::new(field.resolved_name()).into(),
            )))
        }
        BlockTerm::IfTerm(_) => None,
        BlockTerm::BranchTable(_) | BlockTerm::Raise(_) | BlockTerm::Jump(_) => None,
    }
}

fn is_store_to_any_alias(instr: &InstrCodegen, aliases: &HashSet<LocalLocation>) -> bool {
    let InstrCodegenOp::Store(store) = instr else {
        return false;
    };
    store
        .name
        .location
        .as_local()
        .is_some_and(|location| aliases.contains(&location))
}

fn is_local_alias_load(instr: &InstrCodegen, aliases: &HashSet<LocalLocation>) -> bool {
    let InstrCodegenOp::Load(load) = instr else {
        return false;
    };
    load.name
        .location
        .as_local()
        .is_some_and(|location| aliases.contains(&location))
}

fn instr_references_any_alias(instr: &InstrCodegen, aliases: &HashSet<LocalLocation>) -> bool {
    match instr {
        InstrCodegenOp::Store(store)
            if store
                .name
                .location
                .as_local()
                .is_some_and(|location| aliases.contains(&location)) =>
        {
            true
        }
        InstrCodegenOp::Del(del)
            if del
                .name
                .location
                .as_local()
                .is_some_and(|location| aliases.contains(&location)) =>
        {
            true
        }
        _ => instr_any(instr, |child| match child {
            InstrCodegenOp::Load(load) => load
                .name
                .location
                .as_local()
                .is_some_and(|location| aliases.contains(&location)),
            _ => false,
        }),
    }
}

fn term_references_any_alias(
    term: &BlockTerm<InstrCodegen>,
    aliases: &HashSet<LocalLocation>,
) -> bool {
    match term {
        BlockTerm::Return(value) => instr_references_any_alias(value, aliases),
        BlockTerm::IfTerm(term) => instr_references_any_alias(&term.test, aliases),
        BlockTerm::BranchTable(term) => instr_references_any_alias(&term.index, aliases),
        BlockTerm::Raise(term) => term
            .exc
            .as_ref()
            .is_some_and(|exc| instr_references_any_alias(exc, aliases)),
        BlockTerm::Jump(_) => false,
    }
}

fn block_term_edge_args_reference_any_alias(
    term: &BlockTerm<InstrCodegen>,
    function: &BlockPyFunction<CodegenModuleShape>,
    aliases: &HashSet<LocalLocation>,
) -> bool {
    match term {
        BlockTerm::Jump(edge) => block_edge_references_any_alias(edge, function, aliases),
        BlockTerm::IfTerm(_)
        | BlockTerm::BranchTable(_)
        | BlockTerm::Raise(_)
        | BlockTerm::Return(_) => false,
    }
}

fn block_exc_edge_references_any_alias(
    edge: Option<&BlockEdge>,
    function: &BlockPyFunction<CodegenModuleShape>,
    aliases: &HashSet<LocalLocation>,
) -> bool {
    edge.is_some_and(|edge| block_edge_references_any_alias(edge, function, aliases))
}

fn block_edge_references_any_alias(
    edge: &BlockEdge,
    function: &BlockPyFunction<CodegenModuleShape>,
    aliases: &HashSet<LocalLocation>,
) -> bool {
    edge.args.iter().any(|arg| {
        let BlockArg::Name(name) = arg else {
            return false;
        };
        aliases.iter().any(|location| {
            local_name_for_location(function, *location).is_some_and(|alias| alias == name)
        })
    })
}

fn local_name_for_location(
    function: &BlockPyFunction<CodegenModuleShape>,
    location: LocalLocation,
) -> Option<&str> {
    function
        .storage_layout
        .as_ref()?
        .stack_slots()
        .get(location.slot() as usize)
        .map(String::as_str)
}

fn constant_string_from_constants(
    constants: &[InstrResolved],
    instr: &InstrCodegen,
) -> Option<String> {
    let InstrCodegenOp::Load(load) = instr else {
        return None;
    };
    let constant_index = load.name.location.as_constant()? as usize;
    match constants.get(constant_index)? {
        InstrResolved::Literal(value) => match value.as_literal() {
            Literal::StringLiteral(value) => Some(value.value.clone()),
            Literal::BytesLiteral(_) | Literal::NumberLiteral(_) => None,
        },
        _ => None,
    }
}

struct InlineStoreCandidate {
    instr_index: usize,
    callee_id: RuntimeFunctionId,
    call: CallDirect<InstrCodegen>,
    target: ResolvedName,
}

fn find_inline_store_candidate(
    block: &Block<InstrCodegen>,
    caller_id: RuntimeFunctionId,
    inline_plan: &InlinePlanModule,
    callees: &HashMap<RuntimeFunctionId, InlineCallee>,
) -> Option<InlineStoreCandidate> {
    block
        .body
        .iter()
        .enumerate()
        .find_map(|(instr_index, instr)| {
            let InstrCodegenOp::Store(store) = instr else {
                return None;
            };
            let InstrCodegenOp::CallDirect(call) = store.value.as_ref() else {
                return None;
            };
            if call.function_id == caller_id {
                return None;
            }
            let callee = callees.get(&call.function_id)?;
            if call_direct_looks_like_constructor_allocation(call, callee.function(), inline_plan) {
                return None;
            }
            Some(InlineStoreCandidate {
                instr_index,
                callee_id: call.function_id,
                call: call.clone(),
                target: store.name.clone(),
            })
        })
}

fn call_direct_looks_like_constructor_allocation(
    call: &CallDirect<InstrCodegen>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    inline_plan: &InlinePlanModule,
) -> bool {
    if callee.names.fn_name != "__init__"
        || inline_plan
            .straightline_constructor(call.function_id)
            .is_none()
        || !call.keywords.is_empty()
    {
        return false;
    }
    let mut positional_arg_count = 0usize;
    for arg in &call.args {
        match arg {
            CallArgPositional::Positional(_) => positional_arg_count += 1,
            CallArgPositional::Starred(_) => return false,
        }
    }
    let accepted_positional_args = callee
        .params
        .iter()
        .filter(|param| matches!(param.kind, ParamKind::PosOnly | ParamKind::Any))
        .count();
    positional_arg_count + 1 == accepted_positional_args
}

pub fn bind_simple_direct_call_inline_args(
    callee: &BlockPyFunction<CodegenModuleShape>,
    call: &CallDirect<InstrCodegen>,
) -> Result<InlineValueBindings, InlineUnsupportedReason> {
    if !call.keywords.is_empty() {
        return Err(InlineUnsupportedReason::KeywordArguments);
    }
    if call
        .args
        .iter()
        .any(|arg| matches!(arg, CallArgPositional::Starred(_)))
    {
        return Err(InlineUnsupportedReason::StarredArguments);
    }

    let values = call
        .args
        .iter()
        .map(|arg| {
            let CallArgPositional::Positional(value) = arg else {
                unreachable!("starred arguments were rejected before binding");
            };
            value.clone()
        })
        .collect::<Vec<_>>();
    bind_simple_direct_call_inline_values(callee, values)
}

pub fn bind_simple_direct_method_inline_args(
    callee: &BlockPyFunction<CodegenModuleShape>,
    receiver: InstrCodegen,
    args: &[CallArgPositional<InstrCodegen>],
) -> Result<InlineValueBindings, InlineUnsupportedReason> {
    if args
        .iter()
        .any(|arg| matches!(arg, CallArgPositional::Starred(_)))
    {
        return Err(InlineUnsupportedReason::StarredArguments);
    }

    let mut values = Vec::with_capacity(args.len() + 1);
    values.push(receiver);
    values.extend(args.iter().map(|arg| {
        let CallArgPositional::Positional(value) = arg else {
            unreachable!("starred arguments were rejected before binding");
        };
        value.clone()
    }));
    bind_simple_direct_call_inline_values(callee, values)
}

fn bind_simple_direct_call_inline_values(
    callee: &BlockPyFunction<CodegenModuleShape>,
    values: Vec<InstrCodegen>,
) -> Result<InlineValueBindings, InlineUnsupportedReason> {
    let supported_params = callee
        .params
        .iter()
        .map(|param| {
            if matches!(param.kind, ParamKind::PosOnly | ParamKind::Any) {
                Ok(param)
            } else {
                Err(InlineUnsupportedReason::UnsupportedParameterKind {
                    name: param.name.clone(),
                    kind: param.kind,
                })
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected = supported_params.len();
    let actual = values.len();
    if expected != actual {
        return Err(InlineUnsupportedReason::ArityMismatch { expected, actual });
    }

    let mut bindings = InlineValueBindings::new();
    for (param, value) in supported_params.into_iter().zip(values) {
        let location = parameter_local_location(callee, &param.name)?;
        bindings.insert(location, value);
    }
    Ok(bindings)
}

pub fn build_single_block_inline_fragment(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    build_single_block_inline_fragment_with_bindings(
        caller,
        callee,
        continuation,
        &InlineValueBindings::new(),
    )
}

pub fn build_single_block_inline_fragment_with_bindings(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &InlineValueBindings,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    build_single_block_inline_fragment_with_constant_scope(
        caller,
        callee,
        continuation,
        value_bindings,
        InlineReturnPlacement::FreshContinuationArg,
        InlineConstantScope::SameModule,
    )
}

pub fn build_single_block_inline_fragment_to_target(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &InlineValueBindings,
    return_target: ResolvedName,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    build_single_block_inline_fragment_with_constant_scope(
        caller,
        callee,
        continuation,
        value_bindings,
        InlineReturnPlacement::StoreTo(return_target),
        InlineConstantScope::SameModule,
    )
}

pub fn build_direct_call_inline_fragment_to_target(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &InlineValueBindings,
    return_target: ResolvedName,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    if callee.blocks.len() == 1 {
        return build_single_block_inline_fragment_to_target(
            caller,
            callee,
            continuation,
            value_bindings,
            return_target,
        );
    }
    build_multi_block_inline_fragment_to_target(
        caller,
        callee,
        continuation,
        value_bindings,
        return_target,
    )
}

pub fn build_cross_module_direct_call_inline_fragment_to_target(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    caller_constants: &mut Vec<InstrResolved>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    callee_constants: &[InstrResolved],
    continuation: BlockLabel,
    value_bindings: &InlineValueBindings,
    return_target: ResolvedName,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    if callee.blocks.len() == 1 {
        return build_single_block_inline_fragment_with_constant_scope(
            caller,
            callee,
            continuation,
            value_bindings,
            InlineReturnPlacement::StoreTo(return_target),
            InlineConstantScope::CrossModule(InlineConstantRemapper::new(
                caller_constants,
                callee_constants,
            )),
        );
    }
    build_multi_block_inline_fragment_to_target_impl(
        caller,
        callee,
        continuation,
        value_bindings,
        return_target,
        InlineConstantScope::CrossModule(InlineConstantRemapper::new(
            caller_constants,
            callee_constants,
        )),
    )
}

pub fn build_direct_method_inline_fragment_to_target(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
    receiver: InstrCodegen,
    args: &[CallArgPositional<InstrCodegen>],
    return_target: ResolvedName,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    let bindings = bind_simple_direct_method_inline_args(callee, receiver, args)?;
    build_direct_call_inline_fragment_to_target(
        caller,
        callee,
        continuation,
        &bindings,
        return_target,
    )
}

pub fn build_cross_module_direct_method_inline_fragment_to_target(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    caller_constants: &mut Vec<InstrResolved>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    callee_constants: &[InstrResolved],
    continuation: BlockLabel,
    receiver: InstrCodegen,
    args: &[CallArgPositional<InstrCodegen>],
    return_target: ResolvedName,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    let bindings = bind_simple_direct_method_inline_args(callee, receiver, args)?;
    build_cross_module_direct_call_inline_fragment_to_target(
        caller,
        caller_constants,
        callee,
        callee_constants,
        continuation,
        &bindings,
        return_target,
    )
}

fn build_multi_block_inline_fragment_to_target(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &InlineValueBindings,
    return_target: ResolvedName,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    build_multi_block_inline_fragment_to_target_impl(
        caller,
        callee,
        continuation,
        value_bindings,
        return_target,
        InlineConstantScope::SameModule,
    )
}

fn build_multi_block_inline_fragment_to_target_impl(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &InlineValueBindings,
    return_target: ResolvedName,
    mut constant_scope: InlineConstantScope<'_>,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(InlineUnsupportedReason::MissingCalleeStorageLayout)?;
    for location in value_bindings.keys().copied() {
        if location.slot() as usize >= callee_layout.stack_slots().len() {
            return Err(InlineUnsupportedReason::MissingCalleeLocal(location));
        }
    }
    if callee.blocks.len() > MAX_INLINE_DIRECT_CALL_BLOCKS {
        return Err(InlineUnsupportedReason::TooManyBlocks {
            count: callee.blocks.len(),
            max: MAX_INLINE_DIRECT_CALL_BLOCKS,
        });
    }
    for block in &callee.blocks {
        if !block.params.is_empty() {
            return Err(InlineUnsupportedReason::BlockParams);
        }
        if block.exc_edge.is_some() {
            return Err(InlineUnsupportedReason::ExceptionEdge);
        }
        if term_has_jump_args(&block.term) {
            return Err(InlineUnsupportedReason::JumpArgs);
        }
    }

    let mut locals = HashMap::new();
    for (slot, _name) in callee_layout.stack_slots().iter().enumerate() {
        let location =
            LocalLocation(u32::try_from(slot).expect("callee stack slot index should fit in u32"));
        if value_bindings.contains_key(&location) {
            continue;
        }
        let fresh = allocate_inline_local(caller)?;
        locals.insert(location, fresh);
    }

    let label_map = callee
        .blocks
        .iter()
        .map(|block| (block.label, caller.name_gen.next_block_name()))
        .collect::<HashMap<_, _>>();
    let entry_label = remapped_label(&label_map, callee.blocks[0].label)?;
    let mut remapper = InlineLocalRemapper::new(&locals, value_bindings, &mut constant_scope);
    let mut blocks = Vec::with_capacity(callee.blocks.len());
    for callee_block in &callee.blocks {
        let label = remapped_label(&label_map, callee_block.label)?;
        let mut body = callee_block
            .body
            .iter()
            .cloned()
            // Inlined profiling counters belong to the callee's counter
            // layout; the caller does not have storage for those ids.
            .filter(|instr| !matches!(instr, InstrCodegenOp::IncrementCounter(_)))
            .map(|instr| remapper.try_map_instr(instr))
            .collect::<Result<Vec<_>, _>>()?;
        let term = match &callee_block.term {
            BlockTerm::Return(value) => {
                let return_value = remapper.try_map_instr(value.clone())?;
                let return_meta = return_value.meta();
                body.push(
                    Store::new(return_target.clone(), Box::new(return_value))
                        .with_meta(return_meta)
                        .into(),
                );
                BlockTerm::Jump(BlockEdge::new(continuation))
            }
            term => remap_inline_term_labels(remapper.try_map_term(term.clone())?, &label_map)?,
        };
        blocks.push(Block::new(label, body, term, Vec::new(), None));
    }

    Ok(InlineFragment {
        entry_label,
        blocks,
        locals,
        return_local: None,
    })
}

fn term_has_jump_args(term: &BlockTerm<InstrCodegen>) -> bool {
    match term {
        BlockTerm::Jump(edge) => !edge.args.is_empty(),
        BlockTerm::IfTerm(_)
        | BlockTerm::BranchTable(_)
        | BlockTerm::Raise(_)
        | BlockTerm::Return(_) => false,
    }
}

fn remapped_label(
    label_map: &HashMap<BlockLabel, BlockLabel>,
    label: BlockLabel,
) -> Result<BlockLabel, InlineUnsupportedReason> {
    label_map
        .get(&label)
        .copied()
        .ok_or(InlineUnsupportedReason::UnknownLabel(label))
}

fn remap_inline_term_labels(
    term: BlockTerm<InstrCodegen>,
    label_map: &HashMap<BlockLabel, BlockLabel>,
) -> Result<BlockTerm<InstrCodegen>, InlineUnsupportedReason> {
    Ok(match term {
        BlockTerm::Jump(edge) => {
            BlockTerm::Jump(BlockEdge::new(remapped_label(label_map, edge.target)?))
        }
        BlockTerm::IfTerm(mut term) => {
            term.then_label = remapped_label(label_map, term.then_label)?;
            term.else_label = remapped_label(label_map, term.else_label)?;
            BlockTerm::IfTerm(term)
        }
        BlockTerm::BranchTable(mut term) => {
            for target in &mut term.targets {
                *target = remapped_label(label_map, *target)?;
            }
            term.default_label = remapped_label(label_map, term.default_label)?;
            BlockTerm::BranchTable(term)
        }
        BlockTerm::Raise(term) => BlockTerm::Raise(term),
        BlockTerm::Return(_) => return Err(InlineUnsupportedReason::NonReturnTerm),
    })
}

fn build_single_block_inline_fragment_with_constant_scope(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &InlineValueBindings,
    return_placement: InlineReturnPlacement,
    mut constant_scope: InlineConstantScope<'_>,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    let callee_layout = callee
        .storage_layout
        .as_ref()
        .ok_or(InlineUnsupportedReason::MissingCalleeStorageLayout)?;
    for location in value_bindings.keys().copied() {
        if location.slot() as usize >= callee_layout.stack_slots().len() {
            return Err(InlineUnsupportedReason::MissingCalleeLocal(location));
        }
    }
    if callee.blocks.len() != 1 {
        return Err(InlineUnsupportedReason::MultipleBlocks {
            count: callee.blocks.len(),
        });
    }
    let callee_block = &callee.blocks[0];
    if !callee_block.params.is_empty() {
        return Err(InlineUnsupportedReason::BlockParams);
    }
    if callee_block.exc_edge.is_some() {
        return Err(InlineUnsupportedReason::ExceptionEdge);
    }

    let BlockTerm::Return(return_value) = &callee_block.term else {
        return Err(InlineUnsupportedReason::NonReturnTerm);
    };

    let mut locals = HashMap::new();
    for (slot, _name) in callee_layout.stack_slots().iter().enumerate() {
        let location =
            LocalLocation(u32::try_from(slot).expect("callee stack slot index should fit in u32"));
        if value_bindings.contains_key(&location) {
            continue;
        }
        let fresh = allocate_inline_local(caller)?;
        locals.insert(location, fresh);
    }
    let (return_target, return_local, continuation_args) = match return_placement {
        InlineReturnPlacement::FreshContinuationArg => {
            let return_local = allocate_inline_local(caller)?;
            (
                return_local.resolved_name(),
                Some(return_local.clone()),
                vec![BlockArg::Name(return_local.name)],
            )
        }
        InlineReturnPlacement::StoreTo(target) => (target, None, Vec::new()),
    };

    let mut remapper = InlineLocalRemapper::new(&locals, value_bindings, &mut constant_scope);
    let mut body = callee_block
        .body
        .iter()
        .cloned()
        // Inlined profiling counters belong to the callee's counter layout;
        // the caller does not have storage for those ids.
        .filter(|instr| !matches!(instr, InstrCodegenOp::IncrementCounter(_)))
        .map(|instr| remapper.try_map_instr(instr))
        .collect::<Result<Vec<_>, _>>()?;
    let return_value = remapper.try_map_instr(return_value.clone())?;
    let return_meta = return_value.meta();
    body.push(
        Store::new(return_target, Box::new(return_value))
            .with_meta(return_meta)
            .into(),
    );

    let entry_label = caller.name_gen.next_block_name();
    let block = Block::new(
        entry_label,
        body,
        BlockTerm::Jump(BlockEdge::with_args(continuation, continuation_args)),
        Vec::new(),
        None,
    );

    Ok(InlineFragment {
        entry_label,
        blocks: vec![block],
        locals,
        return_local,
    })
}

enum InlineReturnPlacement {
    FreshContinuationArg,
    StoreTo(ResolvedName),
}

fn allocate_inline_local(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
) -> Result<InlineLocal, InlineUnsupportedReason> {
    let temp = try_allocate_codegen_stack_temp(caller, "inline")
        .map_err(|_| InlineUnsupportedReason::MissingCallerStorageLayout)?;
    Ok(InlineLocal {
        name: temp.name,
        location: temp.location,
    })
}

fn parameter_local_location(
    function: &BlockPyFunction<CodegenModuleShape>,
    name: &str,
) -> Result<LocalLocation, InlineUnsupportedReason> {
    let layout = function
        .storage_layout
        .as_ref()
        .ok_or(InlineUnsupportedReason::MissingCalleeStorageLayout)?;
    let Some(slot) = layout
        .stack_slots()
        .iter()
        .position(|slot_name| slot_name == name)
    else {
        return Err(InlineUnsupportedReason::MissingParameterLocal(
            name.to_string(),
        ));
    };
    Ok(LocalLocation(
        u32::try_from(slot).expect("parameter stack slot index should fit in u32"),
    ))
}

impl InlineLocal {
    fn resolved_name(&self) -> ResolvedName {
        ResolvedName {
            id: self.name.clone().into(),
            location: NameLocation::Local(self.location),
        }
    }
}

struct InlineLocalRemapper<'locals, 'bindings, 'scope, 'constants> {
    locals: &'locals HashMap<LocalLocation, InlineLocal>,
    value_bindings: &'bindings InlineValueBindings,
    constant_scope: &'scope mut InlineConstantScope<'constants>,
}

impl<'locals, 'bindings, 'scope, 'constants>
    InlineLocalRemapper<'locals, 'bindings, 'scope, 'constants>
{
    fn new(
        locals: &'locals HashMap<LocalLocation, InlineLocal>,
        value_bindings: &'bindings InlineValueBindings,
        constant_scope: &'scope mut InlineConstantScope<'constants>,
    ) -> Self {
        Self {
            locals,
            value_bindings,
            constant_scope,
        }
    }
}

enum InlineConstantScope<'a> {
    SameModule,
    CrossModule(InlineConstantRemapper<'a>),
}

impl InlineConstantScope<'_> {
    fn is_cross_module(&self) -> bool {
        matches!(self, Self::CrossModule(_))
    }

    fn remap_location(
        &mut self,
        location: NameLocation,
    ) -> Result<NameLocation, InlineUnsupportedReason> {
        match (self, location) {
            (Self::SameModule, location) => Ok(location),
            (Self::CrossModule(remapper), NameLocation::Constant(index)) => {
                Ok(NameLocation::Constant(remapper.remap(index)?))
            }
            (Self::CrossModule(_), location) => Ok(location),
        }
    }
}

struct InlineConstantRemapper<'a> {
    caller_constants: &'a mut Vec<InstrResolved>,
    callee_constants: &'a [InstrResolved],
    mapped_indices: HashMap<u32, u32>,
}

impl<'a> InlineConstantRemapper<'a> {
    fn new(
        caller_constants: &'a mut Vec<InstrResolved>,
        callee_constants: &'a [InstrResolved],
    ) -> Self {
        Self {
            caller_constants,
            callee_constants,
            mapped_indices: HashMap::new(),
        }
    }

    fn remap(&mut self, callee_index: u32) -> Result<u32, InlineUnsupportedReason> {
        if let Some(caller_index) = self.mapped_indices.get(&callee_index).copied() {
            return Ok(caller_index);
        }
        let constant = self
            .callee_constants
            .get(callee_index as usize)
            .ok_or(InlineUnsupportedReason::MissingCalleeConstant(callee_index))?
            .clone();
        let caller_index = u32::try_from(self.caller_constants.len())
            .map_err(|_| InlineUnsupportedReason::TooManyCallerConstants)?;
        self.caller_constants.push(constant);
        self.mapped_indices.insert(callee_index, caller_index);
        Ok(caller_index)
    }
}

impl TryMapInstr<InstrCodegen, InstrCodegen, InlineUnsupportedReason>
    for InlineLocalRemapper<'_, '_, '_, '_>
{
    fn try_map_instr(
        &mut self,
        instr: InstrCodegen,
    ) -> Result<InstrCodegen, InlineUnsupportedReason> {
        let mapped = match instr {
            InstrCodegenOp::BinOp(op) => InstrCodegenOp::BinOp(op.try_map_children(self)?),
            InstrCodegenOp::UnaryOp(op) => InstrCodegenOp::UnaryOp(op.try_map_children(self)?),
            InstrCodegenOp::CalleeFunctionId(op) => {
                InstrCodegenOp::CalleeFunctionId(op.try_map_children(self)?)
            }
            InstrCodegenOp::DirectFunctionIdGuardTest(op) => {
                InstrCodegenOp::DirectFunctionIdGuardTest(op.try_map_children(self)?)
            }
            InstrCodegenOp::Tuple(op) => InstrCodegenOp::Tuple(op.try_map_children(self)?),
            InstrCodegenOp::Call(op) => InstrCodegenOp::Call(op.try_map_children(self)?),
            InstrCodegenOp::CallDirect(op) => {
                InstrCodegenOp::CallDirect(op.try_map_children(self)?)
            }
            InstrCodegenOp::GetAttr(op) => InstrCodegenOp::GetAttr(op.try_map_children(self)?),
            InstrCodegenOp::SetAttr(op) => InstrCodegenOp::SetAttr(op.try_map_children(self)?),
            InstrCodegenOp::GetItem(op) => InstrCodegenOp::GetItem(op.try_map_children(self)?),
            InstrCodegenOp::SetItem(op) => InstrCodegenOp::SetItem(op.try_map_children(self)?),
            InstrCodegenOp::DelItem(op) => InstrCodegenOp::DelItem(op.try_map_children(self)?),
            InstrCodegenOp::Load(op) => {
                if let Some(location) = op.name.local_location() {
                    if let Some(value) = self.value_bindings.get(&location) {
                        return Ok(clear_codegen_instr_ids(value.clone()));
                    }
                }
                InstrCodegenOp::Load(op.try_map_children(self)?)
            }
            InstrCodegenOp::Store(op) => {
                if let Some(location) = op.name.local_location() {
                    if self.value_bindings.contains_key(&location) {
                        return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
                    }
                }
                InstrCodegenOp::Store(op.try_map_children(self)?)
            }
            InstrCodegenOp::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    if self.value_bindings.contains_key(&location) {
                        return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
                    }
                }
                InstrCodegenOp::Del(op.try_map_children(self)?)
            }
            InstrCodegenOp::MakeCell(op) => InstrCodegenOp::MakeCell(op.try_map_children(self)?),
            InstrCodegenOp::IncrementCounter(op) => InstrCodegenOp::IncrementCounter(op),
            InstrCodegenOp::CellRef(op) => InstrCodegenOp::CellRef(op),
            InstrCodegenOp::MakeFunctionWithClosure(op) => {
                InstrCodegenOp::MakeFunctionWithClosure(op.try_map_children(self)?)
            }
        };
        Ok(clear_codegen_instr_id(mapped))
    }

    fn try_map_name(
        &mut self,
        mut name: ResolvedName,
    ) -> Result<ResolvedName, InlineUnsupportedReason> {
        name.location = self.constant_scope.remap_location(name.location)?;
        if self.constant_scope.is_cross_module()
            && (name.location.is_global() || name.location.is_global_name())
        {
            let Some(runtime_name) = RuntimeName::from_name(name.id.as_str()) else {
                return Err(InlineUnsupportedReason::CrossModuleGlobalName(
                    name.id.to_string(),
                ));
            };
            name.location = NameLocation::RuntimeName(runtime_name);
        }
        let Some(location) = name.location.as_local() else {
            return Ok(name);
        };
        if self.value_bindings.contains_key(&location) {
            return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
        }
        let Some(fresh) = self.locals.get(&location) else {
            return Err(InlineUnsupportedReason::MissingCalleeLocal(location));
        };
        name.id = fresh.name.clone().into();
        name.location = NameLocation::Local(fresh.location);
        Ok(name)
    }
}

fn clear_codegen_instr_ids(instr: InstrCodegen) -> InstrCodegen {
    InstrIdScrubber.map_instr(instr)
}

fn clear_codegen_instr_id(instr: InstrCodegen) -> InstrCodegen {
    let mut meta = instr.meta();
    meta.instr_id = None;
    instr.with_meta(meta)
}

struct InstrIdScrubber;

impl MapInstr<InstrCodegen, InstrCodegen> for InstrIdScrubber {
    fn map_instr(&mut self, instr: InstrCodegen) -> InstrCodegen {
        let mapped = match instr {
            InstrCodegenOp::BinOp(op) => InstrCodegenOp::BinOp(op.map_children(self)),
            InstrCodegenOp::UnaryOp(op) => InstrCodegenOp::UnaryOp(op.map_children(self)),
            InstrCodegenOp::CalleeFunctionId(op) => {
                InstrCodegenOp::CalleeFunctionId(op.map_children(self))
            }
            InstrCodegenOp::DirectFunctionIdGuardTest(op) => {
                InstrCodegenOp::DirectFunctionIdGuardTest(op.map_children(self))
            }
            InstrCodegenOp::Tuple(op) => InstrCodegenOp::Tuple(op.map_children(self)),
            InstrCodegenOp::Call(op) => InstrCodegenOp::Call(op.map_children(self)),
            InstrCodegenOp::CallDirect(op) => InstrCodegenOp::CallDirect(op.map_children(self)),
            InstrCodegenOp::GetAttr(op) => InstrCodegenOp::GetAttr(op.map_children(self)),
            InstrCodegenOp::SetAttr(op) => InstrCodegenOp::SetAttr(op.map_children(self)),
            InstrCodegenOp::GetItem(op) => InstrCodegenOp::GetItem(op.map_children(self)),
            InstrCodegenOp::SetItem(op) => InstrCodegenOp::SetItem(op.map_children(self)),
            InstrCodegenOp::DelItem(op) => InstrCodegenOp::DelItem(op.map_children(self)),
            InstrCodegenOp::Load(op) => InstrCodegenOp::Load(op.map_children(self)),
            InstrCodegenOp::Store(op) => InstrCodegenOp::Store(op.map_children(self)),
            InstrCodegenOp::Del(op) => InstrCodegenOp::Del(op.map_children(self)),
            InstrCodegenOp::MakeCell(op) => InstrCodegenOp::MakeCell(op.map_children(self)),
            InstrCodegenOp::IncrementCounter(op) => InstrCodegenOp::IncrementCounter(op),
            InstrCodegenOp::CellRef(op) => InstrCodegenOp::CellRef(op),
            InstrCodegenOp::MakeFunctionWithClosure(op) => {
                InstrCodegenOp::MakeFunctionWithClosure(op.map_children(self))
            }
        };
        clear_codegen_instr_id(mapped)
    }

    fn map_name(&mut self, name: ResolvedName) -> ResolvedName {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::{
        plan_module_inlining, summarize_module_escapes, validate_codegen_instr_ids,
    };
    use soac_core::block_py::{
        BlockParam, BlockParamRole, Call, CallDirect, InstrId, Load, NameLike,
    };
    use soac_lowering::lower_python_to_blockpy_for_testing;

    fn function_by_qualname<'a>(
        module: &'a soac_core::block_py::BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> &'a BlockPyFunction<CodegenModuleShape> {
        module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("{qualname} should be present"))
    }

    fn function_index_by_qualname(
        module: &soac_core::block_py::BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> usize {
        module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("{qualname} should be present"))
    }

    fn local_location(function: &BlockPyFunction<CodegenModuleShape>, name: &str) -> LocalLocation {
        let slot = function
            .storage_layout
            .as_ref()
            .expect("function should have storage")
            .stack_slots()
            .iter()
            .position(|slot_name| slot_name == name)
            .unwrap_or_else(|| panic!("{name} should have a local slot"));
        LocalLocation(u32::try_from(slot).expect("slot index should fit in u32"))
    }

    fn local_resolved_name(
        function: &BlockPyFunction<CodegenModuleShape>,
        name: &str,
    ) -> ResolvedName {
        ResolvedName {
            id: name.to_string().into(),
            location: NameLocation::Local(local_location(function, name)),
        }
    }

    fn local_load(function: &BlockPyFunction<CodegenModuleShape>, name: &str) -> InstrCodegen {
        Load::new(local_resolved_name(function, name)).into()
    }

    fn bound_local_location(
        bindings: &InlineValueBindings,
        location: LocalLocation,
    ) -> LocalLocation {
        let Some(InstrCodegen::Load(load)) = bindings.get(&location) else {
            panic!("binding should be a local load");
        };
        load.name
            .local_location()
            .expect("binding should load a local")
    }

    fn global_load(name: &str) -> InstrCodegen {
        Load::new(ResolvedName {
            id: name.to_string().into(),
            location: NameLocation::GlobalName,
        })
        .into()
    }

    fn rewrite_first_store_call_to_direct(
        module: &mut soac_core::block_py::BlockPyModule<CodegenModuleShape>,
        qualname: &str,
        function_id: RuntimeFunctionId,
    ) {
        let function_index = function_index_by_qualname(module, qualname);
        let function = &mut module.callable_defs[function_index];
        let store = function
            .blocks
            .iter_mut()
            .flat_map(|block| block.body.iter_mut())
            .find_map(|instr| {
                let InstrCodegen::Store(store) = instr else {
                    return None;
                };
                matches!(store.value.as_ref(), InstrCodegen::Call(_)).then_some(store)
            })
            .unwrap_or_else(|| panic!("{qualname} should have a store-call site"));
        let InstrCodegen::Call(call) = store.value.as_ref() else {
            panic!("{qualname} store-call site should still be generic");
        };
        let meta = call.meta();
        *store.value = InstrCodegen::CallDirect(
            CallDirect::new(
                call.func.clone(),
                function_id,
                call.args.clone(),
                call.keywords.clone(),
            )
            .with_meta(meta),
        );
    }

    fn string_constant_value(constants: &[InstrResolved], index: u32) -> String {
        let Some(InstrResolved::Literal(value)) = constants.get(index as usize) else {
            panic!("constant {index} should be a literal");
        };
        let Literal::StringLiteral(value) = value.as_literal() else {
            panic!("constant {index} should be a string literal");
        };
        value.value.clone()
    }

    #[test]
    fn runtime_name_match_accepts_constant_slot_loads_with_runtime_id() {
        let stop_iteration = Load::new(ResolvedName {
            id: RuntimeName::StopIteration.name().to_string().into(),
            location: NameLocation::Constant(7),
        })
        .into();

        assert!(is_runtime_stop_iteration_expr(&stop_iteration, &[]));
    }

    #[test]
    fn runtime_name_match_looks_through_copied_module_constants() {
        let constants = vec![
            Load::new(ResolvedName {
                id: RuntimeName::StopIteration.name().to_string().into(),
                location: NameLocation::RuntimeName(RuntimeName::StopIteration),
            })
            .into(),
        ];
        let stop_iteration = Load::new(ResolvedName {
            id: "constant slot 0".to_string().into(),
            location: NameLocation::Constant(0),
        })
        .into();

        assert!(is_runtime_stop_iteration_expr(&stop_iteration, &constants));
    }

    #[test]
    fn rewrites_static_runtime_constructor_calls_to_direct_calls() {
        let constructor_id = RuntimeFunctionId::from_raw_parts(42, 7);
        let constants = vec![
            Load::new(ResolvedName {
                id: RuntimeName::IterRange.name().to_string().into(),
                location: NameLocation::RuntimeName(RuntimeName::IterRange),
            })
            .into(),
        ];
        let target = ResolvedName {
            id: "out".to_string().into(),
            location: NameLocation::Local(LocalLocation(0)),
        };
        let runtime_constructor = Load::new(ResolvedName {
            id: "constant 0".to_string().into(),
            location: NameLocation::Constant(0),
        });
        let value = Load::new(ResolvedName {
            id: "value".to_string().into(),
            location: NameLocation::Local(LocalLocation(1)),
        });
        let mut blocks = vec![Block::new(
            BlockLabel::from_index(0),
            vec![InstrCodegen::Store(Store::new(
                target,
                InstrCodegen::Call(Call::new(
                    InstrCodegen::Load(runtime_constructor),
                    vec![CallArgPositional::Positional(InstrCodegen::Load(value))],
                    Vec::new(),
                )),
            ))],
            BlockTerm::Return(InstrCodegen::Load(Load::new(ResolvedName {
                id: "value".to_string().into(),
                location: NameLocation::Local(LocalLocation(1)),
            }))),
            Vec::new(),
            None,
        )];

        let rewritten = rewrite_static_runtime_constructor_call_stores(
            &mut blocks,
            &constants,
            |runtime_name| (runtime_name == RuntimeName::IterRange).then_some(constructor_id),
        );

        assert_eq!(rewritten, 1);
        let InstrCodegen::Store(store) = &blocks[0].body[0] else {
            panic!("body instruction should remain a store");
        };
        let InstrCodegen::CallDirect(call) = store.value.as_ref() else {
            panic!("runtime constructor call should be direct");
        };
        assert_eq!(call.function_id, constructor_id);
    }

    #[test]
    fn binds_simple_positional_call_args_to_callee_param_locals() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
def callee(a, b):
    return a + b

def caller(x, y):
    return x
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let callee = function_by_qualname(&module, "callee");
        let caller = function_by_qualname(&module, "caller");
        let call = CallDirect::new(
            local_load(caller, "x"),
            callee.function_id,
            vec![
                CallArgPositional::Positional(local_load(caller, "x")),
                CallArgPositional::Positional(local_load(caller, "y")),
            ],
            Vec::new(),
        );

        let bindings = bind_simple_direct_call_inline_args(callee, &call).unwrap();

        assert_eq!(bindings.len(), 2);
        assert!(bindings.contains_key(&local_location(callee, "a")));
        assert!(bindings.contains_key(&local_location(callee, "b")));
    }

    #[test]
    fn binds_direct_method_receiver_as_first_callee_param() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
class Thing:
    def f(self, x):
        return x

def caller(obj, value):
    return obj
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let callee = function_by_qualname(&module, "Thing.f");
        let caller = function_by_qualname(&module, "caller");

        let bindings = bind_simple_direct_method_inline_args(
            callee,
            local_load(caller, "obj"),
            &[CallArgPositional::Positional(local_load(caller, "value"))],
        )
        .unwrap();

        assert_eq!(bindings.len(), 2);
        assert_eq!(
            bound_local_location(&bindings, local_location(callee, "self")),
            local_location(caller, "obj")
        );
        assert_eq!(
            bound_local_location(&bindings, local_location(callee, "x")),
            local_location(caller, "value")
        );
    }

    #[test]
    fn builds_direct_method_inline_fragment_to_target() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
class Thing:
    def f(self, x):
        return x

def caller(obj, value):
    out = None
    return out
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let callee = function_by_qualname(&module, "Thing.f");
        let mut caller = function_by_qualname(&module, "caller").clone();
        let continuation = caller.name_gen.next_block_name();
        let receiver = local_load(&caller, "obj");
        let arg = local_load(&caller, "value");
        let target = local_resolved_name(&caller, "out");

        let fragment = build_direct_method_inline_fragment_to_target(
            &mut caller,
            callee,
            continuation,
            receiver,
            &[CallArgPositional::Positional(arg)],
            target,
        )
        .expect("simple method should inline");

        assert_eq!(
            fragment.locals.len(),
            0,
            "bound receiver and arg should avoid fresh callee-param locals"
        );
        assert!(fragment.return_local.is_none());
        assert_eq!(fragment.blocks.len(), 1);
        let BlockTerm::Jump(edge) = &fragment.blocks[0].term else {
            panic!("method inline fragment should jump to continuation");
        };
        assert_eq!(edge.target, continuation);
        assert!(edge.args.is_empty());
    }

    #[test]
    fn cross_module_inline_fragment_remaps_callee_constants_into_caller_module() {
        let callee_module = lower_python_to_blockpy_for_testing(
            r#"
class IterRange:
    def __next__(self):
        return self.current
"#,
        )
        .expect("callee transform should succeed")
        .codegen_module;
        let callee = function_by_qualname(&callee_module, "IterRange.__next__");
        let mut caller_module = lower_python_to_blockpy_for_testing(
            r#"
def caller(obj):
    out = None
    return out
"#,
        )
        .expect("caller transform should succeed")
        .codegen_module;
        let caller_index = function_index_by_qualname(&caller_module, "caller");
        let mut caller = caller_module.callable_defs.remove(caller_index);
        let mut caller_constants = std::mem::take(&mut caller_module.module_constants);
        let original_constant_count = caller_constants.len();
        let continuation = caller.name_gen.next_block_name();
        let receiver = local_load(&caller, "obj");
        let target = local_resolved_name(&caller, "out");

        let fragment = build_cross_module_direct_method_inline_fragment_to_target(
            &mut caller,
            &mut caller_constants,
            callee,
            callee_module.module_constants.as_slice(),
            continuation,
            receiver,
            &[],
            target,
        )
        .expect("cross-module inline fragment should build");

        let attr_constant_index = fragment
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .find_map(|instr| {
                let InstrCodegenOp::Store(store) = instr else {
                    return None;
                };
                let InstrCodegenOp::GetAttr(getattr) = store.value.as_ref() else {
                    return None;
                };
                let InstrCodegenOp::Load(load) = getattr.attr.as_ref() else {
                    return None;
                };
                load.name.location.as_constant()
            })
            .expect("inlined self.current should load an attribute-name constant");
        assert!(attr_constant_index as usize >= original_constant_count);
        assert_eq!(
            string_constant_value(&caller_constants, attr_constant_index),
            "current"
        );
    }

    #[test]
    fn rejects_simple_positional_binding_when_arity_differs() {
        let module = lower_python_to_blockpy_for_testing("def callee(a, b):\n    return a\n")
            .expect("transform should succeed")
            .codegen_module;
        let callee = function_by_qualname(&module, "callee");
        let call = CallDirect::new(
            local_load(callee, "a"),
            callee.function_id,
            vec![CallArgPositional::Positional(local_load(callee, "a"))],
            Vec::new(),
        );

        let err = bind_simple_direct_call_inline_args(callee, &call).unwrap_err();

        assert_eq!(
            err,
            InlineUnsupportedReason::ArityMismatch {
                expected: 2,
                actual: 1
            }
        );
    }

    #[test]
    fn clones_single_block_callee_into_fresh_caller_locals() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
def callee(a, b):
    c = a + b
    return c

def caller(x, y):
    return x
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let callee = function_by_qualname(&module, "callee");
        let mut caller = function_by_qualname(&module, "caller").clone();
        let original_slot_count = caller
            .storage_layout
            .as_ref()
            .expect("caller should have storage")
            .stack_slots()
            .len();
        let continuation = BlockLabel::from_index(10_000);

        let fragment =
            build_single_block_inline_fragment(&mut caller, callee, continuation).unwrap();

        assert_eq!(fragment.blocks.len(), 1);
        assert_ne!(fragment.entry_label, callee.blocks[0].label);
        assert_eq!(fragment.blocks[0].label, fragment.entry_label);
        assert!(
            fragment.blocks[0]
                .body
                .iter()
                .all(|instr| instr.meta().instr_id.is_none())
        );
        assert_eq!(
            caller
                .storage_layout
                .as_ref()
                .expect("caller should have storage")
                .stack_slots()
                .len(),
            original_slot_count + callee.storage_layout.as_ref().unwrap().stack_slots().len() + 1
        );

        let BlockTerm::Jump(edge) = &fragment.blocks[0].term else {
            panic!("inlined block should jump to continuation");
        };
        assert_eq!(edge.target, continuation);
        assert_eq!(edge.args.len(), 1);
        let return_local = fragment
            .return_local
            .as_ref()
            .expect("default fragment should create a synthetic return local");
        let BlockArg::Name(return_arg) = &edge.args[0] else {
            panic!("continuation argument should name the synthetic return local");
        };
        assert_eq!(return_arg, &return_local.name);

        let Some(InstrCodegen::Store(return_store)) = fragment.blocks[0].body.last() else {
            panic!("inlined block should store the return value before jumping");
        };
        assert_eq!(
            return_store.name.local_location(),
            Some(return_local.location)
        );

        for (callee_location, fresh) in &fragment.locals {
            assert_ne!(callee_location, &fresh.location);
            assert!(
                caller
                    .storage_layout
                    .as_ref()
                    .unwrap()
                    .stack_slots()
                    .contains(&fresh.name)
            );
        }
    }

    #[test]
    fn rejects_callee_with_block_params() {
        let module = lower_python_to_blockpy_for_testing("def callee(a):\n    return a\n")
            .expect("transform should succeed")
            .codegen_module;
        let mut callee = function_by_qualname(&module, "callee").clone();
        callee.blocks[0].params.push(BlockParam {
            name: "incoming".to_string(),
            role: BlockParamRole::AbruptPayload,
        });
        let mut caller = callee.clone();

        let err =
            build_single_block_inline_fragment(&mut caller, &callee, BlockLabel::from_index(99))
                .unwrap_err();

        assert_eq!(err, InlineUnsupportedReason::BlockParams);
    }

    #[test]
    fn can_store_return_into_explicit_target_without_continuation_arg() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
def callee(a):
    return a

def caller(x):
    out = None
    return out
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let callee = function_by_qualname(&module, "callee");
        let mut caller = function_by_qualname(&module, "caller").clone();
        let callee_a = local_location(callee, "a");
        let mut bound_x = local_load(&caller, "x");
        let mut bound_x_meta = bound_x.meta();
        bound_x_meta.instr_id = Some(InstrId::new(BlockLabel::from_index(20_000), 7));
        bound_x = bound_x.with_meta(bound_x_meta);
        let mut bindings = InlineValueBindings::new();
        bindings.insert(callee_a, bound_x);
        let return_target = local_resolved_name(&caller, "out");

        let fragment = build_single_block_inline_fragment_to_target(
            &mut caller,
            callee,
            BlockLabel::from_index(10_003),
            &bindings,
            return_target,
        )
        .unwrap();

        assert!(fragment.return_local.is_none());
        let BlockTerm::Jump(edge) = &fragment.blocks[0].term else {
            panic!("inlined block should jump to continuation");
        };
        assert!(edge.args.is_empty());
        let Some(InstrCodegen::Store(return_store)) = fragment.blocks[0].body.last() else {
            panic!("inlined block should store the return value before jumping");
        };
        assert!(return_store.value.meta().instr_id.is_none());
        assert_eq!(
            return_store.name.local_location(),
            Some(local_location(&caller, "out"))
        );
    }

    #[test]
    fn substitutes_bound_callee_locals_with_caller_values() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
def callee(a, b):
    return a + b

def caller(x, y):
    return x
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let callee = function_by_qualname(&module, "callee");
        let mut caller = function_by_qualname(&module, "caller").clone();
        let original_slot_count = caller
            .storage_layout
            .as_ref()
            .expect("caller should have storage")
            .stack_slots()
            .len();
        let callee_a = local_location(callee, "a");
        let callee_b = local_location(callee, "b");
        let mut bindings = InlineValueBindings::new();
        bindings.insert(callee_a, local_load(&caller, "x"));
        bindings.insert(callee_b, local_load(&caller, "y"));

        let fragment = build_single_block_inline_fragment_with_bindings(
            &mut caller,
            callee,
            BlockLabel::from_index(10_001),
            &bindings,
        )
        .unwrap();

        assert!(!fragment.locals.contains_key(&callee_a));
        assert!(!fragment.locals.contains_key(&callee_b));
        assert_eq!(
            caller
                .storage_layout
                .as_ref()
                .expect("caller should have storage")
                .stack_slots()
                .len(),
            original_slot_count + callee.storage_layout.as_ref().unwrap().stack_slots().len()
                - bindings.len()
                + 1
        );
    }

    #[test]
    fn rejects_store_to_bound_callee_local() {
        let module = lower_python_to_blockpy_for_testing(
            r#"
def callee(a):
    a = 1
    return a

def caller(x):
    return x
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let callee = function_by_qualname(&module, "callee");
        let mut caller = function_by_qualname(&module, "caller").clone();
        let callee_a = local_location(callee, "a");
        let mut bindings = InlineValueBindings::new();
        bindings.insert(callee_a, local_load(&caller, "x"));

        let err = build_single_block_inline_fragment_with_bindings(
            &mut caller,
            callee,
            BlockLabel::from_index(10_002),
            &bindings,
        )
        .unwrap_err();

        assert_eq!(err, InlineUnsupportedReason::RebindsBoundLocal(callee_a));
    }

    #[test]
    fn rewrites_store_of_planned_direct_call_into_inline_blocks() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def make(obj, x):
    out = None
    return out
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let inline_plan = plan_module_inlining(&summarize_module_escapes(&module));
        let constructor_id =
            module.callable_defs[function_index_by_qualname(&module, "Box.__init__")].function_id;
        let make_index = function_index_by_qualname(&module, "make");
        let original_block_label = module.callable_defs[make_index].blocks[0].label;
        let out_target = local_resolved_name(&module.callable_defs[make_index], "out");
        let obj_arg = local_load(&module.callable_defs[make_index], "obj");
        let x_arg = local_load(&module.callable_defs[make_index], "x");
        module.callable_defs[make_index].blocks[0]
            .body
            .push(InstrCodegen::Store(Store::new(
                out_target,
                InstrCodegen::CallDirect(CallDirect::new(
                    global_load("Box.__init__"),
                    constructor_id,
                    vec![
                        CallArgPositional::Positional(obj_arg),
                        CallArgPositional::Positional(x_arg),
                    ],
                    Vec::new(),
                )),
            )));

        let stats = inline_simple_direct_call_stores(&mut module, &inline_plan);

        assert_eq!(
            stats,
            InlineRewriteStats {
                rewritten_stores: 1,
                skipped_candidates: 0,
            }
        );
        validate_codegen_instr_ids(&module).expect("rewritten module should have valid instr ids");
        let caller = function_by_qualname(&module, "make");
        assert_eq!(caller.blocks.len(), 3);
        let prelude = caller
            .blocks
            .iter()
            .find(|block| block.label == original_block_label)
            .expect("original block should become rewrite prelude");
        let BlockTerm::Jump(edge) = &prelude.term else {
            panic!("prelude should jump to inline fragment");
        };
        let fragment = caller
            .blocks
            .iter()
            .find(|block| block.label == edge.target)
            .expect("inline fragment should be inserted");
        assert!(
            fragment
                .body
                .iter()
                .any(|instr| matches!(instr, InstrCodegen::SetAttr(_)))
        );
        let BlockTerm::Jump(edge) = &fragment.term else {
            panic!("inline fragment should jump to continuation");
        };
        assert!(edge.args.is_empty());
        let continuation = caller
            .blocks
            .iter()
            .find(|block| block.label == edge.target)
            .expect("continuation should be inserted");
        assert!(continuation.body.is_empty());
        assert!(matches!(continuation.term, BlockTerm::Return(_)));
    }

    #[test]
    fn leaves_constructor_allocation_direct_calls_for_scalar_replacement() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def make(x):
    out = None
    return out
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let inline_plan = plan_module_inlining(&summarize_module_escapes(&module));
        let constructor_id =
            module.callable_defs[function_index_by_qualname(&module, "Box.__init__")].function_id;
        let make_index = function_index_by_qualname(&module, "make");
        let out_target = local_resolved_name(&module.callable_defs[make_index], "out");
        let x_arg = local_load(&module.callable_defs[make_index], "x");
        module.callable_defs[make_index].blocks[0]
            .body
            .push(InstrCodegen::Store(Store::new(
                out_target,
                InstrCodegen::CallDirect(CallDirect::new(
                    global_load("Box"),
                    constructor_id,
                    vec![CallArgPositional::Positional(x_arg)],
                    Vec::new(),
                )),
            )));

        let stats = inline_simple_direct_call_stores(&mut module, &inline_plan);

        assert_eq!(
            stats,
            InlineRewriteStats {
                rewritten_stores: 0,
                skipped_candidates: 0,
            }
        );
        let make = function_by_qualname(&module, "make");
        assert!(
            make.blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(instr, InstrCodegen::Store(store) if matches!(store.value.as_ref(), InstrCodegen::CallDirect(call) if call.function_id == constructor_id))),
            "constructor allocation should remain direct for scalar replacement: {make:#?}"
        );
    }

    #[test]
    fn scalar_replaces_same_block_non_escaping_constructor_allocation() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def make(x):
    out = Box(x)
    out.value = x
    return out.value
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let constructor_id =
            module.callable_defs[function_index_by_qualname(&module, "Box.__init__")].function_id;
        let make_index = function_index_by_qualname(&module, "make");
        let allocation_store = module.callable_defs[make_index].blocks[0]
            .body
            .iter_mut()
            .find_map(|instr| match instr {
                InstrCodegenOp::Store(store) if store.name.id_str() == "out" => Some(store),
                _ => None,
            })
            .expect("make should store the constructor result into out");
        let InstrCodegenOp::Call(call) = allocation_store.value.as_ref() else {
            panic!("constructor store should start as a generic call");
        };
        let args = call.args.clone();
        let keywords = call.keywords.clone();
        *allocation_store.value = InstrCodegen::CallDirect(CallDirect::new(
            (*call.func).clone(),
            constructor_id,
            args,
            keywords,
        ));

        let escape_summary = summarize_module_escapes(&module);
        let inline_plan = plan_module_inlining(&escape_summary);
        let stats = scalar_replace_non_escaping_constructor_allocations(&mut module, &inline_plan);

        assert_eq!(
            stats,
            ScalarReplacementStats {
                candidate_allocations: 1,
                planned_allocations: 1,
                replaced_allocations: 1,
                skipped_allocations: 0,
                skipped_unbuildable_allocations: 0,
                skipped_live_alias_control_flow_allocations: 0,
            }
        );
        validate_codegen_instr_ids(&module).expect("rewritten module should have valid instr ids");
        let make = function_by_qualname(&module, "make");
        assert!(
            !make.blocks[0]
                .body
                .iter()
                .any(|instr| matches!(instr, InstrCodegen::CallDirect(_)))
        );
        assert!(
            !make.blocks[0].body.iter().any(|instr| {
                matches!(instr, InstrCodegen::GetAttr(_) | InstrCodegen::SetAttr(_))
            })
        );
        assert!(make.blocks[0].body.iter().any(|instr| {
            matches!(
                instr,
                InstrCodegen::Store(store)
                    if store.name.id_str().starts_with("_dp_scalar_field")
            )
        }));
        assert!(matches!(
            &make.blocks[0].term,
            BlockTerm::Return(InstrCodegen::Load(load))
                if load.name.id_str().starts_with("_dp_scalar_field")
        ));
    }

    #[test]
    fn scalar_replaces_straightline_successor_constructor_uses() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def make(x):
    out = Box(x)
    out.value = x
    return out.value
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let constructor_id =
            module.callable_defs[function_index_by_qualname(&module, "Box.__init__")].function_id;
        let make_index = function_index_by_qualname(&module, "make");
        let allocation_index = {
            let block = &mut module.callable_defs[make_index].blocks[0];
            let allocation_index = block
                .body
                .iter()
                .position(|instr| matches!(instr, InstrCodegenOp::Store(store) if store.name.id_str() == "out"))
                .expect("make should store the constructor result into out");
            let InstrCodegenOp::Store(allocation_store) = &mut block.body[allocation_index] else {
                unreachable!("allocation index should point at a store");
            };
            let InstrCodegenOp::Call(call) = allocation_store.value.as_ref() else {
                panic!("constructor store should start as a generic call");
            };
            let args = call.args.clone();
            let keywords = call.keywords.clone();
            *allocation_store.value = InstrCodegen::CallDirect(CallDirect::new(
                (*call.func).clone(),
                constructor_id,
                args,
                keywords,
            ));
            allocation_index
        };
        let continuation_label = module.callable_defs[make_index].name_gen.next_block_name();
        let continuation = {
            let block = &mut module.callable_defs[make_index].blocks[0];
            let continuation_body = block.body.split_off(allocation_index + 1);
            let continuation_term = std::mem::replace(
                &mut block.term,
                BlockTerm::Jump(BlockEdge::with_args(
                    continuation_label,
                    vec![BlockArg::Name("out".to_string())],
                )),
            );
            Block::new(
                continuation_label,
                continuation_body,
                continuation_term,
                vec![BlockParam {
                    name: "out".to_string(),
                    role: BlockParamRole::AbruptPayload,
                }],
                block.exc_edge.clone(),
            )
        };
        module.callable_defs[make_index]
            .blocks
            .insert(1, continuation);

        let escape_summary = summarize_module_escapes(&module);
        let inline_plan = plan_module_inlining(&escape_summary);
        let stats = scalar_replace_non_escaping_constructor_allocations(&mut module, &inline_plan);

        assert_eq!(
            stats,
            ScalarReplacementStats {
                candidate_allocations: 1,
                planned_allocations: 1,
                replaced_allocations: 1,
                skipped_allocations: 0,
                skipped_unbuildable_allocations: 0,
                skipped_live_alias_control_flow_allocations: 0,
            }
        );
        validate_codegen_instr_ids(&module).expect("rewritten module should have valid instr ids");
        let make = function_by_qualname(&module, "make");
        assert!(
            !make
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(instr, InstrCodegen::CallDirect(_)))
        );
        assert!(
            !make
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(instr, InstrCodegen::GetAttr(_) | InstrCodegen::SetAttr(_)))
        );
        assert!(matches!(
            &make.blocks[1].term,
            BlockTerm::Return(InstrCodegen::Load(load))
                if load.name.id_str().starts_with("_dp_scalar_field")
        ));
        assert!(
            make.blocks[1].params.is_empty(),
            "scalarized object block param should be removed"
        );
    }

    #[test]
    fn scalar_replaces_live_object_across_closed_branch() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def make(x, cond):
    out = Box(x)
    if cond:
        return out.value
    return x
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let constructor_id =
            module.callable_defs[function_index_by_qualname(&module, "Box.__init__")].function_id;
        let make_index = function_index_by_qualname(&module, "make");
        let allocation_store = module.callable_defs[make_index]
            .blocks
            .iter_mut()
            .flat_map(|block| block.body.iter_mut())
            .find_map(|instr| match instr {
                InstrCodegenOp::Store(store) if store.name.id_str() == "out" => Some(store),
                _ => None,
            })
            .expect("make should store the constructor result into out");
        let InstrCodegenOp::Call(call) = allocation_store.value.as_ref() else {
            panic!("constructor store should start as a generic call");
        };
        let args = call.args.clone();
        let keywords = call.keywords.clone();
        *allocation_store.value = InstrCodegen::CallDirect(CallDirect::new(
            (*call.func).clone(),
            constructor_id,
            args,
            keywords,
        ));

        let escape_summary = summarize_module_escapes(&module);
        let inline_plan = plan_module_inlining(&escape_summary);
        let stats = scalar_replace_non_escaping_constructor_allocations(&mut module, &inline_plan);

        assert_eq!(
            stats,
            ScalarReplacementStats {
                candidate_allocations: 1,
                planned_allocations: 1,
                replaced_allocations: 1,
                skipped_allocations: 0,
                skipped_unbuildable_allocations: 0,
                skipped_live_alias_control_flow_allocations: 0,
            }
        );
        validate_codegen_instr_ids(&module).expect("rewritten module should have valid instr ids");
        let make = function_by_qualname(&module, "make");
        assert!(
            !make
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| instr_any(instr, |child| matches!(
                    child,
                    InstrCodegen::CallDirect(_)
                )))
        );
        assert!(
            !make
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(instr, InstrCodegen::GetAttr(_) | InstrCodegen::SetAttr(_)))
        );
    }

    #[test]
    fn rewrites_non_constructor_direct_call_store() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
def callee(x):
    return x

def make(x):
    out = None
    return out
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let inline_plan = plan_module_inlining(&summarize_module_escapes(&module));
        let callee_id =
            module.callable_defs[function_index_by_qualname(&module, "callee")].function_id;
        let make_index = function_index_by_qualname(&module, "make");
        let out_target = local_resolved_name(&module.callable_defs[make_index], "out");
        let x_arg = local_load(&module.callable_defs[make_index], "x");
        module.callable_defs[make_index].blocks[0]
            .body
            .push(InstrCodegen::Store(Store::new(
                out_target,
                InstrCodegen::CallDirect(CallDirect::new(
                    global_load("callee"),
                    callee_id,
                    vec![CallArgPositional::Positional(x_arg)],
                    Vec::new(),
                )),
            )));

        let stats = inline_simple_direct_call_stores(&mut module, &inline_plan);

        assert_eq!(stats.rewritten_stores, 1);
        assert_eq!(
            module.callable_defs[make_index].blocks.len(),
            3,
            "single-block direct call should become prelude, inline block, continuation"
        );
    }

    #[test]
    fn rewrites_multi_block_direct_call_store_and_forwards_exception_edge() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
def callee(x):
    if x:
        raise StopIteration
    return x

def make(x):
    out = None
    return out
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let inline_plan = plan_module_inlining(&summarize_module_escapes(&module));
        let callee_id =
            module.callable_defs[function_index_by_qualname(&module, "callee")].function_id;
        let make_index = function_index_by_qualname(&module, "make");
        let out_target = local_resolved_name(&module.callable_defs[make_index], "out");
        let x_arg = local_load(&module.callable_defs[make_index], "x");
        let inherited_exc_target = BlockLabel::from_index(50_000);
        module.callable_defs[make_index].blocks[0].exc_edge =
            Some(BlockEdge::new(inherited_exc_target));
        module.callable_defs[make_index].blocks[0]
            .body
            .push(InstrCodegen::Store(Store::new(
                out_target,
                InstrCodegen::CallDirect(CallDirect::new(
                    global_load("callee"),
                    callee_id,
                    vec![CallArgPositional::Positional(x_arg)],
                    Vec::new(),
                )),
            )));

        let stats = inline_simple_direct_call_stores(&mut module, &inline_plan);

        assert_eq!(stats.rewritten_stores, 1);
        let make = function_by_qualname(&module, "make");
        assert!(
            make.blocks
                .iter()
                .any(|block| matches!(block.term, BlockTerm::Raise(_))
                    && block
                        .exc_edge
                        .as_ref()
                        .is_some_and(|edge| edge.target == inherited_exc_target)),
            "inlined explicit raise blocks should inherit the caller exception edge: {make:#?}"
        );
        assert!(
            make.blocks.iter().flat_map(|block| block.body.iter()).any(
                |instr| matches!(instr, InstrCodegen::Store(store) if store.name.id_str() == "out")
            ),
            "inlined return blocks should store into the original target: {make:#?}"
        );
        assert!(
            !make
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(instr, InstrCodegen::CallDirect(_))),
            "direct call should be removed after inlining: {make:#?}"
        );
    }

    #[test]
    fn fixed_point_scalar_replaces_allocation_exposed_by_nested_inlining() {
        let mut module = lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def build_box(x):
    out = Box(x)
    return out

def make_box(x):
    out = build_box(x)
    return out

def caller(x):
    y = make_box(x)
    return y.value
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let constructor_id =
            module.callable_defs[function_index_by_qualname(&module, "Box.__init__")].function_id;
        let build_box_id =
            module.callable_defs[function_index_by_qualname(&module, "build_box")].function_id;
        let make_box_id =
            module.callable_defs[function_index_by_qualname(&module, "make_box")].function_id;
        rewrite_first_store_call_to_direct(&mut module, "build_box", constructor_id);
        rewrite_first_store_call_to_direct(&mut module, "make_box", build_box_id);
        rewrite_first_store_call_to_direct(&mut module, "caller", make_box_id);

        let stats = inline_and_scalar_replace_until_fixed_point(&mut module);

        assert!(
            stats.iterations >= 2,
            "nested direct calls should require a follow-up fixed-point iteration: {stats:#?}"
        );
        assert!(
            stats.inline_rewrite.rewritten_stores >= 2,
            "fixed-point inline pass should inline the factory chain: {stats:#?}"
        );
        assert_eq!(
            stats.scalar_replacement.replaced_allocations, 1,
            "constructor allocation exposed in caller should be scalarized: {stats:#?}"
        );
        validate_codegen_instr_ids(&module).expect("rewritten module should keep semantic ids");
        let caller = function_by_qualname(&module, "caller");
        assert!(
            !caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| instr_any(instr, |child| matches!(
                    child,
                    InstrCodegen::CallDirect(_)
                ))),
            "caller should not retain direct calls after fixed-point inlining: {caller:#?}"
        );
        assert!(
            !caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| instr_any(instr, |child| matches!(child, InstrCodegen::GetAttr(_)))),
            "caller field read should be replaced with scalar local load: {caller:#?}"
        );
    }
}
