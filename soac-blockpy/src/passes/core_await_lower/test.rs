use super::*;

use crate::block_py::{
    Block, BlockLabel, BlockPyFunction, BlockPyNameLike, BlockTerm, CallableScopeInfo,
    InstrWithAwaitAndYield, InstrWithYield, FunctionKind, FunctionName,
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
        term: BlockTerm::Return(InstrWithAwaitAndYield::from(crate::py_expr!(
            "await foo()"
        ))),
        params: Vec::new(),
        exc_edge: None,
    };
    let module = BlockPyModule {
        module_name_gen: crate::block_py::ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: vec![BlockPyFunction {
            function_id: crate::block_py::FunctionId::new(0, 0),
            name_gen: test_name_gen(),
            names: FunctionName::new("f", "f", "f", "f"),
            kind: FunctionKind::Coroutine,
            params: Default::default(),
            blocks: vec![Block {
                label: structured_block.label,
                body: structured_block.body,
                term: structured_block.term,
                params: structured_block.params,
                exc_edge: structured_block.exc_edge,
            }],
            doc: None,
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
