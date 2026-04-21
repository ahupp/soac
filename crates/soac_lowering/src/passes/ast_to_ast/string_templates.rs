use crate::block_py::{HasMeta, Mappable};
use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::InstrRuff;
use crate::transformer::{walk_expr, Transformer};
use crate::{passes::ast_to_ast::expr_utils::make_tuple, py_expr};
use ruff_python_ast::{self as ast, Expr};
use ruff_python_parser::{ParseError, ParseErrorType};
use ruff_text_size::{Ranged, TextRange};

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
        let expr_text = crate::ruff_ast_to_string(&*interp.expression)
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
    let expr_text = crate::ruff_ast_to_string(&*interp.expression)
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

pub fn rewrite_fstring(expr: ast::ExprFString) -> Expr {
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

pub fn rewrite_tstring(expr: ast::ExprTString) -> Expr {
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

pub fn lower_string_templates_in_expr(expr: &mut Expr) {
    StringTemplateLowerer.visit_expr(expr);
}

fn source_slice(source: &str, range: TextRange) -> Option<&str> {
    source.get(range.start().to_usize()..range.end().to_usize())
}

fn hex_value(ch: u8) -> Option<u32> {
    match ch {
        b'0'..=b'9' => Some((ch - b'0') as u32),
        b'a'..=b'f' => Some((ch - b'a' + 10) as u32),
        b'A'..=b'F' => Some((ch - b'A' + 10) as u32),
        _ => None,
    }
}

fn parse_hex_escape(bytes: &[u8], offset: usize, len: usize) -> Option<u32> {
    let end = offset.checked_add(len)?;
    if end > bytes.len() {
        return None;
    }
    let mut value = 0u32;
    for byte in &bytes[offset..end] {
        value = (value << 4) | hex_value(*byte)?;
    }
    Some(value)
}

fn has_active_lone_surrogate_escape(content: &str) -> bool {
    let bytes = content.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }

        let run_start = index;
        while index < bytes.len() && bytes[index] == b'\\' {
            index += 1;
        }
        let run_len = index - run_start;
        if run_len % 2 == 0 || index >= bytes.len() {
            continue;
        }

        let value = match bytes[index] {
            b'u' => parse_hex_escape(bytes, index + 1, 4),
            b'U' => parse_hex_escape(bytes, index + 1, 8),
            _ => None,
        };
        if matches!(value, Some(0xD800..=0xDFFF)) {
            return true;
        }
    }
    false
}

fn string_literal_has_lone_surrogate_escape(
    context: &Context,
    value: &ast::StringLiteralValue,
) -> bool {
    value.iter().any(|part| {
        !part.flags.prefix().is_raw()
            && source_slice(&context.source, part.content_range())
                .is_some_and(has_active_lone_surrogate_escape)
    })
}

fn interpolated_elements_have_lone_surrogate_escape(
    context: &Context,
    elements: &ast::InterpolatedStringElements,
    is_raw: bool,
) -> bool {
    elements.iter().any(|element| match element {
        ast::InterpolatedStringElement::Literal(lit) => {
            !is_raw
                && source_slice(&context.source, lit.range())
                    .is_some_and(has_active_lone_surrogate_escape)
        }
        ast::InterpolatedStringElement::Interpolation(interp) => {
            interp.format_spec.as_ref().is_some_and(|format_spec| {
                interpolated_elements_have_lone_surrogate_escape(
                    context,
                    &format_spec.elements,
                    is_raw,
                )
            })
        }
    })
}

fn fstring_value_has_lone_surrogate_escape(context: &Context, value: &ast::FStringValue) -> bool {
    value.iter().any(|part| match part {
        ast::FStringPart::Literal(lit) => {
            !lit.flags.prefix().is_raw()
                && source_slice(&context.source, lit.content_range())
                    .is_some_and(has_active_lone_surrogate_escape)
        }
        ast::FStringPart::FString(fstring) => interpolated_elements_have_lone_surrogate_escape(
            context,
            &fstring.elements,
            fstring.flags.prefix().is_raw(),
        ),
    })
}

fn tstring_value_has_lone_surrogate_escape(context: &Context, value: &ast::TStringValue) -> bool {
    value.iter().any(|tstring| {
        interpolated_elements_have_lone_surrogate_escape(
            context,
            &tstring.elements,
            tstring.flags.prefix().is_raw(),
        )
    })
}

struct LoneSurrogateStringLiteralRejecter<'a> {
    context: &'a Context,
    error: Option<ParseError>,
}

impl Transformer for LoneSurrogateStringLiteralRejecter<'_> {
    fn visit_expr(&mut self, expr: &mut Expr) {
        if self.error.is_some() {
            return;
        }
        match expr {
            Expr::StringLiteral(node)
                if string_literal_has_lone_surrogate_escape(self.context, &node.value) =>
            {
                self.error = Some(ParseError {
                    error: ParseErrorType::OtherError(
                        "SOAC does not support string literals containing lone surrogate escapes"
                            .to_string(),
                    ),
                    location: node.range(),
                });
            }
            Expr::FString(node)
                if fstring_value_has_lone_surrogate_escape(self.context, &node.value) =>
            {
                self.error = Some(ParseError {
                    error: ParseErrorType::OtherError(
                        "SOAC does not support f-string literals containing lone surrogate escapes"
                            .to_string(),
                    ),
                    location: node.range(),
                });
            }
            Expr::TString(node)
                if tstring_value_has_lone_surrogate_escape(self.context, &node.value) =>
            {
                self.error = Some(ParseError {
                    error: ParseErrorType::OtherError(
                        "SOAC does not support t-string literals containing lone surrogate escapes"
                            .to_string(),
                    ),
                    location: node.range(),
                });
            }
            _ => walk_expr(self, expr),
        }
    }
}

pub fn reject_lone_surrogate_string_literals(
    context: &Context,
    body: &mut Suite,
) -> Result<(), ParseError> {
    let mut rejecter = LoneSurrogateStringLiteralRejecter {
        context,
        error: None,
    };
    rejecter.visit_body(body);
    rejecter.error.map_or(Ok(()), Err)
}

pub fn lower_string_templates_in_instr_ruff(expr: InstrRuff) -> InstrRuff {
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
