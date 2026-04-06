use super::lower_try_jump_exception_flow;
use crate::block_py::{
    validate_module, AbruptKind, BindingKind, BlockArg, BlockEdge, BlockLabel, BlockParam,
    BlockParamRole, BlockTerm, CellBindingKind, CodegenBlock, Instr, InstrCodegen, InstrResolved,
    Literal, NameLocation, NumberLiteral, NumberLiteralValue, ResolvedStorageBlock, StorageLayout,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::passes::CodegenModuleShape;

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

fn tracked_codegen_module(source: &str) -> crate::block_py::BlockPyModule<CodegenModuleShape> {
    let name_binding = tracked_name_binding_module(source);
    let lowered = lower_try_jump_exception_flow(&name_binding);
    let mut codegen = crate::passes::normalize_bb_module_strings(&lowered);
    crate::passes::relabel_dense_bb_module(&mut codegen);
    codegen
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

        function.blocks.push(ResolvedStorageBlock {
            label: body_label.clone(),
            body: vec![],
            term: BlockTerm::<InstrResolved>::Return(
                InstrResolved::constant_none(),
            ),
            params: vec![crate::block_py::BlockParam {
                name: "_dp_try_exc_manual".to_string(),
                role: crate::block_py::BlockParamRole::Exception,
            }],
            exc_edge: Some(BlockEdge::new(except_label.clone())),
        });
        function.blocks.push(ResolvedStorageBlock {
            label: except_label.clone(),
            body: vec![],
            term: BlockTerm::<InstrResolved>::Return(
                InstrResolved::constant_none(),
            ),
            params: Vec::new(),
            exc_edge: None,
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
    let mut module = tracked_codegen_module(source);
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
    let mut module = tracked_codegen_module(source);
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
    let mut module = tracked_codegen_module(source);
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
    let mut module = tracked_codegen_module(source);
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
    let mut module = tracked_codegen_module(source);
    let function = module
        .callable_defs
        .first_mut()
        .expect("must contain function");
    function.blocks[0].set_exception_param("_dp_yield_from_exc");
    let target = BlockLabel::from_index(function.blocks.len());
    function.blocks.push(CodegenBlock {
        label: target,
        body: vec![],
        term: BlockTerm::<InstrCodegen>::Return(
            InstrCodegen::constant_none(),
        ),
        params: vec![BlockParam {
            name: "_dp_try_exc".to_string(),
            role: BlockParamRole::Exception,
        }],
        exc_edge: None,
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
    let mut module = tracked_codegen_module(source);
    let function = module
        .callable_defs
        .first_mut()
        .expect("must contain function");
    function.storage_layout = Some(StorageLayout {
        freevars: vec![],
        cellvars: vec![crate::block_py::ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_wrong_storage".to_string(),
            init: crate::block_py::ClosureInit::Deferred,
        }],
        runtime_cells: vec![],
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
        term: BlockTerm::<InstrResolved>::Return(
            InstrResolved::constant_none(),
        ),
        params: Vec::new(),
        exc_edge: None,
    });
    function.blocks[block_index].exc_edge = Some(BlockEdge::new(except_label.clone()));
    function.blocks[block_index].set_exception_param("_dp_try_exc_split");

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
        term: BlockTerm::<InstrResolved>::Return(
            InstrResolved::constant_none(),
        ),
        params: Vec::new(),
        exc_edge: None,
    });
    function.blocks[block_index].exc_edge = Some(BlockEdge::new(except_label.clone()));
    function.blocks[block_index].set_exception_param("_dp_try_exc_group");

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
