use super::*;
use crate::{passes::ast_to_ast::ast_rewrite::Rewrite, py_stmt};

pub(crate) fn rewrite_assert_stmt(ast::StmtAssert { test, msg, .. }: ast::StmtAssert) -> Rewrite {
    Rewrite::Walk(vec![if let Some(msg_expr) = msg {
        py_stmt!(
            "
if __debug__:
    if not {test:expr}:
        raise __soac__.AssertionError({msg:expr})
",
            test = test,
            msg = *msg_expr
        )
    } else {
        py_stmt!(
            "
if __debug__:
    if not {test:expr}:
        raise __soac__.AssertionError
        ",
            test = test
        )
    }])
}

#[cfg(test)]
mod test;
