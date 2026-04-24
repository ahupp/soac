use crate::block_py::{HasMeta, Mappable};
use crate::passes::InstrRuff;
use crate::transformer::{walk_expr, Transformer};
use crate::{passes::ast_to_ast::expr_utils::make_tuple, py_expr};
use ruff_python_ast::{self as ast, Expr};

fn join_parts(parts: Vec<Expr>, force_join: bool) -> Expr {
    match parts.len() {
        0 => py_expr!("\"\""),
        1 if !force_join => parts.into_iter().next().unwrap(),
        _ => {
            let tuple = make_tuple(parts);
            py_expr!("\"\".join({tuple:expr})", tuple = tuple)
        }
    }
}

fn strip_debug_comment(trailing: &str) -> String {
    let mut output = String::with_capacity(trailing.len());
    let mut in_comment = false;
    for ch in trailing.chars() {
        if in_comment {
            if ch == '\n' {
                output.push(ch);
                in_comment = false;
            }
            continue;
        }
        if ch == '#' {
            in_comment = true;
            continue;
        }
        output.push(ch);
    }
    output
}

fn rewrite_interpolation(interp: &ast::InterpolatedElement, is_raw: bool) -> Vec<Expr> {
    let mut parts = Vec::new();

    let mut value = (*interp.expression).clone();
    let conversion = if let Some(debug) = &interp.debug_text {
        let has_format_spec = interp.format_spec.is_some();
        let trailing_has_format = debug.trailing.contains(':');
        if matches!(interp.conversion, ast::ConversionFlag::None) && !has_format_spec {
            ast::ConversionFlag::Repr
        } else if matches!(interp.conversion, ast::ConversionFlag::Repr)
            && has_format_spec
            && trailing_has_format
        {
            ast::ConversionFlag::None
        } else {
            interp.conversion
        }
    } else {
        interp.conversion
    };
    value = match conversion {
        ast::ConversionFlag::Ascii => py_expr!("__soac__.ascii({value:expr})", value = value),
        ast::ConversionFlag::Repr => py_expr!("__soac__.repr({value:expr})", value = value),
        ast::ConversionFlag::Str => py_expr!("__soac__.str({value:expr})", value = value),
        ast::ConversionFlag::None => value,
    };

    if let Some(debug) = &interp.debug_text {
        let expr_text = crate::ruff_ast::ruff_ast_to_string(&*interp.expression)
            .trim_end()
            .to_string();
        let trailing = strip_debug_comment(debug.trailing.as_str());
        let debug_text = format!("{}{}{}", debug.leading, expr_text, trailing);
        if !debug_text.is_empty() {
            parts.push(py_expr!("{literal:literal}", literal = debug_text.as_str()));
        }
    }

    let formatted = if let Some(format_spec) = &interp.format_spec {
        let (parts, _) = rewrite_elements(&format_spec.elements, is_raw);
        let spec = if parts.is_empty() {
            py_expr!("\"\"")
        } else {
            join_parts(parts, false)
        };
        py_expr!(
            "__soac__.format({value:expr}, {format_spec:expr})",
            value = value,
            format_spec = spec
        )
    } else {
        py_expr!("__soac__.format({value:expr})", value = value)
    };

    parts.push(formatted);
    parts
}

fn rewrite_elements(elements: &ast::InterpolatedStringElements, is_raw: bool) -> (Vec<Expr>, bool) {
    let mut parts = Vec::new();
    let mut has_interpolation = false;
    for element in elements.iter() {
        match element {
            ast::InterpolatedStringElement::Literal(lit) => {
                parts.push(rewrite_fstring_literal(lit, is_raw));
            }
            ast::InterpolatedStringElement::Interpolation(interp) => {
                parts.extend(rewrite_interpolation(interp, is_raw));
                has_interpolation = true;
            }
        }
    }
    (parts, has_interpolation)
}

fn rewrite_tstring_interpolation(interp: &ast::InterpolatedElement) -> Expr {
    let value = (*interp.expression).clone();
    let expr_text = crate::ruff_ast::ruff_ast_to_string(&*interp.expression)
        .trim_end()
        .to_string();
    let conversion_expr = match interp.conversion {
        ast::ConversionFlag::None => py_expr!("None"),
        ast::ConversionFlag::Repr => py_expr!("{literal:literal}", literal = "r"),
        ast::ConversionFlag::Str => py_expr!("{literal:literal}", literal = "s"),
        ast::ConversionFlag::Ascii => py_expr!("{literal:literal}", literal = "a"),
    };
    let format_spec = if let Some(format_spec) = &interp.format_spec {
        let (parts, _) = rewrite_elements(&format_spec.elements, false);
        if parts.is_empty() {
            py_expr!("{literal:literal}", literal = "")
        } else {
            join_parts(parts, false)
        }
    } else {
        py_expr!("{literal:literal}", literal = "")
    };
    py_expr!(
        "__soac__.templatelib_Interpolation({value:expr}, {expr_text:literal}, {conversion:expr}, {format_spec:expr})",
        value = value,
        expr_text = expr_text.as_str(),
        conversion = conversion_expr,
        format_spec = format_spec,
    )
}

pub(crate) fn rewrite_fstring(expr: ast::ExprFString) -> Expr {
    let mut parts = Vec::new();
    let mut has_interpolation = false;
    for part in expr.value.iter() {
        match part {
            ast::FStringPart::Literal(lit) => {
                parts.push(rewrite_string_literal(lit));
            }
            ast::FStringPart::FString(f) => {
                let (elements, has_interp) =
                    rewrite_elements(&f.elements, f.flags.prefix().is_raw());
                parts.extend(elements);
                has_interpolation |= has_interp;
            }
        }
    }
    join_parts(parts, !has_interpolation)
}

pub(crate) fn rewrite_tstring(expr: ast::ExprTString) -> Expr {
    let mut parts = Vec::new();
    for t in expr.value.iter() {
        for element in t.elements.iter() {
            match element {
                ast::InterpolatedStringElement::Literal(lit) => {
                    parts.push(py_expr!("{literal:literal}", literal = lit.value.as_ref()));
                }
                ast::InterpolatedStringElement::Interpolation(interp) => {
                    parts.push(rewrite_tstring_interpolation(interp));
                }
            }
        }
    }
    let tuple = make_tuple(parts);
    py_expr!(
        "__soac__.templatelib_Template(*{parts:expr})",
        parts = tuple
    )
}

struct StringTemplateLowerer;

impl Transformer for StringTemplateLowerer {
    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::FString(node) => {
                *expr = rewrite_fstring(node.clone());
                self.visit_expr(expr);
            }
            Expr::TString(node) => {
                *expr = rewrite_tstring(node.clone());
                self.visit_expr(expr);
            }
            _ => walk_expr(self, expr),
        }
    }
}

pub(crate) fn lower_string_templates_in_expr(expr: &mut Expr) {
    StringTemplateLowerer.visit_expr(expr);
}

pub(crate) fn lower_string_templates_in_instr_ruff(expr: InstrRuff) -> InstrRuff {
    match expr {
        InstrRuff::ExprFString(node) => {
            crate::passes::ast_to_instr::from_ast_expr(rewrite_fstring(ast::ExprFString {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: node.value,
            }))
        }
        InstrRuff::ExprTString(node) => {
            crate::passes::ast_to_instr::from_ast_expr(rewrite_tstring(ast::ExprTString {
                range: node.meta().range,
                node_index: node.meta().node_index,
                value: node.value,
            }))
        }
        other => other
            .try_map_same_children(&mut |child| {
                Ok::<_, std::convert::Infallible>(lower_string_templates_in_instr_ruff(child))
            })
            .expect("InstrRuff string-template lowering should be infallible"),
    }
}

fn rewrite_string_literal(lit: &ast::StringLiteral) -> Expr {
    py_expr!("{literal:literal}", literal = lit.value.as_ref())
}

fn rewrite_fstring_literal(lit: &ast::InterpolatedStringLiteralElement, _is_raw: bool) -> Expr {
    py_expr!("{literal:literal}", literal = lit.value.as_ref())
}
