use crate::passes::ast_to_ast::body::Suite;
use crate::py_expr;
use crate::transformer::Transformer;
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_python_codegen::{Generator, Indentation};
use ruff_python_parser::{ParseError, ParseErrorType};
use ruff_source_file::LineEnding;
use std::collections::HashSet;

pub fn rewrite(body: &mut Suite) -> Result<HashSet<String>, ParseError> {
    let mut rewriter = FutureAnnotationsRewriter::new();
    let future_features = rewriter.strip_future_imports(body)?;
    if future_features.contains("annotations") {
        (&mut rewriter).visit_body(body);
    }
    Ok(future_features)
}

struct FutureAnnotationsRewriter {
    indent: Indentation,
}

impl FutureAnnotationsRewriter {
    fn new() -> Self {
        Self {
            indent: Indentation::new("    ".to_string()),
        }
    }

    fn strip_future_imports(&mut self, body: &mut Suite) -> Result<HashSet<String>, ParseError> {
        let mut future_features = HashSet::new();
        let mut index = 0;
        while index < body.len() {
            let mut remove_stmt = false;
            if let Stmt::ImportFrom(import_from) = &mut body[index] {
                if is_future_import(import_from) {
                    for alias in &import_from.names {
                        if !is_known_future_feature(&alias.name) {
                            return Err(ParseError {
                                error: ParseErrorType::OtherError(format!(
                                    "Future feature `{}` is not defined",
                                    alias.name
                                )),
                                location: alias.range,
                            });
                        }
                        future_features.insert(alias.name.id.to_string());
                    }
                    remove_stmt = true;
                }
            }

            if remove_stmt {
                body.remove(index);
            } else {
                index += 1;
            }
        }
        Ok(future_features)
    }

    fn annotation_string(&self, expr: &Expr) -> String {
        Generator::new(&self.indent, LineEnding::default()).expr(expr)
    }
}

fn is_known_future_feature(feature: &str) -> bool {
    matches!(
        feature,
        "nested_scopes"
            | "generators"
            | "division"
            | "absolute_import"
            | "with_statement"
            | "print_function"
            | "unicode_literals"
            | "barry_as_FLUFL"
            | "generator_stop"
            | "annotations"
    )
}

impl Transformer for FutureAnnotationsRewriter {
    fn visit_annotation(&mut self, expr: &mut Expr) {
        let rendered = self.annotation_string(expr);
        *expr = py_expr!("{literal:literal}", literal = rendered.as_str());
    }
}

fn is_future_import(import_from: &ast::StmtImportFrom) -> bool {
    import_from
        .module
        .as_ref()
        .is_some_and(|module| module.id.as_str() == "__future__")
}

#[cfg(test)]
mod test;
