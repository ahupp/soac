use super::*;

use crate::block_py::{
    instr_any, BlockEdge, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, FunctionKind,
    InstrWithAwaitAndYield, ModuleShape, NameLike, ScopeExprNode, TermRaise,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::pass_tracker::LoweringPassTrackerInternalExt;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::stmt_sequences::{
    lower_for_stmt_sequence, lower_if_stmt_sequence, lower_if_stmt_sequence_from_stmt,
    lower_while_stmt_sequence, lower_while_stmt_sequence_from_stmt, plan_instr_sequence_head,
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
                BlockTerm::Return(value) => instr_any(value, &mut predicate),
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
fn lowers_async_for_structurally() {
    let blockpy = wrapped_core_blockpy_with_await_and_yield(
        r#"
async def f(xs):
    async for x in xs:
        body(x)
"#,
    );
    let f = function_by_name(&blockpy, "f");
    assert!(function_has_root_load(f, "anext_or_sentinel"));
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
    let gen_wrapper = function_by_name(&blockpy, "gen");
    let gen_resume = function_by_name(&blockpy, "gen_resume");
    assert_eq!(gen_wrapper.kind, FunctionKind::Generator);
    assert!(function_has_root_load(gen_wrapper, "ClosureGenerator"));
    assert!(gen_resume
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
    let gen_wrapper = function_by_name(&blockpy, "gen");
    assert_eq!(gen_wrapper.kind, FunctionKind::Generator);
    let gen_resume = function_by_name(&blockpy, "gen_resume");
    let entry_label = gen_resume.entry_block().label;
    assert!(!gen_resume.blocks.iter().any(|block| matches!(
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
fn lower_for_stmt_sequence_emits_loop_scaffolding() {
    let module = ruff_python_parser::parse_module(
        r#"
def f(xs):
    for x in xs:
        body(x)
"#,
    )
    .unwrap()
    .into_syntax()
    .body;
    let ast::Stmt::FunctionDef(func) = &module[0] else {
        panic!("expected function def");
    };
    let ast::Stmt::For(for_stmt) = &func.body[0] else {
        panic!("expected for stmt");
    };
    let InstrRuff::StmtFor(for_stmt) =
        crate::passes::ast_to_instr::from_ast_stmt(ast::Stmt::For(for_stmt.clone()))
    else {
        panic!("expected InstrRuff::StmtFor");
    };

    let mut blocks = Vec::new();
    let name_gen = test_name_gen();
    let entry = lower_for_stmt_sequence(
        &Context::new(""),
        &name_gen,
        for_stmt,
        &[],
        RegionTargets::new(label(99), None),
        Vec::new(),
        &mut blocks,
        "_dp_iter_0",
        "_dp_tmp_0",
        label(0),
        label(0),
        label(1),
        label(2),
        vec![
            crate::passes::ast_to_instr::from_ast_stmt(py_stmt!("x = _dp_tmp_0")),
            crate::passes::ast_to_instr::from_ast_stmt(py_stmt!("del _dp_tmp_0")),
        ],
        &mut |_stmts: &[InstrRuff], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            targets.normal_cont
        },
    );

    assert_eq!(entry, label(2));
    assert!(blocks.iter().any(|block| block.label == label(1)));
    assert!(blocks.iter().any(|block| block.label == label(2)));
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
        .expect("entry should be a linear prefix jump into the lowered setup");
    assert_eq!(blocks.len(), 5);
    let if_labels = blocks
        .iter()
        .filter_map(|block| match block.term {
            BlockTerm::IfTerm(_) => Some(block.label),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(if_labels.len(), 2, "{blocks:#?}");
    let dispatch_label = *if_labels
        .iter()
        .find(|&&label| label != setup_label)
        .expect("dispatch if block should exist");
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
            .filter(|&&target| target == dispatch_label)
            .count(),
        2,
        "{blocks:#?}"
    );
    assert!(
        jump_targets.iter().any(|&target| target == setup_label),
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
    assert_eq!(blocks.len(), 4);
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
        .expect("entry should jump to the first short-circuit test block");
    assert!(
        blocks
            .iter()
            .find(|block| block.label == setup_label)
            .is_some_and(|block| matches!(block.term, BlockTerm::IfTerm(_))),
        "{blocks:#?}"
    );
    let second_if_label = blocks
        .iter()
        .find(|block| block.label != setup_label && matches!(block.term, BlockTerm::IfTerm(_)))
        .map(|block| block.label)
        .expect("second if block should exist");
    assert!(
        blocks.iter().any(|block| matches!(
            block.term,
            BlockTerm::Jump(BlockEdge { target, ref args })
                if target == second_if_label && args.is_empty()
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
    assert_eq!(blocks.len(), 4);
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
    let second_if_label = blocks
        .iter()
        .find(|block| block.label != setup_label && matches!(block.term, BlockTerm::IfTerm(_)))
        .map(|block| block.label)
        .expect("second if block should exist");
    assert!(
        blocks.iter().any(|block| matches!(
            block.term,
            BlockTerm::Jump(BlockEdge { target, ref args })
                if target == second_if_label && args.is_empty()
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
        BlockTerm::Raise(TermRaise { exc: Some(_) })
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
        .any(|block| matches!(block.term, BlockTerm::Raise(TermRaise { exc: Some(_) }))));
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
            BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
        }
        if let Some(edge) = &block.exc_edge {
            check(edge.target, "exception");
        }
    }
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
        .any(|block| matches!(block.term, BlockTerm::Raise(TermRaise { exc: Some(_) }))));
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
    let gen_resume = function_by_name(&blockpy, "gen_resume");
    assert!(gen_resume
        .blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::BranchTable(_))));
    assert!(function_has_root_load(gen_resume, "exception_matches"));
    assert!(function_has_root_load(gen_resume, "getattr"));
    assert!(!function_has_root_load(
        gen_resume,
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
    let agen_wrapper = function_by_name(&blockpy, "agen");
    let agen_resume = function_by_name(&blockpy, "agen_resume");
    assert_eq!(agen_wrapper.kind, FunctionKind::AsyncGenerator);
    assert!(function_has_root_load(
        agen_wrapper,
        "ClosureAsyncGenerator"
    ));
    assert!(agen_resume
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
    let rendered = crate::block_py::blockpy_module_to_string(&blockpy);
    let resume = function_by_name(&blockpy, "outer_resume");
    let stop_iteration_raise_labels = resume
        .blocks
        .iter()
        .filter_map(|block| match &block.term {
            BlockTerm::Raise(TermRaise { exc: Some(exc) })
                if crate::block_py::bb_expr_text(exc).contains("StopIteration") =>
            {
                Some(block.label.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !stop_iteration_raise_labels.is_empty(),
        "missing synthetic StopIteration blocks in:\n{rendered}"
    );
    for label in stop_iteration_raise_labels {
        assert_eq!(
            lowered_exception_edges(&resume.blocks)
                .get(&label)
                .cloned()
                .flatten(),
            None,
            "synthetic completion should bypass user handlers for {label}:\n{rendered}"
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
    assert!(matches!(
        fragment.entry.body.as_slice(),
        [_, InstrWithAwaitAndYield::Store(_)]
    ));
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
        BlockTerm::Raise(TermRaise { exc: Some(_) })
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
