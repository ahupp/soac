use std::collections::{HashMap, HashSet};

use crate::block_py::{
    Block, BlockEdge, BlockLabel, BlockPyFunction, BlockTerm, Call, CallArgPositional, CallDirect,
    Del, HasSemanticInstrId, InstrId, Load, Meta, ParamKind, ResolvedName, RuntimeFunctionId,
    Store, TermIf, WithMeta,
};

use super::{
    allocate_codegen_stack_temp, assign_missing_codegen_function_instr_ids, CodegenModuleShape,
    DirectFunctionIdGuardTest, InstrCodegen, InstrCodegenOp,
};

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub struct DirectCallStoreRewriteStats {
    pub rewritten_stores: usize,
    pub skipped_empty_targets: usize,
    pub skipped_incompatible_targets: usize,
    pub skipped_missing_callee_targets: usize,
    pub skipped_arity_mismatch_targets: usize,
    pub skipped_unsupported_init_targets: usize,
    pub skipped_missing_storage_layout_targets: usize,
    pub skipped_unsupported_param_kind_targets: usize,
    pub skipped_missing_param_storage_targets: usize,
}

struct StoreCallCandidate {
    site: CallCandidateSite,
    call: Call<InstrCodegen>,
    targets: Vec<RuntimeFunctionId>,
}

enum CallCandidateSite {
    Store {
        instr_index: usize,
        target: ResolvedName,
    },
}

enum StoreCallRewrite {
    Rewritten(Vec<Block<InstrCodegen>>),
    Unchanged(Block<InstrCodegen>),
}

pub fn rewrite_profiled_function_call_store_sites(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
    callees: &HashMap<RuntimeFunctionId, BlockPyFunction<CodegenModuleShape>>,
) -> DirectCallStoreRewriteStats {
    rewrite_profiled_function_call_store_sites_with_constructor_targets(
        function,
        targets_by_instr_id,
        callees,
        false,
    )
}

pub fn rewrite_profiled_function_call_store_sites_with_constructor_targets(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
    callees: &HashMap<RuntimeFunctionId, BlockPyFunction<CodegenModuleShape>>,
    allow_constructor_targets: bool,
) -> DirectCallStoreRewriteStats {
    let mut stats = DirectCallStoreRewriteStats::default();
    let original_blocks = std::mem::take(&mut function.blocks);
    let mut rewritten_blocks = Vec::with_capacity(original_blocks.len());
    for block in original_blocks {
        match rewrite_profiled_function_call_store_block(
            function,
            block,
            targets_by_instr_id,
            callees,
            allow_constructor_targets,
            &mut stats,
        ) {
            StoreCallRewrite::Rewritten(blocks) => {
                stats.rewritten_stores += 1;
                rewritten_blocks.extend(blocks);
            }
            StoreCallRewrite::Unchanged(block) => rewritten_blocks.push(block),
        }
    }
    function.blocks = rewritten_blocks;
    if stats.rewritten_stores != 0 {
        assign_missing_codegen_function_instr_ids(function);
    }
    stats
}

fn rewrite_profiled_function_call_store_block(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    block: Block<InstrCodegen>,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
    callees: &HashMap<RuntimeFunctionId, BlockPyFunction<CodegenModuleShape>>,
    allow_constructor_targets: bool,
    stats: &mut DirectCallStoreRewriteStats,
) -> StoreCallRewrite {
    let Some(candidate) = find_store_call_candidate(&block, targets_by_instr_id) else {
        return StoreCallRewrite::Unchanged(block);
    };
    if candidate.targets.is_empty() {
        return StoreCallRewrite::Unchanged(block);
    }
    if !candidate.call.keywords.is_empty() {
        return StoreCallRewrite::Unchanged(block);
    }

    let Some(positional_arg_exprs) = positional_arg_exprs(candidate.call.args) else {
        return StoreCallRewrite::Unchanged(block);
    };
    let candidate_targets = compatible_inline_targets(
        positional_arg_exprs.len(),
        candidate.targets,
        callees,
        allow_constructor_targets,
        stats,
    );
    if candidate_targets.is_empty() {
        stats.skipped_empty_targets += 1;
        return StoreCallRewrite::Unchanged(block);
    }

    let temp = allocate_codegen_stack_temp(function, "direct_call_callable");
    let temp_name = temp.resolved_name();
    let arg_temp_names = (0..positional_arg_exprs.len())
        .map(|_| allocate_codegen_stack_temp(function, "direct_call_arg").resolved_name())
        .collect::<Vec<_>>();
    let guard_meta = Meta::synthetic();
    let exc_edge = block.exc_edge.clone();

    let continuation_label = function.name_gen.next_block_name();
    let generic_label = function.name_gen.next_block_name();
    let guard_labels = (0..candidate_targets.len().saturating_sub(1))
        .map(|_| function.name_gen.next_block_name())
        .collect::<Vec<_>>();
    let hot_labels = candidate_targets
        .iter()
        .map(|_| function.name_gen.next_block_name())
        .collect::<Vec<_>>();

    let (mut before, after) = match &candidate.site {
        CallCandidateSite::Store { instr_index, .. } => {
            let mut before = block.body;
            let after = before.split_off(*instr_index + 1);
            before.truncate(*instr_index);
            (before, Some((after, block.term)))
        }
    };
    before.push(
        Store::new(temp_name.clone(), *candidate.call.func)
            .with_meta(Meta::synthetic())
            .into(),
    );
    for (arg_temp_name, arg_expr) in arg_temp_names.iter().cloned().zip(positional_arg_exprs) {
        before.push(
            Store::new(arg_temp_name, arg_expr)
                .with_meta(Meta::synthetic())
                .into(),
        );
    }

    let entry_term = guard_term_for_target(
        &temp_name,
        candidate_targets[0],
        guard_meta.clone(),
        hot_labels[0],
        guard_labels.first().copied().unwrap_or(generic_label),
    );
    let entry = Block::new(
        block.label,
        before,
        entry_term,
        block.params,
        exc_edge.clone(),
    );

    let mut blocks = Vec::with_capacity(candidate_targets.len() * 2 + 3);
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
            guard_term_for_target(
                &temp_name,
                candidate_targets[target_index],
                guard_meta.clone(),
                hot_labels[target_index],
                else_label,
            ),
            Vec::new(),
            exc_edge.clone(),
        ));
    }

    for (function_id, hot_label) in candidate_targets.iter().copied().zip(hot_labels) {
        let result_value = InstrCodegen::CallDirect(
            CallDirect::new(
                load_temp(&temp_name),
                function_id,
                load_temp_args(&arg_temp_names),
                Vec::new(),
            )
            .with_meta(Meta::synthetic()),
        );
        let (body, term) = direct_call_result_block_body_and_term(
            &candidate.site,
            result_value,
            &arg_temp_names,
            &temp_name,
            continuation_label,
        );
        blocks.push(Block::new(
            hot_label,
            body,
            term,
            Vec::new(),
            exc_edge.clone(),
        ));
    }

    let result_value = InstrCodegen::Call(
        Call::new(
            load_temp(&temp_name),
            load_temp_args(&arg_temp_names),
            Vec::new(),
        )
        .with_meta(Meta::synthetic()),
    );
    let (body, term) = direct_call_result_block_body_and_term(
        &candidate.site,
        result_value,
        &arg_temp_names,
        &temp_name,
        continuation_label,
    );
    blocks.push(Block::new(
        generic_label,
        body,
        term,
        Vec::new(),
        exc_edge.clone(),
    ));

    if let Some((after, term)) = after {
        blocks.push(Block::new(
            continuation_label,
            after,
            term,
            Vec::new(),
            exc_edge,
        ));
    }

    StoreCallRewrite::Rewritten(blocks)
}

fn direct_call_result_block_body_and_term(
    site: &CallCandidateSite,
    result_value: InstrCodegen,
    arg_temp_names: &[ResolvedName],
    callable_temp_name: &ResolvedName,
    continuation_label: BlockLabel,
) -> (Vec<InstrCodegen>, BlockTerm<InstrCodegen>) {
    let mut body = Vec::new();
    match site {
        CallCandidateSite::Store { target, .. } => {
            body.push(
                Store::new(target.clone(), result_value)
                    .with_meta(Meta::synthetic())
                    .into(),
            );
            append_cleanup_dels_to_body(&mut body, arg_temp_names);
            append_cleanup_del_to_body(&mut body, callable_temp_name);
            (body, BlockTerm::Jump(BlockEdge::new(continuation_label)))
        }
    }
}

fn find_store_call_candidate(
    block: &Block<InstrCodegen>,
    targets_by_instr_id: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
) -> Option<StoreCallCandidate> {
    block
        .body
        .iter()
        .enumerate()
        .find_map(|(instr_index, instr)| {
            let InstrCodegenOp::Store(store) = instr else {
                return None;
            };
            let InstrCodegenOp::Call(call) = store.value.as_ref() else {
                return None;
            };
            let instr_id = call.try_semantic_instr_id()?;
            let targets = dedup_targets(targets_by_instr_id.get(&instr_id)?);
            Some(StoreCallCandidate {
                site: CallCandidateSite::Store {
                    instr_index,
                    target: store.name.clone(),
                },
                call: call.clone(),
                targets,
            })
        })
}

fn positional_arg_exprs(args: Vec<CallArgPositional<InstrCodegen>>) -> Option<Vec<InstrCodegen>> {
    args.into_iter()
        .map(|arg| match arg {
            CallArgPositional::Positional(expr) => Some(expr),
            CallArgPositional::Starred(_) => None,
        })
        .collect()
}

fn compatible_inline_targets(
    positional_arg_count: usize,
    targets: Vec<RuntimeFunctionId>,
    callees: &HashMap<RuntimeFunctionId, BlockPyFunction<CodegenModuleShape>>,
    allow_constructor_targets: bool,
    stats: &mut DirectCallStoreRewriteStats,
) -> Vec<RuntimeFunctionId> {
    targets
        .into_iter()
        .filter(|target| {
            let Some(callee) = callees.get(target) else {
                stats.skipped_incompatible_targets += 1;
                stats.skipped_missing_callee_targets += 1;
                return false;
            };
            if let Some(reason) = simple_positional_inline_incompatibility(
                callee,
                positional_arg_count,
                allow_constructor_targets,
            ) {
                stats.skipped_incompatible_targets += 1;
                match reason {
                    SimplePositionalInlineIncompatibility::ArityMismatch => {
                        stats.skipped_arity_mismatch_targets += 1;
                    }
                    SimplePositionalInlineIncompatibility::UnsupportedInitTarget => {
                        stats.skipped_unsupported_init_targets += 1;
                    }
                    SimplePositionalInlineIncompatibility::MissingStorageLayout => {
                        stats.skipped_missing_storage_layout_targets += 1;
                    }
                    SimplePositionalInlineIncompatibility::UnsupportedParamKind => {
                        stats.skipped_unsupported_param_kind_targets += 1;
                    }
                    SimplePositionalInlineIncompatibility::MissingParamStorage => {
                        stats.skipped_missing_param_storage_targets += 1;
                    }
                }
                return false;
            }
            true
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum SimplePositionalInlineIncompatibility {
    ArityMismatch,
    UnsupportedInitTarget,
    MissingStorageLayout,
    UnsupportedParamKind,
    MissingParamStorage,
}

fn simple_positional_inline_incompatibility(
    callee: &BlockPyFunction<CodegenModuleShape>,
    positional_arg_count: usize,
    allow_constructor_targets: bool,
) -> Option<SimplePositionalInlineIncompatibility> {
    if callee.names.fn_name == "__init__" && !allow_constructor_targets {
        return Some(SimplePositionalInlineIncompatibility::UnsupportedInitTarget);
    }
    let Some(storage_layout) = &callee.storage_layout else {
        return Some(SimplePositionalInlineIncompatibility::MissingStorageLayout);
    };
    let required_positional_args =
        positional_arg_count + usize::from(callee.names.fn_name == "__init__");
    let accepted_positional_args = callee
        .params
        .iter()
        .filter(|param| matches!(param.kind, ParamKind::PosOnly | ParamKind::Any))
        .count();
    if required_positional_args > accepted_positional_args {
        return Some(SimplePositionalInlineIncompatibility::ArityMismatch);
    }
    let mut consumed_positional_args = 0usize;
    for param in callee.params.iter() {
        match param.kind {
            ParamKind::PosOnly | ParamKind::Any => {
                if consumed_positional_args < required_positional_args {
                    consumed_positional_args += 1;
                } else if !param.has_default {
                    return Some(SimplePositionalInlineIncompatibility::ArityMismatch);
                }
            }
            ParamKind::KwOnly => {
                if !param.has_default {
                    return Some(SimplePositionalInlineIncompatibility::ArityMismatch);
                }
            }
            ParamKind::VarArg | ParamKind::KwArg => {
                return Some(SimplePositionalInlineIncompatibility::UnsupportedParamKind);
            }
        }
        if !storage_layout
            .stack_slots()
            .iter()
            .any(|name| name == &param.name)
        {
            return Some(SimplePositionalInlineIncompatibility::MissingParamStorage);
        }
    }
    None
}

fn dedup_targets(targets: &[RuntimeFunctionId]) -> Vec<RuntimeFunctionId> {
    let mut seen = HashSet::new();
    targets
        .iter()
        .copied()
        .filter(|target| seen.insert(*target))
        .collect()
}

fn guard_term_for_target(
    temp_name: &ResolvedName,
    function_id: RuntimeFunctionId,
    meta: Meta,
    then_label: BlockLabel,
    else_label: BlockLabel,
) -> BlockTerm<InstrCodegen> {
    BlockTerm::IfTerm(TermIf {
        test: InstrCodegen::DirectFunctionIdGuardTest(
            DirectFunctionIdGuardTest::new(load_temp(temp_name), function_id).with_meta(meta),
        ),
        then_label,
        else_label,
    })
}

fn load_temp(temp_name: &ResolvedName) -> InstrCodegen {
    InstrCodegen::Load(Load::new(temp_name.clone()).with_meta(Meta::synthetic()))
}

fn load_temp_args(temp_names: &[ResolvedName]) -> Vec<CallArgPositional<InstrCodegen>> {
    temp_names
        .iter()
        .map(|name| CallArgPositional::Positional(load_temp(name)))
        .collect()
}

fn append_cleanup_dels_to_body(body: &mut Vec<InstrCodegen>, temp_names: &[ResolvedName]) {
    for temp_name in temp_names.iter().rev() {
        append_cleanup_del_to_body(body, temp_name);
    }
}

fn append_cleanup_del_to_body(body: &mut Vec<InstrCodegen>, temp_name: &ResolvedName) {
    body.push(
        Del::new(temp_name.clone(), false)
            .with_meta(Meta::synthetic())
            .into(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::{BlockPyModule, ChildVisitable, Visit};
    use crate::lower_python_to_blockpy_for_testing;
    use crate::passes::validate_codegen_instr_ids;

    fn lowered_module(source: &str) -> BlockPyModule<CodegenModuleShape> {
        lower_python_to_blockpy_for_testing(source)
            .expect("test source should lower")
            .codegen_module
    }

    fn function_index_by_qualname(
        module: &BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> usize {
        module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing lowered function {qualname}"))
    }

    fn callee_map(
        module: &BlockPyModule<CodegenModuleShape>,
    ) -> HashMap<RuntimeFunctionId, BlockPyFunction<CodegenModuleShape>> {
        module
            .callable_defs
            .iter()
            .map(|function| (function.function_id, function.clone()))
            .collect()
    }

    #[derive(Default)]
    struct CallInstrCollector {
        calls: Vec<InstrId>,
    }

    impl Visit<InstrCodegen> for CallInstrCollector {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if matches!(expr, InstrCodegen::Call(_)) {
                self.calls.push(expr.semantic_instr_id());
            }
            expr.visit_children(self);
        }
    }

    #[test]
    fn rewrites_profiled_store_call_into_guarded_direct_call_blocks() {
        let mut module = lowered_module(
            "def callee(x):\n    return x\n\n\
def caller(fn, x):\n    y = fn(x)\n    return y\n",
        );
        let callee_id =
            module.callable_defs[function_index_by_qualname(&module, "callee")].function_id;
        let caller_index = function_index_by_qualname(&module, "caller");
        let mut collector = CallInstrCollector::default();
        collector.visit_fn(&module.callable_defs[caller_index]);
        let call_instr_id = collector
            .calls
            .first()
            .copied()
            .expect("caller should have a generic call");

        let callees = callee_map(&module);
        let stats = rewrite_profiled_function_call_store_sites(
            &mut module.callable_defs[caller_index],
            &HashMap::from([(call_instr_id, vec![callee_id])]),
            &callees,
        );

        assert_eq!(
            stats,
            DirectCallStoreRewriteStats {
                rewritten_stores: 1,
                skipped_empty_targets: 0,
                skipped_incompatible_targets: 0,
                skipped_missing_callee_targets: 0,
                skipped_arity_mismatch_targets: 0,
                skipped_unsupported_init_targets: 0,
                skipped_missing_storage_layout_targets: 0,
                skipped_unsupported_param_kind_targets: 0,
                skipped_missing_param_storage_targets: 0,
            }
        );
        let caller = &module.callable_defs[caller_index];
        assert!(
            caller.blocks.iter().any(|block| {
                matches!(
                    block.term,
                    BlockTerm::IfTerm(TermIf {
                        test: InstrCodegen::DirectFunctionIdGuardTest(_),
                        ..
                    })
                )
            }),
            "rewrite should represent the profiled call guard in BlockPy"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| &block.body)
                .any(|instr| matches!(instr, InstrCodegen::Store(store) if matches!(store.value.as_ref(), InstrCodegen::CallDirect(_)))),
            "hot arm should call the profiled target directly"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| &block.body)
                .any(|instr| matches!(instr, InstrCodegen::Store(store) if matches!(store.value.as_ref(), InstrCodegen::Call(_)))),
            "fallback arm should retain the generic call"
        );
        validate_codegen_instr_ids(&module).expect("rewrite should leave unique instruction ids");
    }

    #[test]
    fn rewrites_profiled_store_call_with_trailing_defaults() {
        let mut module = lowered_module(
            "def callee(x, y=1):\n    return x + y\n\n\
def caller(fn, x):\n    y = fn(x)\n    return y\n",
        );
        let callee_id =
            module.callable_defs[function_index_by_qualname(&module, "callee")].function_id;
        let caller_index = function_index_by_qualname(&module, "caller");
        let mut collector = CallInstrCollector::default();
        collector.visit_fn(&module.callable_defs[caller_index]);
        let call_instr_id = collector
            .calls
            .first()
            .copied()
            .expect("caller should have a generic call");

        let callees = callee_map(&module);
        let stats = rewrite_profiled_function_call_store_sites(
            &mut module.callable_defs[caller_index],
            &HashMap::from([(call_instr_id, vec![callee_id])]),
            &callees,
        );

        assert_eq!(stats.rewritten_stores, 1);
        assert_eq!(stats.skipped_arity_mismatch_targets, 0);
        assert!(
            module.callable_defs[caller_index]
                .blocks
                .iter()
                .flat_map(|block| &block.body)
                .any(|instr| matches!(instr, InstrCodegen::Store(store) if matches!(store.value.as_ref(), InstrCodegen::CallDirect(_)))),
            "hot arm should direct-call with default sentinels supplied by JIT lowering"
        );
        validate_codegen_instr_ids(&module).expect("rewrite should leave unique instruction ids");
    }

    #[test]
    fn leaves_constructor_targets_for_constructor_specialization() {
        let mut module = lowered_module(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(cls, x):\n    y = cls(x)\n    return y\n",
        );
        let init_id =
            module.callable_defs[function_index_by_qualname(&module, "Box.__init__")].function_id;
        let caller_index = function_index_by_qualname(&module, "caller");
        let mut collector = CallInstrCollector::default();
        collector.visit_fn(&module.callable_defs[caller_index]);
        let call_instr_id = collector
            .calls
            .first()
            .copied()
            .expect("caller should have a generic constructor call");

        let callees = callee_map(&module);
        let stats = rewrite_profiled_function_call_store_sites(
            &mut module.callable_defs[caller_index],
            &HashMap::from([(call_instr_id, vec![init_id])]),
            &callees,
        );

        assert_eq!(stats.rewritten_stores, 0);
        assert_eq!(stats.skipped_unsupported_init_targets, 1);
        assert!(
            module.callable_defs[caller_index]
                .blocks
                .iter()
                .flat_map(|block| &block.body)
                .any(|instr| matches!(instr, InstrCodegen::Store(store) if matches!(store.value.as_ref(), InstrCodegen::Call(_)))),
            "ordinary direct-call rewrite should leave constructor calls for typed specialization"
        );
        validate_codegen_instr_ids(&module).expect("unchanged module should still validate");
    }

    #[test]
    fn optionally_rewrites_constructor_targets_for_inline_fragments() {
        let mut module = lowered_module(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(cls, x):\n    y = cls(x)\n    return y\n",
        );
        let init_id =
            module.callable_defs[function_index_by_qualname(&module, "Box.__init__")].function_id;
        let caller_index = function_index_by_qualname(&module, "caller");
        let mut collector = CallInstrCollector::default();
        collector.visit_fn(&module.callable_defs[caller_index]);
        let call_instr_id = collector
            .calls
            .first()
            .copied()
            .expect("caller should have a generic constructor call");

        let callees = callee_map(&module);
        let stats = rewrite_profiled_function_call_store_sites_with_constructor_targets(
            &mut module.callable_defs[caller_index],
            &HashMap::from([(call_instr_id, vec![init_id])]),
            &callees,
            true,
        );

        assert_eq!(stats.rewritten_stores, 1);
        assert_eq!(stats.skipped_unsupported_init_targets, 0);
        assert!(
            module.callable_defs[caller_index]
                .blocks
                .iter()
                .flat_map(|block| &block.body)
                .any(|instr| matches!(instr, InstrCodegen::Store(store) if matches!(store.value.as_ref(), InstrCodegen::CallDirect(call) if call.function_id == init_id))),
            "hot arm should leave constructor calls as CallDirect(__init__) for constructor specialization"
        );
        validate_codegen_instr_ids(&module).expect("rewrite should leave unique instruction ids");
    }

    #[test]
    fn leaves_return_constructor_targets_for_constructor_specialization() {
        let mut module = lowered_module(
            "class Record:\n    def __init__(self, PtrComp=None, Discr=0, EnumComp=0, IntComp=0, StringComp=0):\n        self.PtrComp = PtrComp\n        self.Discr = Discr\n        self.EnumComp = EnumComp\n        self.IntComp = IntComp\n        self.StringComp = StringComp\n    def copy(self):\n        return Record(self.PtrComp, self.Discr, self.EnumComp, self.IntComp, self.StringComp)\n",
        );
        let init_id = module.callable_defs[function_index_by_qualname(&module, "Record.__init__")]
            .function_id;
        let copy_index = function_index_by_qualname(&module, "Record.copy");
        let mut collector = CallInstrCollector::default();
        collector.visit_fn(&module.callable_defs[copy_index]);
        let call_instr_id = collector
            .calls
            .first()
            .copied()
            .expect("copy should have a generic constructor return call");

        let callees = callee_map(&module);
        let stats = rewrite_profiled_function_call_store_sites_with_constructor_targets(
            &mut module.callable_defs[copy_index],
            &HashMap::from([(call_instr_id, vec![init_id])]),
            &callees,
            true,
        );

        assert_eq!(stats.rewritten_stores, 0);
        assert!(
            module.callable_defs[copy_index]
                .blocks
                .iter()
                .any(|block| matches!(block.term, BlockTerm::Return(InstrCodegen::Call(_)))),
            "return-call sites should stay generic until allocation materialization across inline continuations is handled"
        );
        validate_codegen_instr_ids(&module).expect("unchanged module should still validate");
    }

    #[test]
    fn presequence_args_before_profiled_call_guard() {
        let mut module = lowered_module(
            "def callee(x):\n    return x\n\n\
def other(x):\n    return x\n\n\
def caller(fn, x):\n    y = fn(other(x))\n    return y\n",
        );
        let callee_id =
            module.callable_defs[function_index_by_qualname(&module, "callee")].function_id;
        let caller_index = function_index_by_qualname(&module, "caller");
        let mut collector = CallInstrCollector::default();
        collector.visit_fn(&module.callable_defs[caller_index]);
        let outer_call_instr_id = collector
            .calls
            .first()
            .copied()
            .expect("caller should have an outer generic call");

        let callees = callee_map(&module);
        let stats = rewrite_profiled_function_call_store_sites(
            &mut module.callable_defs[caller_index],
            &HashMap::from([(outer_call_instr_id, vec![callee_id])]),
            &callees,
        );

        assert_eq!(stats.rewritten_stores, 1);
        let caller = &module.callable_defs[caller_index];
        assert!(
            caller.blocks.iter().any(|block| {
                matches!(block.term, BlockTerm::IfTerm(_))
                    && block.body.iter().any(
                        |instr| matches!(instr, InstrCodegen::Store(store) if matches!(store.value.as_ref(), InstrCodegen::Call(_))),
                    )
            }),
            "effectful argument call should be presequenced before the guard"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| &block.body)
                .any(|instr| matches!(instr, InstrCodegen::Store(store) if matches!(store.value.as_ref(), InstrCodegen::CallDirect(_)))),
            "hot arm should still call the profiled target directly"
        );
        validate_codegen_instr_ids(&module).expect("rewrite should leave unique instruction ids");
    }

    #[test]
    fn leaves_incompatible_profiled_targets_for_typed_specialization() {
        let mut module = lowered_module(
            "class C:\n    def method(self, x):\n        return x\n\n\
def caller(obj, x):\n    y = obj.method(x)\n    return y\n",
        );
        let method_id =
            module.callable_defs[function_index_by_qualname(&module, "C.method")].function_id;
        let caller_index = function_index_by_qualname(&module, "caller");
        let mut collector = CallInstrCollector::default();
        collector.visit_fn(&module.callable_defs[caller_index]);
        let call_instr_id = collector
            .calls
            .first()
            .copied()
            .expect("caller should have a generic method call");

        let callees = callee_map(&module);
        let stats = rewrite_profiled_function_call_store_sites(
            &mut module.callable_defs[caller_index],
            &HashMap::from([(call_instr_id, vec![method_id])]),
            &callees,
        );

        assert_eq!(
            stats,
            DirectCallStoreRewriteStats {
                rewritten_stores: 0,
                skipped_empty_targets: 1,
                skipped_incompatible_targets: 1,
                skipped_missing_callee_targets: 0,
                skipped_arity_mismatch_targets: 1,
                skipped_unsupported_init_targets: 0,
                skipped_missing_storage_layout_targets: 0,
                skipped_unsupported_param_kind_targets: 0,
                skipped_missing_param_storage_targets: 0,
            }
        );
        assert!(
            module.callable_defs[caller_index]
                .blocks
                .iter()
                .flat_map(|block| &block.body)
                .any(|instr| matches!(instr, InstrCodegen::Store(store) if matches!(store.value.as_ref(), InstrCodegen::Call(_)))),
            "method-shaped targets should remain generic for the typed method specialization path"
        );
        validate_codegen_instr_ids(&module).expect("unchanged module should still validate");
    }
}
