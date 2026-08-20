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

fn lowered_assignment_function(
    source: &str,
) -> crate::block_py::BlockPyFunction<soac_ir_blockpy::BlockPyModuleShape> {
    crate::lower_python_to_blockpy_for_testing(source)
        .expect("assignment source should lower through the production pipeline")
        .blockpy_module
        .callable_defs
        .into_iter()
        .find(|function| function.names.qualname == "assign")
        .expect("the source assignment function must remain present")
}

fn assignment_normal_path(
    function: &crate::block_py::BlockPyFunction<soac_ir_blockpy::BlockPyModuleShape>,
) -> Vec<&soac_ir_blockpy::InstrBlockPy> {
    let mut visited = std::collections::HashSet::new();
    let mut block = function.entry_block();
    let mut result = Vec::new();
    loop {
        assert!(
            visited.insert(block.label),
            "the assignment source has no loop"
        );
        result.extend(&block.body);
        match &block.term {
            crate::block_py::BlockTerm::Jump(edge) => {
                block = function
                    .blocks
                    .iter()
                    .find(|block| block.label == edge.target)
                    .unwrap();
            }
            crate::block_py::BlockTerm::Return(_) => return result,
            _ => panic!("the source assignment's successful path is straight-line"),
        }
    }
}

#[test]
fn assignment_operand_sole_attribute_moves_replacement_and_captured_receiver() {
    use soac_ir_blockpy::InstrBlockPy;

    let function =
        lowered_assignment_function("def assign(target, make):\n    target().value = make()\n");
    let path = assignment_normal_path(&function);
    let layout = function.storage_layout().as_ref().unwrap();
    let (set_index, set) = path
        .iter()
        .enumerate()
        .find_map(|(index, instr)| match instr {
            InstrBlockPy::SetAttr(op) => Some((index, op)),
            _ => None,
        })
        .expect("the source attribute assignment must remain an explicit operation");
    let InstrBlockPy::TakeOperand(replacement) = set.replacement.as_ref() else {
        panic!("the sole target consumes its staged replacement without a second owner");
    };
    let InstrBlockPy::TakeOperand(receiver) = set.value.as_ref() else {
        panic!("a captured receiver moves into the setter without a second owner");
    };
    for take in [replacement, receiver] {
        take.validate_resolved(layout).unwrap();
        let stores = path[..set_index]
            .iter()
            .filter_map(|instr| match instr {
                InstrBlockPy::Store(store) if store.name.location == take.name.location => {
                    Some(store)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(stores.len(), 1, "the source child is acquired only once");
        assert!(matches!(stores[0].lifetime, StoreLifetime::Operand { .. }));
        assert!(matches!(stores[0].value.as_ref(), InstrBlockPy::Call(_)));
        assert!(
            !path[set_index + 1..].iter().any(|instr| matches!(
                instr,
                InstrBlockPy::Del(del) if del.name.location == take.name.location
            )),
            "a moved operand must not be deleted again after the setter"
        );
    }
}

#[test]
fn assignment_operand_subscript_moves_receiver_key_and_replacement() {
    use soac_ir_blockpy::InstrBlockPy;

    let function = lowered_assignment_function(
        "def assign(target, key, make):\n    target()[key()] = make()\n",
    );
    let path = assignment_normal_path(&function);
    let layout = function.storage_layout().as_ref().unwrap();
    let (set_index, set) = path
        .iter()
        .enumerate()
        .find_map(|(index, instr)| match instr {
            InstrBlockPy::SetItem(op) => Some((index, op)),
            _ => None,
        })
        .expect("the source subscript assignment remains an explicit operation");
    let mut acquired = Vec::new();
    for input in [
        set.replacement.as_ref(),
        set.value.as_ref(),
        set.index.as_ref(),
    ] {
        let InstrBlockPy::TakeOperand(take) = input else {
            panic!("every captured source input must move into the setter");
        };
        take.validate_resolved(layout).unwrap();
        let stores = path[..set_index]
            .iter()
            .enumerate()
            .filter_map(|(index, instr)| match instr {
                InstrBlockPy::Store(store) if store.name.location == take.name.location => {
                    Some((index, store))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(stores.len(), 1, "each source child is acquired once");
        assert!(matches!(
            stores[0].1.lifetime,
            StoreLifetime::Operand { .. }
        ));
        assert!(matches!(stores[0].1.value.as_ref(), InstrBlockPy::Call(_)));
        acquired.push(stores[0].0);
        assert!(
            !path[set_index + 1..].iter().any(|instr| matches!(
                instr,
                InstrBlockPy::Del(del) if del.name.location == take.name.location
            )),
            "the operation already owns the captured input on both result edges"
        );
    }
    assert!(acquired[0] < acquired[1] && acquired[1] < acquired[2]);
}

#[test]
fn assignment_operand_chained_targets_copy_until_the_last_move() {
    use soac_ir_blockpy::InstrBlockPy;

    let function = lowered_assignment_function(
        "def assign(first, second, make):\n    first().value = second().value = make()\n",
    );
    let path = assignment_normal_path(&function);
    let setters = path
        .iter()
        .filter_map(|instr| match instr {
            InstrBlockPy::SetAttr(op) => Some(op),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [first, last] = setters.as_slice() else {
        panic!("both source targets must execute in order");
    };
    let InstrBlockPy::TakeOperand(copy) = first.replacement.as_ref() else {
        panic!("the earlier target consumes its explicit COPY operand");
    };
    let InstrBlockPy::TakeOperand(take) = last.replacement.as_ref() else {
        panic!("only the last target consumes the staged RHS owner");
    };
    assert_ne!(copy.name.location, take.name.location);
    let copy_store = path
        .iter()
        .find_map(|instr| match instr {
            InstrBlockPy::Store(store) if store.name.location == copy.name.location => Some(store),
            _ => None,
        })
        .expect("an earlier target must acquire a separate owned duplicate");
    assert!(matches!(copy_store.lifetime, StoreLifetime::Operand { .. }));
    assert!(
        matches!(
            copy_store.value.as_ref(),
            InstrBlockPy::Load(load) if load.name.location == take.name.location
        ),
        "the duplicate must come from the once-evaluated original RHS"
    );
    copy.validate_resolved(function.storage_layout().as_ref().unwrap())
        .unwrap();
    take.validate_resolved(function.storage_layout().as_ref().unwrap())
        .unwrap();
    assert_eq!(
        path.iter()
            .filter(|instr| matches!(
                instr,
                InstrBlockPy::Store(store) if store.name.location == take.name.location
            ))
            .count(),
        1,
        "chained assignment evaluates its RHS once"
    );
    assert!(setters
        .iter()
        .all(|set| matches!(set.value.as_ref(), InstrBlockPy::TakeOperand(_))));
    assert!(
        !path.iter().any(|instr| matches!(
            instr,
            InstrBlockPy::Del(del) if del.name.location == take.name.location
        )),
        "the final move already clears the staged RHS"
    );
}

#[test]
fn assignment_operand_receiver_failure_precedes_replacement_consumption() {
    use crate::block_py::instr_any;
    use soac_ir_blockpy::InstrBlockPy;

    let function = lowered_assignment_function(
        "def assign(receiver, make, record):\n    try:\n        receiver().value = make()\n    except LookupError:\n        record()\n",
    );
    let path = assignment_normal_path(&function);
    let (set_index, set) = path
        .iter()
        .enumerate()
        .find_map(|(index, instr)| match instr {
            InstrBlockPy::SetAttr(op) => Some((index, op)),
            _ => None,
        })
        .expect("source setter");
    let InstrBlockPy::TakeOperand(replacement) = set.replacement.as_ref() else {
        panic!("the replacement moves only at the final setter operation");
    };
    let InstrBlockPy::TakeOperand(receiver) = set.value.as_ref() else {
        panic!("the successfully prepared receiver moves into the setter");
    };
    let acquisition_index = |location| {
        path.iter()
            .position(|instr| {
                matches!(
                    instr,
                    InstrBlockPy::Store(store) if store.name.location == location
                )
            })
            .expect("explicit Operand acquisition")
    };
    let rhs_index = acquisition_index(replacement.name.location);
    let receiver_index = acquisition_index(receiver.name.location);
    assert!(rhs_index < receiver_index && receiver_index < set_index);
    assert!(
        !path[..set_index]
            .iter()
            .any(|instr| instr_any(*instr, |child| matches!(
                child,
                InstrBlockPy::TakeOperand(take) if take.name.location == replacement.name.location
            ))),
        "a failing receiver must still find the unconsumed RHS primary for cleanup"
    );
    let receiver_block = function
        .blocks
        .iter()
        .find(|block| {
            block.body.iter().any(|instr| {
                matches!(
                    instr,
                    InstrBlockPy::Store(store) if store.name.location == receiver.name.location
                )
            })
        })
        .expect("receiver producer block");
    assert!(
        receiver_block.exc_edge.is_some(),
        "the real raising receiver has a handled error edge"
    );
}

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
