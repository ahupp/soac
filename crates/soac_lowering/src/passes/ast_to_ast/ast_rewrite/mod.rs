use std::{backtrace::Backtrace, collections::HashSet, mem::take};

use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use tracing::{enabled, trace, Level};

use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::context::{Context, ScopeFrame};
use crate::passes::ast_to_ast::scope_helpers::ScopeKind;
use crate::ruff_ast::ruff_ast_to_string;
use crate::transformer::{walk_expr, walk_stmt, Transformer};

pub(crate) enum Rewrite {
    Unmodified(Stmt),
    Walk(Vec<Stmt>),
}

pub(crate) struct LoweredExpr {
    pub stmts: Vec<Stmt>,
    pub expr: Expr,
    pub modified: bool,
}

impl LoweredExpr {
    pub(crate) fn modified(expr: Expr, stmts: Vec<Stmt>) -> Self {
        trace!("LoweredExpr::modified {}", Backtrace::capture());
        Self {
            stmts,
            expr,
            modified: true,
        }
    }

    pub(crate) fn unmodified(expr: Expr) -> Self {
        Self {
            stmts: Vec::new(),
            expr,
            modified: false,
        }
    }
}

pub(crate) fn rewrite_once_with_pass<'a>(
    context: &'a Context,
    stmt_pass: Option<&'a dyn StmtRewritePass>,
    expr_pass: Option<&'a dyn ExprRewritePass>,
    body: &mut Suite,
) -> bool {
    let mut rloop = RewriteLoop {
        context,
        stmt_pass,
        expr_pass,
        buf: Vec::new(),
        modified: false,
    };
    rloop.visit_body(body);
    assert!(rloop.buf.is_empty());
    rloop.modified
}

pub(crate) fn rewrite_with_pass<'a>(
    context: &'a Context,
    stmt_pass: Option<&'a dyn StmtRewritePass>,
    expr_pass: Option<&'a dyn ExprRewritePass>,
    body: &mut Suite,
) {
    let pass_name = "rewrite_with_pass";
    let mut iteration = 0usize;
    loop {
        iteration += 1;
        if enabled!(Level::TRACE) {
            trace!(
                "rewrite_with_pass iteration {} start: {}",
                iteration,
                pass_name,
            );
        }
        let modified = rewrite_once_with_pass(context, stmt_pass, expr_pass, body);

        if enabled!(Level::TRACE) {
            trace!(
                "rewrite_with_pass iteration {} end: {} modified={}",
                iteration,
                pass_name,
                modified
            );
        }
        if !modified {
            break;
        }
    }
}

struct RewriteLoop<'a> {
    buf: Vec<Stmt>,
    context: &'a Context,
    stmt_pass: Option<&'a dyn StmtRewritePass>,
    expr_pass: Option<&'a dyn ExprRewritePass>,
    modified: bool,
}

pub(crate) trait StmtRewritePass {
    fn lower_stmt(&self, context: &Context, stmt: Stmt) -> Rewrite;
}

pub(crate) trait BBRewritePass {
    fn lower_bb_stmt(&self, context: &Context, stmt: Stmt) -> Rewrite;
}

impl<T: BBRewritePass + ?Sized> StmtRewritePass for T {
    fn lower_stmt(&self, context: &Context, stmt: Stmt) -> Rewrite {
        self.lower_bb_stmt(context, stmt)
    }
}

pub(crate) trait ExprRewritePass {
    fn lower_expr(&self, context: &Context, expr: Expr) -> LoweredExpr;
}

impl<'a> RewriteLoop<'a> {
    fn flush_buffered(&mut self, mut stmt: Stmt, output: &mut Vec<Stmt>) {
        match &mut stmt {
            Stmt::FunctionDef(func_def) => {
                for decorator in &mut func_def.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(type_params) = func_def.type_params.as_mut() {
                    self.visit_type_params(type_params);
                }
                self.visit_parameters(&mut func_def.parameters);
                if let Some(returns) = func_def.returns.as_mut() {
                    self.visit_annotation(returns);
                }

                let (globals, nonlocals) = collect_declared_bindings(&mut func_def.body);
                let mut frame = ScopeFrame::new(ScopeKind::Function, globals, nonlocals);
                frame.in_async_function = func_def.is_async;
                self.context.push_scope(frame);
                self.visit_body(&mut func_def.body);
                self.context.pop_scope();
            }
            Stmt::ClassDef(class_def) => {
                for decorator in &mut class_def.decorator_list {
                    self.visit_decorator(decorator);
                }
                if let Some(type_params) = class_def.type_params.as_mut() {
                    self.visit_type_params(type_params);
                }
                if let Some(arguments) = class_def.arguments.as_mut() {
                    self.visit_arguments(arguments);
                }

                self.context.push_scope(ScopeFrame::new(
                    ScopeKind::Class,
                    HashSet::new(),
                    HashSet::new(),
                ));
                self.visit_body(&mut class_def.body);
                self.context.pop_scope();
            }
            Stmt::While(while_stmt) => {
                // In BB mode, `while` is lowered structurally in Ruff AST -> BlockPy.
                // Keep control-flow-sensitive test lowering for that phase, but still
                // let helper-scoped expression rewrites synthesize reusable helper
                // functions for comprehensions used by the test expression.
                self.visit_expr(&mut while_stmt.test);
                self.visit_body(&mut while_stmt.body);
                self.visit_body(&mut while_stmt.orelse);
            }
            _ => walk_stmt(self, &mut stmt),
        }

        let buffered = take(&mut self.buf);

        for stmt in buffered.into_iter() {
            self.flush_buffered(stmt, output);
        }
        output.push(stmt);
    }

    fn process_statements(&mut self, initial: Vec<Stmt>) -> Vec<Stmt> {
        let mut output = Vec::new();
        assert!(self.buf.is_empty());

        for stmt in initial.into_iter() {
            if let Some(stmt_pass) = self.stmt_pass {
                let mut before = None;
                if enabled!(Level::TRACE) {
                    before = Some(ruff_ast_to_string(&stmt));
                }
                let res = stmt_pass.lower_stmt(self.context, stmt);
                match res {
                    Rewrite::Unmodified(stmt) => {
                        self.flush_buffered(stmt, &mut output);
                    }
                    Rewrite::Walk(stmts) => {
                        if enabled!(Level::TRACE) {
                            trace!(
                                "rewrite before: \n{} after: \n{}",
                                before.unwrap_or_default(),
                                ruff_ast_to_string(stmts.as_slice()).trim_end()
                            );
                        }
                        self.modified = true;
                        for stmt in stmts {
                            self.flush_buffered(stmt, &mut output);
                        }
                    }
                }
            } else {
                self.flush_buffered(stmt, &mut output);
            }
        }

        output
    }
}

impl<'a> Transformer for RewriteLoop<'a> {
    fn visit_body(&mut self, body: &mut Suite) {
        let saved_buf = take(&mut self.buf);
        let stmts = take(body);
        *body = self.process_statements(stmts);
        self.buf = saved_buf;
    }

    fn visit_expr(&mut self, expr_input: &mut Expr) {
        let expr_pass = if let Some(expr_pass) = self.expr_pass {
            expr_pass
        } else {
            return;
        };
        let original_range = expr_input.range();
        let mut lowered: LoweredExpr;
        let mut current = expr_input.clone();
        let mut modified_any = false;
        let mut iteration = 0usize;
        loop {
            iteration += 1;
            let mut log_input = None;
            if enabled!(Level::TRACE) {
                log_input = Some(ruff_ast_to_string(&current).trim_end().to_string());
            }
            lowered = expr_pass.lower_expr(self.context, current);

            let LoweredExpr {
                stmts,
                expr,
                modified,
            } = lowered;
            if enabled!(Level::TRACE) {
                trace!(
                    "lower_expr iteration={} modified={} \ninput: {}\noutput: \n{}\nstmt: \n{}",
                    iteration,
                    modified,
                    log_input.unwrap_or_default(),
                    ruff_ast_to_string(&expr).trim_end(),
                    ruff_ast_to_string(stmts.as_slice()).trim_end(),
                );
            }
            self.buf.extend(stmts);

            current = expr;

            apply_expr_range(&mut current, original_range);
            if !modified {
                break;
            }
            modified_any = true;
            self.modified = true;
        }

        if !modified_any {
            trace!("walk_expr: {}", ruff_ast_to_string(&current).trim_end());

            walk_expr(self, &mut current);
        }
        *expr_input = current;
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        let rewritten = self.process_statements(vec![stmt.clone()]);
        let [rewritten] = <[Stmt; 1]>::try_from(rewritten).unwrap_or_else(|rewritten| {
            panic!(
                "RewriteLoop::visit_stmt cannot splice {} statements; multi-stmt rewrites must flow through visit_body",
                rewritten.len()
            )
        });
        *stmt = rewritten;
    }
}

fn collect_declared_bindings(body: &Suite) -> (HashSet<String>, HashSet<String>) {
    #[derive(Default)]
    struct Collector {
        globals: HashSet<String>,
        nonlocals: HashSet<String>,
    }

    impl Transformer for Collector {
        fn visit_stmt(&mut self, stmt: &mut Stmt) {
            match stmt {
                Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {
                    return;
                }
                Stmt::Global(ast::StmtGlobal { names, .. }) => {
                    for name in names {
                        self.globals.insert(name.id.to_string());
                    }
                    return;
                }
                Stmt::Nonlocal(ast::StmtNonlocal { names, .. }) => {
                    for name in names {
                        self.nonlocals.insert(name.id.to_string());
                    }
                    return;
                }
                _ => {}
            }
            walk_stmt(self, stmt);
        }
    }

    let mut collector = Collector::default();
    let mut cloned = body.clone();
    collector.visit_body(&mut cloned);
    (collector.globals, collector.nonlocals)
}

fn apply_expr_range(expr: &mut Expr, range: TextRange) {
    match expr {
        Expr::BoolOp(node) => node.range = range,
        Expr::Named(node) => node.range = range,
        Expr::BinOp(node) => node.range = range,
        Expr::UnaryOp(node) => node.range = range,
        Expr::Lambda(node) => node.range = range,
        Expr::If(node) => node.range = range,
        Expr::Dict(node) => node.range = range,
        Expr::Set(node) => node.range = range,
        Expr::ListComp(node) => node.range = range,
        Expr::SetComp(node) => node.range = range,
        Expr::DictComp(node) => node.range = range,
        Expr::Generator(node) => node.range = range,
        Expr::Await(node) => node.range = range,
        Expr::Yield(node) => node.range = range,
        Expr::YieldFrom(node) => node.range = range,
        Expr::Compare(node) => node.range = range,
        Expr::Call(node) => node.range = range,
        Expr::FString(node) => node.range = range,
        Expr::TString(node) => node.range = range,
        Expr::StringLiteral(node) => node.range = range,
        Expr::BytesLiteral(node) => node.range = range,
        Expr::NumberLiteral(node) => node.range = range,
        Expr::BooleanLiteral(node) => node.range = range,
        Expr::NoneLiteral(node) => node.range = range,
        Expr::EllipsisLiteral(node) => node.range = range,
        Expr::Attribute(node) => node.range = range,
        Expr::Subscript(node) => node.range = range,
        Expr::Starred(node) => node.range = range,
        Expr::Name(node) => node.range = range,
        Expr::List(node) => node.range = range,
        Expr::Tuple(node) => node.range = range,
        Expr::Slice(node) => node.range = range,
        Expr::IpyEscapeCommand(node) => node.range = range,
    }
}

#[cfg(test)]
mod test;
