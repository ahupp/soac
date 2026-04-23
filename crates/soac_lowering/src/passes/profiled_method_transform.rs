use std::collections::{HashMap, HashSet};

use crate::block_py::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockPyFunction, BlockTerm, Call, CallArgPositional,
    CallDirect, ChildVisitable, Del, GetAttr, HasMeta, HasSemanticInstrId, InstrId, Load, Meta,
    NameLocation, ResolvedName, RuntimeFunctionId, RuntimeName, Store, Visit, WithMeta,
};

use super::{
    allocate_codegen_stack_temp, assign_missing_codegen_function_instr_ids,
    build_cross_module_direct_method_inline_fragment_to_target,
    build_direct_method_inline_fragment_to_target, rewrite_static_runtime_constructor_call_stores,
    CodegenModuleShape, DirectFunctionIdGuardTest, DirectReceiverTypeVersionGuardTest,
    InlineCallee, InstrCodegen, InstrResolved, TypedAttrOwnerRef,
};
use crate::block_py::literal::Literal;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProfiledOwnerAttrKey {
    pub function_id: RuntimeFunctionId,
    pub attr_name: String,
}

impl ProfiledOwnerAttrKey {
    pub fn new(function_id: RuntimeFunctionId, attr_name: &str) -> Self {
        Self {
            function_id,
            attr_name: attr_name.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfiledOwnerAttrSpecialization {
    pub owner_type_ref: TypedAttrOwnerRef,
    pub type_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfiledRuntimeIterConstructorCall {
    pub constructor_function_id: RuntimeFunctionId,
    pub inline_target: Option<RuntimeFunctionId>,
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct ProfiledMethodInlineRewriteStats {
    pub candidate_stores: usize,
    pub missing_callee_targets: usize,
    pub missing_owner_guard_targets: usize,
    pub owner_fragment_unsupported_targets: usize,
    pub callable_fragment_unsupported_targets: usize,
    pub stop_iteration_candidate_exc_edges: usize,
    pub stop_iteration_current_exception_edges: usize,
    pub stop_iteration_handler_blocks: usize,
    pub stop_iteration_handler_if_terms: usize,
    pub stop_iteration_handler_test_matches: usize,
    pub stop_iteration_handler_targets: usize,
    pub stop_iteration_raises_rewritten: usize,
    pub static_runtime_constructor_calls_rewritten: usize,
    pub rewritten_stores: usize,
}

impl ProfiledMethodInlineRewriteStats {
    pub fn total_attempts(&self) -> usize {
        self.candidate_stores
            + self.missing_callee_targets
            + self.missing_owner_guard_targets
            + self.owner_fragment_unsupported_targets
            + self.callable_fragment_unsupported_targets
            + self.stop_iteration_candidate_exc_edges
            + self.stop_iteration_current_exception_edges
            + self.stop_iteration_handler_blocks
            + self.stop_iteration_handler_if_terms
            + self.stop_iteration_handler_test_matches
            + self.stop_iteration_handler_targets
            + self.stop_iteration_raises_rewritten
            + self.static_runtime_constructor_calls_rewritten
            + self.rewritten_stores
    }
}

struct ProfiledMethodInlineCandidate {
    instr_index: usize,
    target: ResolvedName,
    receiver: InstrCodegen,
    attr: InstrCodegen,
    method_name: String,
    instr_id: InstrId,
}

struct ProfiledMethodInlineFragment {
    guard: ProfiledOwnerAttrSpecialization,
    entry_label: BlockLabel,
    blocks: Vec<Block<InstrCodegen>>,
}

struct ProfiledMethodCallableGuardFragment {
    function_id: RuntimeFunctionId,
    entry_label: BlockLabel,
    blocks: Vec<Block<InstrCodegen>>,
}

struct ProfiledRuntimeIterInlineCandidate {
    instr_index: usize,
    target: ResolvedName,
    func: InstrCodegen,
    receiver: InstrCodegen,
    constructor_instr_id: InstrId,
}

pub fn rewrite_profiled_no_arg_method_call_store_sites(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    caller_constants: &mut Vec<InstrResolved>,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
    direct_owner_attr_specializations: &HashMap<
        ProfiledOwnerAttrKey,
        Vec<ProfiledOwnerAttrSpecialization>,
    >,
    callees: &HashMap<RuntimeFunctionId, InlineCallee>,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
    inline_constructor_calls_by_source: &HashMap<InstrId, Vec<ProfiledRuntimeIterConstructorCall>>,
    runtime_constructor_function_ids: &mut HashSet<RuntimeFunctionId>,
    constructor_for_runtime_name: &mut impl FnMut(RuntimeName) -> Option<RuntimeFunctionId>,
    iter_target_for_constructor_guard: &mut impl FnMut(
        &ProfiledOwnerAttrSpecialization,
    ) -> Option<RuntimeFunctionId>,
) -> ProfiledMethodInlineRewriteStats {
    let mut stats = ProfiledMethodInlineRewriteStats::default();
    let original_blocks = std::mem::take(&mut function.blocks);
    let original_block_by_label = original_blocks
        .iter()
        .cloned()
        .map(|block| (block.label, block))
        .collect::<HashMap<_, _>>();
    let mut rewritten_blocks = Vec::with_capacity(original_blocks.len());
    for block in original_blocks {
        match rewrite_profiled_no_arg_method_call_store_block(
            function,
            caller_constants,
            block.clone(),
            targets_by_instr_id,
            direct_owner_attr_specializations,
            callees,
            straightline_constructor_ids,
            inline_constructor_calls_by_source,
            &original_block_by_label,
            &mut stats,
            runtime_constructor_function_ids,
            constructor_for_runtime_name,
            iter_target_for_constructor_guard,
        ) {
            Some(blocks) => {
                stats.rewritten_stores += 1;
                rewritten_blocks.extend(blocks);
            }
            None => rewritten_blocks.push(block),
        }
    }
    function.blocks = rewritten_blocks;
    if stats.rewritten_stores != 0 {
        assign_missing_codegen_function_instr_ids(function);
    }
    stats
}

fn rewrite_profiled_no_arg_method_call_store_block(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    caller_constants: &mut Vec<InstrResolved>,
    block: Block<InstrCodegen>,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
    direct_owner_attr_specializations: &HashMap<
        ProfiledOwnerAttrKey,
        Vec<ProfiledOwnerAttrSpecialization>,
    >,
    callees: &HashMap<RuntimeFunctionId, InlineCallee>,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
    inline_constructor_calls_by_source: &HashMap<InstrId, Vec<ProfiledRuntimeIterConstructorCall>>,
    original_block_by_label: &HashMap<BlockLabel, Block<InstrCodegen>>,
    stats: &mut ProfiledMethodInlineRewriteStats,
    runtime_constructor_function_ids: &mut HashSet<RuntimeFunctionId>,
    constructor_for_runtime_name: &mut impl FnMut(RuntimeName) -> Option<RuntimeFunctionId>,
    iter_target_for_constructor_guard: &mut impl FnMut(
        &ProfiledOwnerAttrSpecialization,
    ) -> Option<RuntimeFunctionId>,
) -> Option<Vec<Block<InstrCodegen>>> {
    let Some(candidate) = find_profiled_no_arg_method_inline_candidate(
        caller_constants.as_slice(),
        &block,
        targets_by_instr_id,
    ) else {
        return rewrite_profiled_runtime_iter_call_store_block(
            function,
            caller_constants,
            block,
            targets_by_instr_id,
            direct_owner_attr_specializations,
            callees,
            straightline_constructor_ids,
            inline_constructor_calls_by_source,
            stats,
            runtime_constructor_function_ids,
            constructor_for_runtime_name,
            iter_target_for_constructor_guard,
        );
    };
    stats.candidate_stores += 1;
    let targets = targets_by_instr_id.get(&candidate.instr_id)?;
    let continuation_label = function.name_gen.next_block_name();
    let receiver_temp = allocate_codegen_stack_temp(function, "direct_method_receiver");
    let receiver_temp_name = receiver_temp.resolved_name();
    let exc_edge = block.exc_edge.clone();

    let mut fragments = Vec::new();
    for function_id in dedup_function_ids(targets) {
        let Some(callee) = callees.get(&function_id) else {
            stats.missing_callee_targets += 1;
            continue;
        };
        let key = ProfiledOwnerAttrKey::new(function_id, candidate.method_name.as_str());
        let Some(guards) = direct_owner_attr_specializations.get(&key) else {
            stats.missing_owner_guard_targets += 1;
            continue;
        };
        for guard in guards {
            let fragment_result = match callee.module_constants() {
                Some(callee_constants) => {
                    build_cross_module_direct_method_inline_fragment_to_target(
                        function,
                        caller_constants,
                        callee.function(),
                        callee_constants,
                        continuation_label,
                        load_codegen_temp(&receiver_temp_name),
                        &[],
                        candidate.target.clone(),
                    )
                }
                None => build_direct_method_inline_fragment_to_target(
                    function,
                    callee.function(),
                    continuation_label,
                    load_codegen_temp(&receiver_temp_name),
                    &[],
                    candidate.target.clone(),
                ),
            };
            let Ok(mut fragment) = fragment_result else {
                stats.owner_fragment_unsupported_targets += 1;
                continue;
            };
            for fragment_block in &mut fragment.blocks {
                fragment_block.exc_edge = exc_edge.clone();
            }
            let mut record_runtime_constructor = |runtime_name| {
                let function_id = constructor_for_runtime_name(runtime_name)?;
                runtime_constructor_function_ids.insert(function_id);
                Some(function_id)
            };
            stats.static_runtime_constructor_calls_rewritten +=
                rewrite_static_runtime_constructor_call_stores(
                    &mut fragment.blocks,
                    caller_constants.as_slice(),
                    &mut record_runtime_constructor,
                );
            if let Some(stop_iteration_target) = stop_iteration_handler_target(
                exc_edge.as_ref(),
                original_block_by_label,
                caller_constants.as_slice(),
                stats,
            ) {
                stats.stop_iteration_handler_targets += 1;
                stats.stop_iteration_raises_rewritten += rewrite_inlined_stop_iteration_raises(
                    &mut fragment.blocks,
                    stop_iteration_target,
                    caller_constants.as_slice(),
                );
            }
            append_cleanup_before_method_inline_exit(
                &mut fragment.blocks,
                continuation_label,
                &receiver_temp_name,
            );
            fragments.push(ProfiledMethodInlineFragment {
                guard: guard.clone(),
                entry_label: fragment.entry_label,
                blocks: fragment.blocks,
            });
        }
    }
    if fragments.is_empty() {
        return rewrite_profiled_no_arg_method_call_store_block_with_callable_guard(
            function,
            caller_constants,
            block,
            candidate,
            targets.as_slice(),
            callees,
            original_block_by_label,
            stats,
            runtime_constructor_function_ids,
            constructor_for_runtime_name,
        );
    }

    let mut before = block.body;
    let after = before.split_off(candidate.instr_index + 1);
    before.truncate(candidate.instr_index);
    before.push(
        Store::new(receiver_temp_name.clone(), candidate.receiver)
            .with_meta(Meta::synthetic())
            .into(),
    );

    let generic_label = function.name_gen.next_block_name();
    let guard_labels = (0..fragments.len().saturating_sub(1))
        .map(|_| function.name_gen.next_block_name())
        .collect::<Vec<_>>();

    let entry_term = receiver_type_version_guard_term(
        &receiver_temp_name,
        &fragments[0],
        guard_labels.first().copied().unwrap_or(generic_label),
    );
    let entry = Block::new(
        block.label,
        before,
        entry_term,
        block.params,
        exc_edge.clone(),
    );

    let mut blocks = Vec::with_capacity(fragments.len() * 2 + 3);
    blocks.push(entry);

    for (guard_index, guard_label) in guard_labels.iter().copied().enumerate() {
        let target_index = guard_index + 1;
        let else_label = guard_labels
            .get(guard_index + 1)
            .copied()
            .unwrap_or(generic_label);
        blocks.push(Block::new(
            guard_label,
            Vec::new(),
            receiver_type_version_guard_term(
                &receiver_temp_name,
                &fragments[target_index],
                else_label,
            ),
            Vec::new(),
            exc_edge.clone(),
        ));
    }

    for fragment in fragments {
        blocks.extend(fragment.blocks);
    }

    blocks.push(Block::new(
        generic_label,
        vec![
            Store::new(
                candidate.target,
                InstrCodegen::Call(
                    Call::new(
                        InstrCodegen::GetAttr(
                            GetAttr::new(load_codegen_temp(&receiver_temp_name), candidate.attr)
                                .with_meta(Meta::synthetic()),
                        ),
                        Vec::new(),
                        Vec::new(),
                    )
                    .with_meta(Meta::synthetic()),
                ),
            )
            .with_meta(Meta::synthetic())
            .into(),
            Del::new(receiver_temp_name.clone(), false)
                .with_meta(Meta::synthetic())
                .into(),
        ],
        BlockTerm::Jump(BlockEdge::new(continuation_label)),
        Vec::new(),
        exc_edge.clone(),
    ));

    blocks.push(Block::new(
        continuation_label,
        after,
        block.term,
        Vec::new(),
        exc_edge,
    ));

    Some(blocks)
}

fn rewrite_profiled_runtime_iter_call_store_block(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    caller_constants: &mut Vec<InstrResolved>,
    block: Block<InstrCodegen>,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
    direct_owner_attr_specializations: &HashMap<
        ProfiledOwnerAttrKey,
        Vec<ProfiledOwnerAttrSpecialization>,
    >,
    callees: &HashMap<RuntimeFunctionId, InlineCallee>,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
    inline_constructor_calls_by_source: &HashMap<InstrId, Vec<ProfiledRuntimeIterConstructorCall>>,
    stats: &mut ProfiledMethodInlineRewriteStats,
    runtime_constructor_function_ids: &mut HashSet<RuntimeFunctionId>,
    constructor_for_runtime_name: &mut impl FnMut(RuntimeName) -> Option<RuntimeFunctionId>,
    iter_target_for_constructor_guard: &mut impl FnMut(
        &ProfiledOwnerAttrSpecialization,
    ) -> Option<RuntimeFunctionId>,
) -> Option<Vec<Block<InstrCodegen>>> {
    let candidate = find_profiled_runtime_iter_inline_candidate(
        caller_constants.as_slice(),
        &block,
        targets_by_instr_id,
    )?;
    stats.candidate_stores += 1;
    let targets = targets_by_instr_id.get(&candidate.constructor_instr_id)?;
    let continuation_label = function.name_gen.next_block_name();
    let receiver_temp = allocate_codegen_stack_temp(function, "runtime_iter_receiver");
    let receiver_temp_name = receiver_temp.resolved_name();
    let exc_edge = block.exc_edge.clone();

    let mut fragments = Vec::new();
    let mut fragment_constructor_function_ids = Vec::new();
    for function_id in dedup_function_ids(targets) {
        let planned_inline_target = inline_constructor_calls_by_source
            .get(&candidate.constructor_instr_id)
            .and_then(|plans| {
                plans
                    .iter()
                    .find(|plan| plan.constructor_function_id == function_id)
                    .and_then(|plan| plan.inline_target)
            });
        for guard in direct_constructor_owner_attr_specializations_from_source(
            direct_owner_attr_specializations,
            function_id,
        ) {
            let iter_function_id = if let Some(iter_function_id) = planned_inline_target {
                iter_function_id
            } else {
                let Some(function_id) = iter_target_for_constructor_guard(&guard) else {
                    stats.missing_owner_guard_targets += 1;
                    continue;
                };
                function_id
            };
            let Some(callee) = callees.get(&iter_function_id) else {
                stats.missing_callee_targets += 1;
                continue;
            };
            let fragment_result = match callee.module_constants() {
                Some(callee_constants) => {
                    build_cross_module_direct_method_inline_fragment_to_target(
                        function,
                        caller_constants,
                        callee.function(),
                        callee_constants,
                        continuation_label,
                        load_codegen_temp(&receiver_temp_name),
                        &[],
                        candidate.target.clone(),
                    )
                }
                None => build_direct_method_inline_fragment_to_target(
                    function,
                    callee.function(),
                    continuation_label,
                    load_codegen_temp(&receiver_temp_name),
                    &[],
                    candidate.target.clone(),
                ),
            };
            let Ok(mut fragment) = fragment_result else {
                stats.owner_fragment_unsupported_targets += 1;
                continue;
            };
            for fragment_block in &mut fragment.blocks {
                fragment_block.exc_edge = exc_edge.clone();
            }
            let mut record_runtime_constructor = |runtime_name| {
                let function_id = constructor_for_runtime_name(runtime_name)?;
                runtime_constructor_function_ids.insert(function_id);
                Some(function_id)
            };
            stats.static_runtime_constructor_calls_rewritten +=
                rewrite_static_runtime_constructor_call_stores(
                    &mut fragment.blocks,
                    caller_constants.as_slice(),
                    &mut record_runtime_constructor,
                );
            append_cleanup_before_method_inline_exit(
                &mut fragment.blocks,
                continuation_label,
                &receiver_temp_name,
            );
            fragments.push(ProfiledMethodInlineFragment {
                guard,
                entry_label: fragment.entry_label,
                blocks: fragment.blocks,
            });
            fragment_constructor_function_ids.push(function_id);
        }
    }
    if fragments.is_empty() {
        return None;
    }
    let scalarizable_constructor_function_id =
        if let [function_id] = fragment_constructor_function_ids.as_slice() {
            Some(*function_id)
        } else {
            None
        };

    let mut before = block.body;
    let after = before.split_off(candidate.instr_index + 1);
    before.truncate(candidate.instr_index);
    let receiver_for_store = scalarizable_profiled_constructor_receiver(
        candidate.receiver,
        scalarizable_constructor_function_id,
        straightline_constructor_ids,
    );
    before.push(
        Store::new(receiver_temp_name.clone(), receiver_for_store)
            .with_meta(Meta::synthetic())
            .into(),
    );

    let generic_label = function.name_gen.next_block_name();
    let guard_labels = (0..fragments.len().saturating_sub(1))
        .map(|_| function.name_gen.next_block_name())
        .collect::<Vec<_>>();

    let entry_term = receiver_type_version_guard_term(
        &receiver_temp_name,
        &fragments[0],
        guard_labels.first().copied().unwrap_or(generic_label),
    );
    let entry = Block::new(
        block.label,
        before,
        entry_term,
        block.params,
        exc_edge.clone(),
    );

    let mut blocks = Vec::with_capacity(fragments.len() * 2 + 3);
    blocks.push(entry);

    for (guard_index, guard_label) in guard_labels.iter().copied().enumerate() {
        let target_index = guard_index + 1;
        let else_label = guard_labels
            .get(guard_index + 1)
            .copied()
            .unwrap_or(generic_label);
        blocks.push(Block::new(
            guard_label,
            Vec::new(),
            receiver_type_version_guard_term(
                &receiver_temp_name,
                &fragments[target_index],
                else_label,
            ),
            Vec::new(),
            exc_edge.clone(),
        ));
    }

    for fragment in fragments {
        blocks.extend(fragment.blocks);
    }

    blocks.push(Block::new(
        generic_label,
        vec![
            Store::new(
                candidate.target,
                InstrCodegen::Call(
                    Call::new(
                        candidate.func,
                        vec![CallArgPositional::Positional(load_codegen_temp(
                            &receiver_temp_name,
                        ))],
                        Vec::new(),
                    )
                    .with_meta(Meta::synthetic()),
                ),
            )
            .with_meta(Meta::synthetic())
            .into(),
            Del::new(receiver_temp_name.clone(), false)
                .with_meta(Meta::synthetic())
                .into(),
        ],
        BlockTerm::Jump(BlockEdge::new(continuation_label)),
        Vec::new(),
        exc_edge.clone(),
    ));

    blocks.push(Block::new(
        continuation_label,
        after,
        block.term,
        Vec::new(),
        exc_edge,
    ));

    Some(blocks)
}

fn scalarizable_profiled_constructor_receiver(
    receiver: InstrCodegen,
    constructor_function_id: Option<RuntimeFunctionId>,
    straightline_constructor_ids: &HashSet<RuntimeFunctionId>,
) -> InstrCodegen {
    let Some(constructor_function_id) = constructor_function_id else {
        return receiver;
    };
    if !straightline_constructor_ids.contains(&constructor_function_id) {
        return receiver;
    }
    let InstrCodegen::Call(call) = receiver else {
        return receiver;
    };
    let meta = call.meta();
    InstrCodegen::CallDirect(
        CallDirect::new(call.func, constructor_function_id, call.args, call.keywords)
            .with_meta(meta),
    )
}

fn rewrite_profiled_no_arg_method_call_store_block_with_callable_guard(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    caller_constants: &mut Vec<InstrResolved>,
    block: Block<InstrCodegen>,
    candidate: ProfiledMethodInlineCandidate,
    targets: &[RuntimeFunctionId],
    callees: &HashMap<RuntimeFunctionId, InlineCallee>,
    original_block_by_label: &HashMap<BlockLabel, Block<InstrCodegen>>,
    stats: &mut ProfiledMethodInlineRewriteStats,
    runtime_constructor_function_ids: &mut HashSet<RuntimeFunctionId>,
    constructor_for_runtime_name: &mut impl FnMut(RuntimeName) -> Option<RuntimeFunctionId>,
) -> Option<Vec<Block<InstrCodegen>>> {
    let continuation_label = function.name_gen.next_block_name();
    let receiver_temp = allocate_codegen_stack_temp(function, "direct_method_receiver");
    let receiver_temp_name = receiver_temp.resolved_name();
    let method_temp = allocate_codegen_stack_temp(function, "direct_method_callable");
    let method_temp_name = method_temp.resolved_name();
    let exc_edge = block.exc_edge.clone();
    let stop_iteration_target = stop_iteration_handler_target(
        exc_edge.as_ref(),
        original_block_by_label,
        caller_constants.as_slice(),
        stats,
    );

    let mut fragments = Vec::new();
    for function_id in dedup_function_ids(targets) {
        let Some(callee) = callees.get(&function_id) else {
            stats.missing_callee_targets += 1;
            continue;
        };
        let fragment_result = match callee.module_constants() {
            Some(callee_constants) => build_cross_module_direct_method_inline_fragment_to_target(
                function,
                caller_constants,
                callee.function(),
                callee_constants,
                continuation_label,
                load_codegen_temp(&receiver_temp_name),
                &[],
                candidate.target.clone(),
            ),
            None => build_direct_method_inline_fragment_to_target(
                function,
                callee.function(),
                continuation_label,
                load_codegen_temp(&receiver_temp_name),
                &[],
                candidate.target.clone(),
            ),
        };
        let Ok(mut fragment) = fragment_result else {
            stats.callable_fragment_unsupported_targets += 1;
            continue;
        };
        for fragment_block in &mut fragment.blocks {
            fragment_block.exc_edge = exc_edge.clone();
        }
        let mut record_runtime_constructor = |runtime_name| {
            let function_id = constructor_for_runtime_name(runtime_name)?;
            runtime_constructor_function_ids.insert(function_id);
            Some(function_id)
        };
        stats.static_runtime_constructor_calls_rewritten +=
            rewrite_static_runtime_constructor_call_stores(
                &mut fragment.blocks,
                caller_constants.as_slice(),
                &mut record_runtime_constructor,
            );
        if let Some(stop_iteration_target) = stop_iteration_target {
            stats.stop_iteration_handler_targets += 1;
            stats.stop_iteration_raises_rewritten += rewrite_inlined_stop_iteration_raises(
                &mut fragment.blocks,
                stop_iteration_target,
                caller_constants.as_slice(),
            );
        }
        append_cleanup_before_method_inline_exit(
            &mut fragment.blocks,
            continuation_label,
            &method_temp_name,
        );
        append_cleanup_before_method_inline_exit(
            &mut fragment.blocks,
            continuation_label,
            &receiver_temp_name,
        );
        fragments.push(ProfiledMethodCallableGuardFragment {
            function_id,
            entry_label: fragment.entry_label,
            blocks: fragment.blocks,
        });
    }
    if fragments.is_empty() {
        return None;
    }

    let mut before = block.body;
    let after = before.split_off(candidate.instr_index + 1);
    before.truncate(candidate.instr_index);
    before.push(
        Store::new(receiver_temp_name.clone(), candidate.receiver)
            .with_meta(Meta::synthetic())
            .into(),
    );
    before.push(
        Store::new(
            method_temp_name.clone(),
            InstrCodegen::GetAttr(
                GetAttr::new(load_codegen_temp(&receiver_temp_name), candidate.attr)
                    .with_meta(Meta::synthetic()),
            ),
        )
        .with_meta(Meta::synthetic())
        .into(),
    );

    let generic_label = function.name_gen.next_block_name();
    let guard_labels = (0..fragments.len().saturating_sub(1))
        .map(|_| function.name_gen.next_block_name())
        .collect::<Vec<_>>();

    let entry_term = callable_function_id_guard_term(
        &method_temp_name,
        fragments[0].function_id,
        fragments[0].entry_label,
        guard_labels.first().copied().unwrap_or(generic_label),
    );
    let entry = Block::new(
        block.label,
        before,
        entry_term,
        block.params,
        exc_edge.clone(),
    );

    let mut blocks = Vec::with_capacity(fragments.len() * 2 + 3);
    blocks.push(entry);

    for (guard_index, guard_label) in guard_labels.iter().copied().enumerate() {
        let target_index = guard_index + 1;
        let else_label = guard_labels
            .get(guard_index + 1)
            .copied()
            .unwrap_or(generic_label);
        blocks.push(Block::new(
            guard_label,
            Vec::new(),
            callable_function_id_guard_term(
                &method_temp_name,
                fragments[target_index].function_id,
                fragments[target_index].entry_label,
                else_label,
            ),
            Vec::new(),
            exc_edge.clone(),
        ));
    }

    for fragment in fragments {
        blocks.extend(fragment.blocks);
    }

    blocks.push(Block::new(
        generic_label,
        vec![
            Store::new(
                candidate.target,
                InstrCodegen::Call(
                    Call::new(load_codegen_temp(&method_temp_name), Vec::new(), Vec::new())
                        .with_meta(Meta::synthetic()),
                ),
            )
            .with_meta(Meta::synthetic())
            .into(),
            Del::new(method_temp_name.clone(), false)
                .with_meta(Meta::synthetic())
                .into(),
            Del::new(receiver_temp_name.clone(), false)
                .with_meta(Meta::synthetic())
                .into(),
        ],
        BlockTerm::Jump(BlockEdge::new(continuation_label)),
        Vec::new(),
        exc_edge.clone(),
    ));

    blocks.push(Block::new(
        continuation_label,
        after,
        block.term,
        Vec::new(),
        exc_edge,
    ));

    Some(blocks)
}

fn callable_function_id_guard_term(
    callable_temp_name: &ResolvedName,
    function_id: RuntimeFunctionId,
    then_label: BlockLabel,
    else_label: BlockLabel,
) -> BlockTerm<InstrCodegen> {
    BlockTerm::IfTerm(crate::block_py::TermIf {
        test: InstrCodegen::DirectFunctionIdGuardTest(
            DirectFunctionIdGuardTest::new(load_codegen_temp(callable_temp_name), function_id)
                .with_meta(Meta::synthetic()),
        ),
        then_label,
        else_label,
    })
}

fn stop_iteration_handler_target(
    exc_edge: Option<&BlockEdge>,
    original_block_by_label: &HashMap<BlockLabel, Block<InstrCodegen>>,
    module_constants: &[InstrResolved],
    stats: &mut ProfiledMethodInlineRewriteStats,
) -> Option<BlockLabel> {
    let exc_edge = exc_edge?;
    stats.stop_iteration_candidate_exc_edges += 1;
    if !matches!(exc_edge.args.as_slice(), [BlockArg::CurrentException]) {
        return None;
    }
    stats.stop_iteration_current_exception_edges += 1;
    let handler = original_block_by_label.get(&exc_edge.target)?;
    stats.stop_iteration_handler_blocks += 1;
    let BlockTerm::IfTerm(if_term) = &handler.term else {
        return None;
    };
    stats.stop_iteration_handler_if_terms += 1;
    if is_exception_matches_stop_iteration_test(&if_term.test, module_constants) {
        stats.stop_iteration_handler_test_matches += 1;
        Some(if_term.then_label)
    } else {
        None
    }
}

fn rewrite_inlined_stop_iteration_raises(
    blocks: &mut [Block<InstrCodegen>],
    stop_iteration_target: BlockLabel,
    module_constants: &[InstrResolved],
) -> usize {
    let mut rewritten = 0usize;
    for block in blocks {
        if let BlockTerm::Raise(raise) = &block.term {
            if raise
                .exc
                .as_ref()
                .is_some_and(|expr| is_stop_iteration_expr(expr, module_constants))
            {
                block.term = BlockTerm::Jump(BlockEdge::new(stop_iteration_target));
                rewritten += 1;
            }
        }
    }
    rewritten
}

fn is_exception_matches_stop_iteration_test(
    expr: &InstrCodegen,
    module_constants: &[InstrResolved],
) -> bool {
    match expr {
        InstrCodegen::Call(call) => {
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
                    ] if is_stop_iteration_expr(stop_iteration, module_constants)
                )
        }
        InstrCodegen::CallDirect(call) => {
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
                    ] if is_stop_iteration_expr(stop_iteration, module_constants)
                )
        }
        _ => false,
    }
}

fn is_stop_iteration_expr(expr: &InstrCodegen, module_constants: &[InstrResolved]) -> bool {
    is_runtime_name_expr(expr, RuntimeName::StopIteration, module_constants)
}

fn is_runtime_name_expr(
    expr: &InstrCodegen,
    runtime_name: RuntimeName,
    module_constants: &[InstrResolved],
) -> bool {
    let InstrCodegen::Load(load) = expr else {
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

pub fn collect_profiled_runtime_iter_method_target_ids(
    function: &BlockPyFunction<CodegenModuleShape>,
    module_constants: &[InstrResolved],
    direct_owner_attr_specializations: &HashMap<
        ProfiledOwnerAttrKey,
        Vec<ProfiledOwnerAttrSpecialization>,
    >,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
    iter_target_for_constructor_guard: &mut impl FnMut(
        &ProfiledOwnerAttrSpecialization,
    ) -> Option<RuntimeFunctionId>,
) -> HashSet<RuntimeFunctionId> {
    struct Collector<'a, F>
    where
        F: FnMut(&ProfiledOwnerAttrSpecialization) -> Option<RuntimeFunctionId>,
    {
        module_constants: &'a [InstrResolved],
        direct_owner_attr_specializations:
            &'a HashMap<ProfiledOwnerAttrKey, Vec<ProfiledOwnerAttrSpecialization>>,
        targets_by_instr_id: &'a HashMap<InstrId, Vec<RuntimeFunctionId>>,
        iter_target_for_constructor_guard: &'a mut F,
        out: &'a mut HashSet<RuntimeFunctionId>,
    }

    impl<F> Visit<InstrCodegen> for Collector<'_, F>
    where
        F: FnMut(&ProfiledOwnerAttrSpecialization) -> Option<RuntimeFunctionId>,
    {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            if let InstrCodegen::Call(call) = expr {
                self.collect_call(call);
            }
            expr.visit_children(self);
        }
    }

    impl<F> Collector<'_, F>
    where
        F: FnMut(&ProfiledOwnerAttrSpecialization) -> Option<RuntimeFunctionId>,
    {
        fn collect_call(&mut self, call: &Call<InstrCodegen>) {
            if !is_runtime_name_expr(call.func.as_ref(), RuntimeName::Iter, self.module_constants)
                || !call.keywords.is_empty()
                || call.args.len() != 1
            {
                return;
            }
            let CallArgPositional::Positional(receiver) = &call.args[0] else {
                return;
            };
            let InstrCodegen::Call(constructor_call) = receiver else {
                return;
            };
            let Some(instr_id) = constructor_call.try_semantic_instr_id() else {
                return;
            };
            let Some(targets) = self.targets_by_instr_id.get(&instr_id) else {
                return;
            };
            for function_id in targets {
                for owner in direct_constructor_owner_attr_specializations_from_source(
                    self.direct_owner_attr_specializations,
                    *function_id,
                ) {
                    let Some(iter_function_id) = (self.iter_target_for_constructor_guard)(&owner)
                    else {
                        continue;
                    };
                    self.out.insert(iter_function_id);
                }
            }
        }
    }

    let mut out = HashSet::new();
    let mut collector = Collector {
        module_constants,
        direct_owner_attr_specializations,
        targets_by_instr_id,
        iter_target_for_constructor_guard,
        out: &mut out,
    };
    collector.visit_fn(function);
    out
}

fn find_profiled_no_arg_method_inline_candidate(
    module_constants: &[InstrResolved],
    block: &Block<InstrCodegen>,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
) -> Option<ProfiledMethodInlineCandidate> {
    block
        .body
        .iter()
        .enumerate()
        .find_map(|(instr_index, instr)| {
            let InstrCodegen::Store(store) = instr else {
                return None;
            };
            let InstrCodegen::Call(call) = store.value.as_ref() else {
                return None;
            };
            if !call.args.is_empty() || !call.keywords.is_empty() {
                return None;
            }
            let instr_id = call.try_semantic_instr_id()?;
            if targets_by_instr_id.get(&instr_id).is_none_or(Vec::is_empty) {
                return None;
            }
            let InstrCodegen::GetAttr(getattr) = call.func.as_ref() else {
                return None;
            };
            let method_name =
                codegen_constant_string_value_from_slice(module_constants, getattr.attr.as_ref())?;
            Some(ProfiledMethodInlineCandidate {
                instr_index,
                target: store.name.clone(),
                receiver: (*getattr.value).clone(),
                attr: (*getattr.attr).clone(),
                method_name: method_name.to_string(),
                instr_id,
            })
        })
}

fn find_profiled_runtime_iter_inline_candidate(
    module_constants: &[InstrResolved],
    block: &Block<InstrCodegen>,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
) -> Option<ProfiledRuntimeIterInlineCandidate> {
    block
        .body
        .iter()
        .enumerate()
        .find_map(|(instr_index, instr)| {
            let InstrCodegen::Store(store) = instr else {
                return None;
            };
            let InstrCodegen::Call(call) = store.value.as_ref() else {
                return None;
            };
            if !is_runtime_name_expr(call.func.as_ref(), RuntimeName::Iter, module_constants)
                || !call.keywords.is_empty()
                || call.args.len() != 1
            {
                return None;
            }
            let CallArgPositional::Positional(receiver) = &call.args[0] else {
                return None;
            };
            let InstrCodegen::Call(constructor_call) = receiver else {
                return None;
            };
            let constructor_instr_id = constructor_call.try_semantic_instr_id()?;
            if targets_by_instr_id
                .get(&constructor_instr_id)
                .is_none_or(Vec::is_empty)
            {
                return None;
            }
            Some(ProfiledRuntimeIterInlineCandidate {
                instr_index,
                target: store.name.clone(),
                func: (*call.func).clone(),
                receiver: (*receiver).clone(),
                constructor_instr_id,
            })
        })
}

fn codegen_constant_string_value_from_slice<'a>(
    module_constants: &'a [InstrResolved],
    expr: &InstrCodegen,
) -> Option<&'a str> {
    let InstrCodegen::Load(load) = expr else {
        return None;
    };
    let NameLocation::Constant(constant_index) = load.name.location else {
        return None;
    };
    let InstrResolved::Literal(literal) = module_constants.get(constant_index as usize)? else {
        return None;
    };
    let Literal::StringLiteral(literal) = literal.as_literal() else {
        return None;
    };
    Some(literal.value.as_str())
}

fn receiver_type_version_guard_term(
    receiver_temp_name: &ResolvedName,
    fragment: &ProfiledMethodInlineFragment,
    else_label: BlockLabel,
) -> BlockTerm<InstrCodegen> {
    receiver_type_version_guard_term_for_owner(
        receiver_temp_name,
        fragment.guard.owner_type_ref.clone(),
        fragment.guard.type_version,
        fragment.entry_label,
        else_label,
    )
}

fn receiver_type_version_guard_term_for_owner(
    receiver_temp_name: &ResolvedName,
    owner_type_ref: TypedAttrOwnerRef,
    type_version: u32,
    then_label: BlockLabel,
    else_label: BlockLabel,
) -> BlockTerm<InstrCodegen> {
    BlockTerm::IfTerm(crate::block_py::TermIf {
        test: InstrCodegen::DirectReceiverTypeVersionGuardTest(
            DirectReceiverTypeVersionGuardTest::new(
                load_codegen_temp(receiver_temp_name),
                owner_type_ref,
                type_version,
            )
            .with_meta(Meta::synthetic()),
        ),
        then_label,
        else_label,
    })
}

fn append_cleanup_before_method_inline_exit(
    blocks: &mut [Block<InstrCodegen>],
    continuation_label: BlockLabel,
    receiver_temp_name: &ResolvedName,
) {
    for block in blocks {
        match &block.term {
            BlockTerm::Jump(edge) if edge.target == continuation_label => {
                block.body.push(
                    Del::new(receiver_temp_name.clone(), false)
                        .with_meta(Meta::synthetic())
                        .into(),
                );
            }
            BlockTerm::Raise(_) => {
                block.body.push(
                    Del::new(receiver_temp_name.clone(), false)
                        .with_meta(Meta::synthetic())
                        .into(),
                );
            }
            BlockTerm::Jump(_)
            | BlockTerm::IfTerm(_)
            | BlockTerm::BranchTable(_)
            | BlockTerm::Return(_) => {}
        }
    }
}

fn load_codegen_temp(temp_name: &ResolvedName) -> InstrCodegen {
    InstrCodegen::Load(Load::new(temp_name.clone()).with_meta(Meta::synthetic()))
}

fn dedup_function_ids(function_ids: &[RuntimeFunctionId]) -> Vec<RuntimeFunctionId> {
    let mut seen = HashSet::new();
    function_ids
        .iter()
        .copied()
        .filter(|function_id| seen.insert(*function_id))
        .collect()
}

fn direct_constructor_owner_attr_specializations_from_source(
    source: &HashMap<ProfiledOwnerAttrKey, Vec<ProfiledOwnerAttrSpecialization>>,
    function_id: RuntimeFunctionId,
) -> Vec<ProfiledOwnerAttrSpecialization> {
    let key = ProfiledOwnerAttrKey::new(function_id, "__init__");
    source.get(&key).cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::CallArgKeyword;

    fn test_local_name(name: &str, slot: u32) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::local(slot),
        }
    }

    fn test_global_name(name: &str) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::global(0),
        }
    }

    fn name_expr(name: ResolvedName) -> InstrCodegen {
        Load::new(name).into()
    }

    #[test]
    fn profiled_runtime_iter_receiver_call_direct_requires_straightline_constructor() {
        let constructor_id = RuntimeFunctionId::from_raw_parts(7, 11);
        let receiver = InstrCodegen::Call(Call::new(
            name_expr(test_global_name("RangeLike")),
            vec![CallArgPositional::Positional(name_expr(test_local_name(
                "n", 0,
            )))],
            Vec::<CallArgKeyword<InstrCodegen>>::new(),
        ));

        let unchanged = scalarizable_profiled_constructor_receiver(
            receiver.clone(),
            Some(constructor_id),
            &HashSet::new(),
        );
        assert!(
            matches!(unchanged, InstrCodegen::Call(_)),
            "non-straightline constructors must stay as ordinary constructor calls"
        );

        let rewritten = scalarizable_profiled_constructor_receiver(
            receiver,
            Some(constructor_id),
            &HashSet::from([constructor_id]),
        );
        let InstrCodegen::CallDirect(call) = rewritten else {
            panic!("straightline constructors should become scalar-replacement candidates");
        };
        assert_eq!(call.function_id, constructor_id);
    }
}
