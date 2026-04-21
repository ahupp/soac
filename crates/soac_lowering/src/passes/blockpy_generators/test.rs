use super::{
    augment_resume_semantic_for_standard_name_binding, build_blockpy_storage_layout,
    current_exception_value_expr, is_name_not_none_test, persistent_generator_state_order,
    resume_closure_bindings, yield_from_method_lookup_expr, yield_from_send_expr, ErrOnYield,
};
use crate::block_py::{
    core_call_expr_with_meta, BinOpKind, BindingKind, BindingPurpose, Block, BlockBuilder,
    BlockLabel, BlockTerm, CallArgPositional, CallableScopeInfo, CallableScopeKind,
    CellBindingKind, ClosureInit, ClosureSlot, FunctionName, HasMeta, InstrUnresolved,
    InstrWithYield, Literal, Meta, NameLike, RuntimeFunctionId, StorageLayout, TryMapTerm,
    UnaryOpKind, WithMeta, Yield,
};
use crate::passes::ast_to_ast::scope_helpers::is_internal_symbol;
use crate::passes::InstrRuff;
use crate::py_expr;
use ruff_python_ast::{self as ast, Expr};
use ruff_text_size::TextRange;
use std::collections::HashSet;

fn generator_test_semantic() -> CallableScopeInfo {
    CallableScopeInfo {
        names: FunctionName::new("gen", "gen", "gen", "gen"),
        scope_kind: CallableScopeKind::Function,
        ..Default::default()
    }
}

fn generator_resume_source_semantic(layout: &StorageLayout) -> CallableScopeInfo {
    let mut scope = generator_test_semantic();
    for slot in &layout.freevars {
        scope.insert_binding_with_cell_names(
            slot.logical_name.clone(),
            BindingKind::Cell(CellBindingKind::Capture),
            is_internal_symbol(slot.logical_name.as_str()),
            Some(slot.logical_name.clone()),
            Some(slot.storage_name.clone()),
        );
    }
    for slot in &layout.cellvars {
        scope.insert_binding_with_cell_names(
            slot.logical_name.clone(),
            BindingKind::Cell(CellBindingKind::Owner),
            is_internal_symbol(slot.logical_name.as_str()),
            Some(slot.storage_name.clone()),
            Some(slot.storage_name.clone()),
        );
    }
    scope
}

fn build_closure_backed_generator_factory_block(
    _factory_label: &str,
    visible_names: &FunctionName,
    resume_function_id: RuntimeFunctionId,
    _resume_state_order: &[String],
    _layout: &StorageLayout,
    is_coroutine: bool,
    is_async_generator: bool,
) -> Block<InstrRuff> {
    let resume_entry = py_expr!(
        "__dp_make_function({function_id:literal}, \"function\", __dp_tuple(), __dp_tuple(), None)",
        function_id = resume_function_id.to_packed_runtime_u64(),
    );

    let generator_expr = if is_async_generator {
        py_expr!(
            "runtime.ClosureAsyncGenerator(resume={resume:expr}, name={name:literal}, qualname={qualname:literal}, code=runtime.code_template_async_gen.__code__.replace(co_name={name:literal}, co_qualname={qualname:literal}), yieldfrom_cell=__dp_cell_ref(\"_dp_yieldfrom\"), throw_context_cell=__dp_cell_ref(\"_dp_throw_context\"))",
            resume = resume_entry,
            name = visible_names.display_name.as_str(),
            qualname = visible_names.qualname.as_str(),
        )
    } else {
        py_expr!(
            "runtime.ClosureGenerator(resume={resume:expr}, name={name:literal}, qualname={qualname:literal}, code=runtime.code_template_gen.__code__.replace(co_name={name:literal}, co_qualname={qualname:literal}), yieldfrom_cell=__dp_cell_ref(\"_dp_yieldfrom\"), throw_context_cell=__dp_cell_ref(\"_dp_throw_context\"))",
            resume = resume_entry,
            name = visible_names.display_name.as_str(),
            qualname = visible_names.qualname.as_str(),
        )
    };

    let return_value = if is_coroutine {
        py_expr!("runtime.Coroutine({gen:expr})", gen = generator_expr)
    } else {
        generator_expr
    };

    Block::from_builder(
        BlockLabel::from_index(0),
        BlockBuilder::with_term(
            Vec::new(),
            Some(BlockTerm::Return(
                crate::passes::ast_to_instr::from_ast_expr(return_value),
            )),
        ),
        Vec::new(),
        None,
        None,
    )
}

fn name_expr(name: &str) -> ast::ExprName {
    let Expr::Name(name) = py_expr!("{name:id}", name = name) else {
        unreachable!();
    };
    name
}

fn core_load_with_yield(name: &str) -> InstrWithYield {
    let name = name_expr(name);
    let meta = name.meta();
    crate::block_py::Load::new(name.id).with_meta(meta).into()
}

#[test]
fn name_not_none_helper_builds_not_is_none_shape() {
    let expr = is_name_not_none_test("value");
    let InstrUnresolved::UnaryOp(not_expr) = expr else {
        panic!("expected unary not expression");
    };
    assert_eq!(not_expr.kind, UnaryOpKind::Not);
    let InstrUnresolved::BinOp(is_expr) = *not_expr.operand else {
        panic!("expected inner identity test");
    };
    assert_eq!(is_expr.kind, BinOpKind::Is);
    let InstrUnresolved::Load(name) = *is_expr.left else {
        panic!("expected left side to load the named value");
    };
    assert_eq!(name.name.id_str(), "value");
    let InstrUnresolved::Load(name) = *is_expr.right else {
        panic!("expected right side to load NONE");
    };
    assert_eq!(name.name.id_str(), "NONE");
    assert!(name.name.is_runtime_name());
}

#[test]
fn yield_from_send_helper_builds_send_call_shape() {
    let expr = yield_from_send_expr();
    let InstrUnresolved::Call(call) = expr else {
        panic!("expected call expression");
    };
    let InstrUnresolved::GetAttr(get_attr) = *call.func else {
        panic!("expected getattr call target");
    };
    let InstrUnresolved::Load(name) = *get_attr.value else {
        panic!("expected send receiver load");
    };
    assert_eq!(name.name.id_str(), "_dp_yieldfrom");
    let InstrUnresolved::Literal(lit) = *get_attr.attr else {
        panic!("expected send attr literal");
    };
    let Literal::StringLiteral(lit) = lit.into_literal() else {
        panic!("expected string attr literal");
    };
    assert_eq!(lit.value, "send");
    assert_eq!(call.args.len(), 1);
    let CallArgPositional::Positional(InstrUnresolved::Load(name)) = &call.args[0] else {
        panic!("expected positional _dp_send_value argument");
    };
    assert_eq!(name.name.id_str(), "_dp_send_value");
}

#[test]
fn yield_from_lookup_helper_builds_getattr_call_shape() {
    let expr = yield_from_method_lookup_expr("close");
    let InstrUnresolved::Call(call) = expr else {
        panic!("expected call expression");
    };
    let InstrUnresolved::Load(name) = *call.func else {
        panic!("expected getattr load target");
    };
    assert_eq!(name.name.id_str(), "getattr");
    assert!(name.name.is_runtime_name());
    assert_eq!(call.args.len(), 3);
    let CallArgPositional::Positional(InstrUnresolved::Load(name)) = &call.args[0] else {
        panic!("expected first positional _dp_yieldfrom argument");
    };
    assert_eq!(name.name.id_str(), "_dp_yieldfrom");
    let CallArgPositional::Positional(InstrUnresolved::Literal(lit)) = &call.args[1] else {
        panic!("expected second positional string attr argument");
    };
    let Literal::StringLiteral(lit) = lit.clone().into_literal() else {
        panic!("expected string attr literal");
    };
    assert_eq!(lit.value, "close");
    let CallArgPositional::Positional(InstrUnresolved::Load(name)) = &call.args[2] else {
        panic!("expected third positional NONE default argument");
    };
    assert_eq!(name.name.id_str(), "NONE");
    assert!(name.name.is_runtime_name());
}

#[test]
fn current_exception_value_helper_builds_value_attr_lookup() {
    let expr = current_exception_value_expr("_dp_exc");
    let InstrUnresolved::GetAttr(get_attr) = expr else {
        panic!("expected attribute lookup");
    };
    let InstrUnresolved::Load(name) = *get_attr.value else {
        panic!("expected value load on the left");
    };
    assert_eq!(name.name.id_str(), "_dp_exc");
    let InstrUnresolved::Literal(lit) = *get_attr.attr else {
        panic!("expected literal attr name");
    };
    let Literal::StringLiteral(lit) = lit.into_literal() else {
        panic!("expected string attr literal");
    };
    assert_eq!(lit.value, "value");
}

#[test]
fn resume_closure_bindings_keep_internal_eval_state_on_runtime_binding_path() {
    let layout = StorageLayout {
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![
            ClosureSlot {
                logical_name: "total".to_string(),
                storage_name: "_dp_cell_total".to_string(),
                init: ClosureInit::Deferred,
            },
            ClosureSlot {
                logical_name: "_dp_eval_1".to_string(),
                storage_name: "_dp_cell__dp_eval_1".to_string(),
                init: ClosureInit::Deferred,
            },
            ClosureSlot {
                logical_name: "_dp_eval_2".to_string(),
                storage_name: "_dp_cell__dp_eval_2".to_string(),
                init: ClosureInit::Deferred,
            },
            ClosureSlot {
                logical_name: "_dp_try_exc_0".to_string(),
                storage_name: "_dp_cell__dp_try_exc_0".to_string(),
                init: ClosureInit::EmptyCell,
            },
        ],
        runtime_cells: vec![
            ClosureSlot {
                logical_name: "_dp_yieldfrom".to_string(),
                storage_name: "_dp_cell__dp_yieldfrom".to_string(),
                init: ClosureInit::RuntimeNone,
            },
            ClosureSlot {
                logical_name: "_dp_pc".to_string(),
                storage_name: "_dp_cell__dp_pc".to_string(),
                init: ClosureInit::RuntimePcUnstarted,
            },
        ],
        stack_slots: Vec::new(),
    };

    let scope = generator_resume_source_semantic(&layout);
    let closure_bindings = resume_closure_bindings(
        &scope,
        &[
            "captured".to_string(),
            "total".to_string(),
            "_dp_eval_1".to_string(),
            "_dp_eval_2".to_string(),
            "_dp_yieldfrom".to_string(),
            "_dp_pc".to_string(),
            "_dp_try_exc_0".to_string(),
        ],
    );

    assert_eq!(
        closure_bindings.runtime_state_bindings,
        vec![
            ("captured".to_string(), "_dp_cell_captured".to_string()),
            ("total".to_string(), "_dp_cell_total".to_string()),
            ("_dp_eval_1".to_string(), "_dp_cell__dp_eval_1".to_string()),
            ("_dp_eval_2".to_string(), "_dp_cell__dp_eval_2".to_string()),
            (
                "_dp_yieldfrom".to_string(),
                "_dp_cell__dp_yieldfrom".to_string()
            ),
            ("_dp_pc".to_string(), "_dp_cell__dp_pc".to_string()),
            (
                "_dp_try_exc_0".to_string(),
                "_dp_cell__dp_try_exc_0".to_string()
            ),
        ]
    );
}

#[test]
fn persistent_generator_state_order_omits_resume_abi_params() {
    let layout = StorageLayout {
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
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
    };

    assert_eq!(
        persistent_generator_state_order(&layout),
        vec![
            "captured".to_string(),
            "total".to_string(),
            "_dp_pc".to_string(),
        ]
    );
}

#[test]
fn build_blockpy_storage_layout_classifies_capture_local_and_runtime_cells() {
    let scope = generator_test_semantic();
    let layout = build_blockpy_storage_layout(
        &scope,
        &["arg".to_string()],
        &[
            "arg".to_string(),
            "captured".to_string(),
            "_dp_yieldfrom".to_string(),
            "_dp_pc".to_string(),
            "_dp_throw_context".to_string(),
            "_dp_try_exc_0".to_string(),
        ],
        &["captured".to_string()],
        &HashSet::from(["_dp_try_exc_0".to_string()]),
    );

    assert_eq!(
        layout
            .freevars
            .iter()
            .map(|slot| (slot.logical_name.as_str(), slot.storage_name.as_str()))
            .collect::<Vec<_>>(),
        vec![("captured", "_dp_cell_captured")]
    );
    assert_eq!(
        layout
            .cellvars
            .iter()
            .map(|slot| (
                slot.logical_name.as_str(),
                slot.storage_name.as_str(),
                &slot.init
            ))
            .collect::<Vec<_>>(),
        vec![
            ("arg", "_dp_cell_arg", &ClosureInit::Parameter),
            (
                "_dp_try_exc_0",
                "_dp_cell__dp_try_exc_0",
                &ClosureInit::EmptyCell
            ),
        ]
    );
    assert_eq!(
        layout
            .runtime_cells
            .iter()
            .map(|slot| (
                slot.logical_name.as_str(),
                slot.storage_name.as_str(),
                &slot.init
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                "_dp_yieldfrom",
                "_dp_cell__dp_yieldfrom",
                &ClosureInit::RuntimeNone
            ),
            (
                "_dp_pc",
                "_dp_cell__dp_pc",
                &ClosureInit::RuntimePcUnstarted
            ),
            (
                "_dp_throw_context",
                "_dp_cell__dp_throw_context",
                &ClosureInit::RuntimeNone
            ),
        ]
    );
}

#[test]
fn term_conversion_to_no_yield_rejects_nested_yield() {
    let term = BlockTerm::Return(core_call_expr_with_meta(
        core_load_with_yield("f"),
        ast::AtomicNodeIndex::default(),
        TextRange::default(),
        vec![CallArgPositional::Positional(InstrWithYield::Yield(
            Yield::new(core_load_with_yield("x")).with_meta(Meta::default()),
        ))],
        Vec::new(),
    ));

    let mut mapper = ErrOnYield;
    assert!(mapper.try_map_term(term).is_err());
}

#[test]
fn build_blockpy_storage_layout_uses_semantic_classcell_storage_mapping() {
    let mut scope = generator_test_semantic();
    scope.insert_binding(
        "__class__",
        BindingKind::Cell(CellBindingKind::Owner),
        false,
        Some("_dp_classcell".to_string()),
    );

    let layout = build_blockpy_storage_layout(
        &scope,
        &[],
        &["__class__".to_string()],
        &[],
        &HashSet::new(),
    );

    assert_eq!(
        layout
            .cellvars
            .iter()
            .map(|slot| (slot.logical_name.as_str(), slot.storage_name.as_str()))
            .collect::<Vec<_>>(),
        vec![("__class__", "_dp_classcell")]
    );
}

#[test]
fn builds_closure_backed_generator_factory_block() {
    let layout = StorageLayout {
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![ClosureSlot {
            logical_name: "x".to_string(),
            storage_name: "_dp_cell_x".to_string(),
            init: ClosureInit::Parameter,
        }],
        runtime_cells: vec![
            ClosureSlot {
                logical_name: "_dp_pc".to_string(),
                storage_name: "_dp_cell__dp_pc".to_string(),
                init: ClosureInit::RuntimePcUnstarted,
            },
            ClosureSlot {
                logical_name: "_dp_throw_context".to_string(),
                storage_name: "_dp_cell__dp_throw_context".to_string(),
                init: ClosureInit::RuntimeNone,
            },
        ],
        stack_slots: Vec::new(),
    };

    let block = build_closure_backed_generator_factory_block(
        "_dp_bb_demo_factory",
        &FunctionName::new("gen", "gen", "gen", "gen"),
        RuntimeFunctionId::from_raw_parts(0, 1),
        &[
            "_dp_cell_captured".to_string(),
            "_dp_cell_x".to_string(),
            "_dp_cell__dp_pc".to_string(),
        ],
        &layout,
        false,
        false,
    );

    assert_eq!(block.label, BlockLabel::from_index(0));
    assert!(block.body.is_empty(), "{block:?}");
    assert!(matches!(block.term, BlockTerm::Return(_)));
}

#[test]
fn resume_closure_bindings_use_semantic_capture_sources_for_cell_backed_state() {
    let layout = StorageLayout {
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![ClosureSlot {
            logical_name: "total".to_string(),
            storage_name: "_dp_cell_total".to_string(),
            init: ClosureInit::Deferred,
        }],
        runtime_cells: vec![
            ClosureSlot {
                logical_name: "_dp_pc".to_string(),
                storage_name: "_dp_cell__dp_pc".to_string(),
                init: ClosureInit::RuntimePcUnstarted,
            },
            ClosureSlot {
                logical_name: "_dp_throw_context".to_string(),
                storage_name: "_dp_cell__dp_throw_context".to_string(),
                init: ClosureInit::RuntimeNone,
            },
        ],
        stack_slots: Vec::new(),
    };

    let scope = generator_resume_source_semantic(&layout);
    let closure_bindings = resume_closure_bindings(
        &scope,
        &[
            "captured".to_string(),
            "total".to_string(),
            "_dp_pc".to_string(),
            "_dp_throw_context".to_string(),
        ],
    );

    assert_eq!(
        closure_bindings.runtime_state_bindings,
        vec![
            ("captured".to_string(), "_dp_cell_captured".to_string()),
            ("total".to_string(), "_dp_cell_total".to_string()),
            ("_dp_pc".to_string(), "_dp_cell__dp_pc".to_string()),
            (
                "_dp_throw_context".to_string(),
                "_dp_cell__dp_throw_context".to_string()
            ),
        ]
    );
}

#[test]
fn resume_closure_bindings_use_logical_names_for_shared_storage() {
    let layout = StorageLayout {
        freevars: vec![ClosureSlot {
            logical_name: "j".to_string(),
            storage_name: "_dp_cell_j".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![],
        runtime_cells: vec![
            ClosureSlot {
                logical_name: "_dp_pc".to_string(),
                storage_name: "_dp_cell__dp_pc".to_string(),
                init: ClosureInit::RuntimePcUnstarted,
            },
            ClosureSlot {
                logical_name: "_dp_throw_context".to_string(),
                storage_name: "_dp_cell__dp_throw_context".to_string(),
                init: ClosureInit::RuntimeNone,
            },
        ],
        stack_slots: Vec::new(),
    };

    let scope = generator_resume_source_semantic(&layout);
    let closure_bindings = resume_closure_bindings(
        &scope,
        &[
            "j".to_string(),
            "_dp_pc".to_string(),
            "_dp_throw_context".to_string(),
        ],
    );

    assert_eq!(
        closure_bindings.runtime_state_bindings,
        vec![
            ("j".to_string(), "_dp_cell_j".to_string()),
            ("_dp_pc".to_string(), "_dp_cell__dp_pc".to_string()),
            (
                "_dp_throw_context".to_string(),
                "_dp_cell__dp_throw_context".to_string()
            ),
        ]
    );
}

#[test]
fn resume_semantic_marks_generator_state_as_cell_captures() {
    let layout = StorageLayout {
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![ClosureSlot {
            logical_name: "total".to_string(),
            storage_name: "_dp_cell_total".to_string(),
            init: ClosureInit::Deferred,
        }],
        runtime_cells: vec![
            ClosureSlot {
                logical_name: "_dp_pc".to_string(),
                storage_name: "_dp_cell__dp_pc".to_string(),
                init: ClosureInit::RuntimePcUnstarted,
            },
            ClosureSlot {
                logical_name: "_dp_throw_context".to_string(),
                storage_name: "_dp_cell__dp_throw_context".to_string(),
                init: ClosureInit::RuntimeNone,
            },
        ],
        stack_slots: Vec::new(),
    };
    let mut scope = CallableScopeInfo {
        names: FunctionName::new("gen_resume", "_dp_resume", "gen", "gen"),
        scope_kind: CallableScopeKind::Function,
        ..Default::default()
    };
    for slot in layout
        .freevars
        .iter()
        .chain(layout.cellvars.iter())
        .chain(layout.runtime_cells.iter())
    {
        scope.insert_binding(
            slot.logical_name.clone(),
            BindingKind::Cell(CellBindingKind::Capture),
            is_internal_symbol(slot.logical_name.as_str()),
            None,
        );
    }

    assert_eq!(scope.names.bind_name, "gen_resume");
    assert_eq!(
        scope.binding_kind("captured"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.binding_kind("total"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.binding_kind("_dp_pc"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.binding_kind("_dp_throw_context"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_pc"),
        BindingKind::Cell(CellBindingKind::Capture)
    );
    assert_eq!(
        scope.effective_binding("_dp_pc", BindingPurpose::Load),
        Some(crate::block_py::EffectiveBinding::Cell(
            CellBindingKind::Capture
        ))
    );
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_self"),
        BindingKind::Local
    );
}

#[test]
fn resume_semantic_overlay_marks_runtime_and_logical_state_for_standard_name_binding() {
    let layout = StorageLayout {
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![
            ClosureSlot {
                logical_name: "total".to_string(),
                storage_name: "_dp_cell_total".to_string(),
                init: ClosureInit::Deferred,
            },
            ClosureSlot {
                logical_name: "_dp_eval_1".to_string(),
                storage_name: "_dp_cell__dp_eval_1".to_string(),
                init: ClosureInit::Deferred,
            },
            ClosureSlot {
                logical_name: "_dp_eval_2".to_string(),
                storage_name: "_dp_cell__dp_eval_2".to_string(),
                init: ClosureInit::Deferred,
            },
            ClosureSlot {
                logical_name: "_dp_try_exc_0".to_string(),
                storage_name: "_dp_cell__dp_try_exc_0".to_string(),
                init: ClosureInit::EmptyCell,
            },
        ],
        runtime_cells: vec![
            ClosureSlot {
                logical_name: "_dp_yieldfrom".to_string(),
                storage_name: "_dp_cell__dp_yieldfrom".to_string(),
                init: ClosureInit::RuntimeNone,
            },
            ClosureSlot {
                logical_name: "_dp_throw_context".to_string(),
                storage_name: "_dp_cell__dp_throw_context".to_string(),
                init: ClosureInit::RuntimeNone,
            },
            ClosureSlot {
                logical_name: "_dp_pc".to_string(),
                storage_name: "_dp_cell__dp_pc".to_string(),
                init: ClosureInit::RuntimePcUnstarted,
            },
        ],
        stack_slots: Vec::new(),
    };
    let semantic_for_bindings = generator_resume_source_semantic(&layout);
    let closure_bindings = resume_closure_bindings(
        &semantic_for_bindings,
        &[
            "captured".to_string(),
            "total".to_string(),
            "_dp_eval_1".to_string(),
            "_dp_eval_2".to_string(),
            "_dp_yieldfrom".to_string(),
            "_dp_throw_context".to_string(),
            "_dp_pc".to_string(),
            "_dp_try_exc_0".to_string(),
        ],
    );
    let mut scope = CallableScopeInfo {
        names: FunctionName::new("gen_resume", "_dp_resume", "gen", "gen"),
        scope_kind: CallableScopeKind::Function,
        ..Default::default()
    };

    augment_resume_semantic_for_standard_name_binding(&mut scope, &closure_bindings);

    assert_eq!(
        scope.binding_kind("total"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.binding_kind("_dp_pc"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.resolved_load_binding_kind("total"),
        BindingKind::Cell(CellBindingKind::Capture)
    );
    assert_eq!(scope.cell_storage_name("total"), "total");
    assert_eq!(scope.cell_capture_source_name("total"), "_dp_cell_total");
    assert_eq!(
        scope.binding_kind("_dp_eval_1"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_eval_1"),
        BindingKind::Cell(CellBindingKind::Capture)
    );
    assert_eq!(scope.cell_storage_name("_dp_eval_1"), "_dp_eval_1");
    assert_eq!(
        scope.cell_capture_source_name("_dp_eval_1"),
        "_dp_cell__dp_eval_1"
    );
    assert_eq!(
        scope.binding_kind("_dp_eval_2"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_eval_2"),
        BindingKind::Cell(CellBindingKind::Capture)
    );
    assert_eq!(scope.cell_storage_name("_dp_eval_2"), "_dp_eval_2");
    assert_eq!(
        scope.cell_capture_source_name("_dp_eval_2"),
        "_dp_cell__dp_eval_2"
    );
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_pc"),
        BindingKind::Cell(CellBindingKind::Capture)
    );
    assert_eq!(scope.cell_storage_name("_dp_pc"), "_dp_pc");
    assert_eq!(scope.cell_capture_source_name("_dp_pc"), "_dp_cell__dp_pc");
    assert_eq!(
        scope.binding_kind("_dp_yieldfrom"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_yieldfrom"),
        BindingKind::Cell(CellBindingKind::Capture)
    );
    assert_eq!(scope.cell_storage_name("_dp_yieldfrom"), "_dp_yieldfrom");
    assert_eq!(
        scope.cell_capture_source_name("_dp_yieldfrom"),
        "_dp_cell__dp_yieldfrom"
    );
    assert_eq!(
        scope.binding_kind("_dp_throw_context"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_throw_context"),
        BindingKind::Cell(CellBindingKind::Capture)
    );
    assert_eq!(
        scope.cell_storage_name("_dp_throw_context"),
        "_dp_throw_context"
    );
    assert_eq!(
        scope.cell_capture_source_name("_dp_throw_context"),
        "_dp_cell__dp_throw_context"
    );
    assert_eq!(
        scope.binding_kind("_dp_try_exc_0"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_try_exc_0"),
        BindingKind::Cell(CellBindingKind::Capture)
    );
    assert_eq!(scope.cell_storage_name("_dp_try_exc_0"), "_dp_try_exc_0");
    assert_eq!(
        scope.cell_capture_source_name("_dp_try_exc_0"),
        "_dp_cell__dp_try_exc_0"
    );
}
