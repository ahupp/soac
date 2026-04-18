use std::collections::HashSet;

use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::TextRange;

use crate::passes::ast_to_ast::ast_rewrite::ExprRewritePass;
use crate::passes::ast_to_ast::scope_helpers::ScopeKind;
use crate::transformer::{walk_expr, Transformer};
use crate::{
    passes::ast_to_ast::{ast_rewrite::LoweredExpr, context::Context},
    py_expr, py_stmt, py_stmt_typed,
};
use ruff_python_ast::Identifier;

pub mod comprehension;

fn lower_generator_expr(
    context: &Context,
    mut elt: Expr,
    mut generators: Vec<ast::Comprehension>,
) -> LoweredExpr {
    let scope = context.current_scope();
    let named_targets = collect_named_expr_targets(&elt, &generators);
    let mut global_targets: HashSet<String> = HashSet::new();
    let mut class_targets: HashSet<String> = HashSet::new();
    let mut nonlocal_targets: HashSet<String> = HashSet::new();
    let mut dummy_targets: Vec<String> = Vec::new();

    match scope.kind {
        ScopeKind::Module => {
            global_targets = named_targets;
        }
        ScopeKind::Class => {
            class_targets = named_targets;
        }
        ScopeKind::Function => {
            for name in named_targets {
                if scope.globals.contains(&name) {
                    global_targets.insert(name);
                } else {
                    nonlocal_targets.insert(name.clone());
                    if !scope.nonlocals.contains(&name) {
                        dummy_targets.push(name);
                    }
                }
            }
        }
    }

    if !class_targets.is_empty() {
        let mut rewriter = NamedExprRewriter::new(&class_targets);
        rewriter.visit_expr(&mut elt);
        for gen in &mut generators {
            rewriter.visit_expr(&mut gen.iter);
            for if_expr in &mut gen.ifs {
                rewriter.visit_expr(if_expr);
            }
        }
    }

    let first_gen = match generators.first() {
        Some(gen) => gen,
        None => {
            return LoweredExpr::unmodified(Expr::Generator(ast::ExprGenerator {
                elt: Box::new(elt),
                generators,
                parenthesized: true,
                range: TextRange::default(),
                node_index: ast::AtomicNodeIndex::default(),
            }))
        }
    };

    let outer_async = first_gen.is_async;
    let iter_expr = first_gen.iter.clone();
    let iter_value = if outer_async {
        py_expr!("__soac__.aiter({iter:expr})", iter = iter_expr.clone())
    } else {
        py_expr!("__soac__.iter({iter:expr})", iter = iter_expr.clone())
    };

    let func_name = context.fresh("genexpr");
    let iter_param = context.fresh("iter");
    let is_async = genexpr_requires_async(&elt, &generators);
    let mut func_def: ast::StmtFunctionDef = if is_async {
        py_stmt_typed!(
            r#"
async def {func:id}({param:id}):
    pass
"#,
            func = func_name.as_str(),
            param = iter_param.as_str(),
        )
    } else {
        py_stmt_typed!(
            r#"
def {func:id}({param:id}):
    pass
"#,
            func = func_name.as_str(),
            param = iter_param.as_str(),
        )
    };

    let iter_param_expr = py_expr!("{name:id}", name = iter_param.as_str());
    let mut body = vec![py_stmt!("yield {value:expr}", value = elt)];
    let gen_count = generators.len();
    for (rev_index, gen) in generators.into_iter().rev().enumerate() {
        let is_outermost = rev_index + 1 == gen_count;
        let loop_body = wrap_ifs(body, gen.ifs);
        let iter = if is_outermost {
            iter_param_expr.clone()
        } else {
            gen.iter
        };
        let for_stmts = if gen.is_async {
            if is_outermost {
                let iter_name = context.fresh("iter");
                let target_tmp = context.fresh("tmp");
                crate::py_stmts!(
                    r#"
{iter_name:id} = {iter:expr}
while True:
    {target_tmp:id} = await __soac__.anext_or_sentinel({iter_name:id})
    if {target_tmp:id} is __soac__.ITER_COMPLETE:
        break
    else:
        {target:expr} = {target_tmp:id}
        {body:stmt}
"#,
                    iter_name = iter_name.as_str(),
                    iter = iter,
                    target_tmp = target_tmp.as_str(),
                    target = gen.target,
                    body = loop_body,
                )
            } else {
                vec![py_stmt!(
                    r#"
async for {target:expr} in {iter:expr}:
    {body:stmt}
"#,
                    target = gen.target,
                    iter = iter,
                    body = loop_body,
                )]
            }
        } else if is_outermost {
            let iter_name = context.fresh("iter");
            let target_tmp = context.fresh("tmp");
            crate::py_stmts!(
                r#"
{iter_name:id} = {iter:expr}
while True:
    {target_tmp:id} = __soac__.next_or_sentinel({iter_name:id})
    if {target_tmp:id} is __soac__.ITER_COMPLETE:
        break
    else:
        {target:expr} = {target_tmp:id}
        {body:stmt}
"#,
                iter_name = iter_name.as_str(),
                iter = iter,
                target_tmp = target_tmp.as_str(),
                target = gen.target,
                body = loop_body,
            )
        } else {
            vec![py_stmt!(
                r#"
for {target:expr} in {iter:expr}:
    {body:stmt}
"#,
                target = gen.target,
                iter = iter,
                body = loop_body,
            )]
        };
        body = for_stmts;
    }

    let mut func_body: Vec<Stmt> = Vec::new();
    if !global_targets.is_empty() {
        let mut names = global_targets.into_iter().collect::<Vec<_>>();
        names.sort();
        let names = names
            .into_iter()
            .map(|name| Identifier::new(name, TextRange::default()))
            .collect();
        func_body.push(Stmt::Global(ast::StmtGlobal {
            names,
            range: TextRange::default(),
            node_index: ast::AtomicNodeIndex::default(),
        }));
    }
    if !nonlocal_targets.is_empty() {
        let mut names = nonlocal_targets.into_iter().collect::<Vec<_>>();
        names.sort();
        let names = names
            .into_iter()
            .map(|name| Identifier::new(name, TextRange::default()))
            .collect();
        func_body.push(Stmt::Nonlocal(ast::StmtNonlocal {
            names,
            range: TextRange::default(),
            node_index: ast::AtomicNodeIndex::default(),
        }));
    }
    func_body.extend(body);
    func_def.body = func_body;

    let mut prefix: Vec<Stmt> = Vec::new();
    for name in dummy_targets {
        prefix.push(py_stmt!(
            r#"
if False:
    {name:id} = None
"#,
            name = name.as_str()
        ));
    }
    prefix.push(func_def.into());

    let call_expr = py_expr!(
        "{func:id}({iter:expr})",
        func = func_name.as_str(),
        iter = iter_value,
    );

    LoweredExpr::modified(call_expr, prefix)
}

fn genexpr_requires_async(elt: &Expr, generators: &[ast::Comprehension]) -> bool {
    if generators.iter().any(|gen| gen.is_async) {
        return true;
    }
    if expr_requires_async(elt) {
        return true;
    }
    for gen in generators {
        if expr_requires_async(&gen.iter) {
            return true;
        }
        for if_expr in &gen.ifs {
            if expr_requires_async(if_expr) {
                return true;
            }
        }
    }
    false
}

fn comp_is_async(
    elt_or_key: &Expr,
    value: Option<&Expr>,
    generators: &[ast::Comprehension],
) -> bool {
    if generators.iter().any(|gen| gen.is_async) {
        return true;
    }
    if expr_requires_async(elt_or_key) {
        return true;
    }
    if let Some(value) = value {
        if expr_requires_async(value) {
            return true;
        }
    }
    for gen in generators {
        if expr_requires_async(&gen.iter) {
            return true;
        }
        for if_expr in &gen.ifs {
            if expr_requires_async(if_expr) {
                return true;
            }
        }
    }
    false
}

fn expr_requires_async(expr: &Expr) -> bool {
    #[derive(Default)]
    struct AwaitFinder {
        found: bool,
    }

    impl Transformer for AwaitFinder {
        fn visit_expr(&mut self, expr: &mut Expr) {
            if self.found {
                return;
            }
            match expr {
                Expr::Await(_) => {
                    self.found = true;
                    return;
                }
                Expr::ListComp(ast::ExprListComp {
                    elt, generators, ..
                }) => {
                    if comp_is_async(elt.as_ref(), None, generators) {
                        self.found = true;
                    }
                    return;
                }
                Expr::SetComp(ast::ExprSetComp {
                    elt, generators, ..
                }) => {
                    if comp_is_async(elt.as_ref(), None, generators) {
                        self.found = true;
                    }
                    return;
                }
                Expr::DictComp(ast::ExprDictComp {
                    key,
                    value,
                    generators,
                    ..
                }) => {
                    if comp_is_async(key.as_ref(), Some(value.as_ref()), generators) {
                        self.found = true;
                    }
                    return;
                }
                Expr::Lambda(_) | Expr::Generator(_) => {
                    return;
                }
                _ => {}
            }
            walk_expr(self, expr);
        }
    }

    let mut finder = AwaitFinder::default();
    let mut expr = expr.clone();
    finder.visit_expr(&mut expr);
    finder.found
}

fn wrap_ifs(mut body: Vec<Stmt>, ifs: Vec<Expr>) -> Vec<Stmt> {
    for if_expr in ifs.into_iter().rev() {
        body = vec![py_stmt!(
            r#"
if {test:expr}:
    {body:stmt}
"#,
            test = if_expr,
            body = body,
        )];
    }
    body
}

fn collect_named_expr_targets(elt: &Expr, generators: &[ast::Comprehension]) -> HashSet<String> {
    let mut collector = NamedExprTargetCollector::default();
    let mut elt_clone = elt.clone();
    collector.visit_expr(&mut elt_clone);
    for gen in generators {
        let mut iter_clone = gen.iter.clone();
        collector.visit_expr(&mut iter_clone);
        for if_expr in &gen.ifs {
            let mut if_clone = if_expr.clone();
            collector.visit_expr(&mut if_clone);
        }
    }
    collector.names
}

#[derive(Default)]
struct NamedExprTargetCollector {
    names: HashSet<String>,
}

impl Transformer for NamedExprTargetCollector {
    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Named(ast::ExprNamed { target, value, .. }) => {
                if let Expr::Name(ast::ExprName { id, .. }) = target.as_ref() {
                    self.names.insert(id.as_str().to_string());
                } else {
                    self.visit_expr(target);
                }
                self.visit_expr(value);
                return;
            }
            Expr::Lambda(_) | Expr::Generator(_) => {
                return;
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

struct NamedExprRewriter<'a> {
    class_targets: &'a HashSet<String>,
}

impl<'a> NamedExprRewriter<'a> {
    fn new(class_targets: &'a HashSet<String>) -> Self {
        Self { class_targets }
    }
}

impl Transformer for NamedExprRewriter<'_> {
    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Named(ast::ExprNamed { target, value, .. }) => {
                if let Expr::Name(ast::ExprName { id, .. }) = target.as_ref() {
                    let name = id.as_str();
                    if self.class_targets.contains(name) {
                        self.visit_expr(value.as_mut());
                        *expr = py_expr!(
                            "__soac__.store_global(_dp_class_ns, {name:literal}, {value:expr})",
                            name = name,
                            value = value.as_ref().clone(),
                        );
                        return;
                    }
                }
                self.visit_expr(target.as_mut());
                self.visit_expr(value.as_mut());
                return;
            }
            Expr::Lambda(_) | Expr::Generator(_) => {
                return;
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

pub struct ScopedHelperExprPass;

impl ExprRewritePass for ScopedHelperExprPass {
    fn lower_expr(&self, context: &Context, expr: Expr) -> LoweredExpr {
        match expr {
            Expr::ListComp(ast::ExprListComp {
                elt, generators, ..
            }) => comprehension::lower_list_comp(context, *elt, generators),
            Expr::SetComp(ast::ExprSetComp {
                elt, generators, ..
            }) => comprehension::lower_set_comp(context, *elt, generators),
            Expr::DictComp(ast::ExprDictComp {
                key,
                value,
                generators,
                ..
            }) => comprehension::lower_dict_comp(context, *key, *value, generators),
            Expr::Generator(ast::ExprGenerator {
                elt, generators, ..
            }) => lower_generator_expr(context, *elt, generators),
            other => LoweredExpr::unmodified(other),
        }
    }
}
