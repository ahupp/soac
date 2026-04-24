use crate::passes::ast_to_ast::ast_rewrite::Rewrite;
use crate::template::py_stmt;

use super::context::Context;
use ruff_python_ast::{self as ast};
use ruff_python_parser::parse_module;

pub(crate) fn rewrite(ast::StmtImport { names, .. }: ast::StmtImport) -> Rewrite {
    // TODO: hard-coded "import _testinternalcapi"
    let stmts: Vec<ast::Stmt> = names
        .into_iter()
        .flat_map(|alias| {
            let module_name = alias.name.id.to_string();
            let binding = alias
                .asname
                .as_ref()
                .map(|n| n.id.as_str())
                .unwrap_or_else(|| module_name.split('.').next().unwrap());
            let needs_fromlist = alias.asname.is_some() && module_name.contains('.');
            if needs_fromlist {
                let mut expr = format!("__soac__.import_({:?}, globals())", module_name.as_str());
                let mut parts = module_name.split('.').collect::<Vec<_>>();
                if !parts.is_empty() {
                    parts.remove(0);
                }
                for part in parts {
                    expr = format!("__soac__.import_attr({}, {:?})", expr, part);
                }
                let stmt_source = format!("{binding} = {expr}", binding = binding, expr = expr);
                let body = parse_module(stmt_source.as_str())
                    .expect("failed to parse rewritten dotted import")
                    .into_syntax()
                    .body;
                body
            } else {
                vec![py_stmt!(
                    "{name:id} = __soac__.import_({module:literal}, globals())",
                    name = binding,
                    module = module_name.as_str(),
                )]
            }
        })
        .collect();
    Rewrite::Walk(stmts)
}

pub(crate) fn rewrite_from(context: &Context, import_from: ast::StmtImportFrom) -> Rewrite {
    let ast::StmtImportFrom {
        module,
        names,
        level,
        ..
    } = import_from;

    if names.iter().any(|alias| alias.name.id.as_str() == "*") {
        let module_name = module.as_ref().map(|n| n.id.as_str()).unwrap_or("");
        let module_literal = format!("{:?}", module_name);
        let import_stmt_source = format!(
            "__soac__.import_star({module}, globals(), {level})",
            module = module_literal,
            level = level
        );
        let body = parse_module(import_stmt_source.as_str())
            .expect("failed to parse rewritten import-star")
            .into_syntax()
            .body;
        return Rewrite::Walk(body);
    }
    let module_name = module.as_ref().map(|n| n.id.as_str()).unwrap_or("");
    let temp_binding = context.fresh("import");
    let mut statements = Vec::new();

    let fromlist: Vec<String> = names
        .iter()
        .map(|alias| format!("{:?}", alias.name.id.as_str()))
        .collect();
    let fromlist_literal = format!("[{}]", fromlist.join(", "));
    let module_literal = format!("{:?}", module_name);
    let import_stmt_source = if level > 0 {
        format!(
            "{tmp} = __soac__.import_({module}, globals(), {fromlist}, {level})",
            tmp = temp_binding,
            module = module_literal,
            fromlist = fromlist_literal,
            level = level
        )
    } else {
        format!(
            "{tmp} = __soac__.import_({module}, globals(), {fromlist})",
            tmp = temp_binding,
            module = module_literal,
            fromlist = fromlist_literal
        )
    };

    let mut import_stmt = parse_module(import_stmt_source.as_str())
        .expect("failed to parse rewritten import")
        .into_syntax()
        .body;
    let import_stmt = import_stmt
        .pop()
        .expect("expected single statement when parsing import rewrite");
    statements.push(import_stmt);

    for alias in names {
        let orig = alias.name.id.as_str();
        let binding = alias.asname.as_ref().map(|n| n.id.as_str()).unwrap_or(orig);
        statements.push(py_stmt!(
            "{name:id} = __soac__.import_attr({module:id}, {attr:literal})",
            name = binding,
            module = temp_binding.as_str(),
            attr = orig,
        ));
    }

    Rewrite::Walk(statements)
}
