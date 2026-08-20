use std::fmt;

use ruff_python_ast::StringFlags;
use ruff_python_ast::token::{TokenKind, Tokens};
use ruff_text_size::{Ranged, TextRange, TextSize};

/// A Python Unicode escape that Ruff currently replaces with U+FFFD.
///
/// The range covers the original escape bytes, not the decoded payload. A
/// genuine U+FFFD, including an escape for that character, is not an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnsupportedSurrogateEscape {
    range: TextRange,
    code_point: u32,
}

impl UnsupportedSurrogateEscape {
    pub const fn range(self) -> TextRange {
        self.range
    }

    pub const fn code_point(self) -> u32 {
        self.code_point
    }
}

impl fmt::Display for UnsupportedSurrogateEscape {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unsupported Unicode surrogate escape U+{:04X} at bytes {}..{}: SOAC source literals cannot preserve surrogate code points",
            self.code_point,
            u32::from(self.range.start()),
            u32::from(self.range.end()),
        )
    }
}

impl std::error::Error for UnsupportedSurrogateEscape {}

/// Reject active surrogate escapes in the actual tokens of a parsed source.
///
/// Call this after the parser's ordinary syntax validation and before consuming
/// decoded string values. `tokens` must come from this exact `source`; nested
/// annotation parses retain their original absolute source ranges and can use
/// the same function. No ordinary string is reparsed as an annotation here.
///
/// The lexer decides which bytes are strings, literal f/t-string portions, raw
/// portions, bytes, comments, or expressions. Only the escape units inside its
/// non-raw Unicode literal tokens are inspected. Malformed escapes remain the
/// parser's responsibility; this function is not a Python lexer or decoder.
pub fn validate_source_literals(
    source: &str,
    tokens: &Tokens,
) -> Result<(), UnsupportedSurrogateEscape> {
    for token in tokens {
        let contents = match token.kind() {
            TokenKind::String | TokenKind::FStringMiddle | TokenKind::TStringMiddle => {
                let flags = token
                    .string_flags()
                    .expect("Ruff literal tokens carry string flags");
                if flags.is_raw_string() || flags.is_byte_string() {
                    continue;
                }
                if token.kind() == TokenKind::String {
                    token
                        .range()
                        .add_start(flags.opener_len())
                        .sub_end(flags.closer_len())
                } else {
                    // Middle-token flags include the enclosing raw/prefix
                    // state, but the token range does not include its quotes.
                    token.range()
                }
            }
            _ => continue,
        };
        validate_escape_units(&source[contents], contents.start())?;
    }
    Ok(())
}

fn validate_escape_units(
    contents: &str,
    offset: TextSize,
) -> Result<(), UnsupportedSurrogateEscape> {
    let bytes = contents.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'\\' {
            cursor += 1;
            continue;
        }
        let start = cursor;
        cursor += 1;
        let Some(&escape) = bytes.get(cursor) else {
            // A literal f/t-string segment can end immediately before a brace.
            break;
        };
        cursor += 1;
        match escape {
            b'u' | b'U' => {
                let width = if escape == b'u' { 4 } else { 8 };
                let Some(digits) = bytes.get(cursor..cursor + width) else {
                    continue;
                };
                let value = digits.iter().try_fold(0_u32, |value, digit| {
                    let digit = match digit {
                        b'0'..=b'9' => u32::from(digit - b'0'),
                        b'a'..=b'f' => u32::from(digit - b'a' + 10),
                        b'A'..=b'F' => u32::from(digit - b'A' + 10),
                        _ => return None,
                    };
                    Some((value << 4) | digit)
                });
                if let Some(value) = value {
                    cursor += width;
                    if (0xD800..=0xDFFF).contains(&value) {
                        return Err(UnsupportedSurrogateEscape {
                            range: TextRange::new(
                                offset + TextSize::try_from(start).unwrap(),
                                offset + TextSize::try_from(cursor).unwrap(),
                            ),
                            code_point: value,
                        });
                    }
                }
            }
            b'N' if bytes.get(cursor) == Some(&b'{') => {
                // A Unicode character name is one escape unit. Its spelling is
                // not another source fragment and must not be scanned again.
                cursor += 1;
                while cursor < bytes.len() && bytes[cursor] != b'}' {
                    cursor += 1;
                }
                cursor += usize::from(cursor < bytes.len());
            }
            b'\r' if bytes.get(cursor) == Some(&b'\n') => cursor += 1,
            // This consumes escaped backslashes as a pair. Decoding a \x5c
            // or named backslash does not start a new escape in Python either.
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
