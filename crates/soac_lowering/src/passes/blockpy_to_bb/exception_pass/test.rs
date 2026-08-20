use super::lower_try_jump_exception_flow;
use crate::block_py::{
    validate_module, AbruptKind, BindingKind, BlockArg, BlockEdge, BlockLabel, BlockParam,
    BlockParamRole, BlockPyBlock, BlockTerm, CellBindingKind, InstrBlockPy, InstrResolved,
    InstrWithConstantNone, Literal, NameLocation, NumberLiteral, NumberLiteralValue,
    ResolvedStorageBlock, StorageLayout,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::pass_tracker::LoweringPassTrackerInternalExt;
use soac_ir_blockpy::BlockPyModuleShape;

fn tracked_name_binding_module(
    source: &str,
) -> crate::block_py::BlockPyModule<crate::passes::ResolvedStorageModuleShape> {
    lower_python_to_blockpy_for_testing(source)
        .expect("lowering must succeed")
        .pass_tracker
        .pass_name_binding()
        .expect("bb module must exist")
        .clone()
}

fn tracked_blockpy_module(source: &str) -> crate::block_py::BlockPyModule<BlockPyModuleShape> {
    let name_binding = tracked_name_binding_module(source);
    let lowered = lower_try_jump_exception_flow(&name_binding);
    let mut codegen = crate::passes::blockpy_to_bb::strings::hoist_module_constants(&lowered);
    crate::block_py::cfg::relabel_dense_bb_module(&mut codegen);
    soac_ir_blockpy::assign_blockpy_module_instr_ids(codegen)
}

#[test]
fn assignment_operands_unwind_before_the_source_handler_becomes_active() {
    let module = tracked_blockpy_module(
        "def unpack(values):\n    try:\n        first, second = iter(values)\n    except ValueError:\n        pass\n",
    );
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.fn_name == "unpack")
        .unwrap();
    let cleanup = function
        .blocks
        .iter()
        .find(|block| {
            block.extra.handled_exception == crate::block_py::HandledExceptionContext::Preserve
                && block
                    .body
                    .iter()
                    .any(|instr| matches!(instr, InstrBlockPy::Del(op) if op.quietly))
        })
        .expect("assignment operands need an explicit pre-handler unwind block");
    assert!(
        cleanup.exception_param().is_some(),
        "transport retains the pending raised-scope marker"
    );
    assert!(cleanup
        .body
        .iter()
        .all(|instr| matches!(instr, InstrBlockPy::Del(op) if op.quietly)));
    assert!(function
        .blocks
        .iter()
        .any(|block| block.exc_edge.as_ref().is_some_and(|edge| {
            edge.target == cleanup.label
                && edge
                    .args
                    .iter()
                    .any(|arg| matches!(arg, BlockArg::CurrentException))
        })));
    let BlockTerm::Jump(edge) = &cleanup.term else {
        panic!("cleanup must forward the original handler")
    };
    let handler = function
        .blocks
        .iter()
        .find(|block| block.label == edge.target)
        .unwrap();
    assert!(handler.exception_param().is_some());
}

#[test]
fn operand_unwind_uses_producer_order_and_keeps_semantic_local_owners() {
    let module = tracked_blockpy_module(
        "def assign(make, target, key):\n    _dp_source = make()\n    try:\n        target()[key()] = make()\n    except TypeError:\n        pass\n    return _dp_source\n",
    );
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.fn_name == "assign")
        .unwrap();
    let mut operands = std::collections::HashMap::new();
    let mut source = None;
    for instr in function.blocks.iter().flat_map(|block| &block.body) {
        if let InstrBlockPy::Store(store) = instr {
            if store.name.id.as_str() == "_dp_source" {
                source = Some(store.name.location);
            }
            if let crate::block_py::StoreLifetime::Operand { unwind_order } = store.lifetime {
                operands.insert(store.name.location, unwind_order);
            }
        }
    }
    let source = source.expect("source local must remain represented");
    assert!(!operands.contains_key(&source));
    let cleanup = function
        .blocks
        .iter()
        .filter(|block| {
            block.extra.handled_exception == crate::block_py::HandledExceptionContext::Preserve
        })
        .max_by_key(|block| block.body.len())
        .expect("an operand cleanup exists");
    assert!(
        cleanup.body.len() >= 3,
        "value, object, and key are separate operands"
    );
    let ranks = cleanup
        .body
        .iter()
        .map(|instr| {
            let InstrBlockPy::Del(del) = instr else {
                panic!("cleanup contains only explicit deletes")
            };
            assert!(del.quietly);
            assert_ne!(del.name.location, source);
            operands[&del.name.location]
        })
        .collect::<Vec<_>>();
    assert!(ranks.windows(2).all(|pair| pair[0] > pair[1]));
}

#[test]
fn augmented_target_failure_unwinds_receiver_and_key_before_result() {
    let module = tracked_blockpy_module(
        "def attr(owner, rhs):\n    try:\n        owner().value += rhs()\n    except TypeError:\n        pass\ndef item(owner, key, rhs):\n    try:\n        owner()[key()] += rhs()\n    except TypeError:\n        pass\n",
    );
    let location = |instr: &InstrBlockPy| {
        let InstrBlockPy::Load(load) = instr else {
            panic!("augmented target operands must be materialized")
        };
        load.name.location
    };
    for function_name in ["attr", "item"] {
        let function = module
            .callable_defs
            .iter()
            .find(|function| function.names.fn_name == function_name)
            .unwrap();
        let (target, expected) = function
            .blocks
            .iter()
            .find_map(|block| {
                block.body.iter().find_map(|instr| match instr {
                    InstrBlockPy::SetAttr(store) => Some((
                        block,
                        vec![location(&store.value), location(&store.replacement)],
                    )),
                    InstrBlockPy::SetItem(store) => Some((
                        block,
                        vec![
                            location(&store.index),
                            location(&store.value),
                            location(&store.replacement),
                        ],
                    )),
                    _ => None,
                })
            })
            .expect("source must contain an augmented target store");
        let edge = target.exc_edge.as_ref().expect("setter can fail");
        let cleanup = function
            .blocks
            .iter()
            .find(|block| block.label == edge.target)
            .unwrap();
        assert_eq!(
            cleanup.extra.handled_exception,
            crate::block_py::HandledExceptionContext::Preserve
        );
        let actual = cleanup
            .body
            .iter()
            .filter_map(|instr| {
                let InstrBlockPy::Del(del) = instr else {
                    panic!("operand unwind must contain only deletes")
                };
                assert!(del.quietly);
                expected
                    .contains(&del.name.location)
                    .then_some(del.name.location)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual, expected,
            "the result is computed last but unwinds after target operands"
        );
    }
    validate_module(&module).expect("augmented cleanup transports must be valid");
}

#[test]
fn suspended_assignment_operand_unwind_resolves_preserved_storage() {
    let module = tracked_blockpy_module(
        "def unpack():\n    try:\n        first, second = yield None\n    except ValueError:\n        pass\n    yield None\n",
    );
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.fn_name == "unpack")
        .unwrap();
    assert!(function
        .blocks
        .iter()
        .flat_map(|block| &block.body)
        .any(|instr| {
            matches!(instr, InstrBlockPy::Store(store)
            if matches!(store.lifetime, crate::block_py::StoreLifetime::Operand { .. })
                && matches!(store.name.location, NameLocation::Preserved(_)))
        }));
    assert!(function.blocks.iter().filter(|block| {
        block.extra.handled_exception == crate::block_py::HandledExceptionContext::Preserve
    }).flat_map(|block| &block.body).any(|instr| {
        matches!(instr, InstrBlockPy::Del(del) if del.quietly && matches!(del.name.location, NameLocation::Preserved(_)))
    }));
    validate_module(&module).expect("cleanup slots and exception transports must be valid");
}

#[test]
fn delegated_await_moves_augmented_value_without_a_second_owner() {
    use crate::block_py::{ChildVisitable, StoreLifetime, Visit};

    let module = tracked_blockpy_module(
        "def make_runner(base):\n    async def run(wait):\n        total = base\n        total += await wait\n        return total\n    return run\n",
    );
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.fn_name == "run")
        .unwrap();
    let operand = function
        .blocks
        .iter()
        .flat_map(|block| &block.body)
        .find_map(|instr| match instr {
            InstrBlockPy::Store(store)
                if matches!(store.lifetime, StoreLifetime::Operand { .. })
                    && matches!(store.value.as_ref(), InstrBlockPy::Load(load)
                        if load.name.id.as_str() == "total") =>
            {
                Some(store.name.location)
            }
            _ => None,
        })
        .expect("augmented assignment captures the original source value");
    assert!(matches!(operand, NameLocation::Preserved(_)));

    struct OperandUses {
        location: NameLocation,
        loads: usize,
        takes: usize,
        required_deletes: usize,
    }
    impl Visit<InstrBlockPy> for OperandUses {
        fn visit_instr(&mut self, instr: &InstrBlockPy) {
            match instr {
                InstrBlockPy::Load(load) if load.name.location == self.location => {
                    self.loads += 1;
                }
                InstrBlockPy::TakeOperand(take) if take.name.location == self.location => {
                    self.takes += 1;
                }
                InstrBlockPy::Del(del) if del.name.location == self.location && !del.quietly => {
                    self.required_deletes += 1;
                }
                _ => {}
            }
            instr.visit_children(self);
        }
    }
    let mut uses = OperandUses {
        location: operand,
        loads: 0,
        takes: 0,
        required_deletes: 0,
    };
    uses.visit_fn(function);
    assert_eq!(
        uses.takes, 1,
        "the captured value moves into its suspended use"
    );
    assert_eq!(
        uses.loads, 0,
        "await must not retain a duplicate operand owner"
    );
    assert_eq!(
        uses.required_deletes, 0,
        "a consuming handoff cannot leave a second mandatory retirement"
    );
    validate_module(&module).expect("delegated operand cleanup remains valid");
}

fn is_return_of_number_constant(term: &BlockTerm<InstrResolved>) -> bool {
    match term {
        BlockTerm::Return(InstrResolved::Literal(literal))
            if matches!(
                literal.as_literal(),
                Literal::NumberLiteral(NumberLiteral {
                    value: NumberLiteralValue::Int(_),
                    ..
                })
            ) =>
        {
            true
        }
        BlockTerm::Return(InstrResolved::Load(op))
            if matches!(op.name.location, NameLocation::Constant(_)) =>
        {
            true
        }
        _ => false,
    }
}

#[test]
fn preserves_existing_exception_edges() {
    let source = r#"
def f(x):
    return x
"#;
    let mut module = tracked_name_binding_module(source);
    let (body_label, except_label) = {
        let function = module
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "f")
            .expect("must contain f");
        let body_label = BlockLabel::from_index(100);
        let except_label = BlockLabel::from_index(101);

        function
            .storage_layout
            .as_mut()
            .unwrap()
            .ensure_stack_slot("_dp_try_exc_manual");
        function.blocks.push(ResolvedStorageBlock {
            label: body_label.clone(),
            body: vec![],
            term: BlockTerm::<InstrResolved>::Return(InstrResolved::constant_none()),
            params: vec![crate::block_py::BlockParam {
                name: "_dp_try_exc_manual".to_string(),
                role: crate::block_py::BlockParamRole::Exception,
            }],
            exc_edge: Some(BlockEdge::new(except_label.clone())),
            extra: Default::default(),
        });
        function.blocks.push(ResolvedStorageBlock {
            label: except_label.clone(),
            body: vec![],
            term: BlockTerm::<InstrResolved>::Return(InstrResolved::constant_none()),
            params: Vec::new(),
            exc_edge: None,
            extra: Default::default(),
        });
        (body_label, except_label)
    };

    let lowered = lower_try_jump_exception_flow(&module);
    let lowered_function = lowered
        .callable_defs
        .iter()
        .find(|candidate| candidate.names.qualname == "f")
        .expect("must contain lowered f");
    let body_block = lowered_function
        .blocks
        .iter()
        .find(|block| block.label == body_label)
        .expect("body block must exist");
    assert_eq!(
        body_block.exc_edge.as_ref().map(|edge| edge.target),
        Some(except_label),
        "body region should dispatch to except block on exception"
    );
    assert_eq!(
        body_block.exception_param(),
        Some("_dp_try_exc_manual"),
        "exception binding name should be attached to body region"
    );
}

#[test]
fn rejects_try_jump_with_unknown_label() {
    let source = r#"
def f():
    return 1
"#;
    let mut module = tracked_blockpy_module(source);
    let function = module
        .callable_defs
        .first_mut()
        .expect("must contain function");
    function.blocks[0].exc_edge = Some(BlockEdge::new(BlockLabel::from_index(999)));

    let err = validate_module(&module).expect_err("must reject unknown labels");
    assert!(
        err.contains("unknown exception target"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_duplicate_block_labels() {
    let source = r#"
def f(x):
    if x:
        return 1
    return 2
"#;
    let mut module = tracked_blockpy_module(source);
    let function = module
        .callable_defs
        .first_mut()
        .expect("must contain function");
    assert!(function.blocks.len() >= 2, "test requires multiple blocks");
    function.blocks[1].label = function.blocks[0].label;

    let err = validate_module(&module).expect_err("must reject duplicate labels");
    assert!(
        err.contains("non-dense block label"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_exception_edge_with_wrong_arg_arity() {
    let source = r#"
def f():
    return 1
"#;
    let mut module = tracked_blockpy_module(source);
    let function = module
        .callable_defs
        .first_mut()
        .expect("must contain function");
    let target = function.blocks[0].label;
    function.blocks[0].exc_edge = Some(BlockEdge::with_args(target, vec![BlockArg::None]));

    let err = validate_module(&module).expect_err("must reject mismatched exception edge arity");
    assert!(
        err.contains("exception dispatch")
            && err.contains("explicit edge args")
            && err.contains("full params"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_exception_edge_with_abrupt_kind_arg() {
    let source = r#"
def f():
    return 1
"#;
    let mut module = tracked_blockpy_module(source);
    let function = module
        .callable_defs
        .first_mut()
        .expect("must contain function");
    let target = function.blocks[0].label;
    function.blocks[0].set_exception_param("_dp_try_exc");
    function.blocks[0].exc_edge = Some(BlockEdge::with_args(
        target,
        vec![BlockArg::AbruptKind(AbruptKind::Exception)],
    ));

    let err = validate_module(&module).expect_err("must reject abrupt-kind exception edge args");
    assert!(
        err.contains("exception dispatch")
            && err.contains("abrupt-kind edge arg")
            && err.contains("target param"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_jump_that_implicitly_drops_renamed_exception_param() {
    let source = r#"
def f():
    return 1
"#;
    let mut module = tracked_blockpy_module(source);
    let function = module
        .callable_defs
        .first_mut()
        .expect("must contain function");
    function.blocks[0].set_exception_param("_dp_yield_from_exc");
    let target = BlockLabel::from_index(function.blocks.len());
    function.blocks.push(BlockPyBlock {
        label: target,
        body: vec![],
        term: BlockTerm::<InstrBlockPy>::Return(InstrBlockPy::constant_none()),
        params: vec![BlockParam {
            name: "_dp_try_exc".to_string(),
            role: BlockParamRole::Exception,
        }],
        exc_edge: None,
        extra: Default::default(),
    });
    function.blocks[0].term = BlockTerm::Jump(BlockEdge::new(target));

    let err =
        validate_module(&module).expect_err("must reject implicit renamed exception forwarding");
    assert!(
        err.contains("jump target")
            && err.contains("_dp_try_exc")
            && err.contains("_dp_yield_from_exc")
            && err.contains("explicit edge arg"),
        "unexpected error: {err}"
    );
}

#[test]
fn rejects_semantic_cell_binding_storage_drift_from_storage_layout() {
    let source = r#"
def f():
    return 1
"#;
    let mut module = tracked_blockpy_module(source);
    let function = module
        .callable_defs
        .first_mut()
        .expect("must contain function");
    function.storage_layout = Some(StorageLayout {
        class_bindings: None,
        block_parameter_roles: Vec::new(),
        generator_resume_abi: None,
        expression_temporaries: Vec::new(),
        freevars: vec![],
        cellvars: vec![crate::block_py::ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_wrong_storage".to_string(),
            init: crate::block_py::ClosureInit::Deferred,
        }],
        preserved_slots: vec![],
        stack_slots: Vec::new(),
    });
    function.scope.insert_binding(
        "captured",
        BindingKind::Cell(CellBindingKind::Owner),
        false,
        Some("_dp_cell_captured".to_string()),
    );

    let err = validate_module(&module).expect_err("must reject scope/layout drift");
    assert!(
        err.contains("scope info expects _dp_cell_captured")
            && err.contains("_dp_wrong_storage")
            && err.contains("captured"),
        "unexpected error: {err}"
    );
}

#[test]
fn splits_exception_edge_block_into_one_op_segments() {
    let source = r#"
def f():
    a = 1
    b = 2
    return b
"#;
    let mut module = tracked_name_binding_module(source);
    let function = module
        .callable_defs
        .iter_mut()
        .find(|function| function.names.qualname == "f")
        .expect("must contain f");
    let block_index = function
        .blocks
        .iter()
        .position(|block| block.body.len() >= 2)
        .expect("must contain multi-op block");
    let original_label = function.blocks[block_index].label.clone();
    let except_label = BlockLabel::from_index(100);
    function.blocks.push(ResolvedStorageBlock {
        label: except_label.clone(),
        body: vec![],
        term: BlockTerm::<InstrResolved>::Return(InstrResolved::constant_none()),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    });
    function.blocks[block_index].exc_edge = Some(BlockEdge::new(except_label.clone()));
    function.blocks[block_index].set_exception_param("_dp_try_exc_split");
    function
        .storage_layout
        .as_mut()
        .unwrap()
        .ensure_stack_slot("_dp_try_exc_split");

    let lowered = lower_try_jump_exception_flow(&module);
    let lowered_function = lowered
        .callable_defs
        .iter()
        .find(|candidate| candidate.names.qualname == "f")
        .expect("must contain lowered f");

    let first = lowered_function
        .blocks
        .iter()
        .find(|block| block.label == original_label)
        .expect("split must keep original block label");
    assert_eq!(first.body.len(), 1, "first split block must contain one op");
    assert!(
        matches!(first.term, BlockTerm::Jump(_)),
        "split op block must jump to next split block"
    );
    assert_eq!(
        first.exc_edge.as_ref().map(|edge| edge.target),
        Some(except_label),
        "split block must preserve exception edge target"
    );

    let split_tail = lowered_function
        .blocks
        .iter()
        .find(|block| block.label != original_label && block.label != except_label)
        .expect("must contain split tail block");
    assert!(
        split_tail.body.len() <= 1,
        "split tail block should not aggregate ops"
    );
}

#[test]
fn keeps_pure_expr_ops_grouped_until_local_state_changes() {
    let source = r#"
def f():
    x()
    y()
    z = 1
    w()
"#;
    let mut module = tracked_name_binding_module(source);
    let function = module
        .callable_defs
        .iter_mut()
        .find(|function| function.names.qualname == "f")
        .expect("must contain f");
    let block_index = function
        .blocks
        .iter()
        .position(|block| block.body.len() >= 4)
        .expect("must contain multi-op block");
    let original_label = function.blocks[block_index].label.clone();
    let except_label = BlockLabel::from_index(100);
    function.blocks.push(ResolvedStorageBlock {
        label: except_label.clone(),
        body: vec![],
        term: BlockTerm::<InstrResolved>::Return(InstrResolved::constant_none()),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    });
    function.blocks[block_index].exc_edge = Some(BlockEdge::new(except_label.clone()));
    function.blocks[block_index].set_exception_param("_dp_try_exc_group");
    function
        .storage_layout
        .as_mut()
        .unwrap()
        .ensure_stack_slot("_dp_try_exc_group");

    let lowered = lower_try_jump_exception_flow(&module);
    let lowered_function = lowered
        .callable_defs
        .iter()
        .find(|candidate| candidate.names.qualname == "f")
        .expect("must contain lowered f");

    let first = lowered_function
        .blocks
        .iter()
        .find(|block| block.label == original_label)
        .expect("lowered entry block must exist");
    assert_eq!(
        first.body.len(),
        3,
        "pure expr ops should remain grouped until the local assignment"
    );
    assert!(
        matches!(first.term, BlockTerm::Jump(_)),
        "state-changing assignment should still split the block"
    );

    let next = lowered_function
        .blocks
        .iter()
        .find(|block| block.label != original_label && block.label != except_label)
        .expect("must contain split successor");
    assert_eq!(
        next.body.len(),
        1,
        "ops after the assignment should start a new segment"
    );
}

#[test]
fn preserves_value_return_after_plain_try_except() {
    let source = r#"
def f():
    try:
        pass
    except Exception:
        pass
    return 1
"#;
    let module = tracked_name_binding_module(source);
    let raw_function = module
        .callable_defs
        .iter()
        .find(|candidate| candidate.names.qualname == "f")
        .expect("must contain raw f");
    assert!(
        raw_function
            .blocks
            .iter()
            .any(|block| is_return_of_number_constant(&block.term)),
        "{raw_function:#?}"
    );
    let lowered = lower_try_jump_exception_flow(&module);
    let lowered_function = lowered
        .callable_defs
        .iter()
        .find(|candidate| candidate.names.qualname == "f")
        .expect("must contain lowered f");

    assert!(
        lowered_function
            .blocks
            .iter()
            .any(|block| is_return_of_number_constant(&block.term)),
        "{lowered_function:#?}"
    );
}

// Model only the ownership of declared transport slots, not Python values or
// exception matching. A join keeps every possible owned/unbound state. Explicit
// edge arguments bind the destination slots; implicit same-name edges preserve
// them. This makes a missing retirement visible without relying on temp names.
#[derive(Clone, Default)]
struct ExceptionTransportState {
    owned: std::collections::BTreeSet<String>,
    unbound: std::collections::BTreeSet<String>,
}

fn exception_transport_names(
    function: &crate::block_py::BlockPyFunction<crate::passes::ResolvedStorageModuleShape>,
) -> std::collections::BTreeSet<String> {
    function
        .blocks
        .iter()
        .flat_map(|block| &block.params)
        .filter(|param| {
            matches!(
                param.role,
                BlockParamRole::Exception
                    | BlockParamRole::EnclosingException
                    | BlockParamRole::AbruptPayload
            )
        })
        .map(|param| param.name.clone())
        .collect()
}

struct ExceptionTransportStep<'a> {
    tracked: &'a std::collections::BTreeSet<String>,
    state: &'a mut ExceptionTransportState,
    check_reads: bool,
}

impl crate::block_py::Visit<InstrResolved> for ExceptionTransportStep<'_> {
    fn visit_instr(&mut self, instr: &InstrResolved) {
        use crate::block_py::{ChildVisitable, NameLike};
        match instr {
            InstrResolved::Load(load) => {
                if self.check_reads && self.tracked.contains(load.name.id.as_str()) {
                    assert!(
                        !self.state.unbound.contains(load.name.id.as_str()),
                        "transport {} was retired before its last read",
                        load.name.id
                    );
                }
            }
            InstrResolved::Store(store) => {
                self.visit_instr(&store.value);
                let name = store.name.id.as_str();
                if self.tracked.contains(name) {
                    let owns_value = match store.value.as_ref() {
                        InstrResolved::Load(load) if load.name.is_runtime_symbol("NONE") => false,
                        InstrResolved::Load(load)
                            if self.tracked.contains(load.name.id.as_str()) =>
                        {
                            self.state.owned.contains(load.name.id.as_str())
                        }
                        _ => true,
                    };
                    self.state.unbound.remove(name);
                    self.state.owned.remove(name);
                    if owns_value {
                        self.state.owned.insert(name.into());
                    }
                }
            }
            InstrResolved::Del(del) => {
                let name = del.name.id.as_str();
                if self.tracked.contains(name) {
                    self.state.owned.remove(name);
                    self.state.unbound.insert(name.into());
                }
            }
            _ => instr.visit_children(self),
        }
    }
}

fn exception_transport_inputs(
    function: &crate::block_py::BlockPyFunction<crate::passes::ResolvedStorageModuleShape>,
    tracked: &std::collections::BTreeSet<String>,
) -> std::collections::HashMap<BlockLabel, ExceptionTransportState> {
    use crate::block_py::Visit;
    use std::collections::{HashMap, VecDeque};
    let blocks: HashMap<_, _> = function
        .blocks
        .iter()
        .map(|block| (block.label, block))
        .collect();
    let entry = function.entry_block().label;
    let mut inputs = HashMap::from([(
        entry,
        ExceptionTransportState {
            owned: Default::default(),
            unbound: tracked.clone(),
        },
    )]);
    let mut work = VecDeque::from([entry]);
    while let Some(label) = work.pop_front() {
        let block = blocks[&label];
        let mut state = inputs[&label].clone();
        let mut propagate = |edge: &BlockEdge, source: &ExceptionTransportState| {
            let mut incoming = source.clone();
            for (param, arg) in blocks[&edge.target].params.iter().zip(&edge.args) {
                if !tracked.contains(&param.name) {
                    continue;
                }
                incoming.owned.remove(&param.name);
                incoming.unbound.remove(&param.name);
                match arg {
                    BlockArg::Name(name) if tracked.contains(name) => {
                        if source.owned.contains(name) {
                            incoming.owned.insert(param.name.clone());
                        }
                        if source.unbound.contains(name) {
                            incoming.unbound.insert(param.name.clone());
                        }
                    }
                    BlockArg::Name(_) | BlockArg::CurrentException => {
                        incoming.owned.insert(param.name.clone());
                    }
                    BlockArg::None | BlockArg::AbruptKind(_) => {}
                }
            }
            if let Some(previous) = inputs.get_mut(&edge.target) {
                let old = (previous.owned.len(), previous.unbound.len());
                previous.owned.extend(incoming.owned);
                previous.unbound.extend(incoming.unbound);
                if old != (previous.owned.len(), previous.unbound.len()) {
                    work.push_back(edge.target);
                }
            } else {
                inputs.insert(edge.target, incoming);
                work.push_back(edge.target);
            }
        };
        for instr in &block.body {
            if let Some(edge) = &block.exc_edge {
                propagate(edge, &state);
            }
            ExceptionTransportStep {
                tracked,
                state: &mut state,
                check_reads: false,
            }
            .visit_instr(instr);
        }
        if let Some(edge) = &block.exc_edge {
            propagate(edge, &state);
        }
        ExceptionTransportStep {
            tracked,
            state: &mut state,
            check_reads: false,
        }
        .visit_term(&block.term);
        match &block.term {
            BlockTerm::Jump(edge) => propagate(edge, &state),
            BlockTerm::IfTerm(branch) => {
                for target in [branch.then_label, branch.else_label] {
                    propagate(&BlockEdge::new(target), &state);
                }
            }
            BlockTerm::BranchTable(branch) => {
                for target in branch.targets.iter().chain([&branch.default_label]) {
                    propagate(&BlockEdge::new(*target), &state);
                }
            }
            BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) | BlockTerm::Raise(_) => {}
        }
    }
    inputs
}

#[test]
fn exception_transport_retirement_precedes_the_original_cancellation_observer() {
    use crate::block_py::{instr_any, Visit};
    let module =
        lower_try_jump_exception_flow(&tracked_name_binding_module(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/integration_modules/taskgroup_propagate_cancellation_refcycle.py"
        ))));
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.fn_name == "run")
        .unwrap();
    let transports = exception_transport_names(function);
    assert!(!transports.is_empty());
    assert!(
        !transports.contains("exc"),
        "the explicit cause alias is source-owned"
    );
    assert!(
        !transports.contains("err"),
        "except-as is not the hidden exception transport"
    );
    let inputs = exception_transport_inputs(function, &transports);
    let observes_references = |instr: &InstrResolved| {
        instr_any(instr, |instr| {
            matches!(instr, InstrResolved::GetAttr(attr)
                if matches!(attr.value.as_ref(), InstrResolved::Load(load) if load.name.id.as_str() == "gc"))
        })
    };
    let mut observations = Vec::new();
    for block in &function.blocks {
        let Some(mut state) = inputs.get(&block.label).cloned() else {
            continue;
        };
        for (index, instr) in block.body.iter().enumerate() {
            if observes_references(instr) {
                observations.push((block.label, Some(index)));
                assert!(
                    state.owned.is_empty(),
                    "compiler-owned exception transports survive beyond their handlers: {:?}",
                    state.owned
                );
            }
            ExceptionTransportStep {
                tracked: &transports,
                state: &mut state,
                check_reads: true,
            }
            .visit_instr(instr);
        }
        if let BlockTerm::Return(value) = &block.term {
            if observes_references(value) {
                observations.push((block.label, None));
                assert!(
                    state.owned.is_empty(),
                    "compiler-owned exception transports survive beyond their handlers: {:?}",
                    state.owned
                );
            }
        }
    }
    assert!(
        !observations.is_empty(),
        "the original validator must observe references before run returns"
    );
    // Control-flow lowering can duplicate a source continuation. Every copy
    // above must retire its owners; the number of copies is not the contract.
    eprintln!("checked cancellation observer sites: {observations:?}");
}

#[test]
fn exception_transport_retirement_keeps_finally_payloads_and_source_aliases_live() {
    use crate::block_py::{instr_any, Visit};
    let module = lower_try_jump_exception_flow(&tracked_name_binding_module(
        r#"
def keep_alias(error, observe):
    _dp_try_exc_kept = error
    try:
        raise error
    except Exception as user_error:
        _dp_try_exc_kept = user_error
    observe(_dp_try_exc_kept)
    return _dp_try_exc_kept

def return_finally(value, observe):
    try:
        try:
            return value
        finally:
            observe(value)
    finally:
        observe(value)

def raise_finally(error, observe):
    try:
        try:
            raise error
        finally:
            observe(error)
    finally:
        observe(error)
"#,
    ));
    for name in ["keep_alias", "return_finally", "raise_finally"] {
        let function = module
            .callable_defs
            .iter()
            .find(|function| function.names.fn_name == name)
            .unwrap();
        let mut tracked = exception_transport_names(function);
        assert!(
            !tracked.contains("_dp_try_exc_kept"),
            "spelling cannot grant a transport role"
        );
        if name == "keep_alias" {
            tracked.insert("_dp_try_exc_kept".into());
        } else {
            assert!(function
                .blocks
                .iter()
                .flat_map(|block| &block.params)
                .any(|param| param.role == BlockParamRole::AbruptPayload));
        }
        let inputs = exception_transport_inputs(function, &tracked);
        let mut observations = 0;
        for block in &function.blocks {
            let Some(mut state) = inputs.get(&block.label).cloned() else {
                continue;
            };
            let mut step = ExceptionTransportStep {
                tracked: &tracked,
                state: &mut state,
                check_reads: true,
            };
            for instr in &block.body {
                if instr_any(instr, |instr| {
                    matches!(instr, InstrResolved::Call(call)
                    if matches!(call.func.as_ref(), InstrResolved::Load(load) if load.name.id.as_str() == "observe"))
                }) {
                    observations += 1;
                    if name == "keep_alias" {
                        assert!(step.state.owned.contains("_dp_try_exc_kept"));
                    }
                }
                step.visit_instr(instr);
            }
            step.visit_term(&block.term);
        }
        assert!(
            observations > 0,
            "the source/finally observer must remain reachable"
        );
    }
}

#[test]
fn pending_return_payloads_survive_the_finally_body_and_retire_on_override() {
    use crate::block_py::{instr_any, Visit};
    let module = lower_try_jump_exception_flow(&tracked_name_binding_module(
        r#"
def return_override(value, replacement, observe_inside, observe_after):
    try:
        return value
    finally:
        observe_inside()
        return replacement

def raise_override(value, error, observe_inside, observe_after):
    try:
        return value
    finally:
        observe_inside()
        raise error

def break_override(value, observe_inside, observe_after):
    for unused in (0,):
        try:
            return value
        finally:
            observe_inside()
            break
    observe_after()
"#,
    ));
    for name in ["return_override", "raise_override", "break_override"] {
        let function = module
            .callable_defs
            .iter()
            .find(|function| function.names.fn_name == name)
            .unwrap();
        let tracked = exception_transport_names(function);
        let payloads = function
            .blocks
            .iter()
            .flat_map(|block| &block.params)
            .filter(|param| param.role == BlockParamRole::AbruptPayload)
            .map(|param| param.name.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!payloads.is_empty());
        let inputs = exception_transport_inputs(function, &tracked);
        let mut inside = 0;
        let mut after = 0;
        for block in &function.blocks {
            let Some(mut state) = inputs.get(&block.label).cloned() else {
                continue;
            };
            for instr in &block.body {
                for observer in ["observe_inside", "observe_after"] {
                    if instr_any(instr, |instr| {
                        matches!(instr, InstrResolved::Call(call)
                        if matches!(call.func.as_ref(), InstrResolved::Load(load) if load.name.id.as_str() == observer))
                    }) {
                        let pending = state.owned.intersection(&payloads).collect::<Vec<_>>();
                        if observer == "observe_inside" {
                            inside += 1;
                            assert!(
                                !pending.is_empty(),
                                "{name} retired its pending return before the finally body"
                            );
                        } else {
                            after += 1;
                            assert!(
                                pending.is_empty(),
                                "{name} kept a discarded return after finally: {pending:?}"
                            );
                        }
                    }
                }
                ExceptionTransportStep {
                    tracked: &tracked,
                    state: &mut state,
                    check_reads: true,
                }
                .visit_instr(instr);
            }
        }
        assert!(inside > 0, "{name} must exercise a finally observer");
        if name == "break_override" {
            assert!(after > 0);
        }
    }
}

#[test]
fn exception_transport_retirement_addresses_local_and_saved_owners_independently() {
    use super::transport_storage::{TransportLocation, TransportStorage};
    use crate::block_py::{FunctionKind, NameLike, Visit};
    let mut module = tracked_name_binding_module(
        "def gen(factory, observe):\n    try:\n        raise factory()\n    except ValueError:\n        yield 1\n    observe()\n    yield 2\n",
    );
    let function = module
        .callable_defs
        .iter_mut()
        .find(|function| function.kind == FunctionKind::Generator)
        .unwrap();
    let logical = function
        .blocks
        .iter()
        .find_map(|block| block.exception_param())
        .expect("the handler has an explicit transport")
        .to_owned();
    let saved = function
        .storage_layout
        .as_mut()
        .unwrap()
        .preserved_slots
        .iter_mut()
        .find(|slot| slot.logical_name == logical)
        .expect("handler survives suspension");
    // Logical identity and storage spelling are independent. The consumer
    // must still select this same physical owner after a legal slot rename.
    saved.storage_name = "distinct_saved_exception_storage".into();
    let storage = TransportStorage::new(function);
    let owners = storage
        .for_logical(&logical)
        .collect::<std::collections::BTreeSet<_>>();
    assert!(owners
        .iter()
        .any(|owner| matches!(owner, TransportLocation::Local(_))));
    assert!(owners
        .iter()
        .any(|owner| matches!(owner, TransportLocation::Preserved(_))));
    let function_id = function.function_id;
    let lowered = lower_try_jump_exception_flow(&module);
    let function = lowered
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .unwrap();
    struct Clears<'a> {
        storage: &'a TransportStorage,
        cleared: std::collections::BTreeSet<TransportLocation>,
    }
    impl Visit<InstrResolved> for Clears<'_> {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            use crate::block_py::ChildVisitable;
            if let InstrResolved::Store(store) = instr {
                if matches!(store.value.as_ref(), InstrResolved::Load(load) if load.name.is_runtime_symbol("NONE"))
                {
                    self.cleared.extend(self.storage.key(&store.name));
                }
            }
            instr.visit_children(self);
        }
    }
    let mut clears = Clears {
        storage: &storage,
        cleared: Default::default(),
    };
    for block in &function.blocks {
        for instr in &block.body {
            clears.visit_instr(instr);
        }
    }
    assert!(
        owners.is_subset(&clears.cleared),
        "each physical owner must retire, not just one same-name alias"
    );
    assert!(
        function
            .blocks
            .iter()
            .all(|block| block.extra.suspension_resume.is_none()),
        "semantic resume metadata is consumed before optimization"
    );
}

#[test]
fn exception_transport_copy_aliases_retire_after_physical_renaming_and_resume_cycles() {
    use super::transport_storage::{TransportLocation, TransportStorage};
    use crate::block_py::{ChildVisitable, FunctionKind, NameLike, Visit, VisitMut};

    let mut module = tracked_name_binding_module(
        "def gen(factory, observe):\n    try:\n        raise factory()\n    except ValueError:\n        while True:\n            yield 1\n            observe()\n",
    );
    let function = module
        .callable_defs
        .iter_mut()
        .find(|function| function.kind == FunctionKind::Generator)
        .unwrap();
    let aliases = block_parameter_transport_local_aliases(function);
    assert!(
        !aliases.is_empty(),
        "resume arguments must own explicit local copies"
    );
    let alias = aliases[0].clone();
    let NameLocation::Local(slot) = alias.location else {
        unreachable!()
    };
    let renamed = "ordinary_spelling_for_a_declared_transport";
    struct Rename {
        slot: crate::block_py::LocalLocation,
        previous: String,
        replacement: String,
    }
    impl VisitMut<InstrResolved> for Rename {
        fn visit_instr_mut(&mut self, instr: &mut InstrResolved) {
            let name = match instr {
                InstrResolved::Load(load) => Some(&mut load.name),
                InstrResolved::Store(store) => Some(&mut store.name),
                InstrResolved::Del(del) => Some(&mut del.name),
                _ => None,
            };
            if let Some(name) = name {
                if name.location == NameLocation::Local(self.slot) {
                    name.id = self.replacement.clone().into();
                }
            }
            crate::block_py::walk_expr_mut(self, instr);
        }
        fn visit_block_arg_mut(&mut self, arg: &mut BlockArg) {
            if let BlockArg::Name(name) = arg {
                if *name == self.previous {
                    *name = self.replacement.clone();
                }
            }
        }
    }
    Rename {
        slot,
        previous: alias.id.to_string(),
        replacement: renamed.into(),
    }
    .visit_fn_mut(function);
    function.storage_layout.as_mut().unwrap().stack_slots[slot.slot() as usize] = renamed.into();
    let storage = TransportStorage::new(function);
    assert_eq!(
        storage.parameter(renamed),
        Some(TransportLocation::Local(slot.slot()))
    );
    let id = function.function_id;
    let lowered = lower_try_jump_exception_flow(&module);
    let function = lowered
        .callable_defs
        .iter()
        .find(|function| function.function_id == id)
        .unwrap();
    struct AliasWrites {
        slot: crate::block_py::LocalLocation,
        cleared: usize,
        copied: usize,
    }
    impl Visit<InstrResolved> for AliasWrites {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            if let InstrResolved::Store(store) = instr {
                if store.name.location == NameLocation::Local(self.slot) {
                    if matches!(store.value.as_ref(), InstrResolved::Load(load) if load.name.is_runtime_symbol("NONE"))
                    {
                        self.cleared += 1;
                    } else {
                        self.copied += 1;
                    }
                }
            }
            instr.visit_children(self);
        }
    }
    let mut writes = AliasWrites {
        slot,
        cleared: 0,
        copied: 0,
    };
    writes.visit_fn(function);
    assert!(
        writes.cleared > 0,
        "a dead incoming argument must still be initialized to None"
    );
    assert_eq!(
        writes.copied, 0,
        "copy/resume cycles cannot create a semantic read of the caught object"
    );
}

fn block_parameter_transport_local_aliases(
    function: &crate::block_py::BlockPyFunction<crate::passes::ResolvedStorageModuleShape>,
) -> Vec<crate::block_py::ResolvedName> {
    use super::transport_storage::TransportStorage;
    use crate::block_py::{ChildVisitable, ResolvedName, Visit};
    struct Copies<'a> {
        storage: &'a TransportStorage,
        aliases: Vec<ResolvedName>,
    }
    impl Visit<InstrResolved> for Copies<'_> {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            if let InstrResolved::Store(store) = instr {
                if matches!(store.name.location, NameLocation::Local(_))
                    && self.storage.copy_source(store).is_some()
                {
                    self.aliases.push(store.name.clone());
                }
            }
            instr.visit_children(self);
        }
    }
    let storage = TransportStorage::new(function);
    let mut copies = Copies {
        storage: &storage,
        aliases: Vec::new(),
    };
    copies.visit_fn(function);
    copies.aliases
}

#[test]
fn exception_transport_copies_keep_exception_group_reads_and_source_owned_aliases() {
    use super::transport_storage::TransportStorage;
    use crate::block_py::{ChildVisitable, FunctionKind, RaiseDisposition, StorePurpose, Visit};
    use std::collections::{HashMap, HashSet, VecDeque};
    let module = tracked_name_binding_module(
        "def group():\n    try:\n        raise ExceptionGroup('g', [ValueError(), TypeError()])\n    except* ValueError:\n        yield 1\n\ndef alias(factory, observe):\n    try:\n        raise factory()\n    except ValueError as user_error:\n        yield 1\n        observe(user_error)\n\ndef pending(value):\n    try:\n        return value\n    finally:\n        yield 1\n",
    );
    let group = module
        .callable_defs
        .iter()
        .find(|function| {
            function.kind == FunctionKind::Generator && function.names.fn_name == "group"
        })
        .unwrap();
    let group_id = group.function_id;
    // Except-star rewriting keeps the remaining subgroup in an independent
    // ordinary binding. Locate the actual post-resume source-raise operand,
    // not a similarly named hidden handler parameter.
    let blocks = group
        .blocks
        .iter()
        .map(|block| (block.label, block))
        .collect::<HashMap<_, _>>();
    let mut pending_labels = group
        .blocks
        .iter()
        .filter_map(|block| block.extra.suspension_resume)
        .collect::<VecDeque<_>>();
    let mut reachable = HashSet::new();
    while let Some(label) = pending_labels.pop_front() {
        if !reachable.insert(label) {
            continue;
        }
        let block = blocks[&label];
        match &block.term {
            BlockTerm::Jump(edge) => pending_labels.push_back(edge.target),
            BlockTerm::IfTerm(branch) => {
                pending_labels.extend([branch.then_label, branch.else_label])
            }
            BlockTerm::BranchTable(branch) => {
                pending_labels.extend(branch.targets.iter().copied().chain([branch.default_label]))
            }
            BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) | BlockTerm::Raise(_) => {}
        }
        if let Some(edge) = &block.exc_edge {
            pending_labels.push_back(edge.target);
        }
    }
    #[derive(Default)]
    struct SavedReads(Vec<crate::block_py::ResolvedName>);
    impl Visit<InstrResolved> for SavedReads {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            if let InstrResolved::Load(load) = instr {
                if matches!(load.name.location, NameLocation::Preserved(_)) {
                    self.0.push(load.name.clone());
                }
            }
            instr.visit_children(self);
        }
    }
    let mut required = SavedReads::default();
    for block in &group.blocks {
        if !reachable.contains(&block.label) {
            continue;
        }
        if let BlockTerm::Raise(raised) = &block.term {
            if raised.disposition == RaiseDisposition::Source {
                if let Some(value) = &raised.exc {
                    required.visit_instr(value);
                }
            }
        }
    }
    assert!(
        !required.0.is_empty(),
        "the remaining subgroup is read after resumption"
    );
    let group_storage = TransportStorage::new(group);
    assert!(
        required
            .0
            .iter()
            .all(|name| group_storage.key(name).is_none()),
        "the actual subgroup remainder is independently owned, not a handler transport"
    );
    let pending = module
        .callable_defs
        .iter()
        .find(|function| {
            function.kind == FunctionKind::Generator && function.names.fn_name == "pending"
        })
        .unwrap();
    let pending_id = pending.function_id;
    let pending_copies = block_parameter_transport_local_aliases(pending);
    assert!(
        !pending_copies.is_empty(),
        "a pending return needs a saved incoming payload"
    );
    let alias = module
        .callable_defs
        .iter()
        .find(|function| {
            function.kind == FunctionKind::Generator && function.names.fn_name == "alias"
        })
        .unwrap();
    let alias_storage = TransportStorage::new(alias);
    assert!(
        alias_storage.for_logical("user_error").next().is_none(),
        "an except-as source binding does not acquire the hidden transport's lifetime"
    );
    let layout = alias.storage_layout.as_ref().unwrap();
    assert!(
        layout
            .preserved_slots
            .iter()
            .any(|slot| slot.logical_name == "user_error"),
        "the explicit user alias must survive suspension independently"
    );
    let lowered = lower_try_jump_exception_flow(&module);
    let group = lowered
        .callable_defs
        .iter()
        .find(|function| function.function_id == group_id)
        .unwrap();
    let mut surviving = SavedReads::default();
    surviving.visit_fn(group);
    assert!(
        required.0.iter().all(|expected| surviving
            .0
            .iter()
            .any(|actual| actual.location == expected.location)),
        "the actual post-yield remainder read must survive transport retirement"
    );
    let pending = lowered
        .callable_defs
        .iter()
        .find(|function| function.function_id == pending_id)
        .unwrap();
    struct Copies<'a> {
        aliases: &'a [crate::block_py::ResolvedName],
        copied: usize,
    }
    impl Visit<InstrResolved> for Copies<'_> {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            if let InstrResolved::Store(store) = instr {
                assert_eq!(
                    store.purpose,
                    StorePurpose::Binding,
                    "copy purpose must be consumed before optimization"
                );
                if self
                    .aliases
                    .iter()
                    .any(|alias| alias.location == store.name.location)
                    && matches!(store.value.as_ref(), InstrResolved::Load(load)
                        if matches!(load.name.location, NameLocation::Preserved(_)))
                {
                    self.copied += 1;
                }
            }
            instr.visit_children(self);
        }
    }
    let mut reads = Copies {
        aliases: &pending_copies,
        copied: 0,
    };
    reads.visit_fn(pending);
    assert!(
        reads.copied > 0,
        "a pending return is still copied for its post-yield completion"
    );
}

#[test]
fn pending_error_propagation_is_independent_of_suspended_activation_shutdown() {
    use crate::block_py::{FunctionKind, HandledExceptionContext, RaiseDisposition};
    let module = tracked_blockpy_module(
        r#"
def ordinary(value, observe):
    try:
        return value
    finally:
        observe()

def source_raise(error):
    raise error

def pending_error(error, observe):
    try:
        raise error
    finally:
        observe()

def suspended(error):
    try:
        yield 1
    finally:
        raise error
"#,
    );
    let ordinary = module
        .callable_defs
        .iter()
        .find(|function| function.names.fn_name == "ordinary")
        .unwrap();
    assert!(
        ordinary
            .blocks
            .iter()
            .all(|block| block.extra.handled_exception != HandledExceptionContext::Terminal),
        "ordinary normalized error propagation must remain composable with an inlined caller"
    );
    assert!(ordinary
        .blocks
        .iter()
        .any(
            |block| block.extra.handled_exception == HandledExceptionContext::Unwind
                && matches!(&block.term, BlockTerm::Raise(raise)
            if raise.disposition == RaiseDisposition::PropagateNormalized && raise.exc.is_some())
        ));
    let source = module
        .callable_defs
        .iter()
        .find(|function| function.names.fn_name == "source_raise")
        .unwrap();
    assert!(source.blocks.iter().any(|block|
        matches!(&block.term, BlockTerm::Raise(raise) if raise.disposition == RaiseDisposition::Source)),
        "a source raise still performs its source normalization and context chaining");
    let pending = module
        .callable_defs
        .iter()
        .find(|function| function.names.fn_name == "pending_error")
        .unwrap();
    assert!(pending.blocks.iter().any(|block|
        block.extra.handled_exception == HandledExceptionContext::Regions
        && matches!(&block.term, BlockTerm::Raise(raise)
            if raise.disposition == RaiseDisposition::PropagateNormalized)),
        "finally forwards the already-raised error without chaining through a replacement current handler");
    let suspended = module
        .callable_defs
        .iter()
        .find(|function| function.kind == FunctionKind::Generator)
        .unwrap();
    assert!(
        suspended
            .blocks
            .iter()
            .any(
                |block| block.extra.handled_exception == HandledExceptionContext::Terminal
                    && matches!(&block.term, BlockTerm::Raise(raise)
            if raise.disposition == RaiseDisposition::PropagateNormalized)
            ),
        "suspended terminal propagation still detaches its distinct owned activation"
    );
}
