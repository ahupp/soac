use super::*;
use ruff_python_parser::parse_module;

fn parse(source: &str, is_package_init: bool) -> Result<Vec<SoacDirective>, SoacDirectiveError> {
    let parsed = parse_module(source).expect("fixture is valid Python syntax");
    parse_soac_directives(source, parsed.tokens(), parsed.suite(), is_package_init)
}

fn expect_error(source: &str, kind: SoacDirectiveErrorKind) -> SoacDirectiveError {
    let error = parse(source, false).expect_err(source);
    assert_eq!(error.kind(), kind, "{source}");
    assert!(source[error.range()].starts_with('#'));
    error
}

#[test]
fn module_and_package_values_remain_explicit_without_resolving_inheritance() {
    let source = "\
#!/usr/bin/python3
# coding: utf-8
# soac: package(strict_assign=false)
\"\"\"Module documentation.\"\"\"
from __future__ import annotations
# soac: module(checked_attr=true)
value = 1
";
    let directives = parse(source, true).unwrap();
    assert_eq!(directives.len(), 2);
    assert_eq!(directives[0].target, SoacDirectiveTarget::Package);
    assert_eq!(directives[0].strict_assign, Some(false));
    assert_eq!(directives[0].checked_attr, None);
    assert_eq!(
        &source[directives[0].range],
        "# soac: package(strict_assign=false)",
    );
    assert_eq!(directives[1].target, SoacDirectiveTarget::Module);
    assert_eq!(directives[1].strict_assign, None);
    assert_eq!(directives[1].checked_attr, Some(true));
    assert!(directives[0].range.end() < directives[1].range.start());
}

#[test]
fn accepts_both_keys_false_values_trailing_comma_and_empty_options() {
    for (source, strict_assign, checked_attr) in [
        (
            "#soac : module ( strict_assign = true , checked_attr = false , )\n",
            Some(true),
            Some(false),
        ),
        (
            "# soac: module(checked_attr=true, strict_assign=false)\n",
            Some(false),
            Some(true),
        ),
        ("# soac: module()\n", None, None),
    ] {
        let directives = parse(source, false).unwrap();
        assert_eq!(directives.len(), 1);
        assert_eq!(directives[0].target, SoacDirectiveTarget::Module);
        assert_eq!(directives[0].strict_assign, strict_assign);
        assert_eq!(directives[0].checked_attr, checked_attr);
    }
}

#[test]
fn binds_decorated_and_nested_classes_to_actual_distinct_ast_ranges() {
    let source = "\
# soac: class(checked_attr=false)
# ordinary explanatory comment

@decorate
class Same:
    # soac: class(checked_attr=true)
    @decorate
    class Same:
        pass

def factory():
    # soac: class(checked_attr=false)
    class Same:
        pass
    return Same
";
    let parsed = parse_module(source).unwrap();
    let directives = parse_soac_directives(source, parsed.tokens(), parsed.suite(), false).unwrap();
    let Stmt::ClassDef(outer) = &parsed.suite()[0] else {
        panic!("expected outer class");
    };
    let Stmt::ClassDef(inner) = &outer.body[0] else {
        panic!("expected nested class");
    };
    let Stmt::FunctionDef(factory) = &parsed.suite()[1] else {
        panic!("expected factory");
    };
    let Stmt::ClassDef(local) = &factory.body[0] else {
        panic!("expected function-local class");
    };
    assert_eq!(directives.len(), 3);
    for (directive, class, checked) in [
        (&directives[0], outer, false),
        (&directives[1], inner, true),
        (&directives[2], local, false),
    ] {
        assert_eq!(
            directive.target,
            SoacDirectiveTarget::Class {
                class_range: class.range(),
            },
        );
        assert_eq!(directive.strict_assign, None);
        assert_eq!(directive.checked_attr, Some(checked));
        assert!(directive.range.end() < class.start());
    }
    assert_ne!(outer.range(), inner.range());
    assert_ne!(inner.range(), local.range());
}

#[test]
fn binds_classes_in_conditional_suites_and_tab_indented_scopes() {
    let source = "\
if condition:
    # soac: class(checked_attr=true)
    class Conditional:
        pass
else:
    # soac: class(checked_attr=false)
    class Conditional:
        pass
def factory():
\t# soac: class(checked_attr=true)
\tclass Local:
\t\tpass
";
    let directives = parse(source, false).unwrap();
    assert_eq!(directives.len(), 3);
    assert_eq!(
        directives
            .iter()
            .map(|item| item.checked_attr)
            .collect::<Vec<_>>(),
        [Some(true), Some(false), Some(true)],
    );
    let class_ranges = directives
        .iter()
        .map(|directive| match directive.target {
            SoacDirectiveTarget::Class { class_range } => class_range,
            _ => panic!("expected class directive"),
        })
        .collect::<HashSet<_>>();
    assert_eq!(class_ranges.len(), 3);
}

#[test]
fn multiline_comments_preserve_the_whole_source_range() {
    for newline in ["\n", "\r\n"] {
        let block = [
            "# soac: module(",
            "#     strict_assign=false,",
            "#     checked_attr=true,",
            "# )",
        ]
        .join(newline);
        let source = format!("{block}{newline}value = 1{newline}");
        let directives = parse(&source, false).unwrap();
        assert_eq!(directives.len(), 1);
        assert_eq!(&source[directives[0].range], block);
        assert_eq!(directives[0].strict_assign, Some(false));
        assert_eq!(directives[0].checked_attr, Some(true));
    }
    let source = "\
def factory():
    # soac: class(
    #     checked_attr=false,
    # )
    @decorate
    class Local:
        pass
";
    let parsed = parse_module(source).unwrap();
    let directives = parse(source, false).unwrap();
    let Stmt::FunctionDef(factory) = &parsed.suite()[0] else {
        panic!("expected function");
    };
    assert_eq!(
        directives[0].target,
        SoacDirectiveTarget::Class {
            class_range: factory.body[0].range(),
        },
    );
}

#[test]
fn only_actual_soac_comment_tokens_are_directives() {
    let source = r###"
text = """
# soac: package(strict_assign=true)
# soac: module(checked_attr=true)
# soac: class(checked_attr=true)
"""
raw = r"# soac: class(checked_attr=false)"
formatted = f"# soac: module(checked_attr=true) {value}"
template = t"# soac: module(checked_attr=true) {value}"
# soacish: module(checked_attr=true)
# example: # soac: module(checked_attr=true)
value = "# soac: class(checked_attr=true)"
"###;
    assert!(parse(source, true).unwrap().is_empty());
}

#[test]
fn rejects_unknown_or_malformed_options_without_coercion() {
    use SoacDirectiveErrorKind as Kind;
    for (comment, expected) in [
        ("# soac module(checked_attr=true)", Kind::Malformed),
        ("# soac:", Kind::Malformed),
        ("# soac: module", Kind::Malformed),
        ("# soac: function(checked_attr=true)", Kind::UnknownScope),
        ("# soac: Module(checked_attr=true)", Kind::UnknownScope),
        ("# soac: module(check_attr=true)", Kind::UnknownKey),
        ("# soac: module(checked_attr=True)", Kind::InvalidBoolean),
        ("# soac: module(checked_attr=1)", Kind::InvalidBoolean),
        ("# soac: module(checked_attr='true')", Kind::InvalidBoolean),
        ("# soac: module(checked_attr=)", Kind::InvalidBoolean),
        (
            "# soac: module(checked_attr=true or false)",
            Kind::InvalidBoolean,
        ),
        ("# soac: module(checked_attr)", Kind::Malformed),
        ("# soac: module(,)", Kind::Malformed),
        ("# soac: module(checked_attr=true,,)", Kind::Malformed),
        (
            "# soac: module(checked_attr=true) trailing",
            Kind::Malformed,
        ),
        ("# soac: module(checked_attr=true))", Kind::Malformed),
        ("# soac: module(checked_attr=true", Kind::Malformed),
        (
            "# soac: module(checked_attr=true, checked_attr=false)",
            Kind::DuplicateKey,
        ),
        (
            "# soac: module(strict_assign=false, strict_assign=false)",
            Kind::DuplicateKey,
        ),
    ] {
        expect_error(&format!("{comment}\nvalue = 1\n"), expected);
    }
    expect_error(
        "# soac: class(strict_assign=false)\nclass A: pass\n",
        Kind::WrongScope,
    );
}

#[test]
fn rejects_duplicate_directives_per_target_not_across_different_classes() {
    for source in [
        "# soac: module(checked_attr=true)\n# soac: module(strict_assign=false)\n",
        "# soac: package(checked_attr=true)\n# soac: package(strict_assign=false)\n",
        "# soac: class(checked_attr=true)\n# another comment\n# soac: class(checked_attr=false)\n@decorate\nclass A: pass\n",
    ] {
        let error = parse(source, true).unwrap_err();
        assert_eq!(error.kind(), SoacDirectiveErrorKind::DuplicateDirective);
        assert!(error.range().start().to_usize() > source.find('\n').unwrap());
    }
    let distinct = "\
# soac: class(checked_attr=true)
class A: pass
# soac: class(checked_attr=false)
class A: pass
";
    let directives = parse(distinct, false).unwrap();
    assert_eq!(directives.len(), 2);
    assert_ne!(directives[0].target, directives[1].target);
}

#[test]
fn enforces_package_context_and_module_header_placement() {
    use SoacDirectiveErrorKind as Kind;
    expect_error(
        "# soac: package(strict_assign=true)\nvalue = 1\n",
        Kind::PackageOutsideInit,
    );
    for source in [
        "value = 1\n# soac: module(checked_attr=true)\n",
        "import os\n# soac: module(checked_attr=true)\n",
        "'docstring'\n'ordinary string statement'\n# soac: module(checked_attr=true)\n",
        "@decorate\n# soac: module(checked_attr=true)\nclass A: pass\n",
        "def factory():\n    # soac: module(checked_attr=true)\n    pass\n",
        "  # soac: module(checked_attr=true)\nvalue = 1\n",
        "value = 1  # soac: module(checked_attr=true)\n",
        "from __future__ import (\n# soac: module(checked_attr=true)\nannotations,\n)\n",
    ] {
        expect_error(source, Kind::InvalidPlacement);
    }
}

#[test]
fn rejects_dangling_class_directives_and_directives_after_decorators() {
    use SoacDirectiveErrorKind as Kind;
    for source in [
        "# soac: class(checked_attr=true)\n",
        "# soac: class(checked_attr=true)\nvalue = 1\nclass A: pass\n",
        "# soac: class(checked_attr=true)\ndef factory():\n    class A: pass\n",
        "# soac: class(checked_attr=true)\nif condition:\n    class A: pass\n",
        "@decorate\n# soac: class(checked_attr=true)\nclass A: pass\n",
        "@first\n# soac: class(checked_attr=true)\n@second\nclass A: pass\n",
        "def factory():\n    pass\n    # soac: class(checked_attr=true)\nclass A: pass\n",
        "def factory():\n    # soac: class(checked_attr=true)\n  class A: pass\n",
    ] {
        expect_error(source, Kind::DanglingClass);
    }
    expect_error(
        "class A: pass  # soac: class(checked_attr=true)\n",
        Kind::InvalidPlacement,
    );
}

#[test]
fn rejects_broken_multiline_comments_and_does_not_consume_new_directives() {
    for source in [
        "# soac: module(\n\n# checked_attr=true)\n",
        "# soac: module(\nvalue = 1\n# checked_attr=true)\n",
        "# soac: module(\n  # checked_attr=true)\n",
        "# soac: module(\n# soac: module(checked_attr=true)\n",
        "# soac: module(\n# checked_attr=true,\n",
        "# soac: module(\n# checked_attr=true\n# ) # trailing text\n",
    ] {
        expect_error(source, SoacDirectiveErrorKind::Malformed);
    }
}

#[test]
fn diagnostic_ranges_are_utf8_byte_offsets_in_the_original_source() {
    let source = "'é module documentation'\n# soac: module(checked_attr=TRUE)\n";
    let error = expect_error(source, SoacDirectiveErrorKind::InvalidBoolean);
    assert_eq!(error.range().start().to_usize(), source.find('#').unwrap());
    assert_eq!(&source[error.range()], "# soac: module(checked_attr=TRUE)",);
}
