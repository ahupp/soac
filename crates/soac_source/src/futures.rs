use ruff_python_ast::Stmt;

/// Identify the explicit strict feature in a module's original parsed suite.
///
/// This selects analysis behavior, not executable authority. Callers must still
/// reject parser/semantic errors, including an incorrectly placed future import,
/// before publishing or consuming a contract. Nested imports and source text
/// inside strings or comments do not select the module's dialect.
pub fn has_strict_future(suite: &[Stmt]) -> bool {
    suite.iter().any(|statement| {
        matches!(statement, Stmt::ImportFrom(import)
            if import.level == 0 && import.module.as_deref() == Some("__future__")
                && import.names.iter().any(|alias| alias.name.as_str() == "strict"))
    })
}

#[cfg(test)]
mod tests {
    use super::has_strict_future;

    #[test]
    fn strict_feature_requires_the_original_top_level_import() {
        for (source, expected) in [
            ("from __future__ import strict\n", true),
            (
                "'module docstring'\nfrom __future__ import (annotations, strict)\n",
                true,
            ),
            ("from __future__ import annotations\n", false),
            ("from another import strict\n", false),
            ("from .__future__ import strict\n", false),
            ("# from __future__ import strict\n", false),
            ("'from __future__ import strict'\n", false),
            ("def nested():\n    from __future__ import strict\n", false),
        ] {
            let parsed = ruff_python_parser::parse_module(source).unwrap();
            assert_eq!(has_strict_future(parsed.suite()), expected, "{source}");
        }
    }
}
