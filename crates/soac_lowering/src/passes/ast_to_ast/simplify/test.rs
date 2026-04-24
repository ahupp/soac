use crate::passes::ast_to_ast::string_templates::lower_string_templates_in_expr;
use crate::passes::ast_to_ast::util::is_dp_helper_lookup_expr;
use crate::transformer::{walk_expr, Transformer};
use ruff_python_ast::{self as ast, Stmt};
use ruff_python_parser::parse_module;

fn parse_assign_module(source: &str) -> ast::ModModule {
    parse_module(source).unwrap().into_syntax()
}

#[derive(Default)]
struct ExprShapeProbe {
    has_value_eq_literal: bool,
    has_value_expr_text: bool,
    has_literal_string_value: bool,
    has_repr_call: bool,
    has_str_call: bool,
    has_template_interpolation_call: bool,
}

impl Transformer for ExprShapeProbe {
    fn visit_expr(&mut self, expr: &mut ast::Expr) {
        match expr {
            ast::Expr::StringLiteral(literal) => {
                self.has_value_eq_literal |= literal.value.to_str() == "value=";
                self.has_value_expr_text |= literal.value.to_str() == "value";
                self.has_literal_string_value |= literal.value.to_str() == "literal";
            }
            ast::Expr::Call(call) => {
                self.has_repr_call |= is_dp_helper_lookup_expr(call.func.as_ref(), "repr");
                self.has_str_call |= is_dp_helper_lookup_expr(call.func.as_ref(), "str");
                self.has_template_interpolation_call |=
                    is_dp_helper_lookup_expr(call.func.as_ref(), "templatelib_Interpolation");
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

fn probe_assignment_value(module: &ast::ModModule) -> ExprShapeProbe {
    let Stmt::Assign(assign) = &module.body[0] else {
        panic!("expected first statement to be an assignment");
    };
    let mut value = assign.value.as_ref().clone();
    let mut probe = ExprShapeProbe::default();
    probe.visit_expr(&mut value);
    probe
}

#[test]
fn lower_string_templates_keeps_fstring_debug_output_correct() {
    let mut module = parse_assign_module("x = f\"{value=}\"\n");
    let Stmt::Assign(assign) = &mut module.body[0] else {
        panic!("expected first statement to be an assignment");
    };
    lower_string_templates_in_expr(assign.value.as_mut());

    let probe = probe_assignment_value(&module);
    assert!(probe.has_value_eq_literal);
    assert!(probe.has_repr_call);
}

#[test]
fn lower_string_templates_keeps_literal_str_conversion_as_literal() {
    let mut module = parse_assign_module("x = f\"{'literal'!s}\"\n");
    let Stmt::Assign(assign) = &mut module.body[0] else {
        panic!("expected first statement to be an assignment");
    };
    lower_string_templates_in_expr(assign.value.as_mut());

    let probe = probe_assignment_value(&module);
    assert!(probe.has_literal_string_value);
    assert!(!probe.has_str_call);
}

#[test]
fn lower_string_templates_keeps_tstring_expr_text_available() {
    let mut module = parse_assign_module("x = t\"{value}\"\n");
    let Stmt::Assign(assign) = &mut module.body[0] else {
        panic!("expected first statement to be an assignment");
    };
    lower_string_templates_in_expr(assign.value.as_mut());

    let probe = probe_assignment_value(&module);
    assert!(probe.has_template_interpolation_call);
    assert!(probe.has_value_expr_text);
}
