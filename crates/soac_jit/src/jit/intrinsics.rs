use super::operation_specializations;
use super::{
    CpythonTypeSymbol, ImportSpec, JitDeoptExitRef, JitEmitCtx, JitGuardMissDispatch,
    OptV3IndexedGlobalAccessPlan, RelocTypeRef, SOAC_RUNTIME_LOAD_GLOBAL_IMPORT,
    SOAC_RUNTIME_STORE_GLOBAL_IMPORT, SigType, emit_exact_type_version_match,
    emit_increment_counter_slot, emit_owned_module_constant_from_parts, step_null_block_args,
};
use crate::jit::blockpy_intrinsics;
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::FunctionBuilder;
use pyo3::ffi;
use soac_core::block_py::{
    CounterId, HasSemanticInstrId, Instr, InstrId, NameLike, NameLocation, ResolvedName,
};
use soac_lowering::passes::{InstrCodegen, InstrTyped, PyExactType, PyObjFacts};
use soac_opt::operator_specialization::{
    BINARY_RHS_TAG_SHIFT, ExactIntBinaryOpKind, ExactIntUnaryOpKind, ExactTypeTag, UNARY_TAG_SHIFT,
    pack_binary_shape, pack_unary_shape, unpack_binary_shape, unpack_unary_shape,
};
use std::mem::offset_of;

const PY_LONG_SIGN_MASK: i64 = 3;
const PY_LONG_NON_SIZE_BITS: i64 = 3;
const PYLONG_COMPACT_TAG_LIMIT: i64 = 2 << PY_LONG_NON_SIZE_BITS;

#[repr(C)]
struct RawPyLongValue {
    lv_tag: usize,
    ob_digit: [u32; 1],
}

#[repr(C)]
struct RawPyLongObject {
    ob_base: ffi::PyObject,
    long_value: RawPyLongValue,
}

pub(super) trait OperationEmitState<'fb, E> {
    fn ctx(&self) -> &JitEmitCtx<'_>;
    fn fb(&mut self) -> &mut FunctionBuilder<'fb>;
    fn import_func(&mut self, spec: &'static ImportSpec) -> ir::FuncRef;
    fn emit_arg_values(&mut self, args: &[&E]) -> Vec<(ir::Value, bool)>;
    fn release_arg_values(&mut self, arg_values: &[(ir::Value, bool)]) {
        let thread_state_value = self.ctx().consts.thread_state_value;
        let decref_ref = self.ctx().decref_ref;
        for (value, borrowed_arg) in arg_values {
            if !borrowed_arg {
                self.fb()
                    .ins()
                    .call(decref_ref, &[thread_state_value, *value]);
            }
        }
    }

    fn finish_owned_result(&mut self, value: ir::Value) -> ir::Value {
        let ptr_ty = self.ctx().consts.ptr_ty;
        let step_null_block = self.ctx().consts.step_null_block;
        let step_null_args = step_null_block_args(self.ctx());
        let null_ptr = self.fb().ins().iconst(ptr_ty, 0);
        let value_is_null = self
            .fb()
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, value, null_ptr);
        let value_ok_block = self.fb().create_block();
        self.fb().append_block_param(value_ok_block, ptr_ty);
        self.fb().ins().brif(
            value_is_null,
            step_null_block,
            &step_null_args,
            value_ok_block,
            &[ir::BlockArg::Value(value)],
        );
        self.fb().switch_to_block(value_ok_block);
        self.fb().block_params(value_ok_block)[0]
    }
    fn emit_owned_bool_from_i32_result(&mut self, result: ir::Value) -> ir::Value;
    fn emit_owned_bool_from_cond(&mut self, cond: ir::Value) -> ir::Value;
    fn emit_i32_bool01_from_pyobject_truthiness(
        &mut self,
        value: ir::Value,
        facts: PyObjFacts,
        borrowed: bool,
        invert: bool,
    ) -> ir::Value;
    fn emit_owned_bool_from_pyobject_truthiness(
        &mut self,
        value: ir::Value,
        facts: PyObjFacts,
        borrowed: bool,
        invert: bool,
    ) -> ir::Value;
    fn emit_type_ptr_value(&mut self, owner_type_ref: &RelocTypeRef) -> Option<ir::Value>;
    fn py_facts_for_arg(&self, arg: &E) -> PyObjFacts;
    fn prepare_guard_miss_dispatch_for_instr(
        &mut self,
        _instr_id: soac_core::block_py::InstrId,
        _pre_guard_operands: &[&E],
        fallback_block: ir::Block,
    ) -> JitGuardMissDispatch {
        JitGuardMissDispatch::FallbackBlock(fallback_block)
    }
    fn emit_deopt_resume_result(
        &mut self,
        _target: JitDeoptExitRef,
        _deopt_resume_ref: ir::FuncRef,
    ) -> ir::Value {
        panic!("this operation emitter cannot materialize JIT deopt live values")
    }

    fn emit_guard_miss_deopt_resume_return(
        &mut self,
        block: ir::Block,
        fallback_counter_id: Option<CounterId>,
        arg_values: &[(ir::Value, bool)],
        target: JitDeoptExitRef,
        deopt_resume_ref: ir::FuncRef,
    ) where
        Self: Sized,
    {
        self.fb().switch_to_block(block);
        self.fb().set_cold_block(block);
        increment_counter_with_state(self, fallback_counter_id);
        self.release_arg_values(arg_values);
        let deopt_result = self.emit_deopt_resume_result(target, deopt_resume_ref);
        self.emit_deopt_result_return_or_step_null(deopt_result);
    }

    fn emit_deopt_result_return_or_step_null(&mut self, deopt_result: ir::Value)
    where
        Self: Sized,
    {
        let ptr_ty = self.ctx().consts.ptr_ty;
        let null_ptr = self.fb().ins().iconst(ptr_ty, 0);
        let deopt_result_is_null =
            self.fb()
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, deopt_result, null_ptr);
        let deopt_success_block = self.fb().create_block();
        self.fb().append_block_param(deopt_success_block, ptr_ty);
        self.fb().set_cold_block(deopt_success_block);
        let step_null_block = self.ctx().consts.step_null_block;
        let step_null_args = super::step_null_block_args(self.ctx());
        self.fb().ins().brif(
            deopt_result_is_null,
            step_null_block,
            &step_null_args,
            deopt_success_block,
            &[ir::BlockArg::Value(deopt_result)],
        );

        self.fb().switch_to_block(deopt_success_block);
        let resumed_result = self.fb().block_params(deopt_success_block)[0];
        self.fb().ins().return_(&[resumed_result]);
    }

    fn emit_owned_string_constant(&mut self, value: &str) -> ir::Value {
        let constant_id = self
            .ctx()
            .module_constants
            .require_unicode_constant_id(value);
        self.emit_owned_module_constant(constant_id)
    }

    fn emit_owned_module_constant(
        &mut self,
        constant_id: crate::module_constants::ModuleConstantId,
    ) -> ir::Value {
        let ptr_ty = self.ctx().consts.ptr_ty;
        let module_constant_accesses = self.ctx().consts.module_constant_accesses.clone();
        let module_constant_object_globals =
            self.ctx().consts.module_constant_object_globals.clone();
        emit_owned_module_constant_from_parts(
            self.fb(),
            constant_id,
            &module_constant_object_globals,
            ptr_ty,
            &module_constant_accesses,
        )
    }
}

macro_rules! define_owned_import_spec {
    ($spec_name:ident, $symbol:literal, $params:expr) => {
        static $spec_name: ImportSpec = ImportSpec::new($symbol, $params, &[SigType::Pointer]);
    };
}

macro_rules! define_bool_import_spec {
    ($spec_name:ident, $symbol:literal, $params:expr) => {
        static $spec_name: ImportSpec = ImportSpec::new($symbol, $params, &[SigType::I32]);
    };
}

define_owned_import_spec!(
    PYNUMBER_ADD_IMPORT,
    "PyNumber_Add",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYLONG_FROM_LONGLONG_IMPORT,
    "PyLong_FromLongLong",
    &[SigType::I64]
);
define_owned_import_spec!(
    PYNUMBER_SUBTRACT_IMPORT,
    "PyNumber_Subtract",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_MULTIPLY_IMPORT,
    "PyNumber_Multiply",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_MATMUL_IMPORT,
    "PyNumber_MatrixMultiply",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_TRUE_DIVIDE_IMPORT,
    "PyNumber_TrueDivide",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_FLOOR_DIVIDE_IMPORT,
    "PyNumber_FloorDivide",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_REMAINDER_IMPORT,
    "PyNumber_Remainder",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_LSHIFT_IMPORT,
    "PyNumber_Lshift",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_RSHIFT_IMPORT,
    "PyNumber_Rshift",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_OR_IMPORT,
    "PyNumber_Or",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_XOR_IMPORT,
    "PyNumber_Xor",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_AND_IMPORT,
    "PyNumber_And",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_ADD_IMPORT,
    "PyNumber_InPlaceAdd",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_SUBTRACT_IMPORT,
    "PyNumber_InPlaceSubtract",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_MULTIPLY_IMPORT,
    "PyNumber_InPlaceMultiply",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_MATMUL_IMPORT,
    "PyNumber_InPlaceMatrixMultiply",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_TRUE_DIVIDE_IMPORT,
    "PyNumber_InPlaceTrueDivide",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_FLOOR_DIVIDE_IMPORT,
    "PyNumber_InPlaceFloorDivide",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_REMAINDER_IMPORT,
    "PyNumber_InPlaceRemainder",
    &[SigType::Pointer, SigType::Pointer]
);
static PYNUMBER_INPLACE_POWER_IMPORT: ImportSpec = ImportSpec::new(
    "PyNumber_InPlacePower",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_LSHIFT_IMPORT,
    "PyNumber_InPlaceLshift",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_RSHIFT_IMPORT,
    "PyNumber_InPlaceRshift",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_OR_IMPORT,
    "PyNumber_InPlaceOr",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_XOR_IMPORT,
    "PyNumber_InPlaceXor",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INPLACE_AND_IMPORT,
    "PyNumber_InPlaceAnd",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_POSITIVE_IMPORT,
    "PyNumber_Positive",
    &[SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_NEGATIVE_IMPORT,
    "PyNumber_Negative",
    &[SigType::Pointer]
);
define_owned_import_spec!(
    PYNUMBER_INVERT_IMPORT,
    "PyNumber_Invert",
    &[SigType::Pointer]
);
define_bool_import_spec!(PYOBJECT_NOT_IMPORT, "PyObject_Not", &[SigType::Pointer]);
define_bool_import_spec!(
    PYOBJECT_IS_TRUE_IMPORT,
    "PyObject_IsTrue",
    &[SigType::Pointer]
);
define_bool_import_spec!(
    PYSEQUENCE_CONTAINS_IMPORT,
    "PySequence_Contains",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    DP_JIT_PYOBJECT_DELITEM_IMPORT,
    "dp_jit_pyobject_delitem",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    DP_JIT_LOAD_RUNTIME_OBJ_BY_ID_IMPORT,
    "dp_jit_load_runtime_obj_by_id",
    &[SigType::I64]
);
define_owned_import_spec!(
    DP_JIT_DEL_GLOBAL_IMPORT,
    "dp_jit_del_global",
    &[SigType::Pointer, SigType::Pointer, SigType::I64]
);
define_owned_import_spec!(
    DP_JIT_DEL_GLOBAL_QUIETLY_IMPORT,
    "dp_jit_del_global_quietly",
    &[SigType::Pointer, SigType::Pointer, SigType::I64]
);
define_owned_import_spec!(
    DP_JIT_DEL_DEREF_QUIETLY_IMPORT,
    "dp_jit_del_deref_quietly",
    &[SigType::Pointer]
);
define_owned_import_spec!(
    DP_JIT_DEL_DEREF_IMPORT,
    "dp_jit_del_deref",
    &[SigType::Pointer]
);

static PYOBJECT_RICHCOMPARE_IMPORT: ImportSpec = ImportSpec::new(
    "PyObject_RichCompare",
    &[SigType::Pointer, SigType::Pointer, SigType::I32],
    &[SigType::Pointer],
);
static PYNUMBER_POWER_IMPORT: ImportSpec = ImportSpec::new(
    "PyNumber_Power",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
static DP_JIT_EXACT_LONG_BINARY_OP_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_exact_long_binary_op",
    &[SigType::I64, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
define_owned_import_spec!(
    DP_JIT_EXACT_LONG_ADD_SLOT_IMPORT,
    "dp_jit_exact_long_add_slot",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    DP_JIT_EXACT_LONG_SUB_SLOT_IMPORT,
    "dp_jit_exact_long_sub_slot",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    DP_JIT_EXACT_LONG_MUL_SLOT_IMPORT,
    "dp_jit_exact_long_mul_slot",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    DP_JIT_EXACT_LONG_TRUE_DIV_SLOT_IMPORT,
    "dp_jit_exact_long_true_div_slot",
    &[SigType::Pointer, SigType::Pointer]
);
define_owned_import_spec!(
    DP_JIT_EXACT_LONG_RICHCOMPARE_SLOT_IMPORT,
    "dp_jit_exact_long_richcompare_slot",
    &[SigType::Pointer, SigType::Pointer, SigType::I32]
);
static DP_JIT_EXACT_LONG_UNARY_OP_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_exact_long_unary_op",
    &[SigType::I64, SigType::Pointer],
    &[SigType::Pointer],
);

pub(super) static OPERATION_IMPORT_SPECS: &[&ImportSpec] = &[
    &PYNUMBER_ADD_IMPORT,
    &PYLONG_FROM_LONGLONG_IMPORT,
    &PYNUMBER_SUBTRACT_IMPORT,
    &PYNUMBER_MULTIPLY_IMPORT,
    &PYNUMBER_MATMUL_IMPORT,
    &PYNUMBER_TRUE_DIVIDE_IMPORT,
    &PYNUMBER_FLOOR_DIVIDE_IMPORT,
    &PYNUMBER_REMAINDER_IMPORT,
    &PYNUMBER_LSHIFT_IMPORT,
    &PYNUMBER_RSHIFT_IMPORT,
    &PYNUMBER_OR_IMPORT,
    &PYNUMBER_XOR_IMPORT,
    &PYNUMBER_AND_IMPORT,
    &PYNUMBER_INPLACE_ADD_IMPORT,
    &PYNUMBER_INPLACE_SUBTRACT_IMPORT,
    &PYNUMBER_INPLACE_MULTIPLY_IMPORT,
    &PYNUMBER_INPLACE_MATMUL_IMPORT,
    &PYNUMBER_INPLACE_TRUE_DIVIDE_IMPORT,
    &PYNUMBER_INPLACE_FLOOR_DIVIDE_IMPORT,
    &PYNUMBER_INPLACE_REMAINDER_IMPORT,
    &PYNUMBER_INPLACE_POWER_IMPORT,
    &PYNUMBER_INPLACE_LSHIFT_IMPORT,
    &PYNUMBER_INPLACE_RSHIFT_IMPORT,
    &PYNUMBER_INPLACE_OR_IMPORT,
    &PYNUMBER_INPLACE_XOR_IMPORT,
    &PYNUMBER_INPLACE_AND_IMPORT,
    &PYNUMBER_POSITIVE_IMPORT,
    &PYNUMBER_NEGATIVE_IMPORT,
    &PYNUMBER_INVERT_IMPORT,
    &PYOBJECT_NOT_IMPORT,
    &PYOBJECT_IS_TRUE_IMPORT,
    &PYSEQUENCE_CONTAINS_IMPORT,
    &DP_JIT_PYOBJECT_DELITEM_IMPORT,
    &DP_JIT_LOAD_RUNTIME_OBJ_BY_ID_IMPORT,
    &DP_JIT_DEL_GLOBAL_IMPORT,
    &DP_JIT_DEL_GLOBAL_QUIETLY_IMPORT,
    &DP_JIT_DEL_DEREF_QUIETLY_IMPORT,
    &DP_JIT_DEL_DEREF_IMPORT,
    &PYOBJECT_RICHCOMPARE_IMPORT,
    &PYNUMBER_POWER_IMPORT,
    &DP_JIT_EXACT_LONG_BINARY_OP_IMPORT,
    &DP_JIT_EXACT_LONG_ADD_SLOT_IMPORT,
    &DP_JIT_EXACT_LONG_SUB_SLOT_IMPORT,
    &DP_JIT_EXACT_LONG_MUL_SLOT_IMPORT,
    &DP_JIT_EXACT_LONG_TRUE_DIV_SLOT_IMPORT,
    &DP_JIT_EXACT_LONG_RICHCOMPARE_SLOT_IMPORT,
    &DP_JIT_EXACT_LONG_UNARY_OP_IMPORT,
];

const PYOBJECT_OB_TYPE_OFFSET: i32 = offset_of!(ffi::PyObject, ob_type) as i32;
fn emit_positional_owned_call<'fb, E>(
    spec: &'static ImportSpec,
    state: &mut impl OperationEmitState<'fb, E>,
    args: &[&E],
) -> ir::Value {
    let arg_values = state.emit_arg_values(args);
    emit_positional_owned_call_from_values(spec, state, &arg_values)
}

fn emit_positional_owned_call_from_values<'fb, E>(
    spec: &'static ImportSpec,
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let func_ref = state.import_func(spec);
    let values = arg_values
        .iter()
        .map(|(value, _)| *value)
        .collect::<Vec<_>>();
    let call_inst = state.fb().ins().call(func_ref, &values);
    state.release_arg_values(arg_values);
    let result = state.fb().inst_results(call_inst)[0];
    state.finish_owned_result(result)
}

fn emit_positional_bool_call_from_values<'fb, E>(
    spec: &'static ImportSpec,
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let func_ref = state.import_func(spec);
    let values = arg_values
        .iter()
        .map(|(value, _)| *value)
        .collect::<Vec<_>>();
    let call_inst = state.fb().ins().call(func_ref, &values);
    state.release_arg_values(arg_values);
    let result = state.fb().inst_results(call_inst)[0];
    state.emit_owned_bool_from_i32_result(result)
}

fn emit_pow_like_from_values<'fb, E>(
    spec: &'static ImportSpec,
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let func_ref = state.import_func(spec);
    let none_constant_id = state.ctx().consts.none_constant_id;
    let none_const = state.emit_owned_module_constant(none_constant_id);
    let call_inst = match arg_values {
        [(left, _), (right, _)] => state
            .fb()
            .ins()
            .call(func_ref, &[*left, *right, none_const]),
        [(left, _), (right, _), (modulo, _)] => {
            state.fb().ins().call(func_ref, &[*left, *right, *modulo])
        }
        _ => panic!(
            "pow-like operation received unsupported arity {}",
            arg_values.len()
        ),
    };
    state.release_arg_values(arg_values);
    let result = state.fb().inst_results(call_inst)[0];
    state.finish_owned_result(result)
}

fn emit_richcompare_from_values<'fb, E>(
    compare_op: i32,
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let func_ref = state.import_func(&PYOBJECT_RICHCOMPARE_IMPORT);
    let compare_op = state.fb().ins().iconst(ir::types::I32, compare_op as i64);
    let call_inst = state
        .fb()
        .ins()
        .call(func_ref, &[arg_values[0].0, arg_values[1].0, compare_op]);
    state.release_arg_values(arg_values);
    let result = state.fb().inst_results(call_inst)[0];
    state.finish_owned_result(result)
}

fn emit_identity_compare_from_values<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let cond = state.fb().ins().icmp(
        ir::condcodes::IntCC::Equal,
        arg_values[0].0,
        arg_values[1].0,
    );
    state.release_arg_values(arg_values);
    state.emit_owned_bool_from_cond(cond)
}

fn emit_counted_getattr_fallback<'fb, E: Instr>(
    state: &mut impl OperationEmitState<'fb, E>,
    instr_id: Option<soac_core::block_py::InstrId>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    if let Some(instr_id) = instr_id {
        let counter_id = state
            .ctx()
            .field_generic_getattr_counter_ids
            .get(&instr_id)
            .copied();
        increment_counter_with_state(state, counter_id);
    }
    let pyobject_getattr_ref = state.ctx().pyobject_getattr_ref;
    let call_inst = state
        .fb()
        .ins()
        .call(pyobject_getattr_ref, &[arg_values[0].0, arg_values[1].0]);
    state.fb().inst_results(call_inst)[0]
}

fn emit_specialized_getattr<'fb>(
    op: &blockpy_intrinsics::GetAttr<InstrCodegen>,
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
) -> Option<ir::Value> {
    let instr_id = op.semantic_instr_id();
    let specializations = state
        .ctx()
        .field_index_specializations_by_instr
        .get(&instr_id)?
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    if specializations.is_empty() {
        return None;
    }
    let arg_values = state.emit_arg_values(&[op.value.as_ref(), op.attr.as_ref()]);
    let ptr_ty = state.ctx().consts.ptr_ty;
    let i64_ty = state.ctx().consts.i64_ty;
    let null_ptr = state.fb().ins().iconst(ptr_ty, 0);
    let probe_field_indexed_ref = state.ctx().probe_field_indexed_ref;
    let incref_ref = state.ctx().incref_ref;
    let hit_counter_id = state
        .ctx()
        .field_indexed_hit_counter_ids
        .get(&instr_id)
        .copied();
    let fallback_counter_id = state
        .ctx()
        .field_indexed_fallback_counter_ids
        .get(&instr_id)
        .copied();

    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);
    let pre_guard_operands = [op.value.as_ref(), op.attr.as_ref()];
    let guard_miss_dispatch =
        state.prepare_guard_miss_dispatch_for_instr(instr_id, &pre_guard_operands, fallback_block);
    for (index, specialization) in specializations.iter().enumerate() {
        let Some(owner_type) = state.emit_type_ptr_value(&specialization.owner_type_ref) else {
            continue;
        };
        let maybe_direct_block = state.fb().create_block();
        let direct_block = state.fb().create_block();
        state.fb().append_block_param(direct_block, ptr_ty);
        let next_guard_block = if index + 1 == specializations.len() {
            fallback_block
        } else {
            state.fb().create_block()
        };
        let expected_index = state
            .fb()
            .ins()
            .iconst(i64_ty, i64::from(specialization.expected_index));
        let type_matches = emit_exact_type_version_match(
            state.fb(),
            arg_values[0].0,
            owner_type,
            specialization.type_version,
        );
        state
            .fb()
            .ins()
            .brif(type_matches, maybe_direct_block, &[], next_guard_block, &[]);

        state.fb().switch_to_block(maybe_direct_block);
        let direct_inst = state.fb().ins().call(
            probe_field_indexed_ref,
            &[arg_values[0].0, arg_values[1].0, expected_index],
        );
        let direct_value = state.fb().inst_results(direct_inst)[0];
        let direct_is_null =
            state
                .fb()
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, direct_value, null_ptr);
        let direct_miss_block = guard_miss_dispatch.branch_block();
        state.fb().ins().brif(
            direct_is_null,
            direct_miss_block,
            &[],
            direct_block,
            &[ir::BlockArg::Value(direct_value)],
        );

        state.fb().switch_to_block(direct_block);
        let direct_value = state.fb().block_params(direct_block)[0];
        state.fb().ins().call(incref_ref, &[direct_value]);
        increment_counter_with_state(state, hit_counter_id);
        state.release_arg_values(&arg_values);
        state
            .fb()
            .ins()
            .jump(result_block, &[ir::BlockArg::Value(direct_value)]);

        if index + 1 != specializations.len() {
            state.fb().switch_to_block(next_guard_block);
        }
    }

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(fallback_block) => {
            state.fb().switch_to_block(fallback_block);
            increment_counter_with_state(state, fallback_counter_id);
            let fallback_value = emit_counted_getattr_fallback(state, None, &arg_values);
            state.release_arg_values(&arg_values);
            state
                .fb()
                .ins()
                .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            state.emit_guard_miss_deopt_resume_return(
                block,
                fallback_counter_id,
                &arg_values,
                target,
                deopt_resume_ref,
            );
        }
    }

    state.fb().switch_to_block(result_block);
    let result = state.fb().block_params(result_block)[0];
    Some(state.finish_owned_result(result))
}

fn emit_setattr_fallback<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    instr_id: Option<soac_core::block_py::InstrId>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    if let Some(instr_id) = instr_id {
        let counter_id = state
            .ctx()
            .field_generic_setattr_counter_ids
            .get(&instr_id)
            .copied();
        increment_counter_with_state(state, counter_id);
    }
    let pyobject_setattr_ref = state.ctx().pyobject_setattr_ref;
    let call_inst = state.fb().ins().call(
        pyobject_setattr_ref,
        &[arg_values[0].0, arg_values[1].0, arg_values[2].0],
    );
    state.fb().inst_results(call_inst)[0]
}

fn emit_specialized_setattr<'fb>(
    op: &blockpy_intrinsics::SetAttr<InstrCodegen>,
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
) -> Option<ir::Value> {
    if !state.ctx().behavior_change_indexed_stores {
        return None;
    }

    let instr_id = op.semantic_instr_id();
    let specializations = state
        .ctx()
        .field_index_specializations_by_instr
        .get(&instr_id)?
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    if specializations.is_empty() {
        return None;
    }
    let arg_values =
        state.emit_arg_values(&[op.value.as_ref(), op.attr.as_ref(), op.replacement.as_ref()]);
    let ptr_ty = state.ctx().consts.ptr_ty;
    let i64_ty = state.ctx().consts.i64_ty;
    let i32_ty = state.ctx().consts.i32_ty;
    let zero_i32 = state.fb().ins().iconst(i32_ty, 0);
    let store_field_indexed_ref = state.ctx().store_field_indexed_ref;
    let hit_counter_id = state
        .ctx()
        .field_indexed_hit_counter_ids
        .get(&instr_id)
        .copied();
    let fallback_counter_id = state
        .ctx()
        .field_indexed_fallback_counter_ids
        .get(&instr_id)
        .copied();

    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);
    let pre_guard_operands = [op.value.as_ref(), op.attr.as_ref(), op.replacement.as_ref()];
    let guard_miss_dispatch =
        state.prepare_guard_miss_dispatch_for_instr(instr_id, &pre_guard_operands, fallback_block);
    for (index, specialization) in specializations.iter().enumerate() {
        let Some(owner_type) = state.emit_type_ptr_value(&specialization.owner_type_ref) else {
            continue;
        };
        let maybe_direct_block = state.fb().create_block();
        let direct_block = state.fb().create_block();
        let next_guard_block = if index + 1 == specializations.len() {
            fallback_block
        } else {
            state.fb().create_block()
        };
        let expected_index = state
            .fb()
            .ins()
            .iconst(i64_ty, i64::from(specialization.expected_index));
        let type_matches = emit_exact_type_version_match(
            state.fb(),
            arg_values[0].0,
            owner_type,
            specialization.type_version,
        );
        state
            .fb()
            .ins()
            .brif(type_matches, maybe_direct_block, &[], next_guard_block, &[]);

        state.fb().switch_to_block(maybe_direct_block);
        let thread_state_value = state.ctx().consts.thread_state_value;
        let direct_inst = state.fb().ins().call(
            store_field_indexed_ref,
            &[
                thread_state_value,
                arg_values[0].0,
                arg_values[1].0,
                expected_index,
                arg_values[2].0,
            ],
        );
        let direct_result = state.fb().inst_results(direct_inst)[0];
        let direct_missed =
            state
                .fb()
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, direct_result, zero_i32);
        let direct_miss_block = guard_miss_dispatch.branch_block();
        state
            .fb()
            .ins()
            .brif(direct_missed, direct_miss_block, &[], direct_block, &[]);

        state.fb().switch_to_block(direct_block);
        increment_counter_with_state(state, hit_counter_id);
        let none_constant_id = state.ctx().consts.none_constant_id;
        let none_const = state.emit_owned_module_constant(none_constant_id);
        let incref_ref = state.ctx().incref_ref;
        state.fb().ins().call(incref_ref, &[none_const]);
        state.release_arg_values(&arg_values);
        state
            .fb()
            .ins()
            .jump(result_block, &[ir::BlockArg::Value(none_const)]);

        if index + 1 != specializations.len() {
            state.fb().switch_to_block(next_guard_block);
        }
    }

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(fallback_block) => {
            state.fb().switch_to_block(fallback_block);
            increment_counter_with_state(state, fallback_counter_id);
            let fallback_value = emit_setattr_fallback(state, None, &arg_values);
            state.release_arg_values(&arg_values);
            state
                .fb()
                .ins()
                .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            state.emit_guard_miss_deopt_resume_return(
                block,
                fallback_counter_id,
                &arg_values,
                target,
                deopt_resume_ref,
            );
        }
    }

    state.fb().switch_to_block(result_block);
    let result = state.fb().block_params(result_block)[0];
    Some(state.finish_owned_result(result))
}

fn emit_make_cell<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    initial_value: Option<&InstrCodegen>,
) -> ir::Value {
    let ptr_ty = state.ctx().consts.ptr_ty;
    let Some(initial_value) = initial_value else {
        let null_ptr = state.fb().ins().iconst(ptr_ty, 0);
        let make_cell_ref = state.ctx().make_cell_ref;
        let call_inst = state.fb().ins().call(make_cell_ref, &[null_ptr]);
        let result = state.fb().inst_results(call_inst)[0];
        return state.finish_owned_result(result);
    };
    let args = [initial_value];
    let arg_values = state.emit_arg_values(&args);
    let make_cell_ref = state.ctx().make_cell_ref;
    let call_inst = state.fb().ins().call(make_cell_ref, &[arg_values[0].0]);
    state.release_arg_values(&arg_values);
    let result = state.fb().inst_results(call_inst)[0];
    state.finish_owned_result(result)
}

fn emit_exact_type_tag_for_value<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    value: ir::Value,
) -> ir::Value {
    let ptr_ty = state.ctx().consts.ptr_ty;
    let i64_ty = state.ctx().consts.i64_ty;
    let py_long_type = state
        .emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(
            super::CpythonTypeSymbol::Long,
        ))
        .expect("PyLong_Type symbol should bind during JIT codegen");
    let object_type = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        value,
        PYOBJECT_OB_TYPE_OFFSET,
    );
    let is_long = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, object_type, py_long_type);
    let exact_int_tag = state
        .fb()
        .ins()
        .iconst(i64_ty, ExactTypeTag::Int.packed() as i64);
    let zero = state.fb().ins().iconst(i64_ty, 0);
    state.fb().ins().select(is_long, exact_int_tag, zero)
}

fn emit_unary_operator_shape_from_values<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let i64_ty = state.ctx().consts.i64_ty;
    let tag = emit_exact_type_tag_for_value(state, arg_values[0].0);
    let shift = state.fb().ins().iconst(i64_ty, UNARY_TAG_SHIFT as i64);
    state.fb().ins().ishl(tag, shift)
}

fn emit_binary_operator_shape_from_values<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let i64_ty = state.ctx().consts.i64_ty;
    let lhs_tag = emit_exact_type_tag_for_value(state, arg_values[0].0);
    let rhs_tag = emit_exact_type_tag_for_value(state, arg_values[1].0);
    let rhs_shift = state.fb().ins().iconst(i64_ty, BINARY_RHS_TAG_SHIFT as i64);
    let rhs_bits = state.fb().ins().ishl(rhs_tag, rhs_shift);
    state.fb().ins().bor(lhs_tag, rhs_bits)
}

fn emit_exact_long_binary_op<'fb, E>(
    kind: ExactIntBinaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let call_inst = match kind {
        ExactIntBinaryOpKind::Add
        | ExactIntBinaryOpKind::Sub
        | ExactIntBinaryOpKind::Mul
        | ExactIntBinaryOpKind::TrueDiv => {
            let spec = match kind {
                ExactIntBinaryOpKind::Add => &DP_JIT_EXACT_LONG_ADD_SLOT_IMPORT,
                ExactIntBinaryOpKind::Sub => &DP_JIT_EXACT_LONG_SUB_SLOT_IMPORT,
                ExactIntBinaryOpKind::Mul => &DP_JIT_EXACT_LONG_MUL_SLOT_IMPORT,
                ExactIntBinaryOpKind::TrueDiv => &DP_JIT_EXACT_LONG_TRUE_DIV_SLOT_IMPORT,
                _ => unreachable!(),
            };
            let func_ref = state.import_func(spec);
            state
                .fb()
                .ins()
                .call(func_ref, &[arg_values[0].0, arg_values[1].0])
        }
        ExactIntBinaryOpKind::Eq
        | ExactIntBinaryOpKind::Ne
        | ExactIntBinaryOpKind::Lt
        | ExactIntBinaryOpKind::Le
        | ExactIntBinaryOpKind::Gt
        | ExactIntBinaryOpKind::Ge => {
            let op = match kind {
                ExactIntBinaryOpKind::Eq => ffi::Py_EQ,
                ExactIntBinaryOpKind::Ne => ffi::Py_NE,
                ExactIntBinaryOpKind::Lt => ffi::Py_LT,
                ExactIntBinaryOpKind::Le => ffi::Py_LE,
                ExactIntBinaryOpKind::Gt => ffi::Py_GT,
                ExactIntBinaryOpKind::Ge => ffi::Py_GE,
                _ => unreachable!(),
            };
            let func_ref = state.import_func(&DP_JIT_EXACT_LONG_RICHCOMPARE_SLOT_IMPORT);
            let compare_op = state.fb().ins().iconst(ir::types::I32, op as i64);
            state
                .fb()
                .ins()
                .call(func_ref, &[arg_values[0].0, arg_values[1].0, compare_op])
        }
        _ => {
            let func_ref = state.import_func(&DP_JIT_EXACT_LONG_BINARY_OP_IMPORT);
            let i64_ty = state.ctx().consts.i64_ty;
            let kind_value = state.fb().ins().iconst(i64_ty, kind as i64);
            state
                .fb()
                .ins()
                .call(func_ref, &[kind_value, arg_values[0].0, arg_values[1].0])
        }
    };
    state.release_arg_values(arg_values);
    let result = state.fb().inst_results(call_inst)[0];
    state.finish_owned_result(result)
}

fn exact_int_compare_cond(kind: ExactIntBinaryOpKind) -> Option<ir::condcodes::IntCC> {
    Some(match kind {
        ExactIntBinaryOpKind::Eq => ir::condcodes::IntCC::Equal,
        ExactIntBinaryOpKind::Ne => ir::condcodes::IntCC::NotEqual,
        ExactIntBinaryOpKind::Lt => ir::condcodes::IntCC::SignedLessThan,
        ExactIntBinaryOpKind::Le => ir::condcodes::IntCC::SignedLessThanOrEqual,
        ExactIntBinaryOpKind::Gt => ir::condcodes::IntCC::SignedGreaterThan,
        ExactIntBinaryOpKind::Ge => ir::condcodes::IntCC::SignedGreaterThanOrEqual,
        _ => return None,
    })
}

fn emit_guarded_compact_long_i64<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    value: ir::Value,
    guard_miss_block: ir::Block,
) -> ir::Value {
    let ptr_ty = state.ctx().consts.ptr_ty;
    let i64_ty = state.ctx().consts.i64_ty;
    let i32_ty = state.ctx().consts.i32_ty;
    let py_long_type = state
        .emit_type_ptr_value(&RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Long))
        .expect("PyLong_Type symbol should bind during JIT codegen");

    let value_not_null_block = state.fb().create_block();
    let is_null = state
        .fb()
        .ins()
        .icmp_imm(ir::condcodes::IntCC::Equal, value, 0);
    state
        .fb()
        .ins()
        .brif(is_null, guard_miss_block, &[], value_not_null_block, &[]);

    state.fb().switch_to_block(value_not_null_block);
    let object_type = state.fb().ins().load(
        ptr_ty,
        ir::MemFlags::trusted(),
        value,
        PYOBJECT_OB_TYPE_OFFSET,
    );
    let is_long = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, object_type, py_long_type);
    let compact_tag_block = state.fb().create_block();
    state
        .fb()
        .ins()
        .brif(is_long, compact_tag_block, &[], guard_miss_block, &[]);

    state.fb().switch_to_block(compact_tag_block);
    let lv_tag_offset =
        offset_of!(RawPyLongObject, long_value) as i32 + offset_of!(RawPyLongValue, lv_tag) as i32;
    let digit_offset = offset_of!(RawPyLongObject, long_value) as i32
        + offset_of!(RawPyLongValue, ob_digit) as i32;
    let lv_tag = state
        .fb()
        .ins()
        .load(i64_ty, ir::MemFlags::trusted(), value, lv_tag_offset);
    let is_compact_long = state.fb().ins().icmp_imm(
        ir::condcodes::IntCC::UnsignedLessThan,
        lv_tag,
        PYLONG_COMPACT_TAG_LIMIT,
    );
    let digit_i32 = state
        .fb()
        .ins()
        .load(i32_ty, ir::MemFlags::trusted(), value, digit_offset);
    let digit_i64 = state.fb().ins().uextend(i64_ty, digit_i32);
    let sign_mask = state.fb().ins().iconst(i64_ty, PY_LONG_SIGN_MASK);
    let sign_bits = state.fb().ins().band(lv_tag, sign_mask);
    let one = state.fb().ins().iconst(i64_ty, 1);
    let sign = state.fb().ins().isub(one, sign_bits);
    let signed_value = state.fb().ins().imul(sign, digit_i64);
    let value_block = state.fb().create_block();
    state.fb().append_block_param(value_block, i64_ty);
    state.fb().ins().brif(
        is_compact_long,
        value_block,
        &[ir::BlockArg::Value(signed_value)],
        guard_miss_block,
        &[],
    );

    state.fb().switch_to_block(value_block);
    state.fb().block_params(value_block)[0]
}

fn emit_i64_overflow_guard<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    value: ir::Value,
    overflow: ir::Value,
    guard_miss_block: ir::Block,
) -> ir::Value {
    let i64_ty = state.ctx().consts.i64_ty;
    let value_ok_block = state.fb().create_block();
    state.fb().append_block_param(value_ok_block, i64_ty);
    state.fb().ins().brif(
        overflow,
        guard_miss_block,
        &[],
        value_ok_block,
        &[ir::BlockArg::Value(value)],
    );
    state.fb().switch_to_block(value_ok_block);
    state.fb().block_params(value_ok_block)[0]
}

pub(super) fn emit_v3_guarded_compact_long_i64<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrTyped>,
    value: ir::Value,
    guard_miss_block: ir::Block,
) -> ir::Value {
    emit_guarded_compact_long_i64(state, value, guard_miss_block)
}

fn emit_compact_long_compare<'fb, E>(
    kind: ExactIntBinaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    lhs: ir::Value,
    rhs: ir::Value,
) -> Option<ir::Value> {
    let cond = exact_int_compare_cond(kind)?;
    let result = state.fb().ins().icmp(cond, lhs, rhs);
    Some(state.emit_owned_bool_from_cond(result))
}

fn emit_compact_long_compare_i32_bool01<'fb, E>(
    kind: ExactIntBinaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    lhs: ir::Value,
    rhs: ir::Value,
) -> Option<ir::Value> {
    let cond = exact_int_compare_cond(kind)?;
    let result = state.fb().ins().icmp(cond, lhs, rhs);
    let i32_ty = state.ctx().consts.i32_ty;
    let zero_i32 = state.fb().ins().iconst(i32_ty, 0);
    let one_i32 = state.fb().ins().iconst(i32_ty, 1);
    Some(state.fb().ins().select(result, one_i32, zero_i32))
}

fn emit_compact_long_arithmetic<'fb, E>(
    kind: ExactIntBinaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    lhs: ir::Value,
    rhs: ir::Value,
    guard_miss_block: ir::Block,
) -> Option<ir::Value> {
    let (value, overflow) = match kind {
        ExactIntBinaryOpKind::Add => state.fb().ins().sadd_overflow(lhs, rhs),
        ExactIntBinaryOpKind::Sub => state.fb().ins().ssub_overflow(lhs, rhs),
        ExactIntBinaryOpKind::Mul => state.fb().ins().smul_overflow(lhs, rhs),
        _ => return None,
    };
    let value = emit_i64_overflow_guard(state, value, overflow, guard_miss_block);
    let py_long_from_i64_ref = state.import_func(&PYLONG_FROM_LONGLONG_IMPORT);
    let call_inst = state.fb().ins().call(py_long_from_i64_ref, &[value]);
    Some(state.fb().inst_results(call_inst)[0])
}

fn emit_compact_long_binary_op_or_deopt<'fb, E>(
    op_kind: blockpy_intrinsics::BinOpKind,
    kind: ExactIntBinaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    instr_id: soac_core::block_py::InstrId,
    pre_guard_operands: &[&E],
    arg_values: &[(ir::Value, bool)],
    fallback_counter_id: Option<CounterId>,
    generic_fallback_on_guard_miss: bool,
) -> Option<ir::Value>
where
    E: Instr,
{
    if !exact_int_kind_supports_compact_long_binary_op(kind) {
        return None;
    }

    let ptr_ty = state.ctx().consts.ptr_ty;
    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);
    let guard_miss_dispatch =
        state.prepare_guard_miss_dispatch_for_instr(instr_id, pre_guard_operands, fallback_block);
    let guard_miss_block = guard_miss_dispatch.branch_block();

    let lhs = emit_guarded_compact_long_i64(state, arg_values[0].0, guard_miss_block);
    let rhs = emit_guarded_compact_long_i64(state, arg_values[1].0, guard_miss_block);
    let direct_result = emit_compact_long_compare(kind, state, lhs, rhs)
        .or_else(|| emit_compact_long_arithmetic(kind, state, lhs, rhs, guard_miss_block))?;
    state.release_arg_values(arg_values);
    let direct_result = state.finish_owned_result(direct_result);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(direct_result)]);

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(fallback_block) => {
            state.fb().switch_to_block(fallback_block);
            increment_counter_with_state(state, fallback_counter_id);
            let fallback_result = if generic_fallback_on_guard_miss {
                emit_binop_with_arg_values(op_kind, state, arg_values)
            } else {
                emit_exact_long_binary_op(kind, state, arg_values)
            };
            state
                .fb()
                .ins()
                .jump(result_block, &[ir::BlockArg::Value(fallback_result)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            state.emit_guard_miss_deopt_resume_return(
                block,
                fallback_counter_id,
                arg_values,
                target,
                deopt_resume_ref,
            );
        }
    }

    state.fb().switch_to_block(result_block);
    Some(state.fb().block_params(result_block)[0])
}

fn exact_int_kind_supports_compact_long_binary_op(kind: ExactIntBinaryOpKind) -> bool {
    matches!(
        kind,
        ExactIntBinaryOpKind::Add
            | ExactIntBinaryOpKind::Sub
            | ExactIntBinaryOpKind::Mul
            | ExactIntBinaryOpKind::Eq
            | ExactIntBinaryOpKind::Ne
            | ExactIntBinaryOpKind::Lt
            | ExactIntBinaryOpKind::Le
            | ExactIntBinaryOpKind::Gt
            | ExactIntBinaryOpKind::Ge
    )
}

fn emit_compact_long_compare_i32_bool01_or_deopt<'fb, E>(
    op_kind: blockpy_intrinsics::BinOpKind,
    exact_int_kind: ExactIntBinaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    instr_id: soac_core::block_py::InstrId,
    pre_guard_operands: &[&E],
    arg_values: &[(ir::Value, bool)],
    hit_counter_id: Option<CounterId>,
    fallback_counter_id: Option<CounterId>,
) -> Option<ir::Value>
where
    E: Instr,
{
    exact_int_compare_cond(exact_int_kind)?;

    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ir::types::I32);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);
    let guard_miss_dispatch =
        state.prepare_guard_miss_dispatch_for_instr(instr_id, pre_guard_operands, fallback_block);
    let guard_miss_block = guard_miss_dispatch.branch_block();

    let lhs = emit_guarded_compact_long_i64(state, arg_values[0].0, guard_miss_block);
    let rhs = emit_guarded_compact_long_i64(state, arg_values[1].0, guard_miss_block);
    increment_counter_with_state(state, hit_counter_id);
    let direct_result = emit_compact_long_compare_i32_bool01(exact_int_kind, state, lhs, rhs)?;
    state.release_arg_values(arg_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(direct_result)]);

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(fallback_block) => {
            state.fb().switch_to_block(fallback_block);
            increment_counter_with_state(state, fallback_counter_id);
            let generic_result = emit_binop_with_arg_values(op_kind, state, arg_values);
            let truth = state.emit_i32_bool01_from_pyobject_truthiness(
                generic_result,
                PyObjFacts::unknown(),
                false,
                false,
            );
            state
                .fb()
                .ins()
                .jump(result_block, &[ir::BlockArg::Value(truth)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            state.emit_guard_miss_deopt_resume_return(
                block,
                fallback_counter_id,
                arg_values,
                target,
                deopt_resume_ref,
            );
        }
    }

    state.fb().switch_to_block(result_block);
    Some(state.fb().block_params(result_block)[0])
}

fn emit_exact_long_unary_op<'fb, E>(
    kind: ExactIntUnaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let func_ref = state.import_func(&DP_JIT_EXACT_LONG_UNARY_OP_IMPORT);
    let i64_ty = state.ctx().consts.i64_ty;
    let kind_value = state.fb().ins().iconst(i64_ty, kind as i64);
    let call_inst = state
        .fb()
        .ins()
        .call(func_ref, &[kind_value, arg_values[0].0]);
    state.release_arg_values(arg_values);
    let result = state.fb().inst_results(call_inst)[0];
    state.finish_owned_result(result)
}

fn emit_binop_with_arg_values<'fb, E>(
    kind: blockpy_intrinsics::BinOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    match kind {
        blockpy_intrinsics::BinOpKind::Add => {
            emit_positional_owned_call_from_values(&PYNUMBER_ADD_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Sub => {
            emit_positional_owned_call_from_values(&PYNUMBER_SUBTRACT_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Mul => {
            emit_positional_owned_call_from_values(&PYNUMBER_MULTIPLY_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::MatMul => {
            emit_positional_owned_call_from_values(&PYNUMBER_MATMUL_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::TrueDiv => {
            emit_positional_owned_call_from_values(&PYNUMBER_TRUE_DIVIDE_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::FloorDiv => {
            emit_positional_owned_call_from_values(&PYNUMBER_FLOOR_DIVIDE_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Mod => {
            emit_positional_owned_call_from_values(&PYNUMBER_REMAINDER_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Pow => {
            emit_pow_like_from_values(&PYNUMBER_POWER_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::LShift => {
            emit_positional_owned_call_from_values(&PYNUMBER_LSHIFT_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::RShift => {
            emit_positional_owned_call_from_values(&PYNUMBER_RSHIFT_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Or => {
            emit_positional_owned_call_from_values(&PYNUMBER_OR_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Xor => {
            emit_positional_owned_call_from_values(&PYNUMBER_XOR_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::And => {
            emit_positional_owned_call_from_values(&PYNUMBER_AND_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::InplaceAdd => {
            emit_positional_owned_call_from_values(&PYNUMBER_INPLACE_ADD_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::InplaceSub => emit_positional_owned_call_from_values(
            &PYNUMBER_INPLACE_SUBTRACT_IMPORT,
            state,
            arg_values,
        ),
        blockpy_intrinsics::BinOpKind::InplaceMul => emit_positional_owned_call_from_values(
            &PYNUMBER_INPLACE_MULTIPLY_IMPORT,
            state,
            arg_values,
        ),
        blockpy_intrinsics::BinOpKind::InplaceMatMul => emit_positional_owned_call_from_values(
            &PYNUMBER_INPLACE_MATMUL_IMPORT,
            state,
            arg_values,
        ),
        blockpy_intrinsics::BinOpKind::InplaceTrueDiv => emit_positional_owned_call_from_values(
            &PYNUMBER_INPLACE_TRUE_DIVIDE_IMPORT,
            state,
            arg_values,
        ),
        blockpy_intrinsics::BinOpKind::InplaceFloorDiv => emit_positional_owned_call_from_values(
            &PYNUMBER_INPLACE_FLOOR_DIVIDE_IMPORT,
            state,
            arg_values,
        ),
        blockpy_intrinsics::BinOpKind::InplaceMod => emit_positional_owned_call_from_values(
            &PYNUMBER_INPLACE_REMAINDER_IMPORT,
            state,
            arg_values,
        ),
        blockpy_intrinsics::BinOpKind::InplacePow => {
            emit_pow_like_from_values(&PYNUMBER_INPLACE_POWER_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::InplaceLShift => emit_positional_owned_call_from_values(
            &PYNUMBER_INPLACE_LSHIFT_IMPORT,
            state,
            arg_values,
        ),
        blockpy_intrinsics::BinOpKind::InplaceRShift => emit_positional_owned_call_from_values(
            &PYNUMBER_INPLACE_RSHIFT_IMPORT,
            state,
            arg_values,
        ),
        blockpy_intrinsics::BinOpKind::InplaceOr => {
            emit_positional_owned_call_from_values(&PYNUMBER_INPLACE_OR_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::InplaceXor => {
            emit_positional_owned_call_from_values(&PYNUMBER_INPLACE_XOR_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::InplaceAnd => {
            emit_positional_owned_call_from_values(&PYNUMBER_INPLACE_AND_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Eq => {
            emit_richcompare_from_values(ffi::Py_EQ, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Ne => {
            emit_richcompare_from_values(ffi::Py_NE, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Lt => {
            emit_richcompare_from_values(ffi::Py_LT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Le => {
            emit_richcompare_from_values(ffi::Py_LE, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Gt => {
            emit_richcompare_from_values(ffi::Py_GT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Ge => {
            emit_richcompare_from_values(ffi::Py_GE, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Contains => {
            emit_positional_bool_call_from_values(&PYSEQUENCE_CONTAINS_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::BinOpKind::Is => emit_identity_compare_from_values(state, arg_values),
    }
}

fn emit_binop<'fb, E>(
    kind: blockpy_intrinsics::BinOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    args: &[&E],
) -> ir::Value {
    let arg_values = state.emit_arg_values(args);
    emit_binop_with_arg_values(kind, state, &arg_values)
}

fn emit_unary_op_with_arg_values<'fb, E>(
    kind: blockpy_intrinsics::UnaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    match kind {
        blockpy_intrinsics::UnaryOpKind::Pos => {
            emit_positional_owned_call_from_values(&PYNUMBER_POSITIVE_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::UnaryOpKind::Neg => {
            emit_positional_owned_call_from_values(&PYNUMBER_NEGATIVE_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::UnaryOpKind::Invert => {
            emit_positional_owned_call_from_values(&PYNUMBER_INVERT_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::UnaryOpKind::Not => {
            emit_positional_bool_call_from_values(&PYOBJECT_NOT_IMPORT, state, arg_values)
        }
        blockpy_intrinsics::UnaryOpKind::Truth => {
            emit_positional_bool_call_from_values(&PYOBJECT_IS_TRUE_IMPORT, state, arg_values)
        }
    }
}

fn emit_unary_op_with_arg_and_values<'fb, E>(
    kind: blockpy_intrinsics::UnaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    arg: &E,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    match kind {
        blockpy_intrinsics::UnaryOpKind::Not | blockpy_intrinsics::UnaryOpKind::Truth => {
            let [(value, borrowed)] = arg_values else {
                panic!(
                    "unary truth operation received unsupported arity {}",
                    arg_values.len()
                );
            };
            state.emit_owned_bool_from_pyobject_truthiness(
                *value,
                state.py_facts_for_arg(arg),
                *borrowed,
                matches!(kind, blockpy_intrinsics::UnaryOpKind::Not),
            )
        }
        _ => emit_unary_op_with_arg_values(kind, state, arg_values),
    }
}

fn emit_unary_op<'fb, E>(
    kind: blockpy_intrinsics::UnaryOpKind,
    state: &mut impl OperationEmitState<'fb, E>,
    args: &[&E],
) -> ir::Value {
    let arg_values = state.emit_arg_values(args);
    let [arg] = args else {
        panic!("unary operation received unsupported arity {}", args.len());
    };
    emit_unary_op_with_arg_and_values(kind, state, *arg, &arg_values)
}

fn emit_specialized_binop<'fb, E>(
    op: &blockpy_intrinsics::BinOp<E>,
    state: &mut impl OperationEmitState<'fb, E>,
) -> Option<ir::Value>
where
    E: Instr,
{
    let instr_id = op.semantic_instr_id();
    let ptr_ty = state.ctx().consts.ptr_ty;
    let i64_ty = state.ctx().consts.i64_ty;
    let exact_int_kind = ExactIntBinaryOpKind::from_binop_kind(op.kind);
    let facts_prove_exact_int = exact_int_kind.is_some()
        && state
            .py_facts_for_arg(op.left.as_ref())
            .is_exact_type(PyExactType::Int)
        && state
            .py_facts_for_arg(op.right.as_ref())
            .is_exact_type(PyExactType::Int);
    let counter_id = state
        .ctx()
        .operator_shape_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_hit_counter_id = state
        .ctx()
        .operator_specialized_hit_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_fallback_counter_id = state
        .ctx()
        .operator_specialized_fallback_counter_ids
        .get(&instr_id)
        .copied();
    let hot_shapes = state
        .ctx()
        .operator_specializations
        .get(&instr_id)
        .cloned()
        .unwrap_or_default();
    let specialized_hit_counter_id = specialized_hit_counter_id;
    let specialized_fallback_counter_id = specialized_fallback_counter_id;
    if counter_id.is_none() && hot_shapes.is_empty() && !facts_prove_exact_int {
        return None;
    }

    let arg_values = state.emit_arg_values(&[op.left.as_ref(), op.right.as_ref()]);
    let mut shape = counter_id.map(|_| emit_binary_operator_shape_from_values(state, &arg_values));
    if let Some(counter_id) = counter_id {
        let counter_slot =
            super::top_value_counter_slot_for_id(state.ctx().counter_slots_by_id, counter_id)
                .unwrap_or_else(|err| panic!("{err}"));
        let top_value_counter_base_value = state
            .ctx()
            .consts
            .top_value_counter_base_value
            .unwrap_or_else(|| {
                panic!(
                    "missing top-value counter base for counter id {}",
                    counter_id.0
                )
            });
        let record_top_value_sample_ref =
            state.ctx().record_top_value_sample_ref.unwrap_or_else(|| {
                panic!(
                    "missing top-value counter helper import for counter id {}",
                    counter_id.0
                )
            });
        super::emit_record_top_value_counter_slot(
            state.fb(),
            top_value_counter_base_value,
            counter_slot,
            shape.expect("operator shape should be materialized when recording a counter"),
            record_top_value_sample_ref,
        );
    }

    let Some(exact_int_kind) = exact_int_kind else {
        return Some(emit_binop_with_arg_values(op.kind, state, &arg_values));
    };
    if facts_prove_exact_int {
        increment_counter_with_state(state, specialized_hit_counter_id);
        if let Some(result) = emit_compact_long_binary_op_or_deopt(
            op.kind,
            exact_int_kind,
            state,
            instr_id,
            &[op.left.as_ref(), op.right.as_ref()],
            &arg_values,
            specialized_fallback_counter_id,
            false,
        ) {
            return Some(result);
        }
        return Some(emit_exact_long_binary_op(
            exact_int_kind,
            state,
            &arg_values,
        ));
    }
    let exact_int_shape = pack_binary_shape(ExactTypeTag::Int, ExactTypeTag::Int);
    let supports_exact_int = hot_shapes.into_iter().any(|shape| {
        unpack_binary_shape(shape)
            .is_some_and(|shape| shape == (ExactTypeTag::Int, ExactTypeTag::Int))
    });
    if !supports_exact_int {
        return Some(emit_binop_with_arg_values(op.kind, state, &arg_values));
    }

    let pre_guard_operands = [op.left.as_ref(), op.right.as_ref()];
    if exact_int_kind_supports_compact_long_binary_op(exact_int_kind) {
        increment_counter_with_state(state, specialized_hit_counter_id);
        return emit_compact_long_binary_op_or_deopt(
            op.kind,
            exact_int_kind,
            state,
            instr_id,
            &pre_guard_operands,
            &arg_values,
            specialized_fallback_counter_id,
            true,
        );
    }

    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let generic_block = state.fb().create_block();
    state.fb().set_cold_block(generic_block);
    let guard_miss_dispatch =
        state.prepare_guard_miss_dispatch_for_instr(instr_id, &pre_guard_operands, generic_block);
    let direct_block = state.fb().create_block();
    let shape =
        shape.get_or_insert_with(|| emit_binary_operator_shape_from_values(state, &arg_values));
    let expected_shape = state.fb().ins().iconst(i64_ty, exact_int_shape as i64);
    let is_match = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, *shape, expected_shape);
    state.fb().ins().brif(
        is_match,
        direct_block,
        &[],
        guard_miss_dispatch.branch_block(),
        &[],
    );

    state.fb().switch_to_block(direct_block);
    increment_counter_with_state(state, specialized_hit_counter_id);
    let direct_result = emit_exact_long_binary_op(exact_int_kind, state, &arg_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(direct_result)]);

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(generic_block) => {
            state.fb().switch_to_block(generic_block);
            increment_counter_with_state(state, specialized_fallback_counter_id);
            let generic_result = emit_binop_with_arg_values(op.kind, state, &arg_values);
            state
                .fb()
                .ins()
                .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            state.emit_guard_miss_deopt_resume_return(
                block,
                specialized_fallback_counter_id,
                &arg_values,
                target,
                deopt_resume_ref,
            );
        }
    }

    state.fb().switch_to_block(result_block);
    Some(state.fb().block_params(result_block)[0])
}

fn emit_specialized_binop_i32_bool01<'fb, E>(
    op: &blockpy_intrinsics::BinOp<E>,
    state: &mut impl OperationEmitState<'fb, E>,
) -> Option<ir::Value>
where
    E: Instr,
{
    let instr_id = op.semantic_instr_id();
    let exact_int_kind = ExactIntBinaryOpKind::from_binop_kind(op.kind)?;
    exact_int_compare_cond(exact_int_kind)?;

    let facts_prove_exact_int = state
        .py_facts_for_arg(op.left.as_ref())
        .is_exact_type(PyExactType::Int)
        && state
            .py_facts_for_arg(op.right.as_ref())
            .is_exact_type(PyExactType::Int);
    let counter_id = state
        .ctx()
        .operator_shape_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_hit_counter_id = state
        .ctx()
        .operator_specialized_hit_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_fallback_counter_id = state
        .ctx()
        .operator_specialized_fallback_counter_ids
        .get(&instr_id)
        .copied();
    let hot_shapes = state
        .ctx()
        .operator_specializations
        .get(&instr_id)
        .cloned()
        .unwrap_or_default();
    if counter_id.is_none() && hot_shapes.is_empty() && !facts_prove_exact_int {
        return None;
    }

    let arg_values = state.emit_arg_values(&[op.left.as_ref(), op.right.as_ref()]);
    let shape = if counter_id.is_some() {
        Some(emit_binary_operator_shape_from_values(state, &arg_values))
    } else {
        None
    };
    if let Some(counter_id) = counter_id {
        let counter_slot =
            super::top_value_counter_slot_for_id(state.ctx().counter_slots_by_id, counter_id)
                .unwrap_or_else(|err| panic!("{err}"));
        let top_value_counter_base_value = state
            .ctx()
            .consts
            .top_value_counter_base_value
            .unwrap_or_else(|| {
                panic!(
                    "missing top-value counter base for counter id {}",
                    counter_id.0
                )
            });
        let record_top_value_sample_ref =
            state.ctx().record_top_value_sample_ref.unwrap_or_else(|| {
                panic!(
                    "missing top-value counter helper import for counter id {}",
                    counter_id.0
                )
            });
        super::emit_record_top_value_counter_slot(
            state.fb(),
            top_value_counter_base_value,
            counter_slot,
            shape.expect("operator shape should be materialized when recording a counter"),
            record_top_value_sample_ref,
        );
    }

    let supports_exact_int = facts_prove_exact_int
        || hot_shapes.into_iter().any(|shape| {
            unpack_binary_shape(shape)
                .is_some_and(|shape| shape == (ExactTypeTag::Int, ExactTypeTag::Int))
        });
    if !supports_exact_int {
        let generic_result = emit_binop_with_arg_values(op.kind, state, &arg_values);
        return Some(state.emit_i32_bool01_from_pyobject_truthiness(
            generic_result,
            PyObjFacts::unknown(),
            false,
            false,
        ));
    }

    let pre_guard_operands = [op.left.as_ref(), op.right.as_ref()];
    emit_compact_long_compare_i32_bool01_or_deopt(
        op.kind,
        exact_int_kind,
        state,
        instr_id,
        &pre_guard_operands,
        &arg_values,
        specialized_hit_counter_id,
        specialized_fallback_counter_id,
    )
}

fn emit_specialized_unary_op<'fb, E>(
    op: &blockpy_intrinsics::UnaryOp<E>,
    state: &mut impl OperationEmitState<'fb, E>,
) -> Option<ir::Value>
where
    E: Instr,
{
    let instr_id = op.semantic_instr_id();
    let ptr_ty = state.ctx().consts.ptr_ty;
    let i64_ty = state.ctx().consts.i64_ty;
    let facts_prove_exact_int = state
        .py_facts_for_arg(op.operand.as_ref())
        .is_exact_type(PyExactType::Int);
    let counter_id = state
        .ctx()
        .operator_shape_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_hit_counter_id = state
        .ctx()
        .operator_specialized_hit_counter_ids
        .get(&instr_id)
        .copied();
    let specialized_fallback_counter_id = state
        .ctx()
        .operator_specialized_fallback_counter_ids
        .get(&instr_id)
        .copied();
    let hot_shapes = state
        .ctx()
        .operator_specializations
        .get(&instr_id)
        .cloned()
        .unwrap_or_default();
    let specialized_hit_counter_id = specialized_hit_counter_id;
    let specialized_fallback_counter_id = specialized_fallback_counter_id;
    if counter_id.is_none() && hot_shapes.is_empty() && !facts_prove_exact_int {
        return None;
    }

    let arg_values = state.emit_arg_values(&[op.operand.as_ref()]);
    let shape = if counter_id.is_some() || (!facts_prove_exact_int && !hot_shapes.is_empty()) {
        Some(emit_unary_operator_shape_from_values(state, &arg_values))
    } else {
        None
    };
    if let Some(counter_id) = counter_id {
        let counter_slot =
            super::top_value_counter_slot_for_id(state.ctx().counter_slots_by_id, counter_id)
                .unwrap_or_else(|err| panic!("{err}"));
        let top_value_counter_base_value = state
            .ctx()
            .consts
            .top_value_counter_base_value
            .unwrap_or_else(|| {
                panic!(
                    "missing top-value counter base for counter id {}",
                    counter_id.0
                )
            });
        let record_top_value_sample_ref =
            state.ctx().record_top_value_sample_ref.unwrap_or_else(|| {
                panic!(
                    "missing top-value counter helper import for counter id {}",
                    counter_id.0
                )
            });
        super::emit_record_top_value_counter_slot(
            state.fb(),
            top_value_counter_base_value,
            counter_slot,
            shape.expect("operator shape should be materialized when recording a counter"),
            record_top_value_sample_ref,
        );
    }

    let exact_int_kind = ExactIntUnaryOpKind::from_unary_op_kind(op.kind);
    if facts_prove_exact_int {
        increment_counter_with_state(state, specialized_hit_counter_id);
        return Some(emit_exact_long_unary_op(exact_int_kind, state, &arg_values));
    }
    let exact_int_shape = pack_unary_shape(ExactTypeTag::Int);
    let supports_exact_int = hot_shapes
        .into_iter()
        .any(|shape| unpack_unary_shape(shape) == Some(ExactTypeTag::Int));
    if !supports_exact_int {
        return Some(emit_unary_op_with_arg_and_values(
            op.kind,
            state,
            op.operand.as_ref(),
            &arg_values,
        ));
    }

    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let generic_block = state.fb().create_block();
    state.fb().set_cold_block(generic_block);
    let pre_guard_operands = [op.operand.as_ref()];
    let guard_miss_dispatch =
        state.prepare_guard_miss_dispatch_for_instr(instr_id, &pre_guard_operands, generic_block);
    let direct_block = state.fb().create_block();
    let expected_shape = state.fb().ins().iconst(i64_ty, exact_int_shape as i64);
    let is_match =
        state
            .fb()
            .ins()
            .icmp(ir::condcodes::IntCC::Equal, shape.unwrap(), expected_shape);
    state.fb().ins().brif(
        is_match,
        direct_block,
        &[],
        guard_miss_dispatch.branch_block(),
        &[],
    );

    state.fb().switch_to_block(direct_block);
    increment_counter_with_state(state, specialized_hit_counter_id);
    let direct_result = emit_exact_long_unary_op(exact_int_kind, state, &arg_values);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(direct_result)]);

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(generic_block) => {
            state.fb().switch_to_block(generic_block);
            increment_counter_with_state(state, specialized_fallback_counter_id);
            let generic_result =
                emit_unary_op_with_arg_and_values(op.kind, state, op.operand.as_ref(), &arg_values);
            state
                .fb()
                .ins()
                .jump(result_block, &[ir::BlockArg::Value(generic_result)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            state.emit_guard_miss_deopt_resume_return(
                block,
                specialized_fallback_counter_id,
                &arg_values,
                target,
                deopt_resume_ref,
            );
        }
    }

    state.fb().switch_to_block(result_block);
    Some(state.fb().block_params(result_block)[0])
}

pub(super) fn increment_counter_with_state<'fb, E>(
    state: &mut impl OperationEmitState<'fb, E>,
    counter_id: Option<CounterId>,
) {
    let Some(counter_id) = counter_id else {
        return;
    };
    let scalar_counter_slot =
        super::scalar_counter_slot_for_id(state.ctx().counter_slots_by_id, counter_id)
            .unwrap_or_else(|err| panic!("{err}"));
    let scalar_counter_base_value =
        state
            .ctx()
            .consts
            .scalar_counter_base_value
            .unwrap_or_else(|| {
                panic!(
                    "missing scalar counter base for counter id {}",
                    counter_id.0
                )
            });
    emit_increment_counter_slot(state.fb(), scalar_counter_base_value, scalar_counter_slot);
}

fn emit_indexed_global_load_with_state<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    globals_obj: ir::Value,
    name_obj: ir::Value,
    slot_index: ir::Value,
    instr_id: InstrId,
) -> ir::Value {
    let ptr_ty = state.ctx().consts.ptr_ty;
    let null_ptr = state.fb().ins().iconst(ptr_ty, 0);
    let probe_global_indexed_ref = state.ctx().probe_global_indexed_ref;
    let load_global_slow_ref = state.ctx().load_global_slow_ref;
    let decref_ref = state.ctx().decref_ref;
    let incref_ref = state.ctx().incref_ref;
    let thread_state_value = state.ctx().consts.thread_state_value;
    let hit_counter_id = state
        .ctx()
        .global_indexed_hit_counter_ids
        .get(&instr_id)
        .copied();
    let fallback_counter_id = state
        .ctx()
        .global_indexed_fallback_counter_ids
        .get(&instr_id)
        .copied();
    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);
    let direct_block = state.fb().create_block();
    state.fb().append_block_param(direct_block, ptr_ty);

    let direct_inst = state.fb().ins().call(
        probe_global_indexed_ref,
        &[globals_obj, name_obj, slot_index],
    );
    let direct_value = state.fb().inst_results(direct_inst)[0];
    let direct_is_null = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, direct_value, null_ptr);
    state.fb().ins().brif(
        direct_is_null,
        fallback_block,
        &[],
        direct_block,
        &[ir::BlockArg::Value(direct_value)],
    );

    state.fb().switch_to_block(direct_block);
    let direct_value = state.fb().block_params(direct_block)[0];
    state.fb().ins().call(incref_ref, &[direct_value]);
    increment_counter_with_state(state, hit_counter_id);
    state
        .fb()
        .ins()
        .call(decref_ref, &[thread_state_value, name_obj]);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(direct_value)]);

    state.fb().switch_to_block(fallback_block);
    increment_counter_with_state(state, fallback_counter_id);
    let fallback_inst = state
        .fb()
        .ins()
        .call(load_global_slow_ref, &[globals_obj, name_obj, slot_index]);
    let fallback_value = state.fb().inst_results(fallback_inst)[0];
    state
        .fb()
        .ins()
        .call(decref_ref, &[thread_state_value, name_obj]);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);

    state.fb().switch_to_block(result_block);
    state.fb().block_params(result_block)[0]
}

fn emit_opt_v3_indexed_global_load_with_state<'fb>(
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
    globals_obj: ir::Value,
    plan: &OptV3IndexedGlobalAccessPlan,
) -> ir::Value {
    plan.expect_lowering_shape(soac_opt::plan_v3::IndexedGlobalAccessKind::Load);
    let name_obj = state.emit_owned_string_constant(plan.name.as_str());
    let slot_index = state
        .fb()
        .ins()
        .iconst(ir::types::I64, i64::from(plan.expected_index));
    emit_indexed_global_load_with_state(state, globals_obj, name_obj, slot_index, plan.source)
}

fn emit_load<'fb>(
    op: &blockpy_intrinsics::Load<InstrCodegen>,
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
) -> ir::Value {
    let func_ref = match op.name.location {
        NameLocation::Global(_) => state.import_func(&SOAC_RUNTIME_LOAD_GLOBAL_IMPORT),
        NameLocation::RuntimeName(_) => state.import_func(&DP_JIT_LOAD_RUNTIME_OBJ_BY_ID_IMPORT),
        _ => unreachable!("emit_load only applies to global and runtime helper names"),
    };
    let decref_ref = state.ctx().decref_ref;
    let thread_state_value = state.ctx().consts.thread_state_value;
    let result = match op.name.location {
        NameLocation::Global(slot) => {
            let globals_obj = state.ctx().consts.block_const;
            let ptr_ty = state.ctx().consts.ptr_ty;
            let step_null_block = state.ctx().consts.step_null_block;
            let step_null_args = super::step_null_block_args(state.ctx());
            let null_ptr = state.fb().ins().iconst(ptr_ty, 0);
            let value_ok_block = state.fb().create_block();
            state.fb().append_block_param(value_ok_block, ptr_ty);
            let instr_id = op.semantic_instr_id();
            let opt_v3_plan = state
                .ctx()
                .opt_v3_indexed_globals_by_instr
                .get(&instr_id)
                .filter(|plan| plan.access == soac_opt::plan_v3::IndexedGlobalAccessKind::Load)
                .cloned();
            let slow_value = if let Some(plan) = opt_v3_plan {
                emit_opt_v3_indexed_global_load_with_state(state, globals_obj, &plan)
            } else {
                let name_obj = state.emit_owned_string_constant(op.name.id_str());
                let slot_index = state
                    .fb()
                    .ins()
                    .iconst(ir::types::I64, i64::from(slot.slot()));
                let call_inst = state
                    .fb()
                    .ins()
                    .call(func_ref, &[globals_obj, name_obj, slot_index]);
                state
                    .fb()
                    .ins()
                    .call(decref_ref, &[thread_state_value, name_obj]);
                state.fb().inst_results(call_inst)[0]
            };
            let slow_value_is_null =
                state
                    .fb()
                    .ins()
                    .icmp(ir::condcodes::IntCC::Equal, slow_value, null_ptr);
            let slow_value_ok = state.fb().create_block();
            state.fb().append_block_param(slow_value_ok, ptr_ty);
            state.fb().ins().brif(
                slow_value_is_null,
                step_null_block,
                &step_null_args,
                slow_value_ok,
                &[ir::BlockArg::Value(slow_value)],
            );
            state.fb().switch_to_block(slow_value_ok);
            let slow_value = state.fb().block_params(slow_value_ok)[0];
            state
                .fb()
                .ins()
                .jump(value_ok_block, &[ir::BlockArg::Value(slow_value)]);

            state.fb().switch_to_block(value_ok_block);
            state.fb().block_params(value_ok_block)[0]
        }
        NameLocation::RuntimeName(_) => {
            let runtime_name = op
                .name
                .runtime_name_id()
                .expect("runtime-name load should carry a RuntimeName id");
            let runtime_name_id = state
                .fb()
                .ins()
                .iconst(ir::types::I64, i64::from(runtime_name.id()));
            let call_inst = state.fb().ins().call(func_ref, &[runtime_name_id]);
            state.fb().inst_results(call_inst)[0]
        }
        _ => unreachable!("emit_load only applies to global and runtime helper names"),
    };
    state.finish_owned_result(result)
}

fn emit_indexed_global_store_with_state<'fb, E: Instr<Name = ResolvedName>>(
    state: &mut impl OperationEmitState<'fb, E>,
    func_ref: ir::FuncRef,
    globals_obj: ir::Value,
    name_obj: ir::Value,
    slot_index: ir::Value,
    instr_id: InstrId,
    value_operand: &E,
    arg_values: &[(ir::Value, bool)],
) -> ir::Value {
    let ptr_ty = state.ctx().consts.ptr_ty;
    let null_ptr = state.fb().ins().iconst(ptr_ty, 0);
    let store_global_indexed_ref = state.ctx().store_global_indexed_ref;
    let decref_ref = state.ctx().decref_ref;
    let thread_state_value = state.ctx().consts.thread_state_value;
    let hit_counter_id = state
        .ctx()
        .global_indexed_hit_counter_ids
        .get(&instr_id)
        .copied();
    let fallback_counter_id = state
        .ctx()
        .global_indexed_fallback_counter_ids
        .get(&instr_id)
        .copied();
    let result_block = state.fb().create_block();
    state.fb().append_block_param(result_block, ptr_ty);
    let fallback_block = state.fb().create_block();
    state.fb().set_cold_block(fallback_block);
    let pre_guard_operands = [value_operand];
    let guard_miss_dispatch =
        state.prepare_guard_miss_dispatch_for_instr(instr_id, &pre_guard_operands, fallback_block);
    let direct_block = state.fb().create_block();
    state.fb().append_block_param(direct_block, ptr_ty);
    let direct_inst = state.fb().ins().call(
        store_global_indexed_ref,
        &[
            thread_state_value,
            globals_obj,
            name_obj,
            slot_index,
            arg_values[0].0,
        ],
    );
    let direct_value = state.fb().inst_results(direct_inst)[0];
    let direct_is_null = state
        .fb()
        .ins()
        .icmp(ir::condcodes::IntCC::Equal, direct_value, null_ptr);
    state.fb().ins().brif(
        direct_is_null,
        guard_miss_dispatch.branch_block(),
        &[],
        direct_block,
        &[ir::BlockArg::Value(direct_value)],
    );

    state.fb().switch_to_block(direct_block);
    let direct_value = state.fb().block_params(direct_block)[0];
    increment_counter_with_state(state, hit_counter_id);
    state.release_arg_values(arg_values);
    state
        .fb()
        .ins()
        .call(decref_ref, &[thread_state_value, name_obj]);
    state
        .fb()
        .ins()
        .jump(result_block, &[ir::BlockArg::Value(direct_value)]);

    match guard_miss_dispatch {
        JitGuardMissDispatch::FallbackBlock(fallback_block) => {
            state.fb().switch_to_block(fallback_block);
            increment_counter_with_state(state, fallback_counter_id);
            let fallback_inst = state.fb().ins().call(
                func_ref,
                &[globals_obj, name_obj, slot_index, arg_values[0].0],
            );
            let fallback_value = state.fb().inst_results(fallback_inst)[0];
            state.release_arg_values(arg_values);
            state
                .fb()
                .ins()
                .call(decref_ref, &[thread_state_value, name_obj]);
            state
                .fb()
                .ins()
                .jump(result_block, &[ir::BlockArg::Value(fallback_value)]);
        }
        JitGuardMissDispatch::DeoptResume {
            block,
            target,
            deopt_resume_ref,
        } => {
            state.fb().switch_to_block(block);
            state.fb().set_cold_block(block);
            increment_counter_with_state(state, fallback_counter_id);
            let deopt_result = state.emit_deopt_resume_result(target, deopt_resume_ref);
            state.release_arg_values(arg_values);
            state
                .fb()
                .ins()
                .call(decref_ref, &[thread_state_value, name_obj]);
            state.emit_deopt_result_return_or_step_null(deopt_result);
        }
    }

    state.fb().switch_to_block(result_block);
    state.fb().block_params(result_block)[0]
}

fn emit_opt_v3_indexed_global_store_with_state<'fb, E: Instr<Name = ResolvedName>>(
    op: &blockpy_intrinsics::Store<E>,
    state: &mut impl OperationEmitState<'fb, E>,
    func_ref: ir::FuncRef,
    globals_obj: ir::Value,
    arg_values: &[(ir::Value, bool)],
    plan: &OptV3IndexedGlobalAccessPlan,
) -> ir::Value {
    plan.expect_lowering_shape(soac_opt::plan_v3::IndexedGlobalAccessKind::Store);
    if !state.ctx().behavior_change_indexed_stores {
        panic!(
            "optimizer v3 indexed-global store emission for {} reached codegen with indexed stores disabled",
            plan.source
        );
    }
    let name_obj = state.emit_owned_string_constant(plan.name.as_str());
    let slot_index = state
        .fb()
        .ins()
        .iconst(ir::types::I64, i64::from(plan.expected_index));
    emit_indexed_global_store_with_state(
        state,
        func_ref,
        globals_obj,
        name_obj,
        slot_index,
        plan.source,
        op.value.as_ref(),
        arg_values,
    )
}

fn emit_store<'fb, E: Instr<Name = ResolvedName>>(
    op: &blockpy_intrinsics::Store<E>,
    state: &mut impl OperationEmitState<'fb, E>,
) -> ir::Value {
    let arg_values = state.emit_arg_values(&[&op.value]);
    let func_ref = state.import_func(&SOAC_RUNTIME_STORE_GLOBAL_IMPORT);
    let decref_ref = state.ctx().decref_ref;
    let thread_state_value = state.ctx().consts.thread_state_value;
    let globals_obj = state.ctx().consts.block_const;
    let instr_id = op.semantic_instr_id();
    let opt_v3_plan = state
        .ctx()
        .opt_v3_indexed_globals_by_instr
        .get(&instr_id)
        .filter(|plan| plan.access == soac_opt::plan_v3::IndexedGlobalAccessKind::Store)
        .cloned();
    let result = if let Some(plan) = opt_v3_plan {
        emit_opt_v3_indexed_global_store_with_state(
            op,
            state,
            func_ref,
            globals_obj,
            &arg_values,
            &plan,
        )
    } else {
        let expected_index = match op.name.location {
            NameLocation::Global(slot) => slot.slot(),
            _ => unreachable!("emit_store only applies to global names"),
        };
        let name_obj = state.emit_owned_string_constant(op.name.id_str());
        let slot_index = state
            .fb()
            .ins()
            .iconst(ir::types::I64, i64::from(expected_index));
        let call_inst = state.fb().ins().call(
            func_ref,
            &[globals_obj, name_obj, slot_index, arg_values[0].0],
        );
        let result = state.fb().inst_results(call_inst)[0];
        state.release_arg_values(&arg_values);
        state
            .fb()
            .ins()
            .call(decref_ref, &[thread_state_value, name_obj]);
        result
    };
    state.finish_owned_result(result)
}

fn emit_del<'fb>(
    op: &blockpy_intrinsics::Del<InstrCodegen>,
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
) -> ir::Value {
    let name_obj = state.emit_owned_string_constant(op.name.id_str());
    let func_ref = if op.quietly {
        state.import_func(&DP_JIT_DEL_GLOBAL_QUIETLY_IMPORT)
    } else {
        state.import_func(&DP_JIT_DEL_GLOBAL_IMPORT)
    };
    let decref_ref = state.ctx().decref_ref;
    let thread_state_value = state.ctx().consts.thread_state_value;
    let globals_obj = state.ctx().consts.block_const;
    let slot_index = match op.name.location {
        NameLocation::Global(slot) => state
            .fb()
            .ins()
            .iconst(ir::types::I64, i64::from(slot.slot())),
        _ => unreachable!("emit_del only applies to global names"),
    };
    let call_inst = state
        .fb()
        .ins()
        .call(func_ref, &[globals_obj, name_obj, slot_index]);
    state
        .fb()
        .ins()
        .call(decref_ref, &[thread_state_value, name_obj]);
    let result = state.fb().inst_results(call_inst)[0];
    state.finish_owned_result(result)
}

pub(super) fn emit_del_deref_raw_cell<'fb, E>(
    cell_obj: ir::Value,
    quietly: bool,
    state: &mut impl OperationEmitState<'fb, E>,
) -> ir::Value {
    let func_ref = state.import_func(if quietly {
        &DP_JIT_DEL_DEREF_QUIETLY_IMPORT
    } else {
        &DP_JIT_DEL_DEREF_IMPORT
    });
    let decref_ref = state.ctx().decref_ref;
    let thread_state_value = state.ctx().consts.thread_state_value;
    let call_inst = state.fb().ins().call(func_ref, &[cell_obj]);
    state
        .fb()
        .ins()
        .call(decref_ref, &[thread_state_value, cell_obj]);
    let result = state.fb().inst_results(call_inst)[0];
    state.finish_owned_result(result)
}

pub(super) fn emit_operation<'fb>(
    operation: &InstrCodegen,
    state: &mut impl OperationEmitState<'fb, InstrCodegen>,
) -> Option<ir::Value> {
    match operation {
        InstrCodegen::CalleeFunctionId(_) => None,
        InstrCodegen::DirectFunctionIdGuardTest(_)
        | InstrCodegen::DirectReceiverTypeVersionGuardTest(_) => None,
        InstrCodegen::Tuple(_) => None,
        InstrCodegen::Call(_) => None,
        InstrCodegen::CallDirect(_) => None,
        InstrCodegen::DirectMethodCall(_) => None,
        InstrCodegen::BinOp(op) => emit_specialized_binop(op, state).or_else(|| {
            Some(emit_binop(
                op.kind,
                state,
                &[op.left.as_ref(), op.right.as_ref()],
            ))
        }),
        InstrCodegen::UnaryOp(op) => emit_specialized_unary_op(op, state)
            .or_else(|| Some(emit_unary_op(op.kind, state, &[op.operand.as_ref()]))),
        InstrCodegen::GetAttr(op) => {
            if let Some(value) = emit_specialized_getattr(op, state) {
                Some(value)
            } else {
                let instr_id = Some(op.semantic_instr_id());
                let arg_values = state.emit_arg_values(&[op.value.as_ref(), op.attr.as_ref()]);
                let result = emit_counted_getattr_fallback(state, instr_id, &arg_values);
                state.release_arg_values(&arg_values);
                Some(state.finish_owned_result(result))
            }
        }
        InstrCodegen::SetAttr(op) => {
            if let Some(value) = emit_specialized_setattr(op, state) {
                Some(value)
            } else {
                let instr_id = Some(op.semantic_instr_id());
                let arg_values = state.emit_arg_values(&[
                    op.value.as_ref(),
                    op.attr.as_ref(),
                    op.replacement.as_ref(),
                ]);
                let result = emit_setattr_fallback(state, instr_id, &arg_values);
                state.release_arg_values(&arg_values);
                Some(state.finish_owned_result(result))
            }
        }
        InstrCodegen::GetItem(op) => Some(operation_specializations::emit_getitem(op, state)),
        InstrCodegen::SetItem(op) => Some(operation_specializations::emit_setitem(op, state)),
        InstrCodegen::DelItem(op) => Some(emit_positional_owned_call(
            &DP_JIT_PYOBJECT_DELITEM_IMPORT,
            state,
            &[op.value.as_ref(), op.index.as_ref()],
        )),
        InstrCodegen::Load(op) => (op.name.location.is_global()
            || op.name.location.is_runtime_name())
        .then(|| emit_load(op, state)),
        InstrCodegen::MakeCell(op) => Some(emit_make_cell(state, op.initial_value.as_deref())),
        InstrCodegen::IncrementCounter(_) => None,
        InstrCodegen::CellRef(_) => None,
        InstrCodegen::MakeFunctionWithClosure(_) => None,
        InstrCodegen::Store(op) => op.name.location.is_global().then(|| emit_store(op, state)),
        InstrCodegen::Del(op) => op.name.location.is_global().then(|| emit_del(op, state)),
    }
}

pub(super) fn emit_typed_operation<'fb>(
    operation: &InstrTyped,
    state: &mut impl OperationEmitState<'fb, InstrTyped>,
) -> Option<ir::Value> {
    match operation {
        InstrTyped::BinOp(op) => emit_specialized_binop(op, state).or_else(|| {
            Some(emit_binop(
                op.kind,
                state,
                &[op.left.as_ref(), op.right.as_ref()],
            ))
        }),
        InstrTyped::LegacyUnaryOp(op) => emit_specialized_unary_op(op, state)
            .or_else(|| Some(emit_unary_op(op.kind, state, &[op.operand.as_ref()]))),
        InstrTyped::LegacyStore(op) => op.name.location.is_global().then(|| emit_store(op, state)),
        _ => None,
    }
}

pub(super) fn emit_typed_i32_bool01_operation<'fb>(
    operation: &InstrTyped,
    state: &mut impl OperationEmitState<'fb, InstrTyped>,
) -> Option<ir::Value> {
    match operation {
        InstrTyped::BinOp(op) => emit_specialized_binop_i32_bool01(op, state),
        _ => None,
    }
}
