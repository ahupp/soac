mod class_bindings;
pub(crate) use class_bindings::{NativeClassBodyBoundary, NativeClassLoweringPlan};

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::passes::ast_to_ast::scope_helpers::ScopeKind;
use crate::passes::ast_to_ast::semantic::LoweredLambdaBody;

use super::source_origins::SourceCatalog;
use crate::namegen::fresh_name;
use crate::transformer::{walk_stmt, Transformer};
use ruff_python_ast::{self as ast, HasNodeIndex, NodeIndex, Stmt};
use ruff_text_size::{Ranged, TextRange};
use soac_contracts::{
    DecoratorKind, DefinitionKind, NominalBindingOwner, ParticipationProposal, SourceIdentity,
    TransformKind, UncertaintyReason, VerifiedModuleTypeFacts,
};
use soac_core::block_py::{
    AnnotationProviderKind, CallableSourceOrigin, CallableSourceRole, CellCaptureProjection,
    FunctionKind, LexicalCellBinding, LexicalCellCapture, RuntimeFunctionId, StrictModuleSource,
    TypeParameterKind, TypeParameterScopeInput,
};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct ScopeFrame {
    pub kind: ScopeKind,
    pub in_async_function: bool,
    pub globals: HashSet<String>,
    pub nonlocals: HashSet<String>,
}

impl ScopeFrame {
    pub(crate) fn module() -> Self {
        Self {
            kind: ScopeKind::Module,
            in_async_function: false,
            globals: HashSet::new(),
            nonlocals: HashSet::new(),
        }
    }

    pub(crate) fn new(
        kind: ScopeKind,
        globals: HashSet<String>,
        nonlocals: HashSet<String>,
    ) -> Self {
        Self {
            kind,
            in_async_function: false,
            globals,
            nonlocals,
        }
    }
}

/// A creation site recorded by the function-definition rewrite, not recovered
/// from a helper's spelling, literal function ID, or source range.
#[derive(Clone)]
pub(crate) struct FunctionConstructionSite {
    pub function_id: RuntimeFunctionId,
    pub kind: FunctionKind,
    pub class_namespace_binding: Option<String>,
    pub creation_cells: Vec<LexicalCellBinding>,
}

#[derive(Clone)]
pub(crate) struct FunctionDefinitionSite {
    pub function_id: RuntimeFunctionId,
    pub definition: SourceIdentity,
}

/// A source-selected decorator boundary recorded by the class rewrite. The
/// record selects an operation, not authority for the actual decorator value.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ClassDecoratorOperation {
    Prepare {
        declaration: TextRange,
        factory: bool,
    },
    Apply {
        declaration: TextRange,
    },
    DiscardPreparation,
    DeleteBinding,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AnnotationOperation {
    NewSet,
    Setup,
    CheckFormat,
    Record {
        index: u32,
    },
    CreateAlias {
        declaration: TextRange,
    },
    CreateParameter {
        declaration: TextRange,
        kind: TypeParameterKind,
    },
    SetParameterDefault {
        declaration: TextRange,
    },
    ConstructTypeParameterScope {
        declaration: TextRange,
        kind: DefinitionKind,
        positional_defaults: bool,
        keyword_defaults: bool,
        complete_function: bool,
    },
    SubscriptGeneric {
        declaration: TextRange,
    },
    SetFunctionTypeParameters {
        declaration: TextRange,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct AnnotationClassCapture {
    pub body_binding: String,
    pub source_name: String,
    pub projection: CellCaptureProjection,
}

#[derive(Clone, Debug)]
pub(crate) struct AnnotationProviderPlan {
    pub kind: AnnotationProviderKind,
    pub native_range: Option<soac_contracts::SourceRange>,
    pub body_format_parameter: String,
    pub class_dictionary: Option<AnnotationClassCapture>,
    pub conditional_annotations: Option<AnnotationConditionalCell>,
}

#[derive(Clone, Debug)]
pub(crate) struct AnnotationConditionalCell {
    pub body_binding: String,
    pub owner_binding: String,
    pub storage_name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TypeParameterScopePlan {
    pub definition: SourceIdentity,
    pub inputs: Vec<TypeParameterScopeInput>,
    pub class_dictionary: Option<AnnotationClassCapture>,
    pub conditional_annotations: Option<AnnotationConditionalCell>,
    pub owned_parameter_tuple: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct GenericFunctionPlan {
    pub positional_defaults: Option<String>,
    pub keyword_defaults: Option<String>,
    pub type_parameters: String,
}

#[derive(Clone, Debug)]
pub(crate) struct GenericClassPlan {
    pub type_parameters: String,
    pub generic_base: String,
}

pub(crate) struct Context {
    native_classes: Option<class_bindings::NativeClassPlans>,
    pub source: String,
    scope_stack: RefCell<Vec<ScopeFrame>>,
    value_forwarding_local_stack: RefCell<Vec<HashSet<String>>>,
    no_raise_local_stack: RefCell<Vec<HashSet<String>>>,
    class_static_attributes: RefCell<HashMap<TextRange, Vec<String>>>,
    class_dict_cells: RefCell<HashSet<TextRange>>,
    class_annotation_cells: RefCell<HashMap<TextRange, AnnotationConditionalCell>>,
    strict_facts: Option<Arc<VerifiedModuleTypeFacts>>,
    future_annotations: bool,
    source_catalog: SourceCatalog,
    callable_origins: RefCell<HashMap<(TextRange, String), CallableSourceOrigin>>,
    lowered_lambda_bodies: RefCell<HashMap<NodeIndex, LoweredLambdaBody>>,
    generator_expressions:
        RefCell<HashMap<(TextRange, String), soac_core::block_py::GeneratorExpressionCode>>,
    function_constructions: RefCell<HashMap<NodeIndex, FunctionConstructionSite>>,
    function_completions: RefCell<HashMap<NodeIndex, FunctionDefinitionSite>>,
    function_descriptor_applications: RefCell<HashMap<NodeIndex, FunctionDefinitionSite>>,
    class_decorator_operations: RefCell<HashMap<NodeIndex, ClassDecoratorOperation>>,
    class_construction_functions: RefCell<HashMap<SourceIdentity, RuntimeFunctionId>>,
    class_namespace_functions: RefCell<HashMap<SourceIdentity, RuntimeFunctionId>>,
    class_namespace_bindings: RefCell<HashMap<SourceIdentity, String>>,
    class_capture_discards: RefCell<HashSet<NodeIndex>>,
    function_annotation_targets: RefCell<HashMap<(TextRange, String), (TextRange, String)>>,
    annotation_providers: RefCell<HashMap<(TextRange, String), AnnotationProviderPlan>>,
    annotation_operations: RefCell<HashMap<NodeIndex, AnnotationOperation>>,
    type_expression_functions:
        RefCell<HashMap<(TextRange, AnnotationProviderKind), RuntimeFunctionId>>,
    type_parameter_scopes: RefCell<HashMap<(TextRange, String), TypeParameterScopePlan>>,
    type_parameter_scope_functions:
        RefCell<HashMap<(TextRange, DefinitionKind), RuntimeFunctionId>>,
    source_functions: RefCell<HashMap<SourceIdentity, RuntimeFunctionId>>,
    generic_functions: RefCell<HashMap<(TextRange, String), GenericFunctionPlan>>,
    generic_classes: RefCell<HashMap<TextRange, GenericClassPlan>>,
    next_annotation_node: Cell<u32>,
}

impl Context {
    pub(crate) fn new(source: &str) -> Self {
        Self {
            native_classes: None,
            source: source.to_string(),
            scope_stack: RefCell::new(vec![ScopeFrame::module()]),
            value_forwarding_local_stack: RefCell::new(vec![HashSet::new()]),
            no_raise_local_stack: RefCell::new(vec![HashSet::new()]),
            class_static_attributes: RefCell::new(HashMap::new()),
            class_dict_cells: RefCell::new(HashSet::new()),
            class_annotation_cells: RefCell::new(HashMap::new()),
            strict_facts: None,
            future_annotations: false,
            source_catalog: SourceCatalog::default(),
            callable_origins: RefCell::new(HashMap::new()),
            lowered_lambda_bodies: RefCell::new(HashMap::new()),
            generator_expressions: RefCell::new(HashMap::new()),
            function_constructions: RefCell::new(HashMap::new()),
            function_completions: RefCell::new(HashMap::new()),
            function_descriptor_applications: RefCell::new(HashMap::new()),
            class_decorator_operations: RefCell::new(HashMap::new()),
            class_construction_functions: RefCell::new(HashMap::new()),
            class_namespace_functions: RefCell::new(HashMap::new()),
            class_namespace_bindings: RefCell::new(HashMap::new()),
            class_capture_discards: RefCell::new(HashSet::new()),
            function_annotation_targets: RefCell::new(HashMap::new()),
            annotation_providers: RefCell::new(HashMap::new()),
            annotation_operations: RefCell::new(HashMap::new()),
            type_expression_functions: RefCell::new(HashMap::new()),
            type_parameter_scopes: RefCell::new(HashMap::new()),
            type_parameter_scope_functions: RefCell::new(HashMap::new()),
            source_functions: RefCell::new(HashMap::new()),
            generic_functions: RefCell::new(HashMap::new()),
            generic_classes: RefCell::new(HashMap::new()),
            next_annotation_node: Cell::new(0),
        }
    }

    pub(crate) fn with_strict_facts(
        source: &str,
        facts: Option<Arc<VerifiedModuleTypeFacts>>,
        original_body: &ast::Suite,
        future_annotations: bool,
        original_tokens: Option<&ast::token::Tokens>,
    ) -> anyhow::Result<Self> {
        let mut context = Self::new(source);
        context.future_annotations = future_annotations;
        struct LastNode(u32);
        impl Transformer for LastNode {
            fn visit_stmt(&mut self, stmt: &mut Stmt) {
                if let Some(index) = stmt.node_index().load().as_u32() {
                    self.0 = self.0.max(index + 1);
                }
                walk_stmt(self, stmt);
            }
            fn visit_expr(&mut self, expr: &mut ast::Expr) {
                if let Some(index) = expr.node_index().load().as_u32() {
                    self.0 = self.0.max(index + 1);
                }
                crate::transformer::walk_expr(self, expr);
            }
        }
        let mut last_node = LastNode(0);
        last_node.visit_body(&mut original_body.clone());
        context.next_annotation_node.set(last_node.0);
        if let Some(facts) = facts {
            if !future_annotations {
                super::rewrite_stmt::annotation::validate_strict_annotation_shapes(original_body)?;
            }
            context.source_catalog = SourceCatalog::from_original(
                facts.facts(),
                original_body,
                original_tokens.expect("strict original source retains parser tokens"),
            )?;
            context.strict_facts = Some(facts);
        }
        // Record original source names before annotation/scoped-helper rewrites.
        // Use execution's private-name rewrite, never a second mangler.
        let mut name_source = original_body.clone();
        super::rewrite_class_def::private::rewrite_private_names(&context, &mut name_source);
        context.source_catalog.record_source_names(&name_source);
        Ok(context)
    }

    pub(crate) fn source_names(&self) -> &super::source_origins::SourceNameCatalog {
        self.source_catalog.source_names()
    }

    pub(crate) fn strict_source(&self) -> Option<StrictModuleSource> {
        self.strict_facts
            .as_deref()
            .map(StrictModuleSource::from_verified)
    }

    pub(crate) fn future_annotations(&self) -> bool {
        self.future_annotations
    }

    pub(crate) fn require_class_dict_cell(&self, class: TextRange) {
        self.class_dict_cells.borrow_mut().insert(class);
    }

    pub(crate) fn requires_class_dict_cell(&self, class: TextRange) -> bool {
        self.class_dict_cells.borrow().contains(&class)
    }

    pub(crate) fn record_class_annotation_cell(
        &self,
        class: TextRange,
        cell: AnnotationConditionalCell,
    ) {
        assert!(self
            .class_annotation_cells
            .borrow_mut()
            .insert(class, cell)
            .is_none());
    }

    pub(crate) fn class_annotation_cell(
        &self,
        class: TextRange,
    ) -> Option<AnnotationConditionalCell> {
        self.class_annotation_cells.borrow().get(&class).cloned()
    }

    pub(crate) fn fresh_annotation_binding(&self, role: &str) -> String {
        loop {
            let name = self.fresh(role);
            if !self.source.contains(&name) {
                return name;
            }
        }
    }

    pub(crate) fn record_annotation_operation(
        &self,
        call: &ast::Expr,
        operation: AnnotationOperation,
    ) {
        assert!(call.node_index().load().as_u32().is_none());
        let next = self.next_annotation_node.get();
        self.next_annotation_node
            .set(next.checked_add(1).expect("annotation node index overflow"));
        let node = NodeIndex::from(next);
        call.node_index().set(node);
        self.record_annotation_operation_node(node, operation);
    }

    pub(crate) fn record_annotation_operation_node(
        &self,
        node: NodeIndex,
        operation: AnnotationOperation,
    ) {
        assert!(
            node.as_u32().is_some(),
            "annotation operation needs an assigned node"
        );
        assert!(self
            .annotation_operations
            .borrow_mut()
            .insert(node, operation)
            .is_none());
    }

    pub(crate) fn annotation_operation(&self, node: NodeIndex) -> Option<AnnotationOperation> {
        self.annotation_operations.borrow().get(&node).copied()
    }

    pub(crate) fn type_expression_definition(
        &self,
        declaration: TextRange,
        kind: DefinitionKind,
    ) -> SourceIdentity {
        self.source_catalog
            .definition(declaration, kind)
            .expect("type expression must identify its original source declaration")
    }

    pub(crate) fn native_definition(
        &self,
        definition: &SourceIdentity,
    ) -> super::source_origins::NativeSourceDefinition {
        self.source_catalog
            .native_definition(
                TextRange::new(
                    definition.source_range.start.into(),
                    definition.source_range.end.into(),
                ),
                definition.definition_kind,
            )
            .expect("native projection must identify its original source declaration")
            .clone()
    }

    pub(crate) fn native_annotation_qualname(
        &self,
        definition: &SourceIdentity,
        kind: AnnotationProviderKind,
    ) -> String {
        if definition.definition_kind == DefinitionKind::Module {
            return "__annotate__".into();
        }
        let native = self.native_definition(definition);
        if kind != AnnotationProviderKind::Dictionary {
            return native.qualname;
        }
        if definition.definition_kind == DefinitionKind::Class {
            return format!("{}.__annotate__", native.qualname);
        }
        match native.qualname.rsplit_once('.') {
            Some((prefix, _)) => format!("{prefix}.__annotate__"),
            None => "__annotate__".into(),
        }
    }

    pub(crate) fn record_type_parameter_scope(
        &self,
        helper: &mut ast::StmtFunctionDef,
        plan: TypeParameterScopePlan,
    ) {
        helper.range = TextRange::new(
            plan.definition.source_range.start.into(),
            plan.definition.source_range.end.into(),
        );
        self.record_callable_origin(
            helper,
            CallableSourceOrigin {
                definition: plan.definition.clone(),
                role: CallableSourceRole::TypeParameterScope,
            },
        );
        assert!(self
            .type_parameter_scopes
            .borrow_mut()
            .insert((helper.range, helper.name.to_string()), plan)
            .is_none());
    }

    pub(crate) fn type_parameter_scope(
        &self,
        helper: &ast::StmtFunctionDef,
    ) -> Option<TypeParameterScopePlan> {
        self.type_parameter_scopes
            .borrow()
            .get(&(helper.range, helper.name.to_string()))
            .cloned()
    }

    pub(crate) fn record_type_parameter_scope_function(
        &self,
        definition: &SourceIdentity,
        function: RuntimeFunctionId,
    ) {
        let range = TextRange::new(
            definition.source_range.start.into(),
            definition.source_range.end.into(),
        );
        assert!(self
            .type_parameter_scope_functions
            .borrow_mut()
            .insert((range, definition.definition_kind), function)
            .is_none());
    }

    pub(crate) fn type_parameter_scope_function(
        &self,
        declaration: TextRange,
        kind: DefinitionKind,
    ) -> RuntimeFunctionId {
        *self
            .type_parameter_scope_functions
            .borrow()
            .get(&(declaration, kind))
            .expect("generic construction must follow its explicit scope definition")
    }

    pub(crate) fn record_source_function(
        &self,
        definition: &SourceIdentity,
        function: RuntimeFunctionId,
    ) {
        assert!(self
            .source_functions
            .borrow_mut()
            .insert(definition.clone(), function)
            .is_none());
    }

    pub(crate) fn source_function(&self, definition: &SourceIdentity) -> RuntimeFunctionId {
        *self
            .source_functions
            .borrow()
            .get(definition)
            .expect("generic function must be lowered before its enclosing scope call")
    }

    pub(crate) fn class_decorator_proposal(
        &self,
        declaration: TextRange,
        decorators: &[ast::Decorator],
    ) -> Option<SourceIdentity> {
        let [decorator] = decorators else {
            return None;
        };
        let facts = self.strict_facts.as_deref()?.facts();
        let definition = self
            .source_catalog
            .definition(declaration, DefinitionKind::Class)?;
        let fact = facts
            .classes
            .iter()
            .find(|fact| fact.identity == definition)?;
        let [proposal] = fact.decorators.as_slice() else {
            return None;
        };
        let transform = fact.transform.as_ref()?;
        let range = decorator.expression.range();
        (fact.participation == ParticipationProposal::Candidate
            && fact
                .uncertainty
                .iter()
                .all(|reason| *reason == UncertaintyReason::OpenWorld)
            && proposal.kind == DecoratorKind::StdlibDataclass
            && proposal.uncertainty.is_empty()
            && proposal.expression_range.start == range.start().to_u32()
            && proposal.expression_range.end == range.end().to_u32()
            && transform.kind == TransformKind::StdlibDataclass
            && transform.dataclass_options.is_some())
        .then_some(definition)
    }

    pub(crate) fn record_class_decorator_operation(
        &self,
        node: NodeIndex,
        operation: ClassDecoratorOperation,
    ) {
        assert!(
            node.as_u32().is_some(),
            "class decorator operation needs an assigned node"
        );
        assert!(self
            .class_decorator_operations
            .borrow_mut()
            .insert(node, operation)
            .is_none());
    }

    pub(crate) fn class_decorator_operation(
        &self,
        node: NodeIndex,
    ) -> Option<ClassDecoratorOperation> {
        self.class_decorator_operations.borrow().get(&node).copied()
    }

    pub(crate) fn record_class_construction_function(
        &self,
        definition: &SourceIdentity,
        function: RuntimeFunctionId,
    ) {
        assert!(self
            .class_construction_functions
            .borrow_mut()
            .insert(definition.clone(), function)
            .is_none());
    }

    pub(crate) fn class_construction_function(
        &self,
        definition: &SourceIdentity,
    ) -> RuntimeFunctionId {
        *self
            .class_construction_functions
            .borrow()
            .get(definition)
            .expect("class construction must be lowered before its enclosing scope")
    }

    pub(crate) fn record_class_namespace_binding(&self, declaration: TextRange, binding: &str) {
        if self.strict_source().is_none() {
            return;
        }
        let definition = self.type_expression_definition(declaration, DefinitionKind::Class);
        assert!(self
            .class_namespace_bindings
            .borrow_mut()
            .insert(definition, binding.to_owned())
            .is_none());
    }

    pub(crate) fn class_namespace_binding(&self, definition: &SourceIdentity) -> String {
        self.class_namespace_bindings
            .borrow()
            .get(definition)
            .expect("class construction must have its explicit namespace binding")
            .clone()
    }

    pub(crate) fn record_class_namespace_function(
        &self,
        definition: &SourceIdentity,
        function: RuntimeFunctionId,
    ) {
        assert!(self
            .class_namespace_functions
            .borrow_mut()
            .insert(definition.clone(), function)
            .is_none());
    }

    pub(crate) fn class_namespace_function(
        &self,
        definition: &SourceIdentity,
    ) -> RuntimeFunctionId {
        *self
            .class_namespace_functions
            .borrow()
            .get(definition)
            .expect("class namespace must be lowered before its constructor")
    }

    /// A source proposal only. The caller checks the actual lexical
    /// storage decision, and runtime creation independently authenticates the
    /// active producer and every signed field leaf before retaining cells.
    pub(crate) fn class_construction_capture_slots(
        &self,
        definition: &SourceIdentity,
    ) -> Vec<LexicalCellCapture> {
        let Some(verified) = self.strict_facts.as_deref() else {
            return Vec::new();
        };
        let facts = verified.facts();
        let Some(class) = facts.classes.iter().find(|class| {
            &class.identity == definition && class.participation == ParticipationProposal::Candidate
        }) else {
            return Vec::new();
        };
        let required: BTreeSet<_> = class
            .required_field_bindings(&facts.language_policy)
            .into_iter()
            .filter_map(|field| field.annotation_reference())
            .collect();
        let mut by_binding: BTreeMap<LexicalCellBinding, Vec<u32>> = BTreeMap::new();
        for (index, leaf) in facts.nominal_bindings.iter().enumerate() {
            let NominalBindingOwner::Field { field } = &leaf.owner else {
                continue;
            };
            if &field.declaring_class.definition != definition
                || !required.contains(field)
                || leaf.binding_scope.definition_kind != DefinitionKind::Function
                // Direct self is bound by the actual native pre-Ready callback;
                // its still-unbound source cell is not a construction input.
                || (&leaf.binding == definition && leaf.class == field.declaring_class)
            {
                continue;
            }
            by_binding
                .entry(LexicalCellBinding {
                    scope: leaf.binding_scope.clone(),
                    name: leaf.name.clone(),
                })
                .or_default()
                .push(
                    u32::try_from(index)
                        .expect("nominal-binding index must fit the signed source size"),
                );
        }
        by_binding
            .into_iter()
            .map(|(binding, nominal_binding_indices)| LexicalCellCapture {
                binding,
                nominal_binding_indices,
            })
            .collect()
    }

    pub(crate) fn record_class_capture_discard(&self, node: NodeIndex) {
        assert!(node.as_u32().is_some());
        assert!(self.class_capture_discards.borrow_mut().insert(node));
    }

    pub(crate) fn is_class_capture_discard(&self, node: NodeIndex) -> bool {
        self.class_capture_discards.borrow().contains(&node)
    }

    pub(crate) fn record_generic_function(
        &self,
        function: &ast::StmtFunctionDef,
        plan: GenericFunctionPlan,
    ) {
        assert!(self
            .generic_functions
            .borrow_mut()
            .insert((function.range, function.name.to_string()), plan)
            .is_none());
    }

    pub(crate) fn generic_function(
        &self,
        function: &ast::StmtFunctionDef,
    ) -> Option<GenericFunctionPlan> {
        self.generic_functions
            .borrow()
            .get(&(function.range, function.name.to_string()))
            .cloned()
    }

    pub(crate) fn record_generic_class(&self, declaration: TextRange, plan: GenericClassPlan) {
        assert!(self
            .generic_classes
            .borrow_mut()
            .insert(declaration, plan)
            .is_none());
    }

    pub(crate) fn generic_class(&self, declaration: TextRange) -> Option<GenericClassPlan> {
        self.generic_classes.borrow().get(&declaration).cloned()
    }

    pub(crate) fn record_type_expression_helper(
        &self,
        declaration: TextRange,
        kind: DefinitionKind,
        helper: &mut ast::StmtFunctionDef,
    ) {
        let definition = self.type_expression_definition(declaration, kind);
        helper.range = declaration;
        self.record_callable_origin(
            helper,
            CallableSourceOrigin {
                definition,
                role: CallableSourceRole::AnnotationProvider,
            },
        );
    }

    pub(crate) fn record_type_expression_function(
        &self,
        definition: &SourceIdentity,
        kind: AnnotationProviderKind,
        function: RuntimeFunctionId,
    ) {
        let range = TextRange::new(
            definition.source_range.start.into(),
            definition.source_range.end.into(),
        );
        assert!(
            self.type_expression_functions
                .borrow_mut()
                .insert((range, kind), function)
                .is_none(),
            "type-expression declaration has multiple lowered evaluators of the same kind"
        );
    }

    pub(crate) fn type_expression_function(
        &self,
        declaration: TextRange,
        kind: AnnotationProviderKind,
    ) -> RuntimeFunctionId {
        *self
            .type_expression_functions
            .borrow()
            .get(&(declaration, kind))
            .expect("type-expression factory must follow its explicit evaluator definition")
    }

    pub(crate) fn record_annotation_provider(
        &self,
        helper: &ast::StmtFunctionDef,
        plan: AnnotationProviderPlan,
    ) {
        assert!(self
            .annotation_providers
            .borrow_mut()
            .insert((helper.range, helper.name.to_string()), plan,)
            .is_none());
    }

    pub(crate) fn annotation_provider(
        &self,
        helper: &ast::StmtFunctionDef,
    ) -> Option<AnnotationProviderPlan> {
        self.annotation_providers
            .borrow()
            .get(&(helper.range, helper.name.to_string()))
            .cloned()
    }

    pub(crate) fn record_function_construction(
        &self,
        node: NodeIndex,
        function_id: RuntimeFunctionId,
        kind: FunctionKind,
        class_namespace_binding: Option<String>,
        creation_cells: Vec<LexicalCellBinding>,
    ) {
        assert!(
            node.as_u32().is_some(),
            "construction site needs an assigned node"
        );
        assert!(
            self.function_constructions
                .borrow_mut()
                .insert(
                    node,
                    FunctionConstructionSite {
                        function_id,
                        kind,
                        class_namespace_binding,
                        creation_cells
                    }
                )
                .is_none(),
            "function construction site must be registered exactly once"
        );
    }

    pub(crate) fn function_construction(
        &self,
        node: NodeIndex,
    ) -> Option<FunctionConstructionSite> {
        self.function_constructions.borrow().get(&node).cloned()
    }

    pub(crate) fn function_descriptor_proposal(
        &self,
        definition: &SourceIdentity,
        decorators: &[ast::Expr],
    ) -> bool {
        let [decorator] = decorators else {
            // No authority is carried through an intervening decorator.
            return false;
        };
        let Some(facts) = self.strict_facts.as_deref() else {
            return false;
        };
        let Some(function) = facts
            .facts()
            .functions
            .iter()
            .find(|function| &function.identity == definition)
        else {
            return false;
        };
        let [proposal] = function.decorators.as_slice() else {
            return false;
        };
        let range = decorator.range();
        matches!(
            proposal.kind,
            DecoratorKind::StaticMethod | DecoratorKind::ClassMethod | DecoratorKind::Property
        ) && proposal.uncertainty.is_empty()
            && proposal.arguments.is_empty()
            && proposal.expression_range.start == range.start().to_u32()
            && proposal.expression_range.end == range.end().to_u32()
    }

    pub(crate) fn record_function_descriptor_application(
        &self,
        node: NodeIndex,
        function_id: RuntimeFunctionId,
        definition: SourceIdentity,
    ) {
        assert!(
            node.as_u32().is_some(),
            "descriptor site needs an assigned node"
        );
        assert!(
            self.function_descriptor_applications
                .borrow_mut()
                .insert(
                    node,
                    FunctionDefinitionSite {
                        function_id,
                        definition
                    },
                )
                .is_none(),
            "descriptor site must be registered exactly once"
        );
    }

    pub(crate) fn function_descriptor_application(
        &self,
        node: NodeIndex,
    ) -> Option<FunctionDefinitionSite> {
        self.function_descriptor_applications
            .borrow()
            .get(&node)
            .cloned()
    }

    pub(crate) fn record_function_completion(
        &self,
        node: NodeIndex,
        function_id: RuntimeFunctionId,
        definition: SourceIdentity,
    ) {
        assert!(
            node.as_u32().is_some(),
            "completion site needs an assigned node"
        );
        assert!(
            self.function_completions
                .borrow_mut()
                .insert(
                    node,
                    FunctionDefinitionSite {
                        function_id,
                        definition
                    }
                )
                .is_none(),
            "function completion site must be registered exactly once"
        );
    }

    pub(crate) fn function_completion(&self, node: NodeIndex) -> Option<FunctionDefinitionSite> {
        self.function_completions.borrow().get(&node).cloned()
    }

    pub(crate) fn record_generator_expression(
        &self,
        expression: TextRange,
        helper: &mut ast::StmtFunctionDef,
    ) {
        helper.range = expression;
        let Some(projection) = self.source_catalog.generator_expression(expression) else {
            // Compiler-created generators without an original expression do
            // not acquire a source-code exposure by sharing a generated name.
            return;
        };
        assert!(self
            .generator_expressions
            .borrow_mut()
            .insert((helper.range, helper.name.to_string()), projection,)
            .is_none());
    }

    pub(crate) fn generator_expression_code(
        &self,
        helper: &ast::StmtFunctionDef,
    ) -> Option<soac_core::block_py::GeneratorExpressionCode> {
        self.generator_expressions
            .borrow()
            .get(&(helper.range, helper.name.to_string()))
            .cloned()
    }

    fn record_callable_origin(
        &self,
        function: &ast::StmtFunctionDef,
        origin: CallableSourceOrigin,
    ) {
        let key = (function.range, function.name.to_string());
        let previous = self.callable_origins.borrow_mut().insert(key, origin);
        assert!(
            previous.is_none(),
            "callable origin must be assigned exactly once"
        );
    }

    pub(crate) fn callable_origin(
        &self,
        function: &ast::StmtFunctionDef,
    ) -> Option<CallableSourceOrigin> {
        self.callable_origins
            .borrow()
            .get(&(function.range, function.name.to_string()))
            .cloned()
    }

    pub(crate) fn record_original_function_origins(&self, body: &mut ast::Suite) {
        struct OriginalFunctions<'a>(&'a Context);
        impl Transformer for OriginalFunctions<'_> {
            fn visit_stmt(&mut self, stmt: &mut Stmt) {
                if let Stmt::FunctionDef(function) = stmt {
                    if let Some(definition) = self
                        .0
                        .source_catalog
                        .definition(function.range, DefinitionKind::Function)
                    {
                        self.0.record_callable_origin(
                            function,
                            CallableSourceOrigin {
                                definition,
                                role: CallableSourceRole::SourceFunction,
                            },
                        );
                    }
                }
                walk_stmt(self, stmt);
            }
        }
        OriginalFunctions(self).visit_body(body);
    }

    pub(crate) fn record_module_body_origin(&self, function: &mut ast::StmtFunctionDef) {
        if let Some(facts) = &self.strict_facts {
            function.range = TextRange::new(0.into(), facts.facts().source_size.into());
            self.record_callable_origin(
                function,
                CallableSourceOrigin {
                    definition: facts.facts().module_body_identity(),
                    role: CallableSourceRole::ModuleBody,
                },
            );
        }
    }

    pub(crate) fn record_class_helper_origin(
        &self,
        class_range: TextRange,
        function: &mut ast::StmtFunctionDef,
        role: CallableSourceRole,
    ) {
        if let Some(definition) = self
            .source_catalog
            .definition(class_range, DefinitionKind::Class)
        {
            function.range = class_range;
            self.record_callable_origin(function, CallableSourceOrigin { definition, role });
        }
    }

    pub(crate) fn record_function_annotation_helper(
        &self,
        target: &ast::StmtFunctionDef,
        helper: &mut ast::StmtFunctionDef,
    ) {
        helper.range = target.range;
        let helper_key = (helper.range, helper.name.to_string());
        let target_key = (target.range, target.name.to_string());
        assert!(
            self.function_annotation_targets
                .borrow_mut()
                .insert(helper_key, target_key)
                .is_none(),
            "annotation helper must have one explicit target"
        );
        if let Some(origin) = self.callable_origin(target) {
            self.record_callable_origin(
                helper,
                CallableSourceOrigin {
                    definition: origin.definition,
                    role: CallableSourceRole::AnnotationProvider,
                },
            );
        }
    }

    pub(crate) fn function_annotation_target(
        &self,
        helper: &ast::StmtFunctionDef,
    ) -> Option<(TextRange, String)> {
        self.function_annotation_targets
            .borrow()
            .get(&(helper.range, helper.name.to_string()))
            .cloned()
    }

    pub(crate) fn record_module_annotation_helper(&self, helper: &mut ast::StmtFunctionDef) {
        if let Some(facts) = &self.strict_facts {
            helper.range = TextRange::new(0.into(), facts.facts().source_size.into());
            self.record_callable_origin(
                helper,
                CallableSourceOrigin {
                    definition: facts.facts().module_body_identity(),
                    role: CallableSourceRole::AnnotationProvider,
                },
            );
        }
    }

    pub(crate) fn take_lowered_lambda_body(&self, node: NodeIndex) -> Option<LoweredLambdaBody> {
        self.lowered_lambda_bodies.borrow_mut().remove(&node)
    }

    pub(crate) fn record_lowered_lambda_body(&self, node: NodeIndex, body: LoweredLambdaBody) {
        assert!(
            node.as_u32().is_some(),
            "a lowered lambda keeps its original node"
        );
        assert!(self
            .lowered_lambda_bodies
            .borrow_mut()
            .insert(node, body)
            .is_none());
    }

    pub(crate) fn take_lowered_lambda_bodies(&self) -> HashMap<NodeIndex, LoweredLambdaBody> {
        std::mem::take(&mut *self.lowered_lambda_bodies.borrow_mut())
    }

    pub(crate) fn record_lambda_origin(
        &self,
        lambda: &ast::ExprLambda,
        function: &mut ast::StmtFunctionDef,
    ) {
        function.range = lambda.range;
        if let Some(definition) = self
            .source_catalog
            .definition(lambda.range, DefinitionKind::Lambda)
        {
            function.range = lambda.range;
            self.record_callable_origin(
                function,
                CallableSourceOrigin {
                    definition,
                    role: CallableSourceRole::SourceFunction,
                },
            );
        }
    }

    pub(crate) fn record_class_static_attributes(
        &self,
        class_range: TextRange,
        attributes: Vec<String>,
    ) {
        let previous = self
            .class_static_attributes
            .borrow_mut()
            .insert(class_range, attributes);
        assert!(
            previous.is_none(),
            "class static attributes were already recorded for source range {class_range:?}"
        );
    }

    pub(crate) fn class_static_attributes(&self, class_range: TextRange) -> Option<Vec<String>> {
        self.class_static_attributes
            .borrow()
            .get(&class_range)
            .cloned()
    }

    pub(crate) fn line_number_at(&self, offset: usize) -> usize {
        self.source[..offset]
            .bytes()
            .filter(|&b| b == b'\n')
            .count()
            + 1
    }

    pub(crate) fn fresh(&self, name: &str) -> String {
        fresh_name(name)
    }

    pub(crate) fn push_scope(&self, frame: ScopeFrame) {
        self.scope_stack.borrow_mut().push(frame);
    }

    pub(crate) fn pop_scope(&self) {
        self.scope_stack.borrow_mut().pop();
    }

    pub(crate) fn current_scope(&self) -> ScopeFrame {
        self.scope_stack
            .borrow()
            .last()
            .cloned()
            .unwrap_or_else(ScopeFrame::module)
    }

    pub(crate) fn push_value_forwarding_locals(&self, names: HashSet<String>) {
        self.value_forwarding_local_stack.borrow_mut().push(names);
    }

    pub(crate) fn pop_value_forwarding_locals(&self) {
        self.value_forwarding_local_stack.borrow_mut().pop();
    }

    pub(crate) fn current_value_forwarding_locals(&self) -> HashSet<String> {
        self.value_forwarding_local_stack
            .borrow()
            .last()
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn push_no_raise_locals(&self, names: HashSet<String>) {
        self.no_raise_local_stack.borrow_mut().push(names);
    }

    pub(crate) fn pop_no_raise_locals(&self) {
        self.no_raise_local_stack.borrow_mut().pop();
    }

    pub(crate) fn current_no_raise_locals(&self) -> HashSet<String> {
        self.no_raise_local_stack
            .borrow()
            .last()
            .cloned()
            .unwrap_or_default()
    }
}
