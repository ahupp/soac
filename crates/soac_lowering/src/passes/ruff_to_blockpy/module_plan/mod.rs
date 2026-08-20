use crate::block_py::{
    AnnotationProviderKind, ApplyClassDecorator, ApplyFunctionDescriptor, BindingKind,
    BlockPyFunction, BlockPyModule, CallArgPositional, CallableScopeInfo, CallableSourceRole,
    CellBindingKind, CellRefForName, CheckAnnotationFormat, ClassConstructionScope,
    CompleteFunctionDefinition, ConstructTypeParameterScope, CreateTypeAlias, CreateTypeParameter,
    DiscardClassConstructionCaptures, FunctionKind, FunctionNameGen, HasMeta,
    InstrWithAwaitAndYield, LexicalCaptureProjection, LexicalCellCapture, Literal, Load,
    MakeFunction, MapFunction, MapInstr, Mappable, ModuleNameGen, ModuleShape, NameLike,
    NewAnnotationSet, NumberLiteralValue, PrepareClassDecorator, PrivateLexicalScope,
    RecordAnnotation, SetFunctionTypeParameters, SetTypeParameterDefault, SetupAnnotations,
    SubscriptGeneric, TypeParameterKind, UnresolvedName, WithMeta,
};
use crate::passes::ast_to_ast::body::{split_docstring, Suite};
use crate::passes::ast_to_ast::context::{AnnotationOperation, ClassDecoratorOperation, Context};
use crate::passes::ast_to_ast::rewrite_class_def::make_type_param_info;
use crate::passes::ast_to_ast::rewrite_stmt;
use crate::passes::ast_to_ast::semantic::{SemanticAstState, SemanticScope};
use crate::passes::ruff_to_blockpy::param_specs::{
    collect_param_spec_and_defaults, param_defaults_to_expr,
};
use crate::passes::CoreModuleShapeWithAwaitAndYield;
use crate::template::{py_expr, py_stmt, py_stmt_typed};
use crate::transformer::{walk_expr, walk_stmt, Transformer};
use ruff_python_ast::{self as ast, Expr, Stmt};

use super::build_core_blockpy_callable_def_from_runtime_input;
mod callable_scope;
mod class_bindings;
use callable_scope::callable_scope_info;

struct FunctionScopeFrame {
    scope: Option<SemanticScope>,
    callable_scope: CallableScopeInfo,
    public_scope: Option<CallableScopeInfo>,
    hoisted_to_parent: Vec<Stmt>,
}

fn merge_lexical_capture(captures: &mut Vec<LexicalCellCapture>, incoming: &LexicalCellCapture) {
    if let Some(existing) = captures
        .iter_mut()
        .find(|capture| capture.binding == incoming.binding)
    {
        existing
            .nominal_binding_indices
            .extend(&incoming.nominal_binding_indices);
        existing.nominal_binding_indices.sort_unstable();
        existing.nominal_binding_indices.dedup();
    } else {
        captures.push(incoming.clone());
        captures.sort_by(|left, right| left.binding.cmp(&right.binding));
    }
}

struct PendingAnnotationHelper {
    target: (ruff_text_size::TextRange, String),
    make_function_expr: Expr,
}

struct PendingTypeParameterScope {
    target: (ruff_text_size::TextRange, soac_contracts::DefinitionKind),
    make_function_expr: Expr,
}

struct BlockPyModuleRewriter<'a, P: ModuleShape> {
    context: &'a Context,
    semantic_state: SemanticAstState,
    module_name_gen: ModuleNameGen,
    function_scope_stack: Vec<FunctionScopeFrame>,
    callable_defs: Vec<BlockPyFunction<P>>,
    pending_annotation_helpers: Vec<PendingAnnotationHelper>,
    pending_type_parameter_scopes: Vec<PendingTypeParameterScope>,
    lower_function_to_blockpy: fn(
        &Context,
        &ast::StmtFunctionDef,
        &CallableScopeInfo,
        FunctionNameGen,
    ) -> BlockPyFunction<P>,
}

#[derive(Default)]
struct YieldFamilyDetector {
    found: bool,
}

pub(crate) fn rewrite_ast_to_core_blockpy_module_plan_with_module(
    context: &Context,
    mut module: Suite,
    semantic_state: &SemanticAstState,
    module_name_gen: ModuleNameGen,
) -> BlockPyModule<CoreModuleShapeWithAwaitAndYield> {
    crate::passes::ast_to_ast::simplify::flatten(&mut module);
    let mut rewriter = BlockPyModuleRewriter {
        context,
        semantic_state: semantic_state.clone(),
        module_name_gen,
        function_scope_stack: Vec::new(),
        callable_defs: Vec::new(),
        pending_annotation_helpers: Vec::new(),
        pending_type_parameter_scopes: Vec::new(),
        lower_function_to_blockpy: try_lower_function_to_core_blockpy_bundle,
    };
    let module_init =
        BlockPyModuleRewriter::<CoreModuleShapeWithAwaitAndYield>::root_module_init_stmt(
            &mut module,
        );
    rewriter.lower_root_function_def(module_init);
    assert!(
        rewriter.pending_type_parameter_scopes.is_empty(),
        "unconsumed type-parameter scope"
    );
    BlockPyModule {
        module_name_gen: rewriter.module_name_gen,
        strict_source: context.strict_source(),
        global_names: Vec::new(),
        callable_defs: rewriter.callable_defs,
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    }
}

impl Transformer for YieldFamilyDetector {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            other => walk_stmt(self, other),
        }
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Yield(_) | Expr::YieldFrom(_) => {
                self.found = true;
            }
            Expr::Lambda(_)
            | Expr::Generator(_)
            | Expr::ListComp(_)
            | Expr::SetComp(_)
            | Expr::DictComp(_) => {}
            other => walk_expr(self, other),
        }
    }
}

fn function_kind(func: &ast::StmtFunctionDef) -> FunctionKind {
    let mut detector = YieldFamilyDetector::default();
    let mut body = func.body.clone();
    detector.visit_body(&mut body);
    match (func.is_async, detector.found) {
        (false, false) => FunctionKind::Function,
        (false, true) => FunctionKind::Generator,
        (true, false) => FunctionKind::Coroutine,
        (true, true) => FunctionKind::AsyncGenerator,
    }
}

fn try_lower_function_to_core_blockpy_bundle(
    context: &Context,
    func: &ast::StmtFunctionDef,
    callable_scope: &CallableScopeInfo,
    name_gen: FunctionNameGen,
) -> BlockPyFunction<CoreModuleShapeWithAwaitAndYield> {
    let (docstring, lowered_input_body) = split_docstring(&func.body);
    let lowered_input_body = class_bindings::lower_native_class_body(
        context,
        callable_scope,
        &lowered_input_body,
        &name_gen,
    );
    let (param_spec, _param_defaults) = collect_param_spec_and_defaults(&func.parameters);

    let end_label = name_gen.next_block_name();

    let mut function = build_core_blockpy_callable_def_from_runtime_input(
        context,
        name_gen,
        callable_scope.names.clone(),
        param_spec,
        &lowered_input_body,
        docstring,
        end_label,
        function_kind(func),
        callable_scope,
    );
    if let Some(provider) = &callable_scope.annotation_provider {
        assert_eq!(function.params.params.len(), 1);
        assert_eq!(
            function.params.params[0].name,
            provider.body_format_parameter
        );
        function.body_params = Some(function.params.clone());
        function.params.params[0].name = provider.kind.parameter_name().into();
        if provider.kind != AnnotationProviderKind::Dictionary {
            context.record_type_expression_function(
                &callable_scope
                    .source_origin
                    .as_ref()
                    .expect("evaluator source origin")
                    .definition,
                provider.kind,
                function.function_id,
            );
        }
    }
    if let Some(scope) = &callable_scope.type_parameter_scope {
        assert_eq!(function.params.params.len(), scope.inputs.len());
        function.body_params = Some(function.params.clone());
        for (parameter, input) in function.params.params.iter_mut().zip(&scope.inputs) {
            assert_eq!(parameter.name, input.body_parameter);
            parameter.name = input.kind.native_parameter_name().into();
        }
        context.record_type_parameter_scope_function(
            &callable_scope
                .source_origin
                .as_ref()
                .expect("type scope source origin")
                .definition,
            function.function_id,
        );
    }
    if let Some(origin) = callable_scope
        .source_origin
        .as_ref()
        .filter(|origin| origin.role == CallableSourceRole::SourceFunction)
    {
        context.record_source_function(&origin.definition, function.function_id);
    }
    if let Some(origin) = callable_scope
        .source_origin
        .as_ref()
        .filter(|origin| origin.role == CallableSourceRole::ClassConstruction)
    {
        context.record_class_construction_function(&origin.definition, function.function_id);
    }
    if let Some(origin) = callable_scope
        .source_origin
        .as_ref()
        .filter(|origin| origin.role == CallableSourceRole::ClassNamespace)
    {
        context.record_class_namespace_function(&origin.definition, function.function_id);
    }
    ResolveFunctionConstructions(context).map_fn(function)
}

struct ResolveFunctionConstructions<'a>(&'a Context);

fn function_kind_name(kind: FunctionKind) -> &'static str {
    match kind {
        FunctionKind::Function => "function",
        FunctionKind::Coroutine => "coroutine",
        FunctionKind::Generator => "generator",
        FunctionKind::AsyncGenerator => "async_generator",
    }
}

impl MapInstr<InstrWithAwaitAndYield, InstrWithAwaitAndYield> for ResolveFunctionConstructions<'_> {
    fn map_instr(&mut self, instr: InstrWithAwaitAndYield) -> InstrWithAwaitAndYield {
        let instr = instr.map_same_children(self);
        let InstrWithAwaitAndYield::Call(call) = &instr else {
            return instr;
        };
        if self
            .0
            .is_class_capture_discard(call.meta().node_index.load())
        {
            let InstrWithAwaitAndYield::Call(call) = instr else {
                unreachable!()
            };
            assert!(call.keywords.is_empty() && call.args.len() == 1);
            let meta = call.meta();
            let CallArgPositional::Positional(function) = call.args.into_iter().next().unwrap()
            else {
                panic!("construction cleanup cannot expand its original helper operand");
            };
            return DiscardClassConstructionCaptures::new(Box::new(function))
                .with_meta(meta)
                .into();
        }
        if let Some(site) = self
            .0
            .function_descriptor_application(call.meta().node_index.load())
        {
            let InstrWithAwaitAndYield::Call(call) = instr else {
                unreachable!()
            };
            assert!(
                call.keywords.is_empty() && call.args.len() == 1,
                "descriptor application must have its one original function operand"
            );
            let meta = call.meta();
            let CallArgPositional::Positional(function) = call.args.into_iter().next().unwrap()
            else {
                panic!("descriptor application cannot expand its function operand");
            };
            assert!(
                matches!(&function, InstrWithAwaitAndYield::MakeFunction(created)
                if created.function_id == site.function_id),
                "descriptor application cannot authenticate an intervening decorator result"
            );
            return ApplyFunctionDescriptor::new(
                site.definition,
                site.function_id,
                call.func,
                Box::new(function),
                call.frame_namespace,
            )
            .with_meta(meta)
            .into();
        }
        if let Some(operation) = self
            .0
            .class_decorator_operation(call.meta().node_index.load())
        {
            let InstrWithAwaitAndYield::Call(call) = instr else {
                unreachable!()
            };
            let meta = call.meta();
            return match operation {
                ClassDecoratorOperation::Prepare {
                    declaration,
                    factory,
                } => {
                    let definition = self.0.type_expression_definition(
                        declaration,
                        soac_contracts::DefinitionKind::Class,
                    );
                    let construction_function = self.0.class_construction_function(&definition);
                    let (decorator, args, keywords) = if factory {
                        (call.func, call.args, call.keywords)
                    } else {
                        assert!(
                            call.keywords.is_empty() && call.args.len() == 1,
                            "bare decorator preparation changed its operand shape"
                        );
                        let CallArgPositional::Positional(decorator) =
                            call.args.into_iter().next().unwrap()
                        else {
                            panic!("bare decorator preparation cannot expand its operand")
                        };
                        (Box::new(decorator), Vec::new(), Vec::new())
                    };
                    PrepareClassDecorator::new(
                        definition,
                        construction_function,
                        decorator,
                        args,
                        keywords,
                        factory,
                        call.frame_namespace,
                    )
                    .with_meta(meta)
                    .into()
                }
                ClassDecoratorOperation::Apply { declaration } => {
                    assert!(
                        call.keywords.is_empty() && call.args.len() == 2,
                        "class decorator application changed its operand shape"
                    );
                    let mut arguments = call.args.into_iter().map(|argument| match argument {
                        CallArgPositional::Positional(value) => value,
                        CallArgPositional::Starred(_) => {
                            panic!("class decorator application cannot expand operands")
                        }
                    });
                    let definition = self.0.type_expression_definition(
                        declaration,
                        soac_contracts::DefinitionKind::Class,
                    );
                    let construction_function = self.0.class_construction_function(&definition);
                    ApplyClassDecorator::new(
                        definition,
                        construction_function,
                        Box::new(arguments.next().unwrap()),
                        Box::new(arguments.next().unwrap()),
                        call.frame_namespace,
                    )
                    .with_meta(meta)
                    .into()
                }
                ClassDecoratorOperation::DiscardPreparation => {
                    assert!(
                        call.keywords.is_empty() && call.args.len() == 1,
                        "class decorator discard changed its operand shape"
                    );
                    let CallArgPositional::Positional(preparation) =
                        call.args.into_iter().next().unwrap()
                    else {
                        panic!("class decorator discard cannot expand its operand")
                    };
                    crate::block_py::DiscardClassDecorator::new(Box::new(preparation))
                        .with_meta(meta)
                        .into()
                }
                ClassDecoratorOperation::DeleteBinding => {
                    assert!(
                        call.keywords.is_empty() && call.args.len() == 1,
                        "class decorator cleanup changed its operand shape"
                    );
                    let CallArgPositional::Positional(InstrWithAwaitAndYield::Load(load)) =
                        call.args.into_iter().next().unwrap()
                    else {
                        panic!("class decorator cleanup requires its recorded binding")
                    };
                    crate::block_py::Del::new(load.name, true)
                        .with_meta(meta)
                        .into()
                }
            };
        }
        if let Some(site) = self.0.function_completion(call.meta().node_index.load()) {
            let InstrWithAwaitAndYield::Call(call) = instr else {
                unreachable!()
            };
            assert!(
                call.keywords.is_empty() && call.args.len() == 1,
                "registered function completion changed operand shape"
            );
            let meta = call.meta();
            let CallArgPositional::Positional(value) = call.args.into_iter().next().unwrap() else {
                panic!("function completion cannot expand its operand")
            };
            return CompleteFunctionDefinition::new(
                site.definition,
                site.function_id,
                Box::new(value),
            )
            .with_meta(meta)
            .into();
        }
        if let Some(operation) = self.0.annotation_operation(call.meta().node_index.load()) {
            let InstrWithAwaitAndYield::Call(call) = instr else {
                unreachable!()
            };
            assert!(
                call.keywords.is_empty(),
                "annotation operation has keyword operands"
            );
            let meta = call.meta();
            let mut arguments = call.args.into_iter().map(|argument| match argument {
                CallArgPositional::Positional(value) => value,
                CallArgPositional::Starred(_) => panic!("annotation operands cannot be expanded"),
            });
            let result = match operation {
                AnnotationOperation::NewSet => NewAnnotationSet::new().with_meta(meta).into(),
                AnnotationOperation::Setup => SetupAnnotations::new(None).with_meta(meta).into(),
                AnnotationOperation::Record { index } => RecordAnnotation::new(
                    arguments
                        .next()
                        .expect("annotation record has no set operand"),
                    index,
                )
                .with_meta(meta)
                .into(),
                AnnotationOperation::CheckFormat => CheckAnnotationFormat::new(
                    arguments
                        .next()
                        .expect("annotation format check has no format operand"),
                )
                .with_meta(meta)
                .into(),
                AnnotationOperation::CreateAlias { declaration } => CreateTypeAlias::new(
                    self.0.type_expression_definition(
                        declaration,
                        soac_contracts::DefinitionKind::TypeAlias,
                    ),
                    self.0.type_expression_function(
                        declaration,
                        AnnotationProviderKind::TypeAliasValue,
                    ),
                    Box::new(arguments.next().expect("type alias name")),
                    Box::new(arguments.next().expect("type alias parameters")),
                    Box::new(arguments.next().expect("type alias evaluator")),
                )
                .with_meta(meta)
                .into(),
                AnnotationOperation::CreateParameter { declaration, kind } => {
                    let evaluator_kind = match kind {
                        TypeParameterKind::TypeVarBound => {
                            Some(AnnotationProviderKind::TypeParameterBound)
                        }
                        TypeParameterKind::TypeVarConstraints => {
                            Some(AnnotationProviderKind::TypeParameterConstraints)
                        }
                        _ => None,
                    };
                    let name = Box::new(arguments.next().expect("type parameter name"));
                    CreateTypeParameter::new(
                        self.0.type_expression_definition(
                            declaration,
                            soac_contracts::DefinitionKind::Parameter,
                        ),
                        kind,
                        name,
                        evaluator_kind
                            .map(|kind| self.0.type_expression_function(declaration, kind)),
                        evaluator_kind
                            .map(|_| Box::new(arguments.next().expect("type parameter evaluator"))),
                    )
                    .with_meta(meta)
                    .into()
                }
                AnnotationOperation::SetParameterDefault { declaration } => {
                    SetTypeParameterDefault::new(
                        self.0.type_expression_definition(
                            declaration,
                            soac_contracts::DefinitionKind::Parameter,
                        ),
                        self.0.type_expression_function(
                            declaration,
                            AnnotationProviderKind::TypeParameterDefault,
                        ),
                        Box::new(arguments.next().expect("default target parameter")),
                        Box::new(arguments.next().expect("default evaluator")),
                    )
                    .with_meta(meta)
                    .into()
                }
                AnnotationOperation::ConstructTypeParameterScope {
                    declaration,
                    kind,
                    positional_defaults,
                    keyword_defaults,
                    complete_function,
                } => {
                    let definition = self.0.type_expression_definition(declaration, kind);
                    let value = ConstructTypeParameterScope::new(
                        definition.clone(),
                        self.0.type_parameter_scope_function(declaration, kind),
                        positional_defaults.then(|| {
                            Box::new(arguments.next().expect("positional defaults container"))
                        }),
                        keyword_defaults.then(|| {
                            Box::new(arguments.next().expect("keyword defaults container"))
                        }),
                        Box::new(
                            arguments
                                .next()
                                .expect("actual type-parameter scope function"),
                        ),
                    )
                    .with_meta(meta.clone())
                    .into();
                    if complete_function {
                        assert_eq!(kind, soac_contracts::DefinitionKind::Function);
                        CompleteFunctionDefinition::new(
                            definition.clone(),
                            self.0.source_function(&definition),
                            Box::new(value),
                        )
                        .with_meta(meta)
                        .into()
                    } else {
                        value
                    }
                }
                AnnotationOperation::SubscriptGeneric { declaration } => SubscriptGeneric::new(
                    self.0.type_expression_definition(
                        declaration,
                        soac_contracts::DefinitionKind::Class,
                    ),
                    Box::new(arguments.next().expect("native Generic parameters")),
                )
                .with_meta(meta)
                .into(),
                AnnotationOperation::SetFunctionTypeParameters { declaration } => {
                    let definition = self.0.type_expression_definition(
                        declaration,
                        soac_contracts::DefinitionKind::Function,
                    );
                    SetFunctionTypeParameters::new(
                        definition.clone(),
                        self.0.source_function(&definition),
                        Box::new(arguments.next().expect("generic source function")),
                        Box::new(arguments.next().expect("source function type parameters")),
                    )
                    .with_meta(meta)
                    .into()
                }
            };
            assert!(
                arguments.next().is_none(),
                "annotation operation has extra operands"
            );
            return result;
        }
        let Some(site) = self.0.function_construction(call.meta().node_index.load()) else {
            return instr;
        };
        assert!(
            matches!(call.func.as_ref(), InstrWithAwaitAndYield::Load(load)
                if load.name.is_runtime_symbol("make_function"))
                && call.keywords.is_empty()
                && call.args.len() == 5,
            "registered function-construction site changed shape"
        );
        let InstrWithAwaitAndYield::Call(call) = instr else {
            unreachable!()
        };
        let meta = call.meta();
        let mut args = call.args.into_iter().map(|arg| match arg {
            CallArgPositional::Positional(value) => value,
            CallArgPositional::Starred(_) => panic!("construction operands cannot be expanded"),
        });
        assert!(
            matches!(args.next().unwrap(), InstrWithAwaitAndYield::Literal(value)
            if matches!(value.as_literal(), Literal::NumberLiteral(value)
                if matches!(&value.value, NumberLiteralValue::Int(value)
                    if value.to_string() == site.function_id.to_packed_runtime_u64().to_string())))
        );
        assert!(
            matches!(args.next().unwrap(), InstrWithAwaitAndYield::Literal(value)
            if matches!(value.as_literal(), Literal::StringLiteral(value)
                if value.value == function_kind_name(site.kind)))
        );
        assert!(
            matches!(args.next().unwrap(), InstrWithAwaitAndYield::Tuple(value)
            if value.values.is_empty()),
            "closure operands are resolved by name binding"
        );
        MakeFunction::new(
            site.function_id,
            site.kind,
            Box::new(args.next().unwrap()),
            Box::new(args.next().unwrap()),
            site.class_namespace_binding.map(|name| {
                Box::new(
                    Load::new(UnresolvedName::SourceName(name.into()))
                        .with_meta(meta.clone())
                        .into(),
                )
            }),
            site.creation_cells
                .into_iter()
                .map(|binding| {
                    CellRefForName::new(binding.name, Some(binding.scope))
                        .with_meta(meta.clone())
                        .into()
                })
                .collect::<Vec<InstrWithAwaitAndYield>>(),
        )
        .with_meta(meta)
        .into()
    }

    fn map_name(&mut self, name: UnresolvedName) -> UnresolvedName {
        name
    }
}

fn build_lowered_function_instantiation_expr(
    context: &Context,
    semantic_state: &SemanticAstState,
    function_id: crate::block_py::RuntimeFunctionId,
    decorator_exprs: Vec<Expr>,
    param_defaults_expr: Expr,
    annotate_fn_expr: Expr,
    kind: FunctionKind,
    descriptor_definition: Option<&soac_contracts::SourceIdentity>,
    scope: &CallableScopeInfo,
) -> Expr {
    let kind_name = function_kind_name(kind);
    let base_function_expr = py_expr!(
        "__soac__.make_function({function_id:literal}, {kind:literal}, {closure:expr}, {param_defaults:expr}, {annotate_fn:expr})",
        function_id = function_id.to_packed_runtime_u64(),
        kind = kind_name,
        closure = py_expr!("()"),
        param_defaults = param_defaults_expr.clone(),
        annotate_fn = annotate_fn_expr.clone(),
    );
    let node = semantic_state.assign_generated_node_index(&base_function_expr);
    context.record_function_construction(
        node,
        function_id,
        kind,
        scope.class_construction.as_ref().map(|_| {
            context.class_namespace_binding(
                &scope
                    .source_origin
                    .as_ref()
                    .expect("construction source")
                    .definition,
            )
        }),
        if let Some(construction) = &scope.class_construction {
            construction
                .captures
                .iter()
                .map(|slot| slot.binding.clone())
                .collect()
        } else if scope
            .source_origin
            .as_ref()
            .is_some_and(|origin| origin.role == CallableSourceRole::SourceFunction)
        {
            scope
                .private_lexical
                .as_ref()
                .map_or_else(Vec::new, |scope| {
                    scope
                        .private_captures()
                        .map(|slot| slot.binding.clone())
                        .collect()
                })
        } else {
            // Namespace-only edges belong to the exact construction handle,
            // never to an escaped helper function's persistent metadata.
            Vec::new()
        },
    );
    let descriptor_definition = descriptor_definition
        .filter(|definition| context.function_descriptor_proposal(definition, &decorator_exprs));
    let expression = rewrite_stmt::decorator::rewrite_exprs(decorator_exprs, base_function_expr);
    if let Some(definition) = descriptor_definition {
        let node = semantic_state.assign_generated_node_index(&expression);
        context.record_function_descriptor_application(node, function_id, definition.clone());
    }
    expression
}

fn complete_function_definition_expr(
    context: &Context,
    semantic_state: &SemanticAstState,
    function_id: crate::block_py::RuntimeFunctionId,
    definition: &soac_contracts::SourceIdentity,
    value: Expr,
) -> Expr {
    // This temporary syntax is erased from an explicit semantic site before
    // name binding. User spelling or a literal function ID cannot create it.
    let expression = py_expr!(
        "_dp_complete_function_definition({value:expr})",
        value = value
    );
    let node = semantic_state.assign_generated_node_index(&expression);
    context.record_function_completion(node, function_id, definition.clone());
    expression
}

#[allow(clippy::too_many_arguments)]
fn rewrite_function_def_stmt_via_blockpy_with_pass<P: ModuleShape>(
    context: &Context,
    semantic_state: &SemanticAstState,
    parent_hoisted: &mut Vec<Stmt>,
    func: &mut ast::StmtFunctionDef,
    callable_scope: &CallableScopeInfo,
    public_scope: Option<&CallableScopeInfo>,
    function_hoisted: Vec<Stmt>,
    module_name_gen: &mut ModuleNameGen,
    callable_defs: &mut Vec<BlockPyFunction<P>>,
    annotate_fn_expr: Expr,
    lower_function_to_blockpy: fn(
        &Context,
        &ast::StmtFunctionDef,
        &CallableScopeInfo,
        FunctionNameGen,
    ) -> BlockPyFunction<P>,
) -> Vec<Stmt> {
    let name_gen = module_name_gen.next_function_name_gen();
    let mut lowered_plan = lower_function_to_blockpy(context, func, callable_scope, name_gen);
    lowered_plan.public_scope = public_scope.cloned();
    let bind_name = lowered_plan.names.bind_name.clone();
    let generic = context.generic_function(func);
    let completion_definition = lowered_plan
        .scope
        .source_origin
        .as_ref()
        .filter(|origin| {
            context.strict_source().is_some()
                && origin.role == CallableSourceRole::SourceFunction
                && func.decorator_list.is_empty()
                && generic.is_none()
        })
        .map(|origin| origin.definition.clone());
    let (_, param_defaults) = collect_param_spec_and_defaults(&func.parameters);
    let defaults = generic.as_ref().map_or_else(
        || param_defaults_to_expr(&param_defaults),
        |generic| {
            let positional = generic.positional_defaults.as_ref().map_or_else(
                || py_expr!("None"),
                |name| py_expr!("{name:id}", name = name.as_str()),
            );
            let keyword = generic.keyword_defaults.as_ref().map_or_else(
                || py_expr!("None"),
                |name| py_expr!("{name:id}", name = name.as_str()),
            );
            py_expr!(
                "({positional:expr}, {keyword:expr})",
                positional = positional,
                keyword = keyword
            )
        },
    );
    let mut decorated = build_lowered_function_instantiation_expr(
        context,
        semantic_state,
        lowered_plan.function_id,
        rewrite_stmt::decorator::collect_exprs(&func.decorator_list),
        defaults,
        annotate_fn_expr,
        lowered_plan.kind,
        lowered_plan
            .scope
            .source_origin
            .as_ref()
            .filter(|origin| origin.role == CallableSourceRole::SourceFunction && generic.is_none())
            .map(|origin| &origin.definition),
        &lowered_plan.scope,
    );
    if let Some(generic) = &generic {
        decorated = py_expr!(
            "_dp_set_function_type_parameters({function:expr}, {parameters:id})",
            function = decorated,
            parameters = generic.type_parameters.as_str()
        );
        let node = semantic_state.assign_generated_node_index(&decorated);
        context.record_annotation_operation_node(
            node,
            AnnotationOperation::SetFunctionTypeParameters {
                declaration: func.range,
            },
        );
    }
    let mut instantiation_stmts = Vec::new();
    let type_param_info = func
        .type_params
        .as_ref()
        .map(|type_params| make_type_param_info((**type_params).clone()));
    if let Some(type_param_info) = &type_param_info {
        instantiation_stmts.extend(type_param_info.bindings.clone());
    }
    // Generic function metadata must finish before the source name is bound.
    // The private temporary exists only for this undecorated definition, never
    // across an arbitrary decorator that may discard the original function.
    let completion_temporary = (completion_definition.is_some() && type_param_info.is_some())
        .then(|| lowered_plan.name_gen.next_tmp_name("function_definition"));
    let metadata_name = completion_temporary
        .as_ref()
        .map_or(bind_name.as_str(), |name| name.as_str());
    let decorated = match (&completion_definition, &completion_temporary) {
        (Some(definition), None) => complete_function_definition_expr(
            context,
            semantic_state,
            lowered_plan.function_id,
            definition,
            decorated,
        ),
        _ => decorated,
    };
    instantiation_stmts.push(py_stmt!(
        "{name:id} = {value:expr}",
        name = metadata_name,
        value = decorated
    ));
    let mut metadata_stmts = Vec::new();
    if let Some(type_param_info) = type_param_info {
        if let Some(type_params_tuple) = type_param_info.type_params_tuple {
            metadata_stmts.push(py_stmt!(
                "{name:id}.__type_params__ = {value:expr}",
                name = metadata_name,
                value = type_params_tuple
            ));
        }
        for type_param_name in type_param_info.param_names {
            metadata_stmts.push(py_stmt!("del {name:id}", name = type_param_name.as_str()));
        }
    }
    if let Some(temporary) = completion_temporary {
        let value = complete_function_definition_expr(
            context,
            semantic_state,
            lowered_plan.function_id,
            completion_definition
                .as_ref()
                .expect("completion temporary has a source definition"),
            py_expr!("{name:id}", name = temporary.as_str()),
        );
        metadata_stmts.push(py_stmt!(
            "{name:id} = {value:expr}",
            name = bind_name.as_str(),
            value = value
        ));
        let mut cleanup: ast::StmtTry = py_stmt_typed!(
            "try:\n    pass\nfinally:\n    del {name:id}",
            name = temporary.as_str()
        );
        cleanup.body = metadata_stmts.into();
        instantiation_stmts.push(Stmt::Try(cleanup));
    } else {
        instantiation_stmts.extend(metadata_stmts);
    }
    callable_defs.push(lowered_plan);
    if bind_name.starts_with("_dp_class_ns_") || bind_name.starts_with("_dp_define_class_") {
        let mut replacement = function_hoisted;
        replacement.extend(instantiation_stmts);
        replacement
    } else {
        parent_hoisted.extend(function_hoisted);
        instantiation_stmts
    }
}

impl<P: ModuleShape> BlockPyModuleRewriter<'_, P> {
    fn visit_function_definition_exprs(&mut self, func: &mut ast::StmtFunctionDef) {
        for decorator in &mut func.decorator_list {
            self.visit_expr(&mut decorator.expression);
        }
        for param in &mut func.parameters.posonlyargs {
            if let Some(default) = &mut param.default {
                self.visit_expr(default);
            }
        }
        for param in &mut func.parameters.args {
            if let Some(default) = &mut param.default {
                self.visit_expr(default);
            }
        }
        for param in &mut func.parameters.kwonlyargs {
            if let Some(default) = &mut param.default {
                self.visit_expr(default);
            }
        }
    }

    fn lower_lambda_expr(&mut self, lambda: &mut ast::ExprLambda) -> Expr {
        let lambda_scope = self
            .semantic_state
            .lambda_scope(lambda)
            .expect("missing preserved lambda scope while lowering lambda");
        let func_name = self.context.fresh("lambda");
        let mut func_def: ast::StmtFunctionDef = py_stmt_typed!(
            r#"
def {func:id}():
    pass
"#,
            func = func_name.as_str(),
        );
        self.context.record_lambda_origin(lambda, &mut func_def);
        if let Some(parameters) = lambda.parameters.take() {
            func_def.parameters = parameters;
        }
        func_def.body = match self.semantic_state.lowered_lambda_body(lambda) {
            Some(mut body) => {
                crate::passes::ast_to_ast::simplify::flatten(&mut body.statements);
                body.statements
            }
            None => {
                let body = std::mem::replace(&mut *lambda.body, py_expr!("None"));
                [py_stmt!("return {value:expr}", value = body)].into()
            }
        };

        // Defaults execute in the enclosing frame, just like a source def.
        // Lower their callable expressions before entering the lambda scope.
        self.visit_function_definition_exprs(&mut func_def);
        let state = self.walk_function_def_with_explicit_scope(&mut func_def, Some(lambda_scope));
        if let Some(parent_frame) = self
            .function_scope_stack
            .last_mut()
            .filter(|frame| frame.callable_scope.class_bindings.is_none())
        {
            for (name, binding) in &state.callable_scope.bindings {
                if matches!(binding, BindingKind::Cell(CellBindingKind::Capture))
                    && parent_frame.callable_scope.local_defs.contains(name)
                {
                    parent_frame.callable_scope.insert_binding(
                        name.clone(),
                        BindingKind::Cell(CellBindingKind::Owner),
                        false,
                        None,
                    );
                }
            }
        }

        let lowered_plan = (self.lower_function_to_blockpy)(
            self.context,
            &func_def,
            &state.callable_scope,
            self.module_name_gen.next_function_name_gen(),
        );
        let (_, param_defaults) = collect_param_spec_and_defaults(&func_def.parameters);
        let lowered_expr = build_lowered_function_instantiation_expr(
            self.context,
            &self.semantic_state,
            lowered_plan.function_id,
            Vec::new(),
            param_defaults_to_expr(&param_defaults),
            py_expr!("None"),
            lowered_plan.kind,
            None,
            &lowered_plan.scope,
        );
        self.callable_defs.push(lowered_plan);
        lowered_expr
    }

    fn root_module_init_stmt<'a>(module: &'a mut Suite) -> &'a mut ast::StmtFunctionDef {
        assert_eq!(
            module.len(),
            1,
            "expected root suite with exactly one statement",
        );
        let Stmt::FunctionDef(func) = &mut module[0] else {
            panic!("expected root suite with exactly one function");
        };
        assert!(
            func.parameters.posonlyargs.is_empty()
                && func.parameters.args.is_empty()
                && func.parameters.vararg.is_none()
                && func.parameters.kwonlyargs.is_empty()
                && func.parameters.kwarg.is_none(),
            "expected root function with no parameters",
        );
        func
    }

    fn walk_function_def_with_scope(
        &mut self,
        func: &mut ast::StmtFunctionDef,
    ) -> FunctionScopeFrame {
        let function_scope = self.semantic_state.function_scope(func);
        self.walk_function_def_with_explicit_scope(func, function_scope)
    }

    /// Resolve one signed lexical owner through the actual nesting stack. A
    /// native source closure is an existing transport boundary and keeps its
    /// normal pre-seal mutation semantics; otherwise forward private cells.
    fn ensure_lexical_capture(&mut self, capture: &LexicalCellCapture) -> bool {
        let frames = &mut self.function_scope_stack;
        let Some(owner) = frames.iter().position(|frame| {
            frame
                .callable_scope
                .source_origin
                .as_ref()
                .is_some_and(|origin| {
                    origin.role == CallableSourceRole::SourceFunction
                        && origin.definition == capture.binding.scope
                })
        }) else {
            return false;
        };
        if !matches!(
            frames[owner]
                .callable_scope
                .binding_kind(&capture.binding.name),
            Some(BindingKind::Local | BindingKind::Cell(CellBindingKind::Owner))
        ) {
            return false;
        }
        if frames[owner + 1..].iter().any(|frame| {
            !frame
                .callable_scope
                .source_origin
                .as_ref()
                .is_some_and(|origin| {
                    matches!(
                        origin.role,
                        CallableSourceRole::SourceFunction | CallableSourceRole::ClassNamespace
                    )
                })
        }) {
            return false;
        }
        let native_capture = |frame: &FunctionScopeFrame| {
            frame
                .callable_scope
                .source_origin
                .as_ref()
                .is_some_and(|origin| origin.role == CallableSourceRole::SourceFunction)
                && frame
                    .public_scope
                    .as_ref()
                    .unwrap_or(&frame.callable_scope)
                    .binding_kind(&capture.binding.name)
                    == Some(BindingKind::Cell(CellBindingKind::Capture))
        };
        let first = frames
            .iter()
            .enumerate()
            .skip(owner + 1)
            .rfind(|(_, frame)| native_capture(frame))
            .map_or(owner, |(index, _)| index);
        if first == owner
            && frames[owner]
                .callable_scope
                .binding_kind(&capture.binding.name)
                == Some(BindingKind::Local)
        {
            let frame = &mut frames[owner];
            frame
                .public_scope
                .get_or_insert_with(|| frame.callable_scope.clone());
            frame.callable_scope.insert_binding(
                capture.binding.name.clone(),
                BindingKind::Cell(CellBindingKind::Owner),
                false,
                None,
            );
        }
        for index in first.max(owner + 1)..frames.len() {
            let creator = frames[index - 1]
                .callable_scope
                .source_origin
                .clone()
                .expect("lexical creator");
            let native_closure =
                native_capture(&frames[index]).then(|| capture.binding.name.clone());
            let frame = &mut frames[index];
            let scope =
                frame
                    .callable_scope
                    .private_lexical
                    .get_or_insert_with(|| PrivateLexicalScope {
                        creator: creator.clone(),
                        captures: Vec::new(),
                    });
            assert_eq!(scope.creator, creator);
            if let Some(existing) = scope
                .captures
                .iter_mut()
                .find(|existing| existing.cell.binding == capture.binding)
            {
                assert_eq!(existing.native_closure, native_closure);
                existing
                    .cell
                    .nominal_binding_indices
                    .extend(&capture.nominal_binding_indices);
                existing.cell.nominal_binding_indices.sort_unstable();
                existing.cell.nominal_binding_indices.dedup();
            } else {
                scope.captures.push(LexicalCaptureProjection {
                    cell: capture.clone(),
                    native_closure,
                });
                scope
                    .captures
                    .sort_by(|left, right| left.cell.binding.cmp(&right.cell.binding));
            }
        }
        true
    }

    fn walk_function_def_with_explicit_scope(
        &mut self,
        func: &mut ast::StmtFunctionDef,
        function_scope: Option<SemanticScope>,
    ) -> FunctionScopeFrame {
        let parent_scope = self
            .function_scope_stack
            .last()
            .and_then(|frame| frame.scope.as_ref())
            .cloned();
        let mut callable_scope = callable_scope_info(
            &self.semantic_state,
            parent_scope.as_ref(),
            function_scope.as_ref(),
            Some(func),
            &func.body,
        );
        callable_scope.source_origin = self.context.callable_origin(func);
        callable_scope.generator_expression_code = self.context.generator_expression_code(func);
        apply_annotation_scope(self.context, func, &mut callable_scope);
        class_bindings::apply_native_class_scope(self.context, func, &mut callable_scope);
        if let Some(parent) = self.function_scope_stack.last() {
            class_bindings::apply_native_class_captures(
                self.context,
                func,
                &parent.callable_scope,
                &mut callable_scope,
            );
        }
        if let Some(origin) = callable_scope
            .source_origin
            .as_ref()
            .filter(|origin| origin.role == CallableSourceRole::ClassConstruction)
        {
            let namespace_function = self.context.class_namespace_function(&origin.definition);
            let mut captures = self
                .context
                .class_construction_capture_slots(&origin.definition);
            let namespace = self
                .callable_defs
                .iter()
                .find(|function| function.function_id == namespace_function)
                .expect("namespace precedes its constructor");
            if let Some(scope) = &namespace.scope.private_lexical {
                for projection in &scope.captures {
                    assert!(
                        projection.native_closure.is_none(),
                        "namespace transport is handle-owned"
                    );
                    merge_lexical_capture(&mut captures, &projection.cell);
                }
            }
            captures.retain(|capture| self.ensure_lexical_capture(capture));
            if !captures.is_empty() {
                callable_scope.class_construction = Some(ClassConstructionScope {
                    producer: self
                        .function_scope_stack
                        .last()
                        .and_then(|frame| frame.callable_scope.source_origin.clone())
                        .expect("actual lexical producer"),
                    namespace_function,
                    captures,
                });
            }
        }
        self.function_scope_stack.push(FunctionScopeFrame {
            scope: function_scope.clone(),
            callable_scope,
            public_scope: None,
            hoisted_to_parent: Vec::new(),
        });
        self.visit_body(&mut func.body);
        self.function_scope_stack
            .pop()
            .expect("function scope stack should pop after walking function def")
    }

    fn lower_root_function_def(&mut self, func: &mut ast::StmtFunctionDef) {
        let state = self.walk_function_def_with_scope(func);
        assert!(
            state.hoisted_to_parent.is_empty(),
            "root _dp_module_init should not produce hoisted statements"
        );
        let lowered_plan = (self.lower_function_to_blockpy)(
            self.context,
            func,
            &state.callable_scope,
            self.module_name_gen.next_function_name_gen(),
        );
        self.callable_defs.push(lowered_plan);
    }

    fn rewrite_visited_function_def(
        &mut self,
        func: &mut ast::StmtFunctionDef,
        state: FunctionScopeFrame,
    ) -> Vec<Stmt> {
        let annotate_fn_expr = self.consume_pending_annotation_helper(func);
        let parent_frame = self
            .function_scope_stack
            .last_mut()
            .expect("nested function rewrite should always have a parent hoist buffer");
        let parent_hoisted = &mut parent_frame.hoisted_to_parent;
        rewrite_function_def_stmt_via_blockpy_with_pass(
            self.context,
            &self.semantic_state,
            parent_hoisted,
            func,
            &state.callable_scope,
            state.public_scope.as_ref(),
            state.hoisted_to_parent,
            &mut self.module_name_gen,
            &mut self.callable_defs,
            annotate_fn_expr,
            self.lower_function_to_blockpy,
        )
    }

    fn lower_pending_annotation_helper(
        &mut self,
        func: &mut ast::StmtFunctionDef,
        target: (ruff_text_size::TextRange, String),
    ) {
        let state = self.walk_function_def_with_scope(func);
        assert!(
            state.hoisted_to_parent.is_empty(),
            "function annotation helper should not hoist child functions"
        );
        let lowered_plan = (self.lower_function_to_blockpy)(
            self.context,
            func,
            &state.callable_scope,
            self.module_name_gen.next_function_name_gen(),
        );
        let (_, param_defaults) = collect_param_spec_and_defaults(&func.parameters);
        let make_function_expr = build_lowered_function_instantiation_expr(
            self.context,
            &self.semantic_state,
            lowered_plan.function_id,
            Vec::new(),
            param_defaults_to_expr(&param_defaults),
            py_expr!("None"),
            lowered_plan.kind,
            None,
            &lowered_plan.scope,
        );
        self.callable_defs.push(lowered_plan);
        self.pending_annotation_helpers
            .push(PendingAnnotationHelper {
                target,
                make_function_expr,
            });
    }

    fn lower_pending_type_parameter_scope(
        &mut self,
        func: &mut ast::StmtFunctionDef,
        definition: &soac_contracts::SourceIdentity,
    ) {
        let state = self.walk_function_def_with_scope(func);
        assert!(
            state.hoisted_to_parent.is_empty(),
            "type-parameter scope cannot hoist its body"
        );
        let lowered = (self.lower_function_to_blockpy)(
            self.context,
            func,
            &state.callable_scope,
            self.module_name_gen.next_function_name_gen(),
        );
        let make_function_expr = build_lowered_function_instantiation_expr(
            self.context,
            &self.semantic_state,
            lowered.function_id,
            Vec::new(),
            param_defaults_to_expr(&[]),
            py_expr!("None"),
            lowered.kind,
            None,
            &lowered.scope,
        );
        self.callable_defs.push(lowered);
        self.pending_type_parameter_scopes
            .push(PendingTypeParameterScope {
                target: (func.range, definition.definition_kind),
                make_function_expr,
            });
    }

    fn consume_pending_annotation_helper(&mut self, func: &ast::StmtFunctionDef) -> Expr {
        let target = (func.range, func.name.to_string());
        if let Some(index) = self
            .pending_annotation_helpers
            .iter()
            .rposition(|pending| pending.target == target)
        {
            return self
                .pending_annotation_helpers
                .remove(index)
                .make_function_expr;
        }
        py_expr!("None")
    }
}

fn apply_annotation_scope(
    context: &Context,
    func: &ast::StmtFunctionDef,
    scope: &mut CallableScopeInfo,
) {
    use crate::block_py::{
        AnnotationProviderScope, CallableSourceRole, ClassBodyFallback, EffectiveBinding,
        FunctionDefaultsProjection, TypeParameterScope,
    };
    fn class_capture(
        scope: &mut CallableScopeInfo,
        capture: &crate::passes::ast_to_ast::context::AnnotationClassCapture,
    ) -> String {
        let logical = "__classdict__".to_owned();
        scope.insert_binding_with_cell_names(
            &logical,
            BindingKind::Cell(CellBindingKind::Capture),
            true,
            Some(logical.clone()),
            Some(capture.source_name.clone()),
        );
        scope
            .cell_capture_projections
            .insert(logical.clone(), capture.projection);
        scope
            .cell_value_aliases
            .insert(capture.body_binding.clone(), logical.clone());
        logical
    }
    fn conditional_capture(
        scope: &mut CallableScopeInfo,
        cell: &crate::passes::ast_to_ast::context::AnnotationConditionalCell,
    ) -> String {
        let logical = "__conditional_annotations__".to_owned();
        scope.bindings.remove(&cell.body_binding);
        scope.insert_binding_with_cell_names(
            &logical,
            BindingKind::Cell(CellBindingKind::Capture),
            true,
            Some(logical.clone()),
            Some(cell.storage_name.clone()),
        );
        scope
            .cell_value_aliases
            .insert(cell.body_binding.clone(), logical.clone());
        logical
    }
    if let Some(origin) = &scope.source_origin {
        if origin.role == CallableSourceRole::SourceFunction {
            let native = context.native_definition(&origin.definition);
            scope.names.display_name = native.name;
            scope.names.qualname = native.qualname;
        }
    }
    if context.generic_function(func).is_some() {
        scope.creation_defaults = FunctionDefaultsProjection::NativeContainers;
    }
    if let Some(generic) = context.generic_class(func.range).filter(|_| {
        scope
            .source_origin
            .as_ref()
            .is_some_and(|origin| origin.role == CallableSourceRole::ClassNamespace)
    }) {
        let logical = ".type_params".to_owned();
        scope.bindings.remove(&generic.type_parameters);
        scope.insert_binding_with_cell_names(
            &logical,
            BindingKind::Cell(CellBindingKind::Capture),
            true,
            Some(logical.clone()),
            Some(logical.clone()),
        );
        scope
            .cell_value_aliases
            .insert(generic.type_parameters, logical);
    }
    if let Some(cell) = context.class_annotation_cell(func.range).filter(|_| {
        scope
            .source_origin
            .as_ref()
            .is_some_and(|origin| origin.role == CallableSourceRole::ClassNamespace)
    }) {
        scope.local_defs.insert(cell.owner_binding.clone());
        scope.insert_binding(
            &cell.owner_binding,
            BindingKind::Cell(CellBindingKind::Owner),
            true,
            Some(cell.storage_name),
        );
        // This is an implicit frame cell, not a key in the prepared mapping.
        scope.effective_load_bindings.insert(
            cell.owner_binding.clone(),
            EffectiveBinding::Cell(CellBindingKind::Owner),
        );
        scope.effective_store_bindings.insert(
            cell.owner_binding,
            EffectiveBinding::Cell(CellBindingKind::Owner),
        );
    }
    if let Some(plan) = context.type_parameter_scope(func) {
        let native = context.native_definition(&plan.definition);
        let display_name = format!("<generic parameters of {}>", native.name);
        scope.names.display_name = display_name.clone();
        scope.names.qualname = match native.qualname.rsplit_once('.') {
            Some((prefix, _)) => format!("{prefix}.{display_name}"),
            None => display_name,
        };
        if let Some(tuple) = &plan.owned_parameter_tuple {
            // The native generic-class scope owns this cell. The generated
            // body name is only its value projection, not a distinct lexical
            // cell that a nested namespace must capture through the module.
            let logical = ".type_params".to_owned();
            scope.bindings.remove(tuple);
            scope.local_defs.remove(tuple);
            scope.local_defs.insert(logical.clone());
            scope.insert_binding(
                &logical,
                BindingKind::Cell(CellBindingKind::Owner),
                true,
                Some(logical.clone()),
            );
            scope.cell_value_aliases.insert(tuple.clone(), logical);
        }
        let class_dictionary = plan
            .class_dictionary
            .as_ref()
            .map(|capture| class_capture(scope, capture));
        if let Some(cell) = &plan.conditional_annotations {
            conditional_capture(scope, cell);
        }
        if class_dictionary.is_some() {
            for (name, binding) in &scope.bindings {
                if scope.cell_value_aliases.contains_key(name)
                    || (crate::block_py::is_internal_symbol(name)
                        && !scope.honors_internal_binding(name))
                {
                    continue;
                }
                let fallback = match binding {
                    BindingKind::Global => ClassBodyFallback::Global,
                    BindingKind::Cell(CellBindingKind::Capture) => ClassBodyFallback::Cell,
                    BindingKind::Local | BindingKind::Cell(CellBindingKind::Owner) => continue,
                };
                scope
                    .effective_load_bindings
                    .insert(name.clone(), EffectiveBinding::ClassBody(fallback));
            }
        }
        scope.type_parameter_scope = Some(TypeParameterScope {
            native_qualname: scope.names.qualname.clone(),
            native_range: plan.definition.source_range,
            native_header_range: soac_contracts::SourceRange::new(
                native.header_offset,
                plan.definition.source_range.end,
            ),
            native_first_line: context.line_number_at(native.first_offset as usize) as u32,
            inputs: plan.inputs,
            class_dictionary,
            class_dictionary_binding: plan.class_dictionary.map(|capture| capture.body_binding),
        });
        return;
    }
    let Some(plan) = context.annotation_provider(func) else {
        return;
    };
    let source = &scope
        .source_origin
        .as_ref()
        .expect("annotation source origin")
        .definition;
    if plan.kind == AnnotationProviderKind::Dictionary {
        scope.names.display_name = "__annotate__".into();
    } else {
        scope.names.display_name = context.native_definition(source).name;
    }
    scope.names.qualname = context.native_annotation_qualname(source, plan.kind);
    let native_first_line = if source.definition_kind == soac_contracts::DefinitionKind::Module {
        1
    } else if source.definition_kind == soac_contracts::DefinitionKind::Class
        && plan.kind == AnnotationProviderKind::Dictionary
    {
        // ste_loc identifies the first annotation expression, while the
        // provider's co_firstlineno still comes from the original class.
        let native = context.native_definition(source);
        context.line_number_at(native.first_offset as usize) as u32
    } else {
        let offset = plan.native_range.map_or_else(
            || {
                let native = context.native_definition(source);
                if source.definition_kind == soac_contracts::DefinitionKind::Class {
                    // Class dictionary providers share the class code's first
                    // line, including decorators. Function providers instead
                    // start at their actual def/async header.
                    native.first_offset
                } else {
                    native.header_offset
                }
            },
            |range| range.start,
        );
        context.line_number_at(offset as usize) as u32
    };
    let class_dictionary = plan
        .class_dictionary
        .as_ref()
        .map(|capture| class_capture(scope, capture));
    let conditional_annotations = plan
        .conditional_annotations
        .as_ref()
        .map(|cell| conditional_capture(scope, cell));
    if class_dictionary.is_some() {
        // Native annotation scopes see the actual class dictionary first,
        // then their lexical cells or globals. Locals/private projection
        // bindings remain ordinary function locals.
        for (name, binding) in &scope.bindings {
            if *name == plan.body_format_parameter
                || scope.cell_value_aliases.contains_key(name)
                || (crate::block_py::is_internal_symbol(name)
                    && !scope.honors_internal_binding(name))
            {
                continue;
            }
            let selected = if class_dictionary.as_ref() == Some(name)
                || conditional_annotations.as_ref() == Some(name)
            {
                EffectiveBinding::ClassBody(ClassBodyFallback::Cell)
            } else {
                match binding {
                    BindingKind::Global => EffectiveBinding::ClassBody(ClassBodyFallback::Global),
                    BindingKind::Cell(_) => EffectiveBinding::ClassBody(ClassBodyFallback::Cell),
                    BindingKind::Local => continue,
                }
            };
            scope.effective_load_bindings.insert(name.clone(), selected);
        }
    }
    scope.annotation_provider = Some(AnnotationProviderScope {
        kind: plan.kind,
        native_first_line,
        native_range: plan.native_range,
        body_format_parameter: plan.body_format_parameter,
        class_dictionary,
        class_dictionary_binding: plan.class_dictionary.map(|capture| capture.body_binding),
        conditional_annotations,
    });
}

impl<P: ModuleShape> Transformer for BlockPyModuleRewriter<'_, P> {
    fn visit_body(&mut self, body: &mut Suite) {
        let mut rewritten = Vec::with_capacity(body.len());
        for stmt in std::mem::take(body) {
            let mut stmt = stmt;
            if let Stmt::FunctionDef(func) = &mut stmt {
                if let Some(plan) = self.context.type_parameter_scope(func) {
                    self.lower_pending_type_parameter_scope(func, &plan.definition);
                    continue;
                }
                if let Some(target) = self.context.function_annotation_target(func) {
                    self.lower_pending_annotation_helper(func, target);
                    continue;
                }
                self.visit_function_definition_exprs(func);
                let state = self.walk_function_def_with_scope(func);
                let replacement = self.rewrite_visited_function_def(func, state);
                rewritten.extend(replacement);
                continue;
            }

            self.visit_stmt(&mut stmt);
            rewritten.push(stmt);
        }
        *body = rewritten.into();
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        if let Expr::Call(call) = expr {
            if let Some(AnnotationOperation::ConstructTypeParameterScope {
                declaration,
                kind,
                positional_defaults,
                keyword_defaults,
                ..
            }) = self.context.annotation_operation(call.node_index.load())
            {
                assert!(call.arguments.keywords.is_empty());
                assert_eq!(
                    call.arguments.args.len(),
                    usize::from(positional_defaults) + usize::from(keyword_defaults)
                );
                for argument in &mut call.arguments.args {
                    self.visit_expr(argument);
                }
                let index = self
                    .pending_type_parameter_scopes
                    .iter()
                    .rposition(|pending| pending.target == (declaration, kind))
                    .expect("generic construction must consume its explicitly created scope");
                let pending = self.pending_type_parameter_scopes.remove(index);
                let mut arguments = std::mem::take(&mut call.arguments.args).into_vec();
                arguments.push(pending.make_function_expr);
                call.arguments.args = arguments.into_boxed_slice();
                return;
            }
        }
        match expr {
            Expr::Lambda(lambda) => {
                *expr = self.lower_lambda_expr(lambda);
            }
            other => walk_expr(self, other),
        }
    }
}

#[cfg(test)]
mod test;
