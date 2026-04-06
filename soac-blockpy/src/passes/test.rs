use crate::block_py::{BindingKind, ClosureInit, ClosureSlot};
use crate::block_py::{
    BlockPyFunction, BlockPyModule, BlockPyNameLike, BlockTerm, Call, CallArgKeyword,
    CallArgPositional, CallableScopeKind, CellBindingKind, InstrLow, InstrResolved,
    FunctionKind, ResolvedName, NameLocation, ResolvedStorageBlock,
};
use crate::passes::{CoreBlockPyPassWithAwaitAndYield, InstrRuff, ResolvedStorageBlockPyPass};
use crate::{lower_python_to_blockpy_for_testing, LoweringResult};
use ruff_python_ast::{self as ast, Expr};

fn tracked_core_blockpy_with_await_and_yield(
    source: &str,
) -> BlockPyModule<CoreBlockPyPassWithAwaitAndYield> {
    lower_python_to_blockpy_for_testing(source)
        .expect("transform should succeed")
        .pass_tracker
        .pass_core_blockpy_with_await_and_yield()
        .expect("core_blockpy_with_await_and_yield pass should be tracked")
        .clone()
}

fn tracked_name_binding_module(
    source: &str,
) -> anyhow::Result<Option<BlockPyModule<ResolvedStorageBlockPyPass>>> {
    Ok(lower_python_to_blockpy_for_testing(source)?
        .pass_tracker
        .pass_name_binding()
        .cloned())
}

struct TrackedLowering {
    result: LoweringResult,
    blockpy_module: BlockPyModule<CoreBlockPyPassWithAwaitAndYield>,
}

impl TrackedLowering {
    fn new(source: &str) -> Self {
        let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
        Self {
            result: lower_python_to_blockpy_for_testing(source).expect("transform should succeed"),
            blockpy_module,
        }
    }

    fn blockpy_module(&self) -> BlockPyModule<CoreBlockPyPassWithAwaitAndYield> {
        self.blockpy_module.clone()
    }

    fn blockpy_text(&self) -> String {
        crate::block_py::pretty::blockpy_module_to_string(&self.blockpy_module())
    }

    fn core_blockpy_with_yield_text(&self) -> String {
        self.pass_text("core_blockpy_with_yield")
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

    fn bb_module(&self) -> &BlockPyModule<ResolvedStorageBlockPyPass> {
        self.result
            .pass_tracker
            .pass_name_binding()
            .expect("bb module should be available")
    }

    fn bb_function(&self, bind_name: &str) -> &BlockPyFunction<ResolvedStorageBlockPyPass> {
        function_by_name(self.bb_module(), bind_name)
    }
}

fn function_by_name<'a>(
    bb_module: &'a BlockPyModule<ResolvedStorageBlockPyPass>,
    bind_name: &str,
) -> &'a BlockPyFunction<ResolvedStorageBlockPyPass> {
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

fn expr_text<N: BlockPyNameLike>(expr: &InstrLow<N>) -> String {
    crate::block_py::pretty::bb_expr_text(expr)
}

fn callable_def_by_name<'a>(
    blockpy_module: &'a BlockPyModule<CoreBlockPyPassWithAwaitAndYield>,
    bind_name: &str,
) -> &'a BlockPyFunction<CoreBlockPyPassWithAwaitAndYield> {
    blockpy_module
        .callable_defs
        .iter()
        .find(|callable| callable.names.bind_name == bind_name)
        .unwrap_or_else(|| {
            panic!("missing callable definition {bind_name}; got {blockpy_module:?}")
        })
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

fn count_occurrences(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

#[test]
fn instr_ruff_from_ast_expr_normalizes_bare_yield_to_explicit_none() {
    let instr = InstrRuff::from_ast_expr(Expr::Yield(ast::ExprYield {
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
    let instr = InstrRuff::from_ast_expr(crate::py_expr!("f(x, *args, y=z, **kw)"));

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

fn function_uses_text(
    function: &BlockPyFunction<ResolvedStorageBlockPyPass>,
    needle: &str,
) -> bool {
    function
        .blocks
        .iter()
        .any(|block| block_uses_text(block, needle))
}

fn module_constant_text(module: &BlockPyModule<ResolvedStorageBlockPyPass>) -> String {
    module
        .module_constants
        .iter()
        .map(expr_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn function_or_constants_use_text(
    module: &BlockPyModule<ResolvedStorageBlockPyPass>,
    function: &BlockPyFunction<ResolvedStorageBlockPyPass>,
    needle: &str,
) -> bool {
    function_uses_text(function, needle) || module_constant_text(module).contains(needle)
}

fn runtime_call_by_name<'a>(
    module: &'a BlockPyModule<ResolvedStorageBlockPyPass>,
    expr: &'a InstrLow<ResolvedName>,
    name: &str,
) -> Option<&'a Call<InstrLow<ResolvedName>>> {
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

#[test]
fn core_blockpy_with_await_keeps_plain_coroutines_without_fake_yield_marker() {
    let source = r#"
async def foo():
    return 1

async def classify():
    return await foo()
"#;

    let lowered = TrackedLowering::new(source);
    let rendered = lowered.blockpy_text();
    assert!(rendered.contains("coroutine classify():"), "{rendered}");
    assert!(rendered.contains("await foo()"), "{rendered}");
    assert!(!rendered.contains("yield __dp_NONE"), "{rendered}");
}

#[test]
fn core_blockpy_lowers_fstring_before_bb_lowering() {
    let source = r#"
def fmt(value):
    return f"{value=}"
"#;

    let lowered = TrackedLowering::new(source);
    let core_blockpy = lowered.blockpy_text();
    assert!(core_blockpy.contains("\"value=\""), "{core_blockpy}");
    assert!(core_blockpy.contains("repr(value)"), "{core_blockpy}");
    assert!(core_blockpy.contains("format("), "{core_blockpy}");

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
    let core_blockpy = lowered.blockpy_text();
    assert!(
        core_blockpy.contains("templatelib_Interpolation(value, \"value\","),
        "{core_blockpy}"
    );

    let fmt = lowered.bb_function("fmt");
    assert!(
        function_or_constants_use_text(lowered.bb_module(), fmt, "templatelib_Interpolation"),
        "{fmt:?}"
    );
    let constant_text = module_constant_text(lowered.bb_module());
    assert!(constant_text.contains("\"value\""), "{constant_text}");
    assert!(constant_text.contains("\"\""), "{constant_text}");
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
fn name_binding_lifts_runtime_helper_loads_into_module_constants() {
    let source = r#"
def f():
    pass
"#;

    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let make_function_index = bb_module
        .module_constants
        .iter()
        .position(|expr| {
            matches!(
                expr,
                InstrResolved::Load(load)
                    if load.name.is_runtime_name()
                        && load.name.id_str() == "make_function"
                        && load.name.location == NameLocation::RuntimeName
            )
        })
        .unwrap_or_else(|| panic!("expected lifted make_function runtime load, got {bb_module:?}"));
    let runtime_make_function = &bb_module.module_constants[make_function_index];
    let InstrResolved::Load(load) = runtime_make_function else {
        unreachable!();
    };
    assert_eq!(load.name.location, NameLocation::RuntimeName, "{load:?}");

    let module_init = function_by_name(&bb_module, "_dp_module_init");
    let rendered_init = module_init
        .blocks
        .iter()
        .flat_map(|block| block.body.iter().map(expr_text))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered_init.contains(format!("constant slot {make_function_index}(").as_str()),
        "expected module init to call through the extracted runtime constant slot:\n{rendered_init}"
    );
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("class_lookup_global"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        function_or_constants_use_text(
            lowered.bb_module(),
            lowered.bb_function("_dp_class_ns_Box"),
            "class_lookup_global",
        ),
        "{}\n{}",
        name_binding_rendered,
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("class_lookup_cell"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        !name_binding_rendered.contains("class_lookup_cell("),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("SetItem(LocalLocation(0), constant slot")
            && name_binding_rendered.contains("CapturedSource(0)"),
        "{name_binding_rendered}"
    );
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
    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("SetItem(LocalLocation(0), constant slot")
            && name_binding_rendered.contains("CapturedSource(0)"),
        "{name_binding_rendered}"
    );
}

#[test]
fn class_body_function_binding_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    def f(self):
        return 1
"#;

    let lowered = TrackedLowering::new(source);
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered.contains("StoreName(\"f\", MakeFunction"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("__dp_setitem(_dp_class_ns, \"f\","),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("SetItem(LocalLocation(0),")
            && module_constant_text(lowered.bb_module()).contains("make_function"),
        "{}\n{}",
        name_binding_rendered,
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered.contains("StoreName(\"x\", 1)"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("__dp_store_cell(_dp_cell_x, 1)"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("StoreLocation(CapturedSource(")
            && name_binding_rendered.contains("constant slot"),
        "{name_binding_rendered}"
    );
}

#[test]
fn class_body_local_assignment_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    x = 1
"#;

    let lowered = TrackedLowering::new(source);
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered.contains("StoreName(\"x\", 1)"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("__dp_setitem(_dp_class_ns, \"x\", 1)"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("SetItem(LocalLocation(0), constant slot"),
        "{name_binding_rendered}"
    );
}

#[test]
fn class_body_delete_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    x = 1
    del x
"#;

    let lowered = TrackedLowering::new(source);
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered.contains("Del {") && core_rendered.contains("quietly: false"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("__dp_delitem(_dp_class_ns, \"x\")"),
        "{core_rendered}"
    );
    assert!(!core_rendered.contains("__dp_DELETED"), "{core_rendered}");

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("DelItem(LocalLocation(0),")
            && name_binding_rendered.contains("constant slot"),
        "{name_binding_rendered}"
    );
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered.contains("Del {") && core_rendered.contains("quietly: false"),
        "{core_rendered}"
    );
    assert!(!core_rendered.contains("cell_contents"), "{core_rendered}");
    assert!(!core_rendered.contains("__dp_DELETED"), "{core_rendered}");

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("Del {")
            && name_binding_rendered.contains("CapturedSource(")
            && name_binding_rendered.contains("quietly: false"),
        "{name_binding_rendered}"
    );
}

#[test]
fn method_dunder_class_load_moves_to_name_binding_pass() {
    let source = concat!(
        "class C:\n",
        "    def f(self):\n",
        "        return __class__\n",
    );

    let lowered = TrackedLowering::new(source);
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered.contains("return __class__"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("_dp_classcell.cell_contents"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("return CapturedSource("),
        "{name_binding_rendered}"
    );
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
    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("function C.f.<locals>.g():"),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("return CapturedSource("),
        "{name_binding_rendered}"
    );
    assert!(
        module_constant_text(lowered.bb_module()).contains("make_function")
            && name_binding_rendered.contains("constant slot")
            && name_binding_rendered.contains("CellRef(Owned(0))"),
        "{}\n{}",
        name_binding_rendered,
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered.contains("CellRefForName(\"__class__\")"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("call_super(super,") && core_rendered.contains(", self)"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("call_super(super, _dp_classcell"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("CellRef(CapturedSource("),
        "{name_binding_rendered}"
    );
    assert!(
        module_constant_text(lowered.bb_module()).contains("call_super")
            && name_binding_rendered.contains(", LocalLocation(0))"),
        "{}\n{}",
        name_binding_rendered,
        module_constant_text(lowered.bb_module())
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
    let semantic_rendered = lowered.blockpy_text();
    assert!(
        !semantic_rendered.contains("__dp_store_global(__dp_globals(), \"caught\""),
        "{semantic_rendered}"
    );
    assert!(
        !semantic_rendered.contains("__dp_current_exception()"),
        "{semantic_rendered}"
    );
    assert!(
        semantic_rendered.contains("StoreName(\"caught\", _dp_try_exc_"),
        "{semantic_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("caught@g")
            && name_binding_rendered.contains("LocalLocation("),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("Del {") && name_binding_rendered.contains("quietly: true"),
        "{name_binding_rendered}"
    );
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
    let semantic_rendered = lowered.blockpy_text();
    assert!(
        !semantic_rendered.contains("__dp_store_cell(_dp_cell_x, __dp_current_exception())"),
        "{semantic_rendered}"
    );
    assert!(
        !semantic_rendered.contains("__dp_current_exception()"),
        "{semantic_rendered}"
    );
    assert!(
        semantic_rendered.contains("StoreName(\"x\", _dp_try_exc_"),
        "{semantic_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("StoreLocation(CapturedSource(")
            && name_binding_rendered.contains("LocalLocation("),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("Del {")
            && name_binding_rendered.contains("CapturedSource(")
            && name_binding_rendered.contains("quietly: true"),
        "{name_binding_rendered}"
    );
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
    let semantic_rendered = lowered.blockpy_text();
    assert!(
        !semantic_rendered
            .contains("__dp_setitem(_dp_class_ns, \"caught\", __dp_current_exception())"),
        "{semantic_rendered}"
    );
    assert!(
        !semantic_rendered.contains("__dp_current_exception()"),
        "{semantic_rendered}"
    );
    assert!(
        semantic_rendered.contains("StoreName(\"caught\", _dp_try_exc_"),
        "{semantic_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("SetItem(LocalLocation(0), constant slot"),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("DelItem(LocalLocation(0),")
            && name_binding_rendered.contains("constant slot"),
        "{name_binding_rendered}"
    );
}

#[test]
fn class_body_global_named_expr_moves_to_name_binding_pass() {
    let source = r#"
class Box:
    global y
    x = (y := 1)
"#;

    let lowered = TrackedLowering::new(source);
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_store_global(__dp_globals(), \"y\""),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"y\", 1)"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("y@g") && name_binding_rendered.contains("constant slot"),
        "{name_binding_rendered}"
    );
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_store_cell(_dp_cell_x, 1)"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"x\", 1)"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("StoreLocation(CapturedSource(")
            && name_binding_rendered.contains("constant slot"),
        "{name_binding_rendered}"
    );
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_store_global(__dp_globals(), \"y\""),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"y\", _dp_tmp_"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("y@g") && name_binding_rendered.contains("LocalLocation("),
        "{name_binding_rendered}"
    );
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_store_cell(_dp_cell_x, _dp_tmp"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"x\", _dp_tmp_"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("StoreLocation(CapturedSource(")
            && name_binding_rendered.contains("LocalLocation("),
        "{name_binding_rendered}"
    );
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered.contains("StoreName(\"value\", contextmanager_enter("),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("SetItem(_dp_class_ns, \"value\", contextmanager_enter("),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("SetItem(LocalLocation(0),")
            && module_constant_text(lowered.bb_module()).contains("contextmanager_enter"),
        "{}\n{}",
        name_binding_rendered,
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("StoreLocation(_dp_cell_value, contextmanager_enter("),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"value\", contextmanager_enter("),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("StoreLocation(CapturedSource(")
            && module_constant_text(lowered.bb_module()).contains("contextmanager_enter"),
        "{}\n{}",
        name_binding_rendered,
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered
            .contains("StoreName(\"B\", _dp_define_class_B(_dp_class_ns_B, _dp_class_ns))"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains(
            "__dp_setitem(__dp_load_deleted_name(\"_dp_class_ns\", _dp_class_ns), \"B\","
        ),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("SetItem(LocalLocation(0),")
            && module_constant_text(lowered.bb_module()).contains("make_function"),
        "{}\n{}",
        name_binding_rendered,
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
    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("branch_table CapturedSource(0)"),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("StoreLocation(CapturedSource(0),"),
        "{name_binding_rendered}"
    );

    let resume = lowered.bb_function("gen");
    let entry_params = resume.entry_block().param_name_vec();
    assert!(
        !entry_params.iter().any(|name| name == "_dp_pc"),
        "{resume:?}"
    );
    assert!(
        resume
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "CapturedSource(")),
        "{resume:?}"
    );
    assert!(
        resume
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "StoreLocation(CapturedSource(")),
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
    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("CapturedSource("),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("StoreLocation(CapturedSource(")
            && (name_binding_rendered.contains("__dp_NONE")
                || name_binding_rendered.contains("child()")),
        "{name_binding_rendered}"
    );

    let resume = lowered.bb_function("delegator");
    let entry_params = resume.entry_block().param_name_vec();
    assert!(
        !entry_params.iter().any(|name| name == "_dp_pc")
            && !entry_params.iter().any(|name| name == "_dp_yieldfrom"),
        "{resume:?}"
    );
    assert!(
        resume
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "CapturedSource(")),
        "{resume:?}"
    );
    assert!(
        resume
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "StoreLocation(CapturedSource(")),
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
    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("return CapturedSource("),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("StoreLocation(CapturedSource("),
        "{name_binding_rendered}"
    );

    let resume = lowered.bb_function("gen");
    let entry_params = resume.entry_block().param_name_vec();
    assert!(
        !entry_params.iter().any(|name| name == "total"),
        "{resume:?}"
    );
    assert!(
        resume
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "CapturedSource(0)")),
        "{resume:?}"
    );
    assert!(
        resume
            .blocks
            .iter()
            .any(|block| { block_uses_text(block, "StoreLocation(CapturedSource(0),") }),
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
            .blocks
            .iter()
            .any(|block| block_uses_text(block, "CapturedSource(")),
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("_dp_cell_total = __dp_make_cell"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("_dp_cell__dp_pc = __dp_make_cell"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("_dp_cell__dp_yieldfrom = __dp_make_cell"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("StoreLocation(Owned(1), MakeCell(constant slot")
            && count_occurrences(name_binding_rendered.as_str(), "MakeCell(") >= 3,
        "{name_binding_rendered}"
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
    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("_dp_try_exc_")
            && name_binding_rendered.contains("_dp_cell__dp_try_exc_"),
        "{name_binding_rendered}"
    );
    assert!(
        function_or_constants_use_text(
            lowered.bb_module(),
            lowered.bb_function("gen"),
            "exception_matches"
        ) && name_binding_rendered.contains("CapturedSource(")
            && !name_binding_rendered.contains("StoreName(\"_dp_eval_"),
        "{}\n{}",
        name_binding_rendered,
        module_constant_text(lowered.bb_module())
    );
    assert!(
        !name_binding_rendered.contains("del _dp_eval_"),
        "{name_binding_rendered}"
    );

    let resume = lowered.bb_function("gen");
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_store_global"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"f\", MakeFunction"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("f@g")
            && module_constant_text(lowered.bb_module()).contains("make_function"),
        "{}\n{}",
        name_binding_rendered,
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_store_global"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("__dp_load_global"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"x\", 1)"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"y\", x)"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("x@g") && name_binding_rendered.contains("constant slot"),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("y@g") && name_binding_rendered.contains("x@g"),
        "{name_binding_rendered}"
    );
}

#[test]
fn top_level_global_named_expr_moves_to_name_binding_pass() {
    let source = r#"
def f():
    return 1

x = (y := f())
"#;

    let lowered = TrackedLowering::new(source);
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_store_global"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("__dp_load_global"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"y\", f())"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"x\", y)"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("y@g"),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("x@g") && name_binding_rendered.contains("y@g"),
        "{name_binding_rendered}"
    );
}

#[test]
fn top_level_comprehension_named_expr_uses_global_decl_then_name_binding_pass() {
    let source = r#"
x = [y := i for i in [1, 2]]
"#;

    let lowered = TrackedLowering::new(source);
    let ast_rendered = lowered.pass_text("ast-to-ast");
    assert!(ast_rendered.contains("global y"), "{ast_rendered}");
    assert!(
        !ast_rendered.contains("__dp_store_global"),
        "{ast_rendered}"
    );

    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_store_global"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"y\", i)"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("y@g") && name_binding_rendered.contains("LocalLocation("),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("x@g"),
        "{name_binding_rendered}"
    );
}

#[test]
fn top_level_for_target_global_binding_moves_to_name_binding_pass() {
    let source = r#"
for x in [1, 2]:
    pass
"#;

    let lowered = TrackedLowering::new(source);
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_store_global"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("StoreName(\"x\", _dp_tmp_"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("x@g") && name_binding_rendered.contains("LocalLocation("),
        "{name_binding_rendered}"
    );
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
    let semantic_rendered = lowered.blockpy_text();
    assert!(
        !semantic_rendered.contains("__dp_store_global(__dp_globals(), \"exc\""),
        "{semantic_rendered}"
    );
    assert!(
        semantic_rendered.contains("del_quietly(exc)"),
        "{semantic_rendered}"
    );
    assert!(
        !semantic_rendered.contains("__dp_current_exception()"),
        "{semantic_rendered}"
    );
    assert!(
        semantic_rendered.contains("StoreName(\"exc\", _dp_try_exc_"),
        "{semantic_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("exc@g") && name_binding_rendered.contains("LocalLocation("),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("Del {") && name_binding_rendered.contains("quietly: true"),
        "{name_binding_rendered}"
    );
}

#[test]
fn top_level_global_delete_moves_to_name_binding_pass() {
    let source = r#"
x = 1
del x
"#;

    let lowered = TrackedLowering::new(source);
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_delitem(__dp_globals(), \"x\")"),
        "{core_rendered}"
    );
    assert!(
        core_rendered.contains("Del {") && core_rendered.contains("quietly: false"),
        "{core_rendered}"
    );
    assert!(!core_rendered.contains("__dp_DELETED"), "{core_rendered}");

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("Del {") && name_binding_rendered.contains("quietly: false"),
        "{name_binding_rendered}"
    );
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("__dp_load_deleted_name"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("StoreName(\"x\", 1)"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        module_constant_text(lowered.bb_module()).contains("load_deleted_name")
            && module_constant_text(lowered.bb_module()).contains("DELETED"),
        "{}\n{}",
        name_binding_rendered,
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
    let blockpy_rendered = lowered.blockpy_text();
    assert!(
        blockpy_rendered.contains("function outer.<locals>.inner():")
            && blockpy_rendered.contains(
                "StoreName(\"inner\", MakeFunction(0:1, Function, tuple_values(), NONE))"
            )
            && blockpy_rendered.contains("Del {")
            && blockpy_rendered.contains("quietly: false"),
        "{blockpy_rendered}"
    );
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        core_rendered.contains("StoreName(\"x\", BinOp(Add, x, 1))"),
        "{core_rendered}"
    );
    assert!(core_rendered.contains("return x"), "{core_rendered}");
    assert!(
        !core_rendered.contains("__dp_store_cell(_dp_cell_x, __dp_add("),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("return __dp_load_cell(_dp_cell_x)"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered.contains("StoreLocation(CapturedSource(")
            && name_binding_rendered.contains("BinOp(Add, CapturedSource("),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("return CapturedSource("),
        "{name_binding_rendered}"
    );
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
    let core_rendered = lowered.pass_text("core_blockpy");
    assert!(
        !core_rendered.contains("_dp_cell_x = __dp_make_cell(x)"),
        "{core_rendered}"
    );
    assert!(
        !core_rendered.contains("_dp_cell_y = __dp_make_cell()"),
        "{core_rendered}"
    );

    let name_binding_rendered = lowered.name_binding_text();
    assert!(
        name_binding_rendered
            .contains("StoreLocation(LocalLocation(1), MakeCell(LocalLocation(0)))"),
        "{name_binding_rendered}"
    );
    assert!(
        name_binding_rendered.contains("StoreLocation(LocalLocation(2), MakeCell(")
            && (name_binding_rendered.contains("MakeCell(DELETED)")
                || name_binding_rendered.contains("MakeCell(constant slot")),
        "{}\n{}",
        name_binding_rendered,
        module_constant_text(lowered.bb_module())
    );
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
    let blockpy_rendered = lowered.blockpy_text();
    assert!(
        blockpy_rendered.contains("StoreName(\"x\", y)"),
        "{blockpy_rendered}"
    );
    assert!(
        blockpy_rendered.contains("return f(x)"),
        "{blockpy_rendered}"
    );
    assert!(!blockpy_rendered.contains(":="), "{blockpy_rendered}");
}

#[test]
fn scoped_helper_expr_pass_lowers_listcomp_before_blockpy() {
    let source = r#"
def choose(xs):
    return f([x for x in xs])
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_rendered = lowered.blockpy_text();
    assert!(
        blockpy_rendered.contains("function choose.<locals>._dp_listcomp_"),
        "{blockpy_rendered}"
    );
    assert!(
        blockpy_rendered.contains("return f(_dp_listcomp"),
        "{blockpy_rendered}"
    );
}

#[test]
fn scoped_helper_expr_pass_lowers_genexpr_before_blockpy() {
    let source = r#"
def choose(xs):
    return tuple(x for x in xs)
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_rendered = lowered.blockpy_text();
    assert!(
        blockpy_rendered.contains("generator choose.<locals>.<genexpr>("),
        "{blockpy_rendered}"
    );
    assert!(
        blockpy_rendered.contains("return tuple(_dp_genexpr"),
        "{blockpy_rendered}"
    );
}

#[test]
fn module_plan_lowers_lambda_before_blockpy() {
    let source = r#"
def choose():
    return f(lambda x: x + 1)
"#;

    let lowered = TrackedLowering::new(source);
    let blockpy_rendered = lowered.blockpy_text();
    assert!(
        blockpy_rendered.contains("function choose.<locals>.<lambda>("),
        "{blockpy_rendered}"
    );
    assert!(
        blockpy_rendered.contains("return f(MakeFunction("),
        "{blockpy_rendered}"
    );
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
    let semantic_blockpy_rendered = lowered.blockpy_text();
    assert!(
        semantic_blockpy_rendered.contains("await Once()"),
        "{semantic_blockpy_rendered}"
    );
    assert!(
        !semantic_blockpy_rendered.contains("await_iter"),
        "{semantic_blockpy_rendered}"
    );

    let blockpy_rendered = lowered.core_blockpy_with_yield_text();
    assert!(
        blockpy_rendered.contains("await_iter"),
        "{blockpy_rendered}"
    );
    assert!(
        !blockpy_rendered.contains("await Once()"),
        "{blockpy_rendered}"
    );
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
    let semantic_blockpy_rendered = lowered.blockpy_text();
    assert!(
        semantic_blockpy_rendered.contains("await Once()"),
        "{semantic_blockpy_rendered}"
    );
    assert!(
        !semantic_blockpy_rendered.contains("await_iter"),
        "{semantic_blockpy_rendered}"
    );

    let blockpy_rendered = lowered.core_blockpy_with_yield_text();
    assert!(
        blockpy_rendered.contains("await_iter"),
        "{blockpy_rendered}"
    );
    assert!(
        !blockpy_rendered.contains("await Once()"),
        "{blockpy_rendered}"
    );
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
    let semantic_blockpy_rendered = lowered.blockpy_text();
    assert!(
        semantic_blockpy_rendered.contains("await asynccontextmanager_aenter"),
        "{semantic_blockpy_rendered}"
    );
    assert!(
        semantic_blockpy_rendered.contains("asynccontextmanager_get_aexit"),
        "{semantic_blockpy_rendered}"
    );
    assert!(
        !semantic_blockpy_rendered.contains("await_iter"),
        "{semantic_blockpy_rendered}"
    );

    let blockpy_rendered = lowered.core_blockpy_with_yield_text();
    assert!(
        blockpy_rendered.contains("await_iter"),
        "{blockpy_rendered}"
    );
    assert!(
        blockpy_rendered.contains("asynccontextmanager_aenter"),
        "{blockpy_rendered}"
    );
    assert!(
        !blockpy_rendered.contains("async with cm as value"),
        "{blockpy_rendered}"
    );
}

#[test]
fn rewritten_ruff_ast_can_keep_coroutine_async_with_while_blockpy_generator_lowering_handles_it() {
    let source = r#"
async def run(cm):
    async with cm as value:
        return value
"#;

    let lowered = TrackedLowering::new(source);
    let semantic_blockpy_rendered = lowered.blockpy_text();
    assert!(
        semantic_blockpy_rendered.contains("await asynccontextmanager_aenter"),
        "{semantic_blockpy_rendered}"
    );
    assert!(
        semantic_blockpy_rendered.contains("asynccontextmanager_get_aexit"),
        "{semantic_blockpy_rendered}"
    );
    assert!(
        !semantic_blockpy_rendered.contains("await_iter"),
        "{semantic_blockpy_rendered}"
    );

    let blockpy_rendered = lowered.core_blockpy_with_yield_text();
    assert!(
        blockpy_rendered.contains("await_iter"),
        "{blockpy_rendered}"
    );
    assert!(
        blockpy_rendered.contains("asynccontextmanager_aenter"),
        "{blockpy_rendered}"
    );
    assert!(
        !blockpy_rendered.contains("async with cm as value"),
        "{blockpy_rendered}"
    );
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
        bump.blocks
            .iter()
            .any(|block| block.body.iter().any(|stmt| matches!(
                stmt,
                expr if expr_text(expr).contains("BinOp(InplaceAdd,")
            ))),
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
    let debug = format!("{run:?}");
    assert!(
        function_or_constants_use_text(&bb_module, run, "anext_or_sentinel"),
        "{run:?}"
    );
    assert!(
        function_or_constants_use_text(&bb_module, run, "aiter"),
        "{run:?}"
    );
    assert!(!debug.contains("_dp_completed_"), "{debug}");
}

#[test]
fn semantic_blockpy_lowers_async_for_to_awaited_fetch_before_await_lowering() {
    let source = r#"
async def run():
    async for x in ait:
        body()
"#;

    let lowered = TrackedLowering::new(source);
    let semantic_blockpy_rendered = lowered.blockpy_text();
    assert!(
        semantic_blockpy_rendered.contains("await anext_or_sentinel"),
        "{semantic_blockpy_rendered}"
    );
    assert!(
        semantic_blockpy_rendered.contains("aiter"),
        "{semantic_blockpy_rendered}"
    );
    assert!(
        !semantic_blockpy_rendered.contains("yield from await_iter"),
        "{semantic_blockpy_rendered}"
    );
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
        let blockpy_rendered = crate::block_py::pretty::blockpy_module_to_string(&blockpy);
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
    let make_function = runtime_call_by_name(bb_module, resume_expr, "make_function")
        .expect("resume should use make_function");
    let captures_expr = match make_function.args.get(2) {
        Some(CallArgPositional::Positional(expr)) => expr,
        other => panic!("expected captures positional arg, got {other:?}"),
    };
    let captures_tuple = runtime_call_by_name(bb_module, captures_expr, "tuple_values")
        .expect("captures should be tuple_values");
    assert_eq!(
        captures_tuple.args.len(),
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
    let debug = format!("{f:?}");
    assert!(!debug.contains("finally:"), "{debug}");
    assert!(!debug.contains("_dp_try_reason_"), "{debug}");
    assert!(!debug.contains("_dp_try_value_"), "{debug}");
    assert!(debug.contains("_dp_try_abrupt_kind_"), "{debug}");
    assert!(debug.contains("_dp_try_abrupt_payload_"), "{debug}");
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

    let lowered = TrackedLowering::new(source);
    let rendered = lowered.pass_text("ast-to-ast");
    assert!(rendered.contains("def _dp_module_init()"), "{rendered}");
    assert!(!rendered.contains("global VALUE"), "{rendered}");
    assert!(!rendered.contains("global build"), "{rendered}");
    assert!(!rendered.contains("global Box"), "{rendered}");
}

#[test]
fn module_init_rebinds_top_level_assignments_and_classes_without_global_prelude() {
    let source = r#"
VALUE = 1

class Box:
    item = VALUE
"#;

    let lowered = TrackedLowering::new(source);
    let rendered = lowered.pass_text("ast-to-ast");
    assert!(!rendered.contains("global VALUE"), "{rendered}");
    assert!(!rendered.contains("global Box"), "{rendered}");

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
        module_constant_text(&bb_module).contains("load_deleted_name"),
        "{}\n{}",
        debug,
        module_constant_text(&bb_module)
    );
    assert!(
        module_constant_text(&bb_module).contains("DELETED"),
        "{}\n{}",
        debug,
        module_constant_text(&bb_module)
    );
    assert!(!debug.contains("x = 1"), "{debug}");
}
