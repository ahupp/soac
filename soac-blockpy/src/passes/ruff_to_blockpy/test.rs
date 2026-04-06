use super::*;

use crate::block_py::{
    BlockEdge, BlockLabel, BlockPyFunction, BlockPyModule, BlockPyPass, BlockTerm,
    InstrWithAwaitAndYield, StructuredInstr, TermRaise,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::stmt_sequences::{
    lower_for_stmt_sequence, lower_if_stmt_sequence, lower_if_stmt_sequence_from_stmt,
    lower_while_stmt_sequence, lower_while_stmt_sequence_from_stmt, plan_stmt_sequence_head,
};
use crate::passes::ruff_to_blockpy::try_regions::build_try_plan;
use crate::passes::{CoreBlockPyPass, CoreBlockPyPassWithAwaitAndYield};
use stmt_lowering::lower_stmt_into;

fn test_name_gen() -> FunctionNameGen {
    let module_name_gen = crate::block_py::ModuleNameGen::new(0);
    module_name_gen.next_function_name_gen()
}

fn wrapped_blockpy(source: &str) -> BlockPyModule<CoreBlockPyPassWithAwaitAndYield> {
    lower_python_to_blockpy_for_testing(source)
        .unwrap()
        .pass_tracker
        .pass_core_blockpy_with_await_and_yield()
        .expect("core_blockpy_with_await_and_yield pass should be tracked")
        .clone()
}

fn wrapped_core_blockpy_with_await_and_yield(
    source: &str,
) -> BlockPyModule<CoreBlockPyPassWithAwaitAndYield> {
    wrapped_blockpy(source)
}

fn wrapped_core_blockpy(source: &str) -> BlockPyModule<CoreBlockPyPass> {
    lower_python_to_blockpy_for_testing(source)
        .unwrap()
        .pass_tracker
        .pass_core_blockpy()
        .expect("core_blockpy pass should be tracked")
        .clone()
}

fn function_by_name<'a, P: BlockPyPass>(
    blockpy: &'a BlockPyModule<P>,
    bind_name: &str,
) -> &'a BlockPyFunction<P> {
    blockpy
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == bind_name)
        .unwrap_or_else(|| panic!("missing BlockPy function {bind_name}; got {blockpy:?}"))
}

fn lower_stmt_for_panic_test(stmt: &Stmt) {
    let context = Context::new("");
    let mut out = crate::block_py::BlockBuilder::<
        StructuredInstr<InstrWithAwaitAndYield>,
        BlockTerm<InstrWithAwaitAndYield>,
    >::new();
    let mut next_label_id = 0usize;
    let _ = lower_stmt_into(&context, stmt, &mut out, None, &mut next_label_id);
}

fn test_context() -> Context {
    Context::new("")
}

fn label(index: u32) -> BlockLabel {
    BlockLabel::from_index(index as usize)
}

type TestBlock =
    Block<StructuredInstr<InstrWithAwaitAndYield>, InstrWithAwaitAndYield>;

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
    let rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);
    assert!(blocks
        .iter()
        .any(|block| matches!(block.term, BlockTerm::IfTerm(_))));
    assert!(
        blocks.iter().any(|block| block.exc_edge.is_some()),
        "{rendered}"
    );
    assert!(rendered.contains("return x"), "{rendered}");
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
    let rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);
    assert!(rendered.contains("await anext_or_sentinel"), "{rendered}");
    assert!(rendered.contains("anext_or_sentinel"), "{rendered}");
}

#[test]
fn lowers_generator_yield_to_explicit_blockpy_dispatch() {
    let blockpy = wrapped_core_blockpy(
        r#"
def gen(n):
    yield n
"#,
    );
    let rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);
    assert!(rendered.contains("generator gen(n):"), "{rendered}");
    assert!(
        rendered.contains("function gen(_dp_self, _dp_send_value, _dp_resume_exc):"),
        "{rendered}"
    );
    assert!(rendered.contains("return ClosureGenerator("), "{rendered}");
    assert!(rendered.contains("branch_table"), "{rendered}");
    assert!(!rendered.contains("yield n"), "{rendered}");
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
        plan_stmt_sequence_head(&test_context(), stmt),
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
        plan_stmt_sequence_head(&test_context(), stmt),
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
        plan_stmt_sequence_head(&test_context(), stmt),
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
        plan_stmt_sequence_head(&test_context(), stmt),
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
        plan_stmt_sequence_head(&test_context(), stmt),
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

    let StmtSequenceHeadPlan::Expanded(body) = plan_stmt_sequence_head(&test_context(), stmt)
    else {
        panic!("expected expanded match body");
    };
    assert!(matches!(body[0], Stmt::Assign(_)));
    assert!(body.iter().any(|stmt| matches!(stmt, Stmt::If(_))));
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

    let StmtSequenceHeadPlan::Expanded(body) = plan_stmt_sequence_head(&test_context(), stmt)
    else {
        panic!("expected expanded match body");
    };
    let match_if = body
        .iter()
        .find(|stmt| matches!(stmt, Stmt::If(_)))
        .expect("expected expanded match body to contain an if");

    assert!(
        matches!(
            plan_stmt_sequence_head(&test_context(), match_if),
            StmtSequenceHeadPlan::If(_)
        ),
        "{}",
        crate::ruff_ast_to_string(match_if).trim_end()
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
    let rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);
    assert!(
        !rendered.contains("and __dp_match_class_attr_exists"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("and __dp_match_class_attr_value"),
        "{rendered}"
    );
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
    let rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);
    assert!(rendered.contains("StoreName(\"_dp_match_1\", \"aa\")"), "{rendered}");
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

    let mut blocks = Vec::new();
    let entry = lower_for_stmt_sequence(
        for_stmt.clone(),
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
        vec![py_stmt!("x = _dp_tmp_0"), py_stmt!("_dp_tmp_0 = None")],
        &mut |_stmts: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
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
        with_stmt.clone(),
        &[],
        RegionTargets::new(label(99), None),
        Vec::new(),
        &mut blocks,
        &name_gen,
        false,
        &mut |_expanded: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            saw_try_stmt = _expanded
                .iter()
                .any(|stmt| matches!(stmt, ast::Stmt::Try(_)));
            saw_with_ok_assign = _expanded.iter().any(|stmt| {
                match stmt {
                    ast::Stmt::Assign(assign) => assign.targets.iter().any(|target| {
                        matches!(target, Expr::Name(name) if name.id.as_str().contains("with_ok"))
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
    let entry = lower_try_stmt_sequence(
        try_stmt.clone(),
        &[],
        RegionTargets::new(label(99), None),
        Vec::new(),
        &mut blocks,
        &name_gen,
        label(0),
        try_plan,
        &mut |_expanded: &[Stmt], targets: RegionTargets, blocks: &mut Vec<TestBlock>| {
            let label = BlockLabel::from_index(100 + blocks.len());
            blocks.push(
                crate::passes::ruff_to_blockpy::compat::compat_block_from_blockpy_with_exc_target_and_expr::<
                    InstrWithAwaitAndYield,
                >(
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
    let entry = lower_expanded_stmt_sequence(
        &context,
        vec![py_stmt!("pass")],
        &[],
        RegionTargets::new(label(99), None),
        Vec::new(),
        &mut blocks,
        None,
        &mut |expanded: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
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
    let entry = lower_expanded_stmt_sequence(
        &context,
        vec![py_stmt!("pass")],
        &[],
        RegionTargets::new(label(99), None),
        vec![py_stmt!("x = 1")],
        &mut blocks,
        Some(label(10)),
        &mut |_expanded: &[Stmt], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| label(11),
    );

    assert_eq!(entry, label(10));
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].label, label(10));
    assert!(matches!(
        &blocks[0].term,
        BlockTerm::Jump(edge) if edge.target == label(11)
    ));
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
        label(10),
        vec![py_stmt!("prefix = 0")],
        py_expr!("flag"),
        &then_body,
        &else_body,
        label(99),
        &RegionTargets::new(label(99), None),
        &mut |stmts: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            calls.push((stmts.len(), targets.normal_cont.clone()));
            label(200 + calls.len() as u32)
        },
    );

    assert_eq!(entry, label(10));
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
        label(10),
        vec![py_stmt!("prefix = 0")],
        py_expr!("left if cond else right"),
        &[py_stmt!("x = 1")],
        &[py_stmt!("x = 2")],
        label(99),
        &RegionTargets::new(label(99), None),
        &mut |_stmts: &[Stmt], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| label(200),
    );

    assert_eq!(entry, label(10));
    assert_eq!(blocks.len(), 5);
    let setup_label = blocks[0].label;
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
        jump_targets
            .iter()
            .any(|&target| target == setup_label),
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
        label(10),
        vec![py_stmt!("prefix = 0")],
        py_expr!("left and right"),
        &[py_stmt!("x = 1")],
        &[py_stmt!("x = 2")],
        label(99),
        &RegionTargets::new(label(99), None),
        &mut |_stmts: &[Stmt], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| label(200),
    );

    assert_eq!(entry, label(10));
    assert_eq!(blocks.len(), 4);
    assert!(matches!(blocks[0].term, BlockTerm::IfTerm(_)));
    assert!(matches!(
        blocks[1].term,
        BlockTerm::Jump(BlockEdge { target, ref args })
            if target == blocks[2].label && args.is_empty()
    ));
    assert!(matches!(blocks[2].term, BlockTerm::IfTerm(_)));
    assert!(matches!(
        blocks[3].term,
        BlockTerm::Jump(BlockEdge { target, ref args })
            if target == blocks[0].label && args.is_empty()
    ));
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
        label(10),
        vec![py_stmt!("prefix = 0")],
        py_expr!("a < b < c"),
        &[py_stmt!("x = 1")],
        &[py_stmt!("x = 2")],
        label(99),
        &RegionTargets::new(label(99), None),
        &mut |_stmts: &[Stmt], _targets: RegionTargets, _blocks: &mut Vec<TestBlock>| label(200),
    );

    assert_eq!(entry, label(10));
    assert_eq!(blocks.len(), 4);
    assert!(matches!(blocks[0].term, BlockTerm::IfTerm(_)));
    assert!(matches!(
        blocks[1].term,
        BlockTerm::Jump(BlockEdge { target, ref args })
            if target == blocks[2].label && args.is_empty()
    ));
    assert!(matches!(blocks[2].term, BlockTerm::IfTerm(_)));
    assert!(matches!(
        blocks[3].term,
        BlockTerm::Jump(BlockEdge { target, ref args })
            if target == blocks[0].label && args.is_empty()
    ));
}

#[test]
fn sequence_jump_helper_emits_jump_block() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let entry = emit_sequence_jump_block::<InstrWithAwaitAndYield>(
        &context,
        &mut blocks,
        label(10),
        vec![py_stmt!("prefix = 0")],
        label(11),
        None,
    );

    assert_eq!(entry, label(10));
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
    let entry =
        emit_sequence_return_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
            &context,
            &mut blocks,
            &test_name_gen(),
            label(10),
            vec![py_stmt!("prefix = 0")],
            Some(py_expr!("value")),
            None,
        )
        .expect("sequence return helper should lower");

    assert_eq!(entry, label(10));
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0].term, BlockTerm::Return(_)));
}

#[test]
fn sequence_raise_helper_emits_raise_block() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let entry =
        emit_sequence_raise_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
            &context,
            &mut blocks,
            &test_name_gen(),
            label(10),
            vec![py_stmt!("prefix = 0")],
            TermRaise {
                exc: Some(py_expr!("exc").into()),
            },
            None,
        )
        .expect("sequence raise helper should lower");

    assert_eq!(entry, label(10));
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
    let entry =
        emit_sequence_return_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
            &context,
            &mut blocks,
            &name_gen,
            label(10),
            vec![py_stmt!("prefix = 0")],
            Some(py_expr!("lhs() and rhs()")),
            None,
        )
        .expect("sequence return helper should lower");

    assert_eq!(entry, label(10));
    assert!(blocks.len() > 1, "{blocks:#?}");
    assert_eq!(blocks[0].label, label(10));
    assert!(blocks.iter().any(|block| matches!(block.term, BlockTerm::Return(_))));
}

#[test]
fn sequence_raise_helper_lowers_compare_chain_via_inline_fragment() {
    let mut blocks = Vec::new();
    let context = Context::new("");
    let name_gen = test_name_gen();
    let entry =
        emit_sequence_raise_block_with_expr_setup_and_expr::<InstrWithAwaitAndYield>(
            &context,
            &mut blocks,
            &name_gen,
            label(10),
            vec![py_stmt!("prefix = 0")],
            TermRaise {
                exc: Some(py_expr!("a() < b() < c()").into()),
            },
            None,
        )
        .expect("sequence raise helper should lower");

    assert_eq!(entry, label(10));
    assert!(blocks.len() > 1, "{blocks:#?}");
    assert_eq!(blocks[0].label, label(10));
    assert!(blocks.iter().any(|block| matches!(
        block.term,
        BlockTerm::Raise(TermRaise { exc: Some(_) })
    )));
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

    assert_eq!(entry, blocks[0].label);
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
        if_stmt.clone(),
        &remaining,
        RegionTargets::new(label(99), None),
        vec![py_stmt!("prefix = 0")],
        &mut blocks,
        label(10),
        &mut |stmts: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            calls.push((stmts.len(), targets.normal_cont.clone()));
            label(200 + calls.len() as u32)
        },
    );

    assert_eq!(entry, label(10));
    assert_eq!(
        calls
            .into_iter()
            .map(|(len, label)| (len, label))
            .collect::<Vec<_>>(),
        vec![(remaining.len(), label(99))]
    );
    assert_eq!(blocks.len(), 4);
    let fragment_label = blocks[0].label;
    assert!(matches!(blocks[0].term, BlockTerm::IfTerm(_)));
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
        jump_targets
            .iter()
            .any(|&target| target == fragment_label),
        "{blocks:#?}"
    );
}

#[test]
fn if_stmt_from_stmt_helper_falls_back_when_branch_setup_is_not_plain() {
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
        if_stmt.clone(),
        &remaining,
        RegionTargets::new(label(99), None),
        vec![py_stmt!("prefix = 0")],
        &mut blocks,
        label(10),
        &mut |stmts: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
            calls.push((stmts.len(), targets.normal_cont.clone()));
            label(200 + calls.len() as u32)
        },
    );

    assert_eq!(entry, label(10));
    assert_eq!(
        calls
            .into_iter()
            .map(|(len, label)| (len, label))
            .collect::<Vec<_>>(),
        vec![
            (remaining.len(), label(99)),
            (1, label(201)),
            (1, label(201))
        ]
    );
    assert_eq!(blocks.len(), 1);
    assert!(matches!(blocks[0].term, BlockTerm::IfTerm(_)));
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
        vec![py_stmt!("prefix = 0")],
        py_expr!("flag"),
        &body,
        &else_body,
        &remaining,
        RegionTargets::new(label(99), None),
        &mut |stmts: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
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
        vec![py_stmt!("prefix = 0")],
        py_expr!("lhs() and rhs()"),
        &body,
        &[],
        &remaining,
        RegionTargets::new(label(99), None),
        &mut |stmts: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
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
    assert_eq!(blocks[0].label, test_label);
    assert!(blocks.iter().any(|block| block.label != test_label && block.label != linear_label));
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
        vec![py_stmt!("prefix = 0")],
        py_expr!("a() < b() < c()"),
        &body,
        &[],
        &remaining,
        RegionTargets::new(label(99), None),
        &mut |stmts: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
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
    assert_eq!(blocks[0].label, test_label);
    assert!(blocks.iter().any(|block| block.label != test_label && block.label != linear_label));
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
        while_stmt.clone(),
        &remaining,
        RegionTargets::new(label(99), None),
        vec![py_stmt!("prefix = 0")],
        &mut blocks,
        label(0),
        Some(label(1)),
        &mut |stmts: &[Stmt], targets: RegionTargets, _blocks: &mut Vec<TestBlock>| {
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
    let rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);
    assert!(rendered.contains("branch_table"));
    assert!(rendered.contains("exception_matches"), "{rendered}");
    assert!(
        rendered.contains("getattr(_dp_yieldfrom, \"throw\", NONE)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("exc_param: _dp_yield_from_exc_"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("__dp_generator_yield_from_step"),
        "{rendered}"
    );
    assert!(!rendered.contains("yield from it"), "{rendered}");
}

#[test]
fn lowers_async_generator_yield_to_explicit_blockpy_dispatch() {
    let blockpy = wrapped_core_blockpy(
        r#"
async def agen(n):
    yield n
"#,
    );
    let rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);
    assert!(rendered.contains("async_generator agen(n):"), "{rendered}");
    assert!(
        rendered.contains(
            "function agen(_dp_self, _dp_send_value, _dp_resume_exc, _dp_transport_sent):"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("return ClosureAsyncGenerator("),
        "{rendered}"
    );
    assert!(rendered.contains("branch_table"), "{rendered}");
    assert!(!rendered.contains("yield n"), "{rendered}");
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
    let rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);
    let resume = function_by_name(&blockpy, "outer_resume");
    let stop_iteration_raise_labels = resume
        .blocks
        .iter()
        .filter_map(|block| match &block.term {
            BlockTerm::Raise(TermRaise { exc: Some(exc) })
                if crate::block_py::pretty::bb_expr_text(exc).contains("StopIteration") =>
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
    let mut out = crate::block_py::BlockBuilder::<
        StructuredInstr<InstrWithAwaitAndYield>,
        BlockTerm<InstrWithAwaitAndYield>,
    >::new();
    let mut next_label_id = 0usize;
    lower_stmt_into(&context, &func.body[0], &mut out, None, &mut next_label_id)
        .expect("assert lowering should succeed");
    let fragment = out.finish();
    assert!(matches!(fragment.body.as_slice(), [StructuredInstr::If(_)]));
}

#[test]
#[should_panic(expected = "ClassDef should be lowered before Ruff AST -> BlockPy conversion")]
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
    let mut out = crate::block_py::BlockBuilder::<
        StructuredInstr<InstrWithAwaitAndYield>,
        BlockTerm<InstrWithAwaitAndYield>,
    >::new();
    let mut next_label_id = 0usize;
    lower_stmt_into(&context, &func.body[0], &mut out, None, &mut next_label_id)
        .expect("augassign lowering should succeed");
    let fragment = out.finish();
    assert!(matches!(
        fragment.body.as_slice(),
        [
            StructuredInstr::Expr(_),
            StructuredInstr::Expr(InstrWithAwaitAndYield::Store(_))
        ]
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
    let mut out = crate::block_py::BlockBuilder::<
        StructuredInstr<InstrWithAwaitAndYield>,
        BlockTerm<InstrWithAwaitAndYield>,
    >::new();
    let mut next_label_id = 0usize;
    lower_stmt_into(&context, &module[0], &mut out, None, &mut next_label_id)
        .expect("type alias lowering should succeed");
    let fragment = out.finish();
    assert!(!fragment.body.is_empty());
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
    let mut out = crate::block_py::BlockBuilder::<
        StructuredInstr<InstrWithAwaitAndYield>,
        BlockTerm<InstrWithAwaitAndYield>,
    >::new();
    let mut next_label_id = 0usize;
    lower_stmt_into(&context, &func.body[0], &mut out, None, &mut next_label_id)
        .expect("match lowering should succeed");
    let fragment = out.finish();
    assert!(!fragment.body.is_empty() || fragment.term.is_some());
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
    let mut out = crate::block_py::BlockBuilder::<
        StructuredInstr<InstrWithAwaitAndYield>,
        BlockTerm<InstrWithAwaitAndYield>,
    >::new();
    let mut next_label_id = 0usize;
    lower_stmt_into(&context, &func.body[0], &mut out, None, &mut next_label_id)
        .expect("import lowering should succeed");
    let fragment = out.finish();
    assert!(matches!(
        fragment.body.as_slice(),
        [StructuredInstr::Expr(
            InstrWithAwaitAndYield::Store(_)
        )]
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
    let mut out = crate::block_py::BlockBuilder::<
        StructuredInstr<InstrWithAwaitAndYield>,
        BlockTerm<InstrWithAwaitAndYield>,
    >::new();
    let mut next_label_id = 0usize;
    lower_stmt_into(&context, &func.body[0], &mut out, None, &mut next_label_id)
        .expect("import-from lowering should succeed");
    let fragment = out.finish();
    assert!(!fragment.body.is_empty());
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
#[should_panic(expected = "raise-from should be lowered before Ruff AST -> BlockPy conversion")]
fn panics_if_raise_from_reaches_blockpy() {
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
    lower_stmt_for_panic_test(&func.body[0]);
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
    let mut out = crate::block_py::BlockBuilder::<
        StructuredInstr<InstrWithAwaitAndYield>,
        BlockTerm<InstrWithAwaitAndYield>,
    >::new();
    let mut next_label_id = 0usize;
    lower_stmt_into(
        &context,
        &Stmt::While(while_stmt.clone()),
        &mut out,
        None,
        &mut next_label_id,
    )
    .unwrap();
}
