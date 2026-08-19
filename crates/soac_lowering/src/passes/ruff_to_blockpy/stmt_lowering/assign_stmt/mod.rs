use super::*;
use crate::block_py::{Del, HasMeta, Meta, Store, WithMeta};
use crate::passes::ast_to_ast::expr_utils::make_tuple;
use crate::passes::ruff_to_blockpy::expr_lowering::{
    try_lower_boolop_assign_direct, ScopedSetupExprLowerer,
};
use crate::passes::InstrRuff;

fn rhs_temp_name(name: &str) -> ast::name::Name {
    name.into()
}

pub(super) fn temp_load_expr<E: RuffToBlockPyExpr>(name: &str) -> E {
    E::from_lowered_expr(crate::passes::ast_to_instr::from_ast_expr(Expr::Name(
        ast::ExprName {
            id: rhs_temp_name(name),
            ctx: ast::ExprContext::Load,
            range: Default::default(),
            node_index: Default::default(),
        },
    )))
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

fn delete_temp<E: RuffToBlockPyExpr>(out: &mut BlockPyStmtBuilder<E>, name: String) {
    let target = rhs_temp_name(&name);
    let meta = Meta::synthetic();
    out.push_stmt(Del::new(target, false).with_meta(meta).into());
}

pub(super) fn lower_target_object_with_setup<E: RuffToBlockPyExpr>(
    context: &Context,
    target_value: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<E, String> {
    crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
        context,
        target_value,
        out,
        loop_ctx,
    )
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
            let can_forward_object =
                is_forwardable_assignment_target_component(context, target.value.as_ref());
            let can_forward_index =
                is_forwardable_assignment_target_component(context, target.slice.as_ref());
            let object_value =
                lower_target_object_with_setup(context, *target.value, out, loop_ctx)?;
            let (object, object_temp_name) = if can_forward_object {
                (object_value, None)
            } else {
                let object_temp_name = context.fresh("assign_obj");
                (
                    bind_temp(out, object_temp_name.clone(), object_value),
                    Some(object_temp_name),
                )
            };
            let index_value =
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
                    context,
                    *target.slice,
                    out,
                    loop_ctx,
                )?;
            let (index, index_temp_name) = if can_forward_index {
                (index_value, None)
            } else {
                let index_temp_name = context.fresh("assign_index");
                (
                    bind_temp(out, index_temp_name.clone(), index_value),
                    Some(index_temp_name),
                )
            };
            out.push_stmt(E::set_item(meta.node_index, meta.range, object, index, rhs));
            if let Some(name) = index_temp_name {
                delete_temp(out, name);
            }
            if let Some(name) = object_temp_name {
                delete_temp(out, name);
            }
            Ok(())
        }
        InstrRuff::ExprAttribute(target) => {
            let meta = target.meta();
            let can_forward_object =
                is_forwardable_assignment_target_component(context, target.value.as_ref());
            let object_value =
                lower_target_object_with_setup(context, *target.value, out, loop_ctx)?;
            let (object, object_temp_name) = if can_forward_object {
                (object_value, None)
            } else {
                let object_temp_name = context.fresh("assign_obj");
                (
                    bind_temp(out, object_temp_name.clone(), object_value),
                    Some(object_temp_name),
                )
            };
            out.push_stmt(E::set_attr(
                meta.node_index,
                meta.range,
                object,
                target.attr.to_string(),
                rhs,
            ));
            if let Some(name) = object_temp_name {
                delete_temp(out, name);
            }
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

fn is_forwardable_assignment_target_component(context: &Context, value: &InstrRuff) -> bool {
    let InstrRuff::ExprName(name) = value else {
        return false;
    };
    context
        .current_value_forwarding_locals()
        .contains(name.id.as_str())
}

fn is_no_raise_assignment_component(context: &Context, value: &InstrRuff) -> bool {
    let InstrRuff::ExprName(name) = value else {
        return false;
    };
    context.current_no_raise_locals().contains(name.id.as_str())
}

fn can_forward_assignment_rhs_without_temp(
    context: &Context,
    targets: &[InstrRuff],
    value: &InstrRuff,
) -> bool {
    let [target] = targets else {
        return false;
    };
    if !is_no_raise_assignment_component(context, value) {
        return false;
    }
    match target {
        InstrRuff::ExprAttribute(target) => {
            is_no_raise_assignment_component(context, target.value.as_ref())
        }
        InstrRuff::ExprSubscript(target) => {
            is_no_raise_assignment_component(context, target.value.as_ref())
                && is_forwardable_assignment_target_component(context, target.slice.as_ref())
        }
        _ => false,
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

    let arity = elts.len();
    let (helper_name, unpack_argument) = if starred_seen {
        ("unpack", make_tuple(spec_elts))
    } else {
        (
            "unpack_fixed",
            py_expr!("{arity:literal}", arity = arity as i64),
        )
    };
    let unpack_meta = unpack_argument.meta();
    let unpacked_name = context.fresh("unpack");
    let unpacked_value = E::helper_call(
        unpack_meta.node_index,
        unpack_meta.range,
        helper_name,
        vec![
            value,
            E::from_lowered_expr(crate::passes::ast_to_instr::from_ast_expr(unpack_argument)),
        ],
    );
    let unpacked_temp = bind_temp(out, unpacked_name.clone(), unpacked_value);

    for (index, elt) in elts.into_iter().enumerate() {
        let index_expr = E::from_lowered_expr(crate::passes::ast_to_instr::from_ast_expr(
            py_expr!("{index:literal}", index = index as i64),
        ));
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
    if let [InstrRuff::ExprName(target)] = stmt.targets.as_slice() {
        if let InstrRuff::ExprBoolOp(bool_op) = (*stmt.value).clone() {
            let lowerer = ScopedSetupExprLowerer::new(context.current_value_forwarding_locals());
            if let Some(lowered) = try_lower_boolop_assign_direct::<_, E>(
                &lowerer,
                out.name_gen(),
                target.id.as_str(),
                bool_op,
                loop_ctx,
            ) {
                out.append_fragment(lowered?);
                return Ok(());
            }
        }
    }

    let mut value = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
        context,
        (*stmt.value).clone(),
        out,
        loop_ctx,
    )?;

    let should_bind_value = should_bind_assignment_value(&stmt.targets)
        && !can_forward_assignment_rhs_without_temp(context, &stmt.targets, stmt.value.as_ref());
    let value_temp_name = should_bind_value.then(|| {
        let name = context.fresh("assign_value");
        value = bind_temp(out, name.clone(), value.clone());
        name
    });

    for target in stmt.targets.iter().cloned() {
        lower_assignment_target_into(context, target, value.clone(), out, loop_ctx)?;
    }
    if let Some(name) = value_temp_name {
        delete_temp(out, name);
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
        crate::block_py::StmtAssign::new(vec![target], rhs).into(),
        crate::block_py::StmtDelete::new(vec![tmp_name_expr(ast::ExprContext::Del)]).into(),
    ]
}

pub(super) fn with_target_object_expr(value: Expr) -> Expr {
    value
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

    if starred_seen {
        out.push(py_stmt!(
            "{tmp:id} = __soac__.unpack({value:expr}, {spec:expr})",
            tmp = unpacked_name.as_str(),
            value = value,
            spec = make_tuple(spec_elts),
        ));
    } else {
        out.push(py_stmt!(
            "{tmp:id} = __soac__.unpack_fixed({value:expr}, {arity:literal})",
            tmp = unpacked_name.as_str(),
            value = value,
            arity = elts.len() as i64,
        ));
    }

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
