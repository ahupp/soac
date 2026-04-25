use crate::passes::{CodegenModuleShape, InstrCodegen, try_allocate_codegen_stack_temp};
use soac_core::block_py::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockPyFunction, BlockTerm, CallArgPositional,
    CallDirect, ConstantExpr, HasMeta, LocalLocation, MapInstr, Mappable, NameLocation, ParamKind,
    ResolvedName, RuntimeName, Store, TryMapInstr, TryMapTerm, WithMeta,
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
    caller_constants: &mut Vec<ConstantExpr>,
    callee: &BlockPyFunction<CodegenModuleShape>,
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
    caller_constants: &mut Vec<ConstantExpr>,
    callee: &BlockPyFunction<CodegenModuleShape>,
    callee_constants: &[ConstantExpr],
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
            .filter(|instr| !matches!(instr, InstrCodegen::IncrementCounter(_)))
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
        .filter(|instr| !matches!(instr, InstrCodegen::IncrementCounter(_)))
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

impl TryMapInstr<InstrCodegen, InstrCodegen, InlineUnsupportedReason>
    for InlineLocalRemapper<'_, '_, '_, '_>
{
    fn try_map_instr(
        &mut self,
        instr: InstrCodegen,
    ) -> Result<InstrCodegen, InlineUnsupportedReason> {
        let mapped = match instr {
            InstrCodegen::BinOp(op) => InstrCodegen::BinOp(op.try_map_children(self)?),
            InstrCodegen::UnaryOp(op) => InstrCodegen::UnaryOp(op.try_map_children(self)?),
            InstrCodegen::Tuple(op) => InstrCodegen::Tuple(op.try_map_children(self)?),
            InstrCodegen::Call(op) => InstrCodegen::Call(op.try_map_children(self)?),
            InstrCodegen::GetAttr(op) => InstrCodegen::GetAttr(op.try_map_children(self)?),
            InstrCodegen::SetAttr(op) => InstrCodegen::SetAttr(op.try_map_children(self)?),
            InstrCodegen::GetItem(op) => InstrCodegen::GetItem(op.try_map_children(self)?),
            InstrCodegen::SetItem(op) => InstrCodegen::SetItem(op.try_map_children(self)?),
            InstrCodegen::DelItem(op) => InstrCodegen::DelItem(op.try_map_children(self)?),
            InstrCodegen::Load(op) => {
                if let Some(location) = op.name.local_location() {
                    if let Some(value) = self.value_bindings.get(&location) {
                        return Ok(clear_codegen_instr_ids(value.clone()));
                    }
                }
                InstrCodegen::Load(op.try_map_children(self)?)
            }
            InstrCodegen::Store(op) => {
                if let Some(location) = op.name.local_location() {
                    if self.value_bindings.contains_key(&location) {
                        return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
                    }
                }
                InstrCodegen::Store(op.try_map_children(self)?)
            }
            InstrCodegen::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    if self.value_bindings.contains_key(&location) {
                        return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
                    }
                }
                InstrCodegen::Del(op.try_map_children(self)?)
            }
            InstrCodegen::MakeCell(op) => InstrCodegen::MakeCell(op.try_map_children(self)?),
            InstrCodegen::IncrementCounter(op) => InstrCodegen::IncrementCounter(op),
            InstrCodegen::CellRef(op) => InstrCodegen::CellRef(op),
            InstrCodegen::MakeFunctionWithClosure(op) => {
                InstrCodegen::MakeFunctionWithClosure(op.try_map_children(self)?)
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
            InstrCodegen::BinOp(op) => InstrCodegen::BinOp(op.map_children(self)),
            InstrCodegen::UnaryOp(op) => InstrCodegen::UnaryOp(op.map_children(self)),
            InstrCodegen::Tuple(op) => InstrCodegen::Tuple(op.map_children(self)),
            InstrCodegen::Call(op) => InstrCodegen::Call(op.map_children(self)),
            InstrCodegen::GetAttr(op) => InstrCodegen::GetAttr(op.map_children(self)),
            InstrCodegen::SetAttr(op) => InstrCodegen::SetAttr(op.map_children(self)),
            InstrCodegen::GetItem(op) => InstrCodegen::GetItem(op.map_children(self)),
            InstrCodegen::SetItem(op) => InstrCodegen::SetItem(op.map_children(self)),
            InstrCodegen::DelItem(op) => InstrCodegen::DelItem(op.map_children(self)),
            InstrCodegen::Load(op) => InstrCodegen::Load(op.map_children(self)),
            InstrCodegen::Store(op) => InstrCodegen::Store(op.map_children(self)),
            InstrCodegen::Del(op) => InstrCodegen::Del(op.map_children(self)),
            InstrCodegen::MakeCell(op) => InstrCodegen::MakeCell(op.map_children(self)),
            InstrCodegen::IncrementCounter(op) => InstrCodegen::IncrementCounter(op),
            InstrCodegen::CellRef(op) => InstrCodegen::CellRef(op),
            InstrCodegen::MakeFunctionWithClosure(op) => {
                InstrCodegen::MakeFunctionWithClosure(op.map_children(self))
            }
        };
        clear_codegen_instr_id(mapped)
    }

    fn map_name(&mut self, name: ResolvedName) -> ResolvedName {
        name
    }
}
