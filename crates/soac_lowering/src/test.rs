use crate::pass_tracker::LoweringPassTrackerExt;
use crate::passes::ast_to_ast::body::Suite;
use crate::template::py_stmt;
use crate::transformer::{walk_stmt, Transformer};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::Ranged;
use soac_core::block_py::{BlockTerm, ChildVisitable, FunctionExecutionMode, PrettyPrint, Visit};
use soac_core::pass_tracker::{PassTracker, RecordingPassTracker};
use soac_ir_blockpy::{
    constructor_entry_function_id_for_init, InstrBlockPy, CONSTRUCTOR_ENTRY_FUNCTION_NAME,
    CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME,
};

pub(crate) mod native_class_bindings;

#[derive(Clone)]
struct TestPrettySuite(Suite);

#[test]
fn cell_load_binding_distinguishes_owned_preserved_and_free_reads() {
    use soac_core::block_py::{CellBindingKind, CellLocation};

    let lowered = crate::lower_python_to_blockpy_for_testing(
        "def owner(value):\n    def read():\n        return value\n    return value, read\n\
def suspended(value):\n    def read():\n        return value\n    yield value\n    yield read\n",
    )
    .expect("ordinary and suspended closure bindings lower");
    #[derive(Default)]
    struct Bindings {
        owned: bool,
        preserved: bool,
        captured: bool,
    }
    impl Visit<InstrBlockPy> for Bindings {
        fn visit_instr(&mut self, instr: &InstrBlockPy) {
            if let InstrBlockPy::Load(load) = instr {
                match load.name.cell_location() {
                    Some(location) => {
                        let binding = load
                            .cell_binding
                            .as_ref()
                            .expect("cell load has source binding");
                        assert_eq!(binding.logical_name.as_str(), "value");
                        match location {
                            CellLocation::Owned(_) => {
                                assert_eq!(binding.kind, CellBindingKind::Owner);
                                self.owned = true;
                            }
                            CellLocation::Preserved(_) => {
                                assert_eq!(binding.kind, CellBindingKind::Owner);
                                self.preserved = true;
                            }
                            CellLocation::Closure(_) | CellLocation::CapturedSource(_) => {
                                assert_eq!(binding.kind, CellBindingKind::Capture);
                                self.captured = true;
                            }
                            CellLocation::Private(_) => {
                                panic!("ordinary closure fixture has no private cell load")
                            }
                        }
                    }
                    None => assert!(load.cell_binding.is_none()),
                }
            }
            instr.visit_children(self);
        }
    }
    let mut bindings = Bindings::default();
    bindings.visit_module(&lowered.blockpy_module);
    assert!(bindings.owned && bindings.preserved && bindings.captured);
}

#[test]
fn suspended_cell_cleanup_releases_storage_without_deleting_captured_contents() {
    use soac_core::block_py::{NameLocation, PreservedSlotStorage};

    for source in [
        "def suspended():\n    value = 2\n    yield lambda: value\n    del value\n",
        "async def suspended(pause):\n    value = 2\n    read = lambda: value\n    await pause()\n    del value\n    return read\n",
        "async def suspended():\n    value = 2\n    yield lambda: value\n    del value\n",
    ] {
        let module = crate::lower_python_to_blockpy_for_testing(source)
            .expect("suspended closure source lowers")
            .blockpy_module;
        let function = module
            .callable_defs
            .iter()
            .find(|function| {
                function.storage_layout.as_ref().is_some_and(|layout| {
                    layout.preserved_slots.iter().any(|slot| {
                        slot.logical_name == "value"
                            && slot.storage == PreservedSlotStorage::PyCellObject
                    })
                })
            })
            .expect("resume function owns the preserved value cell");
        let (index, slot) = function
            .storage_layout
            .as_ref()
            .unwrap()
            .preserved_slots
            .iter()
            .enumerate()
            .find(|(_, slot)| slot.logical_name == "value")
            .unwrap();
        struct Deletes<'a> {
            storage: &'a str,
            slot: u32,
            source_deletes: usize,
            storage_releases: usize,
        }
        impl Visit<InstrBlockPy> for Deletes<'_> {
            fn visit_instr(&mut self, instr: &InstrBlockPy) {
                if let InstrBlockPy::Del(delete) = instr {
                    if delete.name.id.as_str() == self.storage {
                        assert_eq!(
                            delete.name.location,
                            NameLocation::preserved(self.slot),
                            "frame cleanup must release the cell object, not erase its contents"
                        );
                        self.storage_releases += 1;
                    } else if delete.name.id.as_str() == "value" {
                        assert_eq!(
                            delete.name.location,
                            NameLocation::preserved_cell(self.slot),
                            "source del must still empty the shared captured binding"
                        );
                        self.source_deletes += 1;
                    }
                }
                instr.visit_children(self);
            }
        }
        let mut deletes = Deletes {
            storage: &slot.storage_name,
            slot: index as u32,
            source_deletes: 0,
            storage_releases: 0,
        };
        deletes.visit_fn(function);
        assert!(deletes.source_deletes > 0 && deletes.storage_releases > 0);
    }
}

#[test]
fn unsupported_lazy_imports_fail_before_ast_rewriting() {
    for source in [
        "lazy import math\n",
        "lazy from math import sqrt\n",
        "def f():\n    lazy import math\n",
    ] {
        let parsed =
            ruff_python_parser::parse_module(source).expect("Ruff recognizes lazy import syntax");
        let mut tracker = RecordingPassTracker::new();
        let result = crate::lower_source_to_blockpy_module_with_tracker(
            source,
            soac_core::block_py::ModuleNameGen::new(0),
            &mut tracker,
            crate::LoweringOptions::default(),
        );
        let Err(crate::LoweringError::Other(error)) = result else {
            panic!("lazy imports must return an explicit unsupported error");
        };
        assert!(
            matches!(error.downcast_ref::<crate::driver::UnsupportedSyntax>(), Some(crate::driver::UnsupportedSyntax::LazyImport(range)) if parsed.syntax().range.contains_range(*range))
        );
        assert!(!tracker
            .pass_timings()
            .any(|timing| timing.name == "ast-to-ast"));
    }
}

#[test]
fn unsupported_unpacking_comprehensions_fail_before_ast_rewriting() {
    for source in [
        "result = {**item for item in mappings}\n",
        "result = [*item for item in values]\n",
        "result = {*item for item in values}\n",
        "result = (*item for item in values)\n",
    ] {
        let parsed = ruff_python_parser::parse_module(source)
            .expect("Ruff recognizes unpacking comprehension syntax");
        let Stmt::Assign(assignment) = &parsed.syntax().body[0] else {
            panic!("fixture assignment");
        };
        let expected = assignment.value.range();
        let Err(crate::LoweringError::Other(error)) =
            crate::lower_python_to_blockpy_for_testing(source)
        else {
            panic!("unpacking comprehensions must return an explicit unsupported error");
        };
        assert_eq!(
            error.downcast_ref::<crate::driver::UnsupportedSyntax>(),
            Some(&crate::driver::UnsupportedSyntax::UnpackingComprehension(
                expected
            ))
        );
    }
}

#[test]
fn ast_instruction_roundtrip_preserves_call_source_range_and_keywords() {
    let parsed =
        ruff_python_parser::parse_module("value = receiver.call(positional, named=other)\n")
            .expect("fixture source");
    let Stmt::Assign(assignment) = &parsed.syntax().body[0] else {
        panic!("fixture assignment");
    };
    let original = &*assignment.value;
    let converted = crate::passes::ast_to_instr::from_ast_expr(original.clone());
    let roundtrip = crate::passes::ast_to_instr::into_ast_expr(converted);
    assert_eq!(roundtrip.range(), original.range());
    let Expr::Call(call) = roundtrip else {
        panic!("call shape must survive");
    };
    assert_eq!(call.arguments.args.len(), 1);
    assert_eq!(call.arguments.keywords.len(), 1);
    assert_eq!(
        call.arguments.keywords[0]
            .arg
            .as_ref()
            .map(|name| name.as_str()),
        Some("named")
    );
}

impl PrettyPrint for TestPrettySuite {
    fn fmt_pretty(&self, printer: &mut soac_core::block_py::PrettyPrinter<'_>) -> std::fmt::Result {
        std::fmt::Write::write_str(printer, &crate::ruff_ast::ruff_ast_to_string(&self.0))
    }
}

#[test]
#[should_panic(expected = "PassTracker already contains a pass named one")]
fn pass_tracker_rejects_duplicate_names() {
    let mut tracker = RecordingPassTracker::new();
    let _suite: TestPrettySuite =
        tracker.run_pass("one", || TestPrettySuite([py_stmt!("x = 1")].into()));
    let _suite: TestPrettySuite =
        tracker.run_pass("one", || TestPrettySuite([py_stmt!("x = 2")].into()));
}

#[test]
fn pass_tracker_records_timing_without_storing_pass_value() {
    let mut tracker = RecordingPassTracker::new();
    let value: i32 = tracker.record_timing("timed-only", || 7);

    assert_eq!(value, 7);
    assert_eq!(
        tracker
            .pass_timings()
            .map(|timing| timing.name)
            .collect::<Vec<_>>(),
        vec!["timed-only".to_string()]
    );
    assert_eq!(tracker.render_pass_text("timed-only"), None);
    assert_eq!(tracker.render_pass_debug_text("timed-only"), None);
}

#[test]
fn pass_tracker_renders_tracked_pass_text_for_renderable_passes() {
    let mut tracker = RecordingPassTracker::new();
    let _suite: TestPrettySuite =
        tracker.run_pass("one", || TestPrettySuite([py_stmt!("x = 1")].into()));

    assert_eq!(tracker.render_pass_text("one").as_deref(), Some("x = 1\n"));
    assert_eq!(
        tracker.render_pass_debug_text("one").as_deref(),
        Some("x = 1\n")
    );
    assert_eq!(
        tracker
            .pass_timings()
            .map(|timing| timing.name)
            .collect::<Vec<_>>(),
        vec!["one".to_string()]
    );
}

#[test]
fn pure_lowering_does_not_insert_counters() {
    let lowered = crate::lower_python_to_blockpy_for_testing(
        "def f(x):\n    if x:\n        return 1\n    return 0\n",
    )
    .expect("lowering should succeed")
    .blockpy_module;

    assert!(lowered.counter_defs.is_empty());

    let mut probe = IncrementCounterProbe::default();
    for function in &lowered.callable_defs {
        for block in &function.blocks {
            for instr in &block.body {
                probe.visit_instr(instr);
            }
            probe.visit_term(&block.term);
        }
    }
    assert_eq!(probe.increment_counters, 0);
}

#[test]
fn class_lowering_adds_constructor_entry_function() {
    let lowered = crate::lower_python_to_blockpy_for_testing(
        "class C:\n    def __init__(self, value, *, scale=1):\n        self.value = value * scale\n",
    )
    .expect("lowering should succeed")
    .blockpy_module;

    let init_function = lowered
        .callable_defs
        .iter()
        .find(|function| function.names.qualname == "C.__init__")
        .expect("lowered __init__ should exist");
    let constructor_entry_function_id =
        constructor_entry_function_id_for_init(&lowered, init_function.function_id)
            .expect("constructor entry should be associated with __init__");
    let constructor_entry = lowered
        .callable_defs
        .iter()
        .find(|function| function.function_id == constructor_entry_function_id)
        .expect("constructor entry should exist");

    assert_ne!(constructor_entry.function_id, init_function.function_id);
    assert_eq!(
        constructor_entry.names.fn_name,
        CONSTRUCTOR_ENTRY_FUNCTION_NAME
    );
    assert_eq!(constructor_entry.execution_mode, FunctionExecutionMode::Jit);
    assert_eq!(
        constructor_entry.params.names(),
        vec![
            CONSTRUCTOR_ENTRY_TYPE_PARAM_NAME.to_string(),
            "value".to_string(),
            "scale".to_string()
        ]
    );
    assert!(matches!(
        constructor_entry.blocks[0].term,
        BlockTerm::Return(InstrBlockPy::Call(_))
    ));
}

#[test]
fn class_namespace_helper_finishes_with_original_sorted_static_attribute_store() {
    let source = concat!(
        "class Subject:\n",
        "    marker: int = 1\n",
        "    def method(self):\n",
        "        self.zeta = 1\n",
        "        self.__private = 2\n",
        "        self.alpha = 3\n",
    );
    let lowered = crate::lower_python_to_blockpy_for_testing(source)
        .expect("class static-attribute source should lower");
    let mut module = lowered
        .pass_tracker
        .pass_ast_to_ast()
        .expect("the production ast-to-ast pass should be tracked");

    #[derive(Default)]
    struct ClassNamespaceTailProbe {
        helper_count: usize,
        static_attribute_names: Option<Vec<String>>,
    }

    impl Transformer for ClassNamespaceTailProbe {
        fn visit_stmt(&mut self, stmt: &mut Stmt) {
            if let Stmt::FunctionDef(function) = stmt {
                if function.name.id.as_str() != "_dp_class_ns_Subject" {
                    walk_stmt(self, stmt);
                    return;
                }
                self.helper_count += 1;
                let Some(Stmt::Assign(ast::StmtAssign { targets, value, .. })) =
                    function.body.last()
                else {
                    return;
                };
                let [Expr::Subscript(ast::ExprSubscript {
                    value: namespace,
                    slice,
                    ..
                })] = targets.as_slice()
                else {
                    return;
                };
                if !matches!(
                    namespace.as_ref(),
                    Expr::Name(name) if name.id.as_str() == "_dp_class_ns"
                ) || !matches!(
                    slice.as_ref(),
                    Expr::StringLiteral(name)
                        if name.value.to_string() == "__static_attributes__"
                ) {
                    return;
                }
                let Expr::Tuple(attributes) = value.as_ref() else {
                    return;
                };
                let names = attributes
                    .elts
                    .iter()
                    .map(|attribute| match attribute {
                        Expr::StringLiteral(attribute) => attribute.value.to_string(),
                        other => panic!("static attribute must be a string literal: {other:?}"),
                    })
                    .collect::<Vec<_>>();
                self.static_attribute_names = Some(names);
                return;
            }
            walk_stmt(self, stmt);
        }
    }

    let mut probe = ClassNamespaceTailProbe::default();
    probe.visit_body(&mut module.body);
    assert_eq!(
        probe.helper_count, 1,
        "the class helper must be emitted once"
    );
    assert_eq!(
        probe.static_attribute_names,
        Some(vec![
            "__private".to_string(),
            "alpha".to_string(),
            "zeta".to_string(),
        ]),
        "the real production class namespace helper must end with the sorted, unmangled \
         compiler-inferred attribute tuple, after any generated annotation helper"
    );
}

#[derive(Default)]
struct IncrementCounterProbe {
    increment_counters: usize,
}

impl Visit<InstrBlockPy> for IncrementCounterProbe {
    fn visit_instr(&mut self, expr: &InstrBlockPy) {
        if matches!(expr, InstrBlockPy::IncrementCounter(_)) {
            self.increment_counters += 1;
        }
        expr.visit_children(self);
    }
}

pub(crate) mod strict_source {
    use std::sync::Arc;

    use soac_contracts::*;
    use soac_core::block_py::{BlockPyModule, CallableSourceRole, ModuleNameGen};
    use soac_core::pass_tracker::RecordingPassTracker;
    use soac_ir_blockpy::BlockPyModuleShape;

    // The default logical catalog is empty. Explicit proposal fixtures below
    // test compiler structure, not semantic exporter or runtime admission.
    fn verified_source(source: &str) -> Arc<VerifiedModuleTypeFacts> {
        verified_source_with_classes(source, Vec::new(), Vec::new())
    }

    fn verified_source_with_classes(
        source: &str,
        classes: Vec<ClassTypeFact>,
        dependencies: Vec<DependencyFingerprint>,
    ) -> Arc<VerifiedModuleTypeFacts> {
        verified_source_with_catalog(source, classes, Vec::new(), dependencies)
    }

    fn verified_source_with_catalog(
        source: &str,
        classes: Vec<ClassTypeFact>,
        functions: Vec<FunctionTypeFact>,
        dependencies: Vec<DependencyFingerprint>,
    ) -> Arc<VerifiedModuleTypeFacts> {
        verified_source_with_nominal_catalog(source, classes, functions, Vec::new(), dependencies)
    }

    fn verified_source_with_nominal_catalog(
        source: &str,
        classes: Vec<ClassTypeFact>,
        functions: Vec<FunctionTypeFact>,
        nominal_bindings: Vec<NominalBindingFact>,
        dependencies: Vec<DependencyFingerprint>,
    ) -> Arc<VerifiedModuleTypeFacts> {
        verified_source_with_nominal_catalog_policy(
            source,
            classes,
            functions,
            nominal_bindings,
            dependencies,
            ResolvedStrictPolicy::default(),
        )
    }

    fn verified_source_with_nominal_catalog_policy(
        source: &str,
        classes: Vec<ClassTypeFact>,
        functions: Vec<FunctionTypeFact>,
        nominal_bindings: Vec<NominalBindingFact>,
        dependencies: Vec<DependencyFingerprint>,
        policy: ResolvedStrictPolicy,
    ) -> Arc<VerifiedModuleTypeFacts> {
        let hash = Fingerprint::digest(b"source-propagation-test-environment");
        let environment = ArtifactEnvironment {
            ty_revision: "d2620d7312875790b114d821721cddf253f66423".into(),
            checker_source_fingerprint: hash,
            exporter_revision: "source-propagation-test".into(),
            python_version: PythonVersion {
                major: 3,
                minor: 15,
            },
            python_platform: "linux".into(),
            cpython_abi_fingerprint: hash,
            normalized_project_policy: hash,
            resolved_typechecker_configuration: hash,
            import_search_path: hash,
            typeshed_fingerprint: hash,
            installed_stub_fingerprint: hash,
            installed_dependency_fingerprint: hash,
            analysis: ConservativeAnalysis::default(),
        };
        let mut facts = ModuleTypeFacts::new(
            "pkg.strict_origins",
            source.as_bytes(),
            SourceDialect::SoacStrict,
            policy.clone(),
        )
        .unwrap();
        facts.classes = classes;
        facts.functions = functions;
        facts.nominal_bindings = nominal_bindings;
        facts.consumed_dependencies = dependencies.clone();
        let shard = encode_module_shard(&facts).unwrap();
        let manifest = TypeArtifactManifest::new(
            environment.clone(),
            vec![ModuleArtifactIndex::from_shard(&shard).unwrap()],
        )
        .unwrap();
        let key = ArtifactSigningKey::from_bytes(&[67; 32]);
        let expected = ArtifactExpectations {
            generation: manifest.generation,
            environment,
        };
        let manifest = verify_manifest(
            &sign_manifest(&manifest, &key).unwrap(),
            &key.trust_anchor(),
            &expected,
        )
        .unwrap();
        let generation =
            verify_complete_generation(manifest, |_| Ok(shard.bytes().to_vec())).unwrap();
        Arc::new(
            generation
                .manifest()
                .verify_module(
                    "pkg.strict_origins",
                    source.as_bytes(),
                    &policy,
                    &dependencies,
                    shard.bytes(),
                )
                .unwrap(),
        )
    }

    fn lower(
        source: &str,
        facts: Option<Arc<VerifiedModuleTypeFacts>>,
    ) -> crate::Result<BlockPyModule<BlockPyModuleShape>> {
        let canonical_class_bindings = facts
            .as_ref()
            .map(|_| super::native_class_bindings::for_source(source))
            .transpose()?;
        crate::lower_source_to_blockpy_module_with_tracker(
            source,
            ModuleNameGen::new(1),
            &mut RecordingPassTracker::new(),
            crate::LoweringOptions {
                strict_facts: facts,
                canonical_class_bindings,
                ..Default::default()
            },
        )
    }

    /// Full production lowering with the existing signed fixture catalog and
    /// the selected native compiler's original class-binding metadata.
    pub(crate) fn lower_verified(source: &str) -> crate::Result<BlockPyModule<BlockPyModuleShape>> {
        lower(source, Some(verified_source(source)))
    }

    #[test]
    fn strict_builtin_reads_keep_live_global_slots_and_compiler_helpers() {
        use soac_core::block_py::{
            ChildVisitable, ConstantExpr, NameLike, NameLocation, RuntimeName, Visit,
        };
        use soac_ir_blockpy::InstrBlockPy;

        let source = r#"from __future__ import strict
def reads(values):
    return any(values), all(values), len(values), iter(values), later(values)

def implicit_iteration(values):
    for value in values:
        return value
    return None
"#;
        let module = lower_verified(source).unwrap();
        let reads = module
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "reads")
            .unwrap();

        #[derive(Default)]
        struct Loads(Vec<(String, NameLocation)>);
        impl Visit<InstrBlockPy> for Loads {
            fn visit_instr(&mut self, instruction: &InstrBlockPy) {
                if let InstrBlockPy::Load(load) = instruction {
                    self.0
                        .push((load.name.id_str().to_owned(), load.name.location));
                }
                instruction.visit_children(self);
            }
        }
        let mut loads = Loads::default();
        crate::block_py::walk_fn(&mut loads, reads);
        for name in ["any", "all", "len", "iter", "later"] {
            let slot = module
                .global_names
                .iter()
                .position(|global| global == name)
                .expect("source reads reserve a global slot even before the first binding");
            assert!(
                loads.0.iter().any(|(loaded, location)| {
                    loaded == name && *location == NameLocation::global(slot as u32)
                }),
                "{name} must retain module-global lookup before live builtin fallback"
            );
        }
        assert!(
            !module.module_constants.iter().any(|constant| {
                matches!(
                    constant,
                    ConstantExpr::RuntimeName(
                        RuntimeName::Any | RuntimeName::All | RuntimeName::Len
                    )
                )
            }),
            "unbound source builtin names must not become snapshotted constants"
        );
        assert!(
            module.module_constants.iter().any(|constant| {
                matches!(constant, ConstantExpr::RuntimeName(RuntimeName::Iter))
            }),
            "implicit iteration keeps its explicit compiler helper, independently of source iter"
        );
    }

    #[test]
    fn strict_set_comprehension_initialization_uses_a_compiler_literal() {
        use soac_core::block_py::{ChildVisitable, ConstantExpr, NameLocation, RuntimeName, Visit};
        use soac_ir_blockpy::InstrBlockPy;

        #[derive(Default)]
        struct Loads(Vec<NameLocation>);
        impl Visit<InstrBlockPy> for Loads {
            fn visit_instr(&mut self, instruction: &InstrBlockPy) {
                if let InstrBlockPy::Load(load) = instruction {
                    self.0.push(load.name.location);
                }
                instruction.visit_children(self);
            }
        }

        let source = "from __future__ import strict\ndef explicit(values):\n    return set(values)\ndef implicit(values):\n    return {value for value in values}\n";
        let module = lower_verified(source).unwrap();
        let set_global = module
            .global_names
            .iter()
            .position(|name| name == "set")
            .unwrap();
        let explicit = module
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "explicit")
            .unwrap();
        let mut explicit_loads = Loads::default();
        crate::block_py::walk_fn(&mut explicit_loads, explicit);
        assert!(
            explicit_loads
                .0
                .contains(&NameLocation::global(set_global as u32)),
            "an explicit source call still reads the shadowable global"
        );
        let helper = module
            .callable_defs
            .iter()
            .find(|function| function.names.display_name == "<setcomp>")
            .unwrap();
        let mut helper_loads = Loads::default();
        crate::block_py::walk_fn(&mut helper_loads, helper);
        assert!(
            !helper_loads
                .0
                .contains(&NameLocation::global(set_global as u32)),
            "compiler-created collection construction must not read the source global"
        );
        assert!(
            helper_loads.0.iter().any(|location| {
                let NameLocation::Constant(index) = location else {
                    return false;
                };
                matches!(
                    module.module_constants.get(*index as usize),
                    Some(ConstantExpr::RuntimeName(RuntimeName::Set))
                )
            }),
            "the empty set literal reaches the explicit runtime collection constructor"
        );
    }

    #[test]
    fn unrepresented_class_cell_operations_fail_before_name_binding() {
        use soac_core::block_py::{ClassBindingAccessContext, NativeCompileScopeKind};

        for (body, context) in [
            ("        seen = value\n", ClassBindingAccessContext::Load),
            (
                "        nonlocal value\n        value = 3\n",
                ClassBindingAccessContext::Store,
            ),
            (
                "        nonlocal value\n        del value\n",
                ClassBindingAccessContext::Delete,
            ),
        ] {
            let source = format!(
                "from __future__ import strict\ndef factory(value):\n    class C:\n{body}    return C\n"
            );
            let canonical = super::native_class_bindings::for_source(&source).unwrap();
            let class = canonical
                .nodes()
                .iter()
                .find(|node| node.compile_scope == NativeCompileScopeKind::Class)
                .unwrap();
            let mut recipes = canonical.class_recipes().cloned().collect::<Vec<_>>();
            let recipe = recipes
                .iter_mut()
                .find(|recipe| recipe.class_code == class.id)
                .unwrap();
            let original_count = recipe.accesses.len();
            recipe.accesses.retain(|access| access.context != context);
            assert_eq!(original_count - recipe.accesses.len(), 1);
            let incomplete = Arc::new(
                crate::CanonicalClassBindings::from_native_entries(
                    &source,
                    canonical.nodes().to_vec(),
                    recipes,
                )
                .unwrap(),
            );
            let mut tracker = RecordingPassTracker::new();
            let result = crate::lower_source_to_blockpy_module_with_tracker(
                &source,
                ModuleNameGen::new(1),
                &mut tracker,
                crate::LoweringOptions {
                    strict_facts: Some(verified_source(&source)),
                    canonical_class_bindings: Some(incomplete),
                    ..Default::default()
                },
            );
            let Err(crate::LoweringError::StrictAuthentication(message)) = result else {
                panic!("a missing actual cell access must be an explicit lowering refusal");
            };
            assert!(message.starts_with(&format!(
                "retained class factory.<locals>.C: {context:?} value has no canonical native slot access"
            )));
            assert!(tracker.pass_names().any(|name| name == "core_blockpy"));
            assert!(!tracker.pass_names().any(|name| name == "name_binding"));
        }
    }

    #[test]
    fn unrepresented_compiler_tail_cell_stores_fail_before_name_binding() {
        for body in [
            "        captured = __static_attributes__\n",
            "        def captured(self):\n            return __static_attributes__\n",
            "        captured = staticmethod(lambda: __static_attributes__)\n",
            "        nonlocal __static_attributes__\n        captured = __static_attributes__\n",
        ] {
            let source = format!(
                "from __future__ import strict\ndef factory():\n    __static_attributes__ = 'outer'\n    class Subject:\n{body}        def method(self):\n            self.inferred = 1\n    return Subject\n"
            );
            let mut tracker = RecordingPassTracker::new();
            let result = crate::lower_source_to_blockpy_module_with_tracker(
                &source,
                ModuleNameGen::new(1),
                &mut tracker,
                crate::LoweringOptions {
                    strict_facts: Some(verified_source(&source)),
                    canonical_class_bindings: Some(
                        super::native_class_bindings::for_source(&source).unwrap(),
                    ),
                    ..Default::default()
                },
            );
            let Err(crate::LoweringError::StrictAuthentication(message)) = result else {
                panic!("an unrepresented compiler-tail store must refuse before the mapper");
            };
            assert!(message.starts_with(
                "retained class factory.<locals>.Subject: Store __static_attributes__ has no canonical native slot access"
            ));
            assert!(tracker.pass_names().any(|name| name == "core_blockpy"));
            assert!(!tracker.pass_names().any(|name| name == "name_binding"));
        }
    }

    #[test]
    fn represented_cells_and_same_name_namespace_free_bindings_remain_supported() {
        use soac_core::block_py::{BindingPurpose, ClassBodyFallback, EffectiveBinding};

        // An actual original FREE access is represented and remains accepted.
        lower_verified(concat!(
            "from __future__ import strict\n",
            "def factory(value):\n",
            "    class C:\n",
            "        seen = value\n",
            "    return C\n",
        ))
        .expect("a represented native class cell access remains supported");

        // The same spelling is a namespace local AND an independently forwarded
        // FREE cell for the method. Neither the name nor FREE alone is a veto.
        let module = lower_verified(concat!(
            "from __future__ import strict\n",
            "def factory():\n",
            "    __static_attributes__ = 'outer'\n",
            "    class Subject:\n",
            "        __static_attributes__ = ('manual',)\n",
            "        def captured(self):\n",
            "            return __static_attributes__\n",
            "        def method(self):\n",
            "            self.inferred = 1\n",
            "    return Subject\n",
        ))
        .expect("a namespace-local tail must not be rejected by a forwarded FREE name");
        let function = module
            .callable_defs
            .iter()
            .find(|function| {
                function.scope.class_bindings.as_ref().is_some_and(|class| {
                    class.source.lexical_qualname == "factory.<locals>.Subject"
                })
            })
            .unwrap();
        let class = function.scope.class_bindings.as_ref().unwrap();
        assert!(class
            .node
            .slots
            .iter()
            .any(|slot| { slot.name == "__static_attributes__" && slot.kind.is_free() }));
        assert_eq!(
            function
                .scope
                .effective_binding("__static_attributes__", BindingPurpose::Store),
            Some(EffectiveBinding::ClassBody(ClassBodyFallback::Global)),
        );
    }

    fn assert_native_namespace_inputs(
        function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>,
    ) {
        use soac_core::block_py::LocalLocation;
        let class = function
            .scope
            .class_bindings
            .as_ref()
            .expect("native class recipe");
        let layout = function.storage_layout.as_ref().unwrap();
        let projection = layout.class_bindings.as_ref().unwrap();
        projection.validate(class, layout, &function.scope).unwrap();
        let [namespace, execution] = function.params.params.as_slice() else {
            panic!("canonical class entry receives its namespace and execution handle")
        };
        assert_eq!(namespace.name, class.namespace_binding);
        assert_eq!(function.body_params().names(), function.params.names());
        let execution_slot = LocalLocation(
            u32::try_from(
                layout
                    .stack_slots
                    .iter()
                    .position(|name| name == &execution.name)
                    .expect("physical execution handle"),
            )
            .unwrap(),
        );
        assert_ne!(projection.namespace, execution_slot);
        assert!(!layout.is_expression_temporary(execution_slot));
        assert!(
            projection
                .slots
                .iter()
                .all(|slot| { slot.storage.raw_local(layout) != Some(execution_slot) }),
            "native cell primaries are recipe-created, not duplicate call arguments"
        );
    }

    #[test]
    fn strict_module_docstring_is_a_global_store_with_or_without_annotations() {
        use soac_core::block_py::{ChildVisitable, Visit};
        use soac_ir_blockpy::InstrBlockPy;

        struct DocStores(usize);
        impl Visit<InstrBlockPy> for DocStores {
            fn visit_instr(&mut self, instruction: &InstrBlockPy) {
                if matches!(instruction, InstrBlockPy::Store(store) if store.name.id.as_str() == "__doc__")
                {
                    self.0 += 1;
                }
                instruction.visit_children(self);
            }
        }
        for future in ["", ", annotations"] {
            for body in ["VALUE = 1\n", "VALUE: int = 1\n"] {
                let source =
                    format!("\"module docs\"\nfrom __future__ import strict{future}\n{body}");
                // Fixed annotation input for this lowering fixture; class
                // bindings come from actual native compilation. Annotation
                // projection is separately tested through the loader.
                let entries = source.find(": int").map(|offset| {
                    let start = (offset + 2) as u32;
                    (SourceRange::new(start, start + 3), "int".to_owned())
                });
                let canonical =
                    crate::CanonicalAnnotationStrings::from_native_entries(&source, entries)
                        .unwrap();
                let module = crate::lower_source_to_blockpy_module_with_tracker(
                    &source,
                    ModuleNameGen::new(1),
                    &mut RecordingPassTracker::new(),
                    crate::LoweringOptions {
                        strict_facts: Some(verified_source(&source)),
                        canonical_annotations: Some(Arc::new(canonical)),
                        canonical_class_bindings: Some(
                            super::native_class_bindings::for_source(&source).unwrap(),
                        ),
                        ..Default::default()
                    },
                )
                .unwrap();
                let initializer = module
                    .callable_defs
                    .iter()
                    .find(|function| function.names.bind_name == "_dp_module_init")
                    .unwrap();
                let mut stores = DocStores(0);
                stores.visit_fn(initializer);
                assert_eq!(stores.0, 1, "the module needs its own __doc__ binding");
                assert!(
                    initializer.doc.is_none(),
                    "module documentation is not only hidden initializer metadata"
                );
            }
        }
    }

    #[test]
    fn generator_capture_slots_follow_the_same_public_and_resume_order() {
        use soac_core::block_py::{
            CellLocation, ChildVisitable, FunctionKind, NameLocation, Visit,
        };
        use soac_ir_blockpy::InstrBlockPy;

        for ordinary in [
            "def make():\n    a = 1\n    gen = (b := a + i for i in range(2))\n    return a, list(gen), b\n",
            "def make():\n    a = 1\n    b = None\n    def values():\n        nonlocal b\n        for i in range(2):\n            b = a + i\n            yield b\n    return a, list(values()), b\n",
        ] {
            for strict in [false, true] {
                let source = if strict {
                    format!("from __future__ import strict\n{ordinary}")
                } else {
                    ordinary.to_owned()
                };
                let facts = strict.then(|| verified_source(&source));
                let module = lower(&source, facts).expect("generator capture fixture lowers");
                let function = module
                    .callable_defs
                    .iter()
                    .find(|function| matches!(function.lowered_kind(), FunctionKind::Generator))
                    .expect("one generator callable");
                let public = function.public_storage_layout().unwrap();
                let body = function.storage_layout.as_ref().unwrap();
                let public_names = public.freevars.iter().map(|slot| slot.logical_name.as_str()).collect::<Vec<_>>();
                let body_names = body.freevars.iter().map(|slot| slot.logical_name.as_str()).collect::<Vec<_>>();
                assert_eq!(public_names, ["a", "b"], "public closure follows logical capture order, not assignment order");
                assert_eq!(body_names, public_names, "resume reads the same actual closure slots as function creation");

                struct CaptureAccesses { reads: usize, writes: usize }
                impl Visit<InstrBlockPy> for CaptureAccesses {
                    fn visit_instr(&mut self, instruction: &InstrBlockPy) {
                        match instruction {
                            InstrBlockPy::Load(load) if load.name.id.as_str() == "a" => {
                                assert!(matches!(
                                    load.name.location,
                                    NameLocation::Cell(
                                        CellLocation::Closure(0)
                                            | CellLocation::CapturedSource(0)
                                    )
                                ), "read/write capture projection must index the same public cell");
                                self.reads += 1;
                            }
                            InstrBlockPy::Store(store) if store.name.id.as_str() == "b" => {
                                assert!(matches!(
                                    store.name.location,
                                    NameLocation::Cell(
                                        CellLocation::Closure(1)
                                            | CellLocation::CapturedSource(1)
                                    )
                                ), "read/write capture projection must index the same public cell");
                                self.writes += 1;
                            }
                            _ => {}
                        }
                        instruction.visit_children(self);
                    }
                }
                let mut accesses = CaptureAccesses { reads: 0, writes: 0 };
                accesses.visit_fn(function);
                assert!(accesses.reads > 0 && accesses.writes > 0, "actual capture loads and stores must be checked");
            }
        }
    }

    #[test]
    fn generator_expression_code_exposure_keeps_exact_parser_ranges_without_source_admission() {
        use soac_core::block_py::FunctionKind;
        let source = concat!(
            "from __future__ import strict\ndef same_line(values):\n    return (value for value in values), (value + 1 for value in values)\ndef nested(values):\n    return ((value for value in row) for row in values)\ndef multiline(values):\n    return (\n        value\n        for value in values\n    )\nasync def asynchronous(values):\n    return (value async for value in values)\n",
            "def call_one(values):\n    return tuple(implicit_item for implicit_item in values)\ndef call_multiline(values):\n    return tuple(\n        filtered_item\n        for filtered_item in values\n        if filtered_item\n    )\ndef call_parenthesized(values):\n    return tuple((explicit_item for explicit_item in values))\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        let mut actual = Vec::new();
        for function in &module.callable_defs {
            let Some(projection) = &function.scope.generator_expression_code else {
                continue;
            };
            assert!(
                function.scope.source_origin.is_none(),
                "a genexpr is a compiler-created helper, not an invented signed function"
            );
            assert_eq!(
                function.public_scope().generator_expression_code.as_ref(),
                Some(projection)
            );
            assert_eq!(function.params.len(), 1);
            actual.push((
                projection.expression_range.start,
                &source[projection.expression_range.start as usize
                    ..projection.expression_range.end as usize],
                &source[projection.iterable_range.start as usize
                    ..projection.iterable_range.end as usize],
                *function.lowered_kind(),
            ));
        }
        actual.sort_by_key(|item| item.0);
        let mut expected = [
            (
                "(value for value in values)",
                "values",
                FunctionKind::Generator,
            ),
            (
                "(value + 1 for value in values)",
                "values",
                FunctionKind::Generator,
            ),
            (
                "((value for value in row) for row in values)",
                "values",
                FunctionKind::Generator,
            ),
            ("(value for value in row)", "row", FunctionKind::Generator),
            (
                "(\n        value\n        for value in values\n    )",
                "values",
                FunctionKind::Generator,
            ),
            (
                "(value async for value in values)",
                "values",
                FunctionKind::AsyncGenerator,
            ),
            (
                "(implicit_item for implicit_item in values)",
                "values",
                FunctionKind::Generator,
            ),
            (
                "(\n        filtered_item\n        for filtered_item in values\n        if filtered_item\n    )",
                "values",
                FunctionKind::Generator,
            ),
            (
                "(explicit_item for explicit_item in values)",
                "values",
                FunctionKind::Generator,
            ),
        ]
        .map(|(expression, iterable, kind)| {
            (
                source.find(expression).unwrap() as u32,
                expression,
                iterable,
                kind,
            )
        });
        expected.sort_by_key(|item| item.0);
        assert_eq!(actual, expected);
        let ordinary = source.replacen("from __future__ import strict\n", "", 1);
        assert!(lower(&ordinary, None)
            .unwrap()
            .callable_defs
            .iter()
            .all(|function| function.scope.generator_expression_code.is_none()));
    }

    #[test]
    fn strict_source_cannot_be_enabled_by_an_alias_or_an_unverified_artifact() {
        let source = "from __future__ import strict as feature\nvalue = 1\n";
        assert!(lower(source, None).is_err());
        let ordinary = "from __future__ import annotations as strict\nvalue = 1\n";
        assert!(lower(ordinary, None).unwrap().strict_source.is_none());
        assert!(lower(ordinary, Some(verified_source(ordinary))).is_err());
        assert!(lower(
            "from __future__ import strict\nvalue = 2\n",
            Some(verified_source(source))
        )
        .is_err());
    }

    #[test]
    fn private_class_capture_uses_owned_cells_without_public_closure_changes() {
        use ruff_python_ast::Stmt;
        use ruff_text_size::Ranged;
        use soac_core::block_py::{
            walk_fn, BindingKind, CellLocation, ChildVisitable, MakeFunctionWithClosure,
            PreservedSlotStorage, ResolvedName, RuntimeFunctionId, Visit,
        };
        use soac_ir_blockpy::InstrBlockPy;

        for (header, suspend, tail, kind) in [
            (
                "def",
                "",
                "return Target, Holder",
                soac_contracts::FunctionKind::Synchronous,
            ),
            (
                "def",
                "    yield None\n",
                "yield Target, Holder",
                soac_contracts::FunctionKind::Generator,
            ),
            (
                "async def",
                "",
                "return Target, Holder",
                soac_contracts::FunctionKind::Coroutine,
            ),
        ] {
            let source = format!("from __future__ import strict\n{header} build():\n    class Target:\n        pass\n{suspend}    class Holder:\n        def __init__(self, value):\n            self.payload: Target = value\n    {tail}\n");
            let parsed = ruff_python_parser::parse_module(&source).unwrap();
            let Stmt::FunctionDef(builder) = &parsed.syntax().body[1] else {
                panic!("builder");
            };
            let target = builder
                .body
                .iter()
                .find_map(|stmt| match stmt {
                    Stmt::ClassDef(class) if class.name.as_str() == "Target" => Some(class),
                    _ => None,
                })
                .unwrap();
            let holder = builder
                .body
                .iter()
                .find_map(|stmt| match stmt {
                    Stmt::ClassDef(class) if class.name.as_str() == "Holder" => Some(class),
                    _ => None,
                })
                .unwrap();
            let Stmt::FunctionDef(initializer) = &holder.body[0] else {
                panic!("initializer");
            };
            let Stmt::AnnAssign(assignment) = &initializer.body[0] else {
                panic!("field annotation");
            };
            let module =
                ModuleContentId::new("pkg.strict_origins", legacy_source_hash(source.as_bytes()));
            let identity =
                |name: &str, range: ruff_text_size::TextRange, definition_kind| SourceIdentity {
                    module: module.clone(),
                    lexical_qualname: name.into(),
                    source_range: SourceRange::new(range.start().to_u32(), range.end().to_u32()),
                    definition_kind,
                };
            let producer = identity("build", builder.range, DefinitionKind::Function);
            let target = ClassReference {
                definition: identity("build.<locals>.Target", target.range, DefinitionKind::Class),
                source_digest: Fingerprint::digest(source.as_bytes()),
            };
            let holder = ClassReference {
                definition: identity("build.<locals>.Holder", holder.range, DefinitionKind::Class),
                source_digest: target.source_digest,
            };
            let field = FieldTypeFact {
                name: "payload".into(),
                declaring_class: holder.clone(),
                value_type: StaticType::NominalClass(target.clone()),
                annotation_origin: AnnotationOrigin::Explicit,
                annotation_definition: Some(identity(
                    "build.<locals>.Holder.__init__.<locals>.<binding>",
                    assignment.range,
                    DefinitionKind::Assignment,
                )),
                field_kind: FieldKind::InstanceField,
                read_policy: FieldReadPolicy::PythonAttribute,
                write_policy: FieldWritePolicy::DeclaredField,
                initialization: InitializationPolicy::MayBeAbsent,
                default: DefaultFact::Missing,
                descriptor: DescriptorFact::default(),
                uncertainty: Default::default(),
            };
            let leaf = NominalBindingFact {
                owner: NominalBindingOwner::Field {
                    field: field.annotation_reference().unwrap(),
                },
                expression_range: SourceRange::new(
                    assignment.annotation.range().start().to_u32(),
                    assignment.annotation.range().end().to_u32(),
                ),
                name: "Target".into(),
                class: target.clone(),
                binding: target.definition.clone(),
                binding_scope: producer.clone(),
            };
            let class = |reference: &ClassReference, instance_fields| ClassTypeFact {
                identity: reference.definition.clone(),
                bases: Vec::new(),
                metaclass: MetaclassFact::BuiltinType,
                decorators: Vec::new(),
                participation: ParticipationProposal::Candidate,
                dictionary: ClassDictionarySemantics::DictionaryBearing,
                instance_fields,
                methods: Vec::new(),
                class_members: Vec::new(),
                inheritance: InheritanceFact {
                    linearized_bases: Vec::new(),
                    complete: true,
                },
                openness: ClassOpenness::OpenSubclassFamily,
                transform: None,
                uncertainty: [UncertaintyReason::OpenWorld].into(),
            };
            let functions = vec![FunctionTypeFact {
                identity: producer.clone(),
                function_kind: kind,
                signature: CallableSignature {
                    parameters: Vec::new(),
                    return_type: StaticType::Unknown,
                    return_annotation_origin: AnnotationOrigin::Absent,
                    uncertainty: Default::default(),
                },
                decorators: Vec::new(),
                uncertainty: Default::default(),
            }];
            // This explicit catalog tests compiler storage/projection only;
            // genuine offline publication and native behavior live in the
            // checked-field factory integration family.
            let facts = verified_source_with_nominal_catalog_policy(
                &source,
                vec![
                    class(&target, Vec::new()),
                    class(&holder, vec![field.clone()]),
                ],
                functions.clone(),
                vec![leaf.clone()],
                Vec::new(),
                ResolvedStrictPolicy {
                    checked_fields: soac_contracts::CheckedFieldPolicy::SupportedAnnotations,
                    ..Default::default()
                },
            );
            let lowered = lower(&source, Some(facts)).unwrap();
            let body = lowered
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::SourceFunction
                            && origin.definition == producer
                    })
                })
                .unwrap();
            let helper = lowered
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::ClassConstruction
                            && origin.definition == holder.definition
                    })
                })
                .unwrap();
            let namespace = lowered
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::ClassNamespace
                            && origin.definition == holder.definition
                    })
                })
                .unwrap();
            let plan = helper
                .scope
                .class_construction
                .as_ref()
                .expect("method-only field capture plan");
            assert_eq!(plan.producer.definition, producer);
            assert_eq!(plan.producer.role, CallableSourceRole::SourceFunction);
            assert_eq!(plan.namespace_function, namespace.function_id);
            assert_eq!(plan.captures.len(), 1);
            assert_eq!(plan.captures[0].binding.name, "Target");
            assert_eq!(plan.captures[0].binding.scope, producer);
            assert_eq!(plan.captures[0].nominal_binding_indices, vec![0]);
            assert_eq!(
                body.public_scope().binding_kind("Target"),
                Some(BindingKind::Local)
            );
            for function in [body, helper, namespace] {
                assert!(!function
                    .public_storage_layout()
                    .unwrap()
                    .freevars
                    .iter()
                    .any(|slot| slot.logical_name == "Target"));
            }
            struct Probe {
                helper: RuntimeFunctionId,
                created: Vec<MakeFunctionWithClosure<InstrBlockPy>>,
                guard: Vec<ResolvedName>,
                discarded: Vec<ResolvedName>,
                helper_name: String,
            }
            impl Visit<InstrBlockPy> for Probe {
                fn visit_instr(&mut self, node: &InstrBlockPy) {
                    match node {
                        InstrBlockPy::MakeFunctionWithClosure(op)
                            if op.function_id == self.helper =>
                        {
                            self.created.push(op.clone())
                        }
                        InstrBlockPy::Store(op) if matches!(op.value.as_ref(), InstrBlockPy::Load(load) if load.name.id.as_str() == self.helper_name) => {
                            self.guard.push(op.name.clone())
                        }
                        InstrBlockPy::DiscardClassConstructionCaptures(op) => {
                            let InstrBlockPy::Load(load) = op.function.as_ref() else {
                                panic!("original helper cleanup operand");
                            };
                            self.discarded.push(load.name.clone());
                        }
                        _ => {}
                    }
                    node.visit_children(self);
                }
            }
            let mut probe = Probe {
                helper: helper.function_id,
                created: Vec::new(),
                guard: Vec::new(),
                discarded: Vec::new(),
                helper_name: helper.names.bind_name.clone(),
            };
            walk_fn(&mut probe, body);
            let [created] = probe.created.as_slice() else {
                panic!("one helper creation");
            };
            assert!(
                matches!(created.class_namespace.as_deref(), Some(InstrBlockPy::Load(load)) if load.name.id.as_str() == namespace.names.bind_name)
            );
            let [InstrBlockPy::CellRef(cell)] = created.creation_cells.as_slice() else {
                panic!("one original CellRef operand");
            };
            let layout = body.storage_layout.as_ref().unwrap();
            match cell.location {
                CellLocation::Owned(index) => {
                    assert_eq!(layout.owned_slot(index).unwrap().logical_name, "Target")
                }
                CellLocation::Preserved(index) => {
                    let slot = layout.preserved_slot(index).unwrap();
                    assert_eq!(slot.logical_name, "Target");
                    assert_eq!(slot.storage, PreservedSlotStorage::PyCellObject);
                    assert_eq!(
                        body.public_storage_layout().unwrap().preserved_slots[index as usize],
                        *slot
                    );
                }
                _ => panic!("private owner cannot be a fabricated public closure capture"),
            }
            assert!(
                probe.guard.iter().any(|guard| probe
                    .discarded
                    .iter()
                    .any(|discarded| discarded.location == guard.location)),
                "cleanup retains the exact original helper binding"
            );
            assert!(
                body.blocks.iter().any(|block| block.exc_edge.is_some()
                    && block.body.iter().any(|node| soac_core::block_py::instr_any(
                        node,
                        |node| matches!(node, InstrBlockPy::Call(_))
                    ))),
                "class argument/body failure must enter an explicit cleanup region"
            );

            let unchecked = lower(
                &source,
                Some(verified_source_with_nominal_catalog(
                    &source,
                    vec![class(&target, Vec::new()), class(&holder, vec![field])],
                    functions,
                    vec![leaf],
                    Vec::new(),
                )),
            )
            .unwrap();
            let unchecked_body = unchecked
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::SourceFunction
                            && origin.definition == producer
                    })
                })
                .unwrap();
            let unchecked_helper = unchecked
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::ClassConstruction
                            && origin.definition == holder.definition
                    })
                })
                .unwrap();
            assert!(unchecked_helper.scope.class_construction.is_none());
            assert_eq!(
                unchecked_body.scope.binding_kind("Target"),
                Some(BindingKind::Local),
                "an unchecked method-only annotation must not add a private cell edge"
            );
            let mut unchecked_probe = Probe {
                helper: unchecked_helper.function_id,
                created: Vec::new(),
                guard: Vec::new(),
                discarded: Vec::new(),
                helper_name: unchecked_helper.names.bind_name.clone(),
            };
            walk_fn(&mut unchecked_probe, unchecked_body);
            let [unchecked_creation] = unchecked_probe.created.as_slice() else {
                panic!("one unchecked helper creation");
            };
            assert!(unchecked_creation.class_namespace.is_none());
            assert!(unchecked_creation.creation_cells.is_empty());
        }
    }

    #[test]
    fn private_field_cells_follow_native_private_and_namespace_lexical_paths() {
        use ruff_python_ast::Stmt;
        use ruff_text_size::Ranged;
        use soac_core::block_py::{
            walk_fn, BindingKind, CellBindingKind, CellLocation, ChildVisitable,
            MakeFunctionWithClosure, RuntimeFunctionId, Visit,
        };
        use soac_ir_blockpy::InstrBlockPy;

        struct Creations(Vec<MakeFunctionWithClosure<InstrBlockPy>>);
        impl Visit<InstrBlockPy> for Creations {
            fn visit_instr(&mut self, node: &InstrBlockPy) {
                if let InstrBlockPy::MakeFunctionWithClosure(op) = node {
                    self.0.push(op.clone());
                }
                node.visit_children(self);
            }
        }
        let creation = |function: &soac_core::block_py::BlockPyFunction<BlockPyModuleShape>,
                        target: RuntimeFunctionId| {
            let mut found = Creations(Vec::new());
            walk_fn(&mut found, function);
            let mut matches = found.0.into_iter().filter(|op| op.function_id == target);
            let result = matches.next().expect("actual child creation");
            assert!(matches.next().is_none());
            result
        };

        for (case, header, prefix, tail, bridge_role) in [
            (
                "native",
                "def create():",
                "        expected = Target\n",
                "        return Holder\n",
                CallableSourceRole::SourceFunction,
            ),
            (
                "private",
                "def create():",
                "",
                "        return Holder\n",
                CallableSourceRole::SourceFunction,
            ),
            (
                "generator",
                "def create():",
                "        yield None\n",
                "        yield Holder\n",
                CallableSourceRole::SourceFunction,
            ),
            (
                "coroutine",
                "async def create():",
                "",
                "        return Holder\n",
                CallableSourceRole::SourceFunction,
            ),
            (
                "namespace",
                "class Outer:",
                "",
                "",
                CallableSourceRole::ClassNamespace,
            ),
        ] {
            let source = format!("from __future__ import strict\ndef build():\n    class Target:\n        pass\n    {header}\n{prefix}        class Holder:\n            def __init__(self, value):\n                self.payload: Target = value\n{tail}    return Target\n");
            let parsed = ruff_python_parser::parse_module(&source).unwrap();
            let Stmt::FunctionDef(builder) = &parsed.syntax().body[1] else {
                panic!("builder");
            };
            let Stmt::ClassDef(target_ast) = &builder.body[0] else {
                panic!("target");
            };
            let (bridge_name, bridge_range, bridge_body, bridge_kind) = match &builder.body[1] {
                Stmt::FunctionDef(function) => (
                    "build.<locals>.create",
                    function.range,
                    &function.body,
                    DefinitionKind::Function,
                ),
                Stmt::ClassDef(class) => (
                    "build.<locals>.Outer",
                    class.range,
                    &class.body,
                    DefinitionKind::Class,
                ),
                _ => panic!("lexical bridge"),
            };
            let holder_ast = bridge_body
                .iter()
                .find_map(|stmt| match stmt {
                    Stmt::ClassDef(class) => Some(class),
                    _ => None,
                })
                .unwrap();
            let Stmt::FunctionDef(initializer) = &holder_ast.body[0] else {
                panic!("initializer");
            };
            let Stmt::AnnAssign(assignment) = &initializer.body[0] else {
                panic!("field");
            };
            let module =
                ModuleContentId::new("pkg.strict_origins", legacy_source_hash(source.as_bytes()));
            let identity =
                |name: &str, range: ruff_text_size::TextRange, definition_kind| SourceIdentity {
                    module: module.clone(),
                    lexical_qualname: name.into(),
                    source_range: SourceRange::new(range.start().to_u32(), range.end().to_u32()),
                    definition_kind,
                };
            let owner = identity("build", builder.range, DefinitionKind::Function);
            let bridge = identity(bridge_name, bridge_range, bridge_kind);
            let holder_name = if bridge_kind == DefinitionKind::Function {
                format!("{bridge_name}.<locals>.Holder")
            } else {
                format!("{bridge_name}.Holder")
            };
            let target = ClassReference {
                definition: identity(
                    "build.<locals>.Target",
                    target_ast.range,
                    DefinitionKind::Class,
                ),
                source_digest: Fingerprint::digest(source.as_bytes()),
            };
            let holder = ClassReference {
                definition: identity(&holder_name, holder_ast.range, DefinitionKind::Class),
                source_digest: target.source_digest,
            };
            let field = FieldTypeFact {
                name: "payload".into(),
                declaring_class: holder.clone(),
                value_type: StaticType::NominalClass(target.clone()),
                annotation_origin: AnnotationOrigin::Explicit,
                annotation_definition: Some(identity(
                    &format!("{holder_name}.__init__.<locals>.<binding>"),
                    assignment.range,
                    DefinitionKind::Assignment,
                )),
                field_kind: FieldKind::InstanceField,
                read_policy: FieldReadPolicy::PythonAttribute,
                write_policy: FieldWritePolicy::DeclaredField,
                initialization: InitializationPolicy::MayBeAbsent,
                default: DefaultFact::Missing,
                descriptor: DescriptorFact::default(),
                uncertainty: Default::default(),
            };
            let leaf = NominalBindingFact {
                owner: NominalBindingOwner::Field {
                    field: field.annotation_reference().unwrap(),
                },
                expression_range: SourceRange::new(
                    assignment.annotation.range().start().to_u32(),
                    assignment.annotation.range().end().to_u32(),
                ),
                name: "Target".into(),
                class: target.clone(),
                binding: target.definition.clone(),
                binding_scope: owner.clone(),
            };
            let class = |identity, instance_fields| ClassTypeFact {
                identity,
                bases: Vec::new(),
                metaclass: MetaclassFact::BuiltinType,
                decorators: Vec::new(),
                participation: ParticipationProposal::Candidate,
                dictionary: ClassDictionarySemantics::DictionaryBearing,
                instance_fields,
                methods: Vec::new(),
                class_members: Vec::new(),
                inheritance: InheritanceFact {
                    linearized_bases: Vec::new(),
                    complete: true,
                },
                openness: ClassOpenness::OpenSubclassFamily,
                transform: None,
                uncertainty: [UncertaintyReason::OpenWorld].into(),
            };
            let mut classes = vec![
                class(target.definition.clone(), Vec::new()),
                class(holder.definition.clone(), vec![field]),
            ];
            if bridge_role == CallableSourceRole::ClassNamespace {
                classes.push(class(bridge.clone(), Vec::new()));
            }
            let function = |identity, function_kind| FunctionTypeFact {
                identity,
                function_kind,
                signature: CallableSignature {
                    parameters: Vec::new(),
                    return_type: StaticType::Unknown,
                    return_annotation_origin: AnnotationOrigin::Absent,
                    uncertainty: Default::default(),
                },
                decorators: Vec::new(),
                uncertainty: Default::default(),
            };
            let mut functions = vec![function(owner.clone(), FunctionKind::Synchronous)];
            if bridge_role == CallableSourceRole::SourceFunction {
                functions.push(function(
                    bridge.clone(),
                    match case {
                        "generator" => FunctionKind::Generator,
                        "coroutine" => FunctionKind::Coroutine,
                        _ => FunctionKind::Synchronous,
                    },
                ));
            }
            let facts = verified_source_with_nominal_catalog_policy(
                &source,
                classes.clone(),
                functions.clone(),
                vec![leaf.clone()],
                Vec::new(),
                ResolvedStrictPolicy {
                    checked_fields: CheckedFieldPolicy::SupportedAnnotations,
                    ..Default::default()
                },
            );
            let lowered = lower(&source, Some(facts)).unwrap();
            let find = |identity: &SourceIdentity, role| {
                lowered
                    .callable_defs
                    .iter()
                    .find(|function| {
                        function.scope.source_origin.as_ref().is_some_and(|origin| {
                            &origin.definition == identity && origin.role == role
                        })
                    })
                    .unwrap()
            };
            let owner_fn = find(&owner, CallableSourceRole::SourceFunction);
            let bridge_fn = find(&bridge, bridge_role);
            let helper = find(&holder.definition, CallableSourceRole::ClassConstruction);
            let projection = bridge_fn.scope.private_lexical.as_ref().expect(case);
            assert_eq!(projection.creator.definition, owner, "{case}");
            let [projected] = projection.captures.as_slice() else {
                panic!("one lexical binding: {case}");
            };
            assert_eq!(projected.cell.binding.scope, owner);
            assert_eq!(projected.cell.binding.name, "Target");
            assert_eq!(projected.cell.nominal_binding_indices, vec![0]);
            let child_creation = creation(bridge_fn, helper.function_id);
            let [InstrBlockPy::CellRef(cell)] = child_creation.creation_cells.as_slice() else {
                panic!("one child cell: {case}");
            };
            if case == "native" {
                assert_eq!(projected.native_closure.as_deref(), Some("Target"));
                assert!(matches!(
                    cell.location,
                    CellLocation::Closure(_) | CellLocation::CapturedSource(_)
                ));
                assert_eq!(
                    bridge_fn.public_scope().binding_kind("Target"),
                    Some(BindingKind::Cell(CellBindingKind::Capture))
                );
                assert!(
                    creation(owner_fn, bridge_fn.function_id)
                        .creation_cells
                        .is_empty(),
                    "native cells must use the active public closure"
                );
                assert!(owner_fn.scope.private_lexical.is_none());
            } else {
                assert_eq!(projected.native_closure, None);
                assert_eq!(cell.location, CellLocation::Private(0), "{case}");
                assert_eq!(
                    owner_fn.public_scope().binding_kind("Target"),
                    Some(BindingKind::Local)
                );
                assert!(
                    !bridge_fn
                        .public_storage_layout()
                        .unwrap()
                        .freevars
                        .iter()
                        .any(|slot| slot.logical_name == "Target"),
                    "{case}: private cells must not become public closure slots: {:?}",
                    bridge_fn.public_storage_layout()
                );
                let source_creation = creation(owner_fn, bridge_fn.function_id);
                if bridge_role == CallableSourceRole::ClassNamespace {
                    assert!(
                        source_creation.creation_cells.is_empty(),
                        "namespace cells are handle-owned, never persistent helper captures"
                    );
                    let outer_helper = find(&bridge, CallableSourceRole::ClassConstruction);
                    let outer_plan = outer_helper.scope.class_construction.as_ref().unwrap();
                    assert_eq!(outer_plan.namespace_function, bridge_fn.function_id);
                    assert_eq!(outer_plan.captures, vec![projected.cell.clone()]);
                    assert_eq!(
                        creation(owner_fn, outer_helper.function_id)
                            .creation_cells
                            .len(),
                        1
                    );
                } else {
                    let [InstrBlockPy::CellRef(original)] =
                        source_creation.creation_cells.as_slice()
                    else {
                        panic!("original private cell: {case}");
                    };
                    assert!(matches!(
                        original.location,
                        CellLocation::Owned(_) | CellLocation::Preserved(_)
                    ));
                }
            }
            if case == "private" {
                classes
                    .iter_mut()
                    .find(|class| class.identity == holder.definition)
                    .unwrap()
                    .participation =
                    ParticipationProposal::Dynamic([DynamicClassReason::FrameworkManaged].into());
                let excluded = lower(
                    &source,
                    Some(verified_source_with_nominal_catalog_policy(
                        &source,
                        classes,
                        functions,
                        vec![leaf],
                        Vec::new(),
                        ResolvedStrictPolicy {
                            checked_fields: CheckedFieldPolicy::SupportedAnnotations,
                            ..Default::default()
                        },
                    )),
                )
                .unwrap();
                assert!(
                    excluded.callable_defs.iter().all(|function| function
                        .scope
                        .private_lexical
                        .is_none()
                        && function.scope.class_construction.is_none()),
                    "a statically dynamic class cannot create private lifetime edges"
                );
                let owner_fn = excluded
                    .callable_defs
                    .iter()
                    .find(|function| {
                        function.scope.source_origin.as_ref().is_some_and(|origin| {
                            origin.role == CallableSourceRole::SourceFunction
                                && origin.definition == owner
                        })
                    })
                    .unwrap();
                assert_eq!(
                    owner_fn.scope.binding_kind("Target"),
                    Some(BindingKind::Local)
                );
            }
        }
    }

    #[test]
    fn strict_origins_survive_private_names_nested_classes_and_generator_lowering() {
        let source = concat!(
            "from __future__ import strict\n",
            "def outer():\n",
            "    class Item:\n",
            "        def __private(self):\n",
            "            return lambda: self\n",
            "        def values(self):\n",
            "            yield self\n",
            "    return Item\n",
            "def _dp_define_class_Item():\n",
            "    return 17\n",
        );
        let facts = verified_source(source);
        let module = lower(source, Some(facts.clone())).unwrap();
        assert!(module
            .strict_source
            .as_ref()
            .unwrap()
            .matches_verified(&facts));
        let origins = module
            .callable_defs
            .iter()
            .filter_map(|function| function.scope.source_origin.as_ref())
            .collect::<Vec<_>>();
        let find = |name: &str, role| {
            origins
                .iter()
                .find(|origin| origin.definition.lexical_qualname == name && origin.role == role)
                .copied()
                .unwrap()
        };
        assert_eq!(
            find("<module>", CallableSourceRole::ModuleBody)
                .definition
                .source_range,
            SourceRange::new(0, source.len() as u32)
        );
        let namespace = find("outer.<locals>.Item", CallableSourceRole::ClassNamespace);
        let constructor = find("outer.<locals>.Item", CallableSourceRole::ClassConstruction);
        assert_eq!(namespace.definition, constructor.definition);
        assert_eq!(namespace.definition.definition_kind, DefinitionKind::Class);
        let private = find(
            "outer.<locals>.Item.__private",
            CallableSourceRole::SourceFunction,
        );
        assert!(source[private.definition.source_range.start as usize
            ..private.definition.source_range.end as usize]
            .starts_with("def __private"));
        find(
            "outer.<locals>.Item.__private.<locals>.<lambda>",
            CallableSourceRole::SourceFunction,
        );
        find(
            "outer.<locals>.Item.values",
            CallableSourceRole::SourceFunction,
        );
        find("_dp_define_class_Item", CallableSourceRole::SourceFunction);
        assert!(!origins
            .iter()
            .any(
                |origin| origin.definition.lexical_qualname == "_dp_define_class_Item"
                    && origin.role == CallableSourceRole::ClassConstruction
            ));
        // Serialization preserves provenance, but deserializing these public
        // bytes grants no runtime authority: match the independent facts again.
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&module).unwrap();
        let restored: BlockPyModule<BlockPyModuleShape> =
            rkyv::from_bytes::<_, rkyv::rancor::Error>(&bytes).unwrap();
        assert_eq!(restored.strict_source, module.strict_source);
        for (before, after) in module.callable_defs.iter().zip(&restored.callable_defs) {
            assert_eq!(before.scope.source_origin, after.scope.source_origin);
        }
    }

    #[test]
    fn strict_global_definitions_keep_lexical_owners_and_native_qualnames_distinct() {
        use crate::pass_tracker::LoweringPassTrackerExt;
        use crate::transformer::{walk_stmt, Transformer};
        use ruff_python_ast::{self as ast, Expr, Stmt};

        let source = r#"from __future__ import strict
def build():
    saved = object()
    if False:
        global Y, released
    class Y:
        class Inner:
            pass
        def member(self):
            return lambda: saved
    def released():
        def child():
            return saved
        return child
    class Local:
        pass
    return Y, released, Local
"#;
        let mut tracker = RecordingPassTracker::new();
        let module = crate::lower_source_to_blockpy_module_with_tracker(
            source,
            ModuleNameGen::new(1),
            &mut tracker,
            crate::LoweringOptions {
                strict_facts: Some(verified_source(source)),
                canonical_class_bindings: Some(
                    super::native_class_bindings::for_source(source).unwrap(),
                ),
                ..Default::default()
            },
        )
        .unwrap();
        for (lexical, native) in [
            ("build.<locals>.Y.member", "Y.member"),
            (
                "build.<locals>.Y.member.<locals>.<lambda>",
                "Y.member.<locals>.<lambda>",
            ),
            ("build.<locals>.released", "released"),
            (
                "build.<locals>.released.<locals>.child",
                "released.<locals>.child",
            ),
        ] {
            let function = module
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::SourceFunction
                            && origin.definition.lexical_qualname == lexical
                    })
                })
                .unwrap();
            assert_eq!(function.names.qualname, native, "{lexical}");
            assert!(
                function
                    .public_storage_layout()
                    .unwrap()
                    .freevars
                    .iter()
                    .any(|slot| slot.logical_name == "saved"),
                "a global binding changes the native name, not the lexical capture owner"
            );
        }

        for lexical in [
            "build.<locals>.Y",
            "build.<locals>.Y.Inner",
            "build.<locals>.Local",
        ] {
            assert!(
                module.callable_defs.iter().any(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::ClassNamespace
                            && origin.definition.lexical_qualname == lexical
                    })
                }),
                "namespace still belongs to {lexical}"
            );
        }

        #[derive(Default)]
        struct NamespaceQualnames(Vec<String>);
        impl Transformer for NamespaceQualnames {
            fn visit_stmt(&mut self, statement: &mut Stmt) {
                if let Stmt::Assign(ast::StmtAssign { targets, value, .. }) = statement {
                    if targets.iter().any(|target| {
                        matches!(
                            target,
                            Expr::Subscript(subscript) if matches!(
                                subscript.slice.as_ref(),
                                Expr::StringLiteral(key) if key.value.to_string() == "__qualname__"
                            )
                        )
                    }) {
                        let Expr::StringLiteral(value) = value.as_ref() else {
                            panic!("class qualname is an original-scope string literal");
                        };
                        self.0.push(value.value.to_string());
                    }
                }
                walk_stmt(self, statement);
            }
        }
        let mut rewritten = tracker.pass_ast_to_ast().unwrap();
        let mut probe = NamespaceQualnames::default();
        probe.visit_body(&mut rewritten.body);
        probe.0.sort();
        assert_eq!(probe.0, ["Y", "Y.Inner", "build.<locals>.Local"]);
    }

    #[test]
    fn strict_lambda_native_names_preserve_nonclass_scopes_without_class_regions() {
        // Retain supported module/function/generator naming coverage. The
        // identical original mixed class/nonclass source and all eleven
        // original name assertions now run against actual native code data.
        let source = r#"from __future__ import strict
module_list = [lambda: module_index for module_index in range(2)]
module_set = {lambda: set_index for set_index in range(2)}
module_dict = {dict_index: lambda: dict_index for dict_index in range(2)}
module_generator = (lambda: generator_index for generator_index in range(2))
generator_input = (item for item in (lambda: range(2))())
nested = lambda: (lambda: "nested")
def factory():
    local_list = [lambda: local_index for local_index in range(2)]
    return local_list
"#;
        let module = lower(source, Some(verified_source(source))).unwrap();
        let expected = [
            ("lambda: module_index", "<lambda>", "<lambda>"),
            ("lambda: set_index", "<lambda>", "<lambda>"),
            ("lambda: dict_index", "<lambda>", "<lambda>"),
            ("lambda: generator_index", "<lambda>", "<genexpr>.<lambda>"),
            ("lambda: range(2)", "<lambda>", "<lambda>"),
            (
                "lambda: \"nested\"",
                "<lambda>.<lambda>",
                "<lambda>.<locals>.<lambda>",
            ),
            (
                "lambda: local_index",
                "factory.<locals>.<lambda>",
                "factory.<locals>.<lambda>",
            ),
        ];
        for (expression, lexical, native) in expected {
            let start = source.find(expression).unwrap();
            let range = SourceRange::new(start as u32, (start + expression.len()) as u32);
            let function = module
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::SourceFunction
                            && origin.definition.definition_kind == DefinitionKind::Lambda
                            && origin.definition.source_range == range
                    })
                })
                .unwrap();
            let origin = function.scope.source_origin.as_ref().unwrap();
            assert_eq!(origin.definition.lexical_qualname, lexical, "{expression}");
            assert_eq!(function.names.qualname, native, "{expression}");
            assert_eq!(function.names.display_name, "<lambda>");
        }
    }

    #[test]
    fn strict_lambda_defaults_are_lowered_in_the_enclosing_scope() {
        use soac_core::block_py::{walk_fn, ChildVisitable, RuntimeFunctionId, Visit};
        use soac_ir_blockpy::InstrBlockPy;

        #[derive(Default)]
        struct Creations(Vec<RuntimeFunctionId>);
        impl Visit<InstrBlockPy> for Creations {
            fn visit_instr(&mut self, instr: &InstrBlockPy) {
                match instr {
                    InstrBlockPy::MakeFunctionWithClosure(op) => self.0.push(op.function_id),
                    _ => {}
                }
                instr.visit_children(self);
            }
        }

        let source = r#"from __future__ import strict
MARKER = 7
module_lambda = lambda callback=(lambda: MARKER): callback()
def factory(value):
    return lambda callback=(lambda: value): callback()
"#;
        let module = lower(source, Some(verified_source(source))).unwrap();
        for (expression, owner_role, owner_name, captures) in [
            (
                "lambda: MARKER",
                CallableSourceRole::ModuleBody,
                "<module>",
                vec![],
            ),
            (
                "lambda: value",
                CallableSourceRole::SourceFunction,
                "factory",
                vec!["value"],
            ),
        ] {
            let start = source.find(expression).unwrap();
            let range = SourceRange::new(start as u32, (start + expression.len()) as u32);
            let default = module
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == CallableSourceRole::SourceFunction
                            && origin.definition.definition_kind == DefinitionKind::Lambda
                            && origin.definition.source_range == range
                    })
                })
                .unwrap();
            assert_eq!(
                default
                    .public_storage_layout()
                    .unwrap()
                    .freevars
                    .iter()
                    .map(|slot| slot.logical_name.as_str())
                    .collect::<Vec<_>>(),
                captures,
            );
            let owner = module
                .callable_defs
                .iter()
                .find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| {
                        origin.role == owner_role
                            && origin.definition.lexical_qualname == owner_name
                    })
                })
                .unwrap();
            let mut creations = Creations::default();
            walk_fn(&mut creations, owner);
            assert!(creations.0.contains(&default.function_id));
            for function in &module.callable_defs {
                if function.scope.source_origin.as_ref().is_some_and(|origin| {
                    origin.role == CallableSourceRole::SourceFunction
                        && origin.definition.definition_kind == DefinitionKind::Lambda
                }) {
                    let mut creations = Creations::default();
                    walk_fn(&mut creations, function);
                    assert!(!creations.0.contains(&default.function_id));
                }
            }
        }
    }

    #[test]
    fn annotation_provider_roles_come_from_the_rewrite_not_helper_names() {
        let source = concat!(
            "from __future__ import strict\n",
            "module_value: int = 1\n",
            "class Item:\n",
            "    value: int = 2\n",
            "    def method(self, number: int) -> int:\n",
            "        return number\n",
            "def actual(number: int) -> int:\n",
            "    return number\n",
            "def _dp_annotate_func_actual(format):\n",
            "    return {}\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        let origins = module
            .callable_defs
            .iter()
            .filter_map(|function| function.scope.source_origin.as_ref())
            .collect::<Vec<_>>();
        for name in ["<module>", "Item", "Item.method", "actual"] {
            assert_eq!(
                origins
                    .iter()
                    .filter(|origin| origin.definition.lexical_qualname == name
                        && origin.role == CallableSourceRole::AnnotationProvider)
                    .count(),
                1,
                "each generated provider identifies its real lexical owner"
            );
        }
        let user = origins
            .iter()
            .find(|origin| origin.definition.lexical_qualname == "_dp_annotate_func_actual")
            .expect("lookalike function must not be consumed as an annotation helper");
        assert_eq!(user.role, CallableSourceRole::SourceFunction);
        for function in &module.callable_defs {
            let Some(origin) = &function.scope.source_origin else {
                continue;
            };
            if origin.role != CallableSourceRole::AnnotationProvider {
                continue;
            }
            assert_eq!(function.params.params.len(), 1);
            assert_eq!(function.params.params[0].name, "format");
            assert_eq!(
                function.params.params[0].kind,
                soac_core::block_py::ParamKind::PosOnly
            );
            assert!(!function.params.params[0].has_default);
            let projection = function.scope.annotation_provider.as_ref().unwrap();
            assert_ne!(projection.body_format_parameter, "format");
            assert_eq!(
                function.body_params().params[0].name,
                projection.body_format_parameter
            );
            if origin.definition.lexical_qualname.starts_with("Item") {
                assert_eq!(
                    projection.class_dictionary.as_deref(),
                    Some("__classdict__")
                );
                let captures = &function.public_storage_layout().unwrap().freevars;
                assert_eq!(
                    captures
                        .iter()
                        .map(|slot| slot.logical_name.as_str())
                        .collect::<Vec<_>>(),
                    ["__classdict__"]
                );
                assert_eq!(
                    function.scope.cell_capture_projection("__classdict__"),
                    soac_core::block_py::CellCaptureProjection::CellObject
                );
            }
        }
    }

    #[test]
    fn strict_decorated_annotation_providers_keep_distinct_class_and_function_lines() {
        let source = concat!(
            "from __future__ import strict\n",
            "from typing import final\n",
            "@final\n",
            "# a comment between decorator and actual header\n",
            "class \\\n",
            "    Item:\n",
            "    value: int = 1\n",
            "    @final\n",
            "    def \\\n",
            "        method(self, number: int) -> int:\n",
            "        return number\n",
            "@final\n",
            "async \\\n",
            "def task(number: int) -> int:\n",
            "    return number\n",
            "def factory():\n",
            "    @final\n",
            "    # nested class providers retain the class code's first line\n",
            "    class Local:\n",
            "        value: int = 2\n",
            "        @final\n",
            "        def method(self, number: int) -> int:\n",
            "            return number\n",
            "    return Local\n",
            "@final\n",
            "class Generic[T]:\n",
            "    value: T\n",
            "def generic_factory():\n",
            "    @final\n",
            "    class Local[T]:\n",
            "        value: T\n",
            "    return Local\n",
            "@final\n",
            "def generic_function[T](value: T) -> T:\n",
            "    return value\n",
            "@final\n",
            "async def generic_async[T](value: T) -> T:\n",
            "    return value\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        let providers = module
            .callable_defs
            .iter()
            .filter_map(|function| {
                let origin = function.scope.source_origin.as_ref()?;
                let projection = function.scope.annotation_provider.as_ref()?;
                Some((
                    origin.definition.lexical_qualname.as_str(),
                    projection.native_first_line,
                ))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(providers.get("Item"), Some(&3));
        assert_eq!(providers.get("Item.method"), Some(&9));
        assert_eq!(providers.get("task"), Some(&13));
        assert_eq!(providers.get("factory.<locals>.Local"), Some(&17));
        assert_eq!(providers.get("factory.<locals>.Local.method"), Some(&22));
        assert_eq!(providers.get("Generic"), Some(&25));
        assert_eq!(providers.get("generic_factory.<locals>.Local"), Some(&29));
        assert_eq!(providers.get("generic_function"), Some(&34));
        assert_eq!(providers.get("generic_async"), Some(&37));
        for function in &module.callable_defs {
            let Some(origin) = &function.scope.source_origin else {
                continue;
            };
            if origin.role == CallableSourceRole::ClassNamespace {
                let class = function.scope.class_bindings.as_ref().unwrap();
                assert_eq!(class.source, origin.definition);
            }
            if origin.role == CallableSourceRole::TypeParameterScope {
                let (expected, header_line) = match origin.definition.lexical_qualname.as_str() {
                    "Generic" => (25, 26),
                    "generic_factory.<locals>.Local" => (29, 30),
                    "generic_function" => (33, 34),
                    "generic_async" => (36, 37),
                    _ => continue,
                };
                let projection = function.scope.type_parameter_scope.as_ref().unwrap();
                assert_eq!(projection.native_first_line, expected);
                assert_eq!(projection.native_range, origin.definition.source_range);
                assert_eq!(
                    projection.native_header_range.end,
                    projection.native_range.end
                );
                assert!(projection.native_header_range.start > projection.native_range.start);
                assert_eq!(
                    source.as_bytes()[..projection.native_header_range.start as usize]
                        .iter()
                        .filter(|byte| **byte == b'\n')
                        .count()
                        + 1,
                    header_line
                );
            }
        }
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&module).unwrap();
        let restored: BlockPyModule<BlockPyModuleShape> =
            rkyv::from_bytes::<_, rkyv::rancor::Error>(&bytes).unwrap();
        for (before, after) in module
            .callable_defs
            .iter()
            .zip(restored.callable_defs.iter())
        {
            assert_eq!(
                before.scope.annotation_provider,
                after.scope.annotation_provider
            );
            assert_eq!(
                before.scope.type_parameter_scope,
                after.scope.type_parameter_scope
            );
        }
    }

    #[test]
    fn strict_lazy_aliases_select_source_bound_factories_and_native_capture_projections() {
        use soac_core::block_py::{AnnotationProviderKind, ChildVisitable, ParamKind, Visit};
        use soac_ir_blockpy::InstrBlockPy;

        let source = concat!(
            "from __future__ import strict\n",
            "type Lazy = list[Later]\n",
            "def factory(format):\n",
            "    type Captured = format\n",
            "    return Captured\n",
            "class Holder:\n",
            "    type Member = int\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        #[derive(Default)]
        struct Factories(Vec<(SourceIdentity, soac_core::block_py::RuntimeFunctionId)>);
        impl Visit<InstrBlockPy> for Factories {
            fn visit_instr(&mut self, expression: &InstrBlockPy) {
                if let InstrBlockPy::CreateTypeAlias(operation) = expression {
                    self.0
                        .push((operation.definition.clone(), operation.evaluator_function));
                }
                expression.visit_children(self);
            }
        }
        let mut factories = Factories::default();
        for function in &module.callable_defs {
            factories.visit_fn(function);
        }
        assert_eq!(factories.0.len(), 3);
        for (definition, id) in factories.0 {
            let provider = module
                .callable_defs
                .iter()
                .find(|function| function.function_id == id)
                .unwrap();
            let origin = provider.scope.source_origin.as_ref().unwrap();
            let projection = provider.scope.annotation_provider.as_ref().unwrap();
            assert_eq!(origin.role, CallableSourceRole::AnnotationProvider);
            assert_eq!(origin.definition, definition);
            assert_eq!(definition.definition_kind, DefinitionKind::TypeAlias);
            assert_eq!(projection.kind, AnnotationProviderKind::TypeAliasValue);
            assert_eq!(projection.native_range, Some(definition.source_range));
            assert_eq!(provider.params.params.len(), 1);
            assert_eq!(provider.params.params[0].name, ".format");
            assert_eq!(provider.params.params[0].kind, ParamKind::PosOnly);
            assert!(provider.params.params[0].has_default);
            assert_ne!(provider.body_params().params[0].name, "format");
            let captures = provider
                .public_storage_layout()
                .unwrap()
                .freevars
                .iter()
                .map(|slot| slot.logical_name.as_str())
                .collect::<Vec<_>>();
            match definition.lexical_qualname.as_str() {
                "Lazy" => assert!(captures.is_empty()),
                "factory.<locals>.Captured" => assert_eq!(captures, ["format"]),
                "Holder.Member" => assert_eq!(captures, ["__classdict__"]),
                other => panic!("unexpected alias source: {other}"),
            }
        }
    }

    #[test]
    fn strict_generic_scopes_preserve_native_defaults_and_lazy_evaluator_roles() {
        use soac_core::block_py::{
            AnnotationProviderKind, ChildVisitable, FunctionDefaultsProjection, ParamKind,
            RuntimeFunctionId, TypeParameterScopeInputKind, Visit,
        };
        use soac_ir_blockpy::InstrBlockPy;
        let source = concat!(
            "from __future__ import strict\n",
            "def identity[T: Later = Later](value: T = positional(), *, option = keyword()) -> T:\n",
            "    return value\n",
            "class Generic[T]:\n",
            "    item: T\n",
            "type Packed[*Ts = *tuple[int, str]] = tuple[*Ts]\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        #[derive(Default)]
        struct Operations {
            scopes: Vec<(SourceIdentity, RuntimeFunctionId, bool, bool)>,
            metadata: Vec<RuntimeFunctionId>,
            generic_bases: usize,
        }
        impl Visit<InstrBlockPy> for Operations {
            fn visit_instr(&mut self, instruction: &InstrBlockPy) {
                match instruction {
                    InstrBlockPy::ConstructTypeParameterScope(op) => self.scopes.push((
                        op.definition.clone(),
                        op.scope_function_id,
                        op.positional_defaults.is_some(),
                        op.keyword_defaults.is_some(),
                    )),
                    InstrBlockPy::SetFunctionTypeParameters(op) => {
                        self.metadata.push(op.function_id)
                    }
                    InstrBlockPy::SubscriptGeneric(_) => self.generic_bases += 1,
                    _ => {}
                }
                instruction.visit_children(self);
            }
        }
        let mut operations = Operations::default();
        operations.visit_module(&module);
        assert_eq!(operations.scopes.len(), 3);
        assert_eq!(operations.generic_bases, 1);
        assert_eq!(operations.metadata.len(), 1);
        for (definition, id, positional, keyword) in operations.scopes {
            let function = module
                .callable_defs
                .iter()
                .find(|f| f.function_id == id)
                .unwrap();
            let origin = function.scope.source_origin.as_ref().unwrap();
            assert_eq!(origin.role, CallableSourceRole::TypeParameterScope);
            assert_eq!(origin.definition, definition);
            let projection = function.scope.type_parameter_scope.as_ref().unwrap();
            assert_eq!(projection.native_range, definition.source_range);
            let layout = function.public_storage_layout().unwrap();
            // These declarations are module-level. In particular a generic
            // class owns its native parameter-tuple cell; its body captures
            // that cell without making the scope capture it from the module.
            assert!(
                layout.freevars.is_empty(),
                "{} unexpectedly captures {:?}",
                definition.lexical_qualname,
                layout.freevars
            );
            if definition.definition_kind == DefinitionKind::Class {
                assert!(layout.cellvars.iter().any(|cell| {
                    cell.logical_name == ".type_params" && cell.storage_name == ".type_params"
                }));
            }
            if definition.definition_kind == DefinitionKind::Function {
                assert!(positional && keyword);
                assert_eq!(
                    projection.inputs.iter().map(|p| p.kind).collect::<Vec<_>>(),
                    [
                        TypeParameterScopeInputKind::PositionalDefaults,
                        TypeParameterScopeInputKind::KeywordDefaults
                    ]
                );
                assert_eq!(
                    function
                        .params
                        .params
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>(),
                    [".defaults", ".kwdefaults"]
                );
                assert!(function
                    .params
                    .params
                    .iter()
                    .all(|p| p.kind == ParamKind::Any && !p.has_default));
                for (public, body) in function
                    .params
                    .params
                    .iter()
                    .zip(&function.body_params().params)
                {
                    assert_ne!(public.name, body.name);
                }
            } else {
                assert!(!positional && !keyword);
                assert!(projection.inputs.is_empty());
            }
        }
        let actual = module
            .callable_defs
            .iter()
            .find(|f| f.function_id == operations.metadata[0])
            .unwrap();
        assert_eq!(
            actual.scope.creation_defaults,
            FunctionDefaultsProjection::NativeContainers
        );
        assert_eq!(actual.names.qualname, "identity");
        let evaluators = module
            .callable_defs
            .iter()
            .filter_map(|f| {
                f.scope
                    .annotation_provider
                    .as_ref()
                    .map(|projection| (f, projection))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            evaluators
                .iter()
                .filter(|(_, p)| p.kind == AnnotationProviderKind::TypeParameterDefault)
                .count(),
            2
        );
        assert!(evaluators.iter().any(|(f, p)| p.kind
            == AnnotationProviderKind::TypeParameterBound
            && f.names.qualname == "T"
            && p.native_range.is_some()));
        let class_namespace = module
            .callable_defs
            .iter()
            .find(|f| {
                f.scope.source_origin.as_ref().is_some_and(|origin| {
                    origin.role == CallableSourceRole::ClassNamespace
                        && origin.definition.lexical_qualname == "Generic"
                })
            })
            .unwrap();
        assert_eq!(
            class_namespace
                .public_storage_layout()
                .unwrap()
                .freevars
                .iter()
                .map(|slot| slot.logical_name.as_str())
                .collect::<Vec<_>>(),
            [".type_params", "T"]
        );
    }

    #[test]
    fn strict_generic_class_context_uses_explicit_cell_object_then_cell_reference() {
        use soac_core::block_py::{AnnotationProviderKind, CellCaptureProjection};
        let source = concat!(
            "from __future__ import strict\n",
            "class Holder:\n",
            "    type Member[T: int = str] = tuple[T]\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        let scope = module
            .callable_defs
            .iter()
            .find(|function| {
                function
                    .scope
                    .source_origin
                    .as_ref()
                    .is_some_and(|origin| origin.role == CallableSourceRole::TypeParameterScope)
            })
            .unwrap();
        assert_eq!(
            scope
                .public_storage_layout()
                .unwrap()
                .freevars
                .iter()
                .map(|slot| slot.logical_name.as_str())
                .collect::<Vec<_>>(),
            ["__classdict__"]
        );
        assert_eq!(
            scope.scope.cell_capture_projection("__classdict__"),
            CellCaptureProjection::CellObject
        );
        let provider = module
            .callable_defs
            .iter()
            .find(|function| {
                function
                    .scope
                    .annotation_provider
                    .as_ref()
                    .is_some_and(|projection| {
                        projection.kind == AnnotationProviderKind::TypeAliasValue
                    })
            })
            .unwrap();
        assert_eq!(
            provider
                .public_storage_layout()
                .unwrap()
                .freevars
                .iter()
                .map(|slot| slot.logical_name.as_str())
                .collect::<Vec<_>>(),
            ["T", "__classdict__"]
        );
        assert_eq!(
            provider.scope.cell_capture_projection("__classdict__"),
            CellCaptureProjection::CellReference
        );
        assert_eq!(
            provider.scope.cell_capture_source_name("__classdict__"),
            "__classdict__"
        );
    }

    #[test]
    fn strict_future_annotations_select_eager_scope_setup_and_capture_free_function_providers() {
        use soac_core::block_py::{walk_fn, ChildVisitable, Visit};
        use soac_ir_blockpy::InstrBlockPy;

        let source = concat!(
            "from __future__ import strict, annotations\n",
            "module_value: int = 1\n",
            "try:\n",
            "    pass\n",
            "finally:\n",
            "    reached: str\n",
            "class Item:\n",
            "    value: int\n",
            "    if condition:\n",
            "        optional: str\n",
            "    def method(self, first: int, /, second: str) -> int:\n",
            "        return first\n",
        );
        // Fixed annotation input for this lowering fixture. Its class metadata
        // comes from actual native compilation; the loader separately tests
        // canonical annotation-string projection.
        let entries = [(": int", "int"), (": str", "str"), ("-> int", "int")]
            .into_iter()
            .flat_map(|(marker, text)| {
                source.match_indices(marker).map(move |(offset, _)| {
                    let start = (offset + marker.len() - text.len()) as u32;
                    (
                        soac_contracts::SourceRange::new(start, start + text.len() as u32),
                        text.to_owned(),
                    )
                })
            });
        let canonical =
            crate::CanonicalAnnotationStrings::from_native_entries(source, entries).unwrap();
        let module = crate::lower_source_to_blockpy_module_with_tracker(
            source,
            ModuleNameGen::new(1),
            &mut RecordingPassTracker::new(),
            crate::LoweringOptions {
                strict_facts: Some(verified_source(source)),
                canonical_annotations: Some(Arc::new(canonical)),
                canonical_class_bindings: Some(
                    super::native_class_bindings::for_source(source).unwrap(),
                ),
                ..Default::default()
            },
        )
        .unwrap();
        #[derive(Default)]
        struct Probe {
            namespaces: Vec<bool>,
            records: usize,
        }
        impl Visit<InstrBlockPy> for Probe {
            fn visit_instr(&mut self, instruction: &InstrBlockPy) {
                match instruction {
                    InstrBlockPy::SetupAnnotations(op) => {
                        self.namespaces.push(op.namespace.is_some());
                    }
                    InstrBlockPy::RecordAnnotation(_) => self.records += 1,
                    _ => {}
                }
                instruction.visit_children(self);
            }
        }
        let mut providers = 0;
        let mut setups = 0;
        for function in &module.callable_defs {
            let mut probe = Probe::default();
            walk_fn(&mut probe, function);
            assert_eq!(
                probe.records, 0,
                "future annotations record no lazy indices"
            );
            setups += probe.namespaces.len();
            let origin = function.scope.source_origin.as_ref().unwrap();
            match origin.role {
                CallableSourceRole::ModuleBody => assert_eq!(probe.namespaces, [false]),
                CallableSourceRole::ClassNamespace => assert_eq!(probe.namespaces, [true]),
                CallableSourceRole::AnnotationProvider => {
                    providers += 1;
                    assert_eq!(origin.definition.lexical_qualname, "Item.method");
                    let projection = function.scope.annotation_provider.as_ref().unwrap();
                    assert!(projection.class_dictionary.is_none());
                    assert!(projection.conditional_annotations.is_none());
                    assert!(function
                        .public_storage_layout()
                        .unwrap()
                        .freevars
                        .is_empty());
                    assert_eq!(function.params.params[0].name, "format");
                    assert_eq!(
                        function.params.params[0].kind,
                        soac_core::block_py::ParamKind::PosOnly
                    );
                }
                _ => assert!(probe.namespaces.is_empty()),
            }
        }
        assert_eq!(setups, 2);
        assert_eq!(providers, 1);
    }

    #[test]
    fn strict_conditional_annotations_have_explicit_cells_and_reached_index_operations() {
        use soac_core::block_py::{
            walk_fn, BuildCollectionKind, CellLocation, ChildVisitable, ClassBindingInitialValue,
            ClassBindingPhase, ClassBindingStorage, NameLocation, Visit,
        };
        use soac_ir_blockpy::InstrBlockPy;
        #[derive(Default)]
        struct Probe {
            sets: usize,
            built_sets: Vec<NameLocation>,
            records: Vec<u32>,
            record_cells: Vec<Option<CellLocation>>,
            formats: usize,
        }
        impl Visit<InstrBlockPy> for Probe {
            fn visit_instr(&mut self, instr: &InstrBlockPy) {
                match instr {
                    InstrBlockPy::NewAnnotationSet(_) => self.sets += 1,
                    InstrBlockPy::Store(store)
                        if matches!(store.value.as_ref(),
                        InstrBlockPy::BuildCollection(build)
                            if build.kind == BuildCollectionKind::Set && build.values.is_empty()) =>
                    {
                        self.built_sets.push(store.name.location);
                    }
                    InstrBlockPy::RecordAnnotation(op) => {
                        self.records.push(op.index);
                        self.record_cells.push(match op.indices.as_ref() {
                            InstrBlockPy::Load(load) => load.name.cell_location(),
                            _ => None,
                        });
                    }
                    InstrBlockPy::CheckAnnotationFormat(_) => self.formats += 1,
                    _ => {}
                }
                instr.visit_children(self);
            }
        }
        let source = "from __future__ import strict\nfirst: int\nif flag:\n    second: str\nclass Item:\n    fixed: int\n    if flag:\n        reached: str\n";
        let module = lower(source, Some(verified_source(source))).unwrap();
        for function in &module.callable_defs {
            let mut probe = Probe::default();
            walk_fn(&mut probe, function);
            let origin = function.scope.source_origin.as_ref().unwrap();
            match origin.role {
                CallableSourceRole::ModuleBody => {
                    assert_eq!(probe.sets, 1);
                    probe.records.sort();
                    assert_eq!(probe.records, [0, 1]);
                }
                CallableSourceRole::ClassNamespace => {
                    let class = function.scope.class_bindings.as_ref().unwrap();
                    let layout = function.storage_layout.as_ref().unwrap();
                    let projection = layout.class_bindings.as_ref().unwrap();
                    let initializers = class
                        .recipe
                        .initializers
                        .iter()
                        .filter(|init| init.value == ClassBindingInitialValue::ConditionalSetStore)
                        .collect::<Vec<_>>();
                    let [initializer] = initializers.as_slice() else {
                        panic!("one native conditional-set initializer")
                    };
                    assert_eq!(initializer.phase, ClassBindingPhase::ClassHeaderComplete);
                    let ClassBindingStorage::Cell(cell) =
                        projection.slot(initializer.slot).unwrap().storage;
                    assert!(class.recipe.initializers.iter().any(|init| {
                        init.slot == initializer.slot
                            && init.phase == ClassBindingPhase::ClassEntry
                            && init.value == ClassBindingInitialValue::EmptyCell
                    }));
                    assert_eq!(probe.sets, 0, "no parallel legacy set allocation");
                    assert_eq!(probe.built_sets, [NameLocation::Cell(cell)]);
                    assert_eq!(probe.records, [0]);
                    assert_eq!(probe.record_cells, [Some(cell)]);
                    assert_native_namespace_inputs(function);
                }
                CallableSourceRole::AnnotationProvider => {
                    assert_eq!(probe.formats, 1);
                    assert_eq!(probe.sets, 0);
                    assert!(probe.records.is_empty());
                    if origin.definition.definition_kind == DefinitionKind::Class {
                        let projection = function.scope.annotation_provider.as_ref().unwrap();
                        assert_eq!(
                            projection.conditional_annotations.as_deref(),
                            Some("__conditional_annotations__")
                        );
                        assert_eq!(
                            function
                                .public_storage_layout()
                                .unwrap()
                                .freevars
                                .iter()
                                .map(|slot| slot.logical_name.as_str())
                                .collect::<Vec<_>>(),
                            ["__classdict__", "__conditional_annotations__"]
                        );
                    }
                }
                _ => {}
            }
        }
    }

    #[test]
    fn class_annotation_source_names_keep_native_implicit_cell_fallbacks() {
        use soac_core::block_py::{
            walk_fn, BindingPurpose, CallArgPositional, CellBindingKind, CellCaptureProjection,
            CellLocation, ChildVisitable, ClassBodyFallback, ConstantExpr, EffectiveBinding,
            HasMeta, NameLike, Visit,
        };
        use soac_ir_blockpy::InstrBlockPy;

        struct LookupCells<'a> {
            source: &'a str,
            constants: &'a [ConstantExpr],
            freevars: &'a [soac_core::block_py::ClosureSlot],
            names: Vec<String>,
        }
        impl Visit<InstrBlockPy> for LookupCells<'_> {
            fn visit_instr(&mut self, instr: &InstrBlockPy) {
                if let InstrBlockPy::Call(call) = instr {
                    let selected = match call.func.as_ref() {
                        InstrBlockPy::Load(load) => load.name.is_runtime_symbol("class_lookup_cell")
                            || load.name.location.as_constant().is_some_and(|index| {
                                matches!(self.constants.get(index as usize), Some(ConstantExpr::RuntimeName(name))
                                    if name.name() == "class_lookup_cell")
                            }),
                        _ => false,
                    };
                    if selected {
                        let range = call.meta().range;
                        let source_name =
                            &self.source[range.start().to_usize()..range.end().to_usize()];
                        let ordinal = self
                            .freevars
                            .iter()
                            .position(|slot| slot.logical_name == source_name)
                            .expect("the original annotation Name selects its captured fallback");
                        let dictionary = self
                            .freevars
                            .iter()
                            .position(|slot| slot.logical_name == "__classdict__")
                            .unwrap();
                        let [CallArgPositional::Positional(InstrBlockPy::Load(namespace)), CallArgPositional::Positional(_), CallArgPositional::Positional(InstrBlockPy::CellRef(cell))] =
                            call.args.as_slice()
                        else {
                            panic!("dictionary-first lookup must receive a namespace value and raw fallback cell")
                        };
                        assert_eq!(cell.location, CellLocation::CapturedSource(ordinal as u32));
                        assert!(matches!(
                            namespace.name.cell_location(),
                            Some(CellLocation::Closure(slot) | CellLocation::CapturedSource(slot))
                                if slot == dictionary as u32
                        ), "the dictionary value must come from the exact public native closure cell");
                        assert_eq!(
                            namespace.cell_binding.as_ref().unwrap().kind,
                            CellBindingKind::Capture
                        );
                        self.names.push(source_name.to_owned());
                    }
                }
                instr.visit_children(self);
            }
        }
        let source = "from __future__ import strict\ndef outer():\n    __classdict__ = int\n    __conditional_annotations__ = bytes\n    class Item:\n        dictionary: __classdict__\n        indices: __conditional_annotations__\n        def method(self, value: __conditional_annotations__):\n            pass\n    return Item\n";
        let module = lower(source, Some(verified_source(source))).unwrap();
        let mut providers = 0;
        for function in &module.callable_defs {
            let Some(origin) = function.scope.source_origin.as_ref() else {
                continue;
            };
            if origin.role != CallableSourceRole::AnnotationProvider {
                continue;
            }
            providers += 1;
            assert_eq!(
                function
                    .public_storage_layout()
                    .unwrap()
                    .freevars
                    .iter()
                    .map(|slot| slot.logical_name.as_str())
                    .collect::<Vec<_>>(),
                ["__classdict__", "__conditional_annotations__"]
            );
            for name in ["__classdict__", "__conditional_annotations__"] {
                assert_eq!(
                    function.scope.effective_binding(name, BindingPurpose::Load),
                    Some(EffectiveBinding::ClassBody(ClassBodyFallback::Cell))
                );
                assert_eq!(
                    function.scope.cell_capture_projection(name),
                    CellCaptureProjection::CellObject,
                    "dictionary-first reads still capture the exact native raw cell"
                );
            }
            let mut lookups = LookupCells {
                source,
                constants: &module.module_constants,
                freevars: &function.public_storage_layout().unwrap().freevars,
                names: Vec::new(),
            };
            walk_fn(&mut lookups, function);
            lookups.names.sort();
            let expected = if origin.definition.definition_kind == DefinitionKind::Class {
                vec!["__classdict__", "__conditional_annotations__"]
            } else {
                vec!["__conditional_annotations__"]
            };
            assert_eq!(
                lookups.names, expected,
                "actual source Name operations retain dictionary-first lookup"
            );
            let capture_source = function
                .scope
                .cell_capture_source_name("__conditional_annotations__");
            assert_ne!(capture_source, "_dp_cell___conditional_annotations__");
            assert!(function
                .scope
                .cell_value_aliases
                .values()
                .any(|name| name == "__conditional_annotations__"));
        }
        assert_eq!(providers, 2);
    }

    #[test]
    fn class_type_alias_lookup_uses_native_freevar_inventory() {
        use soac_core::block_py::{BindingPurpose, ClassBodyFallback, EffectiveBinding};

        let source = concat!(
            "from __future__ import strict\n",
            "def build():\n",
            "    class Alias: pass\n",
            "    class Shadow:\n",
            "        Alias = bytes\n",
            "        type Selected = Alias\n",
            "    class Fallback:\n",
            "        type Selected = Alias\n",
            "    class DictionaryShadow:\n",
            "        locals()['Alias'] = bytes\n",
            "        type Selected = Alias\n",
            "    return Shadow, Fallback, DictionaryShadow, Alias\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        let mut providers = 0;
        for function in &module.callable_defs {
            let Some(origin) = function.scope.source_origin.as_ref() else {
                continue;
            };
            if origin.role != CallableSourceRole::AnnotationProvider
                || origin.definition.definition_kind != DefinitionKind::TypeAlias
            {
                continue;
            }
            providers += 1;
            let (captures, fallback) = match origin.definition.lexical_qualname.as_str() {
                "build.<locals>.Shadow.Selected" => {
                    (vec!["__classdict__"], ClassBodyFallback::Global)
                }
                "build.<locals>.Fallback.Selected" | "build.<locals>.DictionaryShadow.Selected" => {
                    (vec!["Alias", "__classdict__"], ClassBodyFallback::Cell)
                }
                other => panic!("unexpected alias source: {other}"),
            };
            assert_eq!(
                function
                    .public_storage_layout()
                    .unwrap()
                    .freevars
                    .iter()
                    .map(|slot| slot.logical_name.as_str())
                    .collect::<Vec<_>>(),
                captures,
                "a class-local definition must not acquire an outer lexical cell"
            );
            assert_eq!(
                function
                    .scope
                    .effective_binding("Alias", BindingPurpose::Load),
                Some(EffectiveBinding::ClassBody(fallback)),
                "source lookup retains the native dictionary-first fallback"
            );
        }
        assert_eq!(providers, 3);
    }

    #[test]
    fn class_method_annotation_captures_preserve_the_outer_lexical_cell() {
        for name in ["Local", "Alias"] {
            let source = format!("from __future__ import strict\ndef factory():\n    class Local:\n        def accept(self, value: {name}) -> {name}:\n            return value\n    Alias = Local\n    return Local\n");
            let module = lower(&source, Some(verified_source(&source))).unwrap();
            for function in &module.callable_defs {
                let Some(origin) = function.scope.source_origin.as_ref() else {
                    continue;
                };
                let expected = match origin.role {
                    CallableSourceRole::ClassNamespace => vec![name],
                    CallableSourceRole::AnnotationProvider => vec![name, "__classdict__"],
                    _ => continue,
                };
                assert_eq!(
                    function
                        .public_storage_layout()
                        .unwrap()
                        .freevars
                        .iter()
                        .map(|slot| slot.logical_name.as_str())
                        .collect::<Vec<_>>(),
                    expected
                );
            }
        }
    }

    #[test]
    fn strict_function_completion_follows_only_recorded_undecorated_definitions() {
        use soac_core::block_py::{
            walk_fn, ChildVisitable, CompleteFunctionDefinition, NameLike, Visit,
        };
        use soac_ir_blockpy::InstrBlockPy;

        #[derive(Default)]
        struct CompletionProbe {
            completions: Vec<CompleteFunctionDefinition<InstrBlockPy>>,
            lookalike_calls: usize,
        }
        impl Visit<InstrBlockPy> for CompletionProbe {
            fn visit_instr(&mut self, instr: &InstrBlockPy) {
                match instr {
                    InstrBlockPy::CompleteFunctionDefinition(op) => {
                        self.completions.push(op.clone());
                    }
                    InstrBlockPy::Call(call)
                        if matches!(call.func.as_ref(), InstrBlockPy::Load(load)
                            if load.name.id_str() == "_dp_complete_function_definition") =>
                    {
                        self.lookalike_calls += 1;
                    }
                    _ => {}
                }
                instr.visit_children(self);
            }
        }

        let source = concat!(
            "from __future__ import strict\n",
            "def factory():\n",
            "    def actual(value: int = 1) -> int:\n",
            "        return value\n",
            "    @decorate\n",
            "    def decorated():\n",
            "        return 2\n",
            "    def generic[T](value: T) -> T:\n",
            "        return value\n",
            "    pretend = _dp_complete_function_definition(actual)\n",
            "    return actual, generic, decorated, pretend\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        let mut probe = CompletionProbe::default();
        for function in &module.callable_defs {
            walk_fn(&mut probe, function);
        }
        assert_eq!(probe.lookalike_calls, 1);
        let mut completed = probe
            .completions
            .iter()
            .map(|op| {
                let target = module
                    .callable_defs
                    .iter()
                    .find(|function| function.function_id == op.function_id)
                    .expect("completion must name an actual lowered callable");
                let origin = target.scope.source_origin.as_ref().unwrap();
                assert_eq!(origin.role, CallableSourceRole::SourceFunction);
                assert_eq!(op.definition, origin.definition);
                if target.names.bind_name == "generic" {
                    assert!(
                        matches!(op.function.as_ref(), InstrBlockPy::ConstructTypeParameterScope(_)),
                        "generic metadata finishes inside the explicit scope before outer completion"
                    );
                } else {
                    assert!(matches!(
                        op.function.as_ref(),
                        InstrBlockPy::MakeFunctionWithClosure(_)
                    ));
                }
                target.names.bind_name.as_str()
            })
            .collect::<Vec<_>>();
        completed.sort_unstable();
        assert_eq!(completed, ["actual", "factory", "generic"]);
    }

    #[test]
    fn strict_class_decorator_boundaries_keep_source_order_and_explicit_cleanup_regions() {
        use ruff_python_ast::Stmt;
        use ruff_text_size::Ranged;
        use soac_core::block_py::{
            instr_any, walk_fn, AbruptKind, ApplyClassDecorator, BlockArg, BlockEdge, BlockParam,
            BlockParamRole, BlockTerm, ChildVisitable, NameLike, PrepareClassDecorator,
            ResolvedName, Visit,
        };
        use soac_ir_blockpy::InstrBlockPy;
        use std::collections::{BTreeMap, HashMap, HashSet};

        #[derive(Default)]
        struct Probe {
            prepare: Vec<PrepareClassDecorator<InstrBlockPy>>,
            apply: Vec<ApplyClassDecorator<InstrBlockPy>>,
            discarded: Vec<ResolvedName>,
            prepared_bindings: Vec<ResolvedName>,
            result_bindings: Vec<ResolvedName>,
            quiet_deletes: Vec<ResolvedName>,
            lookalikes: usize,
        }
        impl Visit<InstrBlockPy> for Probe {
            fn visit_instr(&mut self, instr: &InstrBlockPy) {
                match instr {
                    InstrBlockPy::PrepareClassDecorator(op) => self.prepare.push(op.clone()),
                    InstrBlockPy::ApplyClassDecorator(op) => self.apply.push(op.clone()),
                    InstrBlockPy::DiscardClassDecorator(op) => {
                        let InstrBlockPy::Load(load) = op.preparation.as_ref() else {
                            panic!("discard must load its recorded preparation binding");
                        };
                        self.discarded.push(load.name.clone());
                    }
                    InstrBlockPy::Store(op) => match op.value.as_ref() {
                        InstrBlockPy::PrepareClassDecorator(_) => {
                            self.prepared_bindings.push(op.name.clone())
                        }
                        InstrBlockPy::ApplyClassDecorator(_) => {
                            self.result_bindings.push(op.name.clone())
                        }
                        _ => {}
                    },
                    InstrBlockPy::Del(op) if op.quietly => self.quiet_deletes.push(op.name.clone()),
                    InstrBlockPy::Call(op)
                        if matches!(op.func.as_ref(), InstrBlockPy::Load(load)
                        if load.name.id_str() == "_dp_discard_class_decorator") =>
                    {
                        self.lookalikes += 1
                    }
                    _ => {}
                }
                instr.visit_children(self);
            }
        }
        // Finally dispatch is correlated with the explicit completion value
        // carried on its incoming edge. Exploring every BranchTable arm would
        // invent a normal continuation after Discard itself raised.
        fn after_edge(
            edge: &BlockEdge,
            parameters: &[BlockParam],
            kinds: &BTreeMap<String, usize>,
        ) -> BTreeMap<String, usize> {
            let mut next = kinds.clone();
            for (parameter, argument) in parameters.iter().zip(&edge.args) {
                if parameter.role != BlockParamRole::AbruptKind {
                    continue;
                }
                let kind = match argument {
                    BlockArg::AbruptKind(kind) => match kind {
                        AbruptKind::Fallthrough => 0,
                        AbruptKind::Return => 1,
                        AbruptKind::Exception => 2,
                        AbruptKind::Break => 3,
                        AbruptKind::Continue => 4,
                    },
                    BlockArg::Name(name) => {
                        *kinds.get(name).expect("known forwarded completion kind")
                    }
                    other => panic!("invalid completion-kind edge argument: {other:?}"),
                };
                next.insert(parameter.name.clone(), kind);
            }
            next
        }
        fn successors(
            term: &BlockTerm<InstrBlockPy>,
            kinds: &BTreeMap<String, usize>,
        ) -> Vec<BlockEdge> {
            match term {
                BlockTerm::Jump(edge) => vec![edge.clone()],
                BlockTerm::IfTerm(branch) => vec![
                    BlockEdge::new(branch.then_label),
                    BlockEdge::new(branch.else_label),
                ],
                BlockTerm::BranchTable(branch) => {
                    let InstrBlockPy::Load(index) = &branch.index else {
                        panic!("completion dispatch must load its explicit discriminator");
                    };
                    let kind = *kinds
                        .get(index.name.id_str())
                        .expect("dispatch has a known completion kind");
                    vec![BlockEdge::new(
                        branch
                            .targets
                            .get(kind)
                            .copied()
                            .unwrap_or(branch.default_label),
                    )]
                }
                BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) | BlockTerm::Raise(_) => {
                    Vec::new()
                }
            }
        }

        for asynchronous in [false, true] {
            let header = if asynchronous { "async def" } else { "def" };
            let base = if asynchronous {
                "object if await pause() else object"
            } else {
                "object"
            };
            let source = format!(
                "from __future__ import strict\nfrom dataclasses import dataclass\n{header} build():\n    @dataclass(eq=False)\n    class Item({base}):\n        pass\n    _dp_discard_class_decorator(Item)\n    return Item\n"
            );
            let parsed = ruff_python_parser::parse_module(&source).unwrap();
            let Stmt::FunctionDef(builder) = &parsed.syntax().body[2] else {
                panic!("builder")
            };
            let Stmt::ClassDef(class) = &builder.body[0] else {
                panic!("class")
            };
            let decorator_range = class.decorator_list[0].expression.range();
            let identity = SourceIdentity {
                module: verified_source(&source).facts().module.clone(),
                lexical_qualname: "build.<locals>.Item".into(),
                source_range: SourceRange::new(
                    class.range.start().to_u32(),
                    class.range.end().to_u32(),
                ),
                definition_kind: DefinitionKind::Class,
            };
            // A structured compiler-proposal fixture, not a runtime/checker
            // eligibility claim for this suspended or effectful source.
            let decorator_source = b"def dataclass(cls=None, *, eq=True): ...\n";
            let dependency = DependencyFingerprint {
                module: ModuleContentId::new("dataclasses", legacy_source_hash(decorator_source)),
                source_digest: Fingerprint::digest(decorator_source),
                source_size: decorator_source.len() as u32,
                import_resolution: Fingerprint::digest(b"fixture dataclasses stub path"),
                effective_configuration: Fingerprint::digest(b"fixture dependency configuration"),
                strict_policy: None,
                type_contract: None,
            };
            let decorator_definition = SourceIdentity {
                module: dependency.module.clone(),
                lexical_qualname: "dataclass".into(),
                source_range: SourceRange::new(0, dependency.source_size),
                definition_kind: DefinitionKind::Function,
            };
            let fact = ClassTypeFact {
                identity: identity.clone(),
                bases: Vec::new(),
                metaclass: MetaclassFact::BuiltinType,
                decorators: vec![DecoratorFact {
                    kind: DecoratorKind::StdlibDataclass,
                    expression_range: SourceRange::new(
                        decorator_range.start().to_u32(),
                        decorator_range.end().to_u32(),
                    ),
                    definition: Some(decorator_definition.clone()),
                    source_digest: Some(dependency.source_digest),
                    arguments: [("eq".into(), LiteralValue::Bool(false))].into(),
                    uncertainty: Default::default(),
                }],
                participation: ParticipationProposal::Candidate,
                dictionary: ClassDictionarySemantics::DictionaryBearing,
                instance_fields: Vec::new(),
                methods: Vec::new(),
                class_members: Vec::new(),
                inheritance: InheritanceFact {
                    linearized_bases: Vec::new(),
                    complete: true,
                },
                openness: ClassOpenness::OpenSubclassFamily,
                transform: Some(ClassTransformFact {
                    kind: TransformKind::StdlibDataclass,
                    provenance: Some(decorator_definition),
                    dataclass_options: Some(DataclassOptions {
                        eq: false,
                        ..Default::default()
                    }),
                    generated_methods: Default::default(),
                }),
                uncertainty: [UncertaintyReason::OpenWorld].into(),
            };
            let module = lower(
                &source,
                Some(verified_source_with_classes(
                    &source,
                    vec![fact.clone()],
                    vec![dependency.clone()],
                )),
            )
            .unwrap();
            let mut callers = 0;
            for function in &module.callable_defs {
                let mut probe = Probe::default();
                walk_fn(&mut probe, function);
                if probe.prepare.is_empty() {
                    continue;
                }
                callers += 1;
                let [prepare] = probe.prepare.as_slice() else {
                    panic!("one preparation")
                };
                let [apply] = probe.apply.as_slice() else {
                    panic!("one application")
                };
                assert_eq!(prepare.definition, identity);
                assert_eq!(apply.definition, identity);
                assert_eq!(prepare.construction_function, apply.construction_function);
                assert!(prepare.factory && prepare.args.is_empty());
                assert!(
                    matches!(prepare.keywords.as_slice(), [soac_core::block_py::CallArgKeyword::Named { arg, .. }] if arg.as_str() == "eq")
                );
                assert!(
                    !prepare
                        .operands()
                        .any(|operand| instr_any(operand, |node| matches!(
                            node,
                            InstrBlockPy::MakeFunctionWithClosure(_)
                        ))),
                    "preparation must not create the namespace function early"
                );
                let constructor = module
                    .callable_defs
                    .iter()
                    .find(|candidate| candidate.function_id == prepare.construction_function)
                    .unwrap();
                assert_eq!(
                    constructor.scope.source_origin.as_ref().unwrap().role,
                    CallableSourceRole::ClassConstruction
                );
                assert_eq!(constructor.params.params.len(), 5);
                assert_eq!(
                    probe.lookalikes, 1,
                    "source spelling never introduces a private operation"
                );
                let [prepared] = probe.prepared_bindings.as_slice() else {
                    panic!("preparation binding")
                };
                let [result] = probe.result_bindings.as_slice() else {
                    panic!("application result binding")
                };
                assert!(!probe.discarded.is_empty());
                assert!(probe
                    .discarded
                    .iter()
                    .all(|name| name.location == prepared.location));
                for binding in [prepared, result] {
                    assert!(probe
                        .quiet_deletes
                        .iter()
                        .any(|name| name.location == binding.location));
                }

                let application = function
                    .blocks
                    .iter()
                    .find(|block| {
                        block.body.iter().any(|instr| {
                            instr_any(instr, |node| {
                                matches!(node, InstrBlockPy::ApplyClassDecorator(_))
                            })
                        })
                    })
                    .unwrap();
                assert!(
                    application.exc_edge.is_some(),
                    "application errors enter explicit cleanup"
                );
                let blocks: HashMap<_, _> = function
                    .blocks
                    .iter()
                    .map(|block| (block.label, block))
                    .collect();
                let mut pending = vec![(application.label, false, BTreeMap::new())];
                let mut seen = HashSet::new();
                let mut bound_after_discard = false;
                while let Some((label, mut discarded, kinds)) = pending.pop() {
                    if !seen.insert((label, discarded, kinds.clone())) {
                        continue;
                    }
                    let block = blocks[&label];
                    let before = discarded;
                    for instr in &block.body {
                        discarded |= instr_any(instr, |node| {
                            matches!(node, InstrBlockPy::DiscardClassDecorator(_))
                        });
                        if instr_any(
                            instr,
                            |node| matches!(node, InstrBlockPy::Store(store) if store.name.id_str() == "Item"),
                        ) {
                            assert!(
                                discarded,
                                "source class binding cannot precede operand cleanup"
                            );
                            bound_after_discard = true;
                        }
                    }
                    pending.extend(successors(&block.term, &kinds).into_iter().map(|edge| {
                        let next = after_edge(&edge, &blocks[&edge.target].params, &kinds);
                        (edge.target, discarded, next)
                    }));
                    if let Some(error) = &block.exc_edge {
                        let next = after_edge(error, &blocks[&error.target].params, &kinds);
                        pending.push((error.target, before, next));
                    }
                }
                assert!(bound_after_discard);
            }
            assert_eq!(callers, 1);
            let mut wrong_range = fact;
            wrong_range.decorators[0].expression_range.start += 1;
            let unmatched = lower(
                &source,
                Some(verified_source_with_classes(
                    &source,
                    vec![wrong_range],
                    vec![dependency],
                )),
            )
            .unwrap();
            for function in &unmatched.callable_defs {
                let mut probe = Probe::default();
                walk_fn(&mut probe, function);
                assert!(
                    probe.prepare.is_empty()
                        && probe.apply.is_empty()
                        && probe.discarded.is_empty()
                );
            }
        }
    }

    #[test]
    fn single_builtin_descriptor_sites_keep_original_creation_and_evaluation_order() {
        use ruff_python_ast::Stmt;
        use ruff_text_size::Ranged;
        use soac_core::block_py::{walk_fn, ApplyFunctionDescriptor, ChildVisitable, Visit};
        use soac_ir_blockpy::InstrBlockPy;

        let source = r#"from __future__ import strict
class Owner:
    @staticmethod
    def plain(value=mark()):
        return value
    @classmethod
    def bound(value=mark()):
        return value
    @property
    def getter(value=mark()):
        return value
    @staticmethod
    @classmethod
    def chained(value):
        return value
def lookalike(decorator, function):
    return _dp_apply_function_descriptor(decorator, function)
"#;
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        let Stmt::ClassDef(class) = &parsed.syntax().body[1] else {
            panic!("class fixture");
        };
        let module_identity = verified_source(source).facts().module.clone();
        let functions = class
            .body
            .iter()
            .map(|statement| {
                let Stmt::FunctionDef(function) = statement else {
                    panic!("decorated function fixture");
                };
                FunctionTypeFact {
                    identity: SourceIdentity {
                        module: module_identity.clone(),
                        lexical_qualname: format!("Owner.{}", function.name),
                        source_range: SourceRange::new(
                            function.range.start().to_u32(),
                            function.range.end().to_u32(),
                        ),
                        definition_kind: DefinitionKind::Function,
                    },
                    function_kind: soac_contracts::FunctionKind::Synchronous,
                    signature: CallableSignature {
                        parameters: Vec::new(),
                        return_type: StaticType::Unknown,
                        return_annotation_origin: AnnotationOrigin::Absent,
                        uncertainty: Default::default(),
                    },
                    decorators: function
                        .decorator_list
                        .iter()
                        .map(|decorator| {
                            let range = decorator.expression.range();
                            DecoratorFact {
                                kind: match &source
                                    [range.start().to_usize()..range.end().to_usize()]
                                {
                                    "staticmethod" => DecoratorKind::StaticMethod,
                                    "classmethod" => DecoratorKind::ClassMethod,
                                    "property" => DecoratorKind::Property,
                                    _ => unreachable!(),
                                },
                                expression_range: SourceRange::new(
                                    range.start().to_u32(),
                                    range.end().to_u32(),
                                ),
                                definition: None,
                                source_digest: None,
                                arguments: Default::default(),
                                uncertainty: Default::default(),
                            }
                        })
                        .collect(),
                    uncertainty: Default::default(),
                }
            })
            .collect::<Vec<_>>();
        #[derive(Default)]
        struct Probe(Vec<ApplyFunctionDescriptor<InstrBlockPy>>);
        impl Visit<InstrBlockPy> for Probe {
            fn visit_instr(&mut self, instr: &InstrBlockPy) {
                if let InstrBlockPy::ApplyFunctionDescriptor(operation) = instr {
                    self.0.push(operation.clone());
                }
                instr.visit_children(self);
            }
        }
        for (mut proposals, expected) in [(functions.clone(), 3), (functions, 0)] {
            if expected == 0 {
                for function in &mut proposals {
                    function.decorators[0].expression_range.start += 1;
                }
            }
            let module = lower(
                source,
                Some(verified_source_with_catalog(
                    source,
                    Vec::new(),
                    proposals,
                    Vec::new(),
                )),
            )
            .unwrap();
            let mut probe = Probe::default();
            for function in &module.callable_defs {
                walk_fn(&mut probe, function);
            }
            assert_eq!(probe.0.len(), expected);
            for operation in &probe.0 {
                let InstrBlockPy::MakeFunctionWithClosure(created) = operation.function.as_ref()
                else {
                    panic!("descriptor must receive the original creation, not a decorator result");
                };
                assert_eq!(created.function_id, operation.function_id);
                let function = module
                    .callable_defs
                    .iter()
                    .find(|function| function.function_id == created.function_id)
                    .unwrap();
                assert_eq!(
                    &function.scope.source_origin.as_ref().unwrap().definition,
                    &operation.definition
                );
                let operands = operation.operands().collect::<Vec<_>>();
                assert!(std::ptr::eq(operands[0], operation.decorator.as_ref()));
                assert!(std::ptr::eq(operands[1], operation.function.as_ref()));
                assert!(
                    operation.frame_namespace.is_some(),
                    "retain actual class frame context for generic rebound calls"
                );
            }
            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&module).unwrap();
            let restored: BlockPyModule<BlockPyModuleShape> =
                rkyv::from_bytes::<_, rkyv::rancor::Error>(&bytes).unwrap();
            let mut roundtrip = Probe::default();
            for function in &restored.callable_defs {
                walk_fn(&mut roundtrip, function);
            }
            assert_eq!(roundtrip.0.len(), expected);
        }
    }

    #[test]
    fn strict_class_comprehensions_use_helper_scopes_and_keep_class_cells() {
        use soac_core::block_py::{
            BindingKind, CallableSourceRole, CellBindingKind, ClassBindingExportKind,
        };
        for source in [
            "from __future__ import strict\nclass Box:\n    values = [lambda: item for item in (1, 2)]\n",
            "from __future__ import strict\nclass Box:\n    values = {key: value for key, value in ((1, 2), (3, 4))}\n",
            "from __future__ import strict\nclass Box:\n    values = [[lambda: (outer, inner) for inner in (1, 2)] for outer in (3, 4)]\n",
            "from __future__ import strict\nclass Box:\n    values = [lambda: __class__ for item in (1, 2)]\n    def owner(self):\n        return __class__\n",
        ] {
            let module = lower(source, Some(verified_source(source))).unwrap();
            let namespace = module.callable_defs.iter().find(|function| {
                function.scope.source_origin.as_ref().is_some_and(|origin| origin.role == CallableSourceRole::ClassNamespace)
            }).unwrap();
            let class = namespace.scope.class_bindings.as_ref().unwrap();
            let layout = namespace.storage_layout.as_ref().unwrap();
            layout.class_bindings.as_ref().unwrap().validate(class, layout, &namespace.scope).unwrap();
            let has_classcell = source.contains("lambda: __class__");
            assert_eq!(class.recipe.exports.iter().any(|export| export.kind == ClassBindingExportKind::ClassCell), has_classcell);
            assert_eq!(class.slots.len(), class.recipe.exports.len());
            assert!(class.slots.iter().all(|slot| {
                class.recipe.exports.iter().any(|export| export.source == slot.slot)
            }), "only actual class-cell exports need containing-class storage");
            let helpers = module.callable_defs.iter().filter(|function| {
                function.scope.source_origin.is_none()
                    && function.scope.class_bindings.is_none()
                    && function.scope.annotation_provider.is_none()
            }).collect::<Vec<_>>();
            assert!(!helpers.is_empty(), "eager comprehension has an ordinary helper scope");
            if source.contains("lambda: item") {
                assert!(helpers.iter().any(|function| {
                    function.scope.binding_kind("item") == Some(BindingKind::Cell(CellBindingKind::Owner))
                }));
                let lambda = module.callable_defs.iter().find(|function| {
                    function.scope.source_origin.as_ref().is_some_and(|origin| origin.definition.definition_kind == soac_contracts::DefinitionKind::Lambda)
                }).unwrap();
                assert_eq!(lambda.scope.binding_kind("item"), Some(BindingKind::Cell(CellBindingKind::Capture)));
            }
            if has_classcell {
                let classcell = class.recipe.exports.iter().find(|export| export.kind == ClassBindingExportKind::ClassCell).unwrap();
                let current = class.slot_binding(classcell.source).unwrap();
                assert!(helpers.iter().any(|function| {
                    function.scope.cell_capture_source_names.get("__class__").is_some_and(|name| name == current)
                        && function.scope.cell_capture_projection("__class__") == soac_core::block_py::CellCaptureProjection::CellObject
                }), "delayed __class__ capture shares the actual exported class cell");
            }
        }
    }

    #[test]
    fn strict_eager_class_body_keeps_outer_capture_distinct_from_own_method_cell() {
        use soac_core::block_py::{
            instr_any, CallableSourceRole, CellLocation, ClassBindingExportKind,
            ClassBindingInitialValue, ClassBindingPhase, ClassBindingStorage, NameLocation,
        };
        use soac_ir_blockpy::InstrBlockPy;

        let source = concat!(
            "from __future__ import strict\n",
            "def factory():\n",
            "    class Outer:\n",
            "        class Inner:\n",
            "            nonlocal __class__\n",
            "            __class__ = 'construction'\n",
            "            saved = __class__\n",
            "            def own_class(self):\n",
            "                return __class__\n",
            "        def own_class(self):\n",
            "            return __class__\n",
            "    return Outer\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        let namespace = module
            .callable_defs
            .iter()
            .find(|function| {
                function.scope.source_origin.as_ref().is_some_and(|origin| {
                    origin.role == CallableSourceRole::ClassNamespace
                        && origin.definition.lexical_qualname == "factory.<locals>.Outer.Inner"
                })
            })
            .expect("the nested class has an explicit namespace function");
        let layout = namespace.storage_layout.as_ref().unwrap();
        let (outer_ordinal, outer) = layout
            .freevars
            .iter()
            .enumerate()
            .find(|(_, slot)| slot.logical_name == "__class__")
            .expect("the eager body captures its outer class cell");
        let class = namespace.scope.class_bindings.as_ref().unwrap();
        let projection = layout.class_bindings.as_ref().unwrap();
        projection
            .validate(class, layout, &namespace.scope)
            .unwrap();
        assert_eq!(
            class.node.freevar_slot(outer_ordinal as u32).unwrap().name,
            outer.logical_name
        );
        let incoming = class
            .recipe
            .initializers
            .iter()
            .find(|init| {
                init.value
                    == ClassBindingInitialValue::IncomingFree {
                        ordinal: outer_ordinal as u32,
                    }
            })
            .expect("the native entry copies its actual outer free cell");
        let own = class
            .recipe
            .exports
            .iter()
            .find(|export| export.kind == ClassBindingExportKind::ClassCell)
            .expect("the inner methods require the native class-cell export");
        assert_ne!(incoming.slot, own.source);
        let ClassBindingStorage::Cell(own_cell) = projection.slot(own.source).unwrap().storage;
        assert_ne!(
            projection.slot(incoming.slot).unwrap().storage,
            ClassBindingStorage::Cell(own_cell)
        );
        assert!(class.recipe.initializers.iter().any(|init| {
            init.slot == own.source
                && init.phase == ClassBindingPhase::ClassEntry
                && init.value == ClassBindingInitialValue::EmptyCell
        }));
        let incoming_local = projection
            .slot(incoming.slot)
            .unwrap()
            .storage
            .raw_local(layout)
            .unwrap();
        let own_local = projection
            .slot(own.source)
            .unwrap()
            .storage
            .raw_local(layout)
            .unwrap();
        assert!(namespace
            .blocks
            .iter()
            .any(|block| block.body.iter().any(|instruction| {
                instr_any(instruction, |instr| {
                    matches!(instr, InstrBlockPy::Store(store)
                if store.name.location == NameLocation::Local(incoming_local)
                    && matches!(store.value.as_ref(), InstrBlockPy::CellRef(cell)
                        if cell.location == CellLocation::CapturedSource(outer_ordinal as u32)))
                })
            })));
        assert!(namespace
            .blocks
            .iter()
            .any(|block| block.body.iter().any(|instruction| {
                instr_any(instruction, |instr| {
                    matches!(instr, InstrBlockPy::Store(store)
                if store.name.location == NameLocation::Local(own_local)
                    && matches!(store.value.as_ref(), InstrBlockPy::MakeCell(cell)
                        if cell.initial_value.is_none()))
                })
            })));
    }

    #[test]
    fn strict_class_cell_requirement_comes_from_resolved_namespace_captures() {
        use soac_core::block_py::{ChildVisitable, ConstantExpr, NameLocation, RuntimeName, Visit};
        use soac_ir_blockpy::InstrBlockPy;

        let source = "from __future__ import strict\n\
class Capturing:\n    callback = lambda: __class__\n\
class Shadowed:\n    callback = lambda __class__: __class__\n\
class Empty:\n    pass\n";
        let module = lower(source, Some(verified_source(source))).unwrap();
        for (name, expected) in [("Capturing", true), ("Shadowed", false), ("Empty", false)] {
            if name != "Empty" {
                let callback = module
                    .callable_defs
                    .iter()
                    .find(|function| {
                        function.scope.source_origin.as_ref().is_some_and(|origin| {
                            origin.definition.lexical_qualname == format!("{name}.<lambda>")
                        })
                    })
                    .unwrap();
                let layout = callback.storage_layout.as_ref().unwrap();
                assert_eq!(
                    layout
                        .freevars
                        .iter()
                        .any(|slot| slot.logical_name == "__class__"),
                    expected,
                    "actual callback cell capture for {name}: {layout:?}"
                );
            }
            struct Probe<'a> {
                constants: &'a [ConstantExpr],
                definition: &'a str,
                requires: Vec<bool>,
            }
            impl Visit<InstrBlockPy> for Probe<'_> {
                fn visit_instr(&mut self, instr: &InstrBlockPy) {
                    if let Some(operation) = match instr {
                        InstrBlockPy::ConstructClass(operation)
                            if operation.definition.lexical_qualname == self.definition =>
                        {
                            Some(operation)
                        }
                        _ => None,
                    } {
                        let InstrBlockPy::Load(load) = operation.requires_class_cell.as_ref()
                        else {
                            panic!("resolved construction uses a boolean constant");
                        };
                        let value = match load.name.location {
                            NameLocation::RuntimeName(value) => value,
                            NameLocation::Constant(index) => match self.constants[index as usize] {
                                ConstantExpr::RuntimeName(value) => value,
                                _ => panic!("class cell requirement is not a boolean"),
                            },
                            _ => panic!("class cell requirement is not resolved"),
                        };
                        self.requires.push(match value {
                            RuntimeName::True => true,
                            RuntimeName::False => false,
                            _ => panic!("class cell requirement is not a boolean"),
                        });
                    }
                    instr.visit_children(self);
                }
            }
            let mut probe = Probe {
                constants: &module.module_constants,
                definition: name,
                requires: Vec::new(),
            };
            probe.visit_module(&module);
            assert_eq!(probe.requires, [expected], "construction for {name}");
        }
    }

    #[test]
    fn strict_class_construction_is_an_explicit_source_bound_operation() {
        use soac_core::block_py::{walk_fn, ChildVisitable, ConstructClass, Visit};
        use soac_ir_blockpy::InstrBlockPy;

        #[derive(Default)]
        struct ConstructionProbe(Vec<ConstructClass<InstrBlockPy>>);
        impl Visit<InstrBlockPy> for ConstructionProbe {
            fn visit_instr(&mut self, instr: &InstrBlockPy) {
                if let InstrBlockPy::ConstructClass(op) = instr {
                    self.0.push(op.clone());
                }
                instr.visit_children(self);
            }
        }

        let source = concat!(
            "from __future__ import strict\n",
            "@outer(1)\n",
            "@inner(2)\n",
            "class Item(base()):\n",
            "    observed_line = __firstlineno__\n",
            "def _dp_define_class_Forged():\n",
            "    return __soac__.create_class((), 'Forged', None, (), {}, False, 1)\n",
        );
        let module = lower(source, Some(verified_source(source))).unwrap();
        let mut sites = 0;
        for function in &module.callable_defs {
            let mut probe = ConstructionProbe::default();
            walk_fn(&mut probe, function);
            let origin = function.scope.source_origin.as_ref().unwrap();
            if origin.role == CallableSourceRole::ClassConstruction {
                let [site] = probe.0.as_slice() else {
                    panic!("expected one construction operation");
                };
                assert_eq!(site.definition, origin.definition);
                assert_eq!(site.construction_function, function.function_id);
                assert_eq!(site.operands().count(), 7);
                assert_eq!(function.params.params[0].name, "_dp_class_ns_fn");
                sites += 1;
            } else {
                assert!(
                    probe.0.is_empty(),
                    "a source name or helper call cannot introduce construction authority"
                );
                if origin.role == CallableSourceRole::ClassNamespace {
                    assert_native_namespace_inputs(function);
                }
            }
        }
        assert_eq!(sites, 1);

        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&module).unwrap();
        let restored: BlockPyModule<BlockPyModuleShape> =
            rkyv::from_bytes::<_, rkyv::rancor::Error>(&bytes).unwrap();
        let mut probe = ConstructionProbe::default();
        for function in &restored.callable_defs {
            walk_fn(&mut probe, function);
        }
        assert_eq!(probe.0.len(), 1);
        assert_eq!(probe.0[0].definition.lexical_qualname, "Item");
    }
}

#[test]
fn except_as_cell_and_nonlocal_keep_semantic_delete_and_capture() {
    use soac_core::block_py::{CellLocation, NameLocation};
    let source = "def owner():\n    try:\n        raise ValueError()\n    except ValueError as error:\n        def read():\n            nonlocal error\n            return error\n        saved = read\n    return saved\n";
    let module = crate::lower_python_to_blockpy_for_testing(source)
        .unwrap()
        .blockpy_module;
    let owner = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "owner")
        .unwrap();
    let layout = owner.storage_layout.as_ref().unwrap();
    let index = layout
        .cellvars
        .iter()
        .position(|cell| cell.logical_name == "error")
        .unwrap();
    let error_cell = CellLocation::Owned(index as u32);
    struct CellDeletes(CellLocation, usize);
    impl Visit<InstrBlockPy> for CellDeletes {
        fn visit_instr(&mut self, instr: &InstrBlockPy) {
            if let InstrBlockPy::Del(delete) = instr {
                if delete.name.id.as_str() == "error"
                    && delete.name.location == NameLocation::Cell(self.0)
                {
                    self.1 += 1;
                }
            }
            instr.visit_children(self);
        }
    }
    let mut deletes = CellDeletes(error_cell, 0);
    deletes.visit_fn(owner);
    assert!(
        deletes.1 > 0,
        "except-as clearing is a semantic source delete, not frame-retirement metadata"
    );
    let read = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "read")
        .unwrap();
    let layout = read.storage_layout.as_ref().unwrap();
    assert!(
        layout.cellvars.is_empty(),
        "nonlocal does not create another owner cell"
    );
    assert_eq!(layout.freevars.len(), 1);
    assert_eq!(layout.freevars[0].logical_name, "error");
}

#[test]
fn lambda_defaults_keep_containing_scope_bindings() {
    use soac_core::block_py::{BindingKind, CellBindingKind, CellLocation, NameLocation};

    #[derive(Default)]
    struct Stores(Vec<(String, NameLocation)>);
    impl Visit<InstrBlockPy> for Stores {
        fn visit_instr(&mut self, instruction: &InstrBlockPy) {
            if let InstrBlockPy::Store(store) = instruction {
                self.0
                    .push((store.name.id.to_string(), store.name.location));
            }
            instruction.visit_children(self);
        }
    }

    for (source, comprehension_default) in [
        ("def owner(value):\n    callback = lambda argument=(saved := value), /, *, keyword=(keyword_saved := value): (body_only := argument)\n    return callback\n", false),
        ("def owner(values):\n    callbacks = [lambda argument=(saved := item): (body_only := argument) for item in values]\n    return callbacks\n", true),
    ] {
        let module = crate::lower_python_to_blockpy_for_testing(source)
            .unwrap()
            .blockpy_module;
        let owner = module
            .callable_defs
            .iter()
            .find(|function| function.names.display_name == "owner")
            .unwrap();
        assert!(owner.scope.has_local_def("saved"), "{source}");
        assert!(!owner.scope.has_local_def("body_only"), "{source}");
        let lambda = module
            .callable_defs
            .iter()
            .find(|function| function.names.display_name == "<lambda>")
            .unwrap();
        assert!(lambda.scope.has_local_def("body_only"), "{source}");
        assert!(!lambda.scope.has_local_def("saved"), "{source}");
        assert!(!lambda.scope.has_local_def("keyword_saved"), "{source}");
        assert!(lambda.scope.has_local_def("argument"), "{source}");
        let mut lambda_stores = Stores::default();
        lambda_stores.visit_fn(lambda);
        assert!(lambda_stores.0.iter().any(|(name, location)| {
            name == "body_only" && matches!(location, NameLocation::Local(_))
        }));
        assert!(!lambda_stores
            .0
            .iter()
            .any(|(name, _)| matches!(name.as_str(), "saved" | "keyword_saved")));

        if comprehension_default {
            assert_eq!(
                owner.scope.binding_kind("saved"),
                Some(BindingKind::Cell(CellBindingKind::Owner))
            );
            let helper = module
                .callable_defs
                .iter()
                .find(|function| function.names.display_name == "<listcomp>")
                .unwrap();
            assert_eq!(
                helper.scope.binding_kind("saved"),
                Some(BindingKind::Cell(CellBindingKind::Capture))
            );
            let capture = helper
                .storage_layout
                .as_ref()
                .unwrap()
                .freevars
                .iter()
                .position(|slot| slot.logical_name == "saved")
                .expect("outlined comprehension captures the containing source binding")
                as u32;
            let mut stores = Stores::default();
            stores.visit_fn(helper);
            let locations = stores
                .0
                .iter()
                .filter(|(name, _)| name == "saved")
                .map(|(_, location)| *location)
                .collect::<Vec<_>>();
            assert!(!locations.is_empty(), "default evaluation must store saved");
            assert!(locations.into_iter().all(|location| {
                matches!(
                    location,
                    NameLocation::Cell(
                        CellLocation::Closure(index) | CellLocation::CapturedSource(index)
                    ) if index == capture
                )
            }), "default assignment must write the actual outer capture");
        } else {
            let mut stores = Stores::default();
            stores.visit_fn(owner);
            for name in ["saved", "keyword_saved"] {
                assert!(owner.scope.has_local_def(name), "{name}: {source}");
                assert_eq!(owner.scope.binding_kind(name), Some(BindingKind::Local));
                let locations = stores
                    .0
                    .iter()
                    .filter(|(stored, _)| stored == name)
                    .map(|(_, location)| *location)
                    .collect::<Vec<_>>();
                assert!(!locations.is_empty(), "{name}: {source}");
                assert!(locations
                    .into_iter()
                    .all(|location| matches!(location, NameLocation::Local(_))));
            }
        }
    }
}

fn assert_source_prefixed_cells(
    layout: &soac_core::block_py::StorageLayout,
    owned: &[&str],
    captured: &[&str],
) {
    let mut actual_owned = layout
        .cellvars
        .iter()
        .map(|cell| cell.logical_name.as_str())
        .collect::<Vec<_>>();
    actual_owned.sort_unstable();
    assert_eq!(actual_owned, owned);
    assert_eq!(
        layout
            .freevars
            .iter()
            .map(|cell| cell.logical_name.as_str())
            .collect::<Vec<_>>(),
        captured
    );
}

#[test]
fn source_prefixed_bindings_function_capture_uses_source_cells() {
    use soac_core::block_py::CellLocation;

    let module = crate::lower_python_to_blockpy_for_testing(
        "def owner(_dp_parameter):\n    _dp_local = _dp_parameter\n    def read():\n        return _dp_parameter, _dp_local\n    def replace(value):\n        nonlocal _dp_local\n        _dp_local = value\n    def clear():\n        nonlocal _dp_local\n        del _dp_local\n    return read, replace, clear\n",
    )
    .unwrap()
    .blockpy_module;
    let owner = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "owner")
        .unwrap();
    assert_source_prefixed_cells(
        owner.storage_layout.as_ref().unwrap(),
        &["_dp_local", "_dp_parameter"],
        &[],
    );
    let read = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "read")
        .unwrap();
    assert_source_prefixed_cells(
        read.storage_layout.as_ref().unwrap(),
        &[],
        &["_dp_local", "_dp_parameter"],
    );
    #[derive(Default)]
    struct CellWrites {
        store: bool,
        delete: bool,
    }
    impl Visit<InstrBlockPy> for CellWrites {
        fn visit_instr(&mut self, instr: &InstrBlockPy) {
            match instr {
                InstrBlockPy::Store(store)
                    if matches!(
                        store.name.cell_location(),
                        Some(CellLocation::Closure(_) | CellLocation::CapturedSource(_))
                    ) =>
                {
                    self.store = true
                }
                InstrBlockPy::Del(delete)
                    if !delete.quietly
                        && matches!(
                            delete.name.cell_location(),
                            Some(CellLocation::Closure(_) | CellLocation::CapturedSource(_))
                        ) =>
                {
                    self.delete = true
                }
                _ => {}
            }
            instr.visit_children(self);
        }
    }
    let mut writes = CellWrites::default();
    writes.visit_module(&module);
    assert!(
        writes.store && writes.delete,
        "nonlocal writes and semantic deletes must target the same captured source cell"
    );
}

#[test]
fn source_prefixed_bindings_class_body_and_method_preserve_lexical_owner() {
    use soac_core::block_py::{BindingPurpose, BindingTarget, CallableScopeKind};

    let module = crate::lower_python_to_blockpy_for_testing(
        "def owner(_dp_parameter):\n    _dp_local = _dp_parameter\n    class Box:\n        _dp_field = _dp_parameter\n        def read(self):\n            return _dp_parameter, _dp_local\n    return Box\n",
    )
    .unwrap()
    .blockpy_module;
    let owner = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "owner")
        .unwrap();
    assert_source_prefixed_cells(
        owner.storage_layout.as_ref().unwrap(),
        &["_dp_local", "_dp_parameter"],
        &[],
    );
    let method = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "read")
        .unwrap();
    assert_source_prefixed_cells(
        method.storage_layout.as_ref().unwrap(),
        &[],
        &["_dp_local", "_dp_parameter"],
    );
    let namespace = module
        .callable_defs
        .iter()
        .find(|function| function.scope.scope_kind == CallableScopeKind::Class)
        .unwrap();
    assert_eq!(
        namespace
            .scope
            .binding_target_for_name("_dp_field", BindingPurpose::Store),
        BindingTarget::ClassNamespace,
        "a source class member does not become a private helper local"
    );
}

#[test]
fn source_prefixed_bindings_genexpr_keeps_user_captures_separate_from_resume_state() {
    let module = crate::lower_python_to_blockpy_for_testing(
        "def owner(_dp_parameter):\n    _dp_local = _dp_parameter\n    return ((_dp_parameter, _dp_local, item) for item in (1, 2))\n",
    )
    .unwrap()
    .blockpy_module;
    let owner = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "owner")
        .unwrap();
    assert_source_prefixed_cells(
        owner.storage_layout.as_ref().unwrap(),
        &["_dp_local", "_dp_parameter"],
        &[],
    );
    let generator = module
        .callable_defs
        .iter()
        .find(|function| {
            function.names.display_name == "<genexpr>"
                && matches!(
                    function.lowered_kind(),
                    soac_core::block_py::FunctionKind::Generator
                )
        })
        .unwrap();
    assert_source_prefixed_cells(
        generator.storage_layout.as_ref().unwrap(),
        &[],
        &["_dp_local", "_dp_parameter"],
    );
    assert!(
        !generator.scope.scope_internal_names.contains("_dp_pc"),
        "compiler-created resume state does not gain source binding provenance"
    );
}

#[test]
fn source_prefixed_bindings_module_names_keep_the_source_namespace() {
    use soac_core::block_py::{BindingPurpose, BindingTarget, CallableScopeKind};

    let module = crate::lower_python_to_blockpy_for_testing(
        "_dp_module_value = 4\ndef read():\n    return _dp_module_value\ndef clear():\n    global _dp_module_value\n    del _dp_module_value\n",
    )
    .unwrap()
    .blockpy_module;
    let module_body = module
        .callable_defs
        .iter()
        .find(|function| function.scope.scope_kind == CallableScopeKind::Module)
        .unwrap();
    assert_eq!(
        module_body
            .scope
            .binding_target_for_name("_dp_module_value", BindingPurpose::Store),
        BindingTarget::ModuleGlobal
    );
    let read = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "read")
        .unwrap();
    assert_eq!(
        read.scope
            .binding_target_for_name("_dp_module_value", BindingPurpose::Load),
        BindingTarget::ModuleGlobal
    );
    assert_source_prefixed_cells(read.storage_layout.as_ref().unwrap(), &[], &[]);
}

#[test]
fn generator_control_spellings_remain_source_parameters() {
    use soac_core::block_py::{ClosureInit, PreservedSlotStorage};

    let names = [
        "_dp_pc",
        "_dp_is_closed",
        "_dp_yieldfrom",
        "_dp_self",
        "_dp_state",
        "_dp_send_value",
        "_dp_resume_exc",
        "_dp_transport_sent",
    ];
    for (prefix, suspension) in [
        ("def", "yield values"),
        ("async def", "await pause(); return values"),
        ("async def", "yield values"),
    ] {
        let source = format!(
            "{prefix} suspended({}, pause):\n    values = ({})\n    {suspension}\n",
            names.join(", "),
            names.join(", "),
        );
        let module = crate::lower_python_to_blockpy_for_testing(&source)
            .expect("source control spellings must not collide with private resume bindings")
            .blockpy_module;
        let function = module
            .callable_defs
            .iter()
            .find(|function| function.names.display_name == "suspended")
            .unwrap();
        let public = function.public_storage_layout().unwrap();
        for name in names {
            assert!(function.params.iter().any(|param| param.name == name));
            let slot = public
                .preserved_slots
                .iter()
                .find(|slot| slot.logical_name == name)
                .expect("public source parameter retains its own preserved binding");
            assert_eq!(slot.init, ClosureInit::Parameter);
            assert_eq!(slot.storage, PreservedSlotStorage::PyObjectOrNull);
            assert!(
                !function
                    .body_params()
                    .iter()
                    .any(|param| param.name == name),
                "private ABI parameter must not reuse a source binding"
            );
        }
    }
}

#[test]
fn generator_control_spellings_remain_source_locals() {
    use soac_core::block_py::{ClosureInit, PreservedSlotStorage};

    let module = crate::lower_python_to_blockpy_for_testing(
        "def suspended(value):\n    _dp_pc = value\n    _dp_is_closed = value\n    _dp_yieldfrom = value\n    yield (_dp_pc, _dp_is_closed, _dp_yieldfrom)\n    yield from ()\n    return _dp_pc, _dp_is_closed, _dp_yieldfrom\n",
    ).expect("source locals must not become private generator controls").blockpy_module;
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "suspended")
        .unwrap();
    let layout = function.public_storage_layout().unwrap();
    for name in ["_dp_pc", "_dp_is_closed", "_dp_yieldfrom"] {
        let slot = layout
            .preserved_slots
            .iter()
            .find(|slot| slot.logical_name == name)
            .unwrap();
        assert_eq!(slot.init, ClosureInit::Deferred);
        assert_eq!(slot.storage, PreservedSlotStorage::PyObjectOrNull);
    }
}

#[test]
fn generator_control_spellings_keep_source_cells() {
    let module = crate::lower_python_to_blockpy_for_testing(
        "def owner(_dp_pc, _dp_is_closed, _dp_yieldfrom):\n    def read():\n        return _dp_pc, _dp_is_closed, _dp_yieldfrom\n    def suspended():\n        yield _dp_pc, _dp_is_closed, _dp_yieldfrom\n    expression = ((_dp_pc, _dp_is_closed, _dp_yieldfrom) for item in ())\n    return read, suspended, expression\n",
    ).expect("source cell spellings must not be filtered as generator internals").blockpy_module;
    let names = ["_dp_is_closed", "_dp_pc", "_dp_yieldfrom"];
    let owner = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "owner")
        .unwrap();
    assert_source_prefixed_cells(owner.storage_layout.as_ref().unwrap(), &names, &[]);
    for name in ["read", "suspended"] {
        let function = module
            .callable_defs
            .iter()
            .find(|function| function.names.display_name == name)
            .unwrap();
        assert_source_prefixed_cells(function.storage_layout.as_ref().unwrap(), &[], &names);
    }
    let expression = module
        .callable_defs
        .iter()
        .find(|function| {
            function.names.display_name == "<genexpr>"
                && matches!(
                    function.lowered_kind(),
                    soac_core::block_py::FunctionKind::Generator
                )
        })
        .unwrap();
    assert_source_prefixed_cells(expression.storage_layout.as_ref().unwrap(), &[], &names);
}

#[test]
fn generator_role_metadata_rejects_duplicate_missing_and_representational_aliases() {
    use soac_core::block_py::{
        GeneratorControlRole, GeneratorResumeParamRole, PreservedSlotStorage,
    };
    let module = crate::lower_python_to_blockpy_for_testing(
        "def suspended(_dp_pc, _dp_state):\n    yield (_dp_pc, _dp_state)\n",
    )
    .expect("control collision fixture")
    .blockpy_module;
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "suspended")
        .unwrap();
    let layout = function.storage_layout.as_ref().unwrap();
    layout.validate_generator_roles().unwrap();

    let pc = layout
        .generator_control_slot(GeneratorControlRole::ProgramCounter)
        .unwrap();
    let state_name = layout
        .generator_resume_parameter(GeneratorResumeParamRole::StateValue)
        .unwrap();
    assert_ne!(
        layout.preserved_slot(pc.slot()).unwrap().logical_name,
        "_dp_pc"
    );
    assert_ne!(state_name, "_dp_state");

    let mut duplicate = layout.clone();
    duplicate
        .preserved_slots
        .push(layout.preserved_slot(pc.slot()).unwrap().clone());
    assert!(duplicate.validate_generator_roles().is_err());
    assert_eq!(
        duplicate.generator_control_slot(GeneratorControlRole::ProgramCounter),
        None
    );

    let mut missing = layout.clone();
    missing.preserved_slots[pc.slot() as usize].generator_control = None;
    assert!(missing.validate_generator_roles().is_err());
    assert_eq!(
        missing.generator_control_slot(GeneratorControlRole::ProgramCounter),
        None
    );

    let mut wrong_rep = layout.clone();
    wrong_rep.preserved_slots[pc.slot() as usize].storage = PreservedSlotStorage::PyObjectOrNull;
    assert!(wrong_rep.validate_generator_roles().is_err());

    let mut alias = layout.clone();
    alias.generator_resume_abi.as_mut().unwrap().params[1].name = "_dp_state".to_owned();
    assert!(alias.validate_generator_roles().is_err());
}

#[test]
fn generator_resume_abi_rejects_redirected_reordered_and_duplicate_parameters() {
    use soac_core::block_py::GeneratorResumeParamRole;
    let module = crate::lower_python_to_blockpy_for_testing(
        "async def suspended(_dp_state):\n    yield _dp_state\n",
    )
    .expect("five-argument resume collision fixture")
    .blockpy_module;
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "suspended")
        .unwrap();
    let abi = function
        .storage_layout
        .as_ref()
        .unwrap()
        .generator_resume_abi
        .as_ref()
        .unwrap();
    abi.validate(function.kind, function.body_params()).unwrap();

    let mut redirected = abi.clone();
    redirected.params[1].name = "_dp_state".to_owned();
    assert!(redirected
        .validate(function.kind, function.body_params())
        .is_err());

    let mut reordered = abi.clone();
    reordered.params.swap(0, 1);
    assert!(reordered
        .validate(function.kind, function.body_params())
        .is_err());

    let mut duplicate = abi.clone();
    duplicate.params[0].role = GeneratorResumeParamRole::StateValue;
    assert!(duplicate
        .validate(function.kind, function.body_params())
        .is_err());
    assert_eq!(
        duplicate.parameter(GeneratorResumeParamRole::StateValue),
        None
    );

    let mut missing = module.clone();
    let missing_function = missing
        .callable_defs
        .iter_mut()
        .find(|candidate| candidate.function_id == function.function_id)
        .unwrap();
    missing_function
        .storage_layout
        .as_mut()
        .unwrap()
        .generator_resume_abi = None;
    assert!(crate::block_py::validate::validate_blockpy_module(&missing).is_err());
}

#[test]
fn resolved_block_parameter_roles_keep_unbound_source_spellings_ordinary() {
    use soac_core::block_py::NameLocation;
    for name in [
        "_dp_try_exc_source",
        "_dp_try_abrupt_kind_source",
        "_dp_try_abrupt_payload_source",
    ] {
        let source =
            format!("def ordinary(flag):\n    if flag:\n        {name} = 1\n    return {name}\n");
        let module = crate::lower_python_to_blockpy_for_testing(&source)
            .expect("ordinary possibly-unbound declaration")
            .blockpy_module;
        let function = module
            .callable_defs
            .iter()
            .find(|function| function.names.display_name == "ordinary")
            .unwrap();
        let layout = function.storage_layout.as_ref().unwrap();
        let slot = layout
            .stack_slots()
            .iter()
            .position(|candidate| candidate == name)
            .unwrap();
        assert!(
            layout
                .block_parameter_roles_at(NameLocation::local(slot as u32))
                .next()
                .is_none(),
            "source spelling must not initialize or forward a compiler control value"
        );
        assert!(layout.block_parameter_roles.is_empty());
        layout.validate_block_parameter_roles().unwrap();
    }
}

#[test]
fn resolved_block_parameter_roles_keep_actual_transport_copies_after_retirement() {
    use crate::pass_tracker::LoweringPassTrackerInternalExt;
    use soac_core::block_py::BlockParamRole;
    let result = crate::lower_python_to_blockpy_for_testing(
        "def suspended(work):\n    try:\n        try:\n            raise ValueError()\n        except ValueError:\n            yield work()\n            return work()\n    finally:\n        work()\n",
    ).expect("actual suspended handler and pending-return producer");
    let bound = result.pass_tracker.pass_name_binding().unwrap();
    let function = bound
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "suspended")
        .unwrap();
    let layout = function.storage_layout.as_ref().unwrap();
    for role in [
        BlockParamRole::Exception,
        BlockParamRole::AbruptKind,
        BlockParamRole::AbruptPayload,
    ] {
        assert!(layout
            .block_parameter_roles
            .iter()
            .any(|binding| binding.role == role));
    }
    let copies = crate::passes::block_parameter_roles::block_parameter_transport_copies(function);
    let mut covered = 0;
    for (source, target) in copies {
        for role in layout.block_parameter_roles_at(source.location) {
            assert!(layout
                .block_parameter_roles_at(target.location)
                .any(|candidate| candidate == role));
            covered += 1;
        }
    }
    assert!(
        covered > 0,
        "fixture must include real role-bearing raw transport aliases"
    );
    let final_function = result
        .blockpy_module
        .callable_defs
        .iter()
        .find(|candidate| candidate.function_id == function.function_id)
        .unwrap();
    let final_layout = final_function.storage_layout.as_ref().unwrap();
    for binding in &layout.block_parameter_roles {
        assert!(
            final_layout.block_parameter_roles.contains(binding),
            "retiring/consuming transport copies must retain slot semantics"
        );
    }
    final_layout.validate_block_parameter_roles().unwrap();
}

#[test]
fn resolved_block_parameter_roles_reject_invalid_and_source_owner_locations() {
    use soac_core::block_py::{BlockParamRole, NameLocation, ResolvedBlockParameterRole};
    let module = crate::lower_python_to_blockpy_for_testing(
        "def owner(value):\n    def read():\n        return value\n    try:\n        return read\n    finally:\n        observe()\n",
    ).expect("actual owned source cell with control transport").blockpy_module;
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.display_name == "owner")
        .unwrap();
    let layout = function.storage_layout.as_ref().unwrap();
    layout.validate_block_parameter_roles().unwrap();
    let existing = *layout
        .block_parameter_roles
        .iter()
        .find(|binding| binding.role == BlockParamRole::Exception)
        .unwrap();
    let mut missing = layout.clone();
    missing.block_parameter_roles.clear();
    assert!(missing
        .validate_block_parameter_declarations(
            function.blocks.iter().flat_map(|block| &block.params),
        )
        .is_err());
    let mut duplicate = layout.clone();
    duplicate.block_parameter_roles.push(existing);
    assert!(duplicate.validate_block_parameter_roles().is_err());

    let mut incompatible = layout.clone();
    incompatible.record_block_parameter_role(existing.location, BlockParamRole::AbruptKind);
    assert!(incompatible.validate_block_parameter_roles().is_err());

    for location in [
        NameLocation::local(u32::MAX),
        NameLocation::preserved(u32::MAX),
        NameLocation::global(0),
    ] {
        let mut absent = layout.clone();
        absent
            .block_parameter_roles
            .push(ResolvedBlockParameterRole {
                location,
                role: BlockParamRole::Exception,
            });
        assert!(absent.validate_block_parameter_roles().is_err());
    }
    let raw_owner = layout
        .cellvars
        .iter()
        .find(|cell| cell.logical_name == "value")
        .expect("source value is backed by its actual owned cell");
    let raw_slot = layout
        .stack_slots()
        .iter()
        .position(|name| name == &raw_owner.storage_name)
        .unwrap();
    let mut overlap = layout.clone();
    overlap.record_block_parameter_role(
        NameLocation::local(raw_slot as u32),
        BlockParamRole::Exception,
    );
    assert!(
        overlap.validate_block_parameter_roles().is_err(),
        "a raw source-cell owner is not exception transport"
    );
}
