use std::collections::{BTreeMap, HashSet};
use std::fmt;

use ruff_python_ast::statement_visitor::{self, StatementVisitor};
use ruff_python_ast::token::{TokenKind, Tokens};
use ruff_python_ast::{Expr, Stmt};
use ruff_text_size::{Ranged, TextRange, TextSize};

/// The source construct to which an explicit SOAC comment applies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoacDirectiveTarget {
    Package,
    Module,
    /// The actual Ruff class node range, including its decorators.
    ///
    /// Bind this against the same unmodified parsed source. A name alone is
    /// insufficient: separate lexical scopes can define classes with the same
    /// name, and a scope can contain multiple definitions of that name.
    Class {
        class_range: TextRange,
    },
}

/// Explicit source selections, without defaults or inherited policy applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SoacDirective {
    /// The complete comment range, including continuation comments if present.
    pub range: TextRange,
    pub target: SoacDirectiveTarget,
    pub strict_assign: Option<bool>,
    pub checked_attr: Option<bool>,
}

/// A stable classification independent of a diagnostic's rendered wording.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SoacDirectiveErrorKind {
    Malformed,
    UnknownScope,
    UnknownKey,
    WrongScope,
    DuplicateKey,
    InvalidBoolean,
    DuplicateDirective,
    InvalidPlacement,
    PackageOutsideInit,
    DanglingClass,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SoacDirectiveError {
    kind: SoacDirectiveErrorKind,
    range: TextRange,
    message: String,
}

impl SoacDirectiveError {
    pub const fn kind(&self) -> SoacDirectiveErrorKind {
        self.kind
    }

    pub const fn range(&self) -> TextRange {
        self.range
    }

    fn new(kind: SoacDirectiveErrorKind, range: TextRange, message: impl Into<String>) -> Self {
        Self {
            kind,
            range,
            message: message.into(),
        }
    }
}

impl fmt::Display for SoacDirectiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at bytes {}..{}",
            self.message,
            u32::from(self.range.start()),
            u32::from(self.range.end()),
        )
    }
}

impl std::error::Error for SoacDirectiveError {}

#[derive(Clone, Copy)]
struct Comment<'a> {
    range: TextRange,
    text: &'a str,
    indentation: Option<&'a str>,
    parenthesized: bool,
}

#[derive(Clone, Copy)]
enum Scope {
    Package,
    Module,
    Class,
}

impl Scope {
    const fn name(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Module => "module",
            Self::Class => "class",
        }
    }
}

/// Parse bounded SOAC policy comments in a validated, original Python module.
///
/// `source`, `tokens`, and `suite` must describe the same complete parse. The
/// caller validates ordinary Python syntax and decides whether this is a
/// package's `__init__.py`; paths and package policy inheritance are not guessed
/// here. Parsing neither selects executable authority nor installs contracts.
///
/// The grammar is case-sensitive, with ASCII horizontal whitespace permitted
/// around punctuation:
///
/// ```text
/// # soac: package(strict_assign=true, checked_attr=false)
/// # soac: module(checked_attr=true)
/// # soac: class(checked_attr=false)
/// ```
///
/// Only `true` and `false` are values. Package/module scopes allow both keys;
/// class scope allows only `checked_attr`. Keys may be omitted, an empty list
/// means no explicit values, and one trailing comma is allowed. Unknown scopes
/// or keys, repeated keys, extra syntax, and repeated directives for the same
/// module/package/class are errors. A comment beginning with the word `soac`
/// reserves the directive spelling; missing `:` or parentheses are errors.
///
/// Parenthesized arguments may continue across consecutive comment-only lines
/// at exactly the same indentation, for example:
///
/// ```text
/// # soac: module(
/// #     strict_assign=true,
/// #     checked_attr=false,
/// # )
/// ```
///
/// A continuation strips its first `#`; it does not repeat `soac:`. Blank
/// physical lines, intervening code, nested parentheses, and text after `)` are
/// not part of this grammar. Directive-shaped string contents are never read.
/// Real directives must be standalone comments outside Python parentheses.
///
/// Package/module directives are unindented module-header comments, before the
/// first statement other than an initial docstring and future imports. A class
/// directive attaches to the next actual class statement, before all of its
/// decorators and with the same indentation. Ordinary comments and blank lines
/// may intervene, but no executable statement may. Nested classes and repeated
/// class names bind by their distinct Ruff AST ranges. Results retain source
/// order; omitted-value/default and inheritance resolution belong to callers.
pub fn parse_soac_directives(
    source: &str,
    tokens: &Tokens,
    suite: &[Stmt],
    is_package_init: bool,
) -> Result<Vec<SoacDirective>, SoacDirectiveError> {
    let mut comments = Vec::new();
    let mut stream = tokens.iter_with_context();
    while let Some(token) = stream.next() {
        if token.kind() == TokenKind::Comment {
            comments.push(Comment {
                range: token.range(),
                text: source[token.range()]
                    .strip_prefix('#')
                    .expect("Ruff comment tokens start with #"),
                indentation: indentation(source, token.start()),
                parenthesized: stream.in_parenthesized_context(),
            });
        }
    }

    let header_end = module_header_end(suite);
    let mut classes = ClassRanges::default();
    classes.visit_body(suite);
    let mut seen_classes = HashSet::new();
    let mut seen_package = false;
    let mut seen_module = false;
    let mut directives = Vec::new();
    let mut index = 0;
    while index < comments.len() {
        let first = comments[index];
        let Some(spelling) = directive_spelling(first.text) else {
            index += 1;
            continue;
        };
        if first.indentation.is_none() || first.parenthesized {
            return Err(SoacDirectiveError::new(
                SoacDirectiveErrorKind::InvalidPlacement,
                first.range,
                "SOAC directives must be standalone comments outside Python parentheses",
            ));
        }
        let Some(body) = trim_horizontal(spelling).strip_prefix(':') else {
            return Err(malformed(first.range, "expected ':' after 'soac'"));
        };
        let (scope, arguments) = directive_head(body, first.range)?;
        let (arguments, range) = complete_arguments(source, &comments, &mut index, arguments)?;
        let (strict_assign, checked_attr) = parse_arguments(&arguments, scope, range)?;
        let target = match scope {
            Scope::Package | Scope::Module => {
                if first.indentation != Some("") || header_end.is_some_and(|end| range.end() > end)
                {
                    return Err(SoacDirectiveError::new(
                        SoacDirectiveErrorKind::InvalidPlacement,
                        range,
                        format!(
                            "SOAC {} directives belong in the module header",
                            scope.name()
                        ),
                    ));
                }
                if matches!(scope, Scope::Package) && !is_package_init {
                    return Err(SoacDirectiveError::new(
                        SoacDirectiveErrorKind::PackageOutsideInit,
                        range,
                        "SOAC package directives are only valid in package __init__.py files",
                    ));
                }
                let (seen, target) = if matches!(scope, Scope::Package) {
                    (&mut seen_package, SoacDirectiveTarget::Package)
                } else {
                    (&mut seen_module, SoacDirectiveTarget::Module)
                };
                if std::mem::replace(seen, true) {
                    return Err(duplicate(range, scope.name()));
                }
                target
            }
            Scope::Class => {
                let class_range = tokens
                    .after(range.end())
                    .iter()
                    .find(|token| !is_trivia(token.kind()))
                    .and_then(|token| classes.0.get(&token.start()))
                    .copied()
                    .filter(|class_range| {
                        indentation(source, class_range.start()) == first.indentation
                    });
                let Some(class_range) = class_range else {
                    return Err(SoacDirectiveError::new(
                        SoacDirectiveErrorKind::DanglingClass,
                        range,
                        "SOAC class directive must precede the next class and its decorators at the same indentation",
                    ));
                };
                if !seen_classes.insert(class_range) {
                    return Err(duplicate(range, "class"));
                }
                SoacDirectiveTarget::Class { class_range }
            }
        };
        directives.push(SoacDirective {
            range,
            target,
            strict_assign,
            checked_attr,
        });
        index += 1;
    }
    Ok(directives)
}

fn trim_horizontal(text: &str) -> &str {
    text.trim_matches([' ', '\t', '\x0c'])
}

fn indentation(source: &str, offset: TextSize) -> Option<&str> {
    let before = &source[..offset.to_usize()];
    let start = before.rfind(['\n', '\r']).map_or(0, |index| index + 1);
    let prefix = &before[start..];
    prefix
        .bytes()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\x0c'))
        .then_some(prefix)
}

fn directive_spelling(comment: &str) -> Option<&str> {
    let rest = trim_horizontal(comment).strip_prefix("soac")?;
    (rest.is_empty() || rest.starts_with([':', ' ', '\t', '\x0c'])).then_some(rest)
}

fn directive_head(text: &str, range: TextRange) -> Result<(Scope, &str), SoacDirectiveError> {
    let text = trim_horizontal(text);
    let Some((name, arguments)) = text.split_once('(') else {
        return Err(malformed(
            range,
            "expected a scope and parenthesized options",
        ));
    };
    let scope = match trim_horizontal(name) {
        "package" => Scope::Package,
        "module" => Scope::Module,
        "class" => Scope::Class,
        other => {
            return Err(SoacDirectiveError::new(
                SoacDirectiveErrorKind::UnknownScope,
                range,
                format!("unknown SOAC directive scope {other:?}"),
            ));
        }
    };
    Ok((scope, arguments))
}

fn complete_arguments(
    source: &str,
    comments: &[Comment<'_>],
    index: &mut usize,
    first_arguments: &str,
) -> Result<(String, TextRange), SoacDirectiveError> {
    let first = comments[*index];
    let mut current = first_arguments;
    let mut arguments = String::new();
    loop {
        let range = TextRange::new(first.range.start(), comments[*index].range.end());
        if let Some((last, trailing)) = current.split_once(')') {
            if !trim_horizontal(trailing).is_empty() {
                return Err(malformed(range, "unexpected text after ')'"));
            }
            arguments.push_str(last);
            return Ok((arguments, range));
        }
        arguments.push_str(current);
        arguments.push(' ');
        let Some(next) = comments.get(*index + 1).copied() else {
            return Err(malformed(range, "unterminated parenthesized options"));
        };
        let gap = &source[TextRange::new(comments[*index].range.end(), next.range.start())];
        let next_indent = gap
            .strip_prefix("\r\n")
            .or_else(|| gap.strip_prefix('\n'))
            .or_else(|| gap.strip_prefix('\r'));
        if next.indentation != first.indentation
            || next.parenthesized
            || next_indent != first.indentation
            || directive_spelling(next.text).is_some()
        {
            return Err(malformed(
                range,
                "options must close on consecutive continuation comments at the same indentation",
            ));
        }
        *index += 1;
        current = next.text;
    }
}

fn parse_arguments(
    arguments: &str,
    scope: Scope,
    range: TextRange,
) -> Result<(Option<bool>, Option<bool>), SoacDirectiveError> {
    let arguments = trim_horizontal(arguments);
    if arguments.is_empty() {
        return Ok((None, None));
    }
    let mut strict_assign = None;
    let mut checked_attr = None;
    let mut parts = arguments.split(',').peekable();
    while let Some(part) = parts.next() {
        let part = trim_horizontal(part);
        if part.is_empty() && parts.peek().is_none() {
            // Exactly one final comma is allowed; an earlier empty part fails.
            break;
        }
        let Some((name, value)) = part.split_once('=') else {
            return Err(malformed(range, "expected 'key=true' or 'key=false'"));
        };
        let name = trim_horizontal(name);
        let slot = match name {
            "strict_assign" if matches!(scope, Scope::Class) => {
                return Err(SoacDirectiveError::new(
                    SoacDirectiveErrorKind::WrongScope,
                    range,
                    "strict_assign is not a class option; select it on a module or package",
                ));
            }
            "strict_assign" => &mut strict_assign,
            "checked_attr" => &mut checked_attr,
            other => {
                return Err(SoacDirectiveError::new(
                    SoacDirectiveErrorKind::UnknownKey,
                    range,
                    format!("unknown SOAC {} option {other:?}", scope.name()),
                ));
            }
        };
        if slot.is_some() {
            return Err(SoacDirectiveError::new(
                SoacDirectiveErrorKind::DuplicateKey,
                range,
                format!("duplicate SOAC option {name:?}"),
            ));
        }
        *slot = Some(match trim_horizontal(value) {
            "true" => true,
            "false" => false,
            other => {
                return Err(SoacDirectiveError::new(
                    SoacDirectiveErrorKind::InvalidBoolean,
                    range,
                    format!("SOAC option {name:?} requires true or false, not {other:?}"),
                ));
            }
        });
    }
    Ok((strict_assign, checked_attr))
}

fn module_header_end(suite: &[Stmt]) -> Option<TextSize> {
    let skip_docstring = usize::from(matches!(
        suite.first(),
        Some(Stmt::Expr(statement)) if matches!(statement.value.as_ref(), Expr::StringLiteral(_))
    ));
    suite[skip_docstring..]
        .iter()
        .find(|statement| {
            !matches!(statement, Stmt::ImportFrom(import)
                if import.level == 0 && import.module.as_deref() == Some("__future__"))
        })
        .map(Ranged::start)
}

#[derive(Default)]
struct ClassRanges(BTreeMap<TextSize, TextRange>);

impl<'a> StatementVisitor<'a> for ClassRanges {
    fn visit_stmt(&mut self, statement: &'a Stmt) {
        if let Stmt::ClassDef(class) = statement {
            self.0.insert(class.start(), class.range());
        }
        statement_visitor::walk_stmt(self, statement);
    }
}

fn is_trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Comment
            | TokenKind::Newline
            | TokenKind::NonLogicalNewline
            | TokenKind::Indent
            | TokenKind::Dedent
            | TokenKind::EndOfFile
    )
}

fn malformed(range: TextRange, detail: &str) -> SoacDirectiveError {
    SoacDirectiveError::new(
        SoacDirectiveErrorKind::Malformed,
        range,
        format!("malformed SOAC directive: {detail}"),
    )
}

fn duplicate(range: TextRange, scope: &str) -> SoacDirectiveError {
    SoacDirectiveError::new(
        SoacDirectiveErrorKind::DuplicateDirective,
        range,
        format!("duplicate SOAC {scope} directive"),
    )
}

#[cfg(test)]
mod tests;
