use super::rewrite;
use ruff_python_ast::{Expr, Stmt, Suite};
use ruff_python_parser::parse_module;
use std::collections::HashSet;

fn rewrite_module(source: &str) -> (HashSet<String>, Suite) {
    rewrite_module_with_metadata(source, None)
}

fn rewrite_module_with_metadata(
    source: &str,
    canonical: Option<&crate::CanonicalAnnotationStrings>,
) -> (HashSet<String>, Suite) {
    let mut module = parse_module(source)
        .expect("parse should succeed")
        .into_syntax();
    let future_features =
        rewrite(&mut module.body, canonical, false).expect("future imports should be valid");
    (future_features, module.body)
}

#[test]
fn preserves_future_import_bindings_and_returns_feature_names() {
    let source = concat!(
        "from __future__ import annotations, division\n",
        "from __future__ import generator_stop\n",
        "x: Foo = 1\n",
    );

    let (future_features, module) = rewrite_module(source);

    assert_eq!(
        future_features,
        HashSet::from([
            "annotations".to_string(),
            "division".to_string(),
            "generator_stop".to_string(),
        ])
    );
    let [Stmt::ImportFrom(first), Stmt::ImportFrom(second), Stmt::AnnAssign(assign)] =
        module.as_slice()
    else {
        panic!("expected preserved future imports and annotation, got {module:?}");
    };
    assert_eq!(first.names.len(), 2);
    assert_eq!(second.names[0].name.as_str(), "generator_stop");
    assert!(matches!(
        assign.annotation.as_ref(),
        Expr::StringLiteral(annotation) if annotation.value.to_str() == "Foo"
    ));
}

#[test]
fn non_annotations_future_does_not_stringize_annotations() {
    let source = concat!("from __future__ import division\n", "x: Foo = 1\n",);

    let (future_features, module) = rewrite_module(source);

    assert_eq!(future_features, HashSet::from(["division".to_string()]));
    let [Stmt::ImportFrom(_), Stmt::AnnAssign(assign)] = module.as_slice() else {
        panic!("expected preserved future import and annotation, got {module:?}");
    };
    assert!(matches!(
        assign.annotation.as_ref(),
        Expr::Name(annotation) if annotation.id.as_str() == "Foo"
    ));
}

#[test]
fn strict_feature_uses_imported_name_and_preserves_aliases_and_docstring() {
    let source = concat!(
        "\"module docs\"\n",
        "from __future__ import (strict as policy, annotations as strings)\n",
        "from __future__ import strict\n",
        "value: Thing = 1\n",
    );
    let start = source.find("Thing").unwrap() as u32;
    let canonical = crate::CanonicalAnnotationStrings::from_native_entries(
        source,
        [(
            soac_contracts::SourceRange::new(start, start + 5),
            "Thing".into(),
        )],
    )
    .unwrap();
    let (features, body) = rewrite_module_with_metadata(source, Some(&canonical));
    assert_eq!(
        features,
        HashSet::from(["strict".into(), "annotations".into()])
    );
    assert!(matches!(&body[0], Stmt::Expr(_)));
    let Stmt::ImportFrom(import) = &body[1] else {
        panic!("missing import")
    };
    assert_eq!(import.names[0].name.as_str(), "strict");
    assert_eq!(import.names[0].asname.as_ref().unwrap().as_str(), "policy");
    let (_, aliases) = rewrite_module("from __future__ import annotations as strict\n");
    assert_eq!(aliases.len(), 1);
    let (features, _) = rewrite_module("from .__future__ import strict\n");
    assert!(features.is_empty());
}

#[test]
fn rejects_late_and_nested_future_statements_before_scope_rewriting() {
    for source in [
        "x = 1\nfrom __future__ import strict\n",
        "'doc'\n'not another doc'\nfrom __future__ import annotations\n",
        "def f():\n    from __future__ import strict\n",
        "class C:\n    from __future__ import annotations\n",
        "if True:\n    from __future__ import strict\n",
    ] {
        let mut module = parse_module(source).unwrap().into_syntax();
        let error = rewrite(&mut module.body, None, false).expect_err(source);
        assert!(matches!(
            error.error,
            ruff_python_parser::ParseErrorType::OtherError(_)
        ));
        assert!(
            source[error.location.start().to_usize()..error.location.end().to_usize()]
                .starts_with("from __future__")
        );
    }
}

#[test]
fn invalid_future_import_reports_parse_error() {
    let source = "from __future__ import not_a_feature\nx = 1\n";
    let mut module = parse_module(source)
        .expect("parse should succeed")
        .into_syntax();

    let err = rewrite(&mut module.body, None, false).expect_err("future import should be invalid");

    assert!(err.to_string().contains("not_a_feature"), "{err}");
}

#[test]
fn authenticated_future_annotations_require_their_exact_native_range_entries() {
    for source in [
        "from __future__ import strict, annotations\nvalue: tuple[int, str]\n",
        "# soac: module(checked_attr=true)\nfrom __future__ import annotations\nvalue: tuple[int, str]\n",
    ] {
        let original = parse_module(source).unwrap().into_syntax();
        let mut missing = original.body.clone();
        let error = rewrite(&mut missing, None, true).unwrap_err();
        assert_eq!(
            &source[error.location.start().to_usize()..error.location.end().to_usize()],
            "tuple[int, str]",
        );
        let canonical = crate::CanonicalAnnotationStrings::from_native_entries(source, []).unwrap();
        let mut incomplete = original.body;
        assert!(rewrite(&mut incomplete, Some(&canonical), true).is_err());
    }
}
