use crate::passes::ast_to_ast::body::Suite;
use crate::template::py_expr;
use crate::transformer::{walk_stmt, Transformer};
use ruff_python_ast::{self as ast, Expr, HasNodeIndex, Stmt};
use ruff_python_codegen::{Generator, Indentation};
use ruff_python_parser::{ParseError, ParseErrorType};
use ruff_source_file::LineEnding;
use ruff_text_size::Ranged;
use std::collections::HashSet;

pub(crate) fn rewrite(
    body: &mut Suite,
    canonical: Option<&crate::CanonicalAnnotationStrings>,
    authenticated_source: bool,
) -> Result<HashSet<String>, ParseError> {
    let future_features = collect_future_imports(body)?;
    if future_features.contains("annotations") {
        let mut rewriter = FutureAnnotationsRewriter {
            indent: Indentation::new("    ".to_string()),
            canonical,
            require_canonical: authenticated_source || future_features.contains("strict"),
            error: None,
        };
        rewriter.visit_body(body);
        if let Some(error) = rewriter.error {
            return Err(error);
        }
    }
    Ok(future_features)
}

struct FutureAnnotationsRewriter<'a> {
    indent: Indentation,
    canonical: Option<&'a crate::CanonicalAnnotationStrings>,
    require_canonical: bool,
    error: Option<ParseError>,
}

impl FutureAnnotationsRewriter<'_> {
    fn annotation_string(&mut self, expr: &Expr) -> Option<String> {
        if let Some(canonical) = self.canonical {
            if let Some(value) = canonical.get(expr.range()) {
                return Some(value.to_owned());
            }
        }
        if self.require_canonical || self.canonical.is_some() {
            self.error.get_or_insert_with(|| ParseError {
                error: ParseErrorType::OtherError(
                    "future annotation is missing its canonical native source string".into(),
                ),
                location: expr.range(),
            });
            None
        } else {
            Some(Generator::new(&self.indent, LineEnding::default()).expr(expr))
        }
    }
}

/// Future statements are compiler directives *and* ordinary imports. Preserve
/// their binding/evaluation behavior, including aliases; only the language
/// feature set is consumed here. Placement is checked before rewrites can
/// erase lexical scopes or move statements to generated helpers.
fn collect_future_imports(body: &mut Suite) -> Result<HashSet<String>, ParseError> {
    struct NestedFuture {
        error: Option<ParseError>,
    }

    impl Transformer for NestedFuture {
        fn visit_stmt(&mut self, stmt: &mut Stmt) {
            if self.error.is_some() {
                return;
            }
            if let Stmt::ImportFrom(import) = stmt {
                if is_future_import(import) {
                    self.error = Some(misplaced_future(import));
                    return;
                }
            }
            walk_stmt(self, stmt);
        }
    }

    let mut features = HashSet::new();
    let mut allowed = true;
    for (index, stmt) in body.iter_mut().enumerate() {
        if index == 0
            && matches!(stmt, Stmt::Expr(expr) if matches!(&*expr.value, Expr::StringLiteral(_)))
        {
            continue;
        }
        if let Stmt::ImportFrom(import) = stmt {
            if is_future_import(import) {
                if !allowed {
                    return Err(misplaced_future(import));
                }
                for alias in &import.names {
                    if !is_known_future_feature(&alias.name) {
                        return Err(ParseError {
                            error: ParseErrorType::OtherError(format!(
                                "future feature {} is not defined",
                                alias.name
                            )),
                            location: alias.range,
                        });
                    }
                    features.insert(alias.name.id.to_string());
                }
                continue;
            }
        }
        allowed = false;
        let mut nested = NestedFuture { error: None };
        nested.visit_stmt(stmt);
        if let Some(error) = nested.error {
            return Err(error);
        }
    }
    Ok(features)
}

fn misplaced_future(import: &ast::StmtImportFrom) -> ParseError {
    ParseError {
        error: ParseErrorType::OtherError(
            "from __future__ imports must occur at the beginning of the file".into(),
        ),
        location: import.range,
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
            | "strict"
    )
}

impl Transformer for FutureAnnotationsRewriter<'_> {
    fn visit_annotation(&mut self, expr: &mut Expr) {
        let Some(rendered) = self.annotation_string(expr) else {
            return;
        };
        let Expr::StringLiteral(mut literal) =
            py_expr!("{literal:literal}", literal = rendered.as_str())
        else {
            unreachable!()
        };
        literal.range = expr.range();
        literal.node_index = expr.node_index().clone();
        *expr = Expr::StringLiteral(literal);
    }
}

fn is_future_import(import_from: &ast::StmtImportFrom) -> bool {
    import_from.level == 0
        && import_from
            .module
            .as_ref()
            .is_some_and(|module| module.id.as_str() == "__future__")
}

#[cfg(test)]
mod test;
