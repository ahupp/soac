use super::*;
use crate::passes::ast_to_ast::ast_rewrite::Rewrite;
use crate::passes::ast_to_ast::expr_utils::make_tuple;
use ruff_python_ast::{TypeParam, TypeParamParamSpec, TypeParamTypeVar, TypeParamTypeVarTuple};

struct TypeParamInfo {
    bindings: Vec<Stmt>,
    param_names: Vec<String>,
    type_params_tuple: Option<Expr>,
}

fn make_type_param_info(type_params: ast::TypeParams) -> TypeParamInfo {
    let mut bindings = Vec::new();
    let mut param_names = Vec::new();
    let mut type_param_exprs = Vec::new();

    for type_param in type_params.type_params {
        match type_param {
            TypeParam::TypeVar(TypeParamTypeVar {
                name,
                bound,
                default,
                ..
            }) => {
                let param_name = name.as_str().to_string();
                let (constraints, bound_expr) = match bound {
                    Some(expr) => match *expr {
                        Expr::Tuple(ast::ExprTuple { elts, .. }) => (Some(make_tuple(elts)), None),
                        other => (None, Some(other)),
                    },
                    None => (None, None),
                };
                let default_expr = default.map(|expr| *expr);

                let bound_expr = bound_expr.unwrap_or_else(|| py_expr!("None"));
                let default_expr = default_expr.unwrap_or_else(|| py_expr!("None"));
                let constraints_expr = constraints.unwrap_or_else(|| py_expr!("None"));

                bindings.push(py_stmt!(
                    "{name:id} = __soac__.typing_TypeVar({name_literal:literal}, {bound:expr}, {default:expr}, {constraints:expr})",
                    name = param_name.as_str(),
                    name_literal = param_name.as_str(),
                    bound = bound_expr,
                    default = default_expr,
                    constraints = constraints_expr,
                ));
                type_param_exprs.push(py_expr!("{name:id}", name = param_name.as_str()));
                param_names.push(param_name);
            }
            TypeParam::TypeVarTuple(TypeParamTypeVarTuple { name, default, .. }) => {
                let param_name = name.as_str().to_string();
                let binding = match default.map(|expr| *expr) {
                    Some(default_expr) => py_stmt!(
                        "{name:id} = __soac__.typing_TypeVarTuple({name_literal:literal}, default={default:expr})",
                        name = param_name.as_str(),
                        name_literal = param_name.as_str(),
                        default = default_expr,
                    ),
                    None => py_stmt!(
                        "{name:id} = __soac__.typing_TypeVarTuple({name_literal:literal})",
                        name = param_name.as_str(),
                        name_literal = param_name.as_str(),
                    ),
                };

                bindings.push(binding);
                type_param_exprs.push(py_expr!("{name:id}", name = param_name.as_str()));
                param_names.push(param_name);
            }
            TypeParam::ParamSpec(TypeParamParamSpec { name, default, .. }) => {
                let param_name = name.as_str().to_string();
                let binding = match default.map(|expr| *expr) {
                    Some(default_expr) => py_stmt!(
                        "{name:id} = __soac__.typing_ParamSpec({name_literal:literal}, default={default:expr})",
                        name = param_name.as_str(),
                        name_literal = param_name.as_str(),
                        default = default_expr,
                    ),
                    None => py_stmt!(
                        "{name:id} = __soac__.typing_ParamSpec({name_literal:literal})",
                        name = param_name.as_str(),
                        name_literal = param_name.as_str(),
                    ),
                };

                bindings.push(binding);
                type_param_exprs.push(py_expr!("{name:id}", name = param_name.as_str()));
                param_names.push(param_name);
            }
        }
    }

    let type_params_tuple = if type_param_exprs.is_empty() {
        None
    } else {
        Some(make_tuple(type_param_exprs))
    };

    TypeParamInfo {
        bindings,
        param_names,
        type_params_tuple,
    }
}

pub(crate) fn rewrite_type_alias_stmt(
    _context: &Context,
    type_alias: ast::StmtTypeAlias,
) -> Rewrite {
    let ast::StmtTypeAlias {
        name,
        type_params,
        value,
        range,
        node_index,
    } = type_alias;

    let Expr::Name(ast::ExprName { id, .. }) = name.as_ref() else {
        return Rewrite::Unmodified(Stmt::TypeAlias(ast::StmtTypeAlias {
            name,
            type_params,
            value,
            range,
            node_index,
        }));
    };

    let alias_name = id.as_str();

    let mut stmts = Vec::new();
    if let Some(type_params) = type_params {
        let type_param_info = make_type_param_info(*type_params);
        stmts.extend(type_param_info.bindings);
        if let Some(type_params_tuple) = type_param_info.type_params_tuple {
            let alias_expr = py_expr!(
                "__soac__.typing_TypeAliasType({name:literal}, {value:expr}, type_params={params:expr})",
                name = alias_name,
                value = value,
                params = type_params_tuple,
            );
            stmts.push(py_stmt!(
                "{target:expr} = {alias:expr}",
                target = name,
                alias = alias_expr
            ));
        } else {
            let alias_expr = py_expr!(
                "__soac__.typing_TypeAliasType({name:literal}, {value:expr})",
                name = alias_name,
                value = value,
            );
            stmts.push(py_stmt!(
                "{target:expr} = {alias:expr}",
                target = name,
                alias = alias_expr
            ));
        }
        for name in type_param_info.param_names {
            stmts.push(py_stmt!("del {name:id}", name = name.as_str()));
        }
        return Rewrite::Walk(stmts);
    }

    let alias_expr = py_expr!(
        "__soac__.typing_TypeAliasType({name:literal}, {value:expr})",
        name = alias_name,
        value = value,
    );
    stmts.push(py_stmt!(
        "{target:expr} = {alias:expr}",
        target = name,
        alias = alias_expr
    ));

    Rewrite::Walk(stmts)
}

#[cfg(test)]
mod test;
