use crate::block_py::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockPyFunction, BlockTerm, CallArgPositional,
    CallDirect, HasMeta, InstrCodegen, LocalLocation, Mappable, NameLocation, ParamKind,
    ResolvedName, Store, TryMapInstr, WithMeta,
};
use crate::passes::{CodegenModuleShape, InstrCodegenOp};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct InlineFragment {
    pub entry_label: BlockLabel,
    pub blocks: Vec<Block<InstrCodegen>>,
    pub locals: HashMap<LocalLocation, InlineLocal>,
    pub return_local: InlineLocal,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct InlineLocal {
    pub name: String,
    pub location: LocalLocation,
}

pub type InlineValueBindings = HashMap<LocalLocation, InstrCodegen>;

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
    let return_local = allocate_inline_local(caller)?;

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
        Store::new(return_local.resolved_name(), Box::new(return_value))
            .with_meta(return_meta)
            .into(),
    );

    let entry_label = caller.name_gen.next_block_name();
    let return_arg = BlockArg::Name(return_local.name.clone());
    let block = Block::new(
        entry_label,
        body,
        BlockTerm::Jump(BlockEdge::with_args(continuation, vec![return_arg])),
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
        match instr {
            InstrCodegenOp::BinOp(op) => Ok(InstrCodegenOp::BinOp(op.try_map_children(self)?)),
            InstrCodegenOp::UnaryOp(op) => Ok(InstrCodegenOp::UnaryOp(op.try_map_children(self)?)),
            InstrCodegenOp::CalleeFunctionId(op) => {
                Ok(InstrCodegenOp::CalleeFunctionId(op.try_map_children(self)?))
            }
            InstrCodegenOp::Tuple(op) => Ok(InstrCodegenOp::Tuple(op.try_map_children(self)?)),
            InstrCodegenOp::Call(op) => Ok(InstrCodegenOp::Call(op.try_map_children(self)?)),
            InstrCodegenOp::CallDirect(op) => {
                Ok(InstrCodegenOp::CallDirect(op.try_map_children(self)?))
            }
            InstrCodegenOp::GetAttr(op) => Ok(InstrCodegenOp::GetAttr(op.try_map_children(self)?)),
            InstrCodegenOp::SetAttr(op) => Ok(InstrCodegenOp::SetAttr(op.try_map_children(self)?)),
            InstrCodegenOp::GetItem(op) => Ok(InstrCodegenOp::GetItem(op.try_map_children(self)?)),
            InstrCodegenOp::SetItem(op) => Ok(InstrCodegenOp::SetItem(op.try_map_children(self)?)),
            InstrCodegenOp::DelItem(op) => Ok(InstrCodegenOp::DelItem(op.try_map_children(self)?)),
            InstrCodegenOp::Load(op) => {
                if let Some(location) = op.name.local_location() {
                    if let Some(value) = self.value_bindings.get(&location) {
                        return Ok(value.clone());
                    }
                }
                Ok(InstrCodegenOp::Load(op.try_map_children(self)?))
            }
            InstrCodegenOp::Store(op) => {
                if let Some(location) = op.name.local_location() {
                    if self.value_bindings.contains_key(&location) {
                        return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
                    }
                }
                Ok(InstrCodegenOp::Store(op.try_map_children(self)?))
            }
            InstrCodegenOp::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    if self.value_bindings.contains_key(&location) {
                        return Err(InlineUnsupportedReason::RebindsBoundLocal(location));
                    }
                }
                Ok(InstrCodegenOp::Del(op.try_map_children(self)?))
            }
            InstrCodegenOp::MakeCell(op) => {
                Ok(InstrCodegenOp::MakeCell(op.try_map_children(self)?))
            }
            InstrCodegenOp::IncrementCounter(op) => Ok(InstrCodegenOp::IncrementCounter(op)),
            InstrCodegenOp::CellRef(op) => Ok(InstrCodegenOp::CellRef(op)),
            InstrCodegenOp::MakeFunctionWithClosure(op) => Ok(
                InstrCodegenOp::MakeFunctionWithClosure(op.try_map_children(self)?),
            ),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::{BlockParam, BlockParamRole, CallDirect, Load};
    use crate::lower_python_to_blockpy_for_testing;

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

    fn local_load(function: &BlockPyFunction<CodegenModuleShape>, name: &str) -> InstrCodegen {
        Load::new(ResolvedName {
            id: name.to_string().into(),
            location: NameLocation::Local(local_location(function, name)),
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
        let BlockArg::Name(return_arg) = &edge.args[0] else {
            panic!("continuation argument should name the synthetic return local");
        };
        assert_eq!(return_arg, &fragment.return_local.name);

        let Some(InstrCodegen::Store(return_store)) = fragment.blocks[0].body.last() else {
            panic!("inlined block should store the return value before jumping");
        };
        assert_eq!(
            return_store.name.local_location(),
            Some(fragment.return_local.location)
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
}
