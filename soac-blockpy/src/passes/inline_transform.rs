use crate::block_py::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm,
    CallArgPositional, CallDirect, FunctionId, HasMeta, InstrCodegen, LocalLocation, MapInstr,
    Mappable, NameLocation, ParamKind, ResolvedName, Store, TryMapInstr, WithMeta,
};
use crate::passes::{
    reassign_codegen_function_instr_ids, CodegenModuleShape, InlinePlanModule, InstrCodegenOp,
};
use std::collections::HashMap;

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
    MultipleBlocks { count: usize },
    BlockParams,
    ExceptionEdge,
    NonReturnTerm,
}

pub fn inline_simple_direct_call_stores(
    module: &mut BlockPyModule<CodegenModuleShape>,
    inline_plan: &InlinePlanModule,
) -> InlineRewriteStats {
    let callees = module
        .callable_defs
        .iter()
        .map(|function| (function.function_id, function.clone()))
        .collect::<HashMap<_, _>>();
    let mut stats = InlineRewriteStats::default();
    for function in &mut module.callable_defs {
        if inline_simple_direct_call_stores_in_function(function, inline_plan, &callees, &mut stats)
        {
            reassign_codegen_function_instr_ids(function);
        }
    }
    stats
}

fn inline_simple_direct_call_stores_in_function(
    function: &mut BlockPyFunction<CodegenModuleShape>,
    inline_plan: &InlinePlanModule,
    callees: &HashMap<FunctionId, BlockPyFunction<CodegenModuleShape>>,
    stats: &mut InlineRewriteStats,
) -> bool {
    let mut changed = false;
    let original_blocks = std::mem::take(&mut function.blocks);
    let mut rewritten_blocks = Vec::with_capacity(original_blocks.len());
    for block in original_blocks {
        match build_direct_store_rewrite(function, block, inline_plan, callees, stats) {
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
    block: Block<InstrCodegen>,
    inline_plan: &InlinePlanModule,
    callees: &HashMap<FunctionId, BlockPyFunction<CodegenModuleShape>>,
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
    let Ok(bindings) = bind_simple_direct_call_inline_args(callee, &candidate.call) else {
        stats.skipped_candidates += 1;
        return InlineBlockRewrite::Unchanged(block);
    };

    let continuation = caller.name_gen.next_block_name();
    let Ok(mut fragment) = build_single_block_inline_fragment_to_target(
        caller,
        callee,
        continuation,
        &bindings,
        candidate.target,
    ) else {
        stats.skipped_candidates += 1;
        return InlineBlockRewrite::Unchanged(block);
    };

    let exc_edge = block.exc_edge.clone();
    for fragment_block in &mut fragment.blocks {
        fragment_block.exc_edge = exc_edge.clone();
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

enum InlineBlockRewrite {
    Rewritten(Vec<Block<InstrCodegen>>),
    Unchanged(Block<InstrCodegen>),
}

struct InlineStoreCandidate {
    instr_index: usize,
    callee_id: FunctionId,
    call: CallDirect<InstrCodegen>,
    target: ResolvedName,
}

fn find_inline_store_candidate(
    block: &Block<InstrCodegen>,
    caller_id: FunctionId,
    inline_plan: &InlinePlanModule,
    callees: &HashMap<FunctionId, BlockPyFunction<CodegenModuleShape>>,
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
            inline_plan.straightline_constructor(call.function_id)?;
            callees.get(&call.function_id)?;
            Some(InlineStoreCandidate {
                instr_index,
                callee_id: call.function_id,
                call: call.clone(),
                target: store.name.clone(),
            })
        })
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
    let actual = call.args.len();
    if expected != actual {
        return Err(InlineUnsupportedReason::ArityMismatch { expected, actual });
    }

    let mut bindings = InlineValueBindings::new();
    for (param, arg) in supported_params.into_iter().zip(&call.args) {
        let location = parameter_local_location(callee, &param.name)?;
        let CallArgPositional::Positional(value) = arg else {
            unreachable!("starred arguments were rejected before binding");
        };
        bindings.insert(location, value.clone());
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
    build_single_block_inline_fragment_impl(
        caller,
        callee,
        continuation,
        value_bindings,
        InlineReturnPlacement::FreshContinuationArg,
    )
}

pub fn build_single_block_inline_fragment_to_target(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &InlineValueBindings,
    return_target: ResolvedName,
) -> Result<InlineFragment, InlineUnsupportedReason> {
    build_single_block_inline_fragment_impl(
        caller,
        callee,
        continuation,
        value_bindings,
        InlineReturnPlacement::StoreTo(return_target),
    )
}

fn build_single_block_inline_fragment_impl(
    caller: &mut BlockPyFunction<CodegenModuleShape>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    continuation: BlockLabel,
    value_bindings: &InlineValueBindings,
    return_placement: InlineReturnPlacement,
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

    let mut remapper = InlineLocalRemapper {
        locals: &locals,
        value_bindings,
    };
    let mut body = callee_block
        .body
        .iter()
        .cloned()
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
    let name = caller.name_gen.next_tmp_name("inline").as_str().to_string();
    let layout = caller
        .storage_layout
        .as_mut()
        .ok_or(InlineUnsupportedReason::MissingCallerStorageLayout)?;
    let location = LocalLocation(
        u32::try_from(layout.stack_slots().len())
            .expect("caller stack slot index should fit in u32"),
    );
    layout.ensure_stack_slot(name.clone());
    Ok(InlineLocal { name, location })
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

struct InlineLocalRemapper<'a> {
    locals: &'a HashMap<LocalLocation, InlineLocal>,
    value_bindings: &'a InlineValueBindings,
}

impl TryMapInstr<InstrCodegen, InstrCodegen, InlineUnsupportedReason> for InlineLocalRemapper<'_> {
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
    use crate::block_py::{BlockParam, BlockParamRole, CallDirect, InstrId, Load};
    use crate::lower_python_to_blockpy_for_testing;
    use crate::passes::{
        plan_module_inlining, summarize_module_escapes, validate_codegen_instr_ids,
    };

    fn function_by_qualname<'a>(
        module: &'a crate::block_py::BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> &'a BlockPyFunction<CodegenModuleShape> {
        module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("{qualname} should be present"))
    }

    fn function_index_by_qualname(
        module: &crate::block_py::BlockPyModule<CodegenModuleShape>,
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

    fn runtime_load(name: &str) -> InstrCodegen {
        Load::new(ResolvedName {
            id: name.to_string().into(),
            location: NameLocation::RuntimeName,
        })
        .into()
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
        assert!(fragment.blocks[0]
            .body
            .iter()
            .all(|instr| instr.meta().instr_id.is_none()));
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
            assert!(caller
                .storage_layout
                .as_ref()
                .unwrap()
                .stack_slots()
                .contains(&fresh.name));
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
                    runtime_load("Box.__init__"),
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
        assert!(fragment
            .body
            .iter()
            .any(|instr| matches!(instr, InstrCodegen::SetAttr(_))));
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
    fn leaves_unplanned_direct_call_store_unchanged() {
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
        let original_block_count = module.callable_defs[make_index].blocks.len();
        let out_target = local_resolved_name(&module.callable_defs[make_index], "out");
        let x_arg = local_load(&module.callable_defs[make_index], "x");
        module.callable_defs[make_index].blocks[0]
            .body
            .push(InstrCodegen::Store(Store::new(
                out_target,
                InstrCodegen::CallDirect(CallDirect::new(
                    runtime_load("callee"),
                    callee_id,
                    vec![CallArgPositional::Positional(x_arg)],
                    Vec::new(),
                )),
            )));

        let stats = inline_simple_direct_call_stores(&mut module, &inline_plan);

        assert_eq!(stats.rewritten_stores, 0);
        assert_eq!(
            module.callable_defs[make_index].blocks.len(),
            original_block_count
        );
    }
}
