use crate::block_py::{
    compute_storage_layout_from_scope, Block, BlockArg, BlockEdge, BlockLabel, BlockParam,
    BlockPyFunction, BlockPyModule, ModuleShape, BlockTerm, ScopeExprNode,
};

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
            BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
        }
    }
    Ok(())
}

fn validate_non_exception_edge<P: ModuleShape, S>(
    function: &BlockPyFunction<P>,
    source_block: &Block<P::Instr>,
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
    source_block: &Block<P::Instr>,
    target_block: &Block<P::Instr>,
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
    source_block: &Block<P::Instr>,
    target_block: &Block<P::Instr>,
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
) -> Result<&'a Block<P::Instr>, String> {
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
