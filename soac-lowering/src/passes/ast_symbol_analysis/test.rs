use super::*;
use crate::py_stmt;

#[test]
fn collect_bound_names_stays_in_current_scope() {
    let stmts = vec![
        py_stmt!("x = 1"),
        py_stmt!("for item in values:\n    seen = item"),
        py_stmt!("with ctx() as bound:\n    used = bound"),
        py_stmt!("try:\n    pass\nexcept ValueError as err:\n    recovered = err"),
        py_stmt!("del removed"),
        py_stmt!("def inner():\n    nested = 1"),
        py_stmt!("class Thing:\n    member = 1"),
    ];

    let names = collect_bound_names(&stmts);

    for expected in [
        "x",
        "item",
        "seen",
        "bound",
        "used",
        "err",
        "recovered",
        "removed",
        "inner",
        "Thing",
    ] {
        assert!(names.contains(expected), "missing {expected} in {names:?}");
    }
    assert!(!names.contains("nested"), "{names:?}");
    assert!(!names.contains("member"), "{names:?}");
}

#[test]
fn collect_explicit_global_or_nonlocal_names_skips_nested_defs() {
    let Stmt::FunctionDef(outer) = py_stmt!(
            "def outer():\n    global module_name\n    if flag:\n        nonlocal captured\n    def inner():\n        global nested\n"
        ) else {
            unreachable!();
        };

    let names = collect_explicit_global_or_nonlocal_names(&outer.body);

    assert!(names.contains("module_name"), "{names:?}");
    assert!(names.contains("captured"), "{names:?}");
    assert!(!names.contains("nested"), "{names:?}");
}

#[test]
fn collect_loaded_names_stays_in_current_scope() {
    let stmts = vec![
        py_stmt!("x = seen + global_name"),
        py_stmt!("if flag:\n    used = value"),
        py_stmt!("def inner():\n    return nested"),
        py_stmt!("class Thing:\n    member = other"),
        py_stmt!("items = [item + outer for item in source]"),
        py_stmt!("fn = lambda arg: arg + captured"),
    ];

    let names = collect_loaded_names(&stmts);

    for expected in ["seen", "global_name", "flag", "value"] {
        assert!(names.contains(expected), "missing {expected} in {names:?}");
    }
    for skipped in ["nested", "other", "item", "outer", "source", "captured"] {
        assert!(!names.contains(skipped), "{names:?}");
    }
}

#[test]
fn collect_loaded_names_includes_function_and_class_headers() {
    let stmts = vec![
        py_stmt!("@decorator(dep)\ndef fn(arg = default_value) -> result_ty:\n    return nested"),
        py_stmt!("@class_deco(dep)\nclass Thing(base_expr):\n    member = nested"),
    ];

    let names = collect_loaded_names(&stmts);

    for expected in [
        "decorator",
        "dep",
        "default_value",
        "result_ty",
        "class_deco",
        "base_expr",
    ] {
        assert!(names.contains(expected), "missing {expected} in {names:?}");
    }
    for skipped in ["nested", "member"] {
        assert!(!names.contains(skipped), "{names:?}");
    }
}

#[test]
fn collect_bound_names_includes_imports_and_named_expr_targets() {
    let stmts = vec![
        py_stmt!("import pkg.sub"),
        py_stmt!("from pkg import thing as alias"),
        py_stmt!("if (captured := value):\n    pass"),
    ];

    let names = collect_bound_names(&stmts);

    for expected in ["pkg", "alias", "captured"] {
        assert!(names.contains(expected), "missing {expected} in {names:?}");
    }
}
