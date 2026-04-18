use std::collections::{HashMap, HashSet};

use ruff_python_ast::{self as ast};
use ruff_python_ast::{Expr, ExprContext, Stmt};

use crate::passes::ast_to_ast::ast_rewrite::LoweredExpr;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::scope_helpers::{is_internal_symbol, ScopeKind};
use crate::transformer::{walk_expr, Transformer};
use crate::{py_expr, py_stmt, py_stmt_typed};
use ruff_python_ast::name::Name;
use ruff_text_size::TextRange;

#[derive(Clone, Copy)]
pub(crate) enum InlineCompKind {
    List,
    Set,
    Dict,
}

struct LoadRenamer<'a> {
    renames: &'a HashMap<String, Name>,
}

impl Transformer for LoadRenamer<'_> {
    fn visit_expr(&mut self, expr: &mut Expr) {
        if let Expr::Name(ast::ExprName { id, ctx, .. }) = expr {
            if matches!(ctx, ExprContext::Load) {
                if let Some(new) = self.renames.get(id.as_str()) {
                    *id = new.clone();
                }
                return;
            }
        }
        walk_expr(self, expr);
    }
}

struct TargetRenamer<'a> {
    context: &'a Context,
    renames: &'a mut HashMap<String, Name>,
    bound_here: HashSet<String>,
}

impl<'a> TargetRenamer<'a> {
    fn new(
        context: &'a Context,
        renames: &'a mut HashMap<String, Name>,
        bound_here: HashSet<String>,
    ) -> Self {
        Self {
            context,
            renames,
            bound_here,
        }
    }

    fn ensure_binding(&mut self, name: &str) {
        if self.renames.contains_key(name) {
            return;
        }
        if self.bound_here.contains(name) {
            if is_internal_symbol(name) {
                let fresh = Name::new(self.context.fresh("tmp"));
                self.renames.insert(name.to_string(), fresh);
            } else {
                self.renames.insert(name.to_string(), Name::new(name));
            }
        }
    }
}

impl Transformer for TargetRenamer<'_> {
    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Name(ast::ExprName { id, ctx, .. }) => {
                let name = id.as_str();
                if matches!(ctx, ExprContext::Store) {
                    self.ensure_binding(name);
                    if let Some(existing) = self.renames.get(name).cloned() {
                        *id = existing;
                    }
                    return;
                }
                if matches!(ctx, ExprContext::Load) {
                    self.ensure_binding(name);
                    if let Some(new) = self.renames.get(name) {
                        *id = new.clone();
                    }
                    return;
                }
            }
            Expr::Generator(_) | Expr::ListComp(_) | Expr::SetComp(_) | Expr::DictComp(_) => {
                let mut renamer = LoadRenamer {
                    renames: self.renames,
                };
                (&mut renamer).visit_expr(expr);
                return;
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

fn rename_loads(mut expr: Expr, renames: &HashMap<String, Name>) -> Expr {
    let mut renamer = LoadRenamer { renames };
    (&mut renamer).visit_expr(&mut expr);
    expr
}

fn collect_store_names(target: &Expr) -> HashSet<String> {
    #[derive(Default)]
    struct Collector {
        names: HashSet<String>,
    }

    impl Transformer for Collector {
        fn visit_expr(&mut self, expr: &mut Expr) {
            match expr {
                Expr::Name(ast::ExprName { id, ctx, .. }) => {
                    if matches!(ctx, ExprContext::Store) {
                        self.names.insert(id.to_string());
                        return;
                    }
                }
                Expr::Generator(_) | Expr::ListComp(_) | Expr::SetComp(_) | Expr::DictComp(_) => {
                    return;
                }
                _ => {}
            }
            walk_expr(self, expr);
        }
    }

    let mut collector = Collector::default();
    let mut clone = target.clone();
    collector.visit_expr(&mut clone);
    collector.names
}

struct LoweredGenerator {
    target: Expr,
    iter: Expr,
    ifs: Vec<Expr>,
    is_async: bool,
}

fn wrap_ifs(mut body: Vec<Stmt>, ifs: Vec<Expr>) -> Vec<Stmt> {
    for test in ifs.into_iter().rev() {
        body = vec![py_stmt!(
            r#"
if {test:expr}:
    {body:stmt}
"#,
            test = test,
            body = body,
        )];
    }
    body
}

pub(crate) fn lower_list_comp(
    context: &Context,
    elt: Expr,
    generators: Vec<ast::Comprehension>,
) -> LoweredExpr {
    let is_async = comp_is_async(&elt, None, &generators);
    lower_function(
        context,
        InlineCompKind::List,
        elt,
        None,
        generators,
        is_async,
    )
}

pub(crate) fn lower_set_comp(
    context: &Context,
    elt: Expr,
    generators: Vec<ast::Comprehension>,
) -> LoweredExpr {
    let is_async = comp_is_async(&elt, None, &generators);
    lower_function(
        context,
        InlineCompKind::Set,
        elt,
        None,
        generators,
        is_async,
    )
}

pub(crate) fn lower_dict_comp(
    context: &Context,
    key: Expr,
    value: Expr,
    generators: Vec<ast::Comprehension>,
) -> LoweredExpr {
    let is_async = comp_is_async(&key, Some(&value), &generators);
    lower_function(
        context,
        InlineCompKind::Dict,
        key,
        Some(value),
        generators,
        is_async,
    )
}

fn lower_function(
    context: &Context,
    kind: InlineCompKind,
    mut elt_or_key: Expr,
    mut value: Option<Expr>,
    mut generators: Vec<ast::Comprehension>,
    is_async: bool,
) -> LoweredExpr {
    if generators.is_empty() {
        return LoweredExpr::unmodified(match kind {
            InlineCompKind::List => Expr::ListComp(ast::ExprListComp {
                elt: Box::new(elt_or_key),
                generators,
                range: TextRange::default(),
                node_index: ast::AtomicNodeIndex::default(),
            }),
            InlineCompKind::Set => Expr::SetComp(ast::ExprSetComp {
                elt: Box::new(elt_or_key),
                generators,
                range: TextRange::default(),
                node_index: ast::AtomicNodeIndex::default(),
            }),
            InlineCompKind::Dict => Expr::DictComp(ast::ExprDictComp {
                key: Box::new(elt_or_key),
                value: Box::new(value.unwrap_or_else(|| py_expr!("None"))),
                generators,
                range: TextRange::default(),
                node_index: ast::AtomicNodeIndex::default(),
            }),
        });
    }

    let scope = context.current_scope();
    let named_targets = collect_named_expr_targets(&elt_or_key, value.as_ref(), &generators);
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
        let mut rewriter = super::NamedExprRewriter::new(&class_targets);
        rewriter.visit_expr(&mut elt_or_key);
        if let Some(value_expr) = value.as_mut() {
            rewriter.visit_expr(value_expr);
        }
        for gen in &mut generators {
            rewriter.visit_expr(&mut gen.iter);
            for if_expr in &mut gen.ifs {
                rewriter.visit_expr(if_expr);
            }
        }
    }

    let first_gen = generators
        .first()
        .expect("comprehension expects at least one generator");
    let iter_call = first_gen.iter.clone();

    let result_name = context.fresh("tmp");
    let result_expr = py_expr!("{name:id}", name = result_name.as_str());
    let init_stmt = match kind {
        InlineCompKind::List => py_stmt!("{name:id} = []", name = result_name.as_str()),
        InlineCompKind::Set => py_stmt!("{name:id} = set()", name = result_name.as_str()),
        InlineCompKind::Dict => py_stmt!("{name:id} = {}", name = result_name.as_str()),
    };

    let mut renames: HashMap<String, Name> = HashMap::new();
    let mut lowered_gens = Vec::with_capacity(generators.len());

    for gen in generators {
        let iter_expr = rename_loads(gen.iter, &renames);

        let mut target_expr = gen.target;
        let bound_here = collect_store_names(&target_expr);
        let mut target_renamer = TargetRenamer::new(context, &mut renames, bound_here);
        target_renamer.visit_expr(&mut target_expr);

        let mut ifs = Vec::with_capacity(gen.ifs.len());
        for if_expr in gen.ifs {
            ifs.push(rename_loads(if_expr, &renames));
        }

        lowered_gens.push(LoweredGenerator {
            target: target_expr,
            iter: iter_expr,
            ifs,
            is_async: gen.is_async,
        });
    }

    let mut body = match kind {
        InlineCompKind::Dict => {
            let key_expr = rename_loads(elt_or_key, &renames);
            let value_expr =
                rename_loads(value.expect("dict comprehension expects value"), &renames);
            let key_name = context.fresh("dictcomp_key");
            let value_name = context.fresh("dictcomp_value");
            vec![
                py_stmt!(
                    "{name:id} = {value:expr}",
                    name = key_name.as_str(),
                    value = key_expr,
                ),
                py_stmt!(
                    "{name:id} = {value:expr}",
                    name = value_name.as_str(),
                    value = value_expr,
                ),
                py_stmt!(
                    "{result:id}[{key:id}] = {value:id}",
                    result = result_name.as_str(),
                    key = key_name.as_str(),
                    value = value_name.as_str(),
                ),
            ]
        }
        InlineCompKind::List => {
            let elt_expr = rename_loads(elt_or_key, &renames);
            vec![py_stmt!(
                "{result:id}.append({value:expr})",
                result = result_name.as_str(),
                value = elt_expr,
            )]
        }
        InlineCompKind::Set => {
            let elt_expr = rename_loads(elt_or_key, &renames);
            vec![py_stmt!(
                "{result:id}.add({value:expr})",
                result = result_name.as_str(),
                value = elt_expr,
            )]
        }
    };

    let iter_param = context.fresh("iter");
    let func_name = context.fresh(match kind {
        InlineCompKind::List => "listcomp",
        InlineCompKind::Set => "setcomp",
        InlineCompKind::Dict => "dictcomp",
    });
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

    let iter_param_expr = py_expr!("{param:id}", param = iter_param.as_str());
    let gen_count = lowered_gens.len();
    for (rev_index, gen) in lowered_gens.into_iter().rev().enumerate() {
        let is_outermost = rev_index + 1 == gen_count;
        body = wrap_ifs(body, gen.ifs);
        let iter_expr = if is_outermost {
            iter_param_expr.clone()
        } else {
            gen.iter
        };
        let for_stmt = if gen.is_async {
            py_stmt!(
                r#"
async for {target:expr} in {iter:expr}:
    {body:stmt}
"#,
                target = gen.target,
                iter = iter_expr,
                body = body,
            )
        } else {
            py_stmt!(
                r#"
for {target:expr} in {iter:expr}:
    {body:stmt}
"#,
                target = gen.target,
                iter = iter_expr,
                body = body,
            )
        };
        if is_outermost {
            body = vec![for_stmt];
        } else {
            body = vec![for_stmt];
        }
    }

    let mut func_body: Vec<Stmt> = Vec::new();
    if !global_targets.is_empty() {
        let mut names = global_targets.into_iter().collect::<Vec<_>>();
        names.sort();
        let names = names
            .into_iter()
            .map(|name| ast::Identifier::new(name, TextRange::default()))
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
            .map(|name| ast::name::Name::new(name))
            .map(|name| ast::Identifier::new(name, TextRange::default()))
            .collect();
        func_body.push(Stmt::Nonlocal(ast::StmtNonlocal {
            names,
            range: TextRange::default(),
            node_index: ast::AtomicNodeIndex::default(),
        }));
    }
    func_body.push(init_stmt);
    func_body.extend(body);
    func_body.push(py_stmt!("return {result:expr}", result = result_expr));

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
        iter = iter_call,
    );
    let expr = if is_async {
        py_expr!("await {call:expr}", call = call_expr)
    } else {
        call_expr
    };

    LoweredExpr::modified(expr, prefix)
}

fn collect_named_expr_targets(
    elt_or_key: &Expr,
    value: Option<&Expr>,
    generators: &[ast::Comprehension],
) -> HashSet<String> {
    let mut collector = super::NamedExprTargetCollector::default();
    let mut elt_clone = elt_or_key.clone();
    collector.visit_expr(&mut elt_clone);
    if let Some(value) = value {
        let mut value_clone = value.clone();
        collector.visit_expr(&mut value_clone);
    }
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
    let mut cloned = expr.clone();
    finder.visit_expr(&mut cloned);
    finder.found
}
