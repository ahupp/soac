use ruff_python_ast::{self as ast, Expr, Stmt};

use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::semantic::SemanticAstState;
use crate::passes::ast_to_ast::util::{is_dp_helper_lookup_expr, is_noarg_call};
use crate::template::py_expr;
use crate::transformer::{walk_expr, walk_stmt, Transformer};

struct MethodRewriteSuperClasscell<'a> {
    first_arg: Option<String>,
    semantic_state: &'a SemanticAstState,
}

impl Transformer for MethodRewriteSuperClasscell<'_> {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                // Creating this def evaluates its decorators and defaults in
                // the containing callable, before the nested body can run.
                for decorator in &mut func_def.decorator_list {
                    self.visit_decorator(decorator);
                }
                self.visit_parameters(&mut func_def.parameters);
                rewrite_method(func_def, self.semantic_state);
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
        if let Expr::Lambda(lambda) = expr {
            // Defaults keep this callable's first argument; the lambda body
            // must select its own, even when it needs no helper statements.
            if let Some(parameters) = &mut lambda.parameters {
                self.visit_parameters(parameters);
            }
            rewrite_lambda(lambda, self.semantic_state);
            return;
        }
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

pub(crate) fn rewrite_explicit_super_classcell(
    class_def: &mut ast::StmtClassDef,
    semantic_state: &SemanticAstState,
) -> bool {
    let mut rewriter = MethodExplicitSuperRewriter {
        needs_class_cell: false,
        semantic_state,
    };
    (&mut rewriter).visit_body(&mut class_def.body);
    rewriter.needs_class_cell
}

struct MethodExplicitSuperRewriter<'a> {
    needs_class_cell: bool,
    semantic_state: &'a SemanticAstState,
}

impl Transformer for MethodExplicitSuperRewriter<'_> {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                for decorator in &mut func_def.decorator_list {
                    self.visit_decorator(decorator);
                }
                self.visit_parameters(&mut func_def.parameters);
                self.needs_class_cell |= rewrite_method(func_def, self.semantic_state);
            }
            Stmt::ClassDef(_) => {}
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        if let Expr::Lambda(lambda) = expr {
            if let Some(parameters) = &mut lambda.parameters {
                self.visit_parameters(parameters);
            }
            self.needs_class_cell |= rewrite_lambda(lambda, self.semantic_state);
            return;
        }
        walk_expr(self, expr);
    }
}

fn first_parameter_name(parameters: &ast::Parameters) -> Option<String> {
    parameters
        .posonlyargs
        .first()
        .or_else(|| parameters.args.first())
        .map(|parameter| parameter.parameter.name.to_string())
}

fn rewrite_lambda(lambda: &mut ast::ExprLambda, semantic_state: &SemanticAstState) -> bool {
    let first_arg = lambda.parameters.as_deref().and_then(first_parameter_name);
    let mut rewriter = MethodRewriteSuperClasscell {
        first_arg,
        semantic_state,
    };
    if let Some(mut body) = semantic_state.lowered_lambda_body(lambda) {
        rewriter.visit_body(&mut body.statements);
        let uses_class_cell = function_uses_class_cell(&mut body.statements, semantic_state);
        semantic_state.replace_lowered_lambda_body(lambda, body);
        uses_class_cell
    } else {
        rewriter.visit_expr(lambda.body.as_mut());
        let mut detector = FunctionUsesClassCellDetector {
            uses_class_cell: false,
            semantic_state,
        };
        detector.visit_expr(lambda.body.as_mut());
        detector.uses_class_cell
    }
}

fn rewrite_method(func_def: &mut ast::StmtFunctionDef, semantic_state: &SemanticAstState) -> bool {
    let first_arg = first_parameter_name(&func_def.parameters);
    let mut transformer = MethodRewriteSuperClasscell {
        first_arg,
        semantic_state,
    };
    for stmt in &mut func_def.body.iter_mut() {
        (&mut transformer).visit_stmt(stmt);
    }
    function_uses_class_cell(&mut func_def.body, semantic_state)
}

struct FunctionUsesClassCellDetector<'a> {
    uses_class_cell: bool,
    semantic_state: &'a SemanticAstState,
}

impl Transformer for FunctionUsesClassCellDetector<'_> {
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
        if let Expr::Lambda(lambda) = expr {
            if let Some(mut body) = self.semantic_state.lowered_lambda_body(lambda) {
                if let Some(parameters) = &mut lambda.parameters {
                    self.visit_parameters(parameters);
                }
                self.visit_body(&mut body.statements);
                return;
            }
        }
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

fn function_uses_class_cell(body: &mut Suite, semantic_state: &SemanticAstState) -> bool {
    let mut detector = FunctionUsesClassCellDetector {
        uses_class_cell: false,
        semantic_state,
    };
    detector.visit_body(body);
    detector.uses_class_cell
}
