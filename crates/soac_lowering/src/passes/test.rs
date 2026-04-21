use crate::block_py::PrettyPrint;
use crate::block_py::{
    instr_any, BlockPyFunction, BlockPyModule, BlockTerm, Call, CallArgKeyword, CallArgPositional,
    CallableScopeKind, CellBindingKind, CellLocation, ChildVisitable, FunctionExecutionMode,
    FunctionKind, InstrResolved, NameLike, NameLocation, ResolvedStorageBlock, ScopeExprNode,
};
use crate::block_py::{BindingKind, ClosureInit, ClosureSlot, ModuleNameGen};
use crate::passes::ast_to_ast::ast_rewrite::rewrite_with_pass;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::rewrite_class_def;
use crate::passes::ast_to_ast::rewrite_expr::ScopedHelperExprPass;
use crate::passes::ast_to_ast::{
    body::Suite, rewrite_future_annotations, rewrite_stmt, semantic::SemanticAstState,
};
use crate::passes::core_await_lower::lower_awaits_in_core_blockpy_module;
use crate::passes::ruff_to_blockpy::rewrite_ast_to_core_blockpy_module_with_module;
use crate::passes::{
    CoreModuleShapeWithAwaitAndYield, CoreModuleShapeWithYield, InstrRuff, InstrWithAwaitAndYield,
    ResolvedStorageModuleShape,
};
use crate::transformer::{walk_stmt, Transformer};
use crate::{lower_python_to_blockpy_for_testing, LoweringResult};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_python_parser::parse_module;
use std::collections::HashSet;

fn tracked_core_blockpy_with_await_and_yield(
    source: &str,
) -> BlockPyModule<CoreModuleShapeWithAwaitAndYield> {
    lower_python_to_blockpy_for_testing(source)
        .expect("transform should succeed")
        .pass_tracker
        .pass_core_blockpy_with_await_and_yield()
        .expect("core_blockpy_with_await_and_yield pass should be tracked")
        .clone()
}

fn rewrite_ast_to_ast_for_testing(source: &str) -> (Context, Suite, SemanticAstState) {
    let module = parse_module(source)
        .expect("source should parse")
        .into_syntax();
    let mut body = module.body;
    rewrite_future_annotations::rewrite(&mut body).expect("future annotation rewrite");
    let context = Context::new(source);
    rewrite_class_def::private::rewrite_private_names(&context, &mut body);
    rewrite_stmt::annotation::rewrite_ann_assign_to_dunder_annotate(&context, &mut body);
    rewrite_with_pass(&context, None, Some(&ScopedHelperExprPass), &mut body);
    let mut semantic_state = SemanticAstState::from_ruff(&mut body);
    crate::driver::wrap_module_init(&mut semantic_state, &mut body);
    rewrite_class_def::class_body::rewrite_class_body_scopes(
        &context,
        &mut semantic_state,
        &mut body,
    );
    (context, body, semantic_state)
}

#[derive(Default)]
struct AstGlobalProbe {
    function_names: HashSet<String>,
    global_names: HashSet<String>,
}

impl Transformer for AstGlobalProbe {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func) => {
                self.function_names.insert(func.name.to_string());
            }
            Stmt::Global(global) => {
                self.global_names
                    .extend(global.names.iter().map(ToString::to_string));
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }
}

fn probe_rewritten_ast(source: &str) -> AstGlobalProbe {
    let (_context, mut body, _semantic_state) = rewrite_ast_to_ast_for_testing(source);
    let mut probe = AstGlobalProbe::default();
    for stmt in &mut body {
        probe.visit_stmt(stmt);
    }
    probe
}

fn tracked_core_blockpy_with_yield_only(source: &str) -> BlockPyModule<CoreModuleShapeWithYield> {
    let (context, module, semantic_state) = rewrite_ast_to_ast_for_testing(source);
    let core_blockpy = rewrite_ast_to_core_blockpy_module_with_module(
        &context,
        module,
        &semantic_state,
        ModuleNameGen::new(0),
    );
    lower_awaits_in_core_blockpy_module(core_blockpy)
}

fn assert_all_targets_present<P, S>(module: &BlockPyModule<P>)
where
    P: crate::block_py::ModuleShape<Instr = S> + crate::block_py::BlockPyFormat,
    S: crate::block_py::Instr + crate::block_py::PrettyPrint,
    P::Instr: crate::block_py::PrettyPrint,
{
    let rendered = module.pretty_print();
    for callable in &module.callable_defs {
        let labels = callable
            .blocks
            .iter()
            .map(|block| block.label)
            .collect::<std::collections::HashSet<_>>();
        for block in &callable.blocks {
            let check = |target: crate::block_py::BlockLabel, kind| {
                assert!(
                    target.is_fallthrough() || labels.contains(&target),
                    "dangling {kind} in {} from {} to {}\n{rendered}",
                    callable.names.qualname,
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
}

fn tracked_name_binding_module(
    source: &str,
) -> anyhow::Result<Option<BlockPyModule<ResolvedStorageModuleShape>>> {
    Ok(lower_python_to_blockpy_for_testing(source)?
        .pass_tracker
        .pass_name_binding()
        .cloned())
}

fn unsound_runtime_builtin_name_binding_module(
    source: &str,
) -> BlockPyModule<ResolvedStorageModuleShape> {
    let result = lower_python_to_blockpy_for_testing(source).expect("transform should succeed");
    let core_blockpy = result
        .pass_tracker
        .pass_core_blockpy()
        .expect("core_blockpy pass should be available")
        .clone();
    super::name_binding::lower_name_binding_in_core_blockpy_module_with_unsound_runtime_builtins(
        core_blockpy,
        true,
        false,
    )
}

struct TrackedLowering {
    result: LoweringResult,
    blockpy_module: BlockPyModule<CoreModuleShapeWithAwaitAndYield>,
}

impl TrackedLowering {
    fn new(source: &str) -> Self {
        let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
        Self {
            result: lower_python_to_blockpy_for_testing(source).expect("transform should succeed"),
            blockpy_module,
        }
    }

    fn blockpy_module(&self) -> BlockPyModule<CoreModuleShapeWithAwaitAndYield> {
        self.blockpy_module.clone()
    }

    fn name_binding_text(&self) -> String {
        self.pass_text("name_binding")
    }

    fn pass_text(&self, name: &str) -> String {
        self.result
            .pass_tracker
            .render_pass_text(name)
            .unwrap_or_else(|| panic!("expected renderable pass {name}"))
    }

    fn bb_module(&self) -> &BlockPyModule<ResolvedStorageModuleShape> {
        self.result
            .pass_tracker
            .pass_name_binding()
            .expect("bb module should be available")
    }

    fn bb_function(&self, bind_name: &str) -> &BlockPyFunction<ResolvedStorageModuleShape> {
        function_by_name(self.bb_module(), bind_name)
    }
}

fn function_by_name<'a>(
    bb_module: &'a BlockPyModule<ResolvedStorageModuleShape>,
    bind_name: &str,
) -> &'a BlockPyFunction<ResolvedStorageModuleShape> {
    let resume_name = format!("{bind_name}_resume");
    if let Some(resume) = bb_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == resume_name)
    {
        return resume;
    }
    bb_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == bind_name)
        .unwrap_or_else(|| panic!("missing lowered function {bind_name}; got {:?}", bb_module))
}

fn slot_by_name<'a>(slots: &'a [ClosureSlot], logical_name: &str) -> &'a ClosureSlot {
    slots
        .iter()
        .find(|slot| slot.logical_name == logical_name)
        .unwrap_or_else(|| panic!("missing closure slot {logical_name}; got {slots:?}"))
}

fn expr_text(expr: &impl crate::block_py::PrettyPrint) -> String {
    crate::block_py::bb_expr_text(expr)
}

fn callable_def_by_name<'a>(
    blockpy_module: &'a BlockPyModule<CoreModuleShapeWithAwaitAndYield>,
    bind_name: &str,
) -> &'a BlockPyFunction<CoreModuleShapeWithAwaitAndYield> {
    blockpy_module
        .callable_defs
        .iter()
        .find(|callable| callable.names.bind_name == bind_name)
        .unwrap_or_else(|| {
            panic!("missing callable definition {bind_name}; got {blockpy_module:?}")
        })
}

fn blockpy_function_by_name<'a, P: crate::block_py::ModuleShape>(
    blockpy_module: &'a BlockPyModule<P>,
    bind_name: &str,
) -> &'a BlockPyFunction<P> {
    blockpy_module
        .callable_defs
        .iter()
        .find(|callable| callable.names.bind_name == bind_name)
        .unwrap_or_else(|| {
            panic!("missing callable definition {bind_name}; got {blockpy_module:?}")
        })
}

fn blockpy_function_by_qualname<'a, P: crate::block_py::ModuleShape>(
    blockpy_module: &'a BlockPyModule<P>,
    qualname: &str,
) -> &'a BlockPyFunction<P> {
    blockpy_module
        .callable_defs
        .iter()
        .find(|callable| callable.names.qualname == qualname)
        .unwrap_or_else(|| panic!("missing callable qualname {qualname}; got {blockpy_module:?}"))
}

fn blockpy_function_instr_any<P>(
    function: &BlockPyFunction<P>,
    mut predicate: impl FnMut(&P::Instr) -> bool,
) -> bool
where
    P: crate::block_py::ModuleShape,
    P::Instr: ChildVisitable<P::Instr>,
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

fn blockpy_function_has_root_name<P>(function: &BlockPyFunction<P>, expected: &str) -> bool
where
    P: crate::block_py::ModuleShape,
    P::Instr: ScopeExprNode,
{
    blockpy_function_instr_any(function, |expr| expr.root_name_id() == Some(expected))
}

fn blockpy_module_has_root_name<P>(module: &BlockPyModule<P>, expected: &str) -> bool
where
    P: crate::block_py::ModuleShape,
    P::Instr: ScopeExprNode,
{
    module
        .callable_defs
        .iter()
        .any(|function| blockpy_function_has_root_name(function, expected))
}

fn blockpy_function_has_string_literal<P>(function: &BlockPyFunction<P>, expected: &str) -> bool
where
    P: crate::block_py::ModuleShape,
    P::Instr: ScopeExprNode,
{
    blockpy_function_instr_any(function, |expr| {
        expr.root_string_literal_value().as_deref() == Some(expected)
    })
}

fn blockpy_function_has_defined_name<P>(function: &BlockPyFunction<P>, expected: &str) -> bool
where
    P: crate::block_py::ModuleShape,
    P::Instr: ScopeExprNode,
{
    blockpy_function_instr_any(function, |expr| {
        let mut found = false;
        expr.walk_root_defined_names(&mut |name| found |= name == expected);
        found
    })
}

fn blockpy_function_has_store_name<P>(function: &BlockPyFunction<P>, expected: &str) -> bool
where
    P: crate::block_py::ModuleShape,
    P::Instr: ScopeExprNode,
{
    blockpy_function_instr_any(function, |expr| {
        let mut found = false;
        expr.walk_root_defined_names(&mut |name| found |= name == expected);
        found
    })
}

fn blockpy_module_has_store_name<P>(module: &BlockPyModule<P>, expected: &str) -> bool
where
    P: crate::block_py::ModuleShape,
    P::Instr: ScopeExprNode,
{
    module
        .callable_defs
        .iter()
        .any(|function| blockpy_function_has_store_name(function, expected))
}

fn blockpy_function_has_del_name<P>(function: &BlockPyFunction<P>, expected: &str) -> bool
where
    P: crate::block_py::ModuleShape,
    P::Instr: ScopeExprNode,
{
    blockpy_function_instr_any(function, |expr| {
        let mut found = false;
        expr.walk_root_deleted_names(&mut |name| found |= name == expected);
        found
    })
}

fn blockpy_function_has_cell_ref_for_name(
    function: &BlockPyFunction<CoreModuleShapeWithAwaitAndYield>,
    expected: &str,
) -> bool {
    blockpy_function_instr_any(
        function,
        |expr| matches!(expr, InstrWithAwaitAndYield::CellRefForName(cell_ref) if cell_ref.logical_name == expected),
    )
}

fn resolved_function_has_name_location(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    name: &str,
    mut location_predicate: impl FnMut(NameLocation) -> bool,
) -> bool {
    blockpy_function_instr_any(function, |expr| match expr {
        InstrResolved::Load(load) => {
            load.name.id_str() == name && location_predicate(load.name.location)
        }
        InstrResolved::Store(store) => {
            store.name.id_str() == name && location_predicate(store.name.location)
        }
        InstrResolved::Del(del) => {
            del.name.id_str() == name && location_predicate(del.name.location)
        }
        _ => false,
    })
}

fn resolved_function_uses_global(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    name: &str,
) -> bool {
    resolved_function_has_name_location(function, name, |location| {
        location.is_global() || location.is_global_name()
    })
}

fn resolved_module_uses_global(
    module: &BlockPyModule<ResolvedStorageModuleShape>,
    name: &str,
) -> bool {
    module
        .callable_defs
        .iter()
        .any(|function| resolved_function_uses_global(function, name))
}

fn resolved_function_uses_captured_source(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
) -> bool {
    blockpy_function_instr_any(function, |expr| match expr {
        InstrResolved::Load(load) => load
            .name
            .cell_location()
            .is_some_and(CellLocation::is_captured_source),
        InstrResolved::Store(store) => store
            .name
            .cell_location()
            .is_some_and(CellLocation::is_captured_source),
        InstrResolved::Del(del) => del
            .name
            .cell_location()
            .is_some_and(CellLocation::is_captured_source),
        InstrResolved::CellRef(cell_ref) => cell_ref.location.is_captured_source(),
        _ => false,
    })
}

fn resolved_function_has_store_to_captured_source(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
) -> bool {
    blockpy_function_instr_any(function, |expr| {
        matches!(
            expr,
            InstrResolved::Store(store)
                if store
                    .name
                    .cell_location()
                    .is_some_and(CellLocation::is_captured_source)
        )
    })
}

fn resolved_function_has_del(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    name: &str,
    quietly: bool,
) -> bool {
    blockpy_function_instr_any(function, |expr| {
        matches!(
            expr,
            InstrResolved::Del(del)
                if del.name.id_str() == name && del.quietly == quietly
        )
    })
}

fn resolved_function_has_delitem(function: &BlockPyFunction<ResolvedStorageModuleShape>) -> bool {
    blockpy_function_instr_any(function, |expr| matches!(expr, InstrResolved::DelItem(_)))
}

fn resolved_function_has_setitem(function: &BlockPyFunction<ResolvedStorageModuleShape>) -> bool {
    blockpy_function_instr_any(function, |expr| matches!(expr, InstrResolved::SetItem(_)))
}

fn resolved_function_has_make_cell(function: &BlockPyFunction<ResolvedStorageModuleShape>) -> bool {
    blockpy_function_instr_any(function, |expr| matches!(expr, InstrResolved::MakeCell(_)))
}

fn resolved_function_has_make_function_with_closure(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
) -> bool {
    blockpy_function_instr_any(function, |expr| {
        matches!(expr, InstrResolved::MakeFunctionWithClosure(_))
    })
}

fn resolved_function_has_cell_ref(function: &BlockPyFunction<ResolvedStorageModuleShape>) -> bool {
    blockpy_function_instr_any(function, |expr| matches!(expr, InstrResolved::CellRef(_)))
}

fn block_uses_text(block: &ResolvedStorageBlock, needle: &str) -> bool {
    block.body.iter().any(|op| expr_text(op).contains(needle))
        || match &block.term {
            BlockTerm::IfTerm(if_term) => expr_text(&if_term.test).contains(needle),
            BlockTerm::BranchTable(branch) => expr_text(&branch.index).contains(needle),
            BlockTerm::Raise(raise_stmt) => raise_stmt
                .exc
                .as_ref()
                .is_some_and(|value| expr_text(value).contains(needle)),
            BlockTerm::Return(value) => expr_text(value).contains(needle),
            _ => false,
        }
}

#[test]
fn instr_ruff_from_ast_expr_normalizes_bare_yield_to_explicit_none() {
    let instr = crate::passes::ast_to_instr::from_ast_expr(Expr::Yield(ast::ExprYield {
        node_index: ast::AtomicNodeIndex::default(),
        range: ruff_text_size::TextRange::default(),
        value: None,
    }));

    let InstrRuff::Yield(yield_expr) = instr else {
        panic!("expected InstrRuff::Yield");
    };
    assert!(matches!(
        yield_expr.value.as_ref(),
        InstrRuff::ExprNoneLiteral(_)
    ));
}

#[test]
fn instr_ruff_from_ast_expr_normalizes_call_args_and_keywords() {
    let instr =
        crate::passes::ast_to_instr::from_ast_expr(crate::py_expr!("f(x, *args, y=z, **kw)"));

    let InstrRuff::Call(call) = instr else {
        panic!("expected InstrRuff::Call");
    };
    assert_eq!(call.args.len(), 2);
    assert!(matches!(call.args[0], CallArgPositional::Positional(_)));
    assert!(matches!(call.args[1], CallArgPositional::Starred(_)));
    assert_eq!(call.keywords.len(), 2);
    assert!(matches!(
        &call.keywords[0],
        CallArgKeyword::Named { arg, .. } if arg.as_str() == "y"
    ));
    assert!(matches!(call.keywords[1], CallArgKeyword::Starred(_)));
}

#[test]
fn instr_ruff_from_ast_stmt_recursively_lowers_assign_value() {
    let instr = crate::passes::ast_to_instr::from_ast_stmt(crate::py_stmt!("x = bare_yield"));

    let InstrRuff::StmtAssign(assign) = instr else {
        panic!("expected InstrRuff::StmtAssign");
    };
    assert_eq!(assign.targets.len(), 1);
    assert!(matches!(assign.targets[0], InstrRuff::ExprName(_)));
    assert!(matches!(assign.value.as_ref(), InstrRuff::ExprName(_)));
}

#[test]
fn instr_ruff_from_ast_stmt_recursively_lowers_function_body_and_return() {
    let instr =
        crate::passes::ast_to_instr::from_ast_stmt(Stmt::FunctionDef(ast::StmtFunctionDef {
            node_index: ast::AtomicNodeIndex::default(),
            range: ruff_text_size::TextRange::default(),
            is_async: false,
            decorator_list: Vec::new(),
            name: ast::Identifier::new("f", ruff_text_size::TextRange::default()),
            type_params: None,
            parameters: Box::new(ast::Parameters {
                range: ruff_text_size::TextRange::default(),
                node_index: ast::AtomicNodeIndex::default(),
                posonlyargs: Vec::new(),
                args: Vec::new(),
                vararg: None,
                kwonlyargs: Vec::new(),
                kwarg: None,
            }),
            returns: None,
            body: vec![crate::py_stmt!("return g(x)")],
        }));

    let InstrRuff::StmtFunctionDef(func) = instr else {
        panic!("expected InstrRuff::StmtFunctionDef");
    };
    assert_eq!(func.body.len(), 1);
    let InstrRuff::StmtReturn(ret) = &func.body[0] else {
        panic!("expected function body return");
    };
    let value = &ret.value;
    assert!(matches!(value.as_ref(), InstrRuff::Call(_)));
}

#[test]
fn instr_ruff_from_ast_stmt_normalizes_bare_return_to_explicit_none() {
    let instr = crate::passes::ast_to_instr::from_ast_stmt(crate::py_stmt!("return"));

    let InstrRuff::StmtReturn(ret) = instr else {
        panic!("expected InstrRuff::StmtReturn");
    };
    let value = &ret.value;
    assert!(matches!(value.as_ref(), InstrRuff::ExprNoneLiteral(_)));
}

#[test]
fn instr_ruff_from_ast_stmt_recursively_lowers_loop_body_and_orelse() {
    let instr = crate::passes::ast_to_instr::from_ast_stmt(Stmt::While(ast::StmtWhile {
        node_index: ast::AtomicNodeIndex::default(),
        range: ruff_text_size::TextRange::default(),
        test: Box::new(crate::py_expr!("cond")),
        body: vec![crate::py_stmt!("x = 1")],
        orelse: vec![crate::py_stmt!("y = 2")],
    }));

    let InstrRuff::StmtWhile(while_stmt) = instr else {
        panic!("expected InstrRuff::StmtWhile");
    };
    assert!(matches!(while_stmt.test.as_ref(), InstrRuff::ExprName(_)));
    assert!(matches!(&while_stmt.body[..], [InstrRuff::StmtAssign(_)]));
    assert!(matches!(&while_stmt.orelse[..], [InstrRuff::StmtAssign(_)]));
}
fn function_uses_text(
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    needle: &str,
) -> bool {
    function
        .blocks
        .iter()
        .any(|block| block_uses_text(block, needle))
}

fn module_constant_text(module: &BlockPyModule<ResolvedStorageModuleShape>) -> String {
    module
        .module_constants
        .iter()
        .map(expr_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn function_or_constants_use_text(
    module: &BlockPyModule<ResolvedStorageModuleShape>,
    function: &BlockPyFunction<ResolvedStorageModuleShape>,
    needle: &str,
) -> bool {
    function_uses_text(function, needle) || module_constant_text(module).contains(needle)
}

fn runtime_call_by_name<'a>(
    module: &'a BlockPyModule<ResolvedStorageModuleShape>,
    expr: &'a InstrResolved,
    name: &str,
) -> Option<&'a Call<InstrResolved>> {
    let InstrResolved::Call(call) = expr else {
        return None;
    };
    let InstrResolved::Load(load) = call.func.as_ref() else {
        return None;
    };
    if load.name.is_runtime_symbol(name) {
        return Some(call);
    }
    let Some(constant_index) = load.name.location.as_constant() else {
        return None;
    };
    let Some(InstrResolved::Load(helper_load)) =
        module.module_constants.get(constant_index as usize)
    else {
        return None;
    };
    helper_load.name.is_runtime_symbol(name).then_some(call)
}

fn module_constant_runtime_name(
    module: &BlockPyModule<ResolvedStorageModuleShape>,
    name: &str,
) -> bool {
    module
        .module_constants
        .iter()
        .any(|expr| matches!(expr, InstrResolved::Load(load) if load.name.is_runtime_symbol(name)))
}

#[test]
fn core_blockpy_with_await_keeps_plain_coroutines_without_fake_yield_marker() {
    let source = r#"
async def foo():
    return 1

async def classify():
    return await foo()
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let classify = blockpy_function_by_name(&blockpy_module, "classify");
    assert_eq!(classify.kind, FunctionKind::Coroutine);
    assert!(blockpy_function_instr_any(classify, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::Await(_)
    )));
    assert!(!blockpy_function_instr_any(classify, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::Yield(_)
    )));
}

#[test]
fn core_blockpy_lowers_fstring_before_bb_lowering() {
    let source = r#"
def fmt(value):
    return f"{value=}"
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let semantic_fmt = blockpy_function_by_name(&blockpy_module, "fmt");
    assert!(blockpy_function_has_string_literal(semantic_fmt, "value="));
    assert!(blockpy_function_has_root_name(semantic_fmt, "repr"));
    assert!(blockpy_function_has_root_name(semantic_fmt, "format"));

    let fmt = lowered.bb_function("fmt");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), fmt, "repr"),
        "{fmt:?}"
    );
    assert!(
        function_or_constants_use_text(lowered.bb_module(), fmt, "format"),
        "{fmt:?}"
    );
}

#[test]
fn core_blockpy_lowers_tstring_before_bb_lowering() {
    let source = r#"
def fmt(value):
    return t"{value}"
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let semantic_fmt = blockpy_function_by_name(&blockpy_module, "fmt");
    assert!(blockpy_function_has_root_name(
        semantic_fmt,
        "templatelib_Interpolation"
    ));
    assert!(blockpy_function_has_string_literal(semantic_fmt, "value"));

    let fmt = lowered.bb_function("fmt");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), fmt, "templatelib_Interpolation"),
        "{fmt:?}"
    );
    assert!(lowered
        .bb_module()
        .module_constants
        .iter()
        .any(|expr| expr.root_string_literal_value().as_deref() == Some("value")));
    assert!(lowered
        .bb_module()
        .module_constants
        .iter()
        .any(|expr| expr.root_string_literal_value().as_deref() == Some("")));
}

#[test]
fn lowers_simple_if_function_into_basic_blocks() {
    let source = r#"
def foo(a, b):
    c = a + b
    if c > 5:
        print("hi", c)
    else:
        d = b + 1
        print(d)
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let foo = function_by_name(&bb_module, "foo");
    assert!(foo.blocks.len() >= 3, "{foo:?}");
    assert!(
        foo.blocks
            .iter()
            .any(|block| matches!(block.term, BlockTerm::IfTerm(_))),
        "{foo:?}"
    );
}

#[test]
fn name_binding_lowers_make_function_to_structured_closure_node() {
    let source = r#"
def f():
    pass
"#;

    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let f = function_by_name(&bb_module, "f");
    let module_init = function_by_name(&bb_module, "_dp_module_init");
    let mut saw_make_function = false;
    let mut saw_empty_captures = false;
    let mut saw_empty_defaults = false;
    assert!(!module_constant_runtime_name(&bb_module, "make_function"));
    assert!(
        blockpy_function_instr_any(module_init, |expr| {
            let InstrResolved::MakeFunctionWithClosure(op) = expr else {
                return false;
            };
            saw_make_function = true;
            saw_empty_captures = matches!(op.captures.as_ref(), InstrResolved::Tuple(tuple) if tuple.values.is_empty());
            saw_empty_defaults = matches!(op.param_defaults.as_ref(), InstrResolved::Tuple(tuple) if tuple.values.is_empty());
            op.function_id() == f.function_id && op.kind == FunctionKind::Function
        }),
        "expected module init to use MakeFunctionWithClosure, got {module_init:?}"
    );
    assert!(saw_make_function);
    assert!(saw_empty_captures);
    assert!(saw_empty_defaults);
}

#[test]
fn module_init_is_tagged_for_interpreted_execution() {
    let bb_module = tracked_name_binding_module(
        r#"
VALUE = 1

def f():
    return VALUE
"#,
    )
    .expect("transform should succeed")
    .expect("bb module should be available");
    let module_init = function_by_name(&bb_module, "_dp_module_init");
    let f = function_by_name(&bb_module, "f");
    assert_eq!(
        module_init.execution_mode(),
        FunctionExecutionMode::Interpreted
    );
    assert_eq!(f.execution_mode(), FunctionExecutionMode::Jit);
}

#[test]
fn unsound_name_binding_lifts_undeclared_builtin_global_loads_into_module_constants() {
    let source = r#"
def f(value):
    print(len(range(value)))
"#;

    let bb_module = unsound_runtime_builtin_name_binding_module(source);
    let f = function_by_name(&bb_module, "f");
    for name in ["print", "len", "range"] {
        assert!(
            module_constant_runtime_name(&bb_module, name),
            "expected {name} to be an extracted runtime-name constant; got {:?}",
            bb_module.module_constants
        );
        assert!(
            !resolved_function_uses_global(f, name),
            "expected {name} load to skip module globals in unsound mode; got {f:?}",
        );
    }
}

#[test]
fn unsound_name_binding_keeps_assigned_or_declared_builtin_names_global() {
    let source = r#"
len = lambda value: 42

def assigned(value):
    return len(value)

def declared(value):
    global range
    range = lambda value: [value]
    return range(value)
"#;

    let bb_module = unsound_runtime_builtin_name_binding_module(source);
    assert!(
        !module_constant_runtime_name(&bb_module, "len"),
        "assigned module global len should not be a runtime-name constant"
    );
    assert!(
        !module_constant_runtime_name(&bb_module, "range"),
        "explicit global range should not be a runtime-name constant"
    );
    assert!(resolved_module_uses_global(&bb_module, "len"));
    assert!(resolved_module_uses_global(&bb_module, "range"));
}

#[test]
fn exposes_bb_ir_for_lowered_functions() {
    let source = r#"
def foo(a, b):
    if a:
        return b
    return a
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let foo = bb_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "foo")
        .expect("foo should be lowered");
    assert_eq!(
        foo.entry_block().label_str(),
        foo.blocks
            .first()
            .expect("foo should have a first block")
            .label_str()
    );
    assert_ne!(
        foo.entry_block().label_str(),
        "start",
        "{:?}",
        foo.entry_block().label_str()
    );
    assert!(!foo.blocks.is_empty());
}

#[test]
fn nested_global_function_def_stays_lowered() {
    let source = r#"
def build_qualnames():
    def global_function():
        def inner_function():
            global inner_global_function
            def inner_global_function():
                pass
            return inner_global_function
        return inner_function()
    return global_function()
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let inner_global_function = function_by_name(&bb_module, "inner_global_function");
    assert_eq!(
        inner_global_function.names.qualname,
        "inner_global_function"
    );
}

#[test]
fn lowered_class_helper_records_class_scope_kind() {
    let source = r#"
class Box:
    value = 1
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = callable_def_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert_eq!(class_helper.scope.scope_kind, CallableScopeKind::Class);
}

#[test]
fn class_body_local_load_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    y = 1
    z = y
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "class_lookup_global"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(
        function_or_constants_use_text(
            lowered.bb_module(),
            resolved_class_helper,
            "class_lookup_global"
        ),
        "{resolved_class_helper:?}\n{}",
        module_constant_text(lowered.bb_module())
    );
}

#[test]
fn class_body_nonlocal_load_moves_to_name_binding_pass() {
    let source = r#"
def outer():
    x = 1
    class Box:
        y = x
    return Box
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "class_lookup_cell"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(!function_or_constants_use_text(
        lowered.bb_module(),
        resolved_class_helper,
        "class_lookup_cell",
    ));
    assert!(resolved_function_has_setitem(resolved_class_helper));
    assert!(resolved_function_uses_captured_source(
        resolved_class_helper
    ));
}

#[test]
fn class_body_nonlocal_load_passes_raw_cell_to_class_lookup() {
    let source = r#"
def outer():
    x = "outer"
    class Inner:
        y = x
    return Inner.y
"#;

    let lowered = TrackedLowering::new(source);
    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Inner");
    assert!(resolved_function_has_setitem(resolved_class_helper));
    assert!(resolved_function_uses_captured_source(
        resolved_class_helper
    ));
}

#[test]
fn class_body_function_binding_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    def f(self):
        return 1
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_store_name(class_helper, "f"));
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_setitem"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(
        resolved_function_has_setitem(resolved_class_helper)
            && resolved_function_has_make_function_with_closure(resolved_class_helper),
        "{resolved_class_helper:?}\n{}",
        module_constant_text(lowered.bb_module())
    );
}

#[test]
fn class_body_nonlocal_assignment_moves_to_name_binding_pass() {
    let source = r#"
def outer():
    x = 0
    class Box:
        nonlocal x
        x = 1
    return x
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_store_name(class_helper, "x"));
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_store_cell"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_has_store_to_captured_source(
        resolved_class_helper
    ));
}

#[test]
fn class_body_local_assignment_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    x = 1
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_store_name(class_helper, "x"));
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_setitem"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_has_setitem(resolved_class_helper));
}

#[test]
fn class_body_delete_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    x = 1
    del x
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_del_name(class_helper, "x"));
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_delitem"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_has_delitem(resolved_class_helper));
}

#[test]
fn class_body_nonlocal_delete_moves_to_name_binding_pass() {
    let source = r#"
def outer():
    x = 1
    class Box:
        nonlocal x
        del x
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_del_name(class_helper, "x"));
    assert!(!blockpy_module_has_root_name(
        &blockpy_module,
        "cell_contents"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_has_del(resolved_class_helper, "x", false));
    assert!(resolved_function_uses_captured_source(
        resolved_class_helper
    ));
}

#[test]
fn method_dunder_class_load_moves_to_name_binding_pass() {
    let source = concat!(
        "class C:\n",
        "    def f(self):\n",
        "        return __class__\n",
    );

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let method = blockpy_function_by_name(&blockpy_module, "f");
    assert!(blockpy_function_has_root_name(method, "__class__"));
    assert!(!blockpy_function_has_root_name(method, "cell_contents"));

    let resolved_method = lowered.bb_function("f");
    assert!(resolved_function_uses_captured_source(resolved_method));
}

#[test]
fn nested_method_dunder_class_capture_uses_classcell_storage() {
    let source = concat!(
        "class C:\n",
        "    def f(self):\n",
        "        def g():\n",
        "            return __class__\n",
        "        return g()\n",
    );

    let lowered = TrackedLowering::new(source);
    let resolved_inner = lowered.bb_function("g");
    assert_eq!(resolved_inner.names.qualname, "C.f.<locals>.g");
    assert!(resolved_function_uses_captured_source(resolved_inner));
    assert!(
        resolved_function_has_make_function_with_closure(lowered.bb_function("f"))
            && resolved_function_has_cell_ref(lowered.bb_function("f")),
        "{resolved_inner:?}\n{}",
        module_constant_text(lowered.bb_module())
    );
}

#[test]
fn method_super_uses_cell_ref_marker_for_classcell() {
    let source = concat!(
        "class C:\n",
        "    def f(self):\n",
        "        return super().f()\n",
    );

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let method = blockpy_function_by_name(&blockpy_module, "f");
    assert!(blockpy_function_has_cell_ref_for_name(method, "__class__"));
    assert!(blockpy_function_has_root_name(method, "call_super"));
    assert!(!blockpy_function_has_root_name(method, "_dp_classcell"));

    let resolved_method = lowered.bb_function("f");
    assert!(resolved_function_has_cell_ref(resolved_method));
    assert!(
        module_constant_text(lowered.bb_module()).contains("call_super")
            && resolved_function_has_name_location(resolved_method, "self", |location| location
                .as_local()
                .is_some()),
        "{resolved_method:?}\n{}",
        module_constant_text(lowered.bb_module())
    );
}

#[test]
fn method_explicit_super_records_classcell_capture_for_original_code_shape() {
    let source = concat!(
        "class C:\n",
        "    def f(self):\n",
        "        return super(C, self).f()\n",
    );

    let lowered = TrackedLowering::new(source);
    let resolved_method = lowered.bb_function("f");
    let layout = resolved_method
        .storage_layout
        .as_ref()
        .expect("method should have closure layout for CPython __class__ code object");

    assert!(
        layout
            .freevars
            .iter()
            .any(|slot| slot.logical_name == "__class__" && slot.storage_name == "__class__"),
        "{resolved_method:?}\n{}",
        lowered.name_binding_text(),
    );
}

#[test]
fn module_function_explicit_super_does_not_record_classcell_capture() {
    let source = concat!("def f(cls):\n", "    return super(Generic, cls).f()\n",);

    let lowered = TrackedLowering::new(source);
    let resolved_function = lowered.bb_function("f");
    let has_class_freevar = resolved_function
        .storage_layout
        .as_ref()
        .is_some_and(|layout| {
            layout
                .freevars
                .iter()
                .any(|slot| slot.logical_name == "__class__")
        });

    assert!(
        !has_class_freevar,
        "{resolved_function:?}\n{}",
        lowered.name_binding_text(),
    );
}

#[test]
fn nested_method_dunder_class_capture_does_not_leak_classcell_to_enclosing_scopes() {
    let source = concat!(
        "def exercise():\n",
        "    class C:\n",
        "        def f(self):\n",
        "            def g():\n",
        "                return __class__\n",
        "            return g()\n",
        "    return C().f(), C\n",
    );
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let module_init = function_by_name(&bb_module, "_dp_module_init");
    assert!(
        module_init
            .storage_layout()
            .as_ref()
            .is_none_or(|layout| layout.freevars.is_empty()),
        "{module_init:?}"
    );
    let exercise = function_by_name(&bb_module, "exercise");
    assert!(
        exercise
            .storage_layout()
            .as_ref()
            .is_none_or(|layout| layout.freevars.is_empty()),
        "{exercise:?}"
    );
    let class_ns = function_by_name(&bb_module, "_dp_class_ns_C");
    assert!(
        class_ns
            .storage_layout()
            .as_ref()
            .is_none_or(|layout| layout.freevars.is_empty()),
        "{class_ns:?}"
    );
    let method = function_by_name(&bb_module, "f");
    let class_slot = slot_by_name(
        &method
            .storage_layout()
            .as_ref()
            .expect("method should have closure layout")
            .freevars,
        "__class__",
    );
    assert_eq!(class_slot.storage_name, "__class__");
}

#[test]
fn nested_class_closure_capture_does_not_turn_owner_cell_into_outer_freevar() {
    let source = concat!(
        "class Outer:\n",
        "    def run(self):\n",
        "        counter = 0\n",
        "        class Inner:\n",
        "            def bump(self):\n",
        "                nonlocal counter\n",
        "                counter += 1\n",
        "        Inner().bump()\n",
        "        return counter\n",
    );
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let run = function_by_name(&bb_module, "run");
    assert!(
        run.storage_layout()
            .as_ref()
            .is_none_or(|layout| layout.freevars.is_empty()),
        "{run:?}"
    );
    let class_ns = function_by_name(&bb_module, "_dp_class_ns_Inner");
    let counter_slot = slot_by_name(
        &class_ns
            .storage_layout()
            .as_ref()
            .expect("class helper should have closure layout")
            .freevars,
        "counter",
    );
    assert_eq!(counter_slot.storage_name, "_dp_cell_counter");
}

#[test]
fn class_body_local_does_not_satisfy_nested_method_capture() {
    let source = concat!(
        "def outer():\n",
        "    x = \"outer\"\n",
        "    class Inner:\n",
        "        x = \"class\"\n",
        "        def read():\n",
        "            return x\n",
        "    return Inner.read()\n",
    );
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let class_ns = function_by_name(&bb_module, "_dp_class_ns_Inner");
    let class_layout = class_ns
        .storage_layout()
        .as_ref()
        .expect("class helper should capture outer x for nested method");
    let captured_x = slot_by_name(&class_layout.freevars, "x");
    assert_eq!(captured_x.storage_name, "_dp_cell_x");
    assert!(
        class_layout
            .cellvars
            .iter()
            .all(|slot| slot.logical_name != "x"),
        "class namespace local must not become a closure owner: {class_layout:?}"
    );

    let read = function_by_name(&bb_module, "read");
    let read_layout = read
        .storage_layout()
        .as_ref()
        .expect("nested method should capture x");
    let _read_x = slot_by_name(&read_layout.freevars, "x");
}

#[test]
fn class_body_local_does_not_satisfy_nested_class_capture() {
    let source = concat!(
        "def outer():\n",
        "    z2 = \"outer\"\n",
        "    class Inner:\n",
        "        z2 = \"inner\"\n",
        "        class InnerClosure:\n",
        "            y = z2\n",
        "    return Inner.InnerClosure.y\n",
    );
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let outer_class_ns = function_by_name(&bb_module, "_dp_class_ns_Inner");
    let outer_class_layout = outer_class_ns
        .storage_layout()
        .as_ref()
        .expect("outer class helper should capture outer z2 for nested class");
    let captured_z2 = slot_by_name(&outer_class_layout.freevars, "z2");
    assert_eq!(captured_z2.storage_name, "_dp_cell_z2");
    assert!(
        outer_class_layout
            .cellvars
            .iter()
            .all(|slot| slot.logical_name != "z2"),
        "class namespace local must not become a closure owner: {outer_class_layout:?}"
    );

    let inner_class_ns = function_by_name(&bb_module, "_dp_class_ns_InnerClosure");
    let inner_class_layout = inner_class_ns
        .storage_layout()
        .as_ref()
        .expect("nested class helper should capture z2");
    let _inner_z2 = slot_by_name(&inner_class_layout.freevars, "z2");
}

#[test]
fn nested_class_base_capture_keeps_method_local_cell_owned_by_method() {
    let source = concat!(
        "class Tests:\n",
        "    def run(self):\n",
        "        class Base:\n",
        "            pass\n",
        "        class Child[T](Base):\n",
        "            pass\n",
        "        return Child\n",
    );
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");

    let class_ns = function_by_name(&bb_module, "_dp_class_ns_Tests");
    assert!(
        class_ns
            .storage_layout()
            .as_ref()
            .is_none_or(|layout| layout.freevars.is_empty()),
        "{class_ns:?}"
    );

    let run = function_by_name(&bb_module, "run");
    let run_layout = run
        .storage_layout()
        .as_ref()
        .expect("method should own a cell for Base");
    assert!(
        run_layout.freevars.is_empty(),
        "method-local Base must not become an inherited freevar: {run_layout:?}"
    );
    let base_slot = slot_by_name(&run_layout.cellvars, "Base");
    assert_eq!(base_slot.storage_name, "_dp_cell_Base");

    let child_ns = function_by_name(&bb_module, "_dp_class_ns_Child");
    let _child_base_slot = slot_by_name(
        &child_ns
            .storage_layout()
            .as_ref()
            .expect("nested class helper should capture Base")
            .freevars,
        "Base",
    );
}

#[test]
fn method_local_class_base_capture_does_not_leak_to_enclosing_class() {
    let source = concat!(
        "class Container:\n",
        "    def method(self):\n",
        "        class RawBase:\n",
        "            pass\n",
        "        class Derived(RawBase):\n",
        "            pass\n",
        "        return Derived\n",
    );
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let outer_class_ns = function_by_name(&bb_module, "_dp_class_ns_Container");
    assert!(
        outer_class_ns
            .storage_layout()
            .as_ref()
            .is_none_or(|layout| layout.freevars.is_empty()),
        "{outer_class_ns:?}"
    );

    let method = function_by_name(&bb_module, "method");
    assert!(
        method.storage_layout().as_ref().is_none_or(|layout| {
            layout
                .freevars
                .iter()
                .all(|slot| slot.logical_name != "RawBase")
        }),
        "{method:?}"
    );
}

#[test]
fn class_global_dunder_class_does_not_leak_synthetic_classcell_outward() {
    let source = concat!(
        "def exercise():\n",
        "    class X:\n",
        "        global __class__\n",
        "        __class__ = 42\n",
        "        def f(self):\n",
        "            return __class__\n",
        "    return X().f(), X\n",
    );
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let module_init = function_by_name(&bb_module, "_dp_module_init");
    assert!(
        module_init
            .storage_layout()
            .as_ref()
            .is_none_or(|layout| layout.freevars.is_empty()),
        "{module_init:?}"
    );
    let exercise = function_by_name(&bb_module, "exercise");
    assert!(
        exercise
            .storage_layout()
            .as_ref()
            .is_none_or(|layout| layout.freevars.is_empty()),
        "{exercise:?}"
    );
    let class_ns = function_by_name(&bb_module, "_dp_class_ns_X");
    assert!(
        class_ns
            .storage_layout()
            .as_ref()
            .is_none_or(|layout| layout.freevars.is_empty()),
        "{class_ns:?}"
    );
    let method = function_by_name(&bb_module, "f");
    let class_slot = slot_by_name(
        &method
            .storage_layout()
            .as_ref()
            .expect("method should have closure layout")
            .freevars,
        "__class__",
    );
    assert_eq!(class_slot.storage_name, "__class__");
}

#[test]
fn class_body_except_name_global_binding_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    global caught
    try:
        raise Exception("boom")
    except Exception as caught:
        seen = caught
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_store_name(class_helper, "caught"));
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_current_exception"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_uses_global(
        resolved_class_helper,
        "caught"
    ));
    assert!(resolved_function_has_del(
        resolved_class_helper,
        "caught",
        true
    ));
}

#[test]
fn class_body_except_name_nonlocal_binding_moves_to_name_binding_pass() {
    let source = r#"
def outer():
    x = "outer"
    class Box:
        nonlocal x
        try:
            raise Exception("boom")
        except Exception as x:
            pass
    return x
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_store_name(class_helper, "x"));
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_current_exception"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_has_store_to_captured_source(
        resolved_class_helper
    ));
    assert!(resolved_function_has_del(resolved_class_helper, "x", true));
    assert!(resolved_function_uses_captured_source(
        resolved_class_helper
    ));
}

#[test]
fn class_body_except_name_local_binding_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    try:
        raise Exception("boom")
    except Exception as caught:
        seen = str(caught)
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_store_name(class_helper, "caught"));
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_current_exception"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_has_setitem(resolved_class_helper));
    assert!(resolved_function_has_delitem(resolved_class_helper));
}

#[test]
fn class_body_global_named_expr_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    global y
    x = (y := 1)
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_store_global"
    ));
    assert!(blockpy_function_has_store_name(class_helper, "y"));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_uses_global(resolved_class_helper, "y"));
}

#[test]
fn class_body_nonlocal_named_expr_moves_to_name_binding_pass() {
    let source = r#"
def outer():
    x = 0
    class Box:
        nonlocal x
        y = (x := 1)
    return x
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_store_cell"
    ));
    assert!(blockpy_function_has_store_name(class_helper, "x"));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_has_store_to_captured_source(
        resolved_class_helper
    ));
}

#[test]
fn class_body_global_for_target_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    global y
    for y in [1]:
        pass
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_store_global"
    ));
    assert!(blockpy_function_has_store_name(class_helper, "y"));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_uses_global(resolved_class_helper, "y"));
}

#[test]
fn class_body_nonlocal_for_target_moves_to_name_binding_pass() {
    let source = r#"
def outer():
    x = 0
    class Box:
        nonlocal x
        for x in [1]:
            pass
    return x
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_store_cell"
    ));
    assert!(blockpy_function_has_store_name(class_helper, "x"));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(resolved_function_has_store_to_captured_source(
        resolved_class_helper
    ));
}

#[test]
fn class_body_local_with_target_moves_to_name_binding_pass() {
    let source = r#"
from contextlib import nullcontext

class Box:
    with nullcontext(1) as value:
        seen = value
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_store_name(class_helper, "value"));
    assert!(blockpy_function_has_root_name(
        class_helper,
        "contextmanager_enter"
    ));
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_setitem"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(
        resolved_function_has_setitem(resolved_class_helper)
            && module_constant_text(lowered.bb_module()).contains("contextmanager_enter"),
        "{resolved_class_helper:?}\n{}",
        module_constant_text(lowered.bb_module())
    );
}

#[test]
fn class_body_nonlocal_with_target_moves_to_name_binding_pass() {
    let source = r#"
from contextlib import nullcontext

def outer():
    value = "outer"
    class Box:
        nonlocal value
        with nullcontext(1) as value:
            pass
    return value
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_Box");
    assert!(blockpy_function_has_store_name(class_helper, "value"));
    assert!(blockpy_function_has_root_name(
        class_helper,
        "contextmanager_enter"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_Box");
    assert!(
        resolved_function_has_store_to_captured_source(resolved_class_helper)
            && module_constant_text(lowered.bb_module()).contains("contextmanager_enter"),
        "{resolved_class_helper:?}\n{}",
        module_constant_text(lowered.bb_module())
    );
}

#[test]
fn nested_class_binding_moves_to_name_binding_pass() {
    let source = r#"
class A:
    class B:
        pass
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let class_helper = blockpy_function_by_name(&blockpy_module, "_dp_class_ns_A");
    assert!(blockpy_function_has_store_name(class_helper, "B"));
    assert!(blockpy_function_has_root_name(
        class_helper,
        "_dp_define_class_B"
    ));
    assert!(!blockpy_function_has_root_name(
        class_helper,
        "__dp_setitem"
    ));

    let resolved_class_helper = lowered.bb_function("_dp_class_ns_A");
    assert!(
        resolved_function_has_setitem(resolved_class_helper)
            && resolved_function_has_make_function_with_closure(resolved_class_helper),
        "{resolved_class_helper:?}\n{}",
        module_constant_text(lowered.bb_module())
    );
}

#[test]
fn lowered_callable_records_semantic_cell_owner_binding() {
    let source = r#"
def outer():
    def recurse():
        return recurse()
    return recurse
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let outer = callable_def_by_name(&blockpy_module, "outer");
    assert_eq!(
        outer.scope.binding_kind("recurse"),
        Some(crate::block_py::BindingKind::Cell(
            crate::block_py::CellBindingKind::Owner
        )),
        "{:?}",
        outer.scope.bindings
    );
}

#[test]
fn closure_backed_generator_does_not_lift_module_globals() {
    let source = r#"
def child():
    yield "start"

def delegator():
    result = yield from child()
    return ("done", result)
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let delegator = function_by_name(&bb_module, "delegator");
    let layout = delegator
        .storage_layout()
        .as_ref()
        .expect("closure-backed generator should record closure layout");
    assert!(
        !layout
            .cellvars
            .iter()
            .any(|slot| slot.logical_name == "child"),
        "{layout:?}"
    );
}

#[test]
fn generator_resume_yield_from_blocks_drop_cell_storage_alias_params() {
    let source = r#"
def child():
    yield "start"

def delegator():
    result = yield from child()
    return ("done", result)
"#;

    let lowered = TrackedLowering::new(source);
    let core_module = lowered
        .result
        .pass_tracker
        .pass_core_blockpy()
        .expect("expected core no-yield pass");
    let resume_function = core_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "delegator_resume")
        .expect("expected hidden generator resume function");
    let yield_from_except = resume_function
        .blocks
        .iter()
        .find(|block| {
            block
                .params
                .iter()
                .any(|param| param.name.starts_with("_dp_yield_from_exc_"))
        })
        .expect("expected synthesized yield_from_except block");

    assert!(
        yield_from_except
            .params
            .iter()
            .any(|param| param.name.starts_with("_dp_yield_from_exc_")),
        "{yield_from_except:?}"
    );
    assert!(
        yield_from_except
            .params
            .iter()
            .all(|param| !param.name.starts_with("_dp_cell_")),
        "{yield_from_except:?}"
    );
}

#[test]
fn generator_resume_pc_moves_to_name_binding_pass() {
    let source = r#"
def gen():
    yield 1
    yield 2
"#;

    let lowered = TrackedLowering::new(source);
    let resume = lowered.bb_function("gen");
    let entry_params = resume.entry_block().param_name_vec();
    assert!(
        !entry_params.iter().any(|name| name == "_dp_pc"),
        "{resume:?}"
    );
    assert!(resolved_function_uses_captured_source(resume), "{resume:?}");
    assert!(
        resolved_function_has_store_to_captured_source(resume),
        "{resume:?}"
    );
}

#[test]
fn generator_resume_yieldfrom_moves_to_name_binding_pass() {
    let source = r#"
def child():
    yield "start"

def delegator():
    result = yield from child()
    return ("done", result)
"#;

    let lowered = TrackedLowering::new(source);
    let resume = lowered.bb_function("delegator");
    let entry_params = resume.entry_block().param_name_vec();
    assert!(
        !entry_params.iter().any(|name| name == "_dp_pc")
            && !entry_params.iter().any(|name| name == "_dp_yieldfrom"),
        "{resume:?}"
    );
    assert!(resolved_function_uses_captured_source(resume), "{resume:?}");
    assert!(
        resolved_function_has_store_to_captured_source(resume),
        "{resume:?}"
    );
}

#[test]
fn generator_resume_local_state_moves_to_name_binding_pass() {
    let source = r#"
def gen():
    total = 0
    yield total
    total = total + 1
    yield total
"#;

    let lowered = TrackedLowering::new(source);
    let resume = lowered.bb_function("gen");
    let entry_params = resume.entry_block().param_name_vec();
    assert!(
        !entry_params.iter().any(|name| name == "total"),
        "{resume:?}"
    );
    assert!(resolved_function_uses_captured_source(resume), "{resume:?}");
    assert!(
        resolved_function_has_store_to_captured_source(resume),
        "{resume:?}"
    );
}

#[test]
fn async_genexpr_inherited_capture_moves_to_name_binding_pass() {
    let source = r#"
import asyncio

async def asynciter(seq):
    for item in seq:
        yield item

async def run():
    gen = ([i + j async for i in asynciter([1, 2])] for j in [10, 20])
    return [x async for x in gen]
"#;

    let lowered = TrackedLowering::new(source);
    let hidden_listcomp_resume = lowered.bb_function("_dp_listcomp_7");
    assert!(
        hidden_listcomp_resume
            .storage_layout()
            .as_ref()
            .is_some_and(|layout| !layout.freevars.is_empty())
            || resolved_function_uses_captured_source(hidden_listcomp_resume),
        "{hidden_listcomp_resume:?}"
    );
}

#[test]
fn generator_factory_owned_cell_init_moves_to_name_binding_pass() {
    let source = r#"
def gen():
    total = 0
    yield total
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let factory = blockpy_function_by_name(&blockpy_module, "gen");
    assert!(!blockpy_function_has_root_name(factory, "__dp_make_cell"));

    let resolved_factory = blockpy_function_by_name(lowered.bb_module(), "gen");
    let resolved_module = lowered.bb_module();
    let make_cell_count = resolved_module
        .callable_defs
        .iter()
        .flat_map(|function| &function.blocks)
        .flat_map(|block| &block.body)
        .filter(|stmt| {
            instr_any(*stmt, &mut |expr: &InstrResolved| {
                matches!(expr, InstrResolved::MakeCell(_))
            })
        })
        .count();
    assert!(
        resolved_function_has_make_cell(resolved_factory) && make_cell_count >= 3,
        "{resolved_factory:?}"
    );
}

#[test]
fn generator_resume_try_exception_state_moves_to_name_binding_pass() {
    let source = r#"
def gen():
    try:
        yield 1
    except ValueError:
        return 2
"#;

    let lowered = TrackedLowering::new(source);
    let resume = lowered.bb_function("gen");
    let storage_layout = resume
        .storage_layout()
        .as_ref()
        .expect("resume closure layout should contain try-exception state cell");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), resume, "exception_matches")
            && resolved_function_uses_captured_source(resume)
            && !blockpy_function_has_store_name(resume, "_dp_eval_"),
        "{resume:?}\n{}",
        module_constant_text(lowered.bb_module())
    );
    assert!(!resolved_function_has_del(resume, "_dp_eval_", false));

    let try_exc_slot = resume
        .storage_layout()
        .as_ref()
        .and_then(|layout| {
            layout
                .freevars
                .iter()
                .find(|slot| slot.logical_name.starts_with("_dp_try_exc_"))
        })
        .expect("resume closure layout should contain try-exception state cell");
    assert_eq!(try_exc_slot.init, ClosureInit::InheritedCapture);
    assert_eq!(
        resume
            .scope
            .binding_kind(try_exc_slot.logical_name.as_str()),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert!(storage_layout.has_storage_name(&try_exc_slot.storage_name));
}

#[test]
fn blockpy_callable_def_retains_docstring_metadata() {
    let source = r#"
def documented():
    "hello doc"
    return 1
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy = lowered.blockpy_module();
    let documented = callable_def_by_name(&blockpy, "documented");
    let doc = documented
        .doc
        .as_ref()
        .expect("callable definition should retain doc metadata");
    assert_eq!(doc, "hello doc");
}

#[test]
fn top_level_function_global_binding_moves_to_name_binding_pass() {
    let source = r#"
def f():
    return 1
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    assert!(!blockpy_module_has_root_name(
        &blockpy_module,
        "__dp_store_global"
    ));
    assert!(blockpy_module_has_store_name(&blockpy_module, "f"));

    let resolved_init = lowered.bb_function("_dp_module_init");
    assert!(
        resolved_function_uses_global(resolved_init, "f")
            && resolved_function_has_make_function_with_closure(resolved_init),
        "{resolved_init:?}\n{}",
        module_constant_text(lowered.bb_module())
    );
}

#[test]
fn top_level_global_assign_and_load_move_to_name_binding_pass() {
    let source = r#"
x = 1
y = x
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    assert!(!blockpy_module_has_root_name(
        &blockpy_module,
        "__dp_store_global"
    ));
    assert!(!blockpy_module_has_root_name(
        &blockpy_module,
        "__dp_load_global"
    ));
    assert!(blockpy_module_has_store_name(&blockpy_module, "x"));
    assert!(blockpy_module_has_store_name(&blockpy_module, "y"));

    let resolved_init = lowered.bb_function("_dp_module_init");
    assert!(resolved_function_uses_global(resolved_init, "x"));
    assert!(resolved_module_uses_global(lowered.bb_module(), "y"));
}

#[test]
fn top_level_global_named_expr_moves_to_name_binding_pass() {
    let source = r#"
def f():
    return 1

x = (y := f())
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    assert!(!blockpy_module_has_root_name(
        &blockpy_module,
        "__dp_store_global"
    ));
    assert!(!blockpy_module_has_root_name(
        &blockpy_module,
        "__dp_load_global"
    ));
    assert!(blockpy_module_has_store_name(&blockpy_module, "y"));
    assert!(blockpy_module_has_store_name(&blockpy_module, "x"));

    let resolved_init = lowered.bb_function("_dp_module_init");
    assert!(resolved_function_uses_global(resolved_init, "x"));
    assert!(resolved_module_uses_global(lowered.bb_module(), "y"));
}

#[test]
fn top_level_comprehension_named_expr_uses_global_decl_then_name_binding_pass() {
    let source = r#"
x = [y := i for i in [1, 2]]
"#;

    let lowered = TrackedLowering::new(source);
    let ast_probe = probe_rewritten_ast(source);
    assert!(ast_probe.global_names.contains("y"));
    assert!(!ast_probe.function_names.contains("__dp_store_global"));

    let blockpy_module = lowered.blockpy_module();
    assert!(!blockpy_module_has_root_name(
        &blockpy_module,
        "__dp_store_global"
    ));
    assert!(blockpy_module_has_store_name(&blockpy_module, "y"));

    let resolved_init = lowered.bb_function("_dp_module_init");
    assert!(resolved_function_uses_global(resolved_init, "x"));
    assert!(resolved_module_uses_global(lowered.bb_module(), "y"));
}

#[test]
fn top_level_for_target_global_binding_moves_to_name_binding_pass() {
    let source = r#"
for x in [1, 2]:
    pass
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    assert!(!blockpy_module_has_root_name(
        &blockpy_module,
        "__dp_store_global"
    ));
    assert!(blockpy_module_has_store_name(&blockpy_module, "x"));

    let resolved_init = lowered.bb_function("_dp_module_init");
    assert!(resolved_function_uses_global(resolved_init, "x"));
}

#[test]
fn top_level_except_name_global_binding_moves_to_name_binding_pass() {
    let source = r#"
try:
    raise Exception("boom")
except Exception as exc:
    seen = exc
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let init = blockpy_function_by_name(&blockpy_module, "_dp_module_init");
    assert!(!blockpy_function_has_root_name(init, "__dp_store_global"));
    assert!(blockpy_function_has_root_name(init, "del_quietly"));
    assert!(!blockpy_function_has_root_name(
        init,
        "__dp_current_exception"
    ));
    assert!(blockpy_function_has_store_name(init, "exc"));

    let resolved_init = lowered.bb_function("_dp_module_init");
    assert!(resolved_function_uses_global(resolved_init, "exc"));
    assert!(resolved_function_has_del(resolved_init, "exc", true));
}

#[test]
fn top_level_global_delete_moves_to_name_binding_pass() {
    let source = r#"
x = 1
del x
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let init = blockpy_function_by_name(&blockpy_module, "_dp_module_init");
    assert!(!blockpy_function_has_root_name(init, "__dp_delitem"));
    assert!(blockpy_function_has_del_name(init, "x"));

    let resolved_init = lowered.bb_function("_dp_module_init");
    assert!(resolved_function_has_del(resolved_init, "x", false));
}

#[test]
fn local_delete_stays_semantic_del_after_name_binding() {
    let source = r#"
def f():
    x = 1
    del x
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let f = blockpy_function_by_name(&blockpy_module, "f");
    assert!(blockpy_function_has_del_name(f, "x"));

    let resolved_f = lowered.bb_function("f");
    assert!(resolved_function_has_del(resolved_f, "x", false));
}

#[test]
fn dead_tail_local_binding_load_moves_to_name_binding_pass() {
    let source = r#"
def f():
    print(x)
    return
    x = 1
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let f = blockpy_function_by_name(&blockpy_module, "f");
    assert!(!blockpy_function_has_store_name(f, "x"));

    assert!(
        module_constant_text(lowered.bb_module()).contains("raise_deleted_name"),
        "{:?}\n{}",
        lowered.bb_function("f"),
        module_constant_text(lowered.bb_module())
    );
}

#[test]
fn nonlocal_delete_preserves_closure_capture_before_name_binding() {
    let source = r#"
def outer():
    x = 1
    def inner():
        nonlocal x
        del x
        return "ok"
    inner()
    return "done"
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let outer = blockpy_function_by_name(&blockpy_module, "outer");
    let inner = blockpy_function_by_name(&blockpy_module, "inner");
    assert_eq!(inner.names.qualname, "outer.<locals>.inner");
    assert!(blockpy_function_has_store_name(outer, "inner"));
    assert!(blockpy_function_has_del_name(inner, "x"));
}

#[test]
fn nonlocal_assign_and_load_move_to_name_binding_pass() {
    let source = r#"
def outer():
    x = 1
    def inner():
        nonlocal x
        x = x + 1
        return x
    return inner()
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let inner = blockpy_function_by_name(&blockpy_module, "inner");
    assert!(blockpy_function_has_store_name(inner, "x"));
    assert!(inner.blocks.iter().any(|block| matches!(
        &block.term,
        BlockTerm::Return(InstrWithAwaitAndYield::Load(load)) if load.name.id_str() == "x"
    )));
    assert!(!blockpy_function_has_root_name(inner, "__dp_store_cell"));
    assert!(!blockpy_function_has_root_name(inner, "__dp_load_cell"));

    let resolved_inner = lowered.bb_function("inner");
    assert!(resolved_function_has_store_to_captured_source(
        resolved_inner
    ));
    assert!(resolved_function_uses_captured_source(resolved_inner));
}

#[test]
fn owned_cell_init_preamble_moves_to_name_binding_pass() {
    let source = r#"
def outer(x):
    y = 1
    def inner():
        return x + y
    return inner
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let outer_semantic = blockpy_function_by_name(&blockpy_module, "outer");
    assert!(!blockpy_function_has_root_name(
        outer_semantic,
        "__dp_make_cell"
    ));
    let name_binding_module = lowered
        .result
        .pass_tracker
        .pass_name_binding()
        .expect("name_binding pass should be available");
    let outer = name_binding_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "outer")
        .expect("outer function should be present");
    assert!(resolved_function_has_make_cell(outer));
    let Some(InstrResolved::Store(assign)) = outer.entry_block().body.first() else {
        panic!("expected first entry stmt to be Expr(Store(...))");
    };
    assert!(
        matches!(&*assign.value, InstrResolved::MakeCell(_)),
        "{assign:?}"
    );
}

#[test]
fn rewritten_ruff_ast_can_keep_assert_while_stmt_sequence_still_lowers_it() {
    let source = r#"
def check():
    assert cond, msg
"#;

    let lowered = TrackedLowering::new(source);
    let check = lowered.bb_function("check");
    assert!(
        check
            .blocks
            .iter()
            .any(|block| matches!(block.term, crate::block_py::BlockTerm::IfTerm(_))),
        "{check:?}"
    );
}

#[test]
fn rewritten_ruff_ast_can_keep_elif_while_stmt_sequence_still_lowers_it() {
    let source = r#"
def check(a, b):
    if a:
        return 1
    elif b:
        return 2
    else:
        return 3
"#;

    let lowered = TrackedLowering::new(source);
    let check = lowered.bb_function("check");
    let brif_count = check
        .blocks
        .iter()
        .filter(|block| matches!(block.term, crate::block_py::BlockTerm::IfTerm(_)))
        .count();
    assert!(brif_count >= 2, "{check:?}");
}

#[test]
fn rewritten_ruff_ast_can_keep_boolop_while_blockpy_expr_lowering_handles_it() {
    let source = r#"
def choose(a, b, c):
    return f(a and b or c)
"#;

    let lowered = TrackedLowering::new(source);
    let choose = lowered.bb_function("choose");
    assert!(
        choose
            .blocks
            .iter()
            .any(|block| matches!(block.term, crate::block_py::BlockTerm::IfTerm(_))),
        "{choose:?}"
    );
}

#[test]
fn rewritten_ruff_ast_can_keep_if_expr_while_blockpy_expr_lowering_handles_it() {
    let source = r#"
def choose(cond, a, b):
    return f(a if cond else b)
"#;

    let lowered = TrackedLowering::new(source);
    let choose = lowered.bb_function("choose");
    assert!(
        choose
            .blocks
            .iter()
            .any(|block| matches!(block.term, crate::block_py::BlockTerm::IfTerm(_))),
        "{choose:?}"
    );
}

#[test]
fn rewritten_ruff_ast_can_keep_named_expr_while_blockpy_expr_lowering_handles_it() {
    let source = r#"
def choose(y):
    return f((x := y))
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let choose = blockpy_function_by_name(&blockpy_module, "choose");
    assert!(blockpy_function_has_store_name(choose, "x"));
    assert!(blockpy_function_has_root_name(choose, "f"));
    assert!(blockpy_function_has_root_name(choose, "x"));
}

#[test]
fn scoped_helper_expr_pass_lowers_listcomp_before_blockpy() {
    let source = r#"
def choose(xs):
    return f([x for x in xs])
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let listcomp = blockpy_module
        .callable_defs
        .iter()
        .find(|function| {
            function
                .names
                .qualname
                .starts_with("choose.<locals>._dp_listcomp_")
        })
        .unwrap_or_else(|| panic!("missing hidden listcomp; got {blockpy_module:?}"));
    let listcomp_bind_name = listcomp.names.bind_name.clone();
    let choose = blockpy_function_by_name(&blockpy_module, "choose");
    assert!(blockpy_function_has_root_name(choose, "f"));
    assert!(blockpy_function_has_root_name(choose, &listcomp_bind_name));
}

#[test]
fn scoped_helper_expr_pass_lowers_genexpr_before_blockpy() {
    let source = r#"
def choose(xs):
    return tuple(x for x in xs)
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let genexpr = blockpy_function_by_qualname(&blockpy_module, "choose.<locals>.<genexpr>");
    assert_eq!(genexpr.names.qualname, "choose.<locals>.<genexpr>");
    assert_eq!(genexpr.kind, FunctionKind::Generator);
    let choose = blockpy_function_by_name(&blockpy_module, "choose");
    assert!(blockpy_function_has_root_name(choose, "tuple"));
    assert!(blockpy_function_instr_any(choose, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::MakeFunction(_)
    )));
}

#[test]
fn module_plan_lowers_lambda_before_blockpy() {
    let source = r#"
def choose():
    return f(lambda x: x + 1)
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let lambda = blockpy_function_by_qualname(&blockpy_module, "choose.<locals>.<lambda>");
    assert_eq!(lambda.names.qualname, "choose.<locals>.<lambda>");
    let choose = blockpy_function_by_name(&blockpy_module, "choose");
    assert!(blockpy_function_has_root_name(choose, "f"));
    assert!(blockpy_function_instr_any(choose, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::MakeFunction(_)
    )));
}

#[test]
fn module_plan_lowers_lambda_in_function_decorator_before_blockpy() {
    let source = r#"
def keep(value):
    def decorator(func):
        return value
    return decorator

@keep(lambda: 42)
def chosen():
    return 0
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let lambda = blockpy_function_by_qualname(&blockpy_module, "<lambda>");
    assert_eq!(lambda.names.qualname, "<lambda>");
    let module_init = blockpy_function_by_name(&blockpy_module, "_dp_module_init");
    assert!(blockpy_function_has_root_name(module_init, "keep"));
    assert!(blockpy_function_instr_any(module_init, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::MakeFunction(_)
    )));
}

#[test]
fn rewritten_ruff_ast_can_keep_async_generator_await_while_blockpy_generator_lowering_handles_it() {
    let source = r#"
class Once:
    def __await__(self):
        yield 1
        return 2

async def agen():
    value = await Once()
    yield value
"#;

    let lowered = TrackedLowering::new(source);
    let semantic_blockpy = lowered.blockpy_module();
    let semantic_agen = blockpy_function_by_name(&semantic_blockpy, "agen");
    assert!(blockpy_function_instr_any(semantic_agen, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::Await(_)
    )));
    assert!(!blockpy_function_has_root_name(semantic_agen, "await_iter"));

    let core_with_yield = tracked_core_blockpy_with_yield_only(source);
    let lowered_agen = blockpy_function_by_name(&core_with_yield, "agen");
    assert!(blockpy_function_has_root_name(lowered_agen, "await_iter"));
}

#[test]
fn rewritten_ruff_ast_can_keep_coroutine_await_while_blockpy_generator_lowering_handles_it() {
    let source = r#"
class Once:
    def __await__(self):
        yield 1
        return 2

async def run():
    value = await Once()
    return value
"#;

    let lowered = TrackedLowering::new(source);
    let semantic_blockpy = lowered.blockpy_module();
    let semantic_run = blockpy_function_by_name(&semantic_blockpy, "run");
    assert!(blockpy_function_instr_any(semantic_run, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::Await(_)
    )));
    assert!(!blockpy_function_has_root_name(semantic_run, "await_iter"));

    let core_with_yield = tracked_core_blockpy_with_yield_only(source);
    let lowered_run = blockpy_function_by_name(&core_with_yield, "run");
    assert!(blockpy_function_has_root_name(lowered_run, "await_iter"));
}

#[test]
fn coroutine_closure_state_core_blockpy_with_yield_has_no_dangling_labels() {
    let source = r#"
class Once:
    def __await__(self):
        yielded = yield "tick"
        return yielded if yielded is not None else 5

def make_runner(delta):
    outer = delta

    async def run():
        total = 1
        total += outer
        total += await Once()
        return total

    return run()
"#;

    let blockpy = tracked_core_blockpy_with_yield_only(source);
    assert_all_targets_present(&blockpy);
}

#[test]
fn rewritten_ruff_ast_can_keep_async_generator_async_with_while_blockpy_generator_lowering_handles_it(
) {
    let source = r#"
async def agen(cm):
    async with cm as value:
        yield value
"#;

    let lowered = TrackedLowering::new(source);
    let semantic_blockpy = lowered.blockpy_module();
    let semantic_agen = blockpy_function_by_name(&semantic_blockpy, "agen");
    assert!(blockpy_function_has_root_name(
        semantic_agen,
        "asynccontextmanager_aenter"
    ));
    assert!(blockpy_function_has_root_name(
        semantic_agen,
        "asynccontextmanager_get_aexit"
    ));
    assert!(!blockpy_function_has_root_name(semantic_agen, "await_iter"));

    let core_with_yield = tracked_core_blockpy_with_yield_only(source);
    let lowered_agen = blockpy_function_by_name(&core_with_yield, "agen");
    assert!(blockpy_function_has_root_name(lowered_agen, "await_iter"));
    assert!(blockpy_function_has_root_name(
        lowered_agen,
        "asynccontextmanager_aenter"
    ));
}

#[test]
fn rewritten_ruff_ast_can_keep_coroutine_async_with_while_blockpy_generator_lowering_handles_it() {
    let source = r#"
async def run(cm):
    async with cm as value:
        return value
"#;

    let lowered = TrackedLowering::new(source);
    let semantic_blockpy = lowered.blockpy_module();
    let semantic_run = blockpy_function_by_name(&semantic_blockpy, "run");
    assert!(blockpy_function_has_root_name(
        semantic_run,
        "asynccontextmanager_aenter"
    ));
    assert!(blockpy_function_has_root_name(
        semantic_run,
        "asynccontextmanager_get_aexit"
    ));
    assert!(!blockpy_function_has_root_name(semantic_run, "await_iter"));

    let core_with_yield = tracked_core_blockpy_with_yield_only(source);
    let lowered_run = blockpy_function_by_name(&core_with_yield, "run");
    assert!(blockpy_function_has_root_name(lowered_run, "await_iter"));
    assert!(blockpy_function_has_root_name(
        lowered_run,
        "asynccontextmanager_aenter"
    ));
}

#[test]
fn rewritten_ruff_ast_can_keep_match_while_stmt_sequence_still_lowers_it() {
    let source = r#"
def check(x):
    match x:
        case 1:
            return 10
        case _:
            return 20
"#;

    let lowered = TrackedLowering::new(source);
    let check = lowered.bb_function("check");
    assert!(
        check
            .blocks
            .iter()
            .any(|block| matches!(block.term, crate::block_py::BlockTerm::IfTerm(_))),
        "{check:?}"
    );
}

#[test]
fn rewritten_ruff_ast_can_keep_raise_from_while_stmt_sequence_still_lowers_it() {
    let source = r#"
def check():
    raise ValueError() from None
"#;

    let lowered = TrackedLowering::new(source);
    let check = lowered.bb_function("check");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), check, "raise_from"),
        "{check:?}"
    );
    assert!(
        check
            .blocks
            .iter()
            .any(|block| { matches!(block.term, crate::block_py::BlockTerm::Raise(_)) }),
        "{check:?}"
    );
}

#[test]
fn rewritten_ruff_ast_can_keep_typed_try_while_later_passes_still_lower_it() {
    let source = r#"
def check():
    try:
        work()
    except ValueError as exc:
        handle(exc)
"#;

    let lowered = TrackedLowering::new(source);
    let check = lowered.bb_function("check");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), check, "exception_matches"),
        "{check:?}"
    );
    assert!(
        check.blocks.iter().any(|block| block.exc_edge.is_some()),
        "{check:?}"
    );
}

#[test]
fn rewritten_ruff_ast_can_keep_try_star_while_later_passes_still_lower_it() {
    let source = r#"
def check():
    try:
        work()
    except* ValueError as exc:
        handle(exc)
"#;

    let lowered = TrackedLowering::new(source);
    let check = lowered.bb_function("check");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), check, "exceptiongroup_split"),
        "{check:?}"
    );
}

#[test]
fn ast_to_ast_can_lower_import_while_later_passes_still_lower_it() {
    let source = r#"
import pkg.sub as alias
"#;

    let lowered = TrackedLowering::new(source);

    let module_init = lowered.bb_function("_dp_module_init");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), module_init, "import_"),
        "{module_init:?}"
    );
    assert!(
        function_or_constants_use_text(lowered.bb_module(), module_init, "import_attr"),
        "{module_init:?}"
    );
}

#[test]
fn ast_to_ast_can_lower_import_from_while_later_passes_still_lower_it() {
    let source = r#"
from pkg.mod import name as alias
"#;

    let lowered = TrackedLowering::new(source);

    let module_init = lowered.bb_function("_dp_module_init");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), module_init, "import_"),
        "{module_init:?}"
    );
    assert!(
        function_or_constants_use_text(lowered.bb_module(), module_init, "import_attr"),
        "{module_init:?}"
    );
}

#[test]
fn ast_to_ast_can_lower_type_alias_while_later_passes_still_lower_it() {
    let source = r#"
type Alias[T] = list[T]
"#;

    let lowered = TrackedLowering::new(source);

    let module_init = lowered.bb_function("_dp_module_init");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), module_init, "typing_TypeAliasType"),
        "{module_init:?}"
    );
}

#[test]
fn ast_to_ast_can_lower_augassign_while_later_passes_still_lower_it() {
    let source = r#"
def bump(x):
    x += 1
    return x
"#;

    let lowered = TrackedLowering::new(source);

    let bump = lowered.bb_function("bump");
    assert!(
        blockpy_function_instr_any(bump, |expr| matches!(
            expr,
            InstrResolved::BinOp(binop)
                if binop.kind == crate::block_py::BinOpKind::InplaceAdd
        )),
        "{bump:?}"
    );
}

#[test]
fn closure_backed_generator_records_explicit_storage_layout() {
    let source = r#"
def outer(scale):
    factor = scale
    def gen(a):
        total = a
        yield total + factor
        total = total + 1
        yield total
    return gen
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let gen = function_by_name(&bb_module, "gen");
    let layout = gen
        .storage_layout()
        .as_ref()
        .expect("sync generator should record closure layout");

    let factor = slot_by_name(&layout.freevars, "factor");
    assert_eq!(factor.storage_name, "factor");
    assert_eq!(factor.init, ClosureInit::InheritedCapture);

    let a = slot_by_name(&layout.freevars, "a");
    assert_eq!(a.storage_name, "a");
    assert_eq!(a.init, ClosureInit::InheritedCapture);

    let total = slot_by_name(&layout.freevars, "total");
    assert_eq!(total.storage_name, "total");
    assert_eq!(total.init, ClosureInit::InheritedCapture);

    let pc = slot_by_name(&layout.freevars, "_dp_pc");
    assert_eq!(pc.storage_name, "_dp_pc");
    assert_eq!(pc.init, ClosureInit::InheritedCapture);
    assert!(layout.cellvars.is_empty(), "{layout:?}");
    assert!(layout.runtime_cells.is_empty(), "{layout:?}");
}

#[test]
fn closure_backed_generator_layout_preserves_try_exception_slots() {
    let source = r#"
def gen():
    try:
        yield 1
    except ValueError:
        return 2
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let gen = function_by_name(&bb_module, "gen");
    let layout = gen
        .storage_layout()
        .as_ref()
        .expect("sync generator should record closure layout");

    let try_exc = layout
        .freevars
        .iter()
        .find(|slot| slot.logical_name.starts_with("_dp_try_exc_"))
        .unwrap_or_else(|| panic!("missing try-exception slot in {layout:?}"));
    assert_eq!(try_exc.storage_name, try_exc.logical_name);
    assert_eq!(try_exc.init, ClosureInit::InheritedCapture);
    assert!(
        layout
            .freevars
            .iter()
            .any(|slot| slot.logical_name == "_dp_pc"),
        "{layout:?}"
    );
    assert!(layout.cellvars.is_empty(), "{layout:?}");
    assert!(layout.runtime_cells.is_empty(), "{layout:?}");
}

#[test]
fn closure_backed_coroutine_records_explicit_storage_layout() {
    let source = r#"
class Once:
    def __await__(self):
        yield 1
        return 2

def outer(scale):
    factor = scale
    async def run():
        total = 1
        total += factor
        total += await Once()
        return total
    return run
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let run = function_by_name(&bb_module, "run");
    let layout = run
        .storage_layout()
        .as_ref()
        .expect("closure-backed coroutine should record closure layout");

    let factor = slot_by_name(&layout.freevars, "factor");
    assert_eq!(factor.storage_name, "factor");
    assert_eq!(factor.init, ClosureInit::InheritedCapture);

    let total = slot_by_name(&layout.freevars, "total");
    assert_eq!(total.storage_name, "total");
    assert_eq!(total.init, ClosureInit::InheritedCapture);

    let pc = slot_by_name(&layout.freevars, "_dp_pc");
    assert_eq!(pc.storage_name, "_dp_pc");
    assert_eq!(pc.init, ClosureInit::InheritedCapture);
    assert!(layout.cellvars.is_empty(), "{layout:?}");
    assert!(layout.runtime_cells.is_empty(), "{layout:?}");
}

#[test]
fn closure_backed_coroutine_propagates_nested_coroutine_nonlocal_capture() {
    let source = r#"
def outer():
    cancelled = False
    class Test:
        async def test_leaking_task(self):
            async def coro():
                nonlocal cancelled
                cancelled = True
            await coro()
    return Test
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let method = function_by_name(&bb_module, "test_leaking_task");
    let layout = method
        .storage_layout()
        .as_ref()
        .expect("closure-backed coroutine should record closure layout");

    assert!(
        layout
            .freevars
            .iter()
            .any(|slot| slot.logical_name == "cancelled"),
        "visible coroutine should capture nonlocals needed by nested coroutine construction: {layout:#?}"
    );
}

#[test]
fn closure_backed_coroutine_global_store_remains_module_global() {
    let source = r#"
flag = False

async def set_flag():
    global flag
    flag = True
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let resume = function_by_name(&bb_module, "set_flag");
    let layout = resume
        .storage_layout()
        .as_ref()
        .expect("coroutine resume should record closure layout");

    assert!(
        !layout
            .freevars
            .iter()
            .chain(layout.cellvars.iter())
            .chain(layout.runtime_cells.iter())
            .any(|slot| slot.logical_name == "flag"),
        "global assignment target should not become coroutine state: {layout:#?}"
    );
    assert!(
        resume
            .blocks
            .iter()
            .flat_map(|block| &block.body)
            .any(|stmt| {
                matches!(
                    stmt,
                    InstrResolved::Store(store)
                        if store.name.id_str() == "flag"
                            && (store.name.location.is_global_name()
                                || store.name.location.is_global())
                )
            }),
        "coroutine resume should store flag through module-global storage: {resume:#?}"
    );
}

#[test]
fn closure_backed_async_generator_records_explicit_storage_layout() {
    let source = r#"
def outer(scale):
    factor = scale
    async def agen():
        total = 1
        yield total + factor
        total += 1
        yield total + factor
    return agen
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let agen = function_by_name(&bb_module, "agen");
    let layout = agen
        .storage_layout()
        .as_ref()
        .expect("closure-backed async generator should record closure layout");

    let factor = slot_by_name(&layout.freevars, "factor");
    assert_eq!(factor.storage_name, "factor");
    assert_eq!(factor.init, ClosureInit::InheritedCapture);

    let total = slot_by_name(&layout.freevars, "total");
    assert_eq!(total.storage_name, "total");
    assert_eq!(total.init, ClosureInit::InheritedCapture);

    let pc = slot_by_name(&layout.freevars, "_dp_pc");
    assert_eq!(pc.storage_name, "_dp_pc");
    assert_eq!(pc.init, ClosureInit::InheritedCapture);
    assert!(layout.cellvars.is_empty(), "{layout:?}");
    assert!(layout.runtime_cells.is_empty(), "{layout:?}");
}

#[test]
fn async_comprehension_lowering_emits_only_closure_backed_generator_callables() {
    let source = r#"
import asyncio

async def agen():
    for i in range(4):
        await asyncio.sleep(0)
        yield i

async def outer(scale):
    values = [x + scale async for x in agen()]
    return (value * 2 async for value in agen() if value in values)
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let generator_callables = bb_module
        .callable_defs
        .iter()
        .filter(|func| {
            matches!(
                func.lowered_kind(),
                FunctionKind::Generator | FunctionKind::AsyncGenerator
            )
        })
        .collect::<Vec<_>>();
    let generator_names = generator_callables
        .iter()
        .map(|func| format!("{} :: {}", func.names.bind_name, func.names.qualname))
        .collect::<Vec<_>>();
    assert!(
        !generator_callables.is_empty(),
        "expected generator-like BB callables; got {}",
        bb_module
            .callable_defs
            .iter()
            .map(|func| format!("{} :: {}", func.names.bind_name, func.names.qualname))
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert!(
        generator_callables
            .iter()
            .all(|func| func.storage_layout().is_some()),
        "expected only closure-backed generator callables; got {}",
        generator_names.join(", ")
    );
}

#[test]
fn lowers_while_break_continue_into_basic_blocks() {
    let source = r#"
def run(limit):
    i = 0
    out = []
    while i < limit:
        i = i + 1
        if i == 2:
            continue
        if i == 5:
            break
        out.append(i)
    else:
        out.append(99)
    return out, i
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let run = function_by_name(&bb_module, "run");
    assert!(
        run.blocks
            .iter()
            .any(|block| matches!(block.term, BlockTerm::IfTerm(_))),
        "{run:?}"
    );
    assert!(
        run.blocks
            .iter()
            .any(|block| matches!(block.term, BlockTerm::Jump(_))),
        "{run:?}"
    );
}

#[test]
fn lowers_for_else_break_into_basic_blocks() {
    let source = r#"
def run(items):
    out = []
    for x in items:
        if x == 2:
            break
        out.append(x)
    else:
        out.append(99)
    return out
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let run = function_by_name(&bb_module, "run");
    assert!(
        function_or_constants_use_text(&bb_module, run, "next_or_sentinel"),
        "{run:?}"
    );
    assert!(
        function_or_constants_use_text(&bb_module, run, "ITER_COMPLETE"),
        "{run:?}"
    );
    assert!(
        !function_or_constants_use_text(&bb_module, run, "StopIteration"),
        "{run:?}"
    );
    assert!(
        function_or_constants_use_text(&bb_module, run, "iter"),
        "{run:?}"
    );
    assert!(
        run.blocks
            .iter()
            .any(|block| matches!(block.term, BlockTerm::IfTerm(_))),
        "{run:?}"
    );
}

#[test]
fn lowers_async_for_else_directly_without_completed_flag() {
    let source = r#"
async def run():
    async for x in ait:
        body()
    else:
        done()
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let run = function_by_name(&bb_module, "run");
    assert!(
        function_or_constants_use_text(&bb_module, run, "anext_or_sentinel"),
        "{run:?}"
    );
    assert!(
        function_or_constants_use_text(&bb_module, run, "aiter"),
        "{run:?}"
    );
    assert!(!blockpy_function_has_root_name(run, "_dp_completed_"));
}

#[test]
fn semantic_blockpy_lowers_async_for_to_awaited_fetch_before_await_lowering() {
    let source = r#"
async def run():
    async for x in ait:
        body()
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_module = lowered.blockpy_module();
    let run = blockpy_function_by_name(&blockpy_module, "run");
    assert!(blockpy_function_instr_any(run, |expr| matches!(
        expr,
        InstrWithAwaitAndYield::Await(_)
    )));
    assert!(blockpy_function_has_root_name(run, "anext_or_sentinel"));
    assert!(blockpy_function_has_root_name(run, "aiter"));
    assert!(!blockpy_function_has_root_name(run, "await_iter"));
}

#[test]
fn omits_synthetic_end_block_when_unreachable() {
    let source = r#"
def f():
    return 1
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let f = function_by_name(&bb_module, "f");
    assert_eq!(
        f.entry_block().label_str(),
        f.blocks
            .first()
            .expect("f should have a first block")
            .label_str()
    );
    assert_ne!(f.entry_block().label_str(), "start", "{f:?}");
}

#[test]
fn folds_jump_to_trivial_none_return() {
    let source = r#"
def f():
    x = 1
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let f = function_by_name(&bb_module, "f");
    assert!(
        function_or_constants_use_text(&bb_module, f, "NONE"),
        "{f:?}"
    );
    assert!(
        !f.blocks
            .iter()
            .any(|block| matches!(block.term, BlockTerm::Jump(_))),
        "{f:?}"
    );
}

#[test]
fn debug_generator_filter_source_order_ir() {
    let pass_source = r#"
class Field:
    def __init__(self, name, *, init, kw_only=False):
        self.name = name
        self.init = init
        self.kw_only = kw_only

def fields_in_init_order(fields):
    return tuple(
        field.name
        for field in fields
        if field.init and not field.kw_only
    )
"#;
    let fail_source = r#"
def fields_in_init_order(fields):
    return tuple(
        field.name
        for field in fields
        if field.init and not field.kw_only
    )

class Field:
    def __init__(self, name, *, init, kw_only=False):
        self.name = name
        self.init = init
        self.kw_only = kw_only
"#;

    for (name, source) in [("pass", pass_source), ("fail", fail_source)] {
        let lowered =
            lower_python_to_blockpy_for_testing(source).expect("transform should succeed");
        let blockpy = lowered
            .pass_tracker
            .pass_core_blockpy_with_await_and_yield()
            .cloned()
            .expect("expected lowered core BlockPy module");
        let blockpy_rendered = crate::block_py::blockpy_module_to_string(&blockpy);
        eprintln!("==== {name} BLOCKPY ====\n{blockpy_rendered}");

        let bb_module = tracked_name_binding_module(source)
            .expect("transform should succeed")
            .expect("bb module should be available");
        let function_names = bb_module
            .callable_defs
            .iter()
            .map(|func| format!("{} :: {}", func.names.bind_name, func.names.qualname))
            .collect::<Vec<_>>();
        eprintln!(
            "==== {name} BB FUNCTIONS ====\n{}",
            function_names.join("\n")
        );
        let gen = bb_module
            .callable_defs
            .iter()
            .find(|func| func.names.bind_name.contains("_dp_genexpr"))
            .unwrap_or_else(|| panic!("missing genexpr helper in {name}"));
        eprintln!("==== {name} BB {:?} ====\n{gen:#?}", gen.names.qualname);

        let prepared = crate::passes::lower_try_jump_exception_flow(&bb_module);
        let prepared_gen = prepared
            .callable_defs
            .iter()
            .find(|func| func.names.bind_name.contains("_dp_genexpr"))
            .unwrap_or_else(|| panic!("missing prepared genexpr helper in {name}"));
        eprintln!("==== {name} PREPARED ====\n{prepared_gen:#?}");
    }
}

#[test]
fn closure_backed_simple_generator_records_one_resume_per_yield() {
    let source = r#"
def make_counter(delta):
    outer_capture = delta
    def gen():
        total = 1
        total += outer_capture
        sent = yield total
        total += sent
        yield total
    return gen()
"#;

    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let gen = bb_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "gen")
        .expect("missing visible generator factory");
    assert_eq!(gen.lowered_kind(), &FunctionKind::Generator);
}

#[test]
fn closure_backed_simple_generator_preserves_outer_capture_on_visible_factory() {
    let source = r#"
def make_counter(delta):
    outer_capture = delta
    def gen():
        total = 1
        total += outer_capture
        sent = yield total
        total += sent
        yield total
    return gen()
"#;

    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let gen = function_by_name(&bb_module, "gen");
    let layout = gen
        .storage_layout
        .as_ref()
        .expect("visible generator should have a storage layout");
    assert!(
        layout
            .freevars
            .iter()
            .any(|slot| slot.logical_name == "outer_capture"),
        "visible generator should still capture outer_capture for resume closure materialization: {layout:#?}"
    );
}

#[test]
fn closure_backed_simple_generator_resume_make_function_captures_all_resume_freevars() {
    let source = r#"
def make_counter(delta):
    outer_capture = delta
    def gen():
        total = 1
        total += outer_capture
        sent = yield total
        total += sent
        yield total
    return gen()
"#;

    let lowering = TrackedLowering::new(source);
    let bb_module = lowering.bb_module();
    let visible_gen = bb_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "gen")
        .expect("missing visible generator factory");
    let resume = bb_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "gen_resume")
        .expect("missing synthetic resume function");
    let resume_layout = resume
        .storage_layout
        .as_ref()
        .expect("resume function should have a storage layout");
    let BlockTerm::Return(return_expr) = &visible_gen.blocks[0].term else {
        panic!("visible generator factory should return a generator wrapper");
    };
    let closure_generator = runtime_call_by_name(bb_module, return_expr, "ClosureGenerator")
        .expect("expected ClosureGenerator");
    let resume_expr = closure_generator
        .keywords
        .iter()
        .find_map(|keyword| match keyword {
            CallArgKeyword::Named { arg, value } if arg.as_str() == "resume" => Some(value),
            _ => None,
        })
        .expect("ClosureGenerator should carry a resume= keyword");
    let captures_expr = match resume_expr {
        InstrResolved::MakeFunctionWithClosure(make_function) => make_function.captures.as_ref(),
        other => panic!("resume should use MakeFunctionWithClosure, got {other:?}"),
    };
    let InstrResolved::Tuple(captures_tuple) = captures_expr else {
        panic!("captures should be Tuple, got {captures_expr:?}");
    };
    assert_eq!(
        captures_tuple.values.len(),
        resume_layout.freevars.len(),
        "visible generator should materialize one closure capture per resume freevar:\n{}",
        lowering.name_binding_text(),
    );
    assert!(
        resume_layout
            .freevars
            .iter()
            .any(|slot| slot.logical_name == "_dp_pc" && slot.storage_name == "_dp_pc"),
        "resume layout should derive runtime state captures from scope bindings:\n{}",
        lowering.name_binding_text(),
    );
    assert!(
        resume_layout.freevars.iter().any(
            |slot| slot.logical_name == "_dp_yieldfrom" && slot.storage_name == "_dp_yieldfrom"
        ),
        "resume layout should keep logical storage names for runtime captures:\n{}",
        lowering.name_binding_text(),
    );
}

#[test]
fn generator_resume_inherited_capture_is_not_rewritten_as_always_unbound() {
    let source = r#"
def code_template_gen(_it):
    while True:
        yield next(_it)
"#;

    let lowering = TrackedLowering::new(source);
    let bb_module = lowering.bb_module();
    let resume = bb_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "code_template_gen_resume")
        .expect("missing synthetic resume function");

    assert!(
        resolved_function_uses_captured_source(resume),
        "resume should load _it from its inherited closure capture:\n{}",
        lowering.name_binding_text()
    );
    assert!(
        !function_or_constants_use_text(bb_module, resume, "raise_deleted_name"),
        "inherited closure captures are not statically unbound locals:\n{}",
        lowering.name_binding_text()
    );
}

#[test]
fn lowers_outer_with_nested_nonlocal_inner() {
    let source = r#"
def outer():
    x = 5
    def inner():
        nonlocal x
        x = 2
        return x
    return inner()
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let outer = function_by_name(&bb_module, "outer");
    let inner = function_by_name(&bb_module, "inner");
    assert_eq!(
        outer.entry_block().label_str(),
        outer
            .blocks
            .first()
            .expect("outer should have a first block")
            .label_str()
    );
    assert_eq!(
        inner.entry_block().label_str(),
        inner
            .blocks
            .first()
            .expect("inner should have a first block")
            .label_str()
    );
    assert_ne!(outer.entry_block().label_str(), "start", "{outer:?}");
    assert_ne!(inner.entry_block().label_str(), "start", "{inner:?}");
    assert!(
        slot_by_name(
            &outer
                .storage_layout()
                .as_ref()
                .expect("outer should have closure layout")
                .cellvars,
            "x",
        )
        .storage_name
            == "_dp_cell_x",
        "{outer:?}"
    );
}

#[test]
fn lowers_try_finally_with_return_via_dispatch() {
    let source = r#"
def f(x):
    try:
        if x:
            return 1
    finally:
        cleanup()
    return 2
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let f = function_by_name(&bb_module, "f");
    assert!(
        f.blocks.iter().any(|block| block.exc_edge.is_some()),
        "{f:?}"
    );
    let storage_layout = f
        .storage_layout()
        .as_ref()
        .expect("function with try/finally dispatch should have storage layout");
    let stack_slots = storage_layout.stack_slots();
    assert!(!stack_slots.iter().any(|name| name == "_dp_try_reason_"));
    assert!(!stack_slots.iter().any(|name| name == "_dp_try_value_"));
}

#[test]
fn lowers_nested_with_cleanup_and_inner_return_without_hanging() {
    let source = r#"
from pathlib import Path
import tempfile

def run():
    with tempfile.TemporaryDirectory() as temp_dir:
        path = Path(temp_dir) / "example.txt"
        with open(path, "w", encoding="utf8") as handle:
            handle.write("payload")
        with open(path, "r", encoding="utf8") as handle:
            return "ok"
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let run = function_by_name(&bb_module, "run");
    assert!(
        run.blocks.iter().any(|block| block.exc_edge.is_some()),
        "{run:?}"
    );
}

#[test]
fn lowers_plain_try_except_with_try_jump_dispatch() {
    let source = r#"
try:
    print(1)
except Exception:
    print(2)
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let init_fn = function_by_name(&bb_module, "_dp_module_init");
    assert!(
        init_fn.blocks.iter().any(|block| block.exc_edge.is_some()),
        "{init_fn:?}"
    );
}

#[test]
fn module_init_rebinds_lowered_top_level_function_defs() {
    let source = r#"
def outer_read():
    x = 5

    def inner():
        return x

    return inner
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let init_fn = function_by_name(&bb_module, "_dp_module_init");
    assert!(
        init_fn
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "StoreLocation(")),
        "{init_fn:?}"
    );
    assert!(
        init_fn
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "outer_read@g")),
        "{init_fn:?}"
    );
}

#[test]
fn ast_to_ast_module_init_does_not_inject_global_prelude() {
    let source = r#"
VALUE = 1

def build():
    return VALUE

class Box:
    item = VALUE
"#;

    let ast_probe = probe_rewritten_ast(source);
    assert!(ast_probe.function_names.contains("_dp_module_init"));
    assert!(!ast_probe.global_names.contains("VALUE"));
    assert!(!ast_probe.global_names.contains("build"));
    assert!(!ast_probe.global_names.contains("Box"));
}

#[test]
fn module_init_rebinds_top_level_assignments_and_classes_without_global_prelude() {
    let source = r#"
VALUE = 1

class Box:
    item = VALUE
"#;

    let lowered = TrackedLowering::new(source);
    let ast_probe = probe_rewritten_ast(source);
    assert!(!ast_probe.global_names.contains("VALUE"));
    assert!(!ast_probe.global_names.contains("Box"));

    let init_fn = lowered.bb_function("_dp_module_init");
    assert!(
        init_fn
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "StoreLocation(")),
        "{init_fn:?}"
    );
    assert!(
        init_fn
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "VALUE@g")),
        "{init_fn:?}"
    );
    assert!(
        init_fn
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "Box@g")),
        "{init_fn:?}"
    );
}

#[test]
fn lowers_try_star_except_star_via_exceptiongroup_split() {
    let source = r#"
def f():
    try:
        raise ExceptionGroup("eg", [ValueError(1)])
    except* ValueError as exc:
        return exc
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let f = function_by_name(&bb_module, "f");
    assert!(
        function_or_constants_use_text(&bb_module, f, "exceptiongroup_split"),
        "{f:?}"
    );
    assert!(
        f.blocks.iter().any(|block| block.exc_edge.is_some()),
        "{f:?}"
    );
}

#[test]
fn dead_tail_local_binding_still_raises_unbound() {
    let source = r#"
def f():
    print(x)
    return
    x = 1
"#;
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let f = function_by_name(&bb_module, "f");
    let debug = format!("{f:?}");
    assert!(
        module_constant_text(&bb_module).contains("raise_deleted_name"),
        "{}\n{}",
        debug,
        module_constant_text(&bb_module)
    );
    assert!(!blockpy_function_has_defined_name(f, "x"));
}
