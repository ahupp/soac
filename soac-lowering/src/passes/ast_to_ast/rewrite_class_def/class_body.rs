use std::mem::take;

use ruff_python_ast::Stmt;

use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::rewrite_class_def::{class_def_to_create_class_fn, method};
use crate::passes::ast_to_ast::rewrite_stmt;
use crate::passes::ast_to_ast::semantic::{SemanticAstState, SemanticScope, SemanticScopeKind};
use crate::transformer::{walk_stmt, Transformer};
use crate::{py_expr, py_stmt};

pub fn rewrite_class_body_scopes(
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
        *body = rewritten;
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func_def) => {
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
}

impl<'a> ClassBodyScopeRewriter<'a> {
    fn rewrite_stmt_list(&mut self, stmt: Stmt) -> Vec<Stmt> {
        let Stmt::ClassDef(mut class_def) = stmt else {
            let mut stmt = stmt;
            self.visit_stmt(&mut stmt);
            return vec![stmt];
        };

        let decorator_list = take(&mut class_def.decorator_list);
        let needs_class_cell = method::rewrite_explicit_super_classcell(&mut class_def);

        let class_scope = self
            .scope
            .child_scope_for_class(&class_def)
            .expect("no child scope for class");

        let mut class_rewriter =
            ClassBodyScopeRewriter::new(self.context, class_scope.clone(), self.semantic_state);
        class_rewriter.visit_body(&mut class_def.body);
        let mut hoisted = class_rewriter.take_hoisted();

        let (class_ns_def, define_class_fn, bases_tuple, prepare_dict) =
            class_def_to_create_class_fn(
                self.context,
                &mut class_def,
                class_scope.qualname().to_string(),
                needs_class_cell,
            );
        self.semantic_state
            .register_function_scope_override(&class_ns_def, class_scope.clone());
        self.semantic_state
            .register_function_scope_override(&define_class_fn, self.scope.clone());

        hoisted.push(class_ns_def.clone().into());

        let mut children = Vec::new();
        // Keep nested class namespace helpers in lexical scope with the
        // matching `_dp_define_class_*` call site. Hoisting these out
        // of class bodies makes helper resolution depend on module
        // globals, which breaks once top-level code is wrapped in
        // `_dp_module_init`.
        children.append(&mut hoisted);
        children.push(define_class_fn.clone().into());

        let class_ns_outer = if matches!(self.scope.kind(), SemanticScopeKind::Class) {
            py_expr!("_dp_class_ns")
        } else {
            py_expr!("globals()")
        };

        let decorated_class = rewrite_stmt::decorator::rewrite(
            decorator_list,
            py_expr!(
                r"{define_class_fn:id}({class_ns_fn:id}, {class_ns_outer:expr}, {bases:expr}, {prepare_dict:expr})",
                define_class_fn = define_class_fn.name.id.as_str(),
                class_ns_fn = class_ns_def.name.id.as_str(),
                class_ns_outer = class_ns_outer,
                bases = bases_tuple,
                prepare_dict = prepare_dict,
            ),
        );

        children.push(py_stmt!(
            "{name:id} = {value:expr}",
            name = class_def.name.id.as_str(),
            value = decorated_class
        ));
        children
    }
}
