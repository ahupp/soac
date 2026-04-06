use super::*;
use crate::block_py::{Del, HasMeta, Meta, Store, StructuredInstr, WithMeta};
use crate::passes::ast_to_ast::expr_utils::make_tuple;

fn rhs_temp_name(name: &str, ctx: ast::ExprContext) -> ast::ExprName {
    ast::ExprName {
        id: name.into(),
        ctx,
        range: Default::default(),
        node_index: Default::default(),
    }
}

pub(super) fn temp_load_expr<E: RuffToBlockPyExpr>(name: &str) -> E {
    E::from_lowered_expr(Expr::Name(rhs_temp_name(name, ast::ExprContext::Load)))
}

pub(super) fn bind_temp<E: RuffToBlockPyExpr>(
    out: &mut BlockPyStmtBuilder<E>,
    name: String,
    value: E,
) -> E {
    let target = rhs_temp_name(&name, ast::ExprContext::Store);
    let meta = Meta::new(target.node_index.clone(), target.range);
    out.push_stmt(StructuredInstr::Expr(
        Store::new(target, Box::new(value)).with_meta(meta).into(),
    ));
    temp_load_expr(&name)
}

fn delete_temp<E: RuffToBlockPyExpr>(out: &mut BlockPyStmtBuilder<E>, name: String) {
    let target = rhs_temp_name(&name, ast::ExprContext::Del);
    let meta = Meta::new(target.node_index.clone(), target.range);
    out.push_stmt(StructuredInstr::Expr(
        Del::new(target, false).with_meta(meta).into(),
    ));
}

pub(super) fn lower_target_object_with_setup<E: RuffToBlockPyExpr>(
    target_value: Expr,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<E, String> {
    let meta = target_value.meta();
    let maybe_name = target_value.as_name_expr().map(|name| name.id.to_string());
    let value = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
        target_value,
        out,
        loop_ctx,
        next_label_id,
    )?;
    Ok(match maybe_name {
        Some(name) => E::load_deleted_name(meta.node_index, meta.range, name, value),
        None => value,
    })
}

fn lower_assignment_target_into<E>(
    context: &Context,
    target: Expr,
    rhs: E,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr,
{
    match target {
        Expr::Tuple(tuple) => lower_unpack_target_into(
            context,
            tuple.elts,
            rhs,
            out,
            loop_ctx,
            next_label_id,
            UnpackTargetKind::Tuple,
        ),
        Expr::List(list) => lower_unpack_target_into(
            context,
            list.elts,
            rhs,
            out,
            loop_ctx,
            next_label_id,
            UnpackTargetKind::List,
        ),
        Expr::Subscript(ast::ExprSubscript {
            value,
            slice,
            range,
            node_index,
            ..
        }) => {
            let object_value =
                lower_target_object_with_setup(*value, out, loop_ctx, next_label_id)?;
            let object_temp = bind_temp(out, context.fresh("assign_obj"), object_value);
            let index_value =
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                    *slice,
                    out,
                    loop_ctx,
                    next_label_id,
                )?;
            let index_temp = bind_temp(out, context.fresh("assign_index"), index_value);
            out.push_stmt(StructuredInstr::Expr(E::set_item(
                node_index,
                range,
                object_temp,
                index_temp,
                rhs,
            )));
            Ok(())
        }
        Expr::Attribute(ast::ExprAttribute {
            value,
            attr,
            range,
            node_index,
            ..
        }) => {
            let object_value =
                lower_target_object_with_setup(*value, out, loop_ctx, next_label_id)?;
            let object_temp = bind_temp(out, context.fresh("assign_obj"), object_value);
            out.push_stmt(StructuredInstr::Expr(E::set_attr(
                node_index,
                range,
                object_temp,
                attr.to_string(),
                rhs,
            )));
            Ok(())
        }
        Expr::Name(name) => {
            let meta = Meta::new(name.node_index.clone(), name.range);
            out.push_stmt(StructuredInstr::Expr(
                Store::new(name, rhs).with_meta(meta).into(),
            ));
            Ok(())
        }
        other => Err(format!(
            "unsupported assignment target reached BlockPy conversion: {other:?}"
        )),
    }
}

enum UnpackTargetKind {
    Tuple,
    List,
}

fn lower_unpack_target_into<E>(
    context: &Context,
    elts: Vec<Expr>,
    value: E,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
    kind: UnpackTargetKind,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr,
{
    let mut starred_seen = false;
    let mut spec_elts = Vec::new();
    for elt in &elts {
        match elt {
            Expr::Starred(_) => {
                if starred_seen {
                    return Err("unsupported starred assignment target".to_string());
                }
                starred_seen = true;
                spec_elts.push(py_expr!("False"));
            }
            _ => spec_elts.push(py_expr!("True")),
        }
    }

    let spec_expr = make_tuple(spec_elts);
    let unpack_meta = spec_expr.meta();
    let unpacked_name = context.fresh("unpack");
    let unpacked_value = E::helper_call(
        unpack_meta.node_index,
        unpack_meta.range,
        "unpack",
        vec![value, E::from_lowered_expr(spec_expr)],
    );
    let unpacked_temp = bind_temp(out, unpacked_name.clone(), unpacked_value);

    for (index, elt) in elts.into_iter().enumerate() {
        let index_expr = E::from_lowered_expr(py_expr!("{index:literal}", index = index as i64));
        match elt {
            Expr::Starred(ast::ExprStarred { value, .. }) => {
                let item_expr = E::get_item(
                    Default::default(),
                    Default::default(),
                    unpacked_temp.clone(),
                    index_expr,
                );
                let collection_expr = match kind {
                    UnpackTargetKind::Tuple | UnpackTargetKind::List => E::helper_call(
                        Default::default(),
                        Default::default(),
                        "list",
                        vec![item_expr],
                    ),
                };
                lower_assignment_target_into(
                    context,
                    *value,
                    collection_expr,
                    out,
                    loop_ctx,
                    next_label_id,
                )?;
            }
            other => {
                let item_expr = E::get_item(
                    Default::default(),
                    Default::default(),
                    unpacked_temp.clone(),
                    index_expr,
                );
                lower_assignment_target_into(
                    context,
                    other,
                    item_expr,
                    out,
                    loop_ctx,
                    next_label_id,
                )?;
            }
        }
    }

    delete_temp(out, unpacked_name);

    Ok(())
}

fn should_bind_assignment_value(targets: &[Expr]) -> bool {
    targets.len() > 1 || !matches!(targets, [Expr::Name(_)])
}

impl StmtLowerer for ast::StmtAssign {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        single_stmt(self)
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
        let mut value = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
            (*self.value).clone(),
            out,
            loop_ctx,
            next_label_id,
        )?;

        if should_bind_assignment_value(&self.targets) {
            value = bind_temp(out, context.fresh("assign_value"), value);
        }

        for target in self.targets.iter().cloned() {
            lower_assignment_target_into(
                context,
                target,
                value.clone(),
                out,
                loop_ctx,
                next_label_id,
            )?;
        }

        Ok(())
    }
}

pub(super) fn with_target_object_expr(value: Expr) -> Expr {
    if let Expr::Name(name) = &value {
        py_expr!(
            "__soac__.load_deleted_name({name:literal}, {value:expr})",
            name = name.id.as_str(),
            value = value,
        )
    } else {
        value
    }
}

pub(super) fn rewrite_assignment_target<F>(
    target: Expr,
    rhs: Expr,
    out: &mut Vec<Stmt>,
    next_temp: &mut F,
) where
    F: FnMut(&str) -> String,
{
    match target {
        Expr::Tuple(tuple) => rewrite_unpack_target(tuple.elts, rhs, out, next_temp),
        Expr::List(list) => rewrite_unpack_target(list.elts, rhs, out, next_temp),
        Expr::Subscript(ast::ExprSubscript { value, slice, .. }) => {
            out.push(py_stmt!(
                "{obj:expr}[{key:expr}] = {rhs:expr}",
                obj = with_target_object_expr(*value),
                key = *slice,
                rhs = rhs,
            ));
        }
        Expr::Attribute(ast::ExprAttribute { value, attr, .. }) => {
            out.push(py_stmt!(
                "{obj:expr}.{name:id} = {rhs:expr}",
                obj = with_target_object_expr(*value),
                name = attr.as_str(),
                rhs = rhs,
            ));
        }
        Expr::Name(ast::ExprName { id, .. }) => {
            out.push(py_stmt!(
                "{name:id} = {rhs:expr}",
                name = id.as_str(),
                rhs = rhs
            ));
        }
        other => {
            panic!("unsupported assignment target in Ruff AST -> BlockPy lowering: {other:?}");
        }
    }
}

fn rewrite_unpack_target<F>(elts: Vec<Expr>, value: Expr, out: &mut Vec<Stmt>, next_temp: &mut F)
where
    F: FnMut(&str) -> String,
{
    let unpacked_name = next_temp("tmp");
    let unpacked_tmp = py_expr!("{tmp:id}", tmp = unpacked_name.as_str());

    let mut spec_elts = Vec::new();
    let mut starred_seen = false;
    for elt in &elts {
        match elt {
            Expr::Starred(_) => {
                if starred_seen {
                    panic!("unsupported starred with-target assignment");
                }
                starred_seen = true;
                spec_elts.push(py_expr!("False"));
            }
            _ => spec_elts.push(py_expr!("True")),
        }
    }

    out.push(py_stmt!(
        "{tmp:id} = __soac__.unpack({value:expr}, {spec:expr})",
        tmp = unpacked_name.as_str(),
        value = value,
        spec = make_tuple(spec_elts),
    ));

    let starred_index = elts.iter().position(|elt| matches!(elt, Expr::Starred(_)));
    for (idx, elt) in elts.into_iter().enumerate() {
        match elt {
            Expr::Starred(ast::ExprStarred { value, .. }) if Some(idx) == starred_index => {
                rewrite_assignment_target(
                    *value,
                    py_expr!(
                        "__soac__.list({tmp:expr}[{idx:literal}])",
                        tmp = unpacked_tmp.clone(),
                        idx = idx as i64,
                    ),
                    out,
                    next_temp,
                );
            }
            other => {
                rewrite_assignment_target(
                    other,
                    py_expr!(
                        "{tmp:expr}[{idx:literal}]",
                        tmp = unpacked_tmp.clone(),
                        idx = idx as i64,
                    ),
                    out,
                    next_temp,
                );
            }
        }
    }

    out.push(py_stmt!("del {tmp:id}", tmp = unpacked_name.as_str()));
}

pub(crate) fn build_for_target_assign_body<F>(
    target: &Expr,
    rhs: Expr,
    tmp_name: &str,
    next_temp: &mut F,
) -> Vec<Stmt>
where
    F: FnMut(&str) -> String,
{
    let mut out = Vec::new();
    let tmp_expr = py_expr!("{tmp:id}", tmp = tmp_name);
    out.push(py_stmt!("{tmp:id} = {rhs:expr}", tmp = tmp_name, rhs = rhs));
    rewrite_assignment_target(target.clone(), tmp_expr, &mut out, next_temp);
    out.push(py_stmt!("del {tmp:id}", tmp = tmp_name));
    out
}

#[cfg(test)]
mod test;
