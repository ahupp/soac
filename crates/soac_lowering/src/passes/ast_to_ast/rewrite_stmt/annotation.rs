use ruff_python_ast::{
    self as ast, Expr, Parameter, Stmt, TypeParam, TypeParamParamSpec, TypeParamTypeVar,
    TypeParamTypeVarTuple,
};
use ruff_python_codegen::{Generator, Indentation};
use ruff_source_file::LineEnding;

use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::{context::Context, expr_utils::make_tuple};
use crate::template::{py_expr, py_stmt};
use crate::transformer::{walk_stmt, Transformer};

pub(crate) const FUNCTION_ANNOTATE_PREFIX: &str = "_dp_annotate_func_";

mod strict;

/// CPython emits finally suites more than once (including inline unwinds), so
/// one source annotation can have several distinct conditional indices. Until
/// the native compiler exports that occurrence plan, reject only annotations
/// affected by that duplication instead of pretending source order is enough.
pub(crate) fn validate_strict_annotation_shapes(body: &Suite) -> anyhow::Result<()> {
    struct Validator {
        visible_annotations: bool,
        finally_depth: usize,
        unsupported: Option<ruff_text_size::TextRange>,
    }
    impl Validator {
        fn nested_scope(&mut self, body: &mut Suite, visible_annotations: bool) {
            let mut nested = Self {
                visible_annotations,
                finally_depth: 0,
                unsupported: None,
            };
            nested.visit_body(body);
            self.unsupported = self.unsupported.or(nested.unsupported);
        }
    }
    impl Transformer for Validator {
        fn visit_stmt(&mut self, statement: &mut Stmt) {
            match statement {
                Stmt::FunctionDef(function) => self.nested_scope(&mut function.body, false),
                Stmt::ClassDef(class) => self.nested_scope(&mut class.body, true),
                Stmt::Try(statement) => {
                    self.visit_body(&mut statement.body);
                    self.visit_body(&mut statement.orelse);
                    for handler in &mut statement.handlers {
                        self.visit_except_handler(handler);
                    }
                    self.finally_depth += 1;
                    self.visit_body(&mut statement.finalbody);
                    self.finally_depth -= 1;
                }
                Stmt::AnnAssign(annotation)
                    if self.visible_annotations
                        && self.finally_depth != 0
                        && annotation.simple
                        && matches!(annotation.target.as_ref(), Expr::Name(_)) =>
                {
                    self.unsupported.get_or_insert(annotation.range);
                }
                _ => walk_stmt(self, statement),
            }
        }
    }
    let mut validator = Validator {
        visible_annotations: true,
        finally_depth: 0,
        unsupported: None,
    };
    validator.visit_body(&mut body.clone());
    if let Some(range) = validator.unsupported {
        anyhow::bail!("strict annotation replay does not yet support a module or class annotation in a finally suite at {}..{}; native occurrence-index provenance is required", range.start().to_u32(), range.end().to_u32());
    }
    Ok(())
}

pub(crate) fn rewrite_ann_assign_to_dunder_annotate(context: &Context, stmt: &mut Suite) {
    if context.strict_source().is_some() {
        strict::rewrite_module(context, stmt);
        return;
    }
    // Assume called with module body stmt, which gets __annotate__.
    let entries = AnnotationStripper::strip(context, stmt);
    if entries.is_empty() {
        return;
    }
    let mut ds = build_annotate_fn(entries, "__annotate__");
    let Stmt::FunctionDef(helper) = &mut ds else {
        unreachable!()
    };
    context.record_module_annotation_helper(helper);
    stmt.push(ds);
}

struct AnnotationStripper<'a> {
    context: &'a Context,
    entries: Vec<(String, Expr, String)>,
    indent: Indentation,
    capture_names: Vec<String>,
}

impl<'a> AnnotationStripper<'a> {
    fn strip(context: &'a Context, stmt: &mut Suite) -> Vec<(String, Expr, String)> {
        Self::strip_with_captures(context, stmt, Vec::new())
    }

    fn strip_with_captures(
        context: &'a Context,
        stmt: &mut Suite,
        capture_names: Vec<String>,
    ) -> Vec<(String, Expr, String)> {
        let mut collector = AnnotationStripper {
            context,
            entries: Vec::new(),
            indent: Indentation::new("    ".to_string()),
            capture_names,
        };
        collector.visit_body(stmt);
        collector.entries
    }

    fn annotation_string(&self, expr: &Expr) -> String {
        Generator::new(&self.indent, LineEnding::default()).expr(expr)
    }

    fn parameter_annotation_entry(&self, parameter: &Parameter) -> Option<(String, Expr, String)> {
        let annotation = parameter.annotation.as_ref()?;
        let annotation = annotation.as_ref();
        Some((
            parameter.name.id.to_string(),
            annotation.clone(),
            self.annotation_string(annotation),
        ))
    }

    fn function_signature_entries(
        &self,
        func_def: &ast::StmtFunctionDef,
    ) -> Vec<(String, Expr, String)> {
        let mut entries = Vec::new();
        for parameter in func_def
            .parameters
            .posonlyargs
            .iter()
            .chain(func_def.parameters.args.iter())
        {
            if let Some(entry) = self.parameter_annotation_entry(&parameter.parameter) {
                entries.push(entry);
            }
        }
        if let Some(parameter) = &func_def.parameters.vararg {
            if let Some(entry) = self.parameter_annotation_entry(parameter.as_ref()) {
                entries.push(entry);
            }
        }
        for parameter in &func_def.parameters.kwonlyargs {
            if let Some(entry) = self.parameter_annotation_entry(&parameter.parameter) {
                entries.push(entry);
            }
        }
        if let Some(parameter) = &func_def.parameters.kwarg {
            if let Some(entry) = self.parameter_annotation_entry(parameter.as_ref()) {
                entries.push(entry);
            }
        }
        if let Some(annotation) = &func_def.returns {
            entries.push((
                "return".to_string(),
                annotation.as_ref().clone(),
                self.annotation_string(annotation.as_ref()),
            ));
        }
        entries
    }

    fn function_annotation_helper(&self, func_def: &ast::StmtFunctionDef) -> Option<Stmt> {
        let entries = self.function_signature_entries(func_def);
        if entries.is_empty() {
            return None;
        }
        let mut capture_values = capture_name_values(&self.capture_names);
        capture_values.extend(type_param_capture_values(func_def.type_params.as_deref()));
        let mut result = build_annotate_fn_with_capture_values(
            entries,
            &format!("{}{}", FUNCTION_ANNOTATE_PREFIX, func_def.name.id),
            capture_values,
        );
        let Stmt::FunctionDef(helper) = &mut result else {
            unreachable!()
        };
        self.context
            .record_function_annotation_helper(func_def, helper);
        Some(result)
    }
}

fn type_param_names(type_params: Option<&ast::TypeParams>) -> Vec<String> {
    type_params
        .into_iter()
        .flat_map(|type_params| type_params.type_params.iter())
        .map(|type_param| match type_param {
            TypeParam::TypeVar(param) => param.name.id.to_string(),
            TypeParam::TypeVarTuple(param) => param.name.id.to_string(),
            TypeParam::ParamSpec(param) => param.name.id.to_string(),
        })
        .collect()
}

fn capture_name_values(names: &[String]) -> Vec<(String, Expr)> {
    names
        .iter()
        .map(|name| (name.clone(), py_expr!("{name:id}", name = name.as_str())))
        .collect()
}

fn type_param_capture_values(type_params: Option<&ast::TypeParams>) -> Vec<(String, Expr)> {
    type_params
        .into_iter()
        .flat_map(|type_params| type_params.type_params.iter())
        .map(|type_param| match type_param {
            TypeParam::TypeVar(TypeParamTypeVar {
                name,
                bound,
                default,
                ..
            }) => {
                let param_name = name.as_str().to_string();
                let (constraints, bound_expr) = match bound.as_deref().cloned() {
                    Some(Expr::Tuple(ast::ExprTuple { elts, .. })) => {
                        (Some(make_tuple(elts)), None)
                    }
                    Some(other) => (None, Some(other)),
                    None => (None, None),
                };
                let bound_expr = bound_expr.unwrap_or_else(|| py_expr!("None"));
                let default_expr = default
                    .as_deref()
                    .cloned()
                    .unwrap_or_else(|| py_expr!("None"));
                let constraints_expr = constraints.unwrap_or_else(|| py_expr!("None"));
                (
                    param_name.clone(),
                    py_expr!(
                        "__soac__.typing_TypeVar({name_literal:literal}, {bound:expr}, {default:expr}, {constraints:expr})",
                        name_literal = param_name.as_str(),
                        bound = bound_expr,
                        default = default_expr,
                        constraints = constraints_expr,
                    ),
                )
            }
            TypeParam::TypeVarTuple(TypeParamTypeVarTuple { name, default, .. }) => {
                let param_name = name.as_str().to_string();
                let value = match default.as_deref().cloned() {
                    Some(default_expr) => py_expr!(
                        "__soac__.typing_TypeVarTuple({name_literal:literal}, default={default:expr})",
                        name_literal = param_name.as_str(),
                        default = default_expr,
                    ),
                    None => py_expr!(
                        "__soac__.typing_TypeVarTuple({name_literal:literal})",
                        name_literal = param_name.as_str(),
                    ),
                };
                (param_name, value)
            }
            TypeParam::ParamSpec(TypeParamParamSpec { name, default, .. }) => {
                let param_name = name.as_str().to_string();
                let value = match default.as_deref().cloned() {
                    Some(default_expr) => py_expr!(
                        "__soac__.typing_ParamSpec({name_literal:literal}, default={default:expr})",
                        name_literal = param_name.as_str(),
                        default = default_expr,
                    ),
                    None => py_expr!(
                        "__soac__.typing_ParamSpec({name_literal:literal})",
                        name_literal = param_name.as_str(),
                    ),
                };
                (param_name, value)
            }
        })
        .collect()
}

impl Transformer for AnnotationStripper<'_> {
    fn visit_body(&mut self, body: &mut Suite) {
        let mut rewritten = Vec::with_capacity(body.len());
        for mut stmt in std::mem::take(body) {
            if let Stmt::FunctionDef(func_def) = &mut stmt {
                let helper = self.function_annotation_helper(func_def);
                AnnotationStripper::strip(self.context, &mut func_def.body);
                if let Some(helper) = helper {
                    rewritten.push(helper);
                }
                rewritten.push(stmt);
                continue;
            }
            self.visit_stmt(&mut stmt);
            rewritten.push(stmt);
        }
        *body = rewritten.into();
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                AnnotationStripper::strip(self.context, &mut func_def.body);
                // drop the collected annotations
            }
            Stmt::ClassDef(class_def) => {
                let capture_names = type_param_names(class_def.type_params.as_deref());
                let entries = AnnotationStripper::strip_with_captures(
                    self.context,
                    &mut class_def.body,
                    capture_names.clone(),
                );
                if !entries.is_empty() {
                    // CPython stores class annotation thunks under __annotate_func__,
                    // and exposes __annotate__ via type-level descriptor logic.
                    let mut ds = build_annotate_fn_with_capture_values(
                        entries,
                        "__annotate_func__",
                        capture_name_values(&capture_names),
                    );
                    let Stmt::FunctionDef(helper) = &mut ds else {
                        unreachable!()
                    };
                    self.context.record_class_helper_origin(
                        class_def.range,
                        helper,
                        soac_core::block_py::CallableSourceRole::AnnotationProvider,
                    );
                    class_def.body.push(ds);
                }
            }
            Stmt::AnnAssign(ast::StmtAnnAssign {
                target,
                annotation,
                value,
                ..
            }) => {
                if let Expr::Name(ast::ExprName { id, .. }) = target.as_ref() {
                    self.entries.push((
                        id.to_string(),
                        annotation.as_ref().clone(),
                        self.annotation_string(annotation),
                    ));
                } else {
                    // ignore annotations on stuff like "self.x: int = 1"
                }

                if let Some(value) = value.as_mut() {
                    self.visit_expr(value);
                    // TODO: copy range and node_index from the original statement
                    *stmt = py_stmt!(
                        "{target:expr} = {value:expr}",
                        target = target.clone(),
                        value = value.clone()
                    );
                } else {
                    *stmt = py_stmt!("pass");
                }
            }
            _ => {
                walk_stmt(self, stmt);
            }
        }
    }
}

pub(crate) fn build_annotate_fn(entries: Vec<(String, Expr, String)>, name: &str) -> Stmt {
    build_annotate_fn_with_capture_values(entries, name, Vec::new())
}

pub(crate) fn build_annotate_fn_with_capture_values(
    entries: Vec<(String, Expr, String)>,
    name: &str,
    capture_values: Vec<(String, Expr)>,
) -> Stmt {
    let capture_tuple = make_tuple(
        capture_values
            .iter()
            .map(|(_, value)| value.clone())
            .collect(),
    );
    let capture_bindings = capture_values
        .iter()
        .enumerate()
        .map(|(index, (name, _))| {
            py_stmt!(
                "{name:id} = __soac_type_params__[{index:literal}]",
                name = name.as_str(),
                index = index,
            )
        })
        .collect::<Vec<_>>();
    let value_pairs = entries
        .into_iter()
        .map(|(key, value, source)| {
            (
                py_expr!(
                    "({key:literal}, {value:expr})",
                    key = key.as_str(),
                    value = value.clone()
                ),
                py_expr!(
                    "({key:literal}, {value:literal})",
                    key = key.as_str(),
                    value = source.as_str()
                ),
                py_expr!(
                    "({key:literal}, __soac__.annotation_forwardref_value(lambda: {value:expr}, {source:literal}, __name__))",
                    key = key.as_str(),
                    value = value,
                    source = source.as_str()
                ),
            )
        })
        .collect::<Vec<_>>();
    let value_dict = py_expr!(
        "__soac__.dict({items:expr})",
        items = make_tuple(
            value_pairs
                .iter()
                .map(|(value_pair, _, _)| value_pair.clone())
                .collect()
        )
    );
    let string_dict = py_expr!(
        "__soac__.dict({items:expr})",
        items = make_tuple(
            value_pairs
                .iter()
                .map(|(_, string_pair, _)| string_pair.clone())
                .collect()
        )
    );
    let forwardref_dict = py_expr!(
        "__soac__.dict({items:expr})",
        items = make_tuple(
            value_pairs
                .iter()
                .map(|(_, _, forwardref_pair)| forwardref_pair.clone())
                .collect()
        )
    );
    // Capture runtime at definition time so annotationlib fallback cloning cannot replace
    // runtime builtins used by this thunk.
    // Format values in Python 3.15's annotationlib are:
    // VALUE=1, VALUE_WITH_FAKE_GLOBALS=2, FORWARDREF=3, STRING=4.
    // We handle STRING and FORWARDREF directly so annotationlib does not need to clone
    // this helper's placeholder __code__ into a bytecode function.
    py_stmt!(
        r#"
def {annotate_name:id}(
    _dp_format,
    __soac__=__import__("soac.runtime", globals(), dict(), ("runtime",), 0),
    __soac_type_params__={capture_tuple:expr},
):
    {capture_bindings:stmt}
    if _dp_format == 4:
        return {string_dict:expr}
    if _dp_format == 3:
        return {forwardref_dict:expr}
    if _dp_format > 2:
        raise __soac__.builtins.NotImplementedError
    return {value_dict:expr}
"#,
        annotate_name = name,
        capture_tuple = capture_tuple,
        capture_bindings = capture_bindings,
        forwardref_dict = forwardref_dict,
        string_dict = string_dict,
        value_dict = value_dict,
    )
}

#[cfg(test)]
mod strict_shape_tests {
    use super::*;

    fn validate(source: &str) -> anyhow::Result<()> {
        let body = ruff_python_parser::parse_module(source)
            .unwrap()
            .into_syntax()
            .body;
        validate_strict_annotation_shapes(&body)
    }

    #[test]
    fn duplicated_finally_annotation_occurrences_fail_explicitly() {
        assert!(validate("try:\n    pass\nfinally:\n    value: int\n").is_err());
        assert!(validate("class C:\n    try:\n        pass\n    finally:\n        if flag:\n            value: int\n").is_err());
        assert!(validate(
            "try:\n    pass\nexcept* ValueError:\n    pass\nfinally:\n    value: int\n"
        )
        .is_err());
    }

    #[test]
    fn unrelated_finally_suites_and_function_local_annotations_remain_supported() {
        assert!(validate("try:\n    value: int\nexcept ValueError:\n    error: str\nelse:\n    success: bool\nfinally:\n    cleanup()\n").is_ok());
        assert!(
            validate("def f():\n    try:\n        pass\n    finally:\n        value: int\n")
                .is_ok()
        );
        assert!(validate("try:\n    pass\nfinally:\n    target.value: int\n").is_ok());
    }
}
