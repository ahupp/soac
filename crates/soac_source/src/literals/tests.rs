use super::*;
use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::{parse_module, parse_string_annotation};

fn validate(source: &str) -> Result<(), UnsupportedSurrogateEscape> {
    let parsed = parse_module(source).expect("fixture is valid Python source");
    validate_source_literals(source, parsed.tokens())
}

#[test]
fn rejects_surrogates_in_each_actual_unicode_literal_part() {
    for (source, escape, value) in [
        (r#"value = '\ud800'"#, r"\ud800", 0xD800),
        (r#"value = U'\uDfFf'"#, r"\uDfFf", 0xDFFF),
        (r#"value = '\U0000dCa7'"#, r"\U0000dCa7", 0xDCA7),
        (r#"value = '\ud8000'"#, r"\ud800", 0xD800),
        (r#"value = 'a' '\udca7' 'b'"#, r"\udca7", 0xDCA7),
        (r#"value = r'\ud800' '\udfff'"#, r"\udfff", 0xDFFF),
        (r#"'''doc \ud800 string'''"#, r"\ud800", 0xD800),
        (r#"value = f'\ud800{x}'"#, r"\ud800", 0xD800),
        (r#"value = F'{x}\U0000DFFF'"#, r"\U0000DFFF", 0xDFFF),
        (r#"value = f'{x:\udca7}'"#, r"\udca7", 0xDCA7),
        (r#"value = f'{x:{width}\ud800}'"#, r"\ud800", 0xD800),
        (r#"value = rf'{"\ud800"}'"#, r"\ud800", 0xD800),
        (r#"value = t'\ud800{x}'"#, r"\ud800", 0xD800),
        (r#"value = T'{x}\U0000DFFF'"#, r"\U0000DFFF", 0xDFFF),
        (r#"value = t'{x:\udca7}'"#, r"\udca7", 0xDCA7),
        (r#"value = t'{x:{width}\ud800}'"#, r"\ud800", 0xD800),
        (r#"value = rt'{"\ud800"}'"#, r"\ud800", 0xD800),
        ("value = '\\\n\\ud800'", r"\ud800", 0xD800),
        ("value = '\\\r\n\\udfff'", r"\udfff", 0xDFFF),
        ("prefix = 'é'\nvalue = '\\ud800'", r"\ud800", 0xD800),
    ] {
        let error = validate(source).expect_err(source);
        let start = source.find(escape).unwrap();
        assert_eq!(
            error.range(),
            TextRange::new(
                TextSize::try_from(start).unwrap(),
                TextSize::try_from(start + escape.len()).unwrap(),
            ),
            "{source}",
        );
        assert_eq!(error.code_point(), value, "{source}");
    }
}

#[test]
fn accepts_lossless_literals_and_nonliteral_text() {
    for source in [
        "value = '�'",
        r#"value = '\ufffd'"#,
        r#"value = '\U0000FFFD'"#,
        r#"value = '\ud7ff\ue000\U0001D800\U0010FFFF'"#,
        r#"value = '\N{REPLACEMENT CHARACTER}'"#,
        r#"value = '\N{REVERSE SOLIDUS}ud800'"#,
        r#"value = '\x5cud800'"#,
        r#"value = '\134ud800'"#,
        r#"value = '\\ud800'"#,
        r#"value = r'\ud800'"#,
        r#"value = R'\U0000D800'"#,
        r#"value = b'\ud800'"#,
        r#"value = br'\ud800'"#,
        r#"value = rf'\ud800{x}'"#,
        r#"value = Fr'\ud800{x}'"#,
        r#"value = RF'{x:\ud800}'"#,
        r#"value = rt'\ud800{x}'"#,
        r#"value = Tr'\ud800{x}'"#,
        r#"value = RT'{x:\ud800}'"#,
        r#"value = f'{r"\ud800"}'"#,
        r#"value = t'{r"\ud800"}'"#,
        r#"value = f'{{escaped}}\N{SNOWMAN}{x}'"#,
        r#"value = t'{{escaped}}\N{SNOWMAN}{x}'"#,
        r#"value = f'{"�":\ufffd}'"#,
        r#"value = t'{"�":\ufffd}'"#,
        "# '\\ud800' is only a comment\nvalue = 1",
        "value = chr(0xD800)",
        "def f():\n    value: str\n    return 'fine'",
    ] {
        assert_eq!(validate(source), Ok(()), "{source}");
    }
}

#[test]
fn escape_units_obey_backslash_parity() {
    for count in 1..=12 {
        for prefix in ["", "f", "t"] {
            let source = format!("value = {prefix}'{}ud800'", "\\".repeat(count));
            let result = validate(&source);
            assert_eq!(result.is_err(), count % 2 == 1, "{source}");
            if let Err(error) = result {
                assert_eq!(&source[error.range()], r"\ud800");
                assert_eq!(error.range().end().to_usize(), source.len() - 1);
            }
        }
    }
}

#[test]
fn actual_string_annotation_tokens_are_checked_separately() {
    // The outer raw string is lossless. A caller that deliberately enters it as
    // an annotation must validate that actual second parse, not reuse the
    // outer token decision. No text guessing or standalone re-lexing is used.
    let source = r#"r"Literal['\ud800']""#;
    let parsed = parse_module(source).unwrap();
    assert_eq!(validate_source_literals(source, parsed.tokens()), Ok(()));
    let Stmt::Expr(statement) = &parsed.suite()[0] else {
        panic!("expected string expression");
    };
    let Expr::StringLiteral(string) = statement.value.as_ref() else {
        panic!("expected string literal");
    };
    let annotation =
        parse_string_annotation(source, string.as_single_part_string().unwrap()).unwrap();
    let error = validate_source_literals(source, annotation.tokens()).unwrap_err();
    assert_eq!(&source[error.range()], r"\ud800");
    assert_eq!(error.code_point(), 0xD800);
}

#[test]
fn malformed_escapes_keep_the_existing_parser_error() {
    for source in [
        r#"value = '\uD80'"#,
        r#"value = '\uD80Z'"#,
        r#"value = '\U0000D80'"#,
        r#"value = '\U00110000'"#,
        r#"value = '\N{not a unicode character name}'"#,
    ] {
        assert!(parse_module(source).is_err(), "{source}");
    }
}
