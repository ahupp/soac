use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::context::Context;
use crate::transformer::{walk_expr, walk_parameter, walk_stmt, Transformer};
use ruff_python_ast::{self as ast, name::Name, Expr, ExprContext, Stmt};

pub fn rewrite_private_names(_context: &Context, body: &mut Suite) {
    let mut rewriter = PrivateRewriter { class_name: None };
    rewriter.visit_body(body);
}

#[derive(Default)]
struct PrivateRewriter {
    class_name: Option<String>,
}

impl PrivateRewriter {
    pub fn maybe_mangle(&self, attr: &str) -> Option<String> {
        let Some(mut class_name) = self.class_name.as_ref().map(|s| s.as_str()) else {
            return None;
        };

        if !attr.starts_with("__") || attr.ends_with("__") {
            return None;
        }

        while class_name.starts_with('_') {
            class_name = &class_name[1..];
        }
        if class_name.is_empty() {
            return None;
        }

        let ret = format!("_{}{}", class_name, attr);
        Some(ret)
    }

    fn mangle_identifier(&self, name: &mut ast::Identifier) {
        if let Some(mangled) = self.maybe_mangle(name.as_str()) {
            name.id = Name::new(mangled);
        }
    }

    fn mangle_name(&self, name: &mut Name) {
        if let Some(mangled) = self.maybe_mangle(name.as_str()) {
            *name = Name::new(mangled);
        }
    }
}

impl Transformer for PrivateRewriter {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::ClassDef(ast::StmtClassDef { name, body, .. }) => {
                PrivateRewriter {
                    class_name: Some(name.to_string()),
                }
                .visit_body(body);
            }
            Stmt::Global(ast::StmtGlobal { names, .. }) => {
                for name in names {
                    self.mangle_identifier(name);
                }
            }
            Stmt::Nonlocal(ast::StmtNonlocal { names, .. }) => {
                for name in names {
                    self.mangle_identifier(name);
                }
            }
            Stmt::FunctionDef(ast::StmtFunctionDef { name, .. }) => {
                self.mangle_name(&mut name.id);
                walk_stmt(self, stmt);
            }
            _ => {
                walk_stmt(self, stmt);
            }
        }
    }

    fn visit_parameter(&mut self, parameter: &mut ast::Parameter) {
        self.mangle_identifier(&mut parameter.name);
        walk_parameter(self, parameter);
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Name(ast::ExprName { id, ctx, .. })
                if matches!(
                    ctx,
                    ExprContext::Load | ExprContext::Store | ExprContext::Del
                ) =>
            {
                self.mangle_name(id);
            }
            Expr::Attribute(ast::ExprAttribute { attr, .. }) => {
                self.mangle_identifier(attr);
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}
