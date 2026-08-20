use crate::block_py::{
    compute_storage_layout_from_scope, Block, BlockArg, BlockEdge, BlockLabel, BlockParam,
    BlockPyFunction, BlockPyModule, BlockTerm, ConstantExpr, FunctionKind, InstrBlockPy,
    ModuleShape, NameLocation, RuntimeName, ScopeExprNode,
};

pub(crate) fn validate_blockpy_module(
    module: &BlockPyModule<soac_ir_blockpy::BlockPyModuleShape>,
) -> Result<(), String> {
    for function in &module.callable_defs {
        validate_compiler_operand_operations(function)?;
        match (
            function.scope.class_bindings.as_ref(),
            function
                .storage_layout
                .as_ref()
                .and_then(|layout| layout.class_bindings.as_ref()),
        ) {
            (Some(source), Some(projection)) => projection
                .validate(
                    source,
                    function.storage_layout.as_ref().expect("class layout"),
                    &function.scope,
                )
                .map_err(|error| format!("class bindings {}: {error}", function.names.qualname))?,
            (None, None) => {}
            _ => {
                return Err(format!(
                    "class bindings {} lost their producer or physical projection",
                    function.names.qualname
                ))
            }
        }
        if let Some(layout) = &function.storage_layout {
            layout.validate_generator_roles()?;
            layout.validate_block_parameter_roles()?;
            layout.validate_block_parameter_declarations(
                function.blocks.iter().flat_map(|block| &block.params),
            )?;
            if let Some(abi) = &layout.generator_resume_abi {
                abi.validate(function.kind, function.body_params())?;
            } else if function.kind != FunctionKind::Function {
                return Err(format!(
                    "generator-like executable {} lost its explicit resume ABI",
                    function.names.qualname
                ));
            }
        }
        if let Some(layout) = &function.public_storage_layout {
            layout.validate_generator_roles()?;
            layout.validate_block_parameter_roles()?;
        }
        if function.kind == FunctionKind::AsyncGenerator {
            for block in &function.blocks {
                if let BlockTerm::GeneratorReturn(value) = &block.term {
                    // Unlike generators and coroutines, an async generator cannot
                    // carry a completion value. Constant lowering can hoist
                    // the immutable runtime singleton; authenticate the actual
                    // table entry, never the displayed name of its load.
                    let is_none = match value {
                        InstrBlockPy::Load(load) if load.cell_binding.is_none() => {
                            match load.name.location {
                                NameLocation::RuntimeName(RuntimeName::None) => true,
                                NameLocation::Constant(index) => matches!(
                                    module.module_constants.get(index as usize),
                                    Some(ConstantExpr::RuntimeName(RuntimeName::None))
                                ),
                                _ => false,
                            }
                        }
                        _ => false,
                    };
                    if !is_none {
                        return Err(format!(
                            "async generator completion at {}:{} requires the canonical None operand",
                            function.names.qualname, block.label,
                        ));
                    }
                }
            }
        }
        if let Some(block) = function
            .blocks
            .iter()
            .find(|block| block.extra.suspension_resume.is_some())
        {
            return Err(format!(
                "unconsumed suspension ownership edge at {}:{}",
                function.names.qualname, block.label,
            ));
        }
    }
    validate_module(module)
}

fn validate_compiler_operand_operations(
    function: &BlockPyFunction<soac_ir_blockpy::BlockPyModuleShape>,
) -> Result<(), String> {
    use crate::block_py::{ChildVisitable, Visit};
    struct Validate<'a> {
        function: &'a BlockPyFunction<soac_ir_blockpy::BlockPyModuleShape>,
        error: Option<String>,
    }
    impl Visit<InstrBlockPy> for Validate<'_> {
        fn visit_instr(&mut self, instr: &InstrBlockPy) {
            if self.error.is_some() {
                return;
            }
            instr.visit_children(self);
            let result = match instr {
                InstrBlockPy::TakeOperand(op) => self
                    .function
                    .storage_layout
                    .as_ref()
                    .ok_or_else(|| "operand take has no resolved layout".to_string())
                    .and_then(|layout| op.validate_resolved(layout).map(|_| ())),
                InstrBlockPy::ComprehensionInsert(op) => self
                    .function
                    .storage_layout
                    .as_ref()
                    .ok_or_else(|| "comprehension insertion has no resolved layout".to_string())
                    .and_then(|layout| op.validate_resolved(layout).map(|_| ())),
                InstrBlockPy::BuildCollection(op) => op.validate_shape(),
                InstrBlockPy::CallArgumentOp(op) => self
                    .function
                    .storage_layout
                    .as_ref()
                    .ok_or_else(|| "CallArgumentOp has no resolved layout".to_string())
                    .and_then(|layout| op.validate_resolved(layout).map(|_| ())),
                InstrBlockPy::PreparedCall(op) => self
                    .function
                    .storage_layout
                    .as_ref()
                    .ok_or_else(|| "PreparedCall has no resolved layout".to_string())
                    .and_then(|layout| op.validate_resolved(layout).map(|_| ())),
                InstrBlockPy::IteratorStep(op) => self
                    .function
                    .storage_layout
                    .as_ref()
                    .ok_or_else(|| "IteratorStep has no resolved layout".to_string())
                    .and_then(|layout| op.validate_resolved(layout).map(|_| ())),
                _ => Ok(()),
            };
            if let Err(error) = result {
                self.error = Some(format!("{}: {error}", self.function.names.qualname));
            }
        }
    }
    let mut visitor = Validate {
        function,
        error: None,
    };
    visitor.visit_fn(function);
    visitor.error.map_or(Ok(()), Err)
}

pub(crate) fn validate_module<P: ModuleShape>(module: &BlockPyModule<P>) -> Result<(), String>
where
    P::Instr: ScopeExprNode + crate::block_py::Instr,
{
    for function in &module.callable_defs {
        validate_function(function)?;
    }
    Ok(())
}

fn validate_function<P: ModuleShape>(function: &BlockPyFunction<P>) -> Result<(), String>
where
    P::Instr: ScopeExprNode + crate::block_py::Instr,
{
    let qualname = function.names.qualname.as_str();
    validate_storage_layout_scoping(function, qualname)?;
    for (index, block) in function.blocks.iter().enumerate() {
        if block.label.index() != index {
            return Err(format!(
                "non-dense block label {} at {}:{}, expected bb{}",
                block.label, qualname, index, index
            ));
        }
    }

    for block in &function.blocks {
        if let BlockTerm::Raise(raise) = &block.term {
            raise
                .validate_exception_operand()
                .map_err(|error| format!("{error} at {qualname}:{}", block.label))?;
        }
        if matches!(block.term, BlockTerm::GeneratorReturn(_))
            && !matches!(
                function.kind,
                FunctionKind::Generator | FunctionKind::Coroutine | FunctionKind::AsyncGenerator
            )
        {
            return Err(format!(
                "generator completion at {qualname}:{} requires a generator, coroutine, or async generator activation",
                block.label
            ));
        }
        if let Some(exc_edge) = block.exc_edge.as_ref() {
            let target_block = lookup_known_block(
                function,
                exc_edge.target,
                qualname,
                block.label,
                "exception target",
            )?;
            if exc_edge.args.len() != target_block.param_name_vec().len() {
                return Err(format!(
                    "exception dispatch from {}:{} has {} explicit edge args for target {} with {} full params",
                    qualname,
                    block.label,
                    exc_edge.args.len(),
                    target_block.label,
                    target_block.param_name_vec().len()
                ));
            }
            for (target_param_name, source) in target_block
                .param_name_vec()
                .iter()
                .zip(exc_edge.args.iter())
            {
                if let BlockArg::AbruptKind(kind) = source {
                    return Err(format!(
                        "exception dispatch from {}:{} uses abrupt-kind edge arg {:?} for target param {}",
                        qualname, block.label, kind, target_param_name
                    ));
                }
            }
        }
        match &block.term {
            BlockTerm::Jump(target) => {
                validate_non_exception_edge(function, block, target, qualname, "jump target")?;
            }
            BlockTerm::IfTerm(if_term) => {
                validate_non_exception_edge(
                    function,
                    block,
                    &BlockEdge::new(if_term.then_label),
                    qualname,
                    "then target",
                )?;
                validate_non_exception_edge(
                    function,
                    block,
                    &BlockEdge::new(if_term.else_label),
                    qualname,
                    "else target",
                )?;
            }
            BlockTerm::BranchTable(branch) => {
                for target in &branch.targets {
                    validate_non_exception_edge(
                        function,
                        block,
                        &BlockEdge::new(*target),
                        qualname,
                        "br_table target",
                    )?;
                }
                validate_non_exception_edge(
                    function,
                    block,
                    &BlockEdge::new(branch.default_label),
                    qualname,
                    "br_table default target",
                )?;
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {}
        }
    }
    Ok(())
}

fn validate_non_exception_edge<P: ModuleShape, S>(
    function: &BlockPyFunction<P>,
    source_block: &Block<P::Instr, P::BlockExtra>,
    edge: &BlockEdge,
    qualname: &str,
    label_kind: &str,
) -> Result<(), String>
where
    P: ModuleShape<Instr = S>,
    S: crate::block_py::Instr,
{
    let target_block = lookup_known_block(
        function,
        edge.target,
        qualname,
        source_block.label,
        label_kind,
    )?;
    validate_edge_param_forwarding::<P, S>(
        source_block,
        target_block,
        edge.args.as_slice(),
        qualname,
        label_kind,
    )
}

fn validate_edge_param_forwarding<P: ModuleShape, S>(
    source_block: &Block<P::Instr, P::BlockExtra>,
    target_block: &Block<P::Instr, P::BlockExtra>,
    explicit_args: &[BlockArg],
    qualname: &str,
    label_kind: &str,
) -> Result<(), String>
where
    P: ModuleShape<Instr = S>,
    S: crate::block_py::Instr,
{
    if explicit_args.len() > target_block.params.len() {
        return Err(format!(
            "{} from {}:{} has {} explicit edge args for target {} with {} full params",
            label_kind,
            qualname,
            source_block.label,
            explicit_args.len(),
            target_block.label,
            target_block.params.len()
        ));
    }

    let explicit_start = target_block
        .params
        .len()
        .saturating_sub(explicit_args.len());
    for target_param in target_block.params.iter().take(explicit_start) {
        // Implicit transport is by exact name. Nested handlers can have more
        // than one EnclosingException parameter, and a surviving outer region
        // can become the current Exception again after inner cleanup.
        if source_block
            .params
            .iter()
            .any(|source_param| source_param.name == target_param.name)
        {
            continue;
        }
        let Some(source_same_role) = source_block
            .params
            .iter()
            .find(|source_param| source_param.role == target_param.role)
        else {
            continue;
        };
        if source_same_role.name != target_param.name {
            return Err(format!(
                "{} from {}:{} reaches target {} with implicit forwarding for param {} ({:?}), but source only has same-role param {}; add an explicit edge arg",
                label_kind,
                qualname,
                source_block.label,
                target_block.label,
                target_param.name,
                target_param.role,
                source_same_role.name,
            ));
        }
    }

    for (target_param, source_arg) in target_block
        .params
        .iter()
        .skip(explicit_start)
        .zip(explicit_args.iter())
    {
        validate_explicit_edge_arg::<P, S>(
            source_block,
            target_block,
            target_param,
            source_arg,
            qualname,
            label_kind,
        )?;
    }

    Ok(())
}

fn validate_explicit_edge_arg<P: ModuleShape, S>(
    source_block: &Block<P::Instr, P::BlockExtra>,
    target_block: &Block<P::Instr, P::BlockExtra>,
    target_param: &BlockParam,
    source_arg: &BlockArg,
    qualname: &str,
    label_kind: &str,
) -> Result<(), String>
where
    P: ModuleShape<Instr = S>,
    S: crate::block_py::Instr,
{
    match (target_param.role, source_arg) {
        (_, BlockArg::Name(_) | BlockArg::None) => Ok(()),
        (crate::block_py::BlockParamRole::Exception, BlockArg::CurrentException) => Ok(()),
        (crate::block_py::BlockParamRole::AbruptKind, BlockArg::AbruptKind(_)) => Ok(()),
        (_, BlockArg::AbruptKind(kind)) => Err(format!(
            "{} from {}:{} uses abrupt-kind edge arg {:?} for target param {}",
            label_kind, qualname, source_block.label, kind, target_param.name
        )),
        (_, BlockArg::CurrentException) => Err(format!(
            "{} from {}:{} uses current-exception edge arg for non-exception target param {} on target {}",
            label_kind, qualname, source_block.label, target_param.name, target_block.label
        )),
    }
}

fn validate_storage_layout_scoping<P: ModuleShape, S>(
    function: &BlockPyFunction<P>,
    qualname: &str,
) -> Result<(), String>
where
    P: ModuleShape<Instr = S>,
    S: ScopeExprNode + crate::block_py::Instr,
{
    let expected_layout = compute_storage_layout_from_scope(function);

    let Some(layout) = function.storage_layout.as_ref() else {
        if expected_layout.is_none() {
            return Ok(());
        }
        return Err(format!(
            "closure layout missing for {} despite scope closure state",
            qualname
        ));
    };

    let Some(expected_layout) = expected_layout else {
        return Ok(());
    };

    for expected_slot in &expected_layout.cellvars {
        if layout.preserved_slots.iter().any(|slot| {
            slot.storage == crate::block_py::PreservedSlotStorage::PyCellObject
                && slot.logical_name == expected_slot.logical_name
                && slot.storage_name == expected_slot.storage_name
        }) {
            continue;
        }
        let Some(actual_slot) = layout
            .cellvars
            .iter()
            .find(|slot| slot.logical_name == expected_slot.logical_name)
        else {
            return Err(format!(
                "closure layout for {} is missing owner cell {}; actual cellvars: {:?}",
                qualname,
                expected_slot.logical_name,
                layout
                    .cellvars
                    .iter()
                    .map(|slot| format!("{}->{}", slot.logical_name, slot.storage_name))
                    .collect::<Vec<_>>()
            ));
        };
        if actual_slot.storage_name != expected_slot.storage_name {
            return Err(format!(
                "closure layout for {} has owner cell {} stored as {}, but scope info expects {}; actual cellvars: {:?}",
                qualname,
                expected_slot.logical_name,
                actual_slot.storage_name,
                expected_slot.storage_name,
                layout
                    .cellvars
                    .iter()
                    .map(|slot| format!("{}->{}", slot.logical_name, slot.storage_name))
                    .collect::<Vec<_>>()
            ));
        }
    }

    for expected_slot in &expected_layout.freevars {
        if !layout
            .freevars
            .iter()
            .any(|slot| slot.logical_name == expected_slot.logical_name)
        {
            return Err(format!(
                "closure layout for {} is missing freevar {}; actual freevars: {:?}",
                qualname,
                expected_slot.logical_name,
                layout
                    .freevars
                    .iter()
                    .map(|slot| format!("{}->{}", slot.logical_name, slot.storage_name))
                    .collect::<Vec<_>>()
            ));
        }
    }
    Ok(())
}

fn lookup_known_block<'a, P: ModuleShape>(
    function: &'a BlockPyFunction<P>,
    label: BlockLabel,
    qualname: &str,
    block_label: BlockLabel,
    label_kind: &str,
) -> Result<&'a Block<P::Instr, P::BlockExtra>, String> {
    let Some(target_block) = function.blocks.get(label.index()) else {
        return Err(format!(
            "unknown {label_kind} {label} in {}:{}",
            qualname, block_label
        ));
    };
    if target_block.label == label {
        return Ok(target_block);
    }
    Err(format!(
        "unknown {label_kind} {label} in {}:{}",
        qualname, block_label
    ))
}
