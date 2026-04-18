use super::rewrite;
use ruff_python_ast::{Expr, Stmt};
use ruff_python_parser::parse_module;
use std::collections::HashSet;

fn rewrite_module(source: &str) -> (HashSet<String>, Vec<Stmt>) {
    let mut module = parse_module(source)
        .expect("parse should succeed")
        .into_syntax();
    let future_features = rewrite(&mut module.body).expect("future imports should be valid");
    (future_features, module.body)
}

#[test]
fn strips_all_future_imports_and_returns_feature_names() {
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
    let [Stmt::AnnAssign(assign)] = module.as_slice() else {
        panic!("expected one annotation stmt after future-strip, got {module:?}");
    };
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
    let [Stmt::AnnAssign(assign)] = module.as_slice() else {
        panic!("expected one annotation stmt after future-strip, got {module:?}");
    };
    assert!(matches!(
        assign.annotation.as_ref(),
        Expr::Name(annotation) if annotation.id.as_str() == "Foo"
    ));
}

#[test]
fn invalid_future_import_reports_parse_error() {
    let source = "from __future__ import not_a_feature\nx = 1\n";
    let mut module = parse_module(source)
        .expect("parse should succeed")
        .into_syntax();

    let err = rewrite(&mut module.body).expect_err("future import should be invalid");

    assert!(err.to_string().contains("not_a_feature"), "{err}");
}
