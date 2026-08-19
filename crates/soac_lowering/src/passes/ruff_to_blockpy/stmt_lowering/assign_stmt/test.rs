use super::super::{lower_instr_for_test, BlockPyStmtBuilder};
use super::*;
use crate::block_py::{
    CallArgPositional, InstrResolved, InstrWithAwaitAndYield, Literal, NameLike,
    NumberLiteralValue, RuntimeName,
};
use crate::pass_tracker::LoweringPassTrackerInternalExt;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;
use ruff_python_ast::{Expr, Stmt};

fn is_soac_attr_call(expr: &Expr, expected_attr: &str) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    matches!(
        call.func.as_ref(),
        Expr::Attribute(attr)
            if matches!(attr.value.as_ref(), Expr::Name(name) if name.id.as_str() == "__soac__")
                && attr.attr.id.as_str() == expected_attr
    )
}

#[test]
fn stmt_assign_to_blockpy_emits_direct_core_setitem() {
    let stmt = py_stmt!("obj[idx] = value");
    let assign_stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &assign_stmt, &name_gen, &mut out, None)
        .expect("assign lowering should succeed");

    let fragment = out.finish();
    assert!(fragment
        .entry
        .body
        .iter()
        .any(|instr| matches!(instr, InstrWithAwaitAndYield::SetItem(_))));
}

#[test]
fn fixed_assignment_unpack_uses_compiler_owned_arity_operation() {
    for source in ["first, second = values", "[first, second] = values"] {
        let stmt = ruff_python_parser::parse_module(source)
            .expect("assignment should parse")
            .into_syntax()
            .body
            .into_iter()
            .next()
            .expect("expected assignment statement");
        let assign_stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt);
        let context = Context::new(source);
        let name_gen = test_name_gen();
        let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
        lower_instr_for_test(&context, &assign_stmt, &name_gen, &mut out, None)
            .expect("assignment lowering should succeed");

        let fragment = out.finish();
        let call = fragment
            .entry
            .body
            .iter()
            .find_map(|instr| match instr {
                InstrWithAwaitAndYield::Store(store) => match store.value.as_ref() {
                    InstrWithAwaitAndYield::Call(call) => Some(call),
                    _ => None,
                },
                _ => None,
            })
            .expect("fixed assignment should materialize one unpack operation");
        assert!(
            matches!(call.func.as_ref(), InstrWithAwaitAndYield::Load(load)
                if load.name.id_str() == "unpack_fixed"),
            "fixed assignment should use the compiler-owned arity operation: {call:?}"
        );
        assert!(
            matches!(
                call.args.as_slice(),
                [
                    CallArgPositional::Positional(_),
                    CallArgPositional::Positional(InstrWithAwaitAndYield::Literal(literal))
                ] if matches!(
                    &literal.literal,
                    Literal::NumberLiteral(number)
                        if matches!(&number.value, NumberLiteralValue::Int(value)
                            if value.as_i64() == Some(2))
                )
            ),
            "fixed assignment should pass its literal target arity: {call:?}"
        );
    }
}

#[test]
fn runtime_bootstrap_keeps_fixed_unpack_intrinsic_out_of_module_constants() {
    let source = "def pair(values):\n    first, second = values\n    return first, second\n";
    let result = crate::lower_python_to_blockpy_for_testing(source)
        .expect("fixed-unpack source should lower for runtime bootstrap");
    let core_module = result
        .pass_tracker
        .pass_core_blockpy()
        .expect("core BlockPy pass should be available")
        .clone();
    let module =
        crate::passes::name_binding::lower_name_binding_in_core_blockpy_module_with_options(
            core_module,
            true,
        );

    assert!(
        module.module_constants.iter().all(|constant| !matches!(
            constant,
            InstrResolved::Load(load)
                if load.name.runtime_name_id() == Some(RuntimeName::UnpackFixed)
        )),
        "bootstrap must not materialize fixed unpack as a runtime Python object"
    );

    let function = module
        .callable_defs
        .iter()
        .find(|function| function.names.fn_name == "pair")
        .expect("expected pair callable");
    assert!(
        function
            .blocks
            .iter()
            .flat_map(|block| &block.body)
            .any(|instr| {
                matches!(
                    instr,
                    InstrResolved::Store(store)
                        if matches!(
                            store.value.as_ref(),
                            InstrResolved::Call(call)
                                if matches!(
                                    call.func.as_ref(),
                                    InstrResolved::Load(load)
                                        if load.name.runtime_name_id()
                                            == Some(RuntimeName::UnpackFixed)
                                )
                        )
                )
            }),
        "runtime bootstrap must preserve compiler-owned fixed-unpack provenance"
    );
}

#[test]
fn rewrite_with_assignment_target_uses_fixed_unpack_arity() {
    let target = py_expr!("first, second");
    let rhs = py_expr!("value");
    let mut out = Vec::new();
    let mut next_temp_id = 0usize;
    let mut next_temp = |prefix: &str| {
        let name = format!("_dp_{prefix}_{next_temp_id}");
        next_temp_id += 1;
        name
    };

    rewrite_assignment_target(target, rhs, &mut out, &mut next_temp);

    assert!(out.iter().any(|stmt| matches!(
        stmt,
        Stmt::Assign(assign)
            if is_soac_attr_call(assign.value.as_ref(), "unpack_fixed")
    )));
}

#[test]
fn rewrite_assignment_target_unpack_uses_native_subscript_ast() {
    let target = py_expr!("a, *b");
    let rhs = py_expr!("value");
    let mut out = Vec::new();
    let mut next_temp_id = 0usize;
    let mut next_temp = |prefix: &str| {
        let name = format!("_dp_{prefix}_{next_temp_id}");
        next_temp_id += 1;
        name
    };

    rewrite_assignment_target(target, rhs, &mut out, &mut next_temp);

    assert!(out.iter().any(|stmt| matches!(
        stmt,
        Stmt::Assign(assign) if is_soac_attr_call(assign.value.as_ref(), "unpack")
    )));
    assert!(out.iter().any(|stmt| matches!(
        stmt,
        Stmt::Assign(assign)
            if matches!(assign.targets.as_slice(), [Expr::Name(name)] if name.id.as_str() == "a")
                && matches!(assign.value.as_ref(), Expr::Subscript(_))
    )));
    assert!(out.iter().any(|stmt| matches!(
        stmt,
        Stmt::Assign(assign)
            if matches!(assign.targets.as_slice(), [Expr::Name(name)] if name.id.as_str() == "b")
                && is_soac_attr_call(assign.value.as_ref(), "list")
    )));
}

#[test]
fn rewrite_assignment_target_uses_native_store_targets() {
    let cases = ["obj[idx]", "obj.attr"];

    for target_src in cases {
        let target = *ruff_python_parser::parse_expression(target_src)
            .unwrap()
            .into_syntax()
            .body;
        let rhs = py_expr!("value");
        let mut out = Vec::new();
        let mut next_temp_id = 0usize;
        let mut next_temp = |prefix: &str| {
            let name = format!("_dp_{prefix}_{next_temp_id}");
            next_temp_id += 1;
            name
        };

        rewrite_assignment_target(target, rhs, &mut out, &mut next_temp);

        let [Stmt::Assign(assign)] = out.as_slice() else {
            panic!("expected one assignment for {target_src}, got {out:?}");
        };
        assert!(
            matches!(
                (target_src, assign.targets.as_slice(), assign.value.as_ref()),
                ("obj[idx]", [Expr::Subscript(_)], Expr::Name(name)) if name.id.as_str() == "value"
            ) || matches!(
                (target_src, assign.targets.as_slice(), assign.value.as_ref()),
                ("obj.attr", [Expr::Attribute(_)], Expr::Name(name)) if name.id.as_str() == "value"
            ),
            "unexpected assignment rewrite for {target_src}: {assign:?}",
        );
    }
}
