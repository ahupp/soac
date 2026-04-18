use ruff_python_ast::{self as ast, Expr, Stmt};

use crate::passes::ast_to_ast::body::Suite;
use crate::transformer::{walk_expr, walk_stmt, Transformer};
use crate::{
    passes::ast_to_ast::util::{is_dp_helper_lookup_expr, is_noarg_call},
    py_expr,
};

struct MethodRewriteSuperClasscell {
    first_arg: Option<String>,
}

impl Transformer for MethodRewriteSuperClasscell {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                rewrite_method(func_def);
                return;
            }
            Stmt::Delete(ast::StmtDelete { targets, .. }) => {
                if targets.iter().any(|target| {
                    matches!(
                        target,
                        Expr::Name(ast::ExprName { id, .. }) if id.as_str() == "__class__"
                    )
                }) {
                    return;
                }
            }
            Stmt::Nonlocal(ast::StmtNonlocal { names, .. }) => {
                if names.iter().any(|name| name.id.as_str() == "__class__") {
                    return;
                }
            }
            _ => {}
        }

        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Call(_) => {
                if is_noarg_call("super", expr) {
                    *expr = match &self.first_arg {
                        Some(arg) => py_expr!(
                            "__soac__.call_super(super, __soac__.cell_ref(\"__class__\"), {arg:id})",
                            arg = arg.as_str()
                        ),
                        None => py_expr!("__soac__.call_super_noargs(super)"),
                    };
                    return;
                }
            }
            Expr::Name(ast::ExprName { id, .. }) => {
                if id.as_str() == "__class__" {
                    return;
                }
            }
            _ => {}
        }

        walk_expr(self, expr);
    }
}

fn is_dp_call(expr: &Expr, name: &str) -> bool {
    let Expr::Call(ast::ExprCall { func, .. }) = expr else {
        return false;
    };
    is_dp_helper_lookup_expr(func, name)
}

fn is_super_call(expr: &Expr) -> bool {
    let Expr::Call(ast::ExprCall { func, .. }) = expr else {
        return false;
    };
    matches!(
        func.as_ref(),
        Expr::Name(ast::ExprName { id, .. }) if id.as_str() == "super"
    )
}

pub fn rewrite_explicit_super_classcell(class_def: &mut ast::StmtClassDef) -> bool {
    let mut rewriter = MethodExplicitSuperRewriter {
        needs_class_cell: false,
    };
    (&mut rewriter).visit_body(&mut class_def.body);
    rewriter.needs_class_cell
}

struct MethodExplicitSuperRewriter {
    needs_class_cell: bool,
}

impl Transformer for MethodExplicitSuperRewriter {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                self.needs_class_cell |= rewrite_method(func_def);
            }
            Stmt::ClassDef(_) => {}
            _ => walk_stmt(self, stmt),
        }
    }
}

fn rewrite_method(func_def: &mut ast::StmtFunctionDef) -> bool {
    let first_arg = func_def
        .parameters
        .posonlyargs
        .first()
        .map(|a| a.parameter.name.to_string())
        .or_else(|| {
            func_def
                .parameters
                .args
                .first()
                .map(|a| a.parameter.name.to_string())
        });

    let mut transformer = MethodRewriteSuperClasscell { first_arg };
    for stmt in &mut func_def.body.iter_mut() {
        (&mut transformer).visit_stmt(stmt);
    }
    function_uses_class_cell(&mut func_def.body)
}

#[derive(Default)]
struct FunctionUsesClassCellDetector {
    uses_class_cell: bool,
}

impl Transformer for FunctionUsesClassCellDetector {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::Delete(ast::StmtDelete { targets, .. }) => {
                if targets.iter().any(|target| {
                    matches!(
                        target,
                        Expr::Name(ast::ExprName { id, .. }) if id.as_str() == "__class__"
                    )
                }) {
                    self.uses_class_cell = true;
                    return;
                }
            }
            Stmt::Nonlocal(ast::StmtNonlocal { names, .. }) => {
                if names.iter().any(|name| name.id.as_str() == "__class__") {
                    self.uses_class_cell = true;
                    return;
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Call(_) => {
                if is_super_call(expr)
                    || is_dp_call(expr, "call_super")
                    || is_dp_call(expr, "call_super_noargs")
                {
                    self.uses_class_cell = true;
                    return;
                }
            }
            Expr::Name(ast::ExprName { id, .. }) => {
                if id.as_str() == "__class__" {
                    self.uses_class_cell = true;
                    return;
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

fn function_uses_class_cell(body: &mut Suite) -> bool {
    let mut detector = FunctionUsesClassCellDetector::default();
    detector.visit_body(body);
    detector.uses_class_cell
}
