use crate::{passes::ast_to_ast::body::Suite, transformer::Transformer};
use ruff_python_ast::{self as ast, Expr, Stmt};

pub(crate) struct Flattener;

impl Flattener {
    fn visit_body(&mut self, body: &mut Suite) {
        let mut i = 0;
        while i < body.len() {
            self.visit_stmt(&mut body[i]);
            if let Stmt::If(ast::StmtIf {
                test,
                body: inner,
                elif_else_clauses,
                ..
            }) = &mut body[i]
            {
                if elif_else_clauses.is_empty()
                    && matches!(
                        test.as_ref(),
                        Expr::BooleanLiteral(ast::ExprBooleanLiteral { value: true, .. })
                    )
                {
                    let replacement = std::mem::take(inner);
                    body.splice(i..=i, replacement);
                    continue;
                }
            }
            i += 1;
        }
    }
}

fn remove_placeholder_pass(body: &mut Suite) {
    if body.len() == 1 {
        if let Stmt::Pass(ast::StmtPass { range, .. }) = &body[0] {
            if range.is_empty() {
                body.clear();
            }
        }
    }
}

impl Transformer for Flattener {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::If(ast::StmtIf {
                body,
                elif_else_clauses,
                ..
            }) => {
                self.visit_body(body);
                remove_placeholder_pass(body);
                for clause in elif_else_clauses.iter_mut() {
                    self.visit_body(&mut clause.body);
                    remove_placeholder_pass(&mut clause.body);
                }
            }
            Stmt::For(ast::StmtFor {
                body: inner,
                orelse,
                ..
            }) => {
                self.visit_body(inner);
                remove_placeholder_pass(inner);
                self.visit_body(orelse);
                remove_placeholder_pass(orelse);
            }
            Stmt::While(ast::StmtWhile {
                body: inner,
                orelse,
                ..
            }) => {
                self.visit_body(inner);
                remove_placeholder_pass(inner);
                self.visit_body(orelse);
                remove_placeholder_pass(orelse);
            }
            Stmt::Try(ast::StmtTry {
                body: inner,
                handlers,
                orelse,
                finalbody,
                ..
            }) => {
                self.visit_body(inner);
                remove_placeholder_pass(inner);
                for handler in handlers.iter_mut() {
                    let ast::ExceptHandler::ExceptHandler(ast::ExceptHandlerExceptHandler {
                        body,
                        ..
                    }) = handler;
                    self.visit_body(body);
                    remove_placeholder_pass(body);
                }
                self.visit_body(orelse);
                remove_placeholder_pass(orelse);
                self.visit_body(finalbody);
                remove_placeholder_pass(finalbody);
            }
            Stmt::FunctionDef(ast::StmtFunctionDef { body: inner, .. }) => {
                self.visit_body(inner);
                remove_placeholder_pass(inner);
            }
            _ => {}
        }
    }
}

pub fn flatten(stmts: &mut Suite) {
    let mut flattener = Flattener;
    (&mut flattener).visit_body(stmts);
}

#[cfg(test)]
mod test;
