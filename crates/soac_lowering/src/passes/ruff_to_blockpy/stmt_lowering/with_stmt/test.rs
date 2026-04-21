use super::super::{lower_instr_for_test, simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::util::is_dp_helper_lookup_expr;
use crate::passes::ruff_to_blockpy::test_name_gen;
use crate::transformer::{walk_expr, Transformer};
use ruff_python_ast::{CmpOp, Expr};

#[derive(Default)]
struct WithRewriteShapeProbe {
    has_helper_identity_test: bool,
    has_native_is_not_none_test: bool,
}

impl Transformer for WithRewriteShapeProbe {
    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Call(call) => {
                self.has_helper_identity_test |=
                    is_dp_helper_lookup_expr(call.func.as_ref(), "is_not");
            }
            Expr::Compare(compare) => {
                self.has_native_is_not_none_test |= compare.ops.len() == 1
                    && compare.ops[0] == CmpOp::IsNot
                    && matches!(compare.comparators.first(), Some(Expr::NoneLiteral(_)));
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

fn probe_rewritten_with_shape(stmts: &[Stmt]) -> WithRewriteShapeProbe {
    let mut stmts = stmts.to_vec();
    let mut probe = WithRewriteShapeProbe::default();
    for stmt in &mut stmts {
        probe.visit_stmt(stmt);
    }
    probe
}

#[test]
fn stmt_with_simplify_ast_desugars_before_blockpy_lowering() {
    let stmt = py_stmt!("with cm:\n    body()");
    let Stmt::With(with_stmt) = stmt else {
        panic!("expected with stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::With(with_stmt));

    assert!(!matches!(simplified.as_slice(), [Stmt::With(_)]));
}

#[test]
fn stmt_with_simplify_ast_uses_native_identity_test() {
    let stmt = py_stmt!("async def f(cm):\n    async with cm:\n        body()");
    let Stmt::FunctionDef(func) = stmt else {
        panic!("expected function def");
    };
    let Stmt::With(with_stmt) = func.body[0].clone() else {
        panic!("expected with stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::With(with_stmt));
    let probe = probe_rewritten_with_shape(simplified.as_slice());

    assert!(!probe.has_helper_identity_test);
    assert!(probe.has_native_is_not_none_test);
}

#[test]
#[should_panic(expected = "With should be lowered before Ruff AST -> BlockPy stmt-list conversion")]
fn stmt_with_to_blockpy_rejects_sequence_only_stmt_lowering() {
    let stmt = py_stmt!("with cm:\n    body()");
    let with_stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let _ = lower_instr_for_test(&context, &with_stmt, &name_gen, &mut out, None);
}
