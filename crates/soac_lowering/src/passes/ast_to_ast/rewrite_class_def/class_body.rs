use std::mem::take;

use ruff_python_ast::{Expr, Stmt};

use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::context::{AnnotationOperation, ClassDecoratorOperation, Context};
use crate::passes::ast_to_ast::rewrite_class_def::{class_def_to_create_class_fn, method};
use crate::passes::ast_to_ast::rewrite_stmt;
use crate::passes::ast_to_ast::semantic::{
    SemanticAstState, SemanticBindingKind, SemanticScope, SemanticScopeKind,
};
use crate::template::{py_expr, py_stmt};
use crate::transformer::{walk_expr, walk_stmt, Transformer};

pub(crate) fn rewrite_class_body_scopes(
    context: &Context,
    semantic_state: &mut SemanticAstState,
    body: &mut Suite,
) {
    let scope = semantic_state.module_scope();
    ClassBodyScopeRewriter::new(context, scope, semantic_state).visit_body(body);
}

struct ClassBodyScopeRewriter<'a> {
    context: &'a Context,
    scope: SemanticScope,
    semantic_state: &'a mut SemanticAstState,
    hoisted_class_defs: Vec<Stmt>,
    native_namespace: Option<String>,
}

struct StaticAttributeClosureCaptureFinder<'a> {
    scope: SemanticScope,
    semantic_state: &'a SemanticAstState,
    captured: bool,
}

impl Transformer for StaticAttributeClosureCaptureFinder<'_> {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        if self.captured {
            return;
        }

        match stmt {
            Stmt::FunctionDef(function) => {
                let Some(child_scope) = self.scope.child_scope_for_function(function) else {
                    return;
                };
                match child_scope.binding_in_current_scope("__static_attributes__") {
                    Some(SemanticBindingKind::Nonlocal) => self.captured = true,
                    Some(SemanticBindingKind::Global | SemanticBindingKind::Local) => {}
                    None => {
                        let previous = std::mem::replace(&mut self.scope, child_scope);
                        self.visit_body(&mut function.body);
                        self.scope = previous;
                    }
                }
            }
            Stmt::ClassDef(class) => {
                let Some(child_scope) = self.scope.child_scope_for_class(class) else {
                    return;
                };
                if matches!(
                    child_scope.binding_in_current_scope("__static_attributes__"),
                    Some(SemanticBindingKind::Nonlocal)
                ) {
                    self.captured = true;
                    return;
                }

                // Class locals do not hide an enclosing function cell from
                // methods, so continue through nested class scopes.
                let previous = std::mem::replace(&mut self.scope, child_scope);
                self.visit_body(&mut class.body);
                self.scope = previous;
            }
            other => walk_stmt(self, other),
        }
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        if self.captured {
            return;
        }

        let Expr::Lambda(lambda) = expr else {
            walk_expr(self, expr);
            return;
        };
        let Some(child_scope) = self.semantic_state.lambda_scope(lambda) else {
            return;
        };
        match child_scope.binding_in_current_scope("__static_attributes__") {
            Some(SemanticBindingKind::Nonlocal) => self.captured = true,
            Some(SemanticBindingKind::Global | SemanticBindingKind::Local) => {}
            None => {
                let previous = std::mem::replace(&mut self.scope, child_scope);
                if let Some(mut body) = self.semantic_state.lowered_lambda_body(lambda) {
                    self.visit_body(&mut body.statements);
                } else {
                    self.visit_expr(lambda.body.as_mut());
                }
                self.scope = previous;
            }
        }
    }
}

fn class_descendants_capture_static_attribute_cell(
    scope: &SemanticScope,
    semantic_state: &SemanticAstState,
    body: &mut Suite,
) -> bool {
    if scope
        .binding_in_current_scope("__static_attributes__")
        .is_some()
        || !matches!(
            scope.resolved_load_binding("__static_attributes__"),
            SemanticBindingKind::Nonlocal
        )
    {
        return false;
    }

    let mut finder = StaticAttributeClosureCaptureFinder {
        scope: scope.clone(),
        semantic_state,
        captured: false,
    };
    finder.visit_body(body);
    finder.captured
}

impl<'a> ClassBodyScopeRewriter<'a> {
    fn new(
        context: &'a Context,
        scope: SemanticScope,
        semantic_state: &'a mut SemanticAstState,
    ) -> Self {
        Self {
            context,
            scope,
            semantic_state,
            hoisted_class_defs: Vec::new(),
            native_namespace: None,
        }
    }

    fn take_hoisted(&mut self) -> Vec<Stmt> {
        take(&mut self.hoisted_class_defs)
    }
}

impl<'a> Transformer for ClassBodyScopeRewriter<'a> {
    fn visit_body(&mut self, body: &mut Suite) {
        let mut rewritten = Vec::with_capacity(body.len());
        for stmt in std::mem::take(body) {
            rewritten.extend(self.rewrite_stmt_list(stmt));
        }
        *body = rewritten.into();
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                // Lambda defaults still belong to this enclosing scope.
                for decorator in &mut func_def.decorator_list {
                    self.visit_decorator(decorator);
                }
                self.visit_parameters(&mut func_def.parameters);
                let func_scope = self
                    .scope
                    .child_scope_for_function(func_def)
                    .expect("no child scope for function");
                ClassBodyScopeRewriter::new(self.context, func_scope, self.semantic_state)
                    .visit_body(&mut func_def.body);
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        if let Expr::Lambda(lambda) = expr {
            if let Some(mut body) = self.semantic_state.lowered_lambda_body(lambda) {
                if let Some(parameters) = &mut lambda.parameters {
                    self.visit_parameters(parameters);
                }
                let scope = self
                    .semantic_state
                    .lambda_scope(lambda)
                    .expect("lowered lambda body retains its semantic scope");
                let mut rewriter =
                    ClassBodyScopeRewriter::new(self.context, scope, self.semantic_state);
                rewriter.visit_body(&mut body.statements);
                assert!(
                    rewriter.take_hoisted().is_empty(),
                    "lambda helpers cannot hoist a class"
                );
                self.semantic_state
                    .replace_lowered_lambda_body(lambda, body);
                return;
            }
        }
        walk_expr(self, expr);
    }
}

impl<'a> ClassBodyScopeRewriter<'a> {
    fn rewrite_stmt_list(&mut self, stmt: Stmt) -> Vec<Stmt> {
        let Stmt::ClassDef(mut class_def) = stmt else {
            let mut stmt = stmt;
            self.visit_stmt(&mut stmt);
            return vec![stmt];
        };

        let has_decorator_proposal = self
            .context
            .class_decorator_proposal(class_def.range, &class_def.decorator_list)
            .is_some();
        let mut decorator_list = take(&mut class_def.decorator_list);
        let preparation = has_decorator_proposal.then(|| {
            let decorator = decorator_list.pop().unwrap().expression;
            let (expression, factory) = match decorator {
                Expr::Call(original) => {
                    let mut expression = py_expr!("_dp_prepare_class_decorator()");
                    let Expr::Call(call) = &mut expression else {
                        unreachable!()
                    };
                    // Keep source argument evaluation and expansion unchanged,
                    // but give this compiler operation its own provenance node.
                    call.func = original.func;
                    call.arguments = original.arguments;
                    (expression, true)
                }
                decorator => (
                    py_expr!(
                        "_dp_prepare_class_decorator({decorator:expr})",
                        decorator = decorator
                    ),
                    false,
                ),
            };
            let node = self.semantic_state.assign_generated_node_index(&expression);
            self.context.record_class_decorator_operation(
                node,
                ClassDecoratorOperation::Prepare {
                    declaration: class_def.range,
                    factory,
                },
            );
            (
                self.context.fresh_annotation_binding("class_decorator"),
                expression,
            )
        });
        let native = self.context.strict_source().is_some().then(|| {
            self.context.native_definition(
                &self.context.type_expression_definition(
                    class_def.range,
                    soac_contracts::DefinitionKind::Class,
                ),
            )
        });
        let first_line_offset = if let Some(native) = &native {
            ruff_text_size::TextSize::from(native.first_offset)
        } else {
            class_def.range.start()
        };
        let class_firstlineno = self.context.line_number_at(first_line_offset.to_usize());
        let needs_class_cell =
            method::rewrite_explicit_super_classcell(&mut class_def, self.semantic_state);

        let class_scope = self
            .scope
            .child_scope_for_class(&class_def)
            .expect("no child scope for class");
        let captures_outer_static_attributes = class_descendants_capture_static_attribute_cell(
            &class_scope,
            self.semantic_state,
            &mut class_def.body,
        );

        let native_namespace = self
            .context
            .native_class_plan(class_def.range)
            .map(|plan| plan.scope.namespace_binding);
        let mut class_rewriter =
            ClassBodyScopeRewriter::new(self.context, class_scope.clone(), self.semantic_state);
        class_rewriter.native_namespace = native_namespace;
        class_rewriter.visit_body(&mut class_def.body);
        let mut hoisted = class_rewriter.take_hoisted();

        let (class_ns_def, define_class_fn, bases_tuple, prepare_dict) =
            class_def_to_create_class_fn(
                self.context,
                self.semantic_state,
                &mut class_def,
                native.as_ref().map_or_else(
                    || class_scope.qualname().to_string(),
                    |native| native.qualname.clone(),
                ),
                needs_class_cell,
                &class_scope,
                captures_outer_static_attributes,
                class_firstlineno,
                preparation.is_some(),
            );
        self.semantic_state
            .register_function_scope_override(&class_ns_def, class_scope.clone());
        self.semantic_state
            .register_function_scope_override(&define_class_fn, self.scope.clone());
        self.context
            .record_class_namespace_binding(class_def.range, class_ns_def.name.id.as_str());

        hoisted.push(class_ns_def.clone().into());

        let mut children = Vec::new();
        if let Some((binding, expression)) = &preparation {
            // CPython evaluates the decorator/factory before creating the
            // class namespace function, evaluating bases, or running the body.
            let store = py_stmt!(
                "{binding:id} = {expression:expr}",
                binding = binding.as_str(),
                expression = expression.clone(),
            );
            children.push(store);
        }
        // Keep nested class namespace helpers in lexical scope with the
        // matching `_dp_define_class_*` call site. Hoisting these out
        // of class bodies makes helper resolution depend on module
        // globals, which breaks once top-level code is wrapped in
        // `_dp_module_init`.
        children.append(&mut hoisted);
        if let Some(generic) = self.context.generic_class(class_def.range) {
            let expression = py_expr!(
                "_dp_subscript_generic({parameters:id})",
                parameters = generic.type_parameters.as_str()
            );
            let node = self.semantic_state.assign_generated_node_index(&expression);
            self.context.record_annotation_operation_node(
                node,
                AnnotationOperation::SubscriptGeneric {
                    declaration: class_def.range,
                },
            );
            children.push(py_stmt!(
                "{base:id} = {value:expr}",
                base = generic.generic_base.as_str(),
                value = expression
            ));
        }
        children.push(define_class_fn.clone().into());
        // This private binding retains the exact successful MakeFunction
        // result. Neither later public arguments nor a source-name write can
        // choose the helper whose ephemeral cells the finally block releases.
        let capture_region = self.context.strict_source().is_some().then(|| {
            let original = self.context.fresh_annotation_binding("class_constructor");
            children.push(py_stmt!(
                "{original:id} = {created:id}",
                original = original.as_str(),
                created = define_class_fn.name.id.as_str(),
            ));
            (children.len(), original)
        });

        let class_ns_outer = if matches!(self.scope.kind(), SemanticScopeKind::Class) {
            py_expr!(
                "{namespace:id}",
                namespace = self.native_namespace.as_deref().unwrap_or("_dp_class_ns")
            )
        } else {
            py_expr!("globals()")
        };

        let mut construction = py_expr!(
            r"{define_class_fn:id}({class_ns_fn:id}, {class_ns_outer:expr}, {bases:expr}, {prepare_dict:expr})",
            define_class_fn = capture_region
                .as_ref()
                .map_or(define_class_fn.name.id.as_str(), |(_, original)| original
                    .as_str(),),
            class_ns_fn = class_ns_def.name.id.as_str(),
            class_ns_outer = class_ns_outer,
            bases = bases_tuple,
            prepare_dict = prepare_dict,
        );
        let decorated_class = if let Some((binding, _)) = &preparation {
            let Expr::Call(call) = &mut construction else {
                unreachable!()
            };
            let mut arguments = call.arguments.args.to_vec();
            arguments.push(py_expr!("{binding:id}", binding = binding.as_str()));
            call.arguments.args = arguments.into_boxed_slice();
            let application = py_expr!(
                "_dp_apply_class_decorator({preparation:id}, {class:expr})",
                preparation = binding.as_str(),
                class = construction,
            );
            let node = self
                .semantic_state
                .assign_generated_node_index(&application);
            self.context.record_class_decorator_operation(
                node,
                ClassDecoratorOperation::Apply {
                    declaration: class_def.range,
                },
            );
            application
        } else {
            rewrite_stmt::decorator::rewrite(decorator_list.into(), construction)
        };

        if let Some((binding, _)) = preparation {
            let result = self.context.fresh_annotation_binding("class_result");
            children.push(py_stmt!(
                "{result:id} = {value:expr}",
                result = result.as_str(),
                value = decorated_class,
            ));
            if let Some((start, original)) = &capture_region {
                wrap_class_capture_region(
                    self.context,
                    self.semantic_state,
                    &mut children,
                    *start,
                    original,
                );
            }
            let prepared = children.remove(0);
            let cleanup = py_expr!(
                "_dp_discard_class_decorator({binding:id})",
                binding = binding.as_str(),
            );
            let node = self.semantic_state.assign_generated_node_index(&cleanup);
            self.context.record_class_decorator_operation(
                node,
                ClassDecoratorOperation::DiscardPreparation,
            );
            let delete_binding = |binding: &str| {
                let expression =
                    py_expr!("_dp_delete_class_binding({binding:id})", binding = binding,);
                let node = self.semantic_state.assign_generated_node_index(&expression);
                self.context
                    .record_class_decorator_operation(node, ClassDecoratorOperation::DeleteBinding);
                expression
            };
            let delete_preparation = delete_binding(&binding);
            let delete_result = delete_binding(&result);
            // This cleanup is a control-flow region, not a function-wide
            // exception hook. Internal await/yield-from StopIteration edges
            // retain the preparation; leaving this region clears the actual
            // local or preserved slot before an enclosing handler observes it.
            // The source STORE happens only after class-argument and decorator
            // cleanup. A separate quiet result cleanup also covers failed
            // construction/application and a failing source binding.
            let construction = py_stmt!(
                "try:\n    {body:stmt}\nfinally:\n    try:\n        {cleanup:expr}\n    finally:\n        {delete:expr}",
                body = children,
                cleanup = cleanup,
                delete = delete_preparation,
            );
            return vec![prepared, py_stmt!(
                "try:\n    {construction:stmt}\n    {name:id} = {result:id}\nfinally:\n    {delete:expr}",
                construction = construction,
                name = class_def.name.id.as_str(),
                result = result.as_str(),
                delete = delete_result,
            )];
        }
        children.push(py_stmt!(
            "{name:id} = {value:expr}",
            name = class_def.name.id.as_str(),
            value = decorated_class
        ));
        if let Some((start, original)) = &capture_region {
            wrap_class_capture_region(
                self.context,
                self.semantic_state,
                &mut children,
                *start,
                original,
            );
        }
        children
    }
}

fn wrap_class_capture_region(
    context: &Context,
    semantic_state: &SemanticAstState,
    children: &mut Vec<Stmt>,
    start: usize,
    original: &str,
) {
    let cleanup = py_expr!(
        "_dp_discard_class_construction_captures({original:id})",
        original = original,
    );
    context.record_class_capture_discard(semantic_state.assign_generated_node_index(&cleanup));
    let delete = py_expr!(
        "_dp_delete_class_binding({original:id})",
        original = original
    );
    context.record_class_decorator_operation(
        semantic_state.assign_generated_node_index(&delete),
        ClassDecoratorOperation::DeleteBinding,
    );
    let body = children.split_off(start);
    children.push(py_stmt!(
        "try:\n    {body:stmt}\nfinally:\n    try:\n        {cleanup:expr}\n    finally:\n        {delete:expr}",
        body = body,
        cleanup = cleanup,
        delete = delete,
    ));
}
