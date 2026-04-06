use super::*;
use crate::passes::ast_to_ast::ast_rewrite::Rewrite;
use crate::py_stmt;

// Direct stmt lowerers map one Ruff stmt to either a BlockPy stmt, a terminator,
// or no output at all. They do not need their own AST rewrite helpers.

pub(crate) fn rewrite_raise_stmt(mut raise: ast::StmtRaise) -> Rewrite {
    match (raise.exc.take(), raise.cause.take()) {
        (Some(exc), Some(cause)) => Rewrite::Walk(vec![py_stmt!(
            "raise __soac__.raise_from({exc:expr}, {cause:expr})",
            exc = exc,
            cause = cause,
        )]),
        (exc, None) => {
            raise.exc = exc;
            Rewrite::Unmodified(raise.into())
        }
        (None, Some(_)) => {
            panic!("raise with a cause but without an exception should be impossible")
        }
    }
}

#[cfg(test)]
mod test;
