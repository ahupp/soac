use super::*;

use crate::block_py::{
    instr_any, BlockEdge, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm,
    FunctionKind, InstrWithAwaitAndYield, MapTerm, ModuleShape, NameLike, RaiseDisposition,
    ScopeExprNode, TermRaise, TryMapTerm,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::pass_tracker::LoweringPassTrackerInternalExt;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::stmt_sequences::{
    lower_if_stmt_sequence, lower_if_stmt_sequence_from_stmt, lower_while_stmt_sequence,
    lower_while_stmt_sequence_from_stmt, plan_instr_sequence_head,
};
use crate::passes::ruff_to_blockpy::try_regions::build_try_plan;
use crate::passes::{CoreModuleShape, CoreModuleShapeWithAwaitAndYield, InstrRuff};
use stmt_lowering::lower_instr_for_test;

fn test_name_gen() -> FunctionNameGen {
    let module_name_gen = crate::block_py::ModuleNameGen::new(0);
    module_name_gen.next_function_name_gen()
}

fn wrapped_blockpy(source: &str) -> BlockPyModule<CoreModuleShapeWithAwaitAndYield> {
    lower_python_to_blockpy_for_testing(source)
        .unwrap()
        .pass_tracker
        .pass_core_blockpy_with_await_and_yield()
        .expect("core_blockpy_with_await_and_yield pass should be tracked")
        .clone()
}

fn wrapped_core_blockpy_with_await_and_yield(
    source: &str,
) -> BlockPyModule<CoreModuleShapeWithAwaitAndYield> {
    wrapped_blockpy(source)
}

fn wrapped_core_blockpy(source: &str) -> BlockPyModule<CoreModuleShape> {
    lower_python_to_blockpy_for_testing(source)
        .unwrap()
        .pass_tracker
        .pass_core_blockpy()
        .expect("core_blockpy pass should be tracked")
        .clone()
}

fn function_by_name<'a, P: ModuleShape>(
    blockpy: &'a BlockPyModule<P>,
    bind_name: &str,
) -> &'a BlockPyFunction<P> {
    blockpy
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == bind_name)
        .unwrap_or_else(|| panic!("missing BlockPy function {bind_name}; got {blockpy:?}"))
}

fn function_instr_any<P>(
    function: &BlockPyFunction<P>,
    mut predicate: impl FnMut(&P::Instr) -> bool,
) -> bool
where
    P: ModuleShape,
    P::Instr: crate::block_py::ChildVisitable<P::Instr>,
{
    function.blocks.iter().any(|block| {
        block
            .body
            .iter()
            .any(|stmt| instr_any(stmt, &mut predicate))
            || match &block.term {
                BlockTerm::IfTerm(if_term) => instr_any(&if_term.test, &mut predicate),
                BlockTerm::BranchTable(branch) => instr_any(&branch.index, &mut predicate),
                BlockTerm::Raise(raise) => raise
                    .exc
                    .as_ref()
                    .is_some_and(|exc| instr_any(exc, &mut predicate)),
                BlockTerm::Return(value) | BlockTerm::GeneratorReturn(value) => {
                    instr_any(value, &mut predicate)
                }
                BlockTerm::Jump(_) => false,
            }
    })
}

fn function_has_root_load<P>(function: &BlockPyFunction<P>, name: &str) -> bool
where
    P: ModuleShape,
    P::Instr: ScopeExprNode,
{
    function_instr_any(function, |expr| expr.root_name_id() == Some(name))
}

fn function_has_instr<P>(
    function: &BlockPyFunction<P>,
    mut predicate: impl FnMut(&P::Instr) -> bool,
) -> bool
where
    P: ModuleShape,
    P::Instr: crate::block_py::ChildVisitable<P::Instr>,
{
    function_instr_any(function, &mut predicate)
}

fn lower_stmt_for_panic_test(stmt: &Stmt) {
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = crate::passes::ruff_to_blockpy::InlineBlockBuilder::<InstrWithAwaitAndYield>::new(
        &name_gen,
    );
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt.clone());
    let _ = lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None);
}

fn test_context() -> Context {
    Context::new("")
}

fn label(index: u32) -> BlockLabel {
    BlockLabel::from_index(index as usize)
}

type TestBlock = Block<InstrWithAwaitAndYield>;

fn instr_stmt(stmt: Stmt) -> InstrRuff {
    crate::passes::ast_to_instr::from_ast_stmt(stmt)
}

fn instr_expr(expr: Expr) -> InstrRuff {
    crate::passes::ast_to_instr::from_ast_expr(expr)
}

#[test]
fn lowers_post_simplification_control_flow() {
    let blockpy = wrapped_blockpy(
        r#"
def f(x, ys):
    while x:
        for y in ys:
            if y:
                break
            continue
    try:
        return x
    except ValueError as err:
        return err
"#,
    );
    let blocks = &function_by_name(&blockpy, "f").blocks;
    assert!(blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::IfTerm(_))));
    assert!(blocks.iter().any(|block| block.exc_edge.is_some()));
    assert!(blocks.iter().any(|block| matches!(
        &block.term,
        BlockTerm::Return(InstrWithAwaitAndYield::Load(load)) if load.name.id_str() == "x"
    )));
}

#[test]
fn caught_handler_return_uses_a_distinct_finally_exception_region() {
    let source = r#"
def f(work, outer_probe, handler_probe, finally_probe):
    try:
        raise KeyError('outer')
    except KeyError:
        outer_probe()
        try:
            work()
        except ValueError:
            handler_probe()
            return 73
        finally:
            finally_probe()
"#;
    let lowered = lower_python_to_blockpy_for_testing(source).unwrap();
    let blockpy = lowered
        .pass_tracker
        .pass_core_blockpy_with_await_and_yield()
        .unwrap();
    let function = function_by_name(&blockpy, "f");
    let probe_block = |name: &str| {
        let mut matches = function.blocks.iter().filter(|block| {
            block.body.iter().any(|instr| {
                instr_any(instr, &mut |expr: &InstrWithAwaitAndYield| {
                    expr.root_name_id() == Some(name)
                })
            })
        });
        let block = matches.next().expect("the source probe is represented");
        assert!(matches.next().is_none(), "the probe belongs to one region");
        block
    };
    let outer = probe_block("outer_probe");
    let handler = probe_block("handler_probe");
    let finally = probe_block("finally_probe");
    let outer_region = outer.exception_param().expect("outer handler region");
    let handler_region = handler
        .exception_param()
        .expect("caught inner handler region");
    let finally_region = finally
        .exception_param()
        .expect("exceptional finally region");
    assert_ne!(
        handler_region, finally_region,
        "normal return from a caught handler must pop its handled item before the associated finally"
    );
    for block in [handler, finally] {
        assert!(block.params.iter().any(|param| {
            param.role == BlockParamRole::EnclosingException && param.name == outer_region
        }));
    }
    assert!(finally
        .params
        .iter()
        .all(|param| param.name != handler_region));
    assert_all_block_targets_present(&function.blocks);

    // The later physical argument completion must not undo the distinct
    // region decision by forwarding a different parameter with the same role.
    let bound = lowered.pass_tracker.pass_name_binding().unwrap();
    let function = function_by_name(bound, "f");
    let mut normal_entries = 0;
    for block in &function.blocks {
        let BlockTerm::Jump(edge) = &block.term else {
            continue;
        };
        let target = function
            .blocks
            .iter()
            .find(|candidate| candidate.label == edge.target)
            .unwrap();
        let Some(index) = target
            .params
            .iter()
            .position(|param| param.name == finally_region)
        else {
            continue;
        };
        if block
            .params
            .iter()
            .any(|param| param.name == finally_region)
        {
            continue;
        }
        assert_eq!(edge.args.len(), target.params.len());
        assert!(
            matches!(edge.args[index], crate::block_py::BlockArg::None),
            "a new normal finally entry has no escaping exception; another handler's role is not its identity: {:?}",
            edge.args[index]
        );
        normal_entries += 1;
    }
    assert!(
        normal_entries > 0,
        "the fixture must exercise a fresh normal finally edge"
    );
}

#[test]
fn for_loop_ir_operand_statements_preserve_store_purpose_and_lifetime() {
    use crate::block_py::{HasMeta, Store, StoreLifetime, StorePurpose, TakeOperand, WithMeta};
    let context = test_context();
    let name_gen = test_name_gen();
    let value = instr_expr(py_expr!("value"));
    let meta = value.meta();
    let store: InstrRuff = Store::new("operand", value)
        .with_lifetime(StoreLifetime::Operand { unwind_order: 17 })
        .with_purpose(StorePurpose::BlockParameterTransport)
        .with_meta(meta.clone())
        .into();
    let take: InstrRuff = TakeOperand::new("operand").with_meta(meta.clone()).into();
    let mut out = InlineBlockBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    for instr in [store, take] {
        assert!(matches!(
            plan_instr_sequence_head(&context, &instr),
            StmtSequenceHeadPlan::Linear(_)
        ));
        lower_instr_for_test(&context, &instr, &name_gen, &mut out, None)
            .expect("compiler operand statements must stay in IR");
    }
    let fragment = out.finish();
    let [InstrWithAwaitAndYield::Store(store), InstrWithAwaitAndYield::TakeOperand(take)] =
        fragment.entry.body.as_slice()
    else {
        panic!("the statement bridge must preserve the store and consuming discard")
    };
    assert!(matches!(
        store.lifetime,
        StoreLifetime::Operand { unwind_order: 17 }
    ));
    assert_eq!(store.purpose, StorePurpose::BlockParameterTransport);
    assert_eq!(store.meta().range, meta.range);
    assert_eq!(store.name.id_str(), take.name.id_str());
}

#[test]
fn for_loop_simple_target_does_not_self_store_generated_next_temp() {
    use crate::block_py::{CallArgPositional, HandledExceptionContext, HasMeta, StoreLifetime};
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(xs):
    for item in xs:
        sink(item)
"#,
    );
    let function = function_by_name(&blockpy, "f");

    let mut steps = Vec::new();
    function_instr_any(function, |expr| {
        if let InstrWithAwaitAndYield::IteratorStep(op) = expr {
            steps.push(op.name.id_str().to_owned());
        }
        false
    });
    let [iterator] = steps.as_slice() else {
        panic!("one explicit loop step")
    };
    let mut iterator_store = None;
    let mut item_store = None;
    function_instr_any(function, |expr| {
        if let InstrWithAwaitAndYield::Store(store) = expr {
            if store.name.id_str() == iterator {
                iterator_store = Some(store.clone());
            }
            if matches!(
                store.value.as_ref(),
                InstrWithAwaitAndYield::IteratorStep(_)
            ) {
                item_store = Some(store.clone());
            }
        }
        false
    });
    let iterator_store = iterator_store.expect("loop iterator acquisition");
    let item_store = item_store.expect("fetched item acquisition");
    assert!(
        !item_store.meta().range.is_empty(),
        "real iterator errors must retain the original for-statement source range"
    );
    let StoreLifetime::Operand {
        unwind_order: iterator_order,
    } = iterator_store.lifetime
    else {
        panic!("iterator must remain an expression operand, not an ordinary local owner")
    };
    let StoreLifetime::Operand {
        unwind_order: item_order,
    } = item_store.lifetime
    else {
        panic!("fetched item must unwind on target failure")
    };
    assert!(iterator_order < item_order);
    let InstrWithAwaitAndYield::Call(acquire) = iterator_store.value.as_ref() else {
        panic!("canonical iterator acquisition")
    };
    let [CallArgPositional::Positional(InstrWithAwaitAndYield::TakeOperand(iterable))] =
        acquire.args.as_slice()
    else {
        panic!("GetIter consumes its evaluated iterable")
    };
    assert!(function_has_instr(function, |expr| matches!(expr,
        InstrWithAwaitAndYield::Store(store)
        if store.name.id_str() == iterable.name.id_str()
            && matches!(store.lifetime, StoreLifetime::Operand { unwind_order } if unwind_order < iterator_order)
    )));
    assert!(function_has_instr(function, |expr| matches!(expr,
        InstrWithAwaitAndYield::Store(store)
        if store.name.id_str() == "item"
            && matches!(store.value.as_ref(), InstrWithAwaitAndYield::TakeOperand(take)
                if take.name.id_str() == item_store.name.id_str())
    )));
    assert!(!function_has_root_load(function, "next"));
    assert!(function_has_root_load(function, "exception_matches"));
    assert!(function_has_root_load(function, "StopIteration"));
    assert!(!function_has_root_load(function, "object"));
    let cleanups = function
        .blocks
        .iter()
        .filter(|block| {
            block.body.iter().any(|instr|
        matches!(instr, InstrWithAwaitAndYield::TakeOperand(take) if take.name.id_str() == iterator)
    )
        })
        .collect::<Vec<_>>();
    let normal = cleanups
        .iter()
        .find(|block| matches!(block.term, BlockTerm::Return(_)))
        .expect("normal exhaustion must consume the iterator before returning");
    // The ordinary CFG pass folds this cleanup's jump to the empty implicit
    // return block. Assert the resulting semantic exit, not the pre-fold jump.
    assert!(
        matches!(&normal.term, BlockTerm::Return(InstrWithAwaitAndYield::Load(load))
        if load.name.runtime_name_id() == Some(crate::block_py::RuntimeName::None))
    );
    let error = cleanups.iter().find(|block| matches!(&block.term,
        BlockTerm::Raise(raise) if raise.disposition == RaiseDisposition::PropagateNormalized
    )).expect("loop error exits consume the owner before forwarding");
    assert_eq!(
        error.extra.handled_exception,
        HandledExceptionContext::Unwind
    );
    assert!(error.exception_param().is_some());
    let fetch_block = function
        .blocks
        .iter()
        .find(|block| {
            block.body.iter().any(|instr| {
                instr_any(instr, |expr| {
                    matches!(expr, InstrWithAwaitAndYield::IteratorStep(_))
                })
            })
        })
        .expect("the iterator fetch is emitted directly into a BlockPy block");
    let dispatch_label = fetch_block.exc_edge.as_ref().unwrap().target;
    let dispatch = function
        .blocks
        .iter()
        .find(|block| block.label == dispatch_label)
        .unwrap();
    assert_eq!(
        dispatch.extra.handled_exception,
        HandledExceptionContext::Unwind
    );
    let BlockTerm::IfTerm(branch) = &dispatch.term else {
        panic!("only a failed fetch tests the native exhaustion exception")
    };
    assert_eq!(branch.then_label, normal.label);
    assert_eq!(branch.else_label, error.label);
    assert!(instr_any(&branch.test, |expr| expr.root_name_id()
        == Some("exception_matches")));
    let body_block = function
        .blocks
        .iter()
        .find(|block| {
            block
                .body
                .iter()
                .any(|instr| instr_any(instr, |expr| expr.root_name_id() == Some("sink")))
        })
        .expect("the source loop body must be present");
    assert_eq!(
        body_block.exc_edge.as_ref().unwrap().target,
        error.label,
        "body errors must not be mistaken for iterator exhaustion"
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn for_loop_direct_fetch_region_preserves_async_and_fallible_target_shapes() {
    use crate::block_py::StoreLifetime;
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def assign_target(xs, target):
    for target.item in xs:
        body()
    else:
        finished()

async def async_target(xs, target):
    async for target.item in xs:
        body()
    else:
        finished()
"#,
    );
    for (name, stop_exception, asynchronous) in [
        ("assign_target", "StopIteration", false),
        ("async_target", "StopAsyncIteration", true),
    ] {
        let function = function_by_name(&blockpy, name);
        assert!(function_has_root_load(function, stop_exception));
        assert!(function_has_root_load(function, "finished"));
        assert_eq!(
            function_has_instr(function, |expr| matches!(
                expr,
                InstrWithAwaitAndYield::IteratorStep(_)
            )),
            !asynchronous
        );
        assert_eq!(
            function_has_instr(function, |expr| matches!(
                expr,
                InstrWithAwaitAndYield::Await(_)
            )),
            asynchronous
        );
        let (target_block, target_index, target) = function
            .blocks
            .iter()
            .find_map(|block| {
                block.body.iter().enumerate().find_map(|(index, instr)| {
                    let InstrWithAwaitAndYield::SetAttr(target) = instr else {
                        return None;
                    };
                    Some((block, index, target))
                })
            })
            .expect("the original attribute target must be emitted");
        let InstrWithAwaitAndYield::TakeOperand(replacement) = target.replacement.as_ref() else {
            panic!("assignment consumes its staged RHS after evaluating the fallible target")
        };
        let handoff = target_block.body[..target_index]
            .iter()
            .find_map(|instr| match instr {
                InstrWithAwaitAndYield::Store(store)
                    if store.name.id_str() == replacement.name.id_str() =>
                {
                    Some(store)
                }
                _ => None,
            })
            .expect("the target replacement must use the staged fetched item");
        let InstrWithAwaitAndYield::TakeOperand(item) = handoff.value.as_ref() else {
            panic!("staging must consume, not clone, the fetched-item owner")
        };
        let StoreLifetime::Operand {
            unwind_order: handoff_order,
        } = handoff.lifetime
        else {
            panic!("the assignment RHS must retire on target failure")
        };
        assert!(function_has_instr(function, |expr| matches!(expr,
            InstrWithAwaitAndYield::Store(store)
                if store.name.id_str() == item.name.id_str()
                    && matches!(store.lifetime, StoreLifetime::Operand { unwind_order }
                        if unwind_order < handoff_order)
                    && matches!(store.value.as_ref(),
                        InstrWithAwaitAndYield::IteratorStep(_) | InstrWithAwaitAndYield::Await(_))
        )));
        assert!(!target_block.body[target_index + 1..].iter().any(|instr| matches!(instr,
            InstrWithAwaitAndYield::Del(del) if del.name.id_str() == replacement.name.id_str()
        )), "SetAttr must not retain a second staged RHS owner for later cleanup");
        assert!(
            !function_has_instr(function, |expr| matches!(expr,
                InstrWithAwaitAndYield::Load(load) if load.name.id_str() == item.name.id_str()
            )),
            "the fetched-item binding must not acquire a second owner through Load"
        );
        assert_all_block_targets_present(&function.blocks);
    }
}

#[test]
fn for_loop_return_cleanup_is_outside_inner_handlers_and_after_return_evaluation() {
    use crate::block_py::{HandledExceptionContext, StoreLifetime};
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(xs, observe):
    for item in xs:
        try:
            raise ValueError('inner')
        except ValueError:
            return observe()

def explicit_next(iterator):
    return next(iterator)
"#,
    );
    let function = function_by_name(&blockpy, "f");
    let mut iterator = None;
    function_instr_any(function, |expr| {
        if let InstrWithAwaitAndYield::IteratorStep(step) = expr {
            iterator = Some(step.name.id_str().to_owned());
        }
        false
    });
    let iterator = iterator.unwrap();
    let cleanup = function
        .blocks
        .iter()
        .find(|block| {
            matches!(
                &block.term,
                BlockTerm::Return(InstrWithAwaitAndYield::TakeOperand(_))
            ) && block.body.iter().any(|expr| {
                matches!(expr,
                InstrWithAwaitAndYield::TakeOperand(take) if take.name.id_str() == iterator)
            })
        })
        .expect("return must pass through the loop owner cleanup");
    assert_eq!(
        cleanup.extra.handled_exception,
        HandledExceptionContext::Regions
    );
    assert_eq!(cleanup.handled_exception_params().count(), 0);
    let BlockTerm::Return(InstrWithAwaitAndYield::TakeOperand(returned)) = &cleanup.term else {
        unreachable!()
    };
    let predecessors = function
        .blocks
        .iter()
        .filter(|block| {
            matches!(&block.term,
                BlockTerm::Jump(edge) if edge.target == cleanup.label
            )
        })
        .collect::<Vec<_>>();
    let [producer] = predecessors.as_slice() else {
        panic!("one captured return operand")
    };
    assert!(producer.handled_exception_params().count() > 0);
    assert!(producer.body.iter().any(|expr| matches!(expr,
        InstrWithAwaitAndYield::Store(store)
        if store.name.id_str() == returned.name.id_str()
            && matches!(store.lifetime, StoreLifetime::Operand { .. })
            && instr_any(store.value.as_ref(), |input| input.root_name_id() == Some("observe"))
    )));
    let ordinary_call = function_by_name(&blockpy, "explicit_next");
    assert!(function_has_root_load(ordinary_call, "next"));
    assert!(!function_has_instr(ordinary_call, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::IteratorStep(_)
    )));
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn lowers_async_for_structurally() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
async def f(xs):
    async for x in xs:
        body(x)
"#,
    );
    let f = function_by_name(&blockpy, "f");
    assert!(function_has_root_load(f, "anext"));
    assert!(function_has_instr(f, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::Await(_)
    )));
}

#[test]
fn lowers_generator_yield_to_explicit_blockpy_dispatch() {
    let blockpy = wrapped_core_blockpy(
        r#"
def gen(n):
    yield n
"#,
    );
    let gen = function_by_name(&blockpy, "gen");
    assert_eq!(gen.kind, FunctionKind::Generator);
    assert!(gen
        .blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::BranchTable(_))));
}

#[test]
fn lowers_generator_yield_boolop_without_external_fragment_jump() {
    let blockpy = wrapped_core_blockpy(
        r#"
def gen(flag):
    yield flag and 1
"#,
    );
    let gen = function_by_name(&blockpy, "gen");
    assert_eq!(gen.kind, FunctionKind::Generator);
    let entry_label = gen.entry_block().label;
    assert!(!gen.blocks.iter().any(|block| matches!(
        &block.term,
        BlockTerm::Jump(edge) if edge.target == entry_label
    )));
}

#[test]
fn stmt_sequence_head_plan_leaves_yield_expr_linear() {
    let module = ruff_python_parser::parse_module(
        r#"
def gen():
    yield x
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let stmt = &func.body[0];

    assert!(matches!(
        plan_instr_sequence_head(&test_context(), &instr_stmt(stmt.clone())),
        StmtSequenceHeadPlan::Linear(_)
    ));
}

#[test]
fn stmt_sequence_head_plan_leaves_assign_yield_linear() {
    let module = ruff_python_parser::parse_module(
        r#"
def gen():
    x = (yield y)
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let stmt = &func.body[0];

    assert!(matches!(
        plan_instr_sequence_head(&test_context(), &instr_stmt(stmt.clone())),
        StmtSequenceHeadPlan::Linear(_)
    ));
}

#[test]
fn stmt_sequence_head_plan_expands_simple_assign_if_expr() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    result = value if cond else other
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let stmt = &func.body[0];

    let StmtSequenceHeadPlan::Expanded(expanded) =
        plan_instr_sequence_head(&test_context(), &instr_stmt(stmt.clone()))
    else {
        panic!("expected simple conditional assignment to expand");
    };
    let [InstrRuff::StmtIf(if_stmt)] = expanded.as_slice() else {
        panic!("conditional assignment should expand to an if statement: {expanded:#?}");
    };
    assert_eq!(if_stmt.body.len(), 1);
    assert_eq!(if_stmt.orelse.len(), 1);
    assert!(matches!(
        if_stmt.body[0],
        InstrRuff::StmtAssign(ref assign)
            if matches!(assign.targets.as_slice(), [InstrRuff::ExprName(name)] if name.id.as_str() == "result")
    ));
    assert!(matches!(
        if_stmt.orelse[0],
        InstrRuff::StmtAssign(ref assign)
            if matches!(assign.targets.as_slice(), [InstrRuff::ExprName(name)] if name.id.as_str() == "result")
    ));
}

#[test]
fn stmt_sequence_head_plan_keeps_plain_return_as_plain_return() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    return x
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let stmt = &func.body[0];

    assert!(matches!(
        plan_instr_sequence_head(&test_context(), &instr_stmt(stmt.clone())),
        StmtSequenceHeadPlan::Return(_)
    ));
}

#[test]
fn stmt_sequence_head_plan_keeps_return_yield_as_plain_return() {
    let module = ruff_python_parser::parse_module(
        r#"
def gen(n):
    return (yield n)
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let stmt = &func.body[0];

    assert!(matches!(
        plan_instr_sequence_head(&test_context(), &instr_stmt(stmt.clone())),
        StmtSequenceHeadPlan::Return(_)
    ));
}

#[test]
fn stmt_sequence_head_plan_simplifies_assert_to_if() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    assert cond, msg
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let stmt = &func.body[0];

    assert!(matches!(
        plan_instr_sequence_head(&test_context(), &instr_stmt(stmt.clone())),
        StmtSequenceHeadPlan::If(_)
    ));
}

#[test]
fn stmt_sequence_head_plan_simplifies_match_to_expanded_if_chain() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    match "aa":
        case str(slot):
            return slot
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let stmt = &func.body[0];

    let StmtSequenceHeadPlan::Expanded(body) =
        plan_instr_sequence_head(&test_context(), &instr_stmt(stmt.clone()))
    else {
        panic!("expected expanded match body");
    };
    assert!(matches!(body[0], InstrRuff::StmtAssign(_)));
    assert!(body.iter().any(|stmt| matches!(stmt, InstrRuff::StmtIf(_))));
}

#[test]
fn stmt_sequence_head_plan_re_expands_builtin_match_if_head() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    match "aa":
        case str(slot):
            return slot
        case _:
            return None
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let stmt = &func.body[0];

    let StmtSequenceHeadPlan::Expanded(body) =
        plan_instr_sequence_head(&test_context(), &instr_stmt(stmt.clone()))
    else {
        panic!("expected expanded match body");
    };
    let match_if = body
        .iter()
        .find(|stmt| matches!(stmt, InstrRuff::StmtIf(_)))
        .expect("expected expanded match body to contain an if");

    assert!(
        matches!(
            plan_instr_sequence_head(&test_context(), match_if),
            StmtSequenceHeadPlan::If(_)
        ),
        "{}",
        crate::ruff_ast::ruff_ast_to_string(crate::passes::ast_to_instr::into_ast_stmt(
            match_if.clone()
        ))
        .trim_end()
    );
}

#[test]
fn blockpy_match_builtin_class_pattern_lowers_short_circuit_test_before_bb() {
    let blockpy = wrapped_blockpy(
        r#"
def f():
    match "aa":
        case str(slot):
            return slot
        case _:
            return None
"#,
    );
    let f = function_by_name(&blockpy, "f");
    assert!(!function_has_root_load(f, "__dp_match_class_attr_exists"));
    assert!(!function_has_root_load(f, "__dp_match_class_attr_value"));
}

#[test]
fn blockpy_module_match_builtin_class_pattern_keeps_subject_temp_assignment() {
    let blockpy = wrapped_blockpy(
        r#"
match "aa":
    case str(slot):
        MATCHED = slot
    case _:
        MATCHED = None
"#,
    );
    let init = function_by_name(&blockpy, "_dp_module_init");
    assert!(function_has_instr(init, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::Store(store) if store.name.id_str() == "_dp_match_1"
    )));
}

#[test]
fn lower_with_stmt_sequence_expands_via_structured_desugar() {
    let module = ruff_python_parser::parse_module(
        r#"
def f(ctx, value):
    with ctx() as value:
        body()
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let ast::Stmt::With(with_stmt) = &func.body[0] else {
        panic!("expected with stmt");
    };

    let mut blocks = Vec::new();
    let name_gen = test_name_gen();
    let context = Context::new("");
    let mut saw_try_stmt = false;
    let mut saw_with_ok_assign = false;
    let entry = lower_with_stmt_sequence(
        &context,
        match crate::passes::ast_to_instr::from_ast_stmt(Stmt::With(with_stmt.clone())) {
            InstrRuff::StmtWith(with_stmt) => with_stmt,
            _ => unreachable!(),
        },
        &[],
        RegionTargets::new(label(99), None),
        Vec::new(),
        &mut blocks,
        &name_gen,
        false,
        &mut |_expanded: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            saw_try_stmt = _expanded
                .iter()
                .any(|stmt| matches!(stmt, InstrRuff::StmtTry(_)));
            saw_with_ok_assign = _expanded.iter().any(|stmt| {
                match stmt {
                    InstrRuff::StmtAssign(assign) => assign.targets.iter().any(|target| {
                        matches!(target, InstrRuff::ExprName(name) if name.id.as_str().contains("with_ok"))
                    }),
                    _ => false,
                }
            });
            targets.normal_cont
        },
    );

    assert_eq!(entry, label(99));
    assert!(blocks.is_empty());
    assert!(saw_try_stmt);
    assert!(saw_with_ok_assign);
}

#[test]
fn lower_try_stmt_sequence_emits_entry_jump_and_except_edge() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    try:
        body()
    except ValueError:
        handle()
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let ast::Stmt::Try(try_stmt) = &func.body[0] else {
        panic!("expected try stmt");
    };

    let mut blocks = Vec::new();
    let name_gen = test_name_gen();
    let try_plan = build_try_plan(&name_gen, false, false);
    let entry_label = name_gen.next_block_name();
    let entry = lower_try_stmt_sequence(
        match crate::passes::ast_to_instr::from_ast_stmt(Stmt::Try(try_stmt.clone())) {
            InstrRuff::StmtTry(try_stmt) => try_stmt,
            _ => unreachable!(),
        },
        &[],
        RegionTargets::new(label(99), None),
        Vec::new(),
        &mut blocks,
        &name_gen,
        entry_label,
        try_plan,
        &mut |_expanded: &[InstrRuff], targets: RegionTargets, blocks: &mut Vec<TestBlock>| {
            let label = BlockLabel::from_index(100 + blocks.len());
            blocks.push(
                crate::passes::ruff_to_blockpy::compat::compat_block_from_blockpy_with_exc_target_and_expr::<
                    InstrWithAwaitAndYield,
                >(
                    &test_context(),
                    &test_name_gen(),
                    label,
                    Vec::new(),
                    BlockTerm::Jump(BlockEdge::new(targets.normal_cont)),
                    targets.active_exc.as_ref(),
                ),
            );
            label
        },
    );

    assert!(blocks.iter().any(|block| block.label == entry));
    let Some(try_entry_block) = blocks.iter().find(|block| block.label == entry) else {
        panic!("expected try entry block");
    };
    let BlockTerm::Jump(try_body_edge) = &try_entry_block.term else {
        panic!("expected try entry jump");
    };
    let Some(body_block) = blocks
        .iter()
        .find(|block| block.label == try_body_edge.target)
    else {
        panic!("expected try body block");
    };
    let exc_edge = body_block
        .exc_edge
        .as_ref()
        .expect("try body block must carry except edge");
    assert_ne!(exc_edge.target, try_body_edge.target);
    assert!(
        blocks.iter().any(|block| block.label == exc_edge.target),
        "except edge target should resolve to another block"
    );
}

#[test]
fn expanded_stmt_helper_returns_expanded_entry_without_linear_prefix() {
    let mut blocks = Vec::new();
    let mut saw_expanded = false;
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry = lower_expanded_stmt_sequence(
        &context,
        &name_gen,
        vec![instr_stmt(py_stmt!("pass"))],
        &[],
        RegionTargets::new(label(99), None),
        Vec::new(),
        &mut blocks,
        None,
        &mut |expanded: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            assert_eq!(expanded.len(), 1);
            assert_eq!(targets.normal_cont, label(99));
            saw_expanded = true;
            label(100)
        },
    );

    assert!(saw_expanded);
    assert_eq!(entry, label(100));
    assert!(blocks.is_empty());
}

#[test]
fn expanded_stmt_helper_emits_linear_jump_prefix() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry = lower_expanded_stmt_sequence(
        &context,
        &name_gen,
        vec![instr_stmt(py_stmt!("pass"))],
        &[],
        RegionTargets::new(label(99), None),
        vec![instr_stmt(py_stmt!("x = 1"))],
        &mut blocks,
        Some(label(10)),
        &mut |_expanded: &[InstrRuff], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            label(11)
        },
    );

    assert_eq!(blocks.len(), 1);
    assert_eq!(entry, blocks[0].label);
    assert!(matches!(
        &blocks[0].term,
        BlockTerm::Jump(edge) if edge.target == label(11)
    ));
}

#[test]
fn expanded_stmt_helper_emits_fragment_for_branching_linear_prefix() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry = lower_expanded_stmt_sequence(
        &context,
        &name_gen,
        vec![instr_stmt(py_stmt!("pass"))],
        &[],
        RegionTargets::new(label(99), None),
        vec![instr_stmt(py_stmt!("x = a if cond else b"))],
        &mut blocks,
        Some(label(10)),
        &mut |_expanded: &[InstrRuff], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            label(11)
        },
    );

    assert!(blocks.iter().any(|block| block.label == entry));
    assert!(
        blocks
            .iter()
            .any(|block| matches!(&block.term, BlockTerm::Jump(edge) if edge.target == label(11))),
        "{blocks:#?}"
    );
    assert!(
        blocks.len() > 1,
        "branching prefix should emit its inline dependency blocks: {blocks:#?}"
    );
}

#[test]
fn if_stmt_helper_lowers_both_branches_via_callback() {
    let mut blocks = Vec::new();
    let then_body = vec![py_stmt!("x = 1")];
    let else_body = vec![py_stmt!("x = 2")];
    let mut calls = Vec::new();
    let context = Context::new("");

    let entry = lower_if_stmt_sequence(
        &context,
        &mut blocks,
        &test_name_gen(),
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        instr_expr(py_expr!("flag")),
        &then_body
            .iter()
            .cloned()
            .map(instr_stmt)
            .collect::<Vec<_>>(),
        &else_body
            .iter()
            .cloned()
            .map(instr_stmt)
            .collect::<Vec<_>>(),
        label(99),
        &RegionTargets::new(label(99), None),
        &mut |stmts: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            calls.push((stmts.len(), targets.normal_cont.clone()));
            label(200 + calls.len() as u32)
        },
    );

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert_eq!(
        calls
            .into_iter()
            .map(|(len, label)| (len, label))
            .collect::<Vec<_>>(),
        vec![(then_body.len(), label(99)), (else_body.len(), label(99))]
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0].term, BlockTerm::IfTerm(_)));
}

#[test]
fn if_stmt_helper_lowers_if_expr_test_via_inline_fragment() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();

    let entry = lower_if_stmt_sequence(
        &context,
        &mut blocks,
        &name_gen,
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        instr_expr(py_expr!("left if cond else right")),
        &vec![instr_stmt(py_stmt!("x = 1"))],
        &vec![instr_stmt(py_stmt!("x = 2"))],
        label(99),
        &RegionTargets::new(label(99), None),
        &mut |_stmts: &[InstrRuff], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            label(200)
        },
    );

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert!(
        !blocks_store_to_prefix(&blocks, "_dp_tmp_"),
        "branch-only if-expression lowering should not materialize a selected-value temp: {blocks:#?}"
    );
    let if_labels = blocks
        .iter()
        .filter_map(|block| match block.term {
            BlockTerm::IfTerm(_) => Some(block.label),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(if_labels.len(), 3, "{blocks:#?}");
    let jump_targets = blocks
        .iter()
        .filter_map(|block| match &block.term {
            BlockTerm::Jump(BlockEdge { target, args }) if args.is_empty() => Some(*target),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        jump_targets
            .iter()
            .any(|&target| if_labels.contains(&target)),
        "{blocks:#?}"
    );
}

#[test]
fn if_stmt_helper_lowers_boolop_test_via_inline_fragment() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();

    let entry = lower_if_stmt_sequence(
        &context,
        &mut blocks,
        &name_gen,
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        instr_expr(py_expr!("left and right")),
        &vec![instr_stmt(py_stmt!("x = 1"))],
        &vec![instr_stmt(py_stmt!("x = 2"))],
        label(99),
        &RegionTargets::new(label(99), None),
        &mut |_stmts: &[InstrRuff], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            label(200)
        },
    );

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert!(
        !blocks_store_to_prefix(&blocks, "_dp_target_"),
        "branch-only boolop lowering should not materialize a selected-value temp: {blocks:#?}"
    );
    let if_count = blocks
        .iter()
        .filter(|block| matches!(block.term, BlockTerm::IfTerm(_)))
        .count();
    assert_eq!(if_count, 2, "{blocks:#?}");
}

#[test]
fn if_stmt_helper_lowers_not_boolop_test_via_inline_fragment() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();

    let entry = lower_if_stmt_sequence(
        &context,
        &mut blocks,
        &name_gen,
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        instr_expr(py_expr!("not (left and right)")),
        &vec![instr_stmt(py_stmt!("x = 1"))],
        &vec![instr_stmt(py_stmt!("x = 2"))],
        label(99),
        &RegionTargets::new(label(99), None),
        &mut |_stmts: &[InstrRuff], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            label(200)
        },
    );

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert!(
        !blocks_store_to_prefix(&blocks, "_dp_target_"),
        "branch-only not-boolop lowering should not materialize a selected-value temp: {blocks:#?}"
    );
    let if_count = blocks
        .iter()
        .filter(|block| matches!(block.term, BlockTerm::IfTerm(_)))
        .count();
    assert_eq!(if_count, 2, "{blocks:#?}");
}

#[test]
fn if_stmt_helper_lowers_compare_chain_test_via_inline_fragment() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();

    let entry = lower_if_stmt_sequence(
        &context,
        &mut blocks,
        &name_gen,
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        instr_expr(py_expr!("a < b < c")),
        &vec![instr_stmt(py_stmt!("x = 1"))],
        &vec![instr_stmt(py_stmt!("x = 2"))],
        label(99),
        &RegionTargets::new(label(99), None),
        &mut |_stmts: &[InstrRuff], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            label(200)
        },
    );

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    let setup_label = blocks
        .iter()
        .find_map(|block| match &block.term {
            BlockTerm::Jump(BlockEdge { target, args })
                if block.label == entry && args.is_empty() =>
            {
                Some(*target)
            }
            _ => None,
        })
        .expect("entry should jump to the first compare-test block");
    assert!(
        blocks
            .iter()
            .find(|block| block.label == setup_label)
            .is_some_and(|block| matches!(block.term, BlockTerm::IfTerm(_))),
        "{blocks:#?}"
    );
    assert!(
        !blocks_store_to_prefix(&blocks, "_dp_target_"),
        "branch-only compare-chain lowering should not materialize a selected-value temp: {blocks:#?}"
    );
    let second_if_label = blocks
        .iter()
        .find(|block| block.label != setup_label && matches!(block.term, BlockTerm::IfTerm(_)))
        .map(|block| block.label)
        .expect("second if block should exist");
    assert!(
        blocks.iter().any(|block| matches!(
            &block.term,
            BlockTerm::IfTerm(if_term) if block.label == setup_label && if_term.then_label == second_if_label
        )),
        "{blocks:#?}"
    );
    assert!(
        blocks.iter().any(|block| matches!(
            block.term,
            BlockTerm::Jump(BlockEdge { target, ref args })
                if target == setup_label && args.is_empty()
        )),
        "{blocks:#?}"
    );
}

#[test]
fn sequence_jump_helper_emits_jump_block() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry = emit_sequence_jump_block::<InstrWithAwaitAndYield>(
        &context,
        &mut blocks,
        &name_gen,
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        label(11),
        None,
    );

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        &blocks[0].term,
        BlockTerm::Jump(edge) if edge.target == label(11)
    ));
}

#[test]
fn sequence_return_helper_emits_return_block() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let entry = emit_sequence_return_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
        &context,
        &mut blocks,
        &test_name_gen(),
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        Some(instr_expr(py_expr!("value"))),
        None,
    )
    .expect("sequence return helper should lower");

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0].term, BlockTerm::Return(_)));
}

#[test]
fn sequence_raise_helper_emits_raise_block() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let entry = emit_sequence_raise_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
        &context,
        &mut blocks,
        &test_name_gen(),
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        TermRaise {
            disposition: RaiseDisposition::Source,
            exc: Some(crate::passes::ast_to_instr::from_ast_expr(py_expr!("exc"))),
        },
        None,
    )
    .expect("sequence raise helper should lower");

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(
        blocks[0].term,
        BlockTerm::Raise(TermRaise { exc: Some(_), .. })
    ));
}

#[test]
fn sequence_return_helper_lowers_boolop_via_inline_fragment() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry = emit_sequence_return_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
        &context,
        &mut blocks,
        &name_gen,
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        Some(instr_expr(py_expr!("lhs() and rhs()"))),
        None,
    )
    .expect("sequence return helper should lower");

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert!(blocks.len() > 1, "{blocks:#?}");
    assert!(blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::Return(_))));
}

#[test]
fn sequence_raise_helper_lowers_compare_chain_via_inline_fragment() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry = emit_sequence_raise_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
        &context,
        &mut blocks,
        &name_gen,
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        TermRaise {
            disposition: RaiseDisposition::Source,
            exc: Some(crate::passes::ast_to_instr::from_ast_expr(py_expr!(
                "a() < b() < c()"
            ))),
        },
        None,
    )
    .expect("sequence raise helper should lower");

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert!(blocks.len() > 1, "{blocks:#?}");
    assert!(blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::Raise(TermRaise { exc: Some(_), .. }))));
}

fn assert_all_block_targets_present<E: Instr>(blocks: &[Block<E>]) {
    let labels = blocks
        .iter()
        .map(|block| block.label)
        .collect::<std::collections::HashSet<_>>();
    for block in blocks {
        let check = |target: BlockLabel, kind: &str| {
            assert!(
                target.is_fallthrough() || labels.contains(&target),
                "dangling {kind} from {} to {} in {blocks:#?}",
                block.label,
                target,
            );
        };
        match &block.term {
            BlockTerm::Jump(edge) => check(edge.target, "jump"),
            BlockTerm::IfTerm(if_term) => {
                check(if_term.then_label, "then");
                check(if_term.else_label, "else");
            }
            BlockTerm::BranchTable(branch) => {
                for target in &branch.targets {
                    check(*target, "branch");
                }
                check(branch.default_label, "branch default");
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {}
        }
        if let Some(edge) = &block.exc_edge {
            check(edge.target, "exception");
        }
    }
}

fn blocks_store_to_prefix(blocks: &[TestBlock], prefix: &str) -> bool {
    blocks.iter().any(|block| {
        block.body.iter().any(|stmt| {
            matches!(
                stmt,
                InstrWithAwaitAndYield::Store(store) if store.name.id_str().starts_with(prefix)
            )
        })
    })
}

fn blocks_store_to_name(blocks: &[TestBlock], name: &str) -> usize {
    blocks
        .iter()
        .map(|block| {
            block
                .body
                .iter()
                .filter(|stmt| {
                    matches!(
                        stmt,
                        InstrWithAwaitAndYield::Store(store) if store.name.id_str() == name
                    )
                })
                .count()
        })
        .sum()
}

fn blocks_have_param_role(blocks: &[TestBlock], role: BlockParamRole) -> bool {
    blocks
        .iter()
        .any(|block| block.params.iter().any(|param| param.role == role))
}

fn return_load_names(blocks: &[TestBlock]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|block| match &block.term {
            BlockTerm::Return(InstrWithAwaitAndYield::Load(load)) => {
                Some(load.name.id_str().to_string())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn sequence_lowering_assign_boolop_local_or_global_without_selected_target_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
BoolGlob = False

def f(BoolLoc):
    BoolLoc = BoolLoc or BoolGlob
    return BoolLoc
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_target_"),
        "direct boolop assignment should not materialize a selected-value target: {:#?}",
        function.blocks
    );
    assert!(
        !blocks_have_param_role(&function.blocks, BlockParamRole::Value),
        "direct boolop assignment should store into the real target instead of a value carrier: {:#?}",
        function.blocks
    );
    assert!(
        blocks_store_to_name(&function.blocks, "BoolLoc") >= 2,
        "selected and final arms should store directly into BoolLoc: {:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_returns_boolop_local_or_global_without_selected_target_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
BoolGlob = False

def f(BoolLoc):
    return BoolLoc or BoolGlob
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_target_"),
        "direct boolop return should not materialize a selected-value target: {:#?}",
        function.blocks
    );
    assert!(
        !blocks_have_param_role(&function.blocks, BlockParamRole::Value),
        "direct boolop return should not need a value carrier: {:#?}",
        function.blocks
    );
    let return_names = return_load_names(&function.blocks);
    assert!(
        return_names.iter().any(|name| name == "BoolLoc"),
        "{:#?}",
        function.blocks
    );
    assert!(
        return_names.iter().any(|name| name == "BoolGlob"),
        "{:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_passes_boolop_value_arg_without_selected_target_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(left, right, sink):
    return sink(left and right)
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_target_"),
        "nested boolop value should use a value block param instead of a selected-value target: {:#?}",
        function.blocks
    );
    assert!(
        blocks_have_param_role(&function.blocks, BlockParamRole::Value),
        "nested boolop value should introduce an explicit value carrier block param: {:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_passes_boolop_value_arg_with_final_global_without_selected_target_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
BoolGlob = False

def f(left, sink):
    return sink(left or BoolGlob)
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_target_"),
        "nested boolop value should not initialize a selected-value target before testing left: {:#?}",
        function.blocks
    );
    assert!(
        blocks_have_param_role(&function.blocks, BlockParamRole::Value),
        "nested boolop value with final global should still join through a value block param: {:#?}",
        function.blocks
    );
    assert!(
        blocks_store_to_prefix(&function.blocks, "_dp_value_"),
        "only the non-forwardable final global path should materialize the value carrier: {:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_raises_boolop_locals_without_selected_target_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(exc, fallback):
    raise exc or fallback
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_target_"),
        "direct boolop raise should not materialize a selected-value target: {:#?}",
        function.blocks
    );
    assert!(
        !blocks_have_param_role(&function.blocks, BlockParamRole::Value),
        "direct boolop raise should not need a value carrier: {:#?}",
        function.blocks
    );
    assert!(
        function
            .blocks
            .iter()
            .filter(|block| matches!(block.term, BlockTerm::Raise(TermRaise { exc: Some(_), .. })))
            .count()
            >= 2,
        "{:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_expands_assign_if_expr_without_synthetic_temp_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(cond, value, other):
    result = value if cond else other
    return result
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_tmp_"),
        "simple conditional assignment should not materialize a selected-value temp: {:#?}",
        function.blocks
    );
    assert_eq!(
        blocks_store_to_name(&function.blocks, "result"),
        2,
        "selected arms should store directly into the real target: {:#?}",
        function.blocks
    );
    assert!(
        function
            .blocks
            .iter()
            .any(|block| matches!(block.term, BlockTerm::IfTerm(_))),
        "{:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_lowers_simple_attribute_target_without_object_temp_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(obj, value):
    obj.field = value
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_assign_obj_"),
        "simple local attribute receivers should stay borrowed instead of using an owned target-object temp: {:#?}",
        function.blocks
    );
    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_assign_value_"),
        "simple local attribute assignment RHS values should stay borrowed instead of using an owned RHS temp: {:#?}",
        function.blocks
    );
    assert!(
        function_instr_any(function, |instr| matches!(
            instr,
            InstrWithAwaitAndYield::SetAttr(_)
        )),
        "{:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_lowers_simple_subscript_target_without_object_or_index_temp_slots() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(obj, idx, value):
    obj[idx] = value
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_assign_obj_"),
        "simple local subscript receivers should stay borrowed instead of using an owned target-object temp: {:#?}",
        function.blocks
    );
    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_assign_index_"),
        "simple local subscript indexes should stay borrowed instead of using an owned target-index temp: {:#?}",
        function.blocks
    );
    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_assign_value_"),
        "simple local subscript assignment RHS values should stay borrowed instead of using an owned RHS temp: {:#?}",
        function.blocks
    );
    assert!(
        function_instr_any(function, |instr| matches!(
            instr,
            InstrWithAwaitAndYield::SetItem(_)
        )),
        "{:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_lowers_simple_subscript_target_without_rhs_temp_for_local_index() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(obj, value):
    idx = 0
    obj[idx] = value
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_assign_value_"),
        "subscript RHS values should stay borrowed when the object and RHS are no-raise locals and the index is a simple local load: {:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_keeps_assignment_value_temp_for_effectful_subscript_index() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(obj, value, g):
    obj[g()] = value
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        blocks_store_to_prefix(&function.blocks, "_dp_assign_value_"),
        "RHS temps must stay when a subscript index may have side effects before the target update: {:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_lowering_keeps_assignment_value_temp_when_rhs_param_can_be_deleted() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(obj, value):
    del value
    obj.field = value
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        blocks_store_to_prefix(&function.blocks, "_dp_assign_value_"),
        "RHS temps must stay when a parameter can be deleted before the assignment: {:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_stmt_expr_lowers_if_expr_effect_only_without_synthetic_temp_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(cond, value, other):
    value if cond else other
    return None
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_tmp_"),
        "effect-only conditional expression should not materialize a selected-value temp: {:#?}",
        function.blocks
    );
    assert!(
        function
            .blocks
            .iter()
            .any(|block| matches!(block.term, BlockTerm::IfTerm(_))),
        "{:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_stmt_expr_lowers_boolop_effect_only_without_selected_target_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(left, right, fallback):
    left and right
    left or fallback
    not (left and right)
    return None
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_target_"),
        "effect-only boolops should not materialize a selected-value target: {:#?}",
        function.blocks
    );
    assert!(
        function
            .blocks
            .iter()
            .filter(|block| matches!(block.term, BlockTerm::IfTerm(_)))
            .count()
            >= 2,
        "{:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_stmt_expr_lowers_compare_chain_effect_only_without_selected_target_slot() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
def f(a, b, c):
    a < b < c
    a < b < c < a
    return None
"#,
    );
    let function = function_by_name(&blockpy, "f");

    assert!(
        !blocks_store_to_prefix(&function.blocks, "_dp_target_"),
        "effect-only compare chains should not materialize a selected-value target: {:#?}",
        function.blocks
    );
    assert!(
        blocks_store_to_prefix(&function.blocks, "_dp_compare_"),
        "compare chains still need one evaluated comparator carrier before the final comparison: {:#?}",
        function.blocks
    );
    assert_all_block_targets_present(&function.blocks);
}

#[test]
fn sequence_return_helper_lowers_if_expr_without_synthetic_temp_slot() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry = emit_sequence_return_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
        &context,
        &mut blocks,
        &name_gen,
        vec![],
        Some(instr_expr(py_expr!("value if cond else other"))),
        None,
    )
    .expect("sequence return helper should lower");

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert!(
        !blocks_store_to_prefix(&blocks, "_dp_tmp_"),
        "direct return should not materialize a conditional expression temp: {blocks:#?}"
    );
    let return_names = return_load_names(&blocks);
    assert!(
        return_names.iter().any(|name| name == "value"),
        "{blocks:#?}"
    );
    assert!(
        return_names.iter().any(|name| name == "other"),
        "{blocks:#?}"
    );
    assert_all_block_targets_present(&blocks);
}

#[test]
fn sequence_return_helper_keeps_direct_if_expr_setup_reachable_after_linear_prefix() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry = emit_sequence_return_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
        &context,
        &mut blocks,
        &name_gen,
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        Some(instr_expr(py_expr!("value if cond else other"))),
        None,
    )
    .expect("sequence return helper should lower");

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert!(blocks.len() > 1, "{blocks:#?}");
    assert!(blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::Return(_))));
    assert!(
        !blocks_store_to_prefix(&blocks, "_dp_tmp_"),
        "direct return should not materialize a conditional expression temp: {blocks:#?}"
    );
    assert_all_block_targets_present(&blocks);
}

#[test]
fn sequence_raise_helper_keeps_direct_if_expr_setup_reachable_after_linear_prefix() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry = emit_sequence_raise_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
        &context,
        &mut blocks,
        &name_gen,
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        TermRaise {
            disposition: RaiseDisposition::Source,
            exc: Some(crate::passes::ast_to_instr::from_ast_expr(py_expr!(
                "value if cond else other"
            ))),
        },
        None,
    )
    .expect("sequence raise helper should lower");

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert!(blocks.len() > 1, "{blocks:#?}");
    assert!(blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::Raise(TermRaise { exc: Some(_), .. }))));
    assert!(
        !blocks_store_to_prefix(&blocks, "_dp_tmp_"),
        "direct raise should not materialize a conditional expression temp: {blocks:#?}"
    );
    assert_all_block_targets_present(&blocks);
}

#[test]
fn inline_fragment_helper_splices_fallthrough_into_entry_and_deps() {
    let mut blocks = Vec::new();
    let name_gen = test_name_gen();
    let fragment = InlineFragment::new(
        Block::new(
            name_gen.next_block_name(),
            vec![InstrWithAwaitAndYield::from_ast_expr(py_expr!("head()"))],
            BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
            Vec::new(),
            None,
        ),
        vec![Block::new(
            name_gen.next_block_name(),
            vec![InstrWithAwaitAndYield::from_ast_expr(py_expr!("tail()"))],
            BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
            Vec::new(),
            None,
        )],
    );

    let entry = emit_inline_fragment_with_exc_target_and_expr::<InstrWithAwaitAndYield>(
        &mut blocks,
        fragment,
        label(99),
        None,
    );

    assert_eq!(entry.label(), blocks[0].label);
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        blocks[0].term,
        BlockTerm::Jump(BlockEdge { target, ref args })
            if target == label(99) && args.is_empty()
    ));
    assert!(matches!(
        blocks[1].term,
        BlockTerm::Jump(BlockEdge { target, ref args })
            if target == label(99) && args.is_empty()
    ));
}

#[test]
fn if_stmt_from_stmt_helper_lowers_remaining_and_branches() {
    let module = ruff_python_parser::parse_module(
        r#"
if flag:
    x = 1
else:
    x = 2
y = 3
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::If(if_stmt) = &module[0] else {
        panic!("expected if stmt");
    };
    let remaining = vec![module[1].clone()];
    let mut blocks = Vec::new();
    let mut calls = Vec::new();
    let context = Context::new("");

    let entry = lower_if_stmt_sequence_from_stmt(
        &context,
        &test_name_gen(),
        match crate::passes::ast_to_instr::from_ast_stmt(Stmt::If(if_stmt.clone())) {
            InstrRuff::StmtIf(if_stmt) => if_stmt,
            _ => unreachable!(),
        },
        &remaining
            .iter()
            .cloned()
            .map(instr_stmt)
            .collect::<Vec<_>>(),
        RegionTargets::new(label(99), None),
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        &mut blocks,
        &mut |stmts: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            calls.push((stmts.len(), targets.normal_cont.clone()));
            label(200 + calls.len() as u32)
        },
    );

    let fragment_label = blocks
        .iter()
        .find_map(|block| match &block.term {
            BlockTerm::Jump(BlockEdge { target, args })
                if block.label == entry && args.is_empty() =>
            {
                Some(*target)
            }
            _ => None,
        })
        .expect("entry should be a linear prefix jump into the lowered if fragment");
    assert_eq!(
        calls
            .into_iter()
            .map(|(len, label)| (len, label))
            .collect::<Vec<_>>(),
        vec![(remaining.len(), label(99))]
    );
    assert_eq!(blocks.len(), 4);
    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert!(
        blocks
            .iter()
            .find(|block| block.label == fragment_label)
            .is_some_and(|block| matches!(block.term, BlockTerm::IfTerm(_))),
        "{blocks:#?}"
    );
    let jump_targets = blocks
        .iter()
        .filter_map(|block| match &block.term {
            BlockTerm::Jump(BlockEdge { target, args }) if args.is_empty() => Some(*target),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        jump_targets
            .iter()
            .filter(|&&target| target == label(201))
            .count(),
        2,
        "{blocks:#?}"
    );
    assert!(
        jump_targets.iter().any(|&target| target == fragment_label),
        "{blocks:#?}"
    );
}

#[test]
fn if_stmt_from_stmt_helper_inlines_fragment_compatible_branch_setup() {
    let module = ruff_python_parser::parse_module(
        r#"
if flag:
    x = (y if cond else z)
else:
    x = 2
done = 1
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::If(if_stmt) = &module[0] else {
        panic!("expected if stmt");
    };
    let remaining = vec![module[1].clone()];
    let mut blocks = Vec::new();
    let mut calls = Vec::new();
    let context = Context::new("");

    let entry = lower_if_stmt_sequence_from_stmt(
        &context,
        &test_name_gen(),
        match crate::passes::ast_to_instr::from_ast_stmt(Stmt::If(if_stmt.clone())) {
            InstrRuff::StmtIf(if_stmt) => if_stmt,
            _ => unreachable!(),
        },
        &remaining
            .iter()
            .cloned()
            .map(instr_stmt)
            .collect::<Vec<_>>(),
        RegionTargets::new(label(99), None),
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        &mut blocks,
        &mut |stmts: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            calls.push((stmts.len(), targets.normal_cont.clone()));
            label(200 + calls.len() as u32)
        },
    );

    assert!(
        blocks.iter().any(|block| block.label == entry),
        "{blocks:#?}"
    );
    assert_eq!(
        calls
            .into_iter()
            .map(|(len, label)| (len, label))
            .collect::<Vec<_>>(),
        vec![(remaining.len(), label(99))]
    );
    assert!(blocks.len() > 1, "{blocks:#?}");
    assert!(
        blocks
            .iter()
            .any(|block| matches!(block.term, BlockTerm::IfTerm(_))),
        "{blocks:#?}"
    );
}

#[test]
fn while_stmt_helper_lowers_loop_and_else_via_callbacks() {
    let mut blocks = Vec::new();
    let body = vec![py_stmt!("x = 1")];
    let else_body = vec![py_stmt!("x = 2")];
    let remaining = vec![py_stmt!("x = 3")];
    let mut sequence_calls = Vec::new();
    let mut loop_calls = Vec::new();
    let context = Context::new("");

    let entry = lower_while_stmt_sequence(
        &context,
        &mut blocks,
        &test_name_gen(),
        label(0),
        Some(label(1)),
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        instr_expr(py_expr!("flag")),
        &body.iter().cloned().map(instr_stmt).collect::<Vec<_>>(),
        &else_body
            .iter()
            .cloned()
            .map(instr_stmt)
            .collect::<Vec<_>>(),
        &remaining
            .iter()
            .cloned()
            .map(instr_stmt)
            .collect::<Vec<_>>(),
        RegionTargets::new(label(99), None),
        &mut |stmts: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            if let Some(loop_labels) = targets.loop_labels {
                loop_calls.push((
                    stmts.len(),
                    targets.normal_cont.clone(),
                    loop_labels.break_label,
                ));
                label(250)
            } else {
                sequence_calls.push((stmts.len(), targets.normal_cont.clone()));
                label(200 + sequence_calls.len() as u32)
            }
        },
    );

    assert_eq!(entry, label(1));
    assert_eq!(
        sequence_calls
            .into_iter()
            .map(|(len, label)| (len, label))
            .collect::<Vec<_>>(),
        vec![(remaining.len(), label(99)), (else_body.len(), label(201))]
    );
    assert_eq!(
        loop_calls
            .into_iter()
            .map(|(len, normal, break_label)| (len, normal, break_label))
            .collect::<Vec<_>>(),
        vec![(body.len(), label(0), label(201))]
    );
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].label, label(0));
    assert_eq!(blocks[1].label, label(1));
}

#[test]
fn while_stmt_helper_lowers_boolop_test_via_inline_fragment() {
    let mut blocks = Vec::new();
    let body = vec![py_stmt!("x = 1")];
    let remaining = vec![py_stmt!("x = 3")];
    let mut sequence_calls = Vec::new();
    let mut loop_calls = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let test_label = name_gen.next_block_name();
    let linear_label = name_gen.next_block_name();

    let entry = lower_while_stmt_sequence(
        &context,
        &mut blocks,
        &name_gen,
        test_label,
        Some(linear_label),
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        instr_expr(py_expr!("lhs() and rhs()")),
        &body.iter().cloned().map(instr_stmt).collect::<Vec<_>>(),
        &[],
        &remaining
            .iter()
            .cloned()
            .map(instr_stmt)
            .collect::<Vec<_>>(),
        RegionTargets::new(label(99), None),
        &mut |stmts: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            if let Some(loop_labels) = targets.loop_labels {
                loop_calls.push((
                    stmts.len(),
                    targets.normal_cont.clone(),
                    loop_labels.break_label,
                ));
                label(250)
            } else {
                sequence_calls.push((stmts.len(), targets.normal_cont.clone()));
                label(200 + sequence_calls.len() as u32)
            }
        },
    );

    assert_eq!(entry, linear_label);
    assert_eq!(
        sequence_calls
            .into_iter()
            .map(|(len, label)| (len, label))
            .collect::<Vec<_>>(),
        vec![(remaining.len(), label(99))]
    );
    assert_eq!(
        loop_calls
            .into_iter()
            .map(|(len, normal, break_label)| (len, normal, break_label))
            .collect::<Vec<_>>(),
        vec![(body.len(), test_label, label(201))]
    );
    assert!(blocks.len() > 2, "{blocks:#?}");
    assert!(
        blocks.iter().any(|block| block.label == test_label),
        "{blocks:#?}"
    );
    assert!(
        blocks.iter().any(|block| block.label == linear_label),
        "{blocks:#?}"
    );
    assert!(
        !blocks_store_to_prefix(&blocks, "_dp_target_"),
        "branch-only while boolop lowering should not materialize a selected-value temp: {blocks:#?}"
    );
    assert!(blocks
        .iter()
        .any(|block| block.label != test_label && block.label != linear_label));
}

#[test]
fn while_stmt_helper_lowers_compare_chain_test_via_inline_fragment() {
    let mut blocks = Vec::new();
    let body = vec![py_stmt!("x = 1")];
    let remaining = vec![py_stmt!("x = 3")];
    let mut sequence_calls = Vec::new();
    let mut loop_calls = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let test_label = name_gen.next_block_name();
    let linear_label = name_gen.next_block_name();

    let entry = lower_while_stmt_sequence(
        &context,
        &mut blocks,
        &name_gen,
        test_label,
        Some(linear_label),
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        instr_expr(py_expr!("a() < b() < c()")),
        &body.iter().cloned().map(instr_stmt).collect::<Vec<_>>(),
        &[],
        &remaining
            .iter()
            .cloned()
            .map(instr_stmt)
            .collect::<Vec<_>>(),
        RegionTargets::new(label(99), None),
        &mut |stmts: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            if let Some(loop_labels) = targets.loop_labels {
                loop_calls.push((
                    stmts.len(),
                    targets.normal_cont.clone(),
                    loop_labels.break_label,
                ));
                label(250)
            } else {
                sequence_calls.push((stmts.len(), targets.normal_cont.clone()));
                label(200 + sequence_calls.len() as u32)
            }
        },
    );

    assert_eq!(entry, linear_label);
    assert_eq!(
        sequence_calls
            .into_iter()
            .map(|(len, label)| (len, label))
            .collect::<Vec<_>>(),
        vec![(remaining.len(), label(99))]
    );
    assert_eq!(
        loop_calls
            .into_iter()
            .map(|(len, normal, break_label)| (len, normal, break_label))
            .collect::<Vec<_>>(),
        vec![(body.len(), test_label, label(201))]
    );
    assert!(blocks.len() > 2, "{blocks:#?}");
    assert!(
        blocks.iter().any(|block| block.label == test_label),
        "{blocks:#?}"
    );
    assert!(
        blocks.iter().any(|block| block.label == linear_label),
        "{blocks:#?}"
    );
    assert!(
        !blocks_store_to_prefix(&blocks, "_dp_target_"),
        "branch-only while compare-chain lowering should not materialize a selected-value temp: {blocks:#?}"
    );
    assert!(blocks
        .iter()
        .any(|block| block.label != test_label && block.label != linear_label));
}

#[test]
fn while_stmt_from_stmt_helper_lowers_remaining_loop_and_else() {
    let module = ruff_python_parser::parse_module(
        r#"
while flag:
    x = 1
else:
    x = 2
y = 3
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::While(while_stmt) = &module[0] else {
        panic!("expected while stmt");
    };
    let remaining = vec![module[1].clone()];
    let mut blocks = Vec::new();
    let mut sequence_calls = Vec::new();
    let mut loop_calls = Vec::new();
    let context = Context::new("");

    let entry = lower_while_stmt_sequence_from_stmt(
        &context,
        &test_name_gen(),
        match crate::passes::ast_to_instr::from_ast_stmt(Stmt::While(while_stmt.clone())) {
            InstrRuff::StmtWhile(while_stmt) => while_stmt,
            _ => unreachable!(),
        },
        &remaining
            .iter()
            .cloned()
            .map(instr_stmt)
            .collect::<Vec<_>>(),
        RegionTargets::new(label(99), None),
        vec![instr_stmt(py_stmt!("prefix = 0"))],
        &mut blocks,
        label(0),
        Some(label(1)),
        &mut |stmts: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            if let Some(loop_labels) = targets.loop_labels {
                loop_calls.push((
                    stmts.len(),
                    targets.normal_cont.clone(),
                    loop_labels.break_label,
                ));
                label(250)
            } else {
                sequence_calls.push((stmts.len(), targets.normal_cont.clone()));
                label(200 + sequence_calls.len() as u32)
            }
        },
    );

    assert_eq!(entry, label(1));
    assert_eq!(
        sequence_calls
            .into_iter()
            .map(|(len, label)| (len, label))
            .collect::<Vec<_>>(),
        vec![(remaining.len(), label(99)), (1, label(201))]
    );
    assert_eq!(
        loop_calls
            .into_iter()
            .map(|(len, normal, break_label)| (len, normal, break_label))
            .collect::<Vec<_>>(),
        vec![(1, label(0), label(201))]
    );
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0].label, label(0));
    assert_eq!(blocks[1].label, label(1));
}

#[test]
fn lowers_generator_yield_from_to_explicit_blockpy_dispatch() {
    let blockpy = wrapped_core_blockpy(
        r#"
def gen(it):
    yield from it
"#,
    );
    let gen = function_by_name(&blockpy, "gen");
    assert!(gen
        .blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::BranchTable(_))));
    assert!(function_has_root_load(gen, "exception_matches"));
    assert!(function_has_root_load(gen, "getattr"));
    assert!(!function_has_root_load(
        gen,
        "__dp_generator_yield_from_step"
    ));
}

#[test]
fn lowers_async_generator_yield_to_explicit_blockpy_dispatch() {
    let blockpy = wrapped_core_blockpy(
        r#"
async def agen(n):
    yield n
"#,
    );
    let agen = function_by_name(&blockpy, "agen");
    assert_eq!(agen.kind, FunctionKind::AsyncGenerator);
    assert!(agen
        .blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::BranchTable(_))));
}

#[test]
fn lowers_coroutine_completion_outside_user_exception_region() {
    let blockpy = wrapped_core_blockpy(
        r#"
async def outer(inner):
    try:
        value = await inner()
        return ("ok", False)
    except Exception:
        return ("StopIteration", True)
"#,
    );
    let resume = function_by_name(&blockpy, "outer");
    let completion_blocks = resume
        .blocks
        .iter()
        .filter(|block| matches!(block.term, BlockTerm::GeneratorReturn(_)))
        .collect::<Vec<_>>();
    assert!(
        !completion_blocks.is_empty(),
        "coroutine completion must use its explicit terminal operation"
    );
    for block in completion_blocks {
        assert_eq!(
            block.extra.handled_exception,
            soac_core::block_py::HandledExceptionContext::Terminal,
            "completion cleanup runs outside the suspended activation"
        );
        assert_eq!(
            lowered_exception_edges(&resume.blocks)
                .get(&block.label)
                .cloned()
                .flatten(),
            None,
            "generator completion must bypass source exception handlers"
        );
    }
}

#[test]
fn lowers_assert_if_it_reaches_blockpy_stmt_lowering() {
    let module = ruff_python_parser::parse_module(
        r#"
def f(x):
    assert x
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let context = test_context();
    let name_gen = test_name_gen();
    let mut out = crate::passes::ruff_to_blockpy::InlineBlockBuilder::<InstrWithAwaitAndYield>::new(
        &name_gen,
    );
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(func.body[0].clone());
    lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None)
        .expect("assert lowering should succeed");
    let fragment = out.finish();
    assert!(!fragment.deps.is_empty());
}

#[test]
#[should_panic(expected = "ClassDef X should be lowered before Ruff AST -> BlockPy conversion")]
fn panics_if_classdef_reaches_blockpy() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    class X:
        pass
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    lower_stmt_for_panic_test(&func.body[0]);
}

#[test]
fn lowers_augassign_if_it_reaches_blockpy_stmt_lowering() {
    let module = ruff_python_parser::parse_module(
        r#"
def f(x):
    x += 1
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let context = test_context();
    let name_gen = test_name_gen();
    let mut out = crate::passes::ruff_to_blockpy::InlineBlockBuilder::<InstrWithAwaitAndYield>::new(
        &name_gen,
    );
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(func.body[0].clone());
    lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None)
        .expect("augassign lowering should succeed");
    let fragment = out.finish();
    assert!(fragment.entry.body.iter().any(|instr| {
        matches!(instr, InstrWithAwaitAndYield::Store(store)
            if matches!(store.value.as_ref(), InstrWithAwaitAndYield::BinOp(op)
                if op.kind == crate::block_py::BinOpKind::InplaceAdd))
    }));
}

#[test]
#[should_panic(expected = "AnnAssign should be lowered before Ruff AST -> BlockPy conversion")]
fn panics_if_annassign_reaches_blockpy() {
    let module = ruff_python_parser::parse_module(
        r#"
def f(x):
    y: int = x
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    lower_stmt_for_panic_test(&func.body[0]);
}

#[test]
fn lowers_typealias_if_it_reaches_blockpy_stmt_lowering() {
    let module = ruff_python_parser::parse_module(
        r#"
type X = int

def f():
    return 1
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let context = test_context();
    let name_gen = test_name_gen();
    let mut out = crate::passes::ruff_to_blockpy::InlineBlockBuilder::<InstrWithAwaitAndYield>::new(
        &name_gen,
    );
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(module[0].clone());
    lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None)
        .expect("type alias lowering should succeed");
    let fragment = out.finish();
    assert!(!fragment.entry.body.is_empty());
}

#[test]
fn lowers_match_if_it_reaches_blockpy_stmt_lowering() {
    let module = ruff_python_parser::parse_module(
        r#"
def f(x):
    match x:
        case 1:
            return 1
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let context = test_context();
    let name_gen = test_name_gen();
    let mut out = crate::passes::ruff_to_blockpy::InlineBlockBuilder::<InstrWithAwaitAndYield>::new(
        &name_gen,
    );
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(func.body[0].clone());
    lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None)
        .expect("match lowering should succeed");
    let fragment = out.finish();
    assert!(!fragment.entry.body.is_empty() || !fragment.deps.is_empty());
}

#[test]
fn lowers_plain_import_if_it_reaches_blockpy_stmt_lowering() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    import os
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let context = test_context();
    let name_gen = test_name_gen();
    let mut out = crate::passes::ruff_to_blockpy::InlineBlockBuilder::<InstrWithAwaitAndYield>::new(
        &name_gen,
    );
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(func.body[0].clone());
    lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None)
        .expect("import lowering should succeed");
    let fragment = out.finish();
    assert!(matches!(
        fragment.entry.body.as_slice(),
        [InstrWithAwaitAndYield::Store(_)]
    ));
}

#[test]
fn lowers_importfrom_if_it_reaches_blockpy_stmt_lowering() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    from math import sqrt
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let context = test_context();
    let name_gen = test_name_gen();
    let mut out = crate::passes::ruff_to_blockpy::InlineBlockBuilder::<InstrWithAwaitAndYield>::new(
        &name_gen,
    );
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(func.body[0].clone());
    lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None)
        .expect("import-from lowering should succeed");
    let fragment = out.finish();
    assert!(!fragment.entry.body.is_empty());
}

#[test]
fn lowers_bare_raise_to_optional_blockpy_raise() {
    let blockpy = wrapped_blockpy(
        r#"
def f():
    raise
"#,
    );
    let raise_stmt = match &function_by_name(&blockpy, "f").blocks[0].term {
        BlockTerm::Raise(raise_stmt) => raise_stmt,
        other => panic!("expected BlockPy raise term, got {other:?}"),
    };
    assert!(raise_stmt.exc.is_none());
}

#[test]
fn lowers_raise_from_if_it_reaches_blockpy_stmt_lowering() {
    let module = ruff_python_parser::parse_module(
        r#"
def f():
    raise E from cause
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let context = test_context();
    let name_gen = test_name_gen();
    let mut out = crate::passes::ruff_to_blockpy::InlineBlockBuilder::<InstrWithAwaitAndYield>::new(
        &name_gen,
    );
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(func.body[0].clone());
    lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None)
        .expect("raise-from lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(
        fragment.entry.term,
        BlockTerm::Raise(TermRaise { exc: Some(_), .. })
    ));
}

#[test]
#[should_panic(
    expected = "While should be lowered before Ruff AST -> BlockPy stmt-list conversion"
)]
fn panics_if_while_reaches_stmt_list_lowering() {
    let module = ruff_python_parser::parse_module("while x:\n    pass\n")
        .unwrap()
        .into_syntax()
        .body;
    let ast::Stmt::While(while_stmt) = &module[0] else {
        panic!("expected while stmt");
    };
    let context = test_context();
    let name_gen = test_name_gen();
    let mut out = crate::passes::ruff_to_blockpy::InlineBlockBuilder::<InstrWithAwaitAndYield>::new(
        &name_gen,
    );
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(Stmt::While(while_stmt.clone()));
    lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None).unwrap();
}

#[test]
fn raise_disposition_survives_compat_expression_and_core_mapping() {
    for disposition in [
        RaiseDisposition::Source,
        RaiseDisposition::PropagateNormalized,
        RaiseDisposition::SourceNormalized,
    ] {
        for expression in [
            None,
            Some(py_expr!("exc")),
            Some(py_expr!("build_error()")),
            Some(py_expr!("left if condition else right")),
            Some(py_expr!("left and middle or right")),
            Some(py_expr!("a() < b() < c()")),
        ] {
            if expression.is_none() && disposition.is_normalized() {
                continue;
            }
            let mut blocks = Vec::new();
            emit_sequence_raise_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
                &Context::new(""),
                &mut blocks,
                &test_name_gen(),
                Vec::new(),
                TermRaise {
                    exc: expression.map(crate::passes::ast_to_instr::from_ast_expr),
                    disposition,
                },
                None,
            )
            .expect("raise expression should lower with its operation unchanged");
            let raises = blocks
                .iter()
                .filter(|block| matches!(block.term, BlockTerm::Raise(_)))
                .collect::<Vec<_>>();
            assert!(!raises.is_empty());
            for block in raises {
                let mut infallible = |instruction: InstrWithAwaitAndYield| instruction;
                let mapped = infallible.map_term(block.term.clone());
                let mut fallible = |instruction: InstrWithAwaitAndYield| Ok::<_, ()>(instruction);
                let try_mapped = fallible.try_map_term(block.term.clone()).unwrap();
                for term in [&block.term, &mapped, &try_mapped] {
                    let BlockTerm::Raise(raise) = term else {
                        panic!("raise shape changed");
                    };
                    assert_eq!(raise.disposition, disposition);
                    if disposition.is_normalized() {
                        assert!(raise.exc.is_some());
                    }
                }
            }
        }
    }
}

#[test]
fn raise_disposition_rejects_normalized_propagation_without_an_operand() {
    let original = lower_python_to_blockpy_for_testing("def source_bare_raise():\n    raise\n")
        .expect("source bare raise is valid syntax")
        .blockpy_module;
    crate::block_py::validate::validate_blockpy_module(&original)
        .expect("ordinary source bare raise remains valid IR");
    for disposition in [
        RaiseDisposition::PropagateNormalized,
        RaiseDisposition::SourceNormalized,
    ] {
        let mut module = original.clone();
        let function = module
            .callable_defs
            .iter_mut()
            .find(|function| function.names.bind_name == "source_bare_raise")
            .unwrap();
        let raise = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.term {
                BlockTerm::Raise(raise) if raise.exc.is_none() => Some(raise),
                _ => None,
            })
            .expect("actual bare source raise");
        assert_eq!(raise.disposition, RaiseDisposition::Source);
        raise.disposition = disposition;
        assert!(crate::block_py::validate::validate_blockpy_module(&module).is_err());
    }
}

fn canonical_class_function(
    module: &BlockPyModule<soac_ir_blockpy::BlockPyModuleShape>,
) -> &BlockPyFunction<soac_ir_blockpy::BlockPyModuleShape> {
    let mut classes = module
        .callable_defs
        .iter()
        .filter(|function| function.scope.class_bindings.is_some());
    let class = classes.next().expect("canonical namespace function");
    assert!(classes.next().is_none(), "fixture has one original class");
    class
}

#[test]
fn canonical_class_namespace_uses_the_actual_mapping_and_native_slot_owners() {
    use crate::block_py::{ClassBindingStorage, ClosureInit};

    let module = crate::test::strict_source::lower_verified(concat!(
        "from __future__ import strict\n",
        "def fail_class():\n",
        "    class Broken:\n",
        "        value = payload()\n",
        "        raise ValueError('namespace failure')\n",
    ))
    .unwrap();
    let function = canonical_class_function(&module);
    let class = function.scope.class_bindings.as_ref().unwrap();
    let layout = function.storage_layout.as_ref().unwrap();
    let projection = layout.class_bindings.as_ref().unwrap();
    projection.validate(class, layout, &function.scope).unwrap();
    assert_eq!(
        function.params.params.len(),
        2,
        "namespace and execution handle only"
    );
    assert_eq!(function.params.params[0].name, class.namespace_binding);
    assert_eq!(
        layout.stack_slots[projection.namespace.slot() as usize],
        class.namespace_binding
    );
    assert_eq!(projection.slots.len(), class.node.slots.len());
    for slot in &projection.slots {
        let local = slot.storage.raw_local(layout).unwrap();
        assert!(!layout.is_expression_temporary(local));
        if let ClassBindingStorage::Cell(crate::block_py::CellLocation::Owned(index)) = slot.storage
        {
            assert_eq!(layout.cellvars[index as usize].init, ClosureInit::Deferred);
        }
    }
}

#[test]
fn canonical_class_provider_keeps_firstline_separate_from_native_scope_span() {
    use crate::block_py::{AnnotationProviderKind, CallableSourceRole};

    let source = concat!(
        "from __future__ import strict\n",
        "class C:\n",
        "    prefix = 1\n",
        "    annotated: int\n",
    );
    let module = crate::test::strict_source::lower_verified(source).unwrap();
    let provider = module
        .callable_defs
        .iter()
        .find(|function| {
            function
                .scope
                .source_origin
                .as_ref()
                .is_some_and(|origin| origin.role == CallableSourceRole::AnnotationProvider)
                && function
                    .scope
                    .annotation_provider
                    .as_ref()
                    .is_some_and(|provider| provider.kind == AnnotationProviderKind::Dictionary)
        })
        .expect("actual native class annotation provider");
    let metadata = provider.scope.annotation_provider.as_ref().unwrap();
    assert_eq!(metadata.native_first_line, 2);
    let start = source.find(": int").unwrap() + 2;
    assert_eq!(
        metadata.native_range,
        Some(soac_contracts::SourceRange::new(
            start as u32,
            (start + 3) as u32
        ))
    );
}
