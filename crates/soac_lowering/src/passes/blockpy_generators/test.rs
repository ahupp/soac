use super::{
    augment_resume_semantic_for_standard_name_binding, build_blockpy_storage_layout,
    current_exception_value_expr, is_name_not_none_test, resume_closure_bindings,
    resume_closure_state_order, yield_from_method_lookup_expr, yield_from_send_expr, ErrOnYield,
};
use crate::block_py::{
    core_call_expr_with_meta, BinOpKind, BindingKind, BindingPurpose, Block, BlockArg, BlockEdge,
    BlockLabel, BlockParam, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm,
    CallArgPositional, CallableScopeInfo, CallableScopeKind, CellBindingKind, ChildVisitable,
    ClosureInit, ClosureSlot, FunctionKind, FunctionName, GeneratorControlRole,
    GeneratorResumeDelivery, GeneratorResumeParamRole, HandledExceptionContext, HasBlockContext,
    HasMeta, InstrResolved, InstrUnresolved, InstrWithConstantNone, InstrWithYield, Literal, Meta,
    ModuleShape, NameLike, PreservedLocation, PreservedSlot, PreservedSlotStorage,
    RaiseDisposition, RuntimeName, StorageLayout, StoreLifetime, TryMapTerm, UnaryOpKind, WithMeta,
    Yield,
};
use crate::pass_tracker::LoweringPassTrackerInternalExt;
use crate::passes::ast_to_ast::scope_helpers::is_internal_symbol;
use crate::passes::ResolvedStorageModuleShape;
use crate::template::py_expr;
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

fn generator_test_state() -> super::ResumeLoweringState {
    let result = crate::lower_python_to_blockpy_for_testing(
        "def gen(_dp_yieldfrom, _dp_send_value):\n    yield (_dp_yieldfrom, _dp_send_value)\n",
    )
    .expect("source-spelling fixture should lower");
    let module = result
        .pass_tracker
        .get::<BlockPyModule<crate::passes::CoreModuleShapeWithYield>>("core_blockpy_with_yield")
        .expect("actual pre-generator producer input");
    let function = suspension_function(module, "gen");
    super::ResumeLoweringState::new(
        function.name_gen.share(),
        function.kind,
        Default::default(),
        Vec::new(),
        super::GeneratorBindingNames::for_callable(function),
    )
}

#[test]
fn delegation_classification_does_not_enter_a_python_exception_handler() {
    let result = crate::lower_python_to_blockpy_for_testing(
        "def delegated(wait):\n    return (yield from wait)\n\nasync def awaited(wait):\n    return await wait\n",
    )
    .expect("delegated source lowers");
    let core = result.pass_tracker.pass_core_blockpy().expect("core pass");
    for name in ["delegated", "awaited"] {
        let function = suspension_function(core, name);
        let internal_handlers = function
            .blocks
            .iter()
            .filter(|block| {
                block.exception_param().is_some()
                    && block.extra.handled_exception != HandledExceptionContext::Terminal
            })
            .collect::<Vec<_>>();
        assert!(
            !internal_handlers.is_empty(),
            "delegation classifies a raised value"
        );
        // Neither original body contains an except suite. The synthetic
        // StopIteration test, value extraction, and escaping-error forwarding
        // transport a raised exception without publishing it as sys.exception.
        let mut propagated = 0;
        let mut injected = 0;
        for block in internal_handlers {
            assert_eq!(
                block.extra.handled_exception,
                HandledExceptionContext::Preserve,
                "{name}: compiler-only delegation block {:?}",
                block.label,
            );
            if let BlockTerm::Raise(raise) = &block.term {
                match raise.disposition {
                    RaiseDisposition::PropagateNormalized => propagated += 1,
                    RaiseDisposition::SourceNormalized => injected += 1,
                    other => panic!("unexpected delegated escape disposition: {other:?}"),
                }
            }
        }
        assert_eq!((propagated, injected), (1, 1));
    }
}

fn preserved_slot(logical_name: &str, storage_name: &str, init: ClosureInit) -> PreservedSlot {
    let storage = match init {
        ClosureInit::RuntimePcUnstarted
        | ClosureInit::RuntimeAbruptKindFallthrough
        | ClosureInit::RuntimeZero => PreservedSlotStorage::I64,
        ClosureInit::InheritedCapture
        | ClosureInit::Parameter
        | ClosureInit::EmptyCell
        | ClosureInit::RuntimeNone
        | ClosureInit::Deferred => PreservedSlotStorage::PyObjectOrNull,
    };
    PreservedSlot {
        generator_control: None,
        logical_name: logical_name.to_string(),
        storage_name: storage_name.to_string(),
        init,
        storage,
    }
}

fn preserved_cell_slot(logical_name: &str, storage_name: &str, init: ClosureInit) -> PreservedSlot {
    PreservedSlot {
        generator_control: None,
        logical_name: logical_name.to_string(),
        storage_name: storage_name.to_string(),
        init,
        storage: PreservedSlotStorage::PyCellObject,
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
    let state = generator_test_state();
    let expr = yield_from_send_expr(&state);
    let InstrUnresolved::Call(call) = expr else {
        panic!("expected call expression");
    };
    let InstrUnresolved::GetAttr(get_attr) = *call.func else {
        panic!("expected getattr call target");
    };
    let InstrUnresolved::Load(name) = *get_attr.value else {
        panic!("expected send receiver load");
    };
    assert_eq!(
        name.name.id_str(),
        state.bindings.control(GeneratorControlRole::Delegate)
    );
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
    assert_eq!(
        name.name.id_str(),
        state
            .bindings
            .parameter(GeneratorResumeParamRole::SendValue)
    );
}

#[test]
fn yield_from_lookup_helper_builds_getattr_call_shape() {
    let state = generator_test_state();
    let expr = yield_from_method_lookup_expr(&state, "close");
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
    assert_eq!(
        name.name.id_str(),
        state.bindings.control(GeneratorControlRole::Delegate)
    );
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
fn resume_closure_bindings_keep_only_outer_captures_on_closure_path() {
    let layout = StorageLayout {
        class_bindings: None,
        block_parameter_roles: Vec::new(),
        generator_resume_abi: None,
        expression_temporaries: Vec::new(),
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![],
        preserved_slots: vec![
            preserved_cell_slot("total", "_dp_cell_total", ClosureInit::EmptyCell),
            preserved_cell_slot("_dp_eval_1", "_dp_cell__dp_eval_1", ClosureInit::EmptyCell),
            preserved_cell_slot("_dp_eval_2", "_dp_cell__dp_eval_2", ClosureInit::EmptyCell),
            preserved_cell_slot(
                "_dp_try_exc_0",
                "_dp_cell__dp_try_exc_0",
                ClosureInit::EmptyCell,
            ),
            preserved_slot(
                "_dp_yieldfrom",
                "_dp_cell__dp_yieldfrom",
                ClosureInit::RuntimeNone,
            ),
            preserved_slot("_dp_pc", "_dp_cell__dp_pc", ClosureInit::RuntimePcUnstarted),
        ],
        stack_slots: Vec::new(),
    };

    let scope = generator_resume_source_semantic(&layout);
    let closure_bindings = resume_closure_bindings(&scope, &resume_closure_state_order(&layout));

    assert_eq!(
        closure_bindings.runtime_state_bindings,
        vec![("captured".to_string(), "_dp_cell_captured".to_string())]
    );
}

#[test]
fn resume_closure_state_order_omits_preserved_slots() {
    let layout = StorageLayout {
        class_bindings: None,
        block_parameter_roles: Vec::new(),
        generator_resume_abi: None,
        expression_temporaries: Vec::new(),
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![],
        preserved_slots: vec![
            preserved_cell_slot("total", "_dp_cell_total", ClosureInit::EmptyCell),
            preserved_slot("_dp_pc", "_dp_cell__dp_pc", ClosureInit::RuntimePcUnstarted),
        ],
        stack_slots: Vec::new(),
    };

    assert_eq!(
        resume_closure_state_order(&layout),
        vec!["captured".to_string()]
    );
}

#[test]
fn build_blockpy_storage_layout_classifies_capture_local_and_preserved_slots() {
    let mut scope = generator_test_semantic();
    scope.insert_binding(
        "z_captured",
        BindingKind::Cell(CellBindingKind::Capture),
        false,
        None,
    );
    let layout = build_blockpy_storage_layout(
        &scope,
        &["arg".to_string()],
        &[
            "arg".to_string(),
            "z_captured".to_string(),
            "_dp_yieldfrom".to_string(),
            "captured".to_string(),
            "_dp_pc".to_string(),
            "_dp_is_closed".to_string(),
            "_dp_try_exc_0".to_string(),
        ],
        &["z_captured".to_string(), "captured".to_string()],
        &HashSet::new(),
        &HashSet::from(["_dp_try_exc_0".to_string()]),
        &[
            (GeneratorControlRole::Delegate, "_dp_yieldfrom".to_owned()),
            (GeneratorControlRole::ProgramCounter, "_dp_pc".to_owned()),
            (GeneratorControlRole::IsClosed, "_dp_is_closed".to_owned()),
        ],
        &HashSet::new(),
    );

    assert_eq!(
        layout
            .freevars
            .iter()
            .map(|slot| (slot.logical_name.as_str(), slot.storage_name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("captured", "_dp_cell_captured"),
            ("z_captured", "_dp_cell_z_captured"),
        ]
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
        Vec::<(&str, &str, &ClosureInit)>::new()
    );
    assert_eq!(
        layout
            .preserved_slots
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
                "_dp_is_closed",
                "_dp_cell__dp_is_closed",
                &ClosureInit::RuntimeZero
            ),
            (
                "_dp_try_exc_0",
                "_dp_cell__dp_try_exc_0",
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
        &HashSet::from(["_dp_classcell".to_string()]),
        &HashSet::new(),
        &[],
        &HashSet::new(),
    );

    assert_eq!(
        layout
            .preserved_slots
            .iter()
            .map(|slot| {
                (
                    slot.logical_name.as_str(),
                    slot.storage_name.as_str(),
                    slot.storage,
                )
            })
            .collect::<Vec<_>>(),
        vec![(
            "__class__",
            "_dp_classcell",
            PreservedSlotStorage::PyCellObject
        )]
    );
}

#[test]
fn resume_closure_bindings_use_semantic_capture_sources_for_cell_backed_state() {
    let layout = StorageLayout {
        class_bindings: None,
        block_parameter_roles: Vec::new(),
        generator_resume_abi: None,
        expression_temporaries: Vec::new(),
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![],
        preserved_slots: vec![
            preserved_cell_slot("total", "_dp_cell_total", ClosureInit::EmptyCell),
            preserved_slot("_dp_pc", "_dp_cell__dp_pc", ClosureInit::RuntimePcUnstarted),
            preserved_slot(
                "_dp_yieldfrom",
                "_dp_cell__dp_yieldfrom",
                ClosureInit::RuntimeNone,
            ),
        ],
        stack_slots: Vec::new(),
    };

    let scope = generator_resume_source_semantic(&layout);
    let closure_bindings = resume_closure_bindings(&scope, &resume_closure_state_order(&layout));

    assert_eq!(
        closure_bindings.runtime_state_bindings,
        vec![("captured".to_string(), "_dp_cell_captured".to_string())]
    );
}

#[test]
fn resume_closure_bindings_use_logical_names_for_shared_storage() {
    let layout = StorageLayout {
        class_bindings: None,
        block_parameter_roles: Vec::new(),
        generator_resume_abi: None,
        expression_temporaries: Vec::new(),
        freevars: vec![ClosureSlot {
            logical_name: "j".to_string(),
            storage_name: "_dp_cell_j".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![],
        preserved_slots: vec![
            preserved_slot("_dp_pc", "_dp_cell__dp_pc", ClosureInit::RuntimePcUnstarted),
            preserved_slot(
                "_dp_yieldfrom",
                "_dp_cell__dp_yieldfrom",
                ClosureInit::RuntimeNone,
            ),
        ],
        stack_slots: Vec::new(),
    };

    let scope = generator_resume_source_semantic(&layout);
    let closure_bindings = resume_closure_bindings(&scope, &resume_closure_state_order(&layout));

    assert_eq!(
        closure_bindings.runtime_state_bindings,
        vec![("j".to_string(), "_dp_cell_j".to_string())]
    );
}

#[test]
fn resume_semantic_marks_only_closure_state_as_cell_captures() {
    let layout = StorageLayout {
        class_bindings: None,
        block_parameter_roles: Vec::new(),
        generator_resume_abi: None,
        expression_temporaries: Vec::new(),
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![],
        preserved_slots: vec![
            preserved_cell_slot("total", "_dp_cell_total", ClosureInit::EmptyCell),
            preserved_slot("_dp_pc", "_dp_cell__dp_pc", ClosureInit::RuntimePcUnstarted),
            preserved_slot(
                "_dp_yieldfrom",
                "_dp_cell__dp_yieldfrom",
                ClosureInit::RuntimeNone,
            ),
        ],
        stack_slots: Vec::new(),
    };
    let mut scope = CallableScopeInfo {
        names: FunctionName::new("gen", "gen", "gen", "gen"),
        scope_kind: CallableScopeKind::Function,
        ..Default::default()
    };
    for slot in &layout.freevars {
        scope.insert_binding(
            slot.logical_name.clone(),
            BindingKind::Cell(CellBindingKind::Capture),
            is_internal_symbol(slot.logical_name.as_str()),
            None,
        );
    }

    assert_eq!(scope.names.bind_name, "gen");
    assert_eq!(
        scope.binding_kind("captured"),
        Some(BindingKind::Cell(CellBindingKind::Capture))
    );
    assert_eq!(scope.binding_kind("total"), None);
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_pc"),
        BindingKind::Local
    );
    assert_eq!(
        scope.effective_binding("_dp_pc", BindingPurpose::Load),
        None
    );
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_self"),
        BindingKind::Local
    );
}

#[test]
fn resume_semantic_overlay_marks_only_closure_state_for_standard_name_binding() {
    let layout = StorageLayout {
        class_bindings: None,
        block_parameter_roles: Vec::new(),
        generator_resume_abi: None,
        expression_temporaries: Vec::new(),
        freevars: vec![ClosureSlot {
            logical_name: "captured".to_string(),
            storage_name: "_dp_cell_captured".to_string(),
            init: ClosureInit::InheritedCapture,
        }],
        cellvars: vec![],
        preserved_slots: vec![
            preserved_cell_slot("total", "_dp_cell_total", ClosureInit::EmptyCell),
            preserved_cell_slot("_dp_eval_1", "_dp_cell__dp_eval_1", ClosureInit::EmptyCell),
            preserved_cell_slot("_dp_eval_2", "_dp_cell__dp_eval_2", ClosureInit::EmptyCell),
            preserved_cell_slot(
                "_dp_try_exc_0",
                "_dp_cell__dp_try_exc_0",
                ClosureInit::EmptyCell,
            ),
            preserved_slot(
                "_dp_yieldfrom",
                "_dp_cell__dp_yieldfrom",
                ClosureInit::RuntimeNone,
            ),
            preserved_slot("_dp_pc", "_dp_cell__dp_pc", ClosureInit::RuntimePcUnstarted),
        ],
        stack_slots: Vec::new(),
    };
    let semantic_for_bindings = generator_resume_source_semantic(&layout);
    let closure_bindings =
        resume_closure_bindings(&semantic_for_bindings, &resume_closure_state_order(&layout));
    let mut scope = CallableScopeInfo {
        names: FunctionName::new("gen", "gen", "gen", "gen"),
        scope_kind: CallableScopeKind::Function,
        ..Default::default()
    };

    augment_resume_semantic_for_standard_name_binding(&mut scope, &closure_bindings);

    assert_eq!(scope.binding_kind("total"), None);
    assert_eq!(
        scope.resolved_load_binding_kind("total"),
        BindingKind::Global
    );
    assert_eq!(scope.cell_storage_name("total"), "_dp_cell_total");
    assert_eq!(scope.cell_capture_source_name("total"), "_dp_cell_total");
    assert_eq!(scope.binding_kind("_dp_eval_1"), None);
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_eval_1"),
        BindingKind::Local
    );
    assert_eq!(scope.cell_storage_name("_dp_eval_1"), "_dp_cell__dp_eval_1");
    assert_eq!(
        scope.cell_capture_source_name("_dp_eval_1"),
        "_dp_cell__dp_eval_1"
    );
    assert_eq!(scope.binding_kind("_dp_eval_2"), None);
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_eval_2"),
        BindingKind::Local
    );
    assert_eq!(scope.cell_storage_name("_dp_eval_2"), "_dp_cell__dp_eval_2");
    assert_eq!(
        scope.cell_capture_source_name("_dp_eval_2"),
        "_dp_cell__dp_eval_2"
    );
    assert_eq!(scope.binding_kind("_dp_pc"), None);
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_pc"),
        BindingKind::Local
    );
    assert_eq!(scope.binding_kind("_dp_yieldfrom"), None);
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_yieldfrom"),
        BindingKind::Local
    );
    assert_eq!(scope.binding_kind("_dp_try_exc_0"), None);
    assert_eq!(
        scope.resolved_load_binding_kind("_dp_try_exc_0"),
        BindingKind::Local
    );
    assert_eq!(
        scope.cell_storage_name("_dp_try_exc_0"),
        "_dp_cell__dp_try_exc_0"
    );
    assert_eq!(
        scope.cell_capture_source_name("_dp_try_exc_0"),
        "_dp_cell__dp_try_exc_0"
    );
}

fn suspension_function<'a, P: ModuleShape>(
    module: &'a BlockPyModule<P>,
    name: &str,
) -> &'a BlockPyFunction<P> {
    module
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == name)
        .unwrap_or_else(|| panic!("missing suspension fixture function {name}"))
}

fn suspension_edges<P: ModuleShape>(
    function: &BlockPyFunction<P>,
) -> Vec<(BlockLabel, BlockLabel)> {
    function
        .blocks
        .iter()
        .filter_map(|block| {
            block
                .extra
                .block_context()
                .suspension_resume
                .map(|target| (block.label, target))
        })
        .collect()
}

#[test]
fn suspension_resume_edges_cover_yield_delegation_and_await() {
    let result = crate::lower_python_to_blockpy_for_testing(
        r#"
def plain():
    yield 1

def assigned():
    value = yield 1
    return value

def returned():
    return (yield 1)

def delegated(values):
    yield from values

def assigned_delegation(values):
    value = yield from values
    return value

def returned_delegation(values):
    return (yield from values)

async def awaited(value):
    return await value

async def async_generator():
    yield 1

def ordinary():
    return 1
"#,
    )
    .expect("suspension fixture should lower");
    let core = result.pass_tracker.pass_core_blockpy().expect("core pass");
    let resolved = result
        .pass_tracker
        .pass_name_binding()
        .expect("name binding pass");
    for name in [
        "plain",
        "assigned",
        "returned",
        "delegated",
        "assigned_delegation",
        "returned_delegation",
        "awaited",
        "async_generator",
    ] {
        let function = suspension_function(core, name);
        super::validate_suspension_resumes(function).expect("valid producer edges");
        let edges = suspension_edges(function);
        assert_eq!(
            edges.len(),
            1,
            "{name} should declare its actual suspension"
        );
        let BlockTerm::BranchTable(dispatch) = &function.entry_block().term else {
            panic!("{name} should have the explicit resume dispatcher");
        };
        for (source, target) in &edges {
            let block = function
                .blocks
                .iter()
                .find(|block| block.label == *source)
                .unwrap();
            assert!(matches!(block.term, BlockTerm::Return(_)));
            assert!(
                dispatch.targets.contains(target),
                "metadata must select the actual runtime resume entry"
            );
            let entry = function
                .blocks
                .iter()
                .find(|block| block.label == *target)
                .unwrap();
            assert!(
                entry.params.is_empty(),
                "resume entry cannot use the previous activation's locals"
            );
        }
        assert_eq!(
            function
                .blocks
                .iter()
                .filter(|block| matches!(block.term, BlockTerm::Return(_)))
                .count(),
            edges.len(),
            "{name} must not confuse completion with suspension",
        );
        let bound = suspension_function(resolved, name);
        super::validate_suspension_resumes(bound).expect("valid bound edges");
        assert_eq!(
            suspension_edges(bound),
            edges,
            "name binding must preserve explicit resume identity"
        );
    }
    assert!(suspension_edges(suspension_function(core, "ordinary")).is_empty());

    let prepared = result
        .pass_tracker
        .get::<BlockPyModule<ResolvedStorageModuleShape>>("bb_prepared")
        .expect("resolved ownership pass");
    assert!(
        prepared
            .callable_defs
            .iter()
            .all(|function| suspension_edges(function).is_empty()),
        "ownership must consume every marker, including functions with no exception transports",
    );
    assert!(
        result
            .blockpy_module
            .callable_defs
            .iter()
            .all(|function| suspension_edges(function).is_empty()),
        "the optimizer must receive only materialized ownership operations",
    );
}

#[test]
fn suspension_resume_reloads_except_star_transport_before_merge() {
    let result = crate::lower_python_to_blockpy_for_testing(
        r#"
def split_group():
    try:
        raise ExceptionGroup("group", [ValueError(), TypeError()])
    except* ValueError:
        yield 1
"#,
    )
    .expect("suspended except-star fixture should lower");
    let core = result.pass_tracker.pass_core_blockpy().expect("core pass");
    let function = suspension_function(core, "split_group");
    let edges = suspension_edges(function);
    assert_eq!(edges.len(), 1);
    let (source_label, target_label) = edges[0];
    let source = function
        .blocks
        .iter()
        .find(|block| block.label == source_label)
        .unwrap();
    let handlers = source
        .handled_exception_params()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    assert!(
        !handlers.is_empty(),
        "the yielded subgroup must retain its original handler identity"
    );

    let resolved = result
        .pass_tracker
        .pass_name_binding()
        .expect("name binding pass");
    let function = suspension_function(resolved, "split_group");
    let layout = function.storage_layout.as_ref().expect("resolved storage");
    let source = function
        .blocks
        .iter()
        .find(|block| block.label == source_label)
        .unwrap();
    let resume = function
        .blocks
        .iter()
        .find(|block| block.label == target_label)
        .unwrap();
    assert!(resume.params.is_empty());
    let BlockTerm::Jump(edge) = &resume.term else {
        panic!("suspended handler resumes through its explicit reload wrapper");
    };
    assert_ne!(edge.target, source_label);
    for handler in handlers {
        let (index, slot) = layout
            .preserved_slots
            .iter()
            .enumerate()
            .find(|(_, slot)| slot.logical_name == handler)
            .expect("saved handler slot");
        assert_eq!(slot.storage, PreservedSlotStorage::PyObjectOrNull);
        let saved = PreservedLocation(u32::try_from(index).expect("small fixture"));
        assert!(
            source.body.iter().any(|instruction| {
                matches!(instruction, InstrResolved::Store(store)
                if store.name.preserved_location() == Some(saved)
                    && matches!(store.value.as_ref(), InstrResolved::Load(load)
                        if load.name.local_location().is_some() && load.name.id_str() == handler))
            }),
            "the yielding activation must explicitly preserve its incoming handler owner"
        );
        assert!(
            resume.body.iter().any(|instruction| {
                matches!(instruction, InstrResolved::Store(store)
                if store.name.local_location().is_some()
                    && matches!(store.value.as_ref(), InstrResolved::Load(load)
                        if load.name.preserved_location() == Some(saved)))
            }),
            "ownership must see the saved handler read before the except-star continuation"
        );
    }
}

#[test]
fn suspension_resume_validation_rejects_non_yields_and_invalid_entries() {
    let result = crate::lower_python_to_blockpy_for_testing("def gen():\n    yield 1\n")
        .expect("generator fixture");
    let core = result.pass_tracker.pass_core_blockpy().expect("core pass");
    let function = suspension_function(core, "gen");
    let (source, target) = suspension_edges(function)[0];
    let source_index = function
        .blocks
        .iter()
        .position(|block| block.label == source)
        .unwrap();
    let target_index = function
        .blocks
        .iter()
        .position(|block| block.label == target)
        .unwrap();

    let mut malformed = function.clone();
    malformed.kind = FunctionKind::Function;
    assert!(super::validate_suspension_resumes(&malformed).is_err());

    let mut malformed = function.clone();
    malformed.blocks[source_index].term = BlockTerm::Jump(BlockEdge::new(target));
    assert!(super::validate_suspension_resumes(&malformed).is_err());

    let mut malformed = function.clone();
    malformed.blocks[source_index].term =
        BlockTerm::GeneratorReturn(InstrUnresolved::constant_none());
    assert!(super::validate_suspension_resumes(&malformed).is_err());

    let mut malformed = function.clone();
    malformed.blocks[source_index].extra.handled_exception = HandledExceptionContext::Terminal;
    assert!(super::validate_suspension_resumes(&malformed).is_err());

    let mut malformed = function.clone();
    let absent = function
        .blocks
        .iter()
        .map(|block| block.label.index())
        .max()
        .unwrap()
        + 1;
    malformed.blocks[source_index].extra.suspension_resume = Some(BlockLabel::from_index(absent));
    assert!(super::validate_suspension_resumes(&malformed).is_err());

    let mut malformed = function.clone();
    malformed.blocks[target_index].params.push(BlockParam {
        name: "previous_activation_local".to_owned(),
        role: BlockParamRole::Value,
    });
    assert!(super::validate_suspension_resumes(&malformed).is_err());

    let mut malformed = function.clone();
    malformed.blocks[target_index].extra.handled_exception = HandledExceptionContext::Terminal;
    assert!(super::validate_suspension_resumes(&malformed).is_err());

    let mut malformed = function.clone();
    malformed.blocks[target_index].term = BlockTerm::Return(InstrUnresolved::constant_none());
    assert!(super::validate_suspension_resumes(&malformed).is_err());

    let mut malformed = function.clone();
    malformed.blocks[target_index].extra.handled_exception = HandledExceptionContext::Regions;
    assert!(super::validate_suspension_resumes(&malformed).is_err());

    let mut final_module = result.blockpy_module.clone();
    let final_function = final_module
        .callable_defs
        .iter_mut()
        .find(|function| function.names.bind_name == "gen")
        .unwrap();
    let final_entry = final_function.entry_block().label;
    final_function
        .blocks
        .iter_mut()
        .find(|block| matches!(block.term, BlockTerm::Return(_)))
        .expect("yielded return")
        .extra
        .suspension_resume = Some(final_entry);
    assert!(
        crate::block_py::validate::validate_blockpy_module(&final_module).is_err(),
        "analysis-only edges cannot cross the optimizer boundary"
    );
}

#[test]
fn suspension_resume_dense_relabel_keeps_the_explicit_target() {
    let mut blocks = vec![
        Block::new(
            BlockLabel::from_index(40),
            Vec::new(),
            BlockTerm::Return(InstrUnresolved::constant_none()),
            Vec::new(),
            None,
        ),
        Block::new(
            BlockLabel::from_index(10),
            Vec::new(),
            BlockTerm::Jump(BlockEdge::new(BlockLabel::from_index(40))),
            Vec::new(),
            None,
        ),
        Block::new(
            BlockLabel::from_index(70),
            Vec::new(),
            BlockTerm::GeneratorReturn(InstrUnresolved::constant_none()),
            Vec::new(),
            None,
        ),
    ];
    blocks[0].extra.suspension_resume = Some(BlockLabel::from_index(70));
    crate::block_py::cfg::relabel_blockpy_blocks_dense(&mut blocks);
    assert_eq!(
        blocks[0].extra.suspension_resume,
        Some(BlockLabel::from_index(2))
    );
    let BlockTerm::Jump(edge) = &blocks[1].term else {
        panic!("unchanged jump");
    };
    assert_eq!(edge.target, BlockLabel::from_index(0));
    assert!(blocks[1..]
        .iter()
        .all(|block| block.extra.suspension_resume.is_none()));
}

#[test]
fn suspension_resume_wrapper_keeps_zero_parameter_delegation_source_entry() {
    let result = crate::lower_python_to_blockpy_for_testing(
        r#"
def after_handler(values):
    try:
        raise ValueError()
    except ValueError:
        pass
    yield from values
"#,
    )
    .expect("delegation after a handler should lower");
    let core = result.pass_tracker.pass_core_blockpy().expect("core pass");
    let function = suspension_function(core, "after_handler");
    assert!(
        function
            .blocks
            .iter()
            .any(|block| block.handled_exception_params().next().is_some()),
        "fixture must exercise nonempty transport ownership"
    );
    let edges = suspension_edges(function);
    assert_eq!(edges.len(), 1);
    let (_, target_label) = edges[0];
    let target = function
        .blocks
        .iter()
        .find(|block| block.label == target_label)
        .unwrap();
    assert!(target.params.is_empty());
    let BlockTerm::Jump(resume_edge) = &target.term else {
        panic!("even zero-parameter suspension needs a distinct final resume wrapper");
    };
    assert_eq!(
        target.extra.handled_exception,
        HandledExceptionContext::Preserve
    );
    let delegate = function
        .blocks
        .iter()
        .find(|block| block.label == resume_edge.target)
        .unwrap();
    assert_ne!(target.label, delegate.label);
    assert!(
        delegate.params.is_empty(),
        "fixture must cover the zero-parameter path"
    );
    let first_entry = function
        .blocks
        .iter()
        .find(|block| {
            block.body.iter().any(|instr| {
                matches!(instr, InstrUnresolved::Store(store)
                if store.name.id_str() == "_dp_yieldfrom"
                    && core_runtime_call(&store.value, RuntimeName::Iter).is_some())
            })
        })
        .expect("source creates its actual fresh delegate");
    let BlockTerm::Jump(first) = &first_entry.term else {
        panic!("fresh source advance")
    };
    assert_ne!(first.target, delegate.label);
    let advance = core_block_at(function, first.target);
    assert!(
        advance.body.iter().any(|instr| {
            matches!(instr, InstrUnresolved::Store(store)
            if core_runtime_call(&store.value, RuntimeName::Next).is_some())
        }),
        "first source entry must call next without consuming a previous resume packet"
    );
}

fn core_runtime_call(
    expr: &InstrUnresolved,
    runtime_name: RuntimeName,
) -> Option<&crate::block_py::Call<InstrUnresolved>> {
    let InstrUnresolved::Call(call) = expr else {
        return None;
    };
    match call.func.as_ref() {
        InstrUnresolved::Load(load) if load.name.runtime_name_id() == Some(runtime_name) => {
            Some(call)
        }
        _ => None,
    }
}

fn assert_same_exception_edge(left: &Option<BlockEdge>, right: &Option<BlockEdge>) {
    let (Some(left), Some(right)) = (left, right) else {
        panic!("both operations must retain an explicit exception edge");
    };
    assert_eq!(left.target, right.target);
    assert_eq!(left.args.len(), right.args.len());
    for (left, right) in left.args.iter().zip(&right.args) {
        match (left, right) {
            (BlockArg::Name(left), BlockArg::Name(right)) => assert_eq!(left, right),
            (BlockArg::AbruptKind(left), BlockArg::AbruptKind(right)) => assert_eq!(left, right),
            (BlockArg::None, BlockArg::None)
            | (BlockArg::CurrentException, BlockArg::CurrentException) => {}
            _ => panic!("exception edge argument changed from {right:?} to {left:?}"),
        }
    }
}

fn core_is_none(expr: &InstrUnresolved) -> bool {
    matches!(expr, InstrUnresolved::Load(load)
        if load.name.runtime_name_id() == Some(RuntimeName::None))
}

fn core_block_at(
    function: &BlockPyFunction<crate::passes::CoreModuleShape>,
    label: BlockLabel,
) -> &Block<InstrUnresolved> {
    function
        .blocks
        .iter()
        .find(|block| block.label == label)
        .unwrap()
}

fn delivery_dispatches(
    function: &BlockPyFunction<crate::passes::CoreModuleShape>,
) -> Vec<&Block<InstrUnresolved>> {
    function
        .blocks
        .iter()
        .filter(|block| {
            matches!(&block.term, BlockTerm::BranchTable(branch)
            if core_runtime_call(&branch.index, RuntimeName::GeneratorResumeDelivery).is_some())
        })
        .collect()
}

fn injection_call(block: &Block<InstrUnresolved>) -> &crate::block_py::Call<InstrUnresolved> {
    let BlockTerm::Raise(raise) = &block.term else {
        panic!("managed injection must preserve the normalized raised error");
    };
    assert!(raise.disposition.is_normalized());
    let operand = raise.exc.as_ref().expect("explicit normalized operand");
    core_runtime_call(operand, RuntimeName::InjectGeneratorResumeException)
        .expect("native-owned injection call")
}

fn core_block_has_load(block: &Block<InstrUnresolved>, name: &str) -> bool {
    struct Find<'a> {
        name: &'a str,
        found: bool,
    }
    impl crate::block_py::Visit<InstrUnresolved> for Find<'_> {
        fn visit_instr(&mut self, instr: &InstrUnresolved) {
            if matches!(instr, InstrUnresolved::Load(load) if load.name.id_str() == self.name) {
                self.found = true;
            }
            instr.visit_children(self);
        }
    }
    let mut find = Find { name, found: false };
    crate::block_py::walk_block(&mut find, block);
    find.found
}

fn core_jump_path_has_load(
    function: &BlockPyFunction<crate::passes::CoreModuleShape>,
    mut label: BlockLabel,
    name: &str,
) -> bool {
    let mut seen = HashSet::new();
    while seen.insert(label) {
        let block = core_block_at(function, label);
        if core_block_has_load(block, name) {
            return true;
        }
        let BlockTerm::Jump(edge) = &block.term else {
            return false;
        };
        label = edge.target;
    }
    false
}

#[test]
fn managed_resume_injection_owns_and_clears_the_control_operand_before_its_handler() {
    let result = crate::lower_python_to_blockpy_for_testing(
        r#"
def receive():
    try:
        raise ValueError("outer")
    except ValueError:
        try:
            yield "paused"
        except TypeError:
            return "caught"
"#,
    )
    .expect("active source-handler suspension should lower");
    let core = result.pass_tracker.pass_core_blockpy().unwrap();
    let function = suspension_function(core, "receive");
    let dispatches = delivery_dispatches(function);
    assert_eq!(dispatches.len(), 1);
    let dispatch = dispatches[0];
    let BlockTerm::BranchTable(branch) = &dispatch.term else {
        unreachable!()
    };
    let ordinary = core_block_at(
        function,
        branch.targets[GeneratorResumeDelivery::Ordinary as usize],
    );
    assert!(matches!(&ordinary.term, BlockTerm::Raise(raise)
        if raise.disposition == RaiseDisposition::Source
            && matches!(raise.exc.as_ref(), Some(InstrUnresolved::Load(load))
                if load.name.id_str() == "_dp_resume_exc")));
    assert_eq!(
        branch.targets[GeneratorResumeDelivery::YieldFromException as usize],
        branch.default_label
    );
    let injection = core_block_at(
        function,
        branch.targets[GeneratorResumeDelivery::DirectRaise as usize],
    );
    assert!(injection.handled_exception_params().next().is_some());
    assert_eq!(injection.params, dispatch.params);
    assert_same_exception_edge(&injection.exc_edge, &ordinary.exc_edge);
    let [InstrUnresolved::Store(capture), InstrUnresolved::Store(clear), InstrUnresolved::Store(delegate)] =
        injection.body.as_slice()
    else {
        panic!("capture, retire ABI argument, retire delegate before source injection")
    };
    assert!(matches!(capture.lifetime, StoreLifetime::Operand { .. }));
    assert!(matches!(capture.value.as_ref(), InstrUnresolved::Load(load)
        if load.name.id_str() == "_dp_resume_exc"));
    assert_eq!(clear.name.id_str(), "_dp_resume_exc");
    assert!(core_is_none(&clear.value));
    assert_eq!(delegate.name.id_str(), "_dp_yieldfrom");
    assert!(core_is_none(&delegate.value));
    let call = injection_call(injection);
    let [CallArgPositional::Positional(InstrUnresolved::Load(state)), CallArgPositional::Positional(InstrUnresolved::Load(error))] =
        call.args.as_slice()
    else {
        panic!("owned state and exact captured error are the injection operands")
    };
    assert_eq!(state.name.id_str(), "_dp_state");
    assert_eq!(error.name.id_str(), capture.name.id_str());

    let entry = function.blocks.iter().find(|block| {
        matches!(&block.term, BlockTerm::IfTerm(test) if test.then_label == dispatch.label)
    }).expect("only an exceptional resume enters delivery dispatch");
    assert!(entry.body.is_empty());
    let BlockTerm::IfTerm(entry_test) = &entry.term else {
        unreachable!()
    };
    assert!(matches!(&entry_test.test, InstrUnresolved::UnaryOp(not)
        if not.kind == UnaryOpKind::Not
            && matches!(not.operand.as_ref(), InstrUnresolved::BinOp(test)
                if test.kind == BinOpKind::Is
                    && matches!(test.right.as_ref(), InstrUnresolved::Load(load)
                        if load.name.runtime_name_id() == Some(RuntimeName::NoDefault)))));
    assert_ne!(entry_test.else_label, dispatch.label);
    assert!(!core_block_has_load(
        core_block_at(function, entry_test.else_label),
        RuntimeName::GeneratorResumeDelivery.name(),
    ));

    let prepared = result
        .pass_tracker
        .get::<BlockPyModule<ResolvedStorageModuleShape>>("bb_prepared")
        .unwrap();
    let function = suspension_function(prepared, "receive");
    let operand = function
        .blocks
        .iter()
        .flat_map(|block| &block.body)
        .find_map(|instr| match instr {
            InstrResolved::Store(store) if store.name.id_str() == capture.name.id_str() => {
                assert!(matches!(store.lifetime, StoreLifetime::Operand { .. }));
                Some(store.name.location)
            }
            _ => None,
        })
        .expect("the owned control operand must survive name binding");
    struct FindInjection<'a> {
        constants: &'a [InstrResolved],
        operand: crate::block_py::NameLocation,
        found: bool,
    }
    impl crate::block_py::Visit<InstrResolved> for FindInjection<'_> {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            if let InstrResolved::Call(call) = instr {
                let helper = match call.func.as_ref() {
                    InstrResolved::Load(load) => match load.name.location {
                        crate::block_py::NameLocation::RuntimeName(name) => Some(name),
                        crate::block_py::NameLocation::Constant(index) => {
                            match self.constants.get(index as usize) {
                                Some(InstrResolved::Load(constant)) => {
                                    constant.name.runtime_name_id()
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if helper == Some(RuntimeName::InjectGeneratorResumeException)
                    && matches!(call.args.get(1),
                        Some(CallArgPositional::Positional(InstrResolved::Load(load)))
                            if load.name.location == self.operand)
                {
                    self.found = true;
                }
            }
            instr.visit_children(self);
        }
    }
    let injections = function
        .blocks
        .iter()
        .filter(|block| {
            let mut find = FindInjection {
                constants: &prepared.module_constants,
                operand,
                found: false,
            };
            crate::block_py::walk_block(&mut find, block);
            find.found
        })
        .collect::<Vec<_>>();
    let [injection] = injections.as_slice() else {
        panic!("the resolved injection operation must exist exactly once");
    };
    // The source-normalized term owns the same explicit error edge. Its
    // captured control operand must retire before source exception dispatch.
    let mut label = injection
        .exc_edge
        .as_ref()
        .expect("source error edge")
        .target;
    let mut seen = HashSet::new();
    let mut retired_before_handler = false;
    while seen.insert(label) {
        let block = function
            .blocks
            .iter()
            .find(|block| block.label == label)
            .unwrap();
        if block.body.iter().any(|instr| {
            matches!(instr, InstrResolved::Del(del) if del.quietly && del.name.location == operand)
        }) {
            assert_eq!(block.extra.handled_exception, HandledExceptionContext::Preserve);
            retired_before_handler = true;
            break;
        }
        let BlockTerm::Jump(edge) = &block.term else {
            break;
        };
        label = edge.target;
    }
    assert!(
        retired_before_handler,
        "the injection operand must unwind before source exception dispatch"
    );
}

#[test]
fn managed_delegate_errors_classify_before_propagation_without_redelegation() {
    let result = crate::lower_python_to_blockpy_for_testing(
        r#"
def delegated(values):
    try:
        raise ValueError("outer")
    except ValueError:
        return (yield from values)

async def awaited(value):
    try:
        raise ValueError("outer")
    except ValueError:
        return await value
"#,
    )
    .expect("source delegation under a handler should lower");
    let core = result.pass_tracker.pass_core_blockpy().unwrap();
    for name in ["delegated", "awaited"] {
        let function = suspension_function(core, name);
        let dispatches = delivery_dispatches(function);
        assert_eq!(dispatches.len(), 1);
        let BlockTerm::BranchTable(branch) = &dispatches[0].term else {
            unreachable!()
        };
        let delegate = core_block_at(
            function,
            branch.targets[GeneratorResumeDelivery::YieldFromException as usize],
        );
        assert_ne!(delegate.label, branch.default_label);
        injection_call(delegate);
        let [InstrUnresolved::Store(capture), InstrUnresolved::Store(clear)] =
            delegate.body.as_slice()
        else {
            panic!("delegate injection may capture and retire only its error argument")
        };
        assert!(matches!(capture.lifetime, StoreLifetime::Operand { .. }));
        assert_eq!(clear.name.id_str(), "_dp_resume_exc");
        assert!(core_is_none(&clear.value));
        assert!(matches!(&delegate.term, BlockTerm::Raise(raise)
            if raise.disposition == RaiseDisposition::PropagateNormalized));
        let advance = function
            .blocks
            .iter()
            .find(|block| {
                block.body.iter().any(|instr| {
                    matches!(instr, InstrUnresolved::Store(store)
                    if core_runtime_call(&store.value, RuntimeName::Next).is_some())
                })
            })
            .expect("existing delegate next operation");
        assert_ne!(
            delegate.exc_edge.as_ref().unwrap().target,
            advance.exc_edge.as_ref().unwrap().target,
            "ordinary callback errors use the normalized propagation branch"
        );
        let handler = core_block_at(function, delegate.exc_edge.as_ref().unwrap().target);
        let ordinary_handler = core_block_at(function, advance.exc_edge.as_ref().unwrap().target);
        assert_eq!(handler.params, ordinary_handler.params);
        assert!(handler
            .params
            .iter()
            .any(|param| param.role == BlockParamRole::EnclosingException));
        assert!(handler
            .params
            .iter()
            .any(|param| param.role == BlockParamRole::Exception));
        let BlockTerm::IfTerm(test) = &handler.term else {
            panic!("native-delivered error classification")
        };
        let BlockTerm::IfTerm(ordinary_test) = &ordinary_handler.term else {
            panic!("ordinary delegate error classification")
        };
        assert_eq!(test.then_label, ordinary_test.then_label);
        assert_ne!(test.else_label, ordinary_test.else_label);
        let caught = handler
            .params
            .iter()
            .find(|param| param.role == BlockParamRole::Exception)
            .unwrap();
        for predicate in [&test.test, &ordinary_test.test] {
            let call = core_runtime_call(predicate, RuntimeName::ExceptionMatches)
                .expect("both paths classify the same normalized exception");
            assert!(matches!(call.args.as_slice(),
                [CallArgPositional::Positional(InstrUnresolved::Load(error)),
                 CallArgPositional::Positional(InstrUnresolved::Load(kind))]
                    if error.name.runtime_name_id().is_none()
                        && error.name.id_str() == caught.name
                        && kind.name.runtime_name_id() == Some(RuntimeName::StopIteration)));
        }
        let consumed = core_block_at(function, test.then_label);
        assert!(matches!(consumed.term, BlockTerm::Jump(_)));
        for (label, expected) in [
            (test.else_label, RaiseDisposition::SourceNormalized),
            (
                ordinary_test.else_label,
                RaiseDisposition::PropagateNormalized,
            ),
        ] {
            let escaped = core_block_at(function, label);
            let BlockTerm::Raise(raise) = &escaped.term else {
                panic!("only a non-StopIteration error escapes delegation");
            };
            assert_eq!(raise.disposition, expected);
            assert!(
                matches!(raise.exc.as_ref(), Some(InstrUnresolved::Load(error))
                if error.name.runtime_name_id().is_none() && error.name.id_str() == caught.name)
            );
            assert_same_exception_edge(&escaped.exc_edge, &handler.exc_edge);
            raise
                .validate_exception_operand()
                .expect("the escaped normalized exception remains explicit");
        }
        let direct = core_block_at(
            function,
            branch.targets[GeneratorResumeDelivery::DirectRaise as usize],
        );
        injection_call(direct);
        assert_ne!(
            direct.exc_edge.as_ref().expect("source handler").target,
            delegate
                .exc_edge
                .as_ref()
                .expect("delegate call handler")
                .target,
        );
        assert!(direct.body.iter().any(|instr| {
            matches!(instr, InstrUnresolved::Store(store)
                if store.name.id_str() == "_dp_yieldfrom" && core_is_none(&store.value))
        }));
        let ordinary = core_block_at(
            function,
            branch.targets[GeneratorResumeDelivery::Ordinary as usize],
        );
        assert!(matches!(&ordinary.term, BlockTerm::IfTerm(test)
            if core_runtime_call(&test.test, RuntimeName::Isinstance).is_some()));

        let prepared = result
            .pass_tracker
            .get::<BlockPyModule<ResolvedStorageModuleShape>>("bb_prepared")
            .unwrap();
        let prepared = suspension_function(prepared, name);
        let mut normalized_raises = 0;
        for block in &prepared.blocks {
            if let BlockTerm::Raise(raise) = &block.term {
                if raise.disposition == RaiseDisposition::SourceNormalized {
                    raise
                        .validate_exception_operand()
                        .expect("name binding preserves the explicit normalized exception");
                    normalized_raises += 1;
                }
            }
        }
        assert_eq!(
            normalized_raises, 2,
            "one direct and one escaped delegated normalized raise at {name}"
        );
    }
}

#[test]
fn async_yield_wraps_before_pc_publication_and_keeps_its_source_oom_edge() {
    let result = crate::lower_python_to_blockpy_for_testing(
        r#"
async def values(value, awaitable):
    try:
        yield value
        await awaitable
        value = yield value
    except MemoryError:
        yield "caught"
"#,
    )
    .expect("async source-yield operations should lower");
    let core = result.pass_tracker.pass_core_blockpy().unwrap();
    let function = suspension_function(core, "values");
    let mut wrapped_yields = 0;
    let mut delegated_yields = 0;
    let mut protected_wraps = 0;
    for block in &function.blocks {
        if !matches!(block.term, BlockTerm::Return(_)) {
            continue;
        }
        let wrapper = block.body.iter().enumerate().find_map(|(index, instr)| {
            let InstrUnresolved::Store(store) = instr else {
                return None;
            };
            core_runtime_call(&store.value, RuntimeName::AsyncGenWrapYield)
                .map(|call| (index, store, call))
        });
        if let Some((index, store, call)) = wrapper {
            wrapped_yields += 1;
            assert_eq!(call.args.len(), 1);
            assert!(matches!(store.lifetime, StoreLifetime::Operand { .. }));
            assert_eq!(store.meta().range, call.meta().range);
            let pc = block.body.iter().rposition(|instr| {
                matches!(instr, InstrUnresolved::Store(op) if op.name.id_str() == "_dp_pc")
            }).expect("suspension PC update");
            let delegate = block.body.iter().rposition(|instr| {
                matches!(instr, InstrUnresolved::Store(op) if op.name.id_str() == "_dp_yieldfrom")
            }).expect("direct-yield delegate state update");
            assert!(
                index < pc && index < delegate,
                "allocation precedes suspension-state stores"
            );
            assert!(
                matches!(&block.term, BlockTerm::Return(InstrUnresolved::Load(load))
                if load.name.id_str() == store.name.id_str())
            );
            if core_jump_path_has_load(
                function,
                block.exc_edge.as_ref().unwrap().target,
                "MemoryError",
            ) {
                protected_wraps += 1;
            }
        } else {
            delegated_yields += 1;
            assert!(
                block.body.iter().all(|instr| {
                    !matches!(instr, InstrUnresolved::Store(store)
                    if store.name.id_str() == "_dp_yieldfrom" && core_is_none(&store.value))
                }),
                "await suspension must retain the explicit delegate"
            );
        }
    }
    assert_eq!(wrapped_yields, 3);
    assert_eq!(delegated_yields, 1);
    assert_eq!(
        protected_wraps, 2,
        "both source yields in the try can catch native wrapper OOM"
    );
}

#[test]
fn async_generator_completion_is_not_a_source_exception_or_python_sentinel() {
    let result = crate::lower_python_to_blockpy_for_testing(
        r#"
async def values(fail):
    yield 1
    if fail:
        raise StopAsyncIteration("source error")
    return
"#,
    )
    .expect("async completion fixture should lower");
    let core = result.pass_tracker.pass_core_blockpy().unwrap();
    let function = suspension_function(core, "values");
    let completions = function
        .blocks
        .iter()
        .filter_map(|block| match &block.term {
            BlockTerm::GeneratorReturn(value) => Some((block, value)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!completions.is_empty());
    for (block, value) in completions {
        assert!(core_is_none(value));
        assert_eq!(
            block.extra.handled_exception,
            HandledExceptionContext::Terminal
        );
        assert!(block.exc_edge.is_none());
    }
    let terminal = function
        .blocks
        .iter()
        .find(|block| {
            block.extra.handled_exception == HandledExceptionContext::Terminal
                && matches!(&block.term, BlockTerm::Raise(raise)
                if raise.exc.as_ref().is_some_and(|expr|
                    core_runtime_call(expr, RuntimeName::Pep479Exception).is_some()))
        })
        .expect("terminal source errors pass through native async PEP479 semantics");
    let BlockTerm::Raise(raise) = &terminal.term else {
        unreachable!()
    };
    assert_eq!(raise.disposition, RaiseDisposition::PropagateNormalized);
    let call =
        core_runtime_call(raise.exc.as_ref().unwrap(), RuntimeName::Pep479Exception).unwrap();
    let Some(CallArgPositional::Positional(InstrUnresolved::Literal(kind))) = call.args.first()
    else {
        panic!("explicit immutable source-function kind")
    };
    assert!(matches!(kind.as_literal(), Literal::NumberLiteral(number)
        if matches!(&number.value, crate::block_py::NumberLiteralValue::Int(value) if value.as_i64() == Some(2))));
    assert!(
        function.blocks.iter().any(|block| {
            matches!(&block.term, BlockTerm::Raise(source)
            if source.disposition == RaiseDisposition::Source && source.exc.is_some())
                && core_block_has_load(block, "StopAsyncIteration")
        }),
        "a genuine source raise remains source-normalized"
    );
}

#[test]
fn async_generator_completion_validation_requires_canonical_none() {
    let module = crate::lower_python_to_blockpy_for_testing(
        "async def values():\n    yield 1\n    return\n",
    )
    .expect("a source async generator can complete without a value")
    .blockpy_module;
    crate::block_py::validate::validate_blockpy_module(&module)
        .expect("canonical async completion is valid resolved IR");
    let function_index = module
        .callable_defs
        .iter()
        .position(|function| function.names.bind_name == "values")
        .unwrap();
    let completion_index = module.callable_defs[function_index]
        .blocks
        .iter()
        .position(|block| matches!(block.term, BlockTerm::GeneratorReturn(_)))
        .expect("explicit completion terminator");

    let BlockTerm::GeneratorReturn(crate::block_py::InstrBlockPy::Load(completion)) =
        &module.callable_defs[function_index].blocks[completion_index].term
    else {
        panic!("resolved completion loads the canonical singleton");
    };
    let crate::block_py::NameLocation::Constant(constant_index) = completion.name.location else {
        panic!("the real constant-lowering path must be covered");
    };
    assert!(matches!(
        module.module_constants.get(constant_index as usize),
        Some(crate::block_py::ConstantExpr::RuntimeName(
            RuntimeName::None
        ))
    ));

    let mut redirected_constant = module.clone();
    redirected_constant.module_constants[constant_index as usize] =
        crate::block_py::ConstantExpr::RuntimeName(RuntimeName::True);
    assert!(
        crate::block_py::validate::validate_blockpy_module(&redirected_constant).is_err(),
        "the same load spelling cannot authenticate a different constant"
    );

    let mut direct_none = module.clone();
    direct_none.callable_defs[function_index].blocks[completion_index].term =
        BlockTerm::GeneratorReturn(crate::block_py::InstrBlockPy::constant_none());
    crate::block_py::validate::validate_blockpy_module(&direct_none)
        .expect("the unhoisted immutable runtime singleton is equivalent");

    let mut with_value = module.clone();
    with_value.callable_defs[function_index].blocks[completion_index].term =
        BlockTerm::GeneratorReturn(
            crate::block_py::Load::new(crate::block_py::ResolvedName::runtime_name("TRUE")).into(),
        );
    let error = crate::block_py::validate::validate_blockpy_module(&with_value)
        .expect_err("async completion cannot return a value");
    assert!(
        error.contains("requires the canonical None operand"),
        "{error}"
    );

    let mut ordinary =
        crate::lower_python_to_blockpy_for_testing("def ordinary():\n    return None\n")
            .expect("ordinary completion control")
            .blockpy_module;
    let ordinary_function = ordinary
        .callable_defs
        .iter_mut()
        .find(|function| function.names.bind_name == "ordinary")
        .unwrap();
    // Change only the completion operation. Reclassifying an async generator
    // would invalidate its private ABI before this terminator is examined.
    ordinary_function.blocks.last_mut().unwrap().term =
        BlockTerm::GeneratorReturn(crate::block_py::InstrBlockPy::constant_none());
    let error = crate::block_py::validate::validate_blockpy_module(&ordinary)
        .expect_err("ordinary functions cannot use generator completion");
    assert!(
        error.contains("requires a generator, coroutine, or async generator activation"),
        "{error}"
    );
}

#[test]
fn async_generator_completion_retains_immutable_none_in_runtime_bootstrap() {
    let module = crate::lower_python_to_blockpy_with_tracker_and_options(
        "async def values():\n    yield 1\n\ndef source_binding():\n    return NONE\n",
        crate::block_py::ModuleNameGen::new(0),
        soac_core::pass_tracker::RecordingPassTracker::new(),
        crate::LoweringOptions {
            runtime_names_as_globals: true,
            ..Default::default()
        },
    )
    .expect("bootstrap rewrites helpers, not immutable completion values")
    .blockpy_module;
    crate::block_py::validate::validate_blockpy_module(&module).unwrap();
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "values")
        .unwrap();
    let mut completions = 0;
    for block in &function.blocks {
        if let BlockTerm::GeneratorReturn(value) = &block.term {
            completions += 1;
            let crate::block_py::InstrBlockPy::Load(load) = value else {
                panic!("completion must load the immutable singleton");
            };
            let crate::block_py::NameLocation::Constant(index) = load.name.location else {
                panic!("bootstrap completion must retain its constant identity");
            };
            assert!(matches!(
                module.module_constants.get(index as usize),
                Some(crate::block_py::ConstantExpr::RuntimeName(
                    RuntimeName::None
                ))
            ));
        }
    }
    assert!(completions > 0);
    assert!(
        module.global_names.iter().any(|name| name == "NONE"),
        "a genuine source reference to NONE remains an ordinary global binding"
    );
}

#[test]
fn fresh_delegation_enters_next_even_after_a_send_or_caught_resume_error() {
    let result = crate::lower_python_to_blockpy_for_testing(
        r#"
def after_send(values):
    marker = yield 1
    return (yield from values)

def after_throw(values):
    try:
        yield 1
    except ValueError:
        return (yield from values)

async def after_await(first, second):
    await first
    return await second

async def after_caught_await(first, second):
    try:
        await first
    except ValueError:
        return await second
"#,
    )
    .expect("fresh delegation must have explicit first-entry semantics");
    let core = result.pass_tracker.pass_core_blockpy().unwrap();
    for name in [
        "after_send",
        "after_throw",
        "after_await",
        "after_caught_await",
    ] {
        let function = suspension_function(core, name);
        let mut fresh_entries = 0;
        for block in &function.blocks {
            let initializes_delegate = block.body.iter().any(|instr| {
                matches!(instr, InstrUnresolved::Store(store)
                    if store.name.id_str() == "_dp_yieldfrom"
                        && core_runtime_call(&store.value, RuntimeName::Iter).is_some())
            });
            if !initializes_delegate {
                continue;
            }
            fresh_entries += 1;
            let BlockTerm::Jump(edge) = &block.term else {
                panic!("explicit initial advance")
            };
            let next = core_block_at(function, edge.target);
            assert!(next.body.iter().any(|instr| {
                matches!(instr, InstrUnresolved::Store(store)
                    if core_runtime_call(&store.value, RuntimeName::Next).is_some())
            }));
            assert!(!core_block_has_load(next, "_dp_send_value"));
            assert!(!core_block_has_load(next, "_dp_resume_exc"));
            assert!(
                function.blocks.iter().any(|candidate| {
                    matches!(&candidate.term, BlockTerm::IfTerm(branch)
                    if branch.then_label == next.label
                        && core_block_has_load(candidate, "_dp_send_value"))
                }),
                "later normal resume still selects next versus send from its fresh packet"
            );
        }
        assert_eq!(fresh_entries, if name.contains("await") { 2 } else { 1 });
    }
}

#[test]
fn suspending_expression_operands_keep_public_active_preserved_roles() {
    use crate::block_py::{InstrBlockPy, OperandLocation, Visit};

    let result = crate::lower_python_to_blockpy_for_testing(
        "def gen():\n    return make_callee()(first(), left() if (yield 1) else right())\n",
    )
    .expect("ordered expression operands may cross a source suspension");
    // Suspension ordering adds preserved operands before generator lowering.
    // Generator lowering then adds local-only resume-error operands. Read the
    // actual resolved locations, not every post-generator Store as preserved.
    let original = result
        .pass_tracker
        .pass_name_binding()
        .expect("actual post-generator resolved operand producers");
    let original = suspension_function(original, "gen");
    struct Producers(Vec<(u64, String, PreservedLocation)>);
    impl Visit<InstrResolved> for Producers {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            if let InstrResolved::Store(store) = instr {
                if let StoreLifetime::Operand { unwind_order } = store.lifetime {
                    match store.name.location {
                        crate::block_py::NameLocation::Preserved(location) => {
                            self.0
                                .push((unwind_order, store.name.id_str().to_owned(), location));
                        }
                        crate::block_py::NameLocation::Local(_) => {}
                        _ => panic!("compiler operands need explicit local or preserved storage"),
                    }
                }
            }
            instr.visit_children(self);
        }
    }
    let mut producers = Producers(Vec::new());
    producers.visit_fn(original);
    producers
        .0
        .sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    producers.0.dedup();
    assert!(
        producers.0.len() >= 2,
        "callee and first argument need independent operands"
    );
    let function = suspension_function(&result.blockpy_module, "gen");
    let public = function.public_storage_layout().expect("factory storage");
    let active = function.storage_layout.as_ref().expect("resume storage");
    let expected = producers
        .0
        .iter()
        .map(|(_, _, location)| {
            let slot = public
                .preserved_slots
                .get(location.slot() as usize)
                .expect("every resolved preserved operand has its exact factory slot");
            assert_eq!(slot.init, ClosureInit::Deferred);
            assert_eq!(slot.storage, PreservedSlotStorage::PyObjectOrNull);
            assert!(slot.generator_control.is_none());
            *location
        })
        .collect::<Vec<_>>();
    let preserved_roles = |layout: &StorageLayout| {
        layout
            .expression_temporaries
            .iter()
            .filter_map(|location| location.preserved_location())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        preserved_roles(public),
        expected,
        "factory acquisition order"
    );
    assert_eq!(
        preserved_roles(active),
        expected,
        "resume must retain exact factory indices"
    );
    struct Takes<'a> {
        layout: &'a StorageLayout,
        preserved: HashSet<PreservedLocation>,
    }
    impl Visit<InstrBlockPy> for Takes<'_> {
        fn visit_instr(&mut self, instr: &InstrBlockPy) {
            if let InstrBlockPy::TakeOperand(op) = instr {
                let location = op
                    .validate_resolved(self.layout)
                    .expect("every consuming read has an explicit compiler operand owner");
                if let OperandLocation::Preserved(location) = location {
                    self.preserved.insert(location);
                }
            }
            instr.visit_children(self);
        }
    }
    let mut takes = Takes {
        layout: active,
        preserved: HashSet::new(),
    };
    takes.visit_fn(function);
    assert!(
        expected
            .iter()
            .all(|location| takes.preserved.contains(location)),
        "both operands must be consumed after resumption, not cloned from preserved storage"
    );
}

#[test]
fn managed_resume_injection_keeps_normalized_errors_in_term_operands() {
    let result = crate::lower_python_to_blockpy_for_testing(
        r#"
def receive(make, consume, later):
    local = make("local")
    yield "first"
    consume(make("operand"), (yield "second"), later())
    return (yield "last")
"#,
    )
    .expect("original direct-yield sites should lower");
    struct YieldRanges(Vec<TextRange>);
    impl crate::block_py::Visit<InstrWithYield> for YieldRanges {
        fn visit_instr(&mut self, instr: &InstrWithYield) {
            if let InstrWithYield::Yield(op) = instr {
                self.0.push(op.meta().range);
            }
            instr.visit_children(self);
        }
    }
    let original = result
        .pass_tracker
        .get::<BlockPyModule<crate::passes::CoreModuleShapeWithYield>>("core_blockpy_with_yield")
        .expect("actual original yield operations before resume lowering");
    let mut original_ranges = YieldRanges(Vec::new());
    for block in &suspension_function(original, "receive").blocks {
        crate::block_py::walk_block(&mut original_ranges, block);
    }
    original_ranges.0.sort_by_key(|range| range.start());
    assert_eq!(original_ranges.0.len(), 3);
    assert!(original_ranges.0.iter().all(|range| !range.is_empty()));

    let core = result.pass_tracker.pass_core_blockpy().unwrap();
    let function = suspension_function(core, "receive");
    let mut injection_ranges = Vec::new();
    for dispatch in delivery_dispatches(function) {
        let BlockTerm::BranchTable(branch) = &dispatch.term else {
            unreachable!()
        };
        let injection = core_block_at(
            function,
            branch.targets[GeneratorResumeDelivery::DirectRaise as usize],
        );
        let call = injection_call(injection);
        let BlockTerm::Raise(raise) = &injection.term else {
            panic!("the already-normalized error must still propagate");
        };
        assert_eq!(raise.disposition, RaiseDisposition::SourceNormalized);
        injection_ranges.push(call.meta().range);
    }
    injection_ranges.sort_by_key(|range| range.start());
    assert_eq!(injection_ranges, original_ranges.0);

    struct InjectionRanges<'a> {
        constants: &'a [InstrResolved],
        ranges: Vec<TextRange>,
    }
    impl crate::block_py::Visit<InstrResolved> for InjectionRanges<'_> {
        fn visit_instr(&mut self, instr: &InstrResolved) {
            if let InstrResolved::Call(call) = instr {
                let helper = match call.func.as_ref() {
                    InstrResolved::Load(load) => match load.name.location {
                        crate::block_py::NameLocation::RuntimeName(name) => Some(name),
                        crate::block_py::NameLocation::Constant(index) => {
                            match self.constants.get(index as usize) {
                                Some(InstrResolved::Load(constant)) => {
                                    constant.name.runtime_name_id()
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if helper == Some(RuntimeName::InjectGeneratorResumeException) {
                    self.ranges.push(call.meta().range);
                }
            }
            instr.visit_children(self);
        }
    }
    // Exception splitting/name binding must retain the normalized term operand,
    // without introducing another operation or a result-owner scratch.
    let prepared = result
        .pass_tracker
        .get::<BlockPyModule<ResolvedStorageModuleShape>>("bb_prepared")
        .unwrap();
    let mut prepared_ranges = Vec::new();
    let function = suspension_function(prepared, "receive");
    for block in &function.blocks {
        for instr in &block.body {
            let mut found = InjectionRanges {
                constants: &prepared.module_constants,
                ranges: Vec::new(),
            };
            crate::block_py::Visit::visit_instr(&mut found, instr);
            assert!(
                found.ranges.is_empty(),
                "the injection remains a term-owned normalized error"
            );
        }
        if let BlockTerm::Raise(raise) = &block.term {
            if let Some(exc) = &raise.exc {
                let mut found = InjectionRanges {
                    constants: &prepared.module_constants,
                    ranges: Vec::new(),
                };
                crate::block_py::Visit::visit_instr(&mut found, exc);
                if !found.ranges.is_empty() {
                    assert_eq!(raise.disposition, RaiseDisposition::SourceNormalized);
                    assert_eq!(found.ranges.len(), 1);
                    raise
                        .validate_exception_operand()
                        .expect("the normalized error operand remains explicit");
                    prepared_ranges.extend(found.ranges);
                }
            }
        }
    }
    prepared_ranges.sort_by_key(|range| range.start());
    assert_eq!(prepared_ranges, original_ranges.0);
}

#[derive(Default)]
struct SourceSuspensionRanges {
    direct: Vec<TextRange>,
    delegated: Vec<(TextRange, Option<&'static str>)>,
}

impl crate::block_py::Visit<InstrWithYield> for SourceSuspensionRanges {
    fn visit_instr(&mut self, instruction: &InstrWithYield) {
        match instruction {
            InstrWithYield::Yield(yielded) => self.direct.push(yielded.meta().range),
            InstrWithYield::YieldFrom(delegated) => {
                let generated_helper = match delegated.value.as_ref() {
                    InstrWithYield::Call(await_iter)
                        if matches!(await_iter.func.as_ref(), InstrWithYield::Load(load)
                            if load.name.is_runtime_symbol("await_iter")) =>
                    {
                        match await_iter.args.first() {
                            Some(CallArgPositional::Positional(InstrWithYield::Call(awaited))) => [
                                "anext",
                                "asynccontextmanager_aenter",
                                "asynccontextmanager_exit",
                            ]
                            .into_iter()
                            .find(|name| {
                                matches!(awaited.func.as_ref(), InstrWithYield::Load(load)
                                            if load.name.is_runtime_symbol(name))
                            }),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                self.delegated
                    .push((delegated.meta().range, generated_helper));
            }
            _ => {}
        }
        instruction.visit_children(self);
    }
}

fn unique_source_ranges(mut ranges: Vec<TextRange>) -> Vec<TextRange> {
    ranges.sort_by_key(|range| (range.start(), range.end()));
    ranges.dedup();
    ranges
}

#[test]
fn managed_async_genexpr_keeps_implicit_iteration_and_explicit_awaits() {
    use ruff_text_size::Ranged;

    for source in [
        "async def outer(values, choose):\n    return (\n        choose(value)\n        async for value in values\n    )\n",
        "async def outer(groups, choose):\n    return (\n        await choose(value)\n        async for group in groups\n        async for value in group\n    )\n",
    ] {
        let syntax = ruff_python_parser::parse_module(source).unwrap().into_syntax();
        let ast::Stmt::FunctionDef(definition) = &syntax.body[0] else { unreachable!() };
        let ast::Stmt::Return(returned) = &definition.body[0] else { unreachable!() };
        let Some(Expr::Generator(original)) = returned.value.as_deref() else { unreachable!() };
        let element_range = original.elt.range();

        let lowered = crate::lower_python_to_blockpy_for_testing(source)
            .expect("async genexpr iteration and explicit awaits should lower");
        let source_yields = lowered.pass_tracker
            .get::<BlockPyModule<crate::passes::CoreModuleShapeWithYield>>("core_blockpy_with_yield")
            .unwrap();
        let helper = source_yields.callable_defs.iter().find(|function| {
            function.kind == FunctionKind::AsyncGenerator
        }).unwrap();
        let mut sites = SourceSuspensionRanges::default();
        crate::block_py::Visit::visit_fn(&mut sites, helper);
        assert_eq!(sites.direct, [element_range]);
        let implicit_waits = sites.delegated.iter()
            .filter(|(_, helper)| *helper == Some("anext"))
            .collect::<Vec<_>>();
        assert_eq!(implicit_waits.len(), original.generators.iter().filter(|gen| gen.is_async).count());
        for (range, helper) in &sites.delegated {
            if helper.is_none() {
                assert!(matches!(original.elt.as_ref(), Expr::Await(_)));
                assert_eq!(*range, element_range, "an explicit await keeps its own source range");
            }
        }

        let helper = lowered.blockpy_module.callable_defs.iter().find(|function| {
            function.kind == FunctionKind::AsyncGenerator
        }).unwrap();
        let normalized = helper.blocks.iter().filter_map(|block| {
            match &block.term {
                BlockTerm::Raise(raise)
                    if raise.disposition == RaiseDisposition::SourceNormalized => Some(raise),
                _ => None,
            }
        }).collect::<Vec<_>>();
        assert!(!normalized.is_empty());
        assert!(normalized.iter().all(|raise| raise.exc.is_some()));
    }
}

#[test]
fn managed_async_with_keeps_context_specific_waits() {
    use crate::transformer::{walk_expr, Transformer};
    use ruff_text_size::Ranged;

    #[derive(Default)]
    struct ExplicitSites {
        awaits: Vec<TextRange>,
        yields: Vec<TextRange>,
    }
    impl Transformer for ExplicitSites {
        fn visit_expr(&mut self, expression: &mut Expr) {
            match expression {
                Expr::Await(node) => self.awaits.push(node.range),
                Expr::Yield(node) => self.yields.push(node.range),
                _ => {}
            }
            walk_expr(self, expression);
        }
    }

    for source in [
        "async def run(factory, setup, body):\n    async with factory(await setup()) as first, factory(2) as second:\n        await body()\n        return first\n",
        "async def run(factory, body):\n    async with factory(1) as first, factory(2) as second:\n        await body()\n        yield first\n",
    ] {
        let syntax = ruff_python_parser::parse_module(source).unwrap().into_syntax();
        let ast::Stmt::FunctionDef(definition) = &syntax.body[0] else { unreachable!() };
        let ast::Stmt::With(with_stmt) = &definition.body[0] else { unreachable!() };
        let contexts = with_stmt.items.iter()
            .map(|item| item.context_expr.range()).collect::<Vec<_>>();
        assert_ne!(contexts[0], contexts[1]);
        let mut explicit = ExplicitSites::default();
        explicit.visit_body(&mut definition.body.clone());
        assert!(explicit.awaits.iter().all(|range| !contexts.contains(range)));

        let lowered = crate::lower_python_to_blockpy_for_testing(source)
            .expect("generated async-with waits must carry each original context location");
        let source_yields = lowered.pass_tracker
            .get::<BlockPyModule<crate::passes::CoreModuleShapeWithYield>>("core_blockpy_with_yield")
            .unwrap();
        let mut sites = SourceSuspensionRanges::default();
        crate::block_py::Visit::visit_fn(&mut sites, suspension_function(source_yields, "run"));
        assert_eq!(unique_source_ranges(sites.direct), unique_source_ranges(explicit.yields.clone()));
        for (range, helper) in &sites.delegated {
            match helper {
                Some("asynccontextmanager_aenter" | "asynccontextmanager_exit") => {
                    assert!(contexts.contains(range), "each implicit wait uses its own context expression");
                }
                None => assert!(explicit.awaits.contains(range), "source awaits are not restamped"),
                Some(other) => panic!("unexpected generated wait helper {other}"),
            }
        }
        for context in &contexts {
            assert!(sites.delegated.contains(&(*context, Some("asynccontextmanager_aenter"))));
            assert!(sites.delegated.iter().filter(|(range, helper)| {
                range == context && *helper == Some("asynccontextmanager_exit")
            }).count() >= 2, "both normal/abrupt and exceptional exits keep the context range");
        }
        for source_await in &explicit.awaits {
            assert!(sites.delegated.contains(&(*source_await, None)));
        }
    }
}
