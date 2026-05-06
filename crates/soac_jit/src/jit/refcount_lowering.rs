use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::FunctionBuilder;
use soac_ir_typed::PyObjFacts;

const PY_IMMORTAL_INITIAL_REFCNT: i64 = 3_i64 << 30;

#[derive(Clone, Copy)]
pub(super) enum RefcountLowering {
    Disabled,
    HelperCalls {
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
    },
    Explicit {
        incref_ref: ir::FuncRef,
        decref_ref: ir::FuncRef,
        dealloc_preserving_error_ref: ir::FuncRef,
    },
}

impl RefcountLowering {
    pub(super) fn emit_incref(
        self,
        fb: &mut FunctionBuilder<'_>,
        ptr_ty: ir::Type,
        value: ir::Value,
        facts: Option<PyObjFacts>,
    ) {
        if facts.is_some_and(PyObjFacts::is_immortal) {
            return;
        }
        match self {
            Self::Disabled => {}
            Self::HelperCalls { incref_ref, .. } => {
                fb.ins().call(incref_ref, &[value]);
            }
            Self::Explicit { incref_ref, .. } => {
                if facts.is_some_and(py_facts_prove_non_null) {
                    emit_explicit_incref(fb, ptr_ty, value, facts);
                } else {
                    fb.ins().call(incref_ref, &[value]);
                }
            }
        }
    }

    pub(super) fn emit_decref(
        self,
        fb: &mut FunctionBuilder<'_>,
        ptr_ty: ir::Type,
        thread_state_value: ir::Value,
        value: ir::Value,
        facts: Option<PyObjFacts>,
    ) {
        if facts.is_some_and(PyObjFacts::is_immortal) {
            return;
        }
        match self {
            Self::Disabled => {}
            Self::HelperCalls { decref_ref, .. } => {
                fb.ins().call(decref_ref, &[thread_state_value, value]);
            }
            Self::Explicit {
                decref_ref,
                dealloc_preserving_error_ref,
                ..
            } => {
                if facts.is_some_and(py_facts_prove_non_null) {
                    emit_explicit_decref(
                        fb,
                        ptr_ty,
                        thread_state_value,
                        value,
                        facts,
                        dealloc_preserving_error_ref,
                    );
                } else {
                    fb.ins().call(decref_ref, &[thread_state_value, value]);
                }
            }
        }
    }
}

fn emit_explicit_incref(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    value: ir::Value,
    facts: Option<PyObjFacts>,
) {
    let done_block = fb.create_block();
    let apply_block = fb.create_block();
    if !facts.is_some_and(py_facts_prove_non_null) {
        let non_null_block = fb.create_block();
        let value_is_null = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, value, 0);
        fb.ins()
            .brif(value_is_null, done_block, &[], non_null_block, &[]);
        fb.switch_to_block(non_null_block);
    }
    let current_refcnt = fb
        .ins()
        .load(ir::types::I32, ir::MemFlags::trusted(), value, 0);
    let value_is_immortal = fb.ins().icmp_imm(
        ir::condcodes::IntCC::UnsignedGreaterThanOrEqual,
        current_refcnt,
        PY_IMMORTAL_INITIAL_REFCNT,
    );
    fb.ins()
        .brif(value_is_immortal, done_block, &[], apply_block, &[]);

    fb.switch_to_block(apply_block);
    let next_refcnt = fb.ins().iadd_imm(current_refcnt, 1);
    fb.ins()
        .store(ir::MemFlags::trusted(), next_refcnt, value, 0);
    fb.ins().jump(done_block, &[]);

    fb.switch_to_block(done_block);
    let _ = ptr_ty;
}

fn emit_explicit_decref(
    fb: &mut FunctionBuilder<'_>,
    ptr_ty: ir::Type,
    thread_state_value: ir::Value,
    value: ir::Value,
    facts: Option<PyObjFacts>,
    dealloc_preserving_error_ref: ir::FuncRef,
) {
    let done_block = fb.create_block();
    let apply_block = fb.create_block();
    let dealloc_block = fb.create_block();
    fb.set_cold_block(dealloc_block);
    if !facts.is_some_and(py_facts_prove_non_null) {
        let non_null_block = fb.create_block();
        let value_is_null = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, value, 0);
        fb.ins()
            .brif(value_is_null, done_block, &[], non_null_block, &[]);
        fb.switch_to_block(non_null_block);
    }
    let current_refcnt = fb
        .ins()
        .load(ir::types::I32, ir::MemFlags::trusted(), value, 0);
    let value_is_immortal =
        fb.ins()
            .icmp_imm(ir::condcodes::IntCC::SignedLessThan, current_refcnt, 0);
    fb.ins()
        .brif(value_is_immortal, done_block, &[], apply_block, &[]);

    fb.switch_to_block(apply_block);
    let next_refcnt = fb.ins().iadd_imm(current_refcnt, -1);
    fb.ins()
        .store(ir::MemFlags::trusted(), next_refcnt, value, 0);
    let refcnt_is_zero = fb
        .ins()
        .icmp_imm(ir::condcodes::IntCC::Equal, next_refcnt, 0);
    fb.ins()
        .brif(refcnt_is_zero, dealloc_block, &[], done_block, &[]);

    fb.switch_to_block(dealloc_block);
    fb.ins()
        .call(dealloc_preserving_error_ref, &[thread_state_value, value]);
    fb.ins().jump(done_block, &[]);

    fb.switch_to_block(done_block);
    let _ = ptr_ty;
}

fn py_facts_prove_non_null(facts: PyObjFacts) -> bool {
    facts.is_non_null_ref()
}
