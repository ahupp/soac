use super::*;
use crate::block_py::{Del, HasMeta, Meta, Store, WithMeta};
use crate::passes::ast_to_ast::expr_utils::make_tuple;
use crate::passes::InstrRuff;

fn rhs_temp_name(name: &str) -> ast::name::Name {
    name.into()
}

pub(super) fn temp_load_expr<E: RuffToBlockPyExpr>(
    name: &str,
) -> E {
    E::from_lowered_expr(crate::passes::ast_to_instr::from_ast_expr(Expr::Name(ast::ExprName {
        id: rhs_temp_name(name),
        ctx: ast::ExprContext::Load,
        range: Default::default(),
        node_index: Default::default(),
    })))
}

pub(super) fn bind_temp<E: RuffToBlockPyExpr>(
    out: &mut BlockPyStmtBuilder<E>,
    name: String,
    value: E,
) -> E {
    let target = rhs_temp_name(&name);
    let meta = Meta::synthetic();
    out.push_stmt(Store::new(target, value).with_meta(meta).into());
    temp_load_expr(&name)
}

fn delete_temp<E: RuffToBlockPyExpr>(
    out: &mut BlockPyStmtBuilder<E>,
    name: String,
) {
    let target = rhs_temp_name(&name);
    let meta = Meta::synthetic();
    out.push_stmt(Del::new(target, false).with_meta(meta).into());
}

pub(super) fn lower_target_object_with_setup<
    E: RuffToBlockPyExpr,
>(
    target_value: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<E, String> {
    let meta = target_value.meta();
    let maybe_name = match &target_value {
        InstrRuff::ExprName(name) => Some(name.id.to_string()),
        _ => None,
    };
    let value = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
        target_value,
        out,
        loop_ctx,
    )?;
    Ok(match maybe_name {
        Some(name) => E::load_deleted_name(meta.node_index, meta.range, name, value),
        None => value,
    })
}

fn lower_assignment_target_into<E>(
    context: &Context,
    target: InstrRuff,
    rhs: E,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr,
{
    match target {
        InstrRuff::ExprTuple(tuple) => lower_unpack_target_into(
            context,
            tuple.elts,
            rhs,
            out,
            loop_ctx,
            UnpackTargetKind::Tuple,
        ),
        InstrRuff::ExprList(list) => lower_unpack_target_into(
            context,
            list.elts,
            rhs,
            out,
            loop_ctx,
            UnpackTargetKind::List,
        ),
        InstrRuff::ExprSubscript(target) => {
            let meta = target.meta();
            let object_value = lower_target_object_with_setup(*target.value, out, loop_ctx)?;
            let object_temp = bind_temp(out, context.fresh("assign_obj"), object_value);
            let index_value =
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                    *target.slice,
                    out,
                    loop_ctx,
                )?;
            let index_temp = bind_temp(out, context.fresh("assign_index"), index_value);
            out.push_stmt(E::set_item(
                meta.node_index,
                meta.range,
                object_temp,
                index_temp,
                rhs,
            ));
            Ok(())
        }
        InstrRuff::ExprAttribute(target) => {
            let meta = target.meta();
            let object_value = lower_target_object_with_setup(*target.value, out, loop_ctx)?;
            let object_temp = bind_temp(out, context.fresh("assign_obj"), object_value);
            out.push_stmt(E::set_attr(
                meta.node_index,
                meta.range,
                object_temp,
                target.attr.to_string(),
                rhs,
            ));
            Ok(())
        }
        InstrRuff::ExprName(name) => {
            let meta = name.meta();
            out.push_stmt(Store::new(name.id.clone(), rhs).with_meta(meta).into());
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
    elts: Vec<InstrRuff>,
    value: E,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    kind: UnpackTargetKind,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr,
{
    let mut starred_seen = false;
    let mut spec_elts = Vec::new();
    for elt in &elts {
        match elt {
            InstrRuff::ExprStarred(_) => {
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
        vec![
            value,
            E::from_lowered_expr(crate::passes::ast_to_instr::from_ast_expr(spec_expr)),
        ],
    );
    let unpacked_temp = bind_temp(out, unpacked_name.clone(), unpacked_value);

    for (index, elt) in elts.into_iter().enumerate() {
        let index_expr = E::from_lowered_expr(crate::passes::ast_to_instr::from_ast_expr(py_expr!(
            "{index:literal}",
            index = index as i64
        )));
        match elt {
            InstrRuff::ExprStarred(starred) => {
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
                    *starred.value,
                    collection_expr,
                    out,
                    loop_ctx,
                )?;
            }
            other => {
                let item_expr = E::get_item(
                    Default::default(),
                    Default::default(),
                    unpacked_temp.clone(),
                    index_expr,
                );
                lower_assignment_target_into(context, other, item_expr, out, loop_ctx)?;
            }
        }
    }

    delete_temp(out, unpacked_name);

    Ok(())
}

fn should_bind_assignment_value(targets: &[InstrRuff]) -> bool {
    targets.len() > 1 || !matches!(targets, [InstrRuff::ExprName(_)])
}

pub(crate) fn lower_assign_instr_into<E>(
    context: &Context,
    stmt: &crate::block_py::StmtAssign<InstrRuff>,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr,
{
    let mut value = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
        (*stmt.value).clone(),
        out,
        loop_ctx,
    )?;

    if should_bind_assignment_value(&stmt.targets) {
        value = bind_temp(out, context.fresh("assign_value"), value);
    }

    for target in stmt.targets.iter().cloned() {
        lower_assignment_target_into(context, target, value.clone(), out, loop_ctx)?;
    }

    Ok(())
}

pub(crate) fn build_for_target_assign_body(
    target: InstrRuff,
    rhs: InstrRuff,
    tmp_name: &str,
) -> Vec<InstrRuff> {
    let tmp_name_expr = |ctx| {
        crate::passes::ast_to_instr::from_ast_expr(Expr::Name(ast::ExprName {
            id: rhs_temp_name(tmp_name),
            ctx,
            range: Default::default(),
            node_index: Default::default(),
        }))
    };
    vec![
        crate::block_py::StmtAssign::new(vec![tmp_name_expr(ast::ExprContext::Store)], rhs).into(),
        crate::block_py::StmtAssign::new(vec![target], tmp_name_expr(ast::ExprContext::Load))
            .into(),
        crate::block_py::StmtDelete::new(vec![tmp_name_expr(ast::ExprContext::Del)]).into(),
    ]
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

#[cfg(test)]
mod test;
