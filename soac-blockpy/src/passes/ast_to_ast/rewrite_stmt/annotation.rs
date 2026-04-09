use ruff_python_ast::{self as ast, Expr, Parameter, Stmt};
use ruff_python_codegen::{Generator, Indentation};
use ruff_source_file::LineEnding;

use crate::passes::ast_to_ast::body::Suite;
use crate::transformer::{walk_stmt, Transformer};
use crate::{
    passes::ast_to_ast::{context::Context, expr_utils::make_tuple},
    py_expr, py_stmt,
};

pub(crate) const FUNCTION_ANNOTATE_PREFIX: &str = "_dp_annotate_func_";

pub fn rewrite_ann_assign_to_dunder_annotate(_context: &Context, stmt: &mut Suite) {
    // Assume called with module body stmt, which gets __annotate__.
    let entries = AnnotationStripper::strip(stmt);
    if entries.is_empty() {
        return;
    }
    let ds = build_annotate_fn(entries, "__annotate__");
    stmt.push(ds);
}

#[derive(Default)]
struct AnnotationStripper {
    entries: Vec<(String, Expr, String)>,
    indent: Indentation,
}

impl AnnotationStripper {
    fn strip(stmt: &mut Suite) -> Vec<(String, Expr, String)> {
        let mut collector = AnnotationStripper {
            entries: Vec::new(),
            indent: Indentation::new("    ".to_string()),
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
        Some(build_annotate_fn(
            entries,
            &format!("{}{}", FUNCTION_ANNOTATE_PREFIX, func_def.name.id),
        ))
    }
}

impl Transformer for AnnotationStripper {
    fn visit_body(&mut self, body: &mut Suite) {
        let mut rewritten = Vec::with_capacity(body.len());
        for mut stmt in std::mem::take(body) {
            if let Stmt::FunctionDef(func_def) = &mut stmt {
                let helper = self.function_annotation_helper(func_def);
                AnnotationStripper::strip(&mut func_def.body);
                if let Some(helper) = helper {
                    rewritten.push(helper);
                }
                rewritten.push(stmt);
                continue;
            }
            self.visit_stmt(&mut stmt);
            rewritten.push(stmt);
        }
        *body = rewritten;
    }

    fn visit_stmt(&mut self, stmt: &mut Stmt) {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                AnnotationStripper::strip(&mut func_def.body);
                // drop the collected annotations
            }
            Stmt::ClassDef(class_def) => {
                let entries = AnnotationStripper::strip(&mut class_def.body);
                if !entries.is_empty() {
                    // CPython stores class annotation thunks under __annotate_func__,
                    // and exposes __annotate__ via type-level descriptor logic.
                    let ds = build_annotate_fn(entries, "__annotate_func__");
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
    // runtime helpers/builtins used by this thunk.
    // Format values in Python 3.15's annotationlib are:
    // VALUE=1, VALUE_WITH_FAKE_GLOBALS=2, FORWARDREF=3, STRING=4.
    // We handle STRING and FORWARDREF directly so annotationlib does not need to clone
    // this helper's placeholder __code__ into a bytecode function.
    py_stmt!(
        r#"
def {annotate_name:id}(
    _dp_format,
    __soac__=__import__("soac.runtime", globals(), dict(), ("runtime",), 0),
):
    if __soac__.eq(_dp_format, 4):
        return {string_dict:expr}
    if __soac__.eq(_dp_format, 3):
        return {forwardref_dict:expr}
    if __soac__.gt(_dp_format, 2):
        raise __soac__.builtins.NotImplementedError
    return {value_dict:expr}
"#,
        annotate_name = name,
        forwardref_dict = forwardref_dict,
        string_dict = string_dict,
        value_dict = value_dict,
    )
}
