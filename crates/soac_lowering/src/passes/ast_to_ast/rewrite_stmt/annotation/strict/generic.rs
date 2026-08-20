//! Explicit PEP 695 construction scopes. Defaults/decorators remain outside
//! this scope; parameter cells and lazy evaluators are created inside it.

use super::*;
use crate::passes::ast_to_ast::context::{
    GenericClassPlan, GenericFunctionPlan, TypeParameterScopePlan,
};
use crate::passes::ast_to_ast::expr_utils::make_tuple;
use crate::passes::ast_to_ast::rewrite_stmt::decorator;
use crate::template::py_stmt_typed;
use soac_contracts::DefinitionKind;
use soac_core::block_py::{
    TypeParameterKind, TypeParameterScopeInput, TypeParameterScopeInputKind,
};

fn parameter_expressions(parameter: &ast::TypeParam) -> Vec<&Expr> {
    match parameter {
        ast::TypeParam::TypeVar(value) => value
            .bound
            .as_deref()
            .into_iter()
            .chain(value.default.as_deref())
            .collect(),
        ast::TypeParam::ParamSpec(value) => value.default.as_deref().into_iter().collect(),
        ast::TypeParam::TypeVarTuple(value) => value.default.as_deref().into_iter().collect(),
    }
}

fn parameters_use_conditional(parameters: &ast::TypeParams) -> bool {
    parameters
        .type_params
        .iter()
        .flat_map(parameter_expressions)
        .any(uses_conditional_cell)
}

fn function_annotations_use_conditional(function: &ast::StmtFunctionDef) -> bool {
    function
        .parameters
        .posonlyargs
        .iter()
        .chain(&function.parameters.args)
        .chain(&function.parameters.kwonlyargs)
        .filter_map(|parameter| parameter.parameter.annotation.as_deref())
        .chain(
            function
                .parameters
                .vararg
                .as_ref()
                .and_then(|parameter| parameter.annotation.as_deref()),
        )
        .chain(
            function
                .parameters
                .kwarg
                .as_ref()
                .and_then(|parameter| parameter.annotation.as_deref()),
        )
        .chain(function.returns.as_deref())
        .any(uses_conditional_cell)
}

impl<'a> Rewriter<'a> {
    fn parameter_scope(
        &mut self,
        needs_conditional: bool,
    ) -> (
        Self,
        Option<AnnotationClassCapture>,
        Option<AnnotationConditionalCell>,
    ) {
        let class_dictionary = self.class_capture();
        let conditional =
            (needs_conditional && self.class_context.is_some()).then(|| self.conditional_cell());
        let mut inner = Self::new(self.context, Scope::Function);
        inner.class_context = self
            .class_context
            .as_ref()
            .map(|class| AnnotationClassContext {
                declaration: class.declaration,
                source_name: "__classdict__".into(),
                projection: CellCaptureProjection::CellReference,
            });
        inner.conditional_cell = conditional.clone().map(|mut cell| {
            cell.storage_name = "__conditional_annotations__".into();
            cell
        });
        (inner, class_dictionary, conditional)
    }

    fn parameter_statements(&mut self, parameters: ast::TypeParams, tuple: &str) -> Vec<Stmt> {
        let mut result = Vec::new();
        let mut saved = Vec::new();
        for parameter in parameters.type_params {
            let declaration = parameter.range();
            let definition = self
                .context
                .type_expression_definition(declaration, DefinitionKind::Parameter);
            let native_name = self.context.native_definition(&definition).name;
            let (binding, kind, bound, default) = match parameter {
                ast::TypeParam::TypeVar(value) => {
                    let kind = match value.bound.as_deref() {
                        Some(Expr::Tuple(_)) => TypeParameterKind::TypeVarConstraints,
                        Some(_) => TypeParameterKind::TypeVarBound,
                        None => TypeParameterKind::TypeVar,
                    };
                    (value.name.to_string(), kind, value.bound, value.default)
                }
                ast::TypeParam::ParamSpec(value) => (
                    value.name.to_string(),
                    TypeParameterKind::ParamSpec,
                    None,
                    value.default,
                ),
                ast::TypeParam::TypeVarTuple(value) => (
                    value.name.to_string(),
                    TypeParameterKind::TypeVarTuple,
                    None,
                    value.default,
                ),
            };
            let saved_name = self
                .context
                .fresh_annotation_binding("created_type_parameter");
            let mut arguments = vec![py_expr!("{name:literal}", name = native_name.as_str())];
            let bound_name = bound.map(|bound| {
                let evaluator_kind = if kind == TypeParameterKind::TypeVarConstraints {
                    AnnotationProviderKind::TypeParameterConstraints
                } else {
                    AnnotationProviderKind::TypeParameterBound
                };
                let (helper, name) = self.expression_provider(
                    declaration,
                    DefinitionKind::Parameter,
                    evaluator_kind,
                    bound.range(),
                    *bound,
                    false,
                );
                result.push(Stmt::FunctionDef(helper));
                arguments.push(py_expr!("{name:id}", name = name.as_str()));
                name
            });
            let create = operation_args(
                self.context,
                AnnotationOperation::CreateParameter { declaration, kind },
                arguments,
            );
            let assignment = py_stmt!(
                "{name:id} = {value:expr}",
                name = saved_name.as_str(),
                value = create
            );
            if let Some(bound_name) = bound_name {
                result.push(py_stmt!(
                    "try:\n    {assignment:stmt}\nfinally:\n    del {helper:id}",
                    assignment = assignment,
                    helper = bound_name.as_str(),
                ));
            } else {
                result.push(assignment);
            }
            if let Some(default) = default {
                let (helper, name) = self.expression_provider(
                    declaration,
                    DefinitionKind::Parameter,
                    AnnotationProviderKind::TypeParameterDefault,
                    default.range(),
                    *default,
                    kind == TypeParameterKind::TypeVarTuple,
                );
                result.push(Stmt::FunctionDef(helper));
                let attach = operation_args(
                    self.context,
                    AnnotationOperation::SetParameterDefault { declaration },
                    vec![
                        py_expr!("{name:id}", name = saved_name.as_str()),
                        py_expr!("{name:id}", name = name.as_str()),
                    ],
                );
                result.push(py_stmt!(
                    "try:\n    {parameter:id} = {value:expr}\nfinally:\n    del {helper:id}",
                    parameter = saved_name.as_str(),
                    value = attach,
                    helper = name.as_str(),
                ));
            }
            // A self-referential bound/default captures the still-empty cell;
            // publication happens only after both native creation operations.
            result.push(py_stmt!(
                "{binding:id} = {parameter:id}",
                binding = binding.as_str(),
                parameter = saved_name.as_str()
            ));
            saved.push(saved_name);
        }
        result.push(py_stmt!(
            "{tuple:id} = {values:expr}",
            tuple = tuple,
            values = make_tuple(
                saved
                    .iter()
                    .map(|name| py_expr!("{name:id}", name = name.as_str()))
                    .collect()
            )
        ));
        for name in saved {
            result.push(py_stmt!("del {name:id}", name = name.as_str()));
        }
        result
    }

    fn scope_helper(&self, body: Vec<Stmt>, plan: TypeParameterScopePlan) -> ast::StmtFunctionDef {
        let name = self
            .context
            .fresh_annotation_binding("type_parameter_scope");
        let mut helper: ast::StmtFunctionDef = match plan.inputs.as_slice() {
            [] => py_stmt_typed!("def {name:id}(): pass", name = name.as_str()),
            [input] => py_stmt_typed!(
                "def {name:id}({input:id}): pass",
                name = name.as_str(),
                input = input.body_parameter.as_str()
            ),
            [positional, keyword] => py_stmt_typed!(
                "def {name:id}({positional:id}, {keyword:id}): pass",
                name = name.as_str(),
                positional = positional.body_parameter.as_str(),
                keyword = keyword.body_parameter.as_str()
            ),
            _ => unreachable!("generic scopes have at most two native default inputs"),
        };
        helper.body = body.into();
        self.context.record_type_parameter_scope(&mut helper, plan);
        helper
    }

    pub(super) fn rewrite_generic_alias(&mut self, mut alias: ast::StmtTypeAlias) -> Vec<Stmt> {
        let parameters = *alias.type_params.take().expect("generic alias parameters");
        let needs_conditional =
            parameters_use_conditional(&parameters) || uses_conditional_cell(&alias.value);
        let (mut inner, class_dictionary, conditional_annotations) =
            self.parameter_scope(needs_conditional);
        let definition = self
            .context
            .type_expression_definition(alias.range, DefinitionKind::TypeAlias);
        let tuple = self.context.fresh_annotation_binding("type_parameters");
        let mut body = inner.parameter_statements(parameters, &tuple);
        let (helper, name) = inner.expression_provider(
            alias.range,
            DefinitionKind::TypeAlias,
            AnnotationProviderKind::TypeAliasValue,
            alias.range,
            *alias.value,
            false,
        );
        body.push(Stmt::FunctionDef(helper));
        let native_name = self.context.native_definition(&definition).name;
        let create = operation_args(
            self.context,
            AnnotationOperation::CreateAlias {
                declaration: alias.range,
            },
            vec![
                py_expr!("{name:literal}", name = native_name.as_str()),
                py_expr!("{name:id}", name = tuple.as_str()),
                py_expr!("{name:id}", name = name.as_str()),
            ],
        );
        body.push(py_stmt!("return {value:expr}", value = create));
        let helper = self.scope_helper(
            body,
            TypeParameterScopePlan {
                definition,
                inputs: Vec::new(),
                class_dictionary,
                conditional_annotations,
                owned_parameter_tuple: None,
            },
        );
        let construct = operation_args(
            self.context,
            AnnotationOperation::ConstructTypeParameterScope {
                declaration: alias.range,
                kind: DefinitionKind::TypeAlias,
                positional_defaults: false,
                keyword_defaults: false,
                complete_function: false,
            },
            Vec::new(),
        );
        vec![
            Stmt::FunctionDef(helper),
            py_stmt!(
                "{target:expr} = {value:expr}",
                target = alias.name,
                value = construct
            ),
        ]
    }

    pub(super) fn rewrite_generic_function(
        &mut self,
        mut function: ast::StmtFunctionDef,
    ) -> Vec<Stmt> {
        let parameters = *function
            .type_params
            .take()
            .expect("generic function parameters");
        let needs_conditional = parameters_use_conditional(&parameters)
            || function_annotations_use_conditional(&function);
        let (mut inner, class_dictionary, conditional_annotations) =
            self.parameter_scope(needs_conditional);
        let definition = self
            .context
            .type_expression_definition(function.range, DefinitionKind::Function);
        let decorators = std::mem::take(&mut function.decorator_list);
        let mut inputs = Vec::new();
        let mut arguments = Vec::new();
        let mut positional_values = Vec::new();
        for parameter in function
            .parameters
            .posonlyargs
            .iter_mut()
            .chain(&mut function.parameters.args)
        {
            if let Some(value) = parameter.default.take() {
                positional_values.push(*value);
                parameter.default = Some(Box::new(py_expr!("None")));
            }
        }
        let positional = (!positional_values.is_empty()).then(|| {
            let name = self.context.fresh_annotation_binding("positional_defaults");
            inputs.push(TypeParameterScopeInput {
                kind: TypeParameterScopeInputKind::PositionalDefaults,
                body_parameter: name.clone(),
            });
            arguments.push(make_tuple(positional_values));
            name
        });
        let mut keyword_items = Vec::new();
        for parameter in &mut function.parameters.kwonlyargs {
            if let Some(value) = parameter.default.take() {
                keyword_items.push(ast::DictItem {
                    key: Some(py_expr!(
                        "{name:literal}",
                        name = parameter.parameter.name.as_str()
                    )),
                    value: *value,
                });
                parameter.default = Some(Box::new(py_expr!("None")));
            }
        }
        let keyword = (!keyword_items.is_empty()).then(|| {
            let name = self.context.fresh_annotation_binding("keyword_defaults");
            inputs.push(TypeParameterScopeInput {
                kind: TypeParameterScopeInputKind::KeywordDefaults,
                body_parameter: name.clone(),
            });
            arguments.push(Expr::Dict(ast::ExprDict {
                range: Default::default(),
                node_index: Default::default(),
                items: keyword_items,
            }));
            name
        });
        let tuple = self.context.fresh_annotation_binding("type_parameters");
        self.context.record_generic_function(
            &function,
            GenericFunctionPlan {
                positional_defaults: positional.clone(),
                keyword_defaults: keyword.clone(),
                type_parameters: tuple.clone(),
            },
        );
        let binding = function.name.to_string();
        let declaration = function.range;
        let mut body = inner.parameter_statements(parameters, &tuple);
        let mut target: Suite = vec![Stmt::FunctionDef(function)].into();
        inner.visit_body(&mut target);
        body.extend(target);
        body.push(py_stmt!("return {name:id}", name = binding.as_str()));
        let helper = self.scope_helper(
            body,
            TypeParameterScopePlan {
                definition,
                inputs,
                class_dictionary,
                conditional_annotations,
                owned_parameter_tuple: None,
            },
        );
        let construct = operation_args(
            self.context,
            AnnotationOperation::ConstructTypeParameterScope {
                declaration,
                kind: DefinitionKind::Function,
                positional_defaults: positional.is_some(),
                keyword_defaults: keyword.is_some(),
                complete_function: decorators.is_empty(),
            },
            arguments,
        );
        let value = decorator::rewrite(decorators.into(), construct);
        vec![
            Stmt::FunctionDef(helper),
            py_stmt!(
                "{name:id} = {value:expr}",
                name = binding.as_str(),
                value = value
            ),
        ]
    }

    pub(super) fn rewrite_generic_class(&mut self, mut class: ast::StmtClassDef) -> Vec<Stmt> {
        let parameters = *class.type_params.take().expect("generic class parameters");
        let needs_conditional = parameters_use_conditional(&parameters);
        let (mut inner, class_dictionary, conditional_annotations) =
            self.parameter_scope(needs_conditional);
        let definition = self
            .context
            .type_expression_definition(class.range, DefinitionKind::Class);
        let decorators = std::mem::take(&mut class.decorator_list);
        let tuple = self.context.fresh_annotation_binding("type_parameters");
        let generic_base = self.context.fresh_annotation_binding("generic_base");
        self.context.record_generic_class(
            class.range,
            GenericClassPlan {
                type_parameters: tuple.clone(),
                generic_base,
            },
        );
        let binding = class.name.to_string();
        let declaration = class.range;
        let mut body = inner.parameter_statements(parameters, &tuple);
        let mut target: Suite = vec![Stmt::ClassDef(class)].into();
        inner.visit_body(&mut target);
        body.extend(target);
        body.push(py_stmt!("return {name:id}", name = binding.as_str()));
        let helper = self.scope_helper(
            body,
            TypeParameterScopePlan {
                definition,
                inputs: Vec::new(),
                class_dictionary,
                conditional_annotations,
                owned_parameter_tuple: Some(tuple),
            },
        );
        let construct = operation_args(
            self.context,
            AnnotationOperation::ConstructTypeParameterScope {
                declaration,
                kind: DefinitionKind::Class,
                positional_defaults: false,
                keyword_defaults: false,
                complete_function: false,
            },
            Vec::new(),
        );
        let value = decorator::rewrite(decorators.into(), construct);
        vec![
            Stmt::FunctionDef(helper),
            py_stmt!(
                "{name:id} = {value:expr}",
                name = binding.as_str(),
                value = value
            ),
        ]
    }
}
