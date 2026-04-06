use super::*;
use crate::block_py::{BlockParam, BlockParamRole, Instr};
use crate::block_py::{
    ClosureInit, ClosureSlot, InstrWithAwaitAndYield, NameLocation, ResolvedName, StorageLayout,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::passes::{CoreModuleShapeWithAwaitAndYield, InstrRuff, ResolvedStorageModuleShape};
use ruff_python_parser::parse_expression;

fn wrapped_blockpy(source: &str) -> BlockPyModule<CoreModuleShapeWithAwaitAndYield> {
    lower_python_to_blockpy_for_testing(source)
        .expect("expected lowered core BlockPy module")
        .pass_tracker
        .pass_core_blockpy_with_await_and_yield()
        .expect("core_blockpy_with_await_and_yield pass should be tracked")
        .clone()
}

fn parse_blockpy_expr(source: &str) -> InstrRuff {
    crate::passes::ast_to_instr::from_ast_expr((*parse_expression(source).unwrap().into_syntax().body).into())
}

fn parse_core_blockpy_expr(source: &str) -> InstrWithAwaitAndYield {
    InstrWithAwaitAndYield::from_ruff_expr(parse_blockpy_expr(source))
}

fn empty_param_spec() -> ParamSpec {
    ParamSpec::default()
}

fn test_name_gen() -> crate::block_py::FunctionNameGen {
    let module_name_gen = crate::block_py::ModuleNameGen::new(0);
    module_name_gen.next_function_name_gen()
}

fn label(index: u32) -> BlockLabel {
    BlockLabel::from_index(index as usize)
}

fn located_name(id: &str, location: NameLocation) -> ResolvedName {
    ResolvedName {
        id: id.into(),
        location,
    }
}
fn function_by_bind_name<'a, P>(
    module: &'a BlockPyModule<P>,
    bind_name: &str,
) -> &'a BlockPyFunction<P>
where
    P: BlockPyPrettyPrinter,
{
    module
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == bind_name)
        .unwrap_or_else(|| panic!("missing function {bind_name}"))
}

#[test]
fn renders_blockpy_module_with_module_init_and_nested_blocks() {
    let blockpy = wrapped_blockpy(
        r#"
seed = 1

def classify(a, /, b: int = 1, *args, c=2, **kwargs):
    if a:
        return "yes"
    return "no"
"#,
    );
    let classify = function_by_bind_name(&blockpy, "classify");
    let rendered = blockpy_module_to_string(&blockpy);

    assert!(
        rendered.contains("function classify(a, /, b, *args, c, **kwargs):"),
        "{rendered}"
    );
    assert!(rendered.contains("function_id: "), "{rendered}");
    assert!(rendered.contains("function _dp_module_init():"));
    assert!(!rendered.contains("module_init: _dp_module_init"));
    assert!(
        rendered.contains(format!("block {}:", classify.entry_block().label_str()).as_str())
            || rendered.contains(format!("block {}(", classify.entry_block().label_str()).as_str()),
        "{rendered}"
    );
    assert!(rendered.contains("if_term a:"));
    assert!(rendered.contains("return \"yes\""));
}

#[test]
fn renders_empty_module_marker() {
    let empty_module: BlockPyModule<CoreModuleShapeWithAwaitAndYield> = BlockPyModule {
        module_name_gen: crate::block_py::ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: Vec::new(),
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    };
    let rendered = blockpy_module_to_string(&empty_module);
    assert_eq!(rendered, "; empty BlockPy module\n");
}

#[test]
fn bb_text_renders_located_names_with_resolved_locations() {
    let closure_name = located_name("captured", NameLocation::closure_cell(2));
    let closure_expr: crate::block_py::InstrResolved =
        crate::block_py::Load::new(closure_name.clone()).into();
    let assign_stmt = crate::block_py::Store::new(
        located_name("temp", NameLocation::local(1)),
        Box::new(closure_expr.clone()),
    )
    .into();
    let global_name = located_name("answer", NameLocation::global(0));
    let global_expr: crate::block_py::InstrResolved =
        crate::block_py::Load::new(global_name.clone()).into();

    let closure_rendered = bb_expr_text(&closure_expr);
    assert!(
        closure_rendered.contains("Closure(2)"),
        "{closure_rendered}"
    );
    let assign_rendered = core_bb_stmt_text(&assign_stmt);
    assert!(
        assign_rendered.contains("LocalLocation(1)"),
        "{assign_rendered}"
    );
    assert!(assign_rendered.contains("Closure(2)"), "{assign_rendered}");
    let global_rendered = bb_expr_text(&global_expr);
    assert_eq!(global_rendered, "answer@g0");
}

#[test]
fn transformed_lowering_result_exposes_module_init_blockpy() {
    let blockpy = lower_python_to_blockpy_for_testing(
        r#"
def classify(n):
    if n < 0:
        return "neg"
    return "pos"
"#,
    )
    .unwrap()
    .pass_tracker
    .pass_core_blockpy_with_await_and_yield()
    .expect("core_blockpy_with_await_and_yield pass should be tracked")
    .clone();
    let rendered = blockpy_module_to_string(&blockpy);

    assert!(blockpy
        .callable_defs
        .iter()
        .any(|function| function.names.bind_name == "_dp_module_init"));
    assert!(rendered.contains("function _dp_module_init():"));
    assert!(rendered.contains("function_id: "), "{rendered}");
}

#[test]
fn debug_blockpy_render_uses_blockpy_expr_text_for_core_ops() {
    let blockpy = lower_python_to_blockpy_for_testing(
        r#"
def tweak(x):
    x += 1
    return x
"#,
    )
    .unwrap()
    .pass_tracker
    .pass_core_blockpy()
    .expect("core_blockpy pass should be tracked")
    .clone();
    let rendered = blockpy_module_to_debug_string(&blockpy);

    assert!(rendered.contains("InplaceAdd"), "{rendered}");
    assert!(!rendered.contains("__dp_iadd"), "{rendered}");
}

#[test]
fn renders_generator_kind_without_internal_metadata() {
    let blockpy = wrapped_blockpy(
        r#"
def gen():
    yield 1
"#,
    );
    let rendered = blockpy_module_to_string(&blockpy);

    assert!(rendered.contains("generator gen():"));
    assert!(rendered.contains("function_id: "), "{rendered}");
    assert!(!rendered.contains("generator_state:"));
}

#[test]
fn renders_referenced_non_inlined_blocks_for_async_generator_shape() {
    let blockpy = wrapped_blockpy(
        r#"
async def a():
    return 3

async def no_lying():
    for i in range((await a()) + 2):
        yield i
"#,
    );
    let function = function_by_bind_name(&blockpy, "no_lying");
    let rendered = blockpy_module_to_string(&BlockPyModule {
        module_name_gen: crate::block_py::ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: vec![function.clone()],
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    });
    let layout = BlockRenderLayout::new(function);
    let inlined_labels = layout
        .inlined_blocks
        .iter()
        .map(|index| function.blocks[*index].label.to_string())
        .collect::<HashSet<_>>();

    let missing_labels =
        collect_referenced_labels_from_blocks::<CoreModuleShapeWithAwaitAndYield>(&function.blocks)
            .into_iter()
            .map(|label| label.to_string())
            .filter(|label| !inlined_labels.contains(label))
            .filter(|label| {
                !rendered.contains(format!("block {label}:").as_str())
                    && !rendered.contains(format!("block {label}(").as_str())
            })
            .collect::<Vec<_>>();

    assert!(missing_labels.is_empty(), "{rendered}");
}

#[test]
fn renders_public_closure_metadata_in_function_header() {
    let rendered = blockpy_module_to_string(&BlockPyModule {
        module_name_gen: crate::block_py::ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: vec![BlockPyFunction::<CoreModuleShapeWithAwaitAndYield> {
            function_id: crate::block_py::FunctionId::new(0, 1),
            name_gen: test_name_gen(),
            names: crate::block_py::FunctionName::new("gen", "gen", "gen", "gen"),
            kind: FunctionKind::Function,
            params: empty_param_spec(),
            blocks: vec![Block {
                label: label(0),
                body: vec![],
                term: BlockTerm::Return(parse_core_blockpy_expr("__dp_NONE")),
                params: Vec::new(),
                exc_edge: None,
            }],
            doc: None,
            storage_layout: Some(StorageLayout {
                freevars: vec![ClosureSlot {
                    logical_name: "factor".to_string(),
                    storage_name: "factor".to_string(),
                    init: ClosureInit::InheritedCapture,
                }],
                cellvars: vec![ClosureSlot {
                    logical_name: "total".to_string(),
                    storage_name: "_dp_cell_total".to_string(),
                    init: ClosureInit::Deferred,
                }],
                runtime_cells: vec![ClosureSlot {
                    logical_name: "_dp_pc".to_string(),
                    storage_name: "_dp_cell__dp_pc".to_string(),
                    init: ClosureInit::RuntimePcUnstarted,
                }],
                stack_slots: Vec::new(),
            }),
            scope: crate::block_py::CallableScopeInfo::default(),
        }],
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    });

    assert!(rendered.contains(
            "function gen():\n    function_id: 0:1\n    freevars: [factor->factor@inherited]\n    cellvars: [total->_dp_cell_total@deferred]\n    runtime_cells: [_dp_pc->_dp_cell__dp_pc@pc_unstarted]"
        ));
    assert!(!rendered.contains("entry:"));
}

#[test]
fn renders_followup_blocks_under_their_owning_entry_block() {
    let function: BlockPyFunction<CoreModuleShapeWithAwaitAndYield> = BlockPyFunction {
        function_id: crate::block_py::FunctionId::new(0, 1),
        name_gen: test_name_gen(),
        names: crate::block_py::FunctionName::new("f", "f", "f", "f"),
        kind: FunctionKind::Function,
        params: empty_param_spec(),
        blocks: vec![
            Block {
                label: label(0),
                body: vec![],
                term: BlockTerm::IfTerm(TermIf {
                    test: parse_core_blockpy_expr("cond"),
                    then_label: label(1),
                    else_label: label(2),
                }),
                params: Vec::new(),
                exc_edge: None,
            },
            Block {
                label: label(1),
                body: vec![parse_core_blockpy_expr("then_side_effect()")],
                term: BlockTerm::Jump(BlockEdge::new(label(3))),
                params: Vec::new(),
                exc_edge: None,
            },
            Block {
                label: label(2),
                body: vec![parse_core_blockpy_expr("else_side_effect()")],
                term: BlockTerm::Jump(BlockEdge::new(label(3))),
                params: Vec::new(),
                exc_edge: None,
            },
            Block {
                label: label(3),
                body: vec![parse_core_blockpy_expr("finish()")],
                term: BlockTerm::Return(parse_core_blockpy_expr("__dp_NONE")),
                params: Vec::new(),
                exc_edge: None,
            },
        ],
        doc: None,
        storage_layout: None,
        scope: crate::block_py::CallableScopeInfo::default(),
    };
    let rendered = blockpy_module_to_string(&BlockPyModule {
        module_name_gen: crate::block_py::ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: vec![function],
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    });

    assert!(rendered.contains("    block bb0:\n"));
    assert!(rendered.contains("        block bb3:\n"));
    assert!(rendered.contains(
            "        if_term cond:\n            then:\n                block bb1:\n                    then_side_effect()\n                    jump bb3\n            else:\n                block bb2:\n                    else_side_effect()\n                    jump bb3\n        block bb3:\n            finish()\n            return __dp_NONE\n"
        ));
}

#[test]
fn elides_trivial_if_term_jump_wrappers_when_rendering() {
    let blockpy = wrapped_blockpy(
        r#"
def choose(a, b):
    total = a + b
    if total > 5:
        return a
    else:
        return b
"#,
    );
    let rendered = blockpy_module_to_string(&blockpy);

    assert!(rendered.contains("return a"), "{rendered}");
    assert!(rendered.contains("return b"), "{rendered}");
    assert!(!rendered.contains("block _dp_bb_choose_1_then"));
    assert!(!rendered.contains("block _dp_bb_choose_1_else"));
}

#[test]
fn sorts_rendered_root_and_child_blocks_by_label() {
    let function: BlockPyFunction<CoreModuleShapeWithAwaitAndYield> = BlockPyFunction {
        function_id: crate::block_py::FunctionId::new(0, 1),
        name_gen: test_name_gen(),
        names: crate::block_py::FunctionName::new("f", "f", "f", "f"),
        kind: FunctionKind::Function,
        params: empty_param_spec(),
        blocks: vec![
            Block {
                label: label(0),
                body: vec![],
                term: BlockTerm::Jump(BlockEdge::new(label(4))),
                params: Vec::new(),
                exc_edge: Some(BlockEdge::new(label(1))),
            },
            Block {
                label: label(4),
                body: vec![],
                term: BlockTerm::Return(parse_core_blockpy_expr("__dp_NONE")),
                params: Vec::new(),
                exc_edge: None,
            },
            Block {
                label: label(1),
                body: vec![],
                term: BlockTerm::Return(parse_core_blockpy_expr("__dp_NONE")),
                params: Vec::new(),
                exc_edge: None,
            },
            Block {
                label: label(3),
                body: vec![],
                term: BlockTerm::Return(parse_core_blockpy_expr("__dp_NONE")),
                params: Vec::new(),
                exc_edge: None,
            },
            Block {
                label: label(2),
                body: vec![],
                term: BlockTerm::Return(parse_core_blockpy_expr("__dp_NONE")),
                params: Vec::new(),
                exc_edge: None,
            },
        ],
        doc: None,
        storage_layout: None,
        scope: crate::block_py::CallableScopeInfo::default(),
    };
    let rendered = blockpy_module_to_string(&BlockPyModule {
        module_name_gen: crate::block_py::ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: vec![function],
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    });

    let alpha_pos = rendered.find("block bb1:").expect("bb1 block");
    let zeta_pos = rendered.find("block bb4:").expect("bb4 block");
    let beta_pos = rendered.find("block bb2:").expect("bb2 block");
    let omega_pos = rendered.find("block bb3:").expect("bb3 block");

    assert!(zeta_pos < alpha_pos, "{rendered}");
    assert!(beta_pos < omega_pos, "{rendered}");
}

#[test]
fn collects_referenced_labels_from_plain_blocks_via_term_and_exc_edges() {
    let referenced = collect_referenced_labels_from_blocks::<CoreModuleShapeWithAwaitAndYield>(&[
        Block {
            label: label(0),
            body: vec![parse_core_blockpy_expr("x")],
            term: BlockTerm::BranchTable(super::super::TermBranchTable {
                index: parse_core_blockpy_expr("index"),
                targets: vec![label(2), label(3)],
                default_label: label(4),
            }),
            params: Vec::new(),
            exc_edge: Some(BlockEdge::new(label(6))),
        },
        Block {
            label: label(1),
            body: Vec::new(),
            term: BlockTerm::Jump(BlockEdge::new(label(5))),
            params: Vec::new(),
            exc_edge: None,
        },
    ]);

    let expected = [label(2), label(3), label(4), label(5), label(6)]
        .into_iter()
        .collect::<HashSet<_>>();

    assert_eq!(referenced, expected);
}

#[test]
fn renders_bb_block_metadata_with_shared_layout() {
    let rendered = blockpy_module_to_string(&BlockPyModule {
        module_name_gen: crate::block_py::ModuleNameGen::new(0),
        global_names: Vec::new(),
        callable_defs: vec![BlockPyFunction::<ResolvedStorageModuleShape> {
            function_id: crate::block_py::FunctionId::new(0, 1),
            name_gen: test_name_gen(),
            names: crate::block_py::FunctionName::new("f", "f", "f", "f"),
            kind: FunctionKind::Function,
            params: empty_param_spec(),
            blocks: vec![
                crate::block_py::ResolvedStorageBlock {
                    label: label(0),
                    body: vec![],
                    term: BlockTerm::Jump(BlockEdge::new(label(1))),
                    params: vec![
                        BlockParam {
                            name: "err".to_string(),
                            role: BlockParamRole::Exception,
                        },
                        BlockParam {
                            name: "x".to_string(),
                            role: BlockParamRole::AbruptPayload,
                        },
                    ],
                    exc_edge: Some(BlockEdge::new(label(1))),
                },
                crate::block_py::ResolvedStorageBlock {
                    label: label(1),
                    body: vec![],
                    term: BlockTerm::Return(
                        crate::block_py::InstrResolved::constant_none(),
                    ),
                    params: vec![BlockParam {
                        name: "err".to_string(),
                        role: BlockParamRole::Exception,
                    }],
                    exc_edge: None,
                },
            ],
            doc: None,
            storage_layout: None,
            scope: crate::block_py::CallableScopeInfo::default(),
        }],
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    });

    assert!(rendered.contains("function f():"), "{rendered}");
    assert!(rendered.contains("function_id: 0:1"), "{rendered}");
    assert!(
        rendered.contains("block bb0(err: Exception, x: AbruptPayload):"),
        "{rendered}"
    );
    assert!(rendered.contains("exc_target: bb1"), "{rendered}");
    assert!(rendered.contains("exc_name: err"), "{rendered}");
    assert!(rendered.contains("jump bb1"), "{rendered}");
}
