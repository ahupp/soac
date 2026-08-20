use super::*;

use crate::block_py::{
    Block, BlockLabel, BlockPyFunction, BlockTerm, CallableScopeInfo, FunctionKind, FunctionName,
    InstrWithAwaitAndYield, InstrWithYield, NameLike,
};

fn test_name_gen() -> crate::block_py::FunctionNameGen {
    let module_name_gen = crate::block_py::ModuleNameGen::new(0);
    module_name_gen.next_function_name_gen()
}

#[test]
fn lowers_await_to_yield_from_await_iter() {
    let structured_block = Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(
            crate::template::py_expr!("await foo()"),
        )),
        params: Vec::new(),
        exc_edge: None,
        extra: soac_core::block_py::BlockContext::default(),
    };
    let module = BlockPyModule {
        strict_source: None,
        module_name_gen: crate::block_py::ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: vec![BlockPyFunction {
            function_id: crate::block_py::RuntimeFunctionId::from_raw_parts(0, 1),
            name_gen: test_name_gen(),
            names: FunctionName::new("f", "f", "f", "f"),
            kind: FunctionKind::Coroutine,
            execution_mode: Default::default(),
            params: Default::default(),
            body_params: None,
            public_scope: None,
            blocks: vec![Block {
                label: structured_block.label,
                body: structured_block.body,
                term: structured_block.term,
                params: structured_block.params,
                exc_edge: structured_block.exc_edge,
                extra: Default::default(),
            }],
            doc: None,
            public_storage_layout: None,
            storage_layout: None,
            scope: CallableScopeInfo::default(),
        }],
        counter_defs: Vec::new(),
        module_constants: Vec::new(),
    };

    let lowered = lower_awaits_in_core_blockpy_module(module);
    let block = &lowered.callable_defs[0].blocks[0];
    assert!(block.body.is_empty());
    let BlockTerm::Return(InstrWithYield::YieldFrom(yield_from)) = &block.term else {
        panic!("expected return of lowered await yield from");
    };
    let InstrWithYield::Call(call) = yield_from.value.as_ref() else {
        panic!("expected await_iter call op");
    };
    let InstrWithYield::Load(operation) = call.func.as_ref() else {
        panic!("expected await helper load");
    };
    assert!(operation.name.is_runtime_symbol("await_iter"));
}
