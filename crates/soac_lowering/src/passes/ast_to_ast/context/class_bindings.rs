use super::*;
use soac_core::block_py::{
    ClassBindingPhase, ClassBindingScope, ClassBindingSlotBinding, ClassBindingSlotId,
    NativeCodeId, NativeCompileScopeKind,
};

/// Private class-cell lowering. Value aliases below name cell contents, never
/// additional owners; native comprehension slots are not compiler bindings.
#[derive(Clone)]
pub(crate) struct NativeClassLoweringPlan {
    pub scope: ClassBindingScope,
    pub execution_binding: String,
    pub return_binding: String,
    pub value_bindings: HashMap<ClassBindingSlotId, String>,
}

impl NativeClassLoweringPlan {
    pub(crate) fn value_binding(&self, slot: ClassBindingSlotId) -> &str {
        self.value_bindings
            .get(&slot)
            .expect("native cell value binding")
    }
}

#[derive(Clone, Copy)]
pub(crate) enum NativeClassBodyBoundary {
    Initialize(ClassBindingPhase),
    Complete,
}

pub(super) struct NativeClassPlans {
    pub canonical: Arc<crate::CanonicalClassBindings>,
    plans: RefCell<HashMap<NativeCodeId, NativeClassLoweringPlan>>,
    phases: RefCell<HashMap<NodeIndex, (NativeCodeId, NativeClassBodyBoundary)>>,
}

impl NativeClassPlans {
    pub(super) fn new(canonical: Arc<crate::CanonicalClassBindings>) -> Self {
        Self {
            canonical,
            plans: RefCell::new(HashMap::new()),
            phases: RefCell::new(HashMap::new()),
        }
    }
}

impl Context {
    pub(crate) fn set_canonical_class_bindings(
        &mut self,
        canonical: Option<Arc<crate::CanonicalClassBindings>>,
    ) {
        self.native_classes = canonical.map(NativeClassPlans::new);
    }

    pub(crate) fn native_class_plan(&self, range: TextRange) -> Option<NativeClassLoweringPlan> {
        self.strict_source()?;
        let classes = self
            .native_classes
            .as_ref()
            .expect("strict class lowering requires its source-bound native binding recipe");
        let source = self.type_expression_definition(range, DefinitionKind::Class);
        let native = self.native_definition(&source);
        let expected_range =
            soac_contracts::SourceRange::new(native.header_offset, source.source_range.end);
        let mut nodes = classes.canonical.nodes().iter().filter(|node| {
            node.compile_scope == NativeCompileScopeKind::Class
                && node.source_range == Some(expected_range)
        });
        let node = nodes
            .next()
            .expect("strict class requires the exact original native ClassDef node");
        assert!(
            nodes.next().is_none(),
            "original ClassDef node must be unique"
        );
        if let Some(plan) = classes.plans.borrow().get(&node.id) {
            return Some(plan.clone());
        }
        let recipe = classes
            .canonical
            .class_recipe(node.id)
            .expect("validated class has a recipe")
            .clone();
        let mut value_bindings = HashMap::new();
        let slots = recipe
            .initializers
            .iter()
            .filter(|init| init.phase == ClassBindingPhase::ClassEntry)
            .map(|init| init.slot)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|slot| {
                assert!(node.slots[slot.index as usize].kind.is_cell());
                value_bindings.insert(slot, self.fresh_annotation_binding("class_cell_value"));
                ClassBindingSlotBinding {
                    slot,
                    binding: self.fresh_annotation_binding("class_current"),
                }
            })
            .collect();
        let plan = NativeClassLoweringPlan {
            scope: ClassBindingScope {
                source,
                node: node.clone(),
                recipe,
                namespace_binding: self.fresh_annotation_binding("class_namespace"),
                slots,
            },
            execution_binding: self.fresh_annotation_binding("namespace_execution"),
            return_binding: self.fresh_annotation_binding("class_return"),
            value_bindings,
        };
        classes.plans.borrow_mut().insert(node.id, plan.clone());
        Some(plan)
    }

    pub(crate) fn native_class_plan_by_code(&self, code: NativeCodeId) -> NativeClassLoweringPlan {
        self.native_classes
            .as_ref()
            .expect("native class recipes")
            .plans
            .borrow()
            .get(&code)
            .expect("allocated original class plan")
            .clone()
    }

    pub(crate) fn native_class_phase_marker(
        &self,
        code: NativeCodeId,
        phase: ClassBindingPhase,
        semantic: &crate::passes::ast_to_ast::semantic::SemanticAstState,
    ) -> Stmt {
        let marker = crate::template::py_stmt!("pass");
        // Class rewriting runs after semantic provenance allocation begins.
        // Use that allocator, never the earlier annotation-stage counter.
        let node = semantic.assign_generated_node_index(&marker);
        assert!(self
            .native_classes
            .as_ref()
            .expect("class recipes")
            .phases
            .borrow_mut()
            .insert(node, (code, NativeClassBodyBoundary::Initialize(phase)))
            .is_none());
        marker
    }

    pub(crate) fn native_class_completion_marker(
        &self,
        code: NativeCodeId,
        semantic: &crate::passes::ast_to_ast::semantic::SemanticAstState,
    ) -> Stmt {
        let marker = crate::template::py_stmt!("pass");
        let node = semantic.assign_generated_node_index(&marker);
        assert!(self
            .native_classes
            .as_ref()
            .expect("class recipes")
            .phases
            .borrow_mut()
            .insert(node, (code, NativeClassBodyBoundary::Complete))
            .is_none());
        marker
    }

    pub(crate) fn native_class_boundary(
        &self,
        node: NodeIndex,
    ) -> Option<(NativeCodeId, NativeClassBodyBoundary)> {
        self.native_classes
            .as_ref()?
            .phases
            .borrow()
            .get(&node)
            .copied()
    }
}

impl Context {
    /// Resolve a native direct child by its original role and range. Generated
    /// helper spelling never chooses a code node or a capture source.
    pub(crate) fn native_class_child(
        &self,
        parent: NativeCodeId,
        function: &ast::StmtFunctionDef,
        scope: &soac_core::block_py::CallableScopeInfo,
    ) -> Option<soac_core::block_py::ClassBindingCodeNode> {
        use soac_core::block_py::CallableSourceRole;
        let classes = self.native_classes.as_ref()?;
        let origin = scope.source_origin.as_ref();
        if origin.is_some_and(|origin| {
            matches!(
                origin.role,
                CallableSourceRole::ModuleBody | CallableSourceRole::ClassConstruction
            )
        }) {
            return None;
        }
        if let Some(class) = &scope.class_bindings {
            return (class.node.parent == Some(parent)).then(|| class.node.clone());
        }
        let (compile_scope, range) = if let Some(provider) = &scope.annotation_provider {
            let range = provider
                .native_range
                .or_else(|| {
                    origin.map(|origin| {
                        let native = self.native_definition(&origin.definition);
                        soac_contracts::SourceRange::new(
                            native.header_offset,
                            origin.definition.source_range.end,
                        )
                    })
                })
                .expect("native annotation child has an original expression or declaration range");
            (NativeCompileScopeKind::Annotations, range)
        } else if let Some(parameters) = &scope.type_parameter_scope {
            (
                NativeCompileScopeKind::Annotations,
                parameters.native_header_range,
            )
        } else if let Some(generator) = &scope.generator_expression_code {
            (
                NativeCompileScopeKind::Comprehension,
                generator.expression_range,
            )
        } else if let Some(origin) = origin.filter(|origin| {
            origin.role == CallableSourceRole::SourceFunction
                && origin.definition.definition_kind == DefinitionKind::Lambda
        }) {
            (
                NativeCompileScopeKind::Lambda,
                origin.definition.source_range,
            )
        } else if let Some(origin) =
            origin.filter(|origin| origin.role == CallableSourceRole::SourceFunction)
        {
            let native = self.native_definition(&origin.definition);
            (
                if function.is_async {
                    NativeCompileScopeKind::AsyncFunction
                } else {
                    NativeCompileScopeKind::Function
                },
                soac_contracts::SourceRange::new(
                    native.header_offset,
                    origin.definition.source_range.end,
                ),
            )
        } else {
            return None;
        };
        let mut nodes = classes.canonical.nodes().iter().filter(|node| {
            node.parent == Some(parent)
                && node.compile_scope == compile_scope
                && node.source_range == Some(range)
                && if scope.type_parameter_scope.is_some() {
                    node.symbol_scope
                        == soac_core::block_py::NativeSymbolScopeKind::TypeParametersBlock
                } else if let Some(provider) = &scope.annotation_provider {
                    use soac_core::block_py::{AnnotationProviderKind, NativeSymbolScopeKind};
                    node.symbol_scope
                        == match provider.kind {
                            AnnotationProviderKind::Dictionary => {
                                NativeSymbolScopeKind::AnnotationBlock
                            }
                            AnnotationProviderKind::TypeAliasValue => {
                                NativeSymbolScopeKind::TypeAliasBlock
                            }
                            AnnotationProviderKind::TypeParameterBound
                            | AnnotationProviderKind::TypeParameterConstraints
                            | AnnotationProviderKind::TypeParameterDefault => {
                                NativeSymbolScopeKind::TypeVariableBlock
                            }
                        }
                } else {
                    node.symbol_scope == soac_core::block_py::NativeSymbolScopeKind::FunctionBlock
                }
        });
        let child = nodes.next()?;
        assert!(
            nodes.next().is_none(),
            "original class child role and range must identify one native node"
        );
        Some(child.clone())
    }
}
