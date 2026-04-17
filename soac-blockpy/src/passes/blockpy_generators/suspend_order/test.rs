use super::*;
use crate::block_py::{
    BinOp, BinOpKind, Block, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm,
    CallArgPositional, CallableScopeInfo, FunctionKind, FunctionName, InstrWithAwaitAndYield,
    InstrWithYield, Meta, ModuleNameGen, RuntimeFunctionId, Store, UnresolvedName, WithMeta,
    YieldFrom,
};
use crate::passes::core_await_lower::lower_awaits_in_core_blockpy_module;
use crate::passes::{CoreModuleShapeWithAwaitAndYield, CoreModuleShapeWithYield};
use ruff_python_ast as ast;

fn test_name(id: &str) -> UnresolvedName {
    let ast::Expr::Name(expr) = crate::py_expr!("{id:id}", id = id) else {
        unreachable!();
    };
    expr.id.into()
}

fn is_name_like(expr: &InstrWithYield) -> bool {
    matches!(expr, InstrWithYield::Load(_))
}

fn test_name_gen() -> crate::block_py::FunctionNameGen {
    ModuleNameGen::new(0).next_function_name_gen()
}

fn test_callable_def_with_yield_block(
    block: Block<InstrWithYield>,
) -> BlockPyFunction<CoreModuleShapeWithYield> {
    BlockPyFunction {
        function_id: RuntimeFunctionId::new(0, 1),
        name_gen: test_name_gen(),
        names: FunctionName::new("f", "f", "f", "f"),
        kind: FunctionKind::Generator,
        execution_mode: Default::default(),
        params: Default::default(),
        blocks: vec![block],
        doc: None,
        storage_layout: None,
        scope: CallableScopeInfo::default(),
    }
}

fn lower_awaits_in_test_block(block: Block<InstrWithAwaitAndYield>) -> Block<InstrWithYield> {
    let lowered = lower_awaits_in_core_blockpy_module(BlockPyModule {
        module_name_gen: ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: vec![BlockPyFunction::<CoreModuleShapeWithAwaitAndYield> {
            function_id: RuntimeFunctionId::new(0, 1),
            name_gen: test_name_gen(),
            names: FunctionName::new("f", "f", "f", "f"),
            kind: FunctionKind::Coroutine,
            execution_mode: Default::default(),
            params: Default::default(),
            blocks: vec![block],
            doc: None,
            storage_layout: None,
            scope: CallableScopeInfo::default(),
        }],
        counter_defs: Vec::new(),
        module_constants: Vec::new(),
    });
    lowered.callable_defs[0].blocks[0].clone()
}

fn lower_yield_block(block: Block<InstrWithAwaitAndYield>) -> Block<InstrWithYield> {
    make_suspend_order_explicit_in_core_callable_def(test_callable_def_with_yield_block(
        lower_awaits_in_test_block(block),
    ))
    .blocks
    .into_iter()
    .next()
    .expect("test callable should have one block")
}

#[test]
fn eval_order_hoists_call_arguments_in_return_value_to_temps() {
    let block = Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!(
            "f(g(x), h(y))"
        ))),
        params: Vec::new(),
        exc_edge: None,
    };

    let lowered = lower_yield_block(block);
    assert!(lowered.body.is_empty());
    let BlockTerm::Return(InstrWithYield::Call(call)) = &lowered.term else {
        panic!("expected call expr");
    };
    assert!(is_name_like(call.func.as_ref()));
    assert!(matches!(
        &call.args[0],
        CallArgPositional::Positional(InstrWithYield::Call(_))
    ));
    assert!(matches!(
        &call.args[1],
        CallArgPositional::Positional(InstrWithYield::Call(_))
    ));
}

#[test]
fn eval_order_hoists_return_value_to_temp() {
    let block = Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!(
            "f(g(x))"
        ))),
        params: Vec::new(),
        exc_edge: None,
    };

    let lowered = lower_yield_block(block);
    assert!(lowered.body.is_empty());
    let BlockTerm::Return(InstrWithYield::Call(call)) = lowered.term else {
        panic!("expected return of recursive call");
    };
    assert!(is_name_like(call.func.as_ref()));
}

#[test]
fn eval_order_hoists_nested_call_in_assignment_rhs() {
    let block = Block {
        label: BlockLabel::from_index(0),
        body: vec![Store::new(
            test_name("tmp"),
            Box::new(InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!(
                "f(g(x))"
            ))),
        )
        .into()],
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!(
            "__dp_NONE"
        ))),
        params: Vec::new(),
        exc_edge: None,
    };

    let lowered = lower_yield_block(block);
    assert_eq!(lowered.body.len(), 1);
    let InstrWithYield::Store(assign) = &lowered.body[0] else {
        panic!("expected rewritten temp store");
    };
    let InstrWithYield::Call(call) = assign.value.as_ref() else {
        panic!("expected outer call");
    };
    assert!(is_name_like(call.func.as_ref()));
    assert!(matches!(
        &call.args[0],
        CallArgPositional::Positional(InstrWithYield::Call(_))
    ));
}

#[test]
fn eval_order_hoists_lowered_await_in_assignment_call_argument() {
    let block = Block {
        label: BlockLabel::from_index(0),
        body: vec![Store::new(
            test_name("total"),
            Box::new(InstrWithAwaitAndYield::BinOp(BinOp::new(
                BinOpKind::InplaceAdd,
                InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!("total")),
                InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!("await it")),
            ))),
        )
        .into()],
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!(
            "__dp_NONE"
        ))),
        params: Vec::new(),
        exc_edge: None,
    };

    let lowered = lower_yield_block(block);
    assert_eq!(lowered.body.len(), 3);
    let InstrWithYield::Store(temp_assign) = &lowered.body[0] else {
        panic!("expected hoisted yield-from temp store");
    };
    assert!(matches!(*temp_assign.value, InstrWithYield::YieldFrom(_)));
    let InstrWithYield::Store(assign) = &lowered.body[1] else {
        panic!("expected rewritten store");
    };
    let InstrWithYield::BinOp(op) = &*assign.value else {
        panic!("expected inplace add operation");
    };
    assert!(matches!(op.right.as_ref(), InstrWithYield::Load(_)));
    assert!(matches!(assign.value.as_ref(), InstrWithYield::BinOp(_)));
    assert!(matches!(lowered.body[2], InstrWithYield::Del(_)));
}

#[test]
fn eval_order_hoists_yield_from_in_assignment_call_argument() {
    let block = Block {
        label: BlockLabel::from_index(0),
        body: vec![Store::new(
            test_name("total"),
            Box::new(InstrWithAwaitAndYield::BinOp(BinOp::new(
                BinOpKind::InplaceAdd,
                InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!("total")),
                InstrWithAwaitAndYield::YieldFrom(
                    YieldFrom::new(InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!("it")))
                        .with_meta(Meta::default()),
                ),
            ))),
        )
        .into()],
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!(
            "__dp_NONE"
        ))),
        params: Vec::new(),
        exc_edge: None,
    };

    let lowered = lower_yield_block(block);
    assert_eq!(lowered.body.len(), 3);
    let InstrWithYield::Store(temp_assign) = &lowered.body[0] else {
        panic!("expected hoisted yield-from temp store");
    };
    assert!(matches!(*temp_assign.value, InstrWithYield::YieldFrom(_)));
    let InstrWithYield::Store(assign) = &lowered.body[1] else {
        panic!("expected rewritten store");
    };
    let InstrWithYield::BinOp(op) = &*assign.value else {
        panic!("expected inplace add operation");
    };
    assert!(matches!(op.right.as_ref(), InstrWithYield::Load(_)));
    assert!(matches!(assign.value.as_ref(), InstrWithYield::BinOp(_)));
    assert!(matches!(lowered.body[2], InstrWithYield::Del(_)));
}

#[test]
fn eval_order_leaves_non_yield_binop_stmt_shape_alone() {
    let block = Block {
        label: BlockLabel::from_index(0),
        body: vec![Store::new(
            test_name("total"),
            Box::new(InstrWithAwaitAndYield::BinOp(BinOp::new(
                BinOpKind::InplaceAdd,
                InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!("total")),
                InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!("rhs")),
            ))),
        )
        .into()],
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(crate::py_expr!(
            "__dp_NONE"
        ))),
        params: Vec::new(),
        exc_edge: None,
    };

    let lowered = lower_yield_block(block);
    assert_eq!(lowered.body.len(), 1);
    let InstrWithYield::Store(assign) = &lowered.body[0] else {
        panic!("expected rewritten store");
    };
    let InstrWithYield::BinOp(op) = &*assign.value else {
        panic!("expected iadd operation");
    };
    assert_eq!(op.kind, BinOpKind::InplaceAdd);
    assert!(matches!(op.right.as_ref(), InstrWithYield::Load(_)));
}
