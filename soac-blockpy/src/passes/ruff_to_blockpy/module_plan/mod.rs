use crate::block_py::param_specs::{collect_param_spec_and_defaults, param_defaults_to_expr};
use crate::block_py::{
    BindingKind, BlockPyFunction, BlockPyModule, ModuleShape, CallableScopeInfo, CellBindingKind,
    FunctionKind, FunctionNameGen, ModuleNameGen,
};
use crate::passes::ast_to_ast::body::{split_docstring, Suite};
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::rewrite_stmt;
use crate::passes::ast_to_ast::semantic::{SemanticAstState, SemanticScope};
use crate::passes::{CoreModuleShapeWithAwaitAndYield, InstrRuff};
use crate::transformer::{walk_expr, walk_stmt, Transformer};
use crate::{py_expr, py_stmt, py_stmt_typed};
use ruff_python_ast::{self as ast, Expr, Stmt};

use super::build_core_blockpy_callable_def_from_runtime_input;
mod callable_scope;
use callable_scope::callable_scope_info;

struct FunctionScopeFrame {
    scope: Option<SemanticScope>,
    callable_scope: CallableScopeInfo,
    hoisted_to_parent: Vec<Stmt>,
}

struct BlockPyModuleRewriter<'a, P: ModuleShape> {
    context: &'a Context,
    semantic_state: SemanticAstState,
    module_name_gen: ModuleNameGen,
    function_scope_stack: Vec<FunctionScopeFrame>,
    callable_defs: Vec<BlockPyFunction<P>>,
    lower_function_to_blockpy: fn(
        &Context,
        &ast::StmtFunctionDef,
        &CallableScopeInfo,
        FunctionNameGen,
    ) -> BlockPyFunction<P>,
}

#[derive(Default)]
struct YieldFamilyDetector {
    found: bool,
}

pub(crate) fn rewrite_ast_to_core_blockpy_module_plan_with_module(
    context: &Context,
    mut module: Suite,
    semantic_state: &SemanticAstState,
    module_name_gen: ModuleNameGen,
) -> BlockPyModule<CoreModuleShapeWithAwaitAndYield> {
    crate::passes::ast_to_ast::simplify::flatten(&mut module);
    let mut rewriter = BlockPyModuleRewriter {
        context,
        semantic_state: semantic_state.clone(),
        module_name_gen,
        function_scope_stack: Vec::new(),
        callable_defs: Vec::new(),
        lower_function_to_blockpy: try_lower_function_to_core_blockpy_bundle,
    };
    let module_init =
        BlockPyModuleRewriter::<CoreModuleShapeWithAwaitAndYield>::root_module_init_stmt(
            &mut module,
        );
    rewriter.lower_root_function_def(module_init);
    BlockPyModule {
        module_name_gen: rewriter.module_name_gen,
        global_names: Vec::new(),
        callable_defs: rewriter.callable_defs,
        module_constants: Vec::new(),
        counter_defs: Vec::new(),
    }
}

impl Transformer for YieldFamilyDetector {
    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            other => walk_stmt(self, other),
        }
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Yield(_) | Expr::YieldFrom(_) => {
                self.found = true;
            }
            Expr::Lambda(_)
            | Expr::Generator(_)
            | Expr::ListComp(_)
            | Expr::SetComp(_)
            | Expr::DictComp(_) => {}
            other => walk_expr(self, other),
        }
    }
}

fn function_kind(func: &ast::StmtFunctionDef) -> FunctionKind {
    let mut detector = YieldFamilyDetector::default();
    let mut body = func.body.to_vec();
    detector.visit_body(&mut body);
    match (func.is_async, detector.found) {
        (false, false) => FunctionKind::Function,
        (false, true) => FunctionKind::Generator,
        (true, false) => FunctionKind::Coroutine,
        (true, true) => FunctionKind::AsyncGenerator,
    }
}

fn try_lower_function_to_core_blockpy_bundle(
    context: &Context,
    func: &ast::StmtFunctionDef,
    callable_scope: &CallableScopeInfo,
    name_gen: FunctionNameGen,
) -> BlockPyFunction<CoreModuleShapeWithAwaitAndYield> {
    let (docstring, lowered_input_body) = split_docstring(&func.body);
    let lowered_input_body = lowered_input_body
        .iter()
        .cloned()
        .map(InstrRuff::from_ast_stmt)
        .collect::<Vec<_>>();
    let (param_spec, _param_defaults) = collect_param_spec_and_defaults(&func.parameters);

    let end_label = name_gen.next_block_name();

    build_core_blockpy_callable_def_from_runtime_input(
        context,
        name_gen,
        callable_scope.names.clone(),
        param_spec,
        &lowered_input_body,
        docstring,
        end_label,
        function_kind(func),
        callable_scope,
    )
}

fn build_lowered_function_instantiation_expr(
    function_id: crate::block_py::FunctionId,
    decorator_exprs: Vec<Expr>,
    param_defaults: &[Expr],
    annotate_fn_expr: Expr,
    kind: FunctionKind,
) -> Expr {
    let param_defaults_expr = param_defaults_to_expr(param_defaults);
    let kind_name = match kind {
        FunctionKind::Function => "function",
        FunctionKind::Coroutine => "coroutine",
        FunctionKind::Generator => "generator",
        FunctionKind::AsyncGenerator => "async_generator",
    };
    let base_function_expr = py_expr!(
        "__soac__.make_function({function_id:literal}, {kind:literal}, {closure:expr}, {param_defaults:expr}, {annotate_fn:expr})",
        function_id = function_id.packed(),
        kind = kind_name,
        closure = py_expr!("__soac__.tuple_values()"),
        param_defaults = param_defaults_expr.clone(),
        annotate_fn = annotate_fn_expr.clone(),
    );
    rewrite_stmt::decorator::rewrite_exprs(decorator_exprs, base_function_expr)
}

#[allow(clippy::too_many_arguments)]
fn rewrite_function_def_stmt_via_blockpy_with_pass<P: ModuleShape>(
    context: &Context,
    parent_hoisted: &mut Vec<Stmt>,
    func: &mut ast::StmtFunctionDef,
    callable_scope: &CallableScopeInfo,
    function_hoisted: Vec<Stmt>,
    module_name_gen: &mut ModuleNameGen,
    callable_defs: &mut Vec<BlockPyFunction<P>>,
    lower_function_to_blockpy: fn(
        &Context,
        &ast::StmtFunctionDef,
        &CallableScopeInfo,
        FunctionNameGen,
    ) -> BlockPyFunction<P>,
) -> Vec<Stmt> {
    let name_gen = module_name_gen.next_function_name_gen();
    let lowered_plan = lower_function_to_blockpy(context, func, callable_scope, name_gen);
    let bind_name = lowered_plan.names.bind_name.clone();
    let (_, param_defaults) = collect_param_spec_and_defaults(&func.parameters);
    let decorated = build_lowered_function_instantiation_expr(
        lowered_plan.function_id,
        rewrite_stmt::decorator::collect_exprs(&func.decorator_list),
        &param_defaults,
        py_expr!("None"),
        lowered_plan.kind,
    );
    let binding_stmt = vec![py_stmt!(
        "{name:id} = {value:expr}",
        name = bind_name.as_str(),
        value = decorated
    )];
    callable_defs.push(lowered_plan);
    if bind_name.starts_with("_dp_class_ns_") || bind_name.starts_with("_dp_define_class_") {
        let mut replacement = function_hoisted;
        replacement.extend(binding_stmt);
        replacement
    } else {
        parent_hoisted.extend(function_hoisted);
        binding_stmt
    }
}

impl<P: ModuleShape> BlockPyModuleRewriter<'_, P> {
    fn lower_lambda_expr(&mut self, lambda: &mut ast::ExprLambda) -> Expr {
        let lambda_scope = self
            .semantic_state
            .lambda_scope(lambda)
            .expect("missing preserved lambda scope while lowering lambda");
        let func_name = self.context.fresh("lambda");
        let mut func_def: ast::StmtFunctionDef = py_stmt_typed!(
            r#"
def {func:id}():
    pass
"#,
            func = func_name.as_str(),
        );
        if let Some(parameters) = lambda.parameters.take() {
            func_def.parameters = parameters;
        }
        let body = std::mem::replace(&mut *lambda.body, py_expr!("None"));
        func_def.body = vec![py_stmt!("return {value:expr}", value = body)];

        let state = self.walk_function_def_with_explicit_scope(&mut func_def, Some(lambda_scope));
        if let Some(parent_frame) = self.function_scope_stack.last_mut() {
            for (name, binding) in &state.callable_scope.bindings {
                if matches!(binding, BindingKind::Cell(CellBindingKind::Capture))
                    && parent_frame.callable_scope.local_defs.contains(name)
                {
                    parent_frame.callable_scope.insert_binding(
                        name.clone(),
                        BindingKind::Cell(CellBindingKind::Owner),
                        false,
                        None,
                    );
                }
            }
        }

        let lowered_plan = (self.lower_function_to_blockpy)(
            self.context,
            &func_def,
            &state.callable_scope,
            self.module_name_gen.next_function_name_gen(),
        );
        let (_, param_defaults) = collect_param_spec_and_defaults(&func_def.parameters);
        let lowered_expr = build_lowered_function_instantiation_expr(
            lowered_plan.function_id,
            Vec::new(),
            &param_defaults,
            py_expr!("None"),
            lowered_plan.kind,
        );
        self.callable_defs.push(lowered_plan);
        lowered_expr
    }

    fn root_module_init_stmt<'a>(module: &'a mut Suite) -> &'a mut ast::StmtFunctionDef {
        assert_eq!(
            module.len(),
            1,
            "expected root suite with exactly one statement",
        );
        let Stmt::FunctionDef(func) = &mut module[0] else {
            panic!("expected root suite with exactly one function");
        };
        assert!(
            func.parameters.posonlyargs.is_empty()
                && func.parameters.args.is_empty()
                && func.parameters.vararg.is_none()
                && func.parameters.kwonlyargs.is_empty()
                && func.parameters.kwarg.is_none(),
            "expected root function with no parameters",
        );
        func
    }

    fn walk_function_def_with_scope(
        &mut self,
        func: &mut ast::StmtFunctionDef,
    ) -> FunctionScopeFrame {
        let function_scope = self.semantic_state.function_scope(func);
        self.walk_function_def_with_explicit_scope(func, function_scope)
    }

    fn walk_function_def_with_explicit_scope(
        &mut self,
        func: &mut ast::StmtFunctionDef,
        function_scope: Option<SemanticScope>,
    ) -> FunctionScopeFrame {
        let parent_scope = self
            .function_scope_stack
            .last()
            .and_then(|frame| frame.scope.as_ref())
            .cloned();
        let callable_scope = callable_scope_info(
            &self.semantic_state,
            parent_scope.as_ref(),
            function_scope.as_ref(),
            Some(func),
            &func.body,
        );
        self.function_scope_stack.push(FunctionScopeFrame {
            scope: function_scope.clone(),
            callable_scope,
            hoisted_to_parent: Vec::new(),
        });
        self.visit_body(&mut func.body);
        self.function_scope_stack
            .pop()
            .expect("function scope stack should pop after walking function def")
    }

    fn lower_root_function_def(&mut self, func: &mut ast::StmtFunctionDef) {
        let state = self.walk_function_def_with_scope(func);
        assert!(
            state.hoisted_to_parent.is_empty(),
            "root _dp_module_init should not produce hoisted statements"
        );
        let lowered_plan = (self.lower_function_to_blockpy)(
            self.context,
            func,
            &state.callable_scope,
            self.module_name_gen.next_function_name_gen(),
        );
        self.callable_defs.push(lowered_plan);
    }

    fn rewrite_visited_function_def(
        &mut self,
        func: &mut ast::StmtFunctionDef,
        state: FunctionScopeFrame,
    ) -> Vec<Stmt> {
        let parent_frame = self
            .function_scope_stack
            .last_mut()
            .expect("nested function rewrite should always have a parent hoist buffer");
        let parent_hoisted = &mut parent_frame.hoisted_to_parent;
        rewrite_function_def_stmt_via_blockpy_with_pass(
            self.context,
            parent_hoisted,
            func,
            &state.callable_scope,
            state.hoisted_to_parent,
            &mut self.module_name_gen,
            &mut self.callable_defs,
            self.lower_function_to_blockpy,
        )
    }
}

impl<P: ModuleShape> Transformer for BlockPyModuleRewriter<'_, P> {
    fn visit_body(&mut self, body: &mut Suite) {
        let mut rewritten = Vec::with_capacity(body.len());
        for stmt in std::mem::take(body) {
            let mut stmt = stmt;
            if let Stmt::FunctionDef(func) = &mut stmt {
                let state = self.walk_function_def_with_scope(func);
                let replacement = self.rewrite_visited_function_def(func, state);
                rewritten.extend(replacement);
                continue;
            }

            self.visit_stmt(&mut stmt);
            rewritten.push(stmt);
        }
        *body = rewritten;
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &mut Expr) {
        match expr {
            Expr::Lambda(lambda) => {
                *expr = self.lower_lambda_expr(lambda);
            }
            other => walk_expr(self, other),
        }
    }
}

#[cfg(test)]
mod test;
