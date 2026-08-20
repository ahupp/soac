//! The VALUE body of a native annotation provider. Other formats deliberately
//! raise the native NotImplementedError so annotationlib can replay the actual
//! authenticated original code with its normal fake globals and closure rules.

use ruff_python_ast::{self as ast, Expr, Parameter, Stmt};
use ruff_text_size::{Ranged, TextRange};
use soac_core::block_py::{AnnotationProviderKind, CallableSourceRole, CellCaptureProjection};

use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::context::{
    AnnotationClassCapture, AnnotationConditionalCell, AnnotationOperation, AnnotationProviderPlan,
    Context,
};
use crate::template::{py_expr, py_stmt};
use crate::transformer::{walk_stmt, Transformer};

mod generic;

#[derive(Clone, Copy)]
enum Scope {
    Module,
    Class(TextRange),
    Function,
}

#[derive(Clone)]
struct AnnotationClassContext {
    declaration: TextRange,
    source_name: String,
    projection: CellCaptureProjection,
}

struct Entry {
    name: String,
    value: Expr,
    conditional_index: Option<u32>,
}

fn uses_conditional_cell(expression: &Expr) -> bool {
    struct Names(bool);
    impl Transformer for Names {
        fn visit_expr(&mut self, expression: &mut Expr) {
            if matches!(expression, Expr::Name(name) if name.id == "__conditional_annotations__"
                && name.ctx == ast::ExprContext::Load)
            {
                self.0 = true;
            }
            crate::transformer::walk_expr(self, expression);
        }
    }
    let mut names = Names(false);
    names.visit_expr(&mut expression.clone());
    names.0
}

struct Rewriter<'a> {
    context: &'a Context,
    scope: Scope,
    conditional_depth: usize,
    next_index: u32,
    entries: Vec<Entry>,
    annotations_used: bool,
    conditional_cell: Option<AnnotationConditionalCell>,
    class_context: Option<AnnotationClassContext>,
}

pub(super) fn rewrite_module(context: &Context, body: &mut Suite) {
    let mut rewriter = Rewriter::new(context, Scope::Module);
    rewriter.visit_body(body);
    // The module body becomes a hidden initializer function. Its leading
    // string must remain a source-ordered module binding, not that helper's
    // function documentation, even when there is no annotation provider.
    make_docstring_store(body);
    if context.future_annotations() {
        if rewriter.annotations_used {
            body.insert(
                0,
                py_stmt!(
                    "{setup:expr}",
                    setup = operation(context, AnnotationOperation::Setup, None)
                ),
            );
            body.insert(
                0,
                py_stmt!(
                    "__conditional_annotations__ = {value:expr}",
                    value = operation(context, AnnotationOperation::NewSet, None)
                ),
            );
        }
        return;
    }
    if rewriter.entries.is_empty() {
        return;
    }
    let (mut helper, plan) = rewriter.provider("__annotate__", None, false);
    context.record_module_annotation_helper(&mut helper);
    context.record_annotation_provider(&helper, plan);
    body.insert(
        0,
        py_stmt!(
            "__conditional_annotations__ = {value:expr}",
            value = operation(context, AnnotationOperation::NewSet, None)
        ),
    );
    // CPython's ANNOTATIONS_PLACEHOLDER precedes the module body, including
    // creation of the conditional-index set and assignment of __doc__.
    body.insert(0, Stmt::FunctionDef(helper));
}

fn operation(context: &Context, kind: AnnotationOperation, argument: Option<Expr>) -> Expr {
    operation_args(context, kind, argument.into_iter().collect())
}

fn operation_args(context: &Context, kind: AnnotationOperation, arguments: Vec<Expr>) -> Expr {
    let Expr::Call(mut call) = py_expr!("_dp_annotation_operation()") else {
        unreachable!()
    };
    call.arguments.args = arguments.into_boxed_slice();
    let expression = Expr::Call(call);
    context.record_annotation_operation(&expression, kind);
    expression
}

fn make_docstring_store(body: &mut Suite) {
    if let Some(Stmt::Expr(statement)) = body.first_mut() {
        if matches!(statement.value.as_ref(), Expr::StringLiteral(_)) {
            let value = statement.value.clone();
            body[0] = py_stmt!("__doc__ = {value:expr}", value = value);
        }
    }
}

impl<'a> Rewriter<'a> {
    fn new(context: &'a Context, scope: Scope) -> Self {
        Self {
            context,
            scope,
            conditional_depth: 0,
            next_index: 0,
            entries: Vec::new(),
            annotations_used: false,
            conditional_cell: None,
            class_context: match scope {
                Scope::Class(declaration) => Some(AnnotationClassContext {
                    declaration,
                    source_name: "_dp_classdictcell_arg".into(),
                    projection: CellCaptureProjection::CellObject,
                }),
                _ => None,
            },
        }
    }

    fn conditional_cell(&mut self) -> AnnotationConditionalCell {
        self.conditional_cell
            .get_or_insert_with(|| AnnotationConditionalCell {
                body_binding: self.context.fresh_annotation_binding("annotation_indices"),
                owner_binding: self
                    .context
                    .fresh_annotation_binding("annotation_indices_owner"),
                storage_name: self
                    .context
                    .fresh_annotation_binding("annotation_indices_cell"),
            })
            .clone()
    }

    fn class_capture(&self) -> Option<AnnotationClassCapture> {
        self.class_context.as_ref().map(|class| {
            self.context.require_class_dict_cell(class.declaration);
            AnnotationClassCapture {
                body_binding: self
                    .context
                    .fresh_annotation_binding("annotation_class_dictionary"),
                source_name: class.source_name.clone(),
                projection: class.projection,
            }
        })
    }

    fn provider(
        &self,
        name: &str,
        class: Option<TextRange>,
        signature: bool,
    ) -> (ast::StmtFunctionDef, AnnotationProviderPlan) {
        let format = self.context.fresh_annotation_binding("annotation_format");
        let result = self.context.fresh_annotation_binding("annotation_result");
        let mut body = vec![py_stmt!(
            "{check:expr}",
            check = operation(
                self.context,
                AnnotationOperation::CheckFormat,
                Some(py_expr!("{name:id}", name = format.as_str()))
            )
        )];
        if signature {
            let value = Expr::Dict(ast::ExprDict {
                range: Default::default(),
                node_index: Default::default(),
                items: self
                    .entries
                    .iter()
                    .map(|entry| ast::DictItem {
                        key: Some(py_expr!("{key:literal}", key = entry.name.as_str())),
                        value: entry.value.clone(),
                    })
                    .collect(),
            });
            body.push(py_stmt!("return {value:expr}", value = value));
        } else {
            body.push(py_stmt!("{result:id} = {}", result = result.as_str()));
            for entry in &self.entries {
                let store = py_stmt!(
                    "{result:id}[{key:literal}] = {value:expr}",
                    result = result.as_str(),
                    key = entry.name.as_str(),
                    value = entry.value.clone()
                );
                if let Some(index) = entry.conditional_index {
                    let indices = self
                        .conditional_cell
                        .as_ref()
                        .map_or("__conditional_annotations__", |cell| {
                            cell.body_binding.as_str()
                        });
                    body.push(py_stmt!(
                        "if {index:literal} in {indices:id}:\n    {store:stmt}",
                        index = index,
                        indices = indices,
                        store = store
                    ));
                } else {
                    body.push(store);
                }
            }
            body.push(py_stmt!("return {result:id}", result = result.as_str()));
        }
        let Stmt::FunctionDef(helper) = py_stmt!(
            "def {name:id}({format:id}, /):\n    {body:stmt}",
            name = name,
            format = format.as_str(),
            body = body
        ) else {
            unreachable!()
        };
        let class_dictionary = class.and_then(|_| self.class_capture());
        (
            helper,
            AnnotationProviderPlan {
                kind: AnnotationProviderKind::Dictionary,
                native_range: (!signature && class.is_some()).then(|| {
                    let range = self
                        .entries
                        .first()
                        .expect("class annotation entry")
                        .value
                        .range();
                    soac_contracts::SourceRange::new(range.start().to_u32(), range.end().to_u32())
                }),
                body_format_parameter: format,
                class_dictionary,
                conditional_annotations: self.conditional_cell.clone(),
            },
        )
    }

    fn take_signature(&self, function: &mut ast::StmtFunctionDef) -> Vec<Entry> {
        fn take(parameter: &mut Parameter, result: &mut Vec<Entry>) {
            if let Some(value) = parameter.annotation.take() {
                result.push(Entry {
                    name: parameter.name.to_string(),
                    value: *value,
                    conditional_index: None,
                });
            }
        }
        let mut result = Vec::new();
        for parameter in function
            .parameters
            .args
            .iter_mut()
            .chain(&mut function.parameters.posonlyargs)
        {
            take(&mut parameter.parameter, &mut result);
        }
        if let Some(parameter) = &mut function.parameters.vararg {
            take(parameter, &mut result);
        }
        for parameter in &mut function.parameters.kwonlyargs {
            take(&mut parameter.parameter, &mut result);
        }
        if let Some(parameter) = &mut function.parameters.kwarg {
            take(parameter, &mut result);
        }
        if let Some(value) = function.returns.take() {
            result.push(Entry {
                name: "return".into(),
                value: *value,
                conditional_index: None,
            });
        }
        result
    }

    fn rewrite_annotation(&mut self, annotation: ast::StmtAnnAssign) -> Vec<Stmt> {
        self.annotations_used = true;
        if self.context.future_annotations()
            && matches!(self.scope, Scope::Class(_))
            && self.conditional_depth != 0
        {
            self.conditional_cell();
        }
        let mut result = Vec::new();
        let ast::StmtAnnAssign {
            target,
            annotation: value,
            value: assigned,
            simple,
            ..
        } = annotation;
        if let Some(assigned) = assigned {
            result.push(py_stmt!(
                "{target:expr} = {value:expr}",
                target = target.clone(),
                value = assigned
            ));
        } else {
            annotation_target_effects(&target, &mut result);
        }
        if simple && !matches!(self.scope, Scope::Function) {
            let Expr::Name(name) = target.as_ref() else {
                return result;
            };
            if self.context.future_annotations() {
                result.push(py_stmt!(
                    "__annotations__[{name:literal}] = {value:expr}",
                    name = name.id.as_str(),
                    value = value
                ));
                return result;
            }
            // Native class annotation scopes also bind this implicit cell
            // when source refers to it without a conditional annotation.
            if matches!(self.scope, Scope::Class(_)) && uses_conditional_cell(&value) {
                self.conditional_cell();
            }
            let conditional = matches!(self.scope, Scope::Module) || self.conditional_depth != 0;
            let index = conditional.then(|| {
                let index = self.next_index;
                self.next_index = index.checked_add(1).expect("annotation index overflow");
                index
            });
            self.entries.push(Entry {
                name: name.id.to_string(),
                value: *value,
                conditional_index: index,
            });
            if let Some(index) = index {
                let binding = if matches!(self.scope, Scope::Class(_)) {
                    self.conditional_cell().owner_binding
                } else {
                    "__conditional_annotations__".into()
                };
                result.push(py_stmt!(
                    "{record:expr}",
                    record = operation(
                        self.context,
                        AnnotationOperation::Record { index },
                        Some(py_expr!("{binding:id}", binding = binding.as_str()))
                    )
                ));
            }
        }
        result
    }

    fn rewrite_alias(&mut self, alias: ast::StmtTypeAlias) -> Vec<Stmt> {
        if alias.type_params.is_some() {
            return self.rewrite_generic_alias(alias);
        }
        let definition = self
            .context
            .type_expression_definition(alias.range, soac_contracts::DefinitionKind::TypeAlias);
        let native_name = self.context.native_definition(&definition).name;
        let (helper, helper_name) = self.expression_provider(
            alias.range,
            soac_contracts::DefinitionKind::TypeAlias,
            AnnotationProviderKind::TypeAliasValue,
            alias.range,
            *alias.value,
            false,
        );
        let create = operation_args(
            self.context,
            AnnotationOperation::CreateAlias {
                declaration: alias.range,
            },
            vec![
                py_expr!("{name:literal}", name = native_name.as_str()),
                py_expr!("None"),
                py_expr!("{name:id}", name = helper_name.as_str()),
            ],
        );
        vec![
            Stmt::FunctionDef(helper),
            py_stmt!(
                "try:\n    {target:expr} = {value:expr}\nfinally:\n    del {temporary:id}",
                target = alias.name,
                value = create,
                temporary = helper_name.as_str(),
            ),
        ]
    }

    fn expression_provider(
        &mut self,
        declaration: TextRange,
        definition_kind: soac_contracts::DefinitionKind,
        kind: AnnotationProviderKind,
        native_range: TextRange,
        value: Expr,
        allow_starred: bool,
    ) -> (ast::StmtFunctionDef, String) {
        let helper_name = self
            .context
            .fresh_annotation_binding("evaluate_type_expression");
        let format = self.context.fresh_annotation_binding("annotation_format");
        let conditional_annotations =
            if self.class_context.is_some() && uses_conditional_cell(&value) {
                Some(self.conditional_cell())
            } else {
                None
            };
        let class_dictionary = self.class_capture();
        let result = if allow_starred {
            if let Expr::Starred(starred) = value {
                let result = self
                    .context
                    .fresh_annotation_binding("type_parameter_default");
                vec![
                    py_stmt!(
                        "{result:id}, = {value:expr}",
                        result = result.as_str(),
                        value = starred.value
                    ),
                    py_stmt!("return {result:id}", result = result.as_str()),
                ]
            } else {
                vec![py_stmt!("return {value:expr}", value = value)]
            }
        } else {
            vec![py_stmt!("return {value:expr}", value = value)]
        };
        let Stmt::FunctionDef(mut helper) = py_stmt!(
            "def {name:id}({format:id}=1, /):\n    {check:expr}\n    {result:stmt}",
            name = helper_name.as_str(),
            format = format.as_str(),
            check = operation(
                self.context,
                AnnotationOperation::CheckFormat,
                Some(py_expr!("{name:id}", name = format.as_str()))
            ),
            result = result,
        ) else {
            unreachable!()
        };
        self.context
            .record_type_expression_helper(declaration, definition_kind, &mut helper);
        self.context.record_annotation_provider(
            &helper,
            AnnotationProviderPlan {
                kind,
                native_range: Some(soac_contracts::SourceRange::new(
                    native_range.start().to_u32(),
                    native_range.end().to_u32(),
                )),
                body_format_parameter: format,
                class_dictionary,
                conditional_annotations,
            },
        );
        (helper, helper_name)
    }
}

// A non-simple annotation without a value evaluates target components but
// does not perform the attribute/subscript lookup itself.
fn annotation_target_effects(target: &Expr, result: &mut Vec<Stmt>) {
    fn effect(value: &Expr, result: &mut Vec<Stmt>) {
        result.push(py_stmt!("{value:expr}", value = value.clone()));
    }
    fn slice(value: &Expr, result: &mut Vec<Stmt>) {
        match value {
            Expr::Slice(value) => {
                for value in [&value.lower, &value.upper, &value.step]
                    .into_iter()
                    .flatten()
                {
                    effect(value, result);
                }
            }
            Expr::Tuple(value) => {
                for value in &value.elts {
                    slice(value, result);
                }
            }
            _ => effect(value, result),
        }
    }
    match target {
        Expr::Attribute(value) => effect(&value.value, result),
        Expr::Subscript(value) => {
            effect(&value.value, result);
            slice(&value.slice, result);
        }
        _ => {}
    }
}

impl Transformer for Rewriter<'_> {
    fn visit_body(&mut self, body: &mut Suite) {
        let mut result = Vec::new();
        for statement in std::mem::take(body) {
            match statement {
                Stmt::AnnAssign(annotation) => result.extend(self.rewrite_annotation(annotation)),
                Stmt::TypeAlias(alias) => result.extend(self.rewrite_alias(alias)),
                Stmt::FunctionDef(function) if function.type_params.is_some() => {
                    result.extend(self.rewrite_generic_function(function));
                }
                Stmt::FunctionDef(mut function) => {
                    let entries = self.take_signature(&mut function);
                    if !entries.is_empty() {
                        let mut provider = Self::new(self.context, self.scope);
                        provider.class_context = self.class_context.clone();
                        if self.class_context.is_some()
                            && entries
                                .iter()
                                .any(|entry| uses_conditional_cell(&entry.value))
                        {
                            provider.conditional_cell = Some(self.conditional_cell());
                        }
                        provider.entries = entries;
                        let class = self
                            .class_context
                            .as_ref()
                            .filter(|_| !self.context.future_annotations())
                            .map(|class| class.declaration);
                        let name = self.context.fresh_annotation_binding("annotate_function");
                        let (mut helper, plan) = provider.provider(&name, class, true);
                        self.context
                            .record_function_annotation_helper(&function, &mut helper);
                        self.context.record_annotation_provider(&helper, plan);
                        result.push(Stmt::FunctionDef(helper));
                    }
                    Self::new(self.context, Scope::Function).visit_body(&mut function.body);
                    result.push(Stmt::FunctionDef(function));
                }
                Stmt::ClassDef(class) if class.type_params.is_some() => {
                    result.extend(self.rewrite_generic_class(class));
                }
                Stmt::ClassDef(mut class) => {
                    let mut nested = Self::new(self.context, Scope::Class(class.range));
                    nested.visit_body(&mut class.body);
                    if self.context.future_annotations() && nested.annotations_used {
                        make_docstring_store(&mut class.body);
                        class.body.insert(
                            0,
                            py_stmt!(
                                "{setup:expr}",
                                setup = operation(self.context, AnnotationOperation::Setup, None)
                            ),
                        );
                    } else if !nested.entries.is_empty() {
                        let (mut helper, plan) =
                            nested.provider("__annotate_func__", Some(class.range), false);
                        self.context.record_class_helper_origin(
                            class.range,
                            &mut helper,
                            CallableSourceRole::AnnotationProvider,
                        );
                        self.context.record_annotation_provider(&helper, plan);
                        class.body.push(Stmt::FunctionDef(helper));
                    }
                    if let Some(cell) = nested.conditional_cell {
                        if self.context.native_class_plan(class.range).is_none() {
                            make_docstring_store(&mut class.body);
                            class.body.insert(
                                0,
                                py_stmt!(
                                    "{binding:id} = {value:expr}",
                                    binding = cell.owner_binding.as_str(),
                                    value =
                                        operation(self.context, AnnotationOperation::NewSet, None)
                                ),
                            );
                        }
                        self.context.record_class_annotation_cell(class.range, cell);
                    }
                    result.push(Stmt::ClassDef(class));
                }
                mut statement => {
                    self.visit_stmt(&mut statement);
                    result.push(statement);
                }
            }
        }
        *body = result.into();
    }

    fn visit_stmt(&mut self, statement: &mut Stmt) {
        let conditional = matches!(
            statement,
            Stmt::If(_)
                | Stmt::For(_)
                | Stmt::While(_)
                | Stmt::With(_)
                | Stmt::Match(_)
                | Stmt::Try(_)
        );
        self.conditional_depth += usize::from(conditional);
        if let Stmt::Try(statement) = statement {
            // Match native code generation order, which differs from AST
            // source order for else and except suites.
            self.visit_body(&mut statement.body);
            self.visit_body(&mut statement.orelse);
            for handler in &mut statement.handlers {
                self.visit_except_handler(handler);
            }
            self.visit_body(&mut statement.finalbody);
        } else {
            walk_stmt(self, statement);
        }
        self.conditional_depth -= usize::from(conditional);
    }
}
