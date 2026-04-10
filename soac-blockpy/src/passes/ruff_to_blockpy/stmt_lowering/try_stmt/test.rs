use super::super::simplify_stmt_ast_once_for_blockpy;
use super::*;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::util::is_dp_helper_lookup_expr;
use crate::transformer::{walk_expr, Transformer};
use ruff_python_ast::Expr;
use std::collections::HashSet;

#[derive(Default)]
struct HelperCallProbe {
    helpers: HashSet<&'static str>,
}

impl Transformer for HelperCallProbe {
    fn visit_expr(&mut self, expr: &mut Expr) {
        for helper in [
            "current_exception",
            "del_quietly",
            "exception_matches",
            "exceptiongroup_split",
        ] {
            if matches!(expr, Expr::Call(call) if is_dp_helper_lookup_expr(call.func.as_ref(), helper))
            {
                self.helpers.insert(helper);
            }
        }
        walk_expr(self, expr);
    }
}

fn rewritten_try_helper_calls(stmts: &[Stmt]) -> HashSet<&'static str> {
    let mut stmts = stmts.to_vec();
    let mut probe = HelperCallProbe::default();
    for stmt in &mut stmts {
        probe.visit_stmt(stmt);
    }
    probe.helpers
}

fn rewritten_call_names(stmts: &[Stmt]) -> HashSet<String> {
    #[derive(Default)]
    struct CallNameProbe {
        names: HashSet<String>,
    }

    impl Transformer for CallNameProbe {
        fn visit_expr(&mut self, expr: &mut Expr) {
            if let Expr::Call(call) = expr {
                if let Expr::Name(name) = call.func.as_ref() {
                    self.names.insert(name.id.to_string());
                }
            }
            walk_expr(self, expr);
        }
    }

    let mut stmts = stmts.to_vec();
    let mut probe = CallNameProbe::default();
    for stmt in &mut stmts {
        probe.visit_stmt(stmt);
    }
    probe.names
}

#[test]
fn stmt_try_simplify_ast_rewrites_typed_except_before_blockpy_lowering() {
    let stmt = py_stmt!(
        r#"
try:
    work()
except ValueError as exc:
    handle(exc)
"#
    );
    let Stmt::Try(try_stmt) = stmt else {
        panic!("expected try stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::Try(try_stmt));
    let helper_calls = rewritten_try_helper_calls(simplified.as_slice());

    assert!(helper_calls.contains("exception_matches"));
    assert!(helper_calls.contains("current_exception"));
    assert!(helper_calls.contains("del_quietly"));
}

#[test]
fn stmt_try_simplify_ast_preserves_multi_stmt_default_handler_after_typed_handler() {
    let stmt = py_stmt!(
        r#"
try:
    work()
except ValueError:
    typed()
except:
    cleanup()
    recover()
"#
    );
    let Stmt::Try(try_stmt) = stmt else {
        panic!("expected try stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::Try(try_stmt));
    let helper_calls = rewritten_try_helper_calls(simplified.as_slice());
    let call_names = rewritten_call_names(simplified.as_slice());

    assert!(helper_calls.contains("exception_matches"));
    assert!(call_names.contains("typed"));
    assert!(call_names.contains("cleanup"));
    assert!(call_names.contains("recover"));
}

#[test]
fn stmt_try_simplify_ast_rewrites_except_star_before_blockpy_lowering() {
    let stmt = py_stmt!(
        r#"
try:
    work()
except* ValueError as exc:
    handle(exc)
"#
    );
    let Stmt::Try(try_stmt) = stmt else {
        panic!("expected try stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::Try(try_stmt));
    let helper_calls = rewritten_try_helper_calls(simplified.as_slice());

    assert!(helper_calls.contains("exceptiongroup_split"));
    assert!(helper_calls.contains("del_quietly"));
}
