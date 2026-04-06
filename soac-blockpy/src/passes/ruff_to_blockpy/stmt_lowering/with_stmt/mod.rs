use super::assign_stmt::rewrite_assignment_target;
use super::*;

impl StmtLowerer for ast::StmtWith {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        desugar_structured_with_stmt_for_blockpy(self)
    }

    fn plan_head(self, _context: &Context) -> StmtSequenceHeadPlan {
        StmtSequenceHeadPlan::With(self)
    }

    fn to_blockpy<E>(
        &self,
        context: &Context,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
        next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        lower_stmt_via_simplify(context, self, out, loop_ctx, next_label_id)
    }
}

fn maybe_placeholder(expr: Expr) -> (Vec<Stmt>, Expr, bool) {
    if is_simple(&expr) && !matches!(&expr, Expr::StringLiteral(_) | Expr::BytesLiteral(_)) {
        return (Vec::new(), expr, false);
    }
    let tmp = fresh_name("tmp");
    let stmt = py_stmt!("{tmp:id} = {expr:expr}", tmp = tmp.as_str(), expr = expr);
    (vec![stmt], py_expr!("{tmp:id}", tmp = tmp.as_str()), true)
}

pub(super) fn desugar_structured_with_stmt_for_blockpy(with_stmt: ast::StmtWith) -> Vec<Stmt> {
    if with_stmt.items.is_empty() {
        let mut body = with_stmt.body;
        return std::mem::take(&mut body);
    }

    let ast::StmtWith {
        items,
        body,
        is_async,
        ..
    } = with_stmt;

    let mut body = body;
    let mut lowered_body: Vec<Stmt> = std::mem::take(&mut body);

    for ast::WithItem {
        context_expr,
        optional_vars,
        ..
    } in items.into_iter().rev()
    {
        let target = optional_vars.map(|var| *var);
        let exit_name = fresh_name("with_exit");
        let ok_name = fresh_name("with_ok");
        let reraise_name = fresh_name("with_reraise");
        let (ctx_placeholder_stmt, ctx_expr, ctx_was_placeholder) = maybe_placeholder(context_expr);
        let ctx_cleanup = if ctx_was_placeholder {
            vec![py_stmt!("{ctx:expr} = None", ctx = ctx_expr.clone())]
        } else {
            Vec::new()
        };

        let enter_value = if is_async {
            py_expr!(
                "await __soac__.asynccontextmanager_aenter({ctx:expr})",
                ctx = ctx_expr.clone()
            )
        } else {
            py_expr!(
                "__soac__.contextmanager_enter({ctx:expr})",
                ctx = ctx_expr.clone()
            )
        };
        let enter_stmt = if let Some(target) = target.clone() {
            let mut enter_stmts = Vec::new();
            let mut next_temp = |prefix: &str| fresh_name(prefix);
            rewrite_assignment_target(target, enter_value, &mut enter_stmts, &mut next_temp);
            enter_stmts
        } else {
            vec![py_stmt!("{value:expr}", value = enter_value)]
        };

        lowered_body = if is_async {
            crate::py_stmts!(
                r#"
{ctx_placeholder_stmt:stmt}
{exit_name:id} = __soac__.asynccontextmanager_get_aexit({ctx_expr:expr})
{enter_stmt:stmt}
{ok_name:id} = True
try:
    {body:stmt}
except BaseException:
    {ok_name:id} = False
    {reraise_name:id} = await __soac__.asynccontextmanager_exit({exit_name:id}, __soac__.current_exception())
    if {reraise_name:id} is not None:
        raise {reraise_name:id}
finally:
    if {ok_name:id}:
        await __soac__.asynccontextmanager_exit({exit_name:id}, None)
    {exit_name:id} = None
    {ctx_cleanup:stmt}
"#,
                ctx_placeholder_stmt = ctx_placeholder_stmt,
                ctx_expr = ctx_expr.clone(),
                enter_stmt = enter_stmt,
                body = lowered_body,
                exit_name = exit_name.as_str(),
                ok_name = ok_name.as_str(),
                reraise_name = reraise_name.as_str(),
                ctx_cleanup = ctx_cleanup,
            )
        } else {
            crate::py_stmts!(
                r#"
{ctx_placeholder_stmt:stmt}
{exit_name:id} = __soac__.contextmanager_get_exit({ctx_expr:expr})
{enter_stmt:stmt}
{ok_name:id} = True
try:
    {body:stmt}
except BaseException:
    {ok_name:id} = False
    __soac__.contextmanager_exit({exit_name:id}, __soac__.current_exception())
finally:
    if {ok_name:id}:
        __soac__.contextmanager_exit({exit_name:id}, None)
    {exit_name:id} = None
    {ctx_cleanup:stmt}
"#,
                ctx_placeholder_stmt = ctx_placeholder_stmt,
                ctx_expr = ctx_expr.clone(),
                enter_stmt = enter_stmt,
                body = lowered_body,
                exit_name = exit_name.as_str(),
                ok_name = ok_name.as_str(),
                ctx_cleanup = ctx_cleanup,
            )
        };
    }

    lowered_body
}

pub(crate) fn lower_with_stmt_sequence<F, E>(
    context: &Context,
    with_stmt: ast::StmtWith,
    remaining_stmts: &[Stmt],
    targets: RegionTargets,
    linear: Vec<Stmt>,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    _needs_finally_return_flow: bool,
    lower_sequence: &mut F,
) -> BlockLabel
where
    F: FnMut(&[Stmt], RegionTargets, &mut Vec<LoweredBlockPyBlock<E>>) -> BlockLabel,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    if with_stmt.items.is_empty() {
        let jump_label = if linear.is_empty() {
            None
        } else {
            Some(name_gen.next_block_name())
        };
        return lower_expanded_stmt_sequence(
            context,
            {
                let mut body = with_stmt.body;
                std::mem::take(&mut body)
            },
            remaining_stmts,
            targets,
            linear,
            blocks,
            jump_label,
            lower_sequence,
        );
    }

    let jump_label = if linear.is_empty() {
        None
    } else {
        Some(name_gen.next_block_name())
    };
    lower_expanded_stmt_sequence(
        context,
        desugar_structured_with_stmt_for_blockpy(with_stmt),
        remaining_stmts,
        targets,
        linear,
        blocks,
        jump_label,
        lower_sequence,
    )
}

#[cfg(test)]
mod test;
