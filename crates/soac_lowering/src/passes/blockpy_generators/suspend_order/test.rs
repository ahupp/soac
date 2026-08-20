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
    let ast::Expr::Name(expr) = crate::template::py_expr!("{id:id}", id = id) else {
        unreachable!();
    };
    expr.id.into()
}

fn same_unresolved_name(left: &UnresolvedName, right: &UnresolvedName) -> bool {
    match (left, right) {
        (UnresolvedName::SourceName(left), UnresolvedName::SourceName(right)) => left == right,
        (UnresolvedName::RuntimeName(left), UnresolvedName::RuntimeName(right)) => left == right,
        _ => false,
    }
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
        function_id: RuntimeFunctionId::from_raw_parts(0, 1),
        name_gen: test_name_gen(),
        names: FunctionName::new("f", "f", "f", "f"),
        kind: FunctionKind::Generator,
        execution_mode: Default::default(),
        params: Default::default(),
        body_params: None,
        public_scope: None,
        blocks: vec![block],
        doc: None,
        public_storage_layout: None,
        storage_layout: None,
        scope: CallableScopeInfo::default(),
    }
}

fn lower_awaits_in_test_block(block: Block<InstrWithAwaitAndYield>) -> Block<InstrWithYield> {
    let lowered = lower_awaits_in_core_blockpy_module(BlockPyModule {
        strict_source: None,
        module_name_gen: ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: vec![BlockPyFunction::<CoreModuleShapeWithAwaitAndYield> {
            function_id: RuntimeFunctionId::from_raw_parts(0, 1),
            name_gen: test_name_gen(),
            names: FunctionName::new("f", "f", "f", "f"),
            kind: FunctionKind::Coroutine,
            execution_mode: Default::default(),
            params: Default::default(),
            body_params: None,
            public_scope: None,
            blocks: vec![block],
            doc: None,
            public_storage_layout: None,
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
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(
            crate::template::py_expr!("f(g(x), h(y))"),
        )),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
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
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(
            crate::template::py_expr!("f(g(x))"),
        )),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
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
            Box::new(InstrWithAwaitAndYield::from_ast_expr(
                crate::template::py_expr!("f(g(x))"),
            )),
        )
        .into()],
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(
            crate::template::py_expr!("__dp_NONE"),
        )),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
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
                InstrWithAwaitAndYield::from_ast_expr(crate::template::py_expr!("total")),
                InstrWithAwaitAndYield::from_ast_expr(crate::template::py_expr!("await it")),
            ))),
        )
        .into()],
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(
            crate::template::py_expr!("__dp_NONE"),
        )),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    };

    let lowered = lower_yield_block(block);
    assert_ordered_binop_suspend(&lowered);
}

#[test]
fn eval_order_hoists_yield_from_in_assignment_call_argument() {
    let block = Block {
        label: BlockLabel::from_index(0),
        body: vec![Store::new(
            test_name("total"),
            Box::new(InstrWithAwaitAndYield::BinOp(BinOp::new(
                BinOpKind::InplaceAdd,
                InstrWithAwaitAndYield::from_ast_expr(crate::template::py_expr!("total")),
                InstrWithAwaitAndYield::YieldFrom(
                    YieldFrom::new(InstrWithAwaitAndYield::from_ast_expr(
                        crate::template::py_expr!("it"),
                    ))
                    .with_meta(Meta::default()),
                ),
            ))),
        )
        .into()],
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(
            crate::template::py_expr!("__dp_NONE"),
        )),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    };

    let lowered = lower_yield_block(block);
    assert_ordered_binop_suspend(&lowered);
}

#[test]
fn eval_order_leaves_non_yield_binop_stmt_shape_alone() {
    let block = Block {
        label: BlockLabel::from_index(0),
        body: vec![Store::new(
            test_name("total"),
            Box::new(InstrWithAwaitAndYield::BinOp(BinOp::new(
                BinOpKind::InplaceAdd,
                InstrWithAwaitAndYield::from_ast_expr(crate::template::py_expr!("total")),
                InstrWithAwaitAndYield::from_ast_expr(crate::template::py_expr!("rhs")),
            ))),
        )
        .into()],
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(
            crate::template::py_expr!("__dp_NONE"),
        )),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
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

#[test]
fn eval_order_preserves_callee_and_earlier_operands_before_nested_yield() {
    use crate::block_py::NameLike;
    let lowered = lower_yield_block(Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(
            crate::template::py_expr!("consume(make(), (yield 1), later())"),
        )),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    });
    let mut before_suspend = Vec::new();
    let mut saw_suspend = false;
    for instr in &lowered.body {
        let InstrWithYield::Store(store) = instr else {
            continue;
        };
        match store.value.as_ref() {
            InstrWithYield::Yield(_) => {
                saw_suspend = true;
                break;
            }
            InstrWithYield::Load(load) => before_suspend.push(load.name.id_str()),
            InstrWithYield::Call(call) => {
                if let InstrWithYield::Load(load) = call.func.as_ref() {
                    before_suspend.push(load.name.id_str());
                }
            }
            _ => {}
        }
    }
    assert!(saw_suspend, "source yield must remain explicit");
    assert_eq!(
        before_suspend,
        ["consume", "make"],
        "callee and earlier operands must execute before suspension; later() must wait"
    );
}

fn assert_ordered_binop_suspend(lowered: &Block<InstrWithYield>) {
    use crate::block_py::{NameLike, StoreLifetime};
    let [InstrWithYield::Store(left), InstrWithYield::Store(resumed), InstrWithYield::Store(result)] =
        lowered.body.as_slice()
    else {
        panic!("expected left Operand, suspended Operand, and the original store");
    };
    let InstrWithYield::Load(load) = left.value.as_ref() else {
        panic!("left evaluated before suspension");
    };
    assert_eq!(load.name.id_str(), "total");
    assert!(matches!(
        resumed.value.as_ref(),
        InstrWithYield::YieldFrom(_)
    ));
    let (
        StoreLifetime::Operand {
            unwind_order: left_order,
        },
        StoreLifetime::Operand {
            unwind_order: resumed_order,
        },
    ) = (left.lifetime, resumed.lifetime)
    else {
        panic!("suspension captures are movable Operand owners, not Frame bindings");
    };
    assert!(left_order < resumed_order);
    let InstrWithYield::BinOp(op) = result.value.as_ref() else {
        panic!("original operation preserved");
    };
    let (InstrWithYield::TakeOperand(left_take), InstrWithYield::TakeOperand(resumed_take)) =
        (op.left.as_ref(), op.right.as_ref())
    else {
        panic!("each prepared value is consumed without a surviving Load owner");
    };
    assert!(same_unresolved_name(&left_take.name, &left.name));
    assert!(same_unresolved_name(&resumed_take.name, &resumed.name));
    assert_eq!(result.name.id_str(), "total");
}

fn suspension_expression(source: &str) -> Block<InstrWithYield> {
    let expr = ruff_python_parser::parse_expression(source)
        .unwrap()
        .into_syntax()
        .body;
    lower_yield_block(Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(*expr)),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    })
}

#[derive(Debug, PartialEq, Eq)]
enum SuspendEvent {
    Read(String),
    Call(String),
    Yield,
    Phase(crate::block_py::CallArgumentOpKind),
    Build(crate::block_py::BuildCollectionKind, usize),
    Insert(crate::block_py::ComprehensionInsertKind),
}

fn suspension_events(block: &Block<InstrWithYield>) -> Vec<SuspendEvent> {
    use crate::block_py::NameLike;
    fn record(expr: &InstrWithYield, out: &mut Vec<SuspendEvent>) {
        match expr {
            InstrWithYield::Store(op) => record(&op.value, out),
            InstrWithYield::Load(op) => out.push(SuspendEvent::Read(op.name.id_str().into())),
            InstrWithYield::Call(op) => {
                if let InstrWithYield::Load(load) = op.func.as_ref() {
                    out.push(SuspendEvent::Call(load.name.id_str().into()));
                }
            }
            InstrWithYield::Yield(_) | InstrWithYield::YieldFrom(_) => {
                out.push(SuspendEvent::Yield)
            }
            InstrWithYield::CallArgumentOp(op) => out.push(SuspendEvent::Phase(op.kind)),
            InstrWithYield::BuildCollection(op) => {
                out.push(SuspendEvent::Build(op.kind, op.values.len()))
            }
            InstrWithYield::ComprehensionInsert(op) => out.push(SuspendEvent::Insert(op.kind)),
            _ => {}
        }
    }
    let mut result = Vec::new();
    for instruction in &block.body {
        record(instruction, &mut result);
    }
    result
}

fn event_position(events: &[SuspendEvent], expected: SuspendEvent) -> usize {
    events
        .iter()
        .position(|event| *event == expected)
        .expect("required selected source phase")
}

#[test]
fn eval_order_preserves_expansion_and_keyword_merge_barriers_before_suspension() {
    use crate::block_py::{
        BuildCollectionKind as Build, CallArgumentOpKind as Phase, StoreLifetime,
    };
    let positional = suspension_expression("consume(first(), *items(), (yield 1), later())");
    let events = suspension_events(&positional);
    let first = event_position(&events, SuspendEvent::Call("first".into()));
    let build = event_position(&events, SuspendEvent::Build(Build::List, 1));
    let items = event_position(&events, SuspendEvent::Call("items".into()));
    let extend = event_position(&events, SuspendEvent::Phase(Phase::ExtendPositional));
    let suspend = event_position(&events, SuspendEvent::Yield);
    let later = event_position(&events, SuspendEvent::Call("later".into()));
    let finish = event_position(&events, SuspendEvent::Phase(Phase::FinishPositionalList));
    assert!(first < build && build < items && items < extend && extend < suspend);
    assert!(suspend < later && later < finish);
    assert!(matches!(
        positional.term,
        BlockTerm::Return(InstrWithYield::PreparedCall(_))
    ));
    assert!(positional
        .body
        .iter()
        .filter_map(|instr| match instr {
            InstrWithYield::Store(op) => Some(op),
            _ => None,
        })
        .all(|store| matches!(store.lifetime, StoreLifetime::Operand { .. })));

    let keyword = suspension_expression("consume(**mapping(), first=first(), second=(yield 1))");
    let events = suspension_events(&keyword);
    let mapping = event_position(&events, SuspendEvent::Call("mapping".into()));
    let suspend = event_position(&events, SuspendEvent::Yield);
    let merges = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            (*event == SuspendEvent::Phase(Phase::MergeKeywords)).then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(merges.len(), 2);
    assert!(mapping < merges[0] && merges[0] < suspend && suspend < merges[1]);
    let group = event_position(&events, SuspendEvent::Build(Build::Dict, 4));
    assert!(
        suspend < group && group < merges[1],
        "evaluate the complete named group before duplicate checking"
    );
}

#[test]
fn eval_order_singleton_star_waits_for_keywords_and_preserves_call_metadata() {
    use crate::block_py::{
        BuildCollectionKind as Build, CallArgumentOpKind as Phase, FrameNamespace, HasMeta, Load,
        NameLike,
    };
    use ruff_text_size::{TextRange, TextSize};
    let InstrWithAwaitAndYield::Call(mut call) = InstrWithAwaitAndYield::from_ast_expr(
        crate::template::py_expr!("consume(*items(), answer=(yield 1))"),
    ) else {
        panic!("source call");
    };
    let meta = Meta::new(
        Default::default(),
        TextRange::new(TextSize::from(11), TextSize::from(49)),
    );
    call = call.with_meta(meta.clone());
    call.frame_namespace = Some(FrameNamespace::Mapping(Box::new(
        Load::new(test_name("namespace")).into(),
    )));
    let lowered = lower_yield_block(Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Return(call.into()),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    });
    let events = suspension_events(&lowered);
    assert!(!events.contains(&SuspendEvent::Phase(Phase::ExtendPositional)));
    let raw = event_position(&events, SuspendEvent::Call("items".into()));
    let suspend = event_position(&events, SuspendEvent::Yield);
    let keywords = event_position(&events, SuspendEvent::Build(Build::Dict, 2));
    let normalize = event_position(&events, SuspendEvent::Phase(Phase::NormalizeSingletonStar));
    assert!(raw < suspend && suspend < keywords && keywords < normalize);
    let BlockTerm::Return(InstrWithYield::PreparedCall(call)) = &lowered.term else {
        panic!("selected complete phase bundle");
    };
    assert_eq!(call.meta().range, meta.range);
    let Some(FrameNamespace::Mapping(namespace)) = &call.frame_namespace else {
        panic!("same explicit frame coordinate");
    };
    let InstrWithYield::Load(namespace) = namespace.as_ref() else {
        panic!("namespace was not turned into an owned argument");
    };
    assert_eq!(namespace.name.id_str(), "namespace");
    let operand_takes = [&call.func, &call.arguments, call.keywords.as_ref().unwrap()];
    assert!(operand_takes
        .into_iter()
        .all(|value| matches!(value.as_ref(), InstrWithYield::TakeOperand(_))));
}

#[test]
fn eval_order_moves_each_yield_result_once_and_is_idempotent() {
    use crate::block_py::{StoreLifetime, Visit};
    let lowered = suspension_expression("consume(first(), (yield 1), (yield 2))");
    let captures = lowered
        .body
        .iter()
        .filter_map(|instr| match instr {
            InstrWithYield::Store(op) => Some(op),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut orders = Vec::new();
    for capture in &captures {
        let StoreLifetime::Operand { unwind_order } = capture.lifetime else {
            panic!("capture cannot pin a Frame owner");
        };
        orders.push(unwind_order);
        struct Count<'a> {
            name: &'a UnresolvedName,
            takes: usize,
            loads: usize,
        }
        impl Visit<InstrWithYield> for Count<'_> {
            fn visit_instr(&mut self, expr: &InstrWithYield) {
                match expr {
                    InstrWithYield::TakeOperand(op)
                        if same_unresolved_name(&op.name, self.name) =>
                    {
                        self.takes += 1
                    }
                    InstrWithYield::Load(op) if same_unresolved_name(&op.name, self.name) => {
                        self.loads += 1
                    }
                    _ => {}
                }
                crate::block_py::ChildVisitable::visit_children(expr, self);
            }
        }
        let mut count = Count {
            name: &capture.name,
            takes: 0,
            loads: 0,
        };
        for instr in &lowered.body {
            count.visit_instr(instr);
        }
        count.visit_term(&lowered.term);
        assert_eq!(
            (count.takes, count.loads),
            (1, 0),
            "one semantic move, no secondary frame pin"
        );
    }
    assert!(orders.windows(2).all(|pair| pair[0] < pair[1]));
    let first_names = captures
        .iter()
        .map(|store| store.name.clone())
        .collect::<Vec<_>>();
    let second = make_suspend_order_explicit_in_core_callable_def(
        test_callable_def_with_yield_block(lowered.clone()),
    );
    let block = &second.blocks[0];
    let second_names = block
        .body
        .iter()
        .filter_map(|instr| match instr {
            InstrWithYield::Store(op) => Some(op.name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(first_names.len(), second_names.len());
    assert!(first_names
        .iter()
        .zip(&second_names)
        .all(|(left, right)| same_unresolved_name(left, right)));
    assert_eq!(suspension_events(&lowered), suspension_events(block));
}

#[test]
fn eval_order_reserves_original_source_names_before_making_suspension_operands() {
    use crate::block_py::{NameLike, StoreLifetime};
    let source_name = "_dp_suspend_operand_0_1_0";
    let source = format!("consume({source_name}, (yield 1))");
    let value = ruff_python_parser::parse_expression(&source)
        .unwrap()
        .into_syntax()
        .body;
    let block = lower_awaits_in_test_block(Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Return(InstrWithAwaitAndYield::from_ast_expr(*value)),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    });
    let mut callable = test_callable_def_with_yield_block(block);
    callable.scope.local_defs.insert(source_name.into());
    let lowered = make_suspend_order_explicit_in_core_callable_def(callable);
    let stores = lowered.blocks[0]
        .body
        .iter()
        .filter_map(|instr| match instr {
            InstrWithYield::Store(op) => Some(op),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!stores.is_empty());
    assert!(stores.iter().all(|store| store.name.id_str() != source_name
        && matches!(store.lifetime, StoreLifetime::Operand { .. })));
    assert!(stores.iter().any(|store| matches!(store.value.as_ref(), InstrWithYield::Load(load) if load.name.id_str() == source_name)));
}

#[test]
fn eval_order_keeps_function_creation_payload_out_of_the_evaluated_prefix() {
    use crate::block_py::{InstrWithConstantNone, Load, MakeFunction, NameLike, Tuple};
    let function_id = RuntimeFunctionId::from_raw_parts(0, 17);
    let defaults = Tuple::new(vec![
        InstrWithAwaitAndYield::from_ast_expr(crate::template::py_expr!("first_default()")),
        InstrWithAwaitAndYield::from_ast_expr(crate::template::py_expr!("(yield 1)")),
    ]);
    let creation = MakeFunction::new(
        function_id,
        FunctionKind::Function,
        Box::new(defaults.into()),
        Box::new(InstrWithAwaitAndYield::constant_none()),
        Some(Box::new(Load::new(test_name("namespace")).into())),
        Vec::new(),
    );
    let lowered = lower_yield_block(Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Return(creation.into()),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    });
    assert_eq!(
        suspension_events(&lowered),
        [
            SuspendEvent::Call("first_default".into()),
            SuspendEvent::Yield
        ]
    );
    let BlockTerm::Return(InstrWithYield::MakeFunction(creation)) = lowered.term else {
        panic!("typed creation operation retained");
    };
    assert_eq!(creation.function_id(), function_id);
    assert_eq!(creation.kind, FunctionKind::Function);
    let InstrWithYield::Load(namespace) = creation.class_namespace.as_deref().unwrap() else {
        panic!("namespace coordinate unchanged");
    };
    assert_eq!(namespace.name.id_str(), "namespace");
}

#[test]
fn eval_order_raises_with_cause_after_evaluating_the_exception_before_suspend() {
    use crate::block_py::{Call, Load, NameLike, RaiseDisposition, TermRaise};
    let call = Call::new(
        InstrWithAwaitAndYield::Load(Load::new(UnresolvedName::runtime_name("raise_from"))),
        vec![
            CallArgPositional::Positional(InstrWithAwaitAndYield::from_ast_expr(
                crate::template::py_expr!("make_exception()"),
            )),
            CallArgPositional::Positional(InstrWithAwaitAndYield::from_ast_expr(
                crate::template::py_expr!("(yield 1)"),
            )),
        ],
        Vec::new(),
    );
    let lowered = lower_yield_block(Block {
        label: BlockLabel::from_index(0),
        body: Vec::new(),
        term: BlockTerm::Raise(TermRaise {
            exc: Some(call.into()),
            disposition: RaiseDisposition::Source,
        }),
        params: Vec::new(),
        exc_edge: None,
        extra: Default::default(),
    });
    let events = suspension_events(&lowered);
    assert!(
        event_position(&events, SuspendEvent::Call("make_exception".into()))
            < event_position(&events, SuspendEvent::Yield)
    );
    let BlockTerm::Raise(raise) = lowered.term else {
        panic!("raise terminator retained");
    };
    assert_eq!(raise.disposition, RaiseDisposition::Source);
    let Some(InstrWithYield::Call(call)) = raise.exc else {
        panic!("same normalizing cause operation");
    };
    assert!(call.args.iter().all(|arg| matches!(
        arg,
        CallArgPositional::Positional(InstrWithYield::TakeOperand(_))
    )));
}
