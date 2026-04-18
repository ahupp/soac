use super::*;
use crate::passes::ast_to_ast::body::Suite;
use ruff_python_ast::{self as ast, name::Name, Expr, Pattern, Stmt};
use ruff_python_parser::parse_expression;
use ruff_text_size::TextRange;
use tracing::{enabled, trace, Level};

use crate::{py_expr, py_stmt, ruff_ast_to_string};

enum PatternTest {
    Test { expr: Expr, assigns: Vec<Stmt> },
    Wildcard { assigns: Vec<Stmt> },
}

fn body_to_vec(body: Suite) -> Vec<Stmt> {
    body
}

fn fold_exprs(exprs: Vec<Expr>, op: ast::BoolOp) -> Expr {
    if exprs.is_empty() {
        panic!("Empty expression list");
    } else {
        Expr::BoolOp(ast::ExprBoolOp {
            range: TextRange::default(),
            node_index: ast::AtomicNodeIndex::default(),
            op,
            values: exprs,
        })
    }
}

fn integer_expr(value: usize) -> Expr {
    *parse_expression(&value.to_string())
        .expect("parse error")
        .into_syntax()
        .body
}

fn trace_expr(label: &str, expr: &Expr) {
    if !enabled!(Level::TRACE) {
        return;
    }
    let stmt = Stmt::Expr(ast::StmtExpr {
        value: Box::new(expr.clone()),
        range: TextRange::default(),
        node_index: ast::AtomicNodeIndex::default(),
    });
    trace!("{label}: {}", ruff_ast_to_string(&stmt).trim_end());
}

fn test_for_pattern(pattern: &Pattern, subject: Expr) -> PatternTest {
    use ast::{
        PatternMatchOr, PatternMatchSequence, PatternMatchSingleton, PatternMatchValue, Singleton,
    };
    use PatternTest::*;
    if enabled!(Level::TRACE) {
        trace!("match_case test_for_pattern pattern={pattern:?}");
        trace_expr("match_case subject", &subject);
    }
    match pattern {
        Pattern::MatchValue(PatternMatchValue { value, .. }) => Test {
            expr: py_expr!(
                "{subject:expr} == {value:expr}",
                subject = subject,
                value = *value.clone()
            ),
            assigns: vec![],
        },
        Pattern::MatchSingleton(PatternMatchSingleton { value, .. }) => {
            let singleton_expr = match value {
                Singleton::None => py_expr!("None"),
                Singleton::True => py_expr!("True"),
                Singleton::False => py_expr!("False"),
            };
            Test {
                expr: py_expr!(
                    "{subject:expr} is {value:expr}",
                    subject = subject,
                    value = singleton_expr
                ),
                assigns: vec![],
            }
        }
        Pattern::MatchOr(PatternMatchOr { patterns, .. }) => {
            let mut branches: Vec<(Expr, Vec<Stmt>)> = Vec::new();
            for p in patterns {
                match test_for_pattern(p, subject.clone()) {
                    Test { expr, assigns } => branches.push((expr, assigns)),
                    Wildcard { assigns } => branches.push((py_expr!("True"), assigns)),
                }
            }

            let tests: Vec<Expr> = branches.iter().map(|(expr, _)| expr.clone()).collect();
            let mut assigns = Vec::new();
            if branches.iter().any(|(_, assigns)| !assigns.is_empty()) {
                let mut chain = py_stmt!("pass");
                for (expr, branch_assigns) in branches.iter().rev() {
                    let block = branch_assigns.clone();
                    chain = py_stmt!(
                        "
if {test:expr}:
    {body:stmt}
else:
    {next:stmt}",
                        test = expr.clone(),
                        body = block,
                        next = chain,
                    );
                }
                assigns.push(chain);
            }

            let test = fold_exprs(tests, ast::BoolOp::Or);
            Test {
                expr: test,
                assigns,
            }
        }
        Pattern::MatchSequence(PatternMatchSequence { patterns, .. }) => {
            let mut tests = vec![
                py_expr!(
                    "hasattr({subject:expr}, '__len__')",
                    subject = subject.clone()
                ),
                py_expr!(
                    "hasattr({subject:expr}, '__getitem__')",
                    subject = subject.clone()
                ),
                py_expr!(
                    "not isinstance({subject:expr}, (str, bytes, bytearray))",
                    subject = subject.clone()
                ),
            ];
            let mut assigns = Vec::new();
            let len_expr = py_expr!("len({subject:expr})", subject = subject.clone());

            if patterns.is_empty() {
                tests.push(py_expr!(
                    "{len:expr} == {count:expr}",
                    len = len_expr.clone(),
                    count = integer_expr(0)
                ));
            } else if let Some(star_index) = patterns
                .iter()
                .position(|pattern| matches!(pattern, Pattern::MatchStar(_)))
            {
                let before = star_index;
                let after = patterns.len() - star_index - 1;
                tests.push(py_expr!(
                    "{len:expr} >= {count:expr}",
                    len = len_expr.clone(),
                    count = integer_expr(before + after)
                ));

                for (index, pattern) in patterns.iter().enumerate() {
                    if index < star_index {
                        let element = py_expr!(
                            "{subject:expr}[{idx:expr}]",
                            subject = subject.clone(),
                            idx = integer_expr(index)
                        );
                        match test_for_pattern(pattern, element) {
                            Test {
                                expr,
                                assigns: mut sub_assigns,
                            } => {
                                tests.push(expr);
                                assigns.append(&mut sub_assigns);
                            }
                            Wildcard {
                                assigns: mut sub_assigns,
                            } => {
                                assigns.append(&mut sub_assigns);
                            }
                        }
                    } else if index == star_index {
                        if let Pattern::MatchStar(ast::PatternMatchStar {
                            name: Some(name), ..
                        }) = pattern
                        {
                            let start_expr = integer_expr(before);
                            let end_expr = py_expr!(
                                "{len:expr} - {offset:expr}",
                                len = len_expr.clone(),
                                offset = integer_expr(after)
                            );
                            let slice_expr = py_expr!(
                                "{subject:expr}[{start:expr}:{end:expr}]",
                                subject = subject.clone(),
                                start = start_expr,
                                end = end_expr
                            );
                            let list_expr =
                                py_expr!("__soac__.list({value:expr})", value = slice_expr);
                            assigns.push(py_stmt!(
                                "{name:id} = {value:expr}",
                                name = name.as_str(),
                                value = list_expr
                            ));
                        }
                    } else {
                        let distance = patterns.len() - index;
                        let offset_expr = integer_expr(distance);
                        let index_expr = py_expr!(
                            "{len:expr} - {offset:expr}",
                            len = len_expr.clone(),
                            offset = offset_expr
                        );
                        let element = py_expr!(
                            "{subject:expr}[{idx:expr}]",
                            subject = subject.clone(),
                            idx = index_expr
                        );
                        match test_for_pattern(pattern, element) {
                            Test {
                                expr,
                                assigns: mut sub_assigns,
                            } => {
                                tests.push(expr);
                                assigns.append(&mut sub_assigns);
                            }
                            Wildcard {
                                assigns: mut sub_assigns,
                            } => {
                                assigns.append(&mut sub_assigns);
                            }
                        }
                    }
                }
            } else {
                tests.push(py_expr!(
                    "{len:expr} == {count:expr}",
                    len = len_expr.clone(),
                    count = integer_expr(patterns.len())
                ));
                for (index, pattern) in patterns.iter().enumerate() {
                    let element = py_expr!(
                        "{subject:expr}[{idx:expr}]",
                        subject = subject.clone(),
                        idx = integer_expr(index)
                    );
                    match test_for_pattern(pattern, element) {
                        Test {
                            expr,
                            assigns: mut sub_assigns,
                        } => {
                            tests.push(expr);
                            assigns.append(&mut sub_assigns);
                        }
                        Wildcard {
                            assigns: mut sub_assigns,
                        } => {
                            assigns.append(&mut sub_assigns);
                        }
                    }
                }
            }

            let test = fold_exprs(tests, ast::BoolOp::And);
            Test {
                expr: test,
                assigns,
            }
        }
        Pattern::MatchMapping(ast::PatternMatchMapping {
            keys,
            patterns,
            rest,
            ..
        }) => {
            let mut tests = vec![
                py_expr!("hasattr({subject:expr}, 'keys')", subject = subject.clone()),
                py_expr!(
                    "hasattr({subject:expr}, '__getitem__')",
                    subject = subject.clone()
                ),
            ];
            let mut assigns = Vec::new();

            for (key, pattern) in keys.iter().zip(patterns.iter()) {
                let contains = py_expr!(
                    "{key:expr} in {subject:expr}",
                    key = key.clone(),
                    subject = subject.clone()
                );
                tests.push(contains);
                let value = py_expr!(
                    "{subject:expr}[{key:expr}]",
                    subject = subject.clone(),
                    key = key.clone()
                );
                match test_for_pattern(pattern, value) {
                    Test {
                        expr,
                        assigns: mut sub_assigns,
                    } => {
                        tests.push(expr);
                        assigns.append(&mut sub_assigns);
                    }
                    Wildcard {
                        assigns: mut sub_assigns,
                    } => {
                        assigns.append(&mut sub_assigns);
                    }
                }
            }

            if let Some(name) = rest {
                assigns.push(py_stmt!(
                    "{name:id} = __soac__.dict({subject:expr})",
                    name = name.as_str(),
                    subject = subject.clone()
                ));
                for key in keys.iter() {
                    assigns.push(py_stmt!(
                        "{name:id}.pop({key:expr}, None)",
                        name = name.as_str(),
                        key = key.clone()
                    ));
                }
            }

            let test = fold_exprs(tests, ast::BoolOp::And);
            Test {
                expr: test,
                assigns,
            }
        }
        Pattern::MatchStar(ast::PatternMatchStar { name, .. }) => {
            let tests = vec![
                py_expr!(
                    "hasattr({subject:expr}, '__len__')",
                    subject = subject.clone()
                ),
                py_expr!(
                    "hasattr({subject:expr}, '__getitem__')",
                    subject = subject.clone()
                ),
                py_expr!(
                    "not isinstance({subject:expr}, (str, bytes, bytearray))",
                    subject = subject.clone()
                ),
            ];
            let expr = fold_exprs(tests, ast::BoolOp::And);
            let mut assigns = Vec::new();
            if let Some(name) = name {
                let list_expr =
                    py_expr!("__soac__.list({subject:expr})", subject = subject.clone());
                assigns.push(py_stmt!(
                    "{name:id} = {value:expr}",
                    name = name.as_str(),
                    value = list_expr
                ));
            }
            Test { expr, assigns }
        }
        Pattern::MatchClass(ast::PatternMatchClass { cls, arguments, .. }) => {
            let mut tests = vec![py_expr!(
                "isinstance({subject:expr}, {cls:expr})",
                subject = subject.clone(),
                cls = *cls.clone()
            )];
            let mut assigns = Vec::new();
            let positional_total = integer_expr(arguments.patterns.len());
            let mut handle_attr =
                |pattern: &Pattern, attr_exists: Expr, attr_value: Expr| match test_for_pattern(
                    pattern, attr_value,
                ) {
                    Test {
                        expr,
                        assigns: mut a,
                    } => {
                        tests.push(attr_exists);
                        tests.push(expr);
                        assigns.append(&mut a);
                    }
                    Wildcard { assigns: mut a } => {
                        tests.push(attr_exists);
                        assigns.append(&mut a);
                    }
                };

            for (i, p) in arguments.patterns.iter().enumerate() {
                let attr_exists = py_expr!(
                    "__soac__.match_class_attr_exists({cls:expr}, {subject:expr}, {idx:expr}, {total:expr})",
                    subject = subject.clone(),
                    cls = *cls.clone(),
                    idx = integer_expr(i),
                    total = positional_total.clone()
                );
                let attr_value = py_expr!(
                    "__soac__.match_class_attr_value({cls:expr}, {subject:expr}, {idx:expr}, {total:expr})",
                    subject = subject.clone(),
                    cls = *cls.clone(),
                    idx = integer_expr(i),
                    total = positional_total.clone()
                );
                handle_attr(p, attr_exists, attr_value);
            }

            for kw in &arguments.keywords {
                let attr_exists = py_expr!(
                    "hasattr({subject:expr}, {name:literal})",
                    subject = subject.clone(),
                    name = kw.attr.as_str()
                );
                let attr_value = py_expr!(
                    "getattr({subject:expr}, {name:literal})",
                    subject = subject.clone(),
                    name = kw.attr.as_str()
                );
                handle_attr(&kw.pattern, attr_exists, attr_value);
            }

            let test = fold_exprs(tests, ast::BoolOp::And);
            Test {
                expr: test,
                assigns,
            }
        }
        Pattern::MatchAs(ast::PatternMatchAs {
            pattern: None,
            name: None,
            ..
        }) => Wildcard { assigns: vec![] },
        Pattern::MatchAs(ast::PatternMatchAs {
            pattern,
            name: Some(name),
            ..
        }) => {
            let assign = py_stmt!(
                "{name:id} = {subject:expr}",
                name = name.as_str(),
                subject = subject.clone(),
            );
            match pattern {
                Some(p) => match test_for_pattern(p, subject) {
                    Test { expr, mut assigns } => {
                        assigns.push(assign);
                        Test { expr, assigns }
                    }
                    Wildcard { mut assigns } => {
                        assigns.push(assign);
                        Wildcard { assigns }
                    }
                },
                None => Wildcard {
                    assigns: vec![assign],
                },
            }
        }
        Pattern::MatchAs(ast::PatternMatchAs {
            pattern: Some(p),
            name: None,
            ..
        }) => test_for_pattern(p, subject),
    }
}

fn assigned_names(stmts: &[Stmt]) -> Vec<Name> {
    let mut names = Vec::new();
    for stmt in stmts {
        if let Stmt::Assign(ast::StmtAssign { targets, .. }) = stmt {
            if let Some(Expr::Name(ast::ExprName { id, .. })) = targets.first() {
                if !names.iter().any(|existing| existing == id) {
                    names.push(id.clone());
                }
            }
        }
    }
    names
}

pub(crate) fn rewrite_match_stmt(context: &Context, match_stmt: ast::StmtMatch) -> Rewrite {
    if match_stmt.cases.is_empty() {
        return Rewrite::Unmodified(match_stmt.into());
    }

    let ast::StmtMatch { subject, cases, .. } = match_stmt;

    let subject_expr = *subject;
    let subject_name = context.fresh("match");
    let tmp_expr = py_expr!("{name:id}", name = subject_name.as_str());
    let assign = py_stmt!(
        "{name:id} = {value:expr}",
        name = subject_name.as_str(),
        value = subject_expr.clone(),
    );
    let subject_tmp = tmp_expr;

    let mut chain = vec![py_stmt!("pass")];
    for case in cases.into_iter().rev() {
        let ast::MatchCase {
            pattern,
            guard,
            mut body,
            ..
        } = case;
        let body = body_to_vec(std::mem::take(&mut body));
        use PatternTest::*;
        match test_for_pattern(&pattern, subject_tmp.clone()) {
            Wildcard { assigns } => {
                if let Some(g) = guard {
                    let mut cleanup = assigned_names(&assigns)
                        .into_iter()
                        .map(|name| py_stmt!("del {name:id}", name = name.as_str()))
                        .collect::<Vec<_>>();
                    cleanup.extend(chain.clone());

                    let guard = py_stmt!(
                        "
if {guard:expr}:
    {body:stmt}
else:
    {cleanup:stmt}",
                        guard = *g,
                        body = body,
                        cleanup = cleanup,
                    );

                    let mut block = assigns;
                    block.push(guard);
                    chain = block;
                } else {
                    let mut block = assigns;
                    block.extend(body);
                    chain = block;
                }
            }
            Test {
                expr: test_expr,
                assigns,
            } => {
                if let Some(g) = guard {
                    let mut cleanup = assigned_names(&assigns)
                        .into_iter()
                        .map(|name| py_stmt!("del {name:id}", name = name.as_str()))
                        .collect::<Vec<_>>();
                    cleanup.extend(chain.clone());

                    let guard = py_stmt!(
                        "
if {guard:expr}:
    {body:stmt}
else:
    {cleanup:stmt}",
                        guard = *g,
                        body = body,
                        cleanup = cleanup,
                    );

                    let mut block = assigns;
                    block.push(guard);
                    chain = vec![py_stmt!(
                        "
if {test:expr}:
    {body:stmt}
else:
    {next:stmt}",
                        test = test_expr,
                        body = block,
                        next = chain,
                    )];
                } else {
                    let mut block = assigns;
                    block.extend(body);
                    chain = vec![py_stmt!(
                        "
if {test:expr}:
    {body:stmt}
else:
    {next:stmt}",
                        test = test_expr,
                        body = block,
                        next = chain,
                    )];
                }
            }
        }
    }

    Rewrite::Walk(crate::py_stmts!(
        "
{assign:stmt}
{chain:stmt}",
        assign = assign,
        chain = chain,
    ))
}

#[cfg(test)]
mod test;
