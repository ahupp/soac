use crate::passes::{BlockPyModuleShape, InstrBlockPy, try_allocate_codegen_stack_temp};
use soac_core::block_py::{
    Block, BlockArg, BlockContext, BlockEdge, BlockLabel, BlockParam, BlockParamRole,
    BlockPyFunction, BlockTerm, CallArgPositional, CallDirect, ConstantExpr,
    HandledExceptionContext, HasBlockContext, HasMeta, Instr, LocalLocation, MapInstr, Mappable,
    ModuleShape, NameLocation, ParamKind, ResolvedName, RuntimeName, Store, TryMapInstr,
    TryMapTerm, WithMeta,
};
use std::collections::HashMap;

/// A necessary activation-safety precondition for copying a callee's body.
///
/// A terminal handled-state operation belongs to the callee's own activation.
/// Copying it into an ordinary caller would retire the wrong exception item.
/// This predicate does not establish caller-region compatibility,
/// authentication, supported operation shapes, or an inline size budget.
pub fn inline_callee_preserves_activation(callee: &BlockPyFunction<impl ModuleShape>) -> bool {
    !callee.blocks.iter().any(|block| {
        block.extra.block_context().handled_exception == HandledExceptionContext::Terminal
    })
}

/// The call site's active handler prefix, not a new Python activation.
///
/// Ordinary callees share the current native handled item. A suspended resume
/// has a different capsule-owned item and cannot be represented by appending
/// lexical regions to this one activation.
#[derive(Debug, Clone)]
pub(crate) struct InlineHandledContext {
    context: BlockContext,
    regions: Vec<BlockParam>,
    pending_payloads: Vec<BlockParam>,
}

impl InlineHandledContext {
    pub(crate) fn for_call_site<I: Instr, E: HasBlockContext>(block: &Block<I, E>) -> Self {
        Self {
            context: block.extra.block_context(),
            regions: block.handled_exception_params().cloned().collect(),
            pending_payloads: block.pending_abrupt_payload_params().cloned().collect(),
        }
    }

    pub(crate) fn can_inline(&self, callee: &BlockPyFunction<impl ModuleShape>) -> bool {
        if !inline_callee_preserves_activation(callee) {
            return false;
        }
        self.context.handled_exception == HandledExceptionContext::Regions
            || callee
                .blocks
                .iter()
                .all(|block| block.handled_exception_params().next().is_none())
    }

    pub(crate) fn preserve_caller<I: Instr, E: HasBlockContext>(&self, block: &mut Block<I, E>) {
        block.extra.set_block_context(self.context);
        // These new blocks are in the original caller's lexical region. Keep
        // its roles, not a second copy of that region nested inside itself.
        block.params.extend(self.regions.iter().rev().cloned());
        block
            .params
            .extend(self.pending_payloads.iter().rev().map(|param| BlockParam {
                name: param.name.clone(),
                role: BlockParamRole::EnclosingAbruptPayload,
            }));
    }

    pub(crate) fn compose_callee<I: Instr, E: HasBlockContext>(&self, block: &mut Block<I, E>) {
        if self.context.handled_exception != HandledExceptionContext::Regions {
            // A no-handler ordinary callee inherits the current dynamic item.
            // Terminal applies to the original caller's cleanup/raise, never
            // to a source raise inside its inlined ordinary callee.
            block.extra.set_block_context(BlockContext {
                handled_exception: HandledExceptionContext::Preserve,
                ..Default::default()
            });
        }
        // Existing explicit edge arguments address the callee parameter
        // prefix. Appended caller regions use same-name edge transport. The
        // region iterator reverses EnclosingException parameters, so append
        // the caller's inner-to-outer prefix after the callee's own regions.
        block
            .params
            .extend(self.regions.iter().rev().map(|param| BlockParam {
                name: param.name.clone(),
                role: BlockParamRole::EnclosingException,
            }));
        block
            .params
            .extend(self.pending_payloads.iter().rev().map(|param| BlockParam {
                name: param.name.clone(),
                role: BlockParamRole::EnclosingAbruptPayload,
            }));
    }
}

#[derive(Debug, Clone)]
pub struct InlineFragment {
    pub entry_label: BlockLabel,
    pub blocks: Vec<Block<InstrBlockPy>>,
    pub locals: HashMap<LocalLocation, InlineLocal>,
    pub return_local: Option<InlineLocal>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct InlineLocal {
    pub name: String,
    pub location: LocalLocation,
}

pub type InlineValueBindings = HashMap<LocalLocation, InstrBlockPy>;

const MAX_INLINE_DIRECT_CALL_BLOCKS: usize = 16;

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
    UnsupportedCallTarget,
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
    ClassConstructionExecutionContext,
    FunctionDefinitionExecutionContext,
    UnknownBlockName(String),
}

pub fn bind_simple_direct_call_inline_args(
    callee: &BlockPyFunction<BlockPyModuleShape>,
    call: &CallDirect<InstrBlockPy>,
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
    callee: &BlockPyFunction<BlockPyModuleShape>,
    receiver: InstrBlockPy,
    args: &[CallArgPositional<InstrBlockPy>],
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
    callee: &BlockPyFunction<BlockPyModuleShape>,
    values: Vec<InstrBlockPy>,
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
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
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
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
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
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
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
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
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
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    caller_constants: &mut Vec<ConstantExpr>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
    callee_constants: &[ConstantExpr],
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
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
    continuation: BlockLabel,
    receiver: InstrBlockPy,
    args: &[CallArgPositional<InstrBlockPy>],
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
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    caller_constants: &mut Vec<ConstantExpr>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
    callee_constants: &[ConstantExpr],
    continuation: BlockLabel,
    receiver: InstrBlockPy,
    args: &[CallArgPositional<InstrBlockPy>],
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
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
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
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
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
    record_inline_block_parameter_roles(caller, callee_layout, &locals);

    let label_map = callee
        .blocks
        .iter()
        .map(|block| (block.label, caller.name_gen.next_block_name()))
        .collect::<HashMap<_, _>>();
    let entry_label = remapped_label(&label_map, callee.blocks[0].label)?;
    let mut remapper =
        InlineLocalRemapper::new(callee_layout, &locals, value_bindings, &mut constant_scope);
    let mut blocks = Vec::with_capacity(callee.blocks.len());
    for callee_block in &callee.blocks {
        let label = remapped_label(&label_map, callee_block.label)?;
        let mut body = callee_block
            .body
            .iter()
            .cloned()
            // Inlined profiling counters belong to the callee's counter
            // layout; the caller does not have storage for those ids.
            .filter(|instr| !matches!(instr, InstrBlockPy::IncrementCounter(_)))
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
            term => remap_inline_term_labels(
                remapper.try_map_term(term.clone())?,
                &label_map,
                &mut remapper,
            )?,
        };
        let params = callee_block
            .params
            .iter()
            .cloned()
            .map(|param| remapper.try_map_block_param(param))
            .collect::<Result<Vec<_>, _>>()?;
        let exc_edge = callee_block
            .exc_edge
            .clone()
            .map(|edge| remap_inline_edge(edge, &label_map, &mut remapper))
            .transpose()?;
        blocks.push(Block::new(label, body, term, params, exc_edge));
    }

    Ok(InlineFragment {
        entry_label,
        blocks,
        locals,
        return_local: None,
    })
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
    term: BlockTerm<InstrBlockPy>,
    label_map: &HashMap<BlockLabel, BlockLabel>,
    remapper: &mut InlineLocalRemapper<'_, '_, '_, '_>,
) -> Result<BlockTerm<InstrBlockPy>, InlineUnsupportedReason> {
    Ok(match term {
        BlockTerm::Jump(edge) => BlockTerm::Jump(remap_inline_edge(edge, label_map, remapper)?),
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
        BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {
            return Err(InlineUnsupportedReason::NonReturnTerm);
        }
    })
}

fn remap_inline_edge(
    mut edge: BlockEdge,
    label_map: &HashMap<BlockLabel, BlockLabel>,
    remapper: &InlineLocalRemapper<'_, '_, '_, '_>,
) -> Result<BlockEdge, InlineUnsupportedReason> {
    edge.target = remapped_label(label_map, edge.target)?;
    edge.args = edge
        .args
        .into_iter()
        .map(|arg| remapper.try_map_block_arg(arg))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(edge)
}

fn build_single_block_inline_fragment_with_constant_scope(
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    callee: &BlockPyFunction<BlockPyModuleShape>,
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
    record_inline_block_parameter_roles(caller, callee_layout, &locals);
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

    let mut remapper =
        InlineLocalRemapper::new(callee_layout, &locals, value_bindings, &mut constant_scope);
    let mut body = callee_block
        .body
        .iter()
        .cloned()
        // Inlined profiling counters belong to the callee's counter layout;
        // the caller does not have storage for those ids.
        .filter(|instr| !matches!(instr, InstrBlockPy::IncrementCounter(_)))
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

fn record_inline_block_parameter_roles(
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
    callee_layout: &soac_core::block_py::StorageLayout,
    locals: &HashMap<LocalLocation, InlineLocal>,
) {
    let layout = caller
        .storage_layout
        .as_mut()
        .expect("inline local allocation requires a caller layout");
    for binding in &callee_layout.block_parameter_roles {
        if let NameLocation::Local(source) = binding.location {
            if let Some(target) = locals.get(&source) {
                layout.record_block_parameter_role(
                    NameLocation::Local(target.location),
                    binding.role,
                );
            }
        }
    }
}

fn allocate_inline_local(
    caller: &mut BlockPyFunction<BlockPyModuleShape>,
) -> Result<InlineLocal, InlineUnsupportedReason> {
    let temp = try_allocate_codegen_stack_temp(caller, "inline")
        .map_err(|_| InlineUnsupportedReason::MissingCallerStorageLayout)?;
    Ok(InlineLocal {
        name: temp.name,
        location: temp.location,
    })
}

fn parameter_local_location(
    function: &BlockPyFunction<BlockPyModuleShape>,
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

fn inline_value_binding_name(
    callee_location: LocalLocation,
    value: &InstrBlockPy,
) -> Result<&ResolvedName, InlineUnsupportedReason> {
    let InstrBlockPy::Load(load) = value else {
        return Err(InlineUnsupportedReason::RebindsBoundLocal(callee_location));
    };
    Ok(&load.name)
}

struct InlineLocalRemapper<'locals, 'bindings, 'scope, 'constants> {
    callee_layout: &'locals soac_core::block_py::StorageLayout,
    locals: &'locals HashMap<LocalLocation, InlineLocal>,
    value_bindings: &'bindings InlineValueBindings,
    constant_scope: &'scope mut InlineConstantScope<'constants>,
}

impl<'locals, 'bindings, 'scope, 'constants>
    InlineLocalRemapper<'locals, 'bindings, 'scope, 'constants>
{
    fn new(
        callee_layout: &'locals soac_core::block_py::StorageLayout,
        locals: &'locals HashMap<LocalLocation, InlineLocal>,
        value_bindings: &'bindings InlineValueBindings,
        constant_scope: &'scope mut InlineConstantScope<'constants>,
    ) -> Self {
        Self {
            callee_layout,
            locals,
            value_bindings,
            constant_scope,
        }
    }

    fn callee_local_location_by_name(&self, name: &str) -> Option<LocalLocation> {
        self.callee_layout
            .stack_slots()
            .iter()
            .position(|slot_name| slot_name == name)
            .map(|slot| {
                LocalLocation(
                    u32::try_from(slot).expect("callee stack slot index should fit in u32"),
                )
            })
    }

    fn try_map_block_local_name(&self, name: String) -> Result<String, InlineUnsupportedReason> {
        let Some(location) = self.callee_local_location_by_name(name.as_str()) else {
            return Err(InlineUnsupportedReason::UnknownBlockName(name));
        };
        if let Some(value) = self.value_bindings.get(&location) {
            let bound_name = inline_value_binding_name(location, value)?;
            if bound_name.local_location().is_none() {
                return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
            }
            return Ok(bound_name.id.as_str().to_string());
        }
        let Some(fresh) = self.locals.get(&location) else {
            return Err(InlineUnsupportedReason::MissingCalleeLocal(location));
        };
        Ok(fresh.name.clone())
    }

    fn try_map_block_param(
        &self,
        mut param: BlockParam,
    ) -> Result<BlockParam, InlineUnsupportedReason> {
        let Some(location) = self.callee_local_location_by_name(param.name.as_str()) else {
            return Err(InlineUnsupportedReason::UnknownBlockName(param.name));
        };
        if self.value_bindings.contains_key(&location) {
            return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
        }
        let Some(fresh) = self.locals.get(&location) else {
            return Err(InlineUnsupportedReason::MissingCalleeLocal(location));
        };
        param.name = fresh.name.clone();
        Ok(param)
    }

    fn try_map_block_arg(&self, arg: BlockArg) -> Result<BlockArg, InlineUnsupportedReason> {
        Ok(match arg {
            BlockArg::Name(name) => BlockArg::Name(self.try_map_block_local_name(name)?),
            BlockArg::None => BlockArg::None,
            BlockArg::CurrentException => BlockArg::CurrentException,
            BlockArg::AbruptKind(kind) => BlockArg::AbruptKind(kind),
        })
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
    caller_constants: &'a mut Vec<ConstantExpr>,
    callee_constants: &'a [ConstantExpr],
    mapped_indices: HashMap<u32, u32>,
}

impl<'a> InlineConstantRemapper<'a> {
    fn new(
        caller_constants: &'a mut Vec<ConstantExpr>,
        callee_constants: &'a [ConstantExpr],
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

impl TryMapInstr<InstrBlockPy, InstrBlockPy, InlineUnsupportedReason>
    for InlineLocalRemapper<'_, '_, '_, '_>
{
    fn try_map_instr(
        &mut self,
        instr: InstrBlockPy,
    ) -> Result<InstrBlockPy, InlineUnsupportedReason> {
        let mapped = match instr {
            InstrBlockPy::BinOp(op) => InstrBlockPy::BinOp(op.try_map_children(self)?),
            InstrBlockPy::UnaryOp(op) => InstrBlockPy::UnaryOp(op.try_map_children(self)?),
            InstrBlockPy::Tuple(op) => InstrBlockPy::Tuple(op.try_map_children(self)?),
            InstrBlockPy::Call(op) => {
                if op.frame_namespace.is_some() {
                    return Err(InlineUnsupportedReason::ClassConstructionExecutionContext);
                }
                InstrBlockPy::Call(op.try_map_children(self)?)
            }
            InstrBlockPy::GetAttr(op) => InstrBlockPy::GetAttr(op.try_map_children(self)?),
            InstrBlockPy::SetAttr(op) => InstrBlockPy::SetAttr(op.try_map_children(self)?),
            InstrBlockPy::GetItem(op) => InstrBlockPy::GetItem(op.try_map_children(self)?),
            InstrBlockPy::SetItem(op) => InstrBlockPy::SetItem(op.try_map_children(self)?),
            InstrBlockPy::DelItem(op) => InstrBlockPy::DelItem(op.try_map_children(self)?),
            InstrBlockPy::Load(op) => {
                if let Some(location) = op.name.local_location() {
                    if let Some(value) = self.value_bindings.get(&location) {
                        return Ok(clear_blockpy_instr_ids(value.clone()));
                    }
                }
                InstrBlockPy::Load(op.try_map_children(self)?)
            }
            InstrBlockPy::Store(op) => {
                if let Some(location) = op.name.local_location() {
                    if self.value_bindings.contains_key(&location) {
                        return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
                    }
                }
                InstrBlockPy::Store(op.try_map_children(self)?)
            }
            InstrBlockPy::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    if self.value_bindings.contains_key(&location) {
                        return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
                    }
                }
                InstrBlockPy::Del(op.try_map_children(self)?)
            }
            InstrBlockPy::MakeCell(op) => InstrBlockPy::MakeCell(op.try_map_children(self)?),
            InstrBlockPy::NewAnnotationSet(op) => {
                InstrBlockPy::NewAnnotationSet(op.try_map_children(self)?)
            }
            InstrBlockPy::SetupAnnotations(op) => {
                InstrBlockPy::SetupAnnotations(op.try_map_children(self)?)
            }
            InstrBlockPy::ConstructTypeParameterScope(op) => {
                InstrBlockPy::ConstructTypeParameterScope(op.try_map_children(self)?)
            }
            InstrBlockPy::SubscriptGeneric(op) => {
                InstrBlockPy::SubscriptGeneric(op.try_map_children(self)?)
            }
            InstrBlockPy::SetFunctionTypeParameters(op) => {
                InstrBlockPy::SetFunctionTypeParameters(op.try_map_children(self)?)
            }
            InstrBlockPy::CreateTypeAlias(op) => {
                InstrBlockPy::CreateTypeAlias(op.try_map_children(self)?)
            }
            InstrBlockPy::CreateTypeParameter(op) => {
                InstrBlockPy::CreateTypeParameter(op.try_map_children(self)?)
            }
            InstrBlockPy::SetTypeParameterDefault(op) => {
                InstrBlockPy::SetTypeParameterDefault(op.try_map_children(self)?)
            }
            InstrBlockPy::CheckAnnotationFormat(op) => {
                InstrBlockPy::CheckAnnotationFormat(op.try_map_children(self)?)
            }
            InstrBlockPy::RecordAnnotation(op) => {
                InstrBlockPy::RecordAnnotation(op.try_map_children(self)?)
            }
            InstrBlockPy::IncrementCounter(op) => InstrBlockPy::IncrementCounter(op),
            InstrBlockPy::CellRef(op) => InstrBlockPy::CellRef(op),
            InstrBlockPy::MakeFunctionWithClosure(op) => {
                InstrBlockPy::MakeFunctionWithClosure(op.try_map_children(self)?)
            }
            InstrBlockPy::TakeOperand(op) => InstrBlockPy::TakeOperand(op.try_map_children(self)?),
            InstrBlockPy::ComprehensionInsert(op) => {
                InstrBlockPy::ComprehensionInsert(op.try_map_children(self)?)
            }
            InstrBlockPy::BuildCollection(op) => {
                InstrBlockPy::BuildCollection(op.try_map_children(self)?)
            }
            InstrBlockPy::CallArgumentOp(op) => {
                InstrBlockPy::CallArgumentOp(op.try_map_children(self)?)
            }
            InstrBlockPy::PreparedCall(op) => {
                InstrBlockPy::PreparedCall(op.try_map_children(self)?)
            }
            InstrBlockPy::IteratorStep(op) => {
                InstrBlockPy::IteratorStep(op.try_map_children(self)?)
            }
            InstrBlockPy::ConstructClass(_)
            | InstrBlockPy::PrepareClassDecorator(_)
            | InstrBlockPy::DiscardClassDecorator(_)
            | InstrBlockPy::DiscardClassConstructionCaptures(_)
            | InstrBlockPy::ApplyClassDecorator(_) => {
                return Err(InlineUnsupportedReason::ClassConstructionExecutionContext);
            }
            InstrBlockPy::CompleteFunctionDefinition(_)
            | InstrBlockPy::ApplyFunctionDescriptor(_) => {
                return Err(InlineUnsupportedReason::FunctionDefinitionExecutionContext);
            }
        };
        Ok(clear_blockpy_instr_id(mapped))
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

fn clear_blockpy_instr_ids(instr: InstrBlockPy) -> InstrBlockPy {
    InstrIdScrubber.map_instr(instr)
}

fn clear_blockpy_instr_id(instr: InstrBlockPy) -> InstrBlockPy {
    let mut meta = instr.meta();
    meta.instr_id = None;
    instr.with_meta(meta)
}

struct InstrIdScrubber;

impl MapInstr<InstrBlockPy, InstrBlockPy> for InstrIdScrubber {
    fn map_instr(&mut self, instr: InstrBlockPy) -> InstrBlockPy {
        let mapped = match instr {
            InstrBlockPy::BinOp(op) => InstrBlockPy::BinOp(op.map_children(self)),
            InstrBlockPy::UnaryOp(op) => InstrBlockPy::UnaryOp(op.map_children(self)),
            InstrBlockPy::Tuple(op) => InstrBlockPy::Tuple(op.map_children(self)),
            InstrBlockPy::Call(op) => InstrBlockPy::Call(op.map_children(self)),
            InstrBlockPy::GetAttr(op) => InstrBlockPy::GetAttr(op.map_children(self)),
            InstrBlockPy::SetAttr(op) => InstrBlockPy::SetAttr(op.map_children(self)),
            InstrBlockPy::GetItem(op) => InstrBlockPy::GetItem(op.map_children(self)),
            InstrBlockPy::SetItem(op) => InstrBlockPy::SetItem(op.map_children(self)),
            InstrBlockPy::DelItem(op) => InstrBlockPy::DelItem(op.map_children(self)),
            InstrBlockPy::Load(op) => InstrBlockPy::Load(op.map_children(self)),
            InstrBlockPy::Store(op) => InstrBlockPy::Store(op.map_children(self)),
            InstrBlockPy::Del(op) => InstrBlockPy::Del(op.map_children(self)),
            InstrBlockPy::MakeCell(op) => InstrBlockPy::MakeCell(op.map_children(self)),
            InstrBlockPy::NewAnnotationSet(op) => {
                InstrBlockPy::NewAnnotationSet(op.map_children(self))
            }
            InstrBlockPy::SetupAnnotations(op) => {
                InstrBlockPy::SetupAnnotations(op.map_children(self))
            }
            InstrBlockPy::ConstructTypeParameterScope(op) => {
                InstrBlockPy::ConstructTypeParameterScope(op.map_children(self))
            }
            InstrBlockPy::SubscriptGeneric(op) => {
                InstrBlockPy::SubscriptGeneric(op.map_children(self))
            }
            InstrBlockPy::SetFunctionTypeParameters(op) => {
                InstrBlockPy::SetFunctionTypeParameters(op.map_children(self))
            }
            InstrBlockPy::CreateTypeAlias(op) => {
                InstrBlockPy::CreateTypeAlias(op.map_children(self))
            }
            InstrBlockPy::CreateTypeParameter(op) => {
                InstrBlockPy::CreateTypeParameter(op.map_children(self))
            }
            InstrBlockPy::SetTypeParameterDefault(op) => {
                InstrBlockPy::SetTypeParameterDefault(op.map_children(self))
            }
            InstrBlockPy::CheckAnnotationFormat(op) => {
                InstrBlockPy::CheckAnnotationFormat(op.map_children(self))
            }
            InstrBlockPy::RecordAnnotation(op) => {
                InstrBlockPy::RecordAnnotation(op.map_children(self))
            }
            InstrBlockPy::IncrementCounter(op) => InstrBlockPy::IncrementCounter(op),
            InstrBlockPy::CellRef(op) => InstrBlockPy::CellRef(op),
            InstrBlockPy::MakeFunctionWithClosure(op) => {
                InstrBlockPy::MakeFunctionWithClosure(op.map_children(self))
            }
            InstrBlockPy::ConstructClass(op) => InstrBlockPy::ConstructClass(op.map_children(self)),
            InstrBlockPy::PrepareClassDecorator(op) => {
                InstrBlockPy::PrepareClassDecorator(op.map_children(self))
            }
            InstrBlockPy::ApplyClassDecorator(op) => {
                InstrBlockPy::ApplyClassDecorator(op.map_children(self))
            }
            InstrBlockPy::DiscardClassDecorator(op) => {
                InstrBlockPy::DiscardClassDecorator(op.map_children(self))
            }
            InstrBlockPy::TakeOperand(op) => InstrBlockPy::TakeOperand(op.map_children(self)),
            InstrBlockPy::ComprehensionInsert(op) => {
                InstrBlockPy::ComprehensionInsert(op.map_children(self))
            }
            InstrBlockPy::BuildCollection(op) => {
                InstrBlockPy::BuildCollection(op.map_children(self))
            }
            InstrBlockPy::CallArgumentOp(op) => InstrBlockPy::CallArgumentOp(op.map_children(self)),
            InstrBlockPy::PreparedCall(op) => InstrBlockPy::PreparedCall(op.map_children(self)),
            InstrBlockPy::IteratorStep(op) => InstrBlockPy::IteratorStep(op.map_children(self)),
            InstrBlockPy::DiscardClassConstructionCaptures(op) => {
                InstrBlockPy::DiscardClassConstructionCaptures(op.map_children(self))
            }
            InstrBlockPy::CompleteFunctionDefinition(op) => {
                InstrBlockPy::CompleteFunctionDefinition(op.map_children(self))
            }
            InstrBlockPy::ApplyFunctionDescriptor(op) => {
                InstrBlockPy::ApplyFunctionDescriptor(op.map_children(self))
            }
        };
        clear_blockpy_instr_id(mapped)
    }

    fn map_name(&mut self, name: ResolvedName) -> ResolvedName {
        name
    }
}

#[cfg(test)]
mod inline_context_tests {
    use super::*;

    #[test]
    fn resolved_block_parameter_roles_follow_legacy_inline_locals() {
        let module = soac_lowering::lower_python_to_blockpy_for_testing(
            "def callee():\n    try:\n        work()\n    except:\n        pass\n    return 1\n\ndef caller():\n    return callee()\n",
        ).expect("actual bounded handler callee").blockpy_module;
        let callee = module
            .callable_defs
            .iter()
            .find(|function| function.names.display_name == "callee")
            .unwrap();
        let mut caller = module
            .callable_defs
            .iter()
            .find(|function| function.names.display_name == "caller")
            .unwrap()
            .clone();
        let continuation = caller.name_gen.next_block_name();
        let return_target = allocate_inline_local(&mut caller).unwrap().resolved_name();
        let fragment = build_multi_block_inline_fragment_to_target(
            &mut caller,
            callee,
            continuation,
            &InlineValueBindings::new(),
            return_target,
        )
        .expect("ordinary compiler inlining should carry control slots");
        let source = callee.storage_layout.as_ref().unwrap();
        assert!(!source.block_parameter_roles.is_empty());
        let target = caller.storage_layout.as_ref().unwrap();
        target.validate_block_parameter_roles().unwrap();
        for binding in &source.block_parameter_roles {
            let Some(location) = binding.location.as_local() else {
                continue;
            };
            let mapped = &fragment.locals[&location];
            assert!(
                target
                    .block_parameter_roles_at(NameLocation::Local(mapped.location))
                    .any(|role| role == binding.role)
            );
        }
    }
}
