use super::SpecializationProfile;
use super::codegen_env::{
    JitCodegenEnv, declare_import_fn, declare_local_fn, lower_static_signature,
};
use super::direct_abi;
use super::intrinsics;
use super::module_data::declare_type_ptr_import;
use super::operation_specializations::field_index_specialization_from_opt_v3;
use super::symbols::RelocCallableRef;
#[cfg(test)]
use super::symbols::reloc_type_ref_from_typed_attr_owner_ref;
use super::symbols::{
    CpythonTypeSymbol, RelocTypeRef, SOAC_RUNTIME_COMPARE_COMPACT_ASCII_UNICODE_SYMBOL,
    SOAC_RUNTIME_DECREF_APPLIED_SYMBOL, SOAC_RUNTIME_DECREF_DEALLOC_PRESERVING_ERROR_SYMBOL,
    SOAC_RUNTIME_DECREF_SYMBOL, SOAC_RUNTIME_INCREF_APPLIED_SYMBOL, SOAC_RUNTIME_INCREF_SYMBOL,
    SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL, SOAC_RUNTIME_PROBE_FIELD_INDEXED_INLINE_VALUES_SYMBOL,
    SOAC_RUNTIME_PROBE_FIELD_INDEXED_SYMBOL, SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL,
    SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_SYMBOL, SOAC_RUNTIME_SET_RAISED_EXCEPTION_SYMBOL,
    SOAC_RUNTIME_STORE_FIELD_INDEXED_INLINE_VALUES_SYMBOL, SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL,
    SOAC_RUNTIME_STORE_GLOBAL_INDEXED_STOLEN_SYMBOL, SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL,
    SOAC_RUNTIME_STORE_GLOBAL_SYMBOL, SOAC_RUNTIME_TUPLE_NEW_SYMBOL,
    SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_SYMBOL, cpython_type_symbol_name,
    ensure_reloc_callable_symbol_registered, ensure_reloc_type_symbol_registered,
    reloc_callable_ref_symbol_name, reloc_type_ref_for_type, reloc_type_ref_symbol_name,
};
use crate::function_instantiation::SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL;
use crate::strict_annotation::{
    CHECK_ANNOTATION_FORMAT_SYMBOL, CONSTRUCT_TYPE_PARAMETER_SCOPE_SYMBOL,
    CREATE_TYPE_ALIAS_SYMBOL, CREATE_TYPE_PARAMETER_SYMBOL, NEW_ANNOTATION_SET_SYMBOL,
    RECORD_ANNOTATION_SYMBOL, SET_FUNCTION_TYPE_PARAMETERS_SYMBOL,
    SET_TYPE_PARAMETER_DEFAULT_SYMBOL, SETUP_ANNOTATIONS_SYMBOL, SUBSCRIPT_GENERIC_SYMBOL,
};
use crate::strict_class::CONSTRUCT_CLASS_SYMBOL;
use crate::strict_function::COMPLETE_FUNCTION_DEFINITION_SYMBOL;
use cranelift_jit::JITModule;
use cranelift_module::{FuncId, Linkage};
#[cfg(test)]
use soac_core::block_py::{BlockPyFunction, ChildVisitable, Visit};
use soac_ir_typed::plan_v3::DirectCallCallee;
#[cfg(test)]
use soac_ir_typed::{
    InstrTyped, TypedBlockPyModuleShape, TypedCallAccessPlan, TypedDirectCallableCallGuard,
    TypedDirectMethodCallGuard,
};
use soac_opt::access_emission_v3::indexed_field_runtime_access_request as opt_v3_indexed_field_runtime_access_request;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_IMPORT_SPEC_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug)]
pub(super) enum SigType {
    Pointer,
    I64,
    I32,
    F64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct StaticSignature {
    pub(super) params: &'static [SigType],
    pub(super) returns: &'static [SigType],
}

impl StaticSignature {
    const fn new(params: &'static [SigType], returns: &'static [SigType]) -> Self {
        Self { params, returns }
    }
}

#[derive(Debug)]
pub(super) struct ImportSpec {
    pub(super) symbol: &'static str,
    pub(super) signature: StaticSignature,
    pub(super) linkage: Linkage,
    internal_id: OnceLock<usize>,
}

impl ImportSpec {
    pub(super) const fn new(
        symbol: &'static str,
        params: &'static [SigType],
        returns: &'static [SigType],
    ) -> Self {
        Self {
            symbol,
            signature: StaticSignature::new(params, returns),
            linkage: Linkage::Import,
            internal_id: OnceLock::new(),
        }
    }

    const fn local(
        symbol: &'static str,
        params: &'static [SigType],
        returns: &'static [SigType],
    ) -> Self {
        Self {
            symbol,
            signature: StaticSignature::new(params, returns),
            linkage: Linkage::Local,
            internal_id: OnceLock::new(),
        }
    }

    pub(super) fn internal_id(&'static self) -> usize {
        *self
            .internal_id
            .get_or_init(|| NEXT_IMPORT_SPEC_ID.fetch_add(1, Ordering::Relaxed))
    }
}

pub(super) static DP_JIT_INCREF_IMPORT: ImportSpec =
    ImportSpec::local(SOAC_RUNTIME_INCREF_SYMBOL, &[SigType::Pointer], &[]);
pub(super) static DP_JIT_DECREF_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_DECREF_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[],
);
pub(super) static DP_JIT_DECREF_DEALLOC_PRESERVING_ERROR_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_DECREF_DEALLOC_PRESERVING_ERROR_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[],
);
pub(super) static SOAC_RUNTIME_INCREF_APPLIED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_INCREF_APPLIED_SYMBOL,
    &[SigType::Pointer],
    &[SigType::I32],
);
pub(super) static SOAC_RUNTIME_DECREF_APPLIED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_DECREF_APPLIED_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);
pub(super) static SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_SET_RAISED_EXCEPTION_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[],
);
pub(super) static SOAC_RUNTIME_LOAD_GLOBAL_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_LOAD_GLOBAL_SYMBOL,
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL,
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_LOAD_GLOBAL_SLOW_IMPORT: ImportSpec = ImportSpec::new(
    "soac_runtime_load_global_slow",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_STORE_GLOBAL_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_STORE_GLOBAL_SYMBOL,
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_STORE_GLOBAL_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL,
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_STORE_GLOBAL_INDEXED_STOLEN_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_STORE_GLOBAL_INDEXED_STOLEN_SYMBOL,
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_PROBE_FIELD_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_PROBE_FIELD_INDEXED_SYMBOL,
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_PROBE_FIELD_INDEXED_INLINE_VALUES_IMPORT: ImportSpec =
    ImportSpec::local(
        SOAC_RUNTIME_PROBE_FIELD_INDEXED_INLINE_VALUES_SYMBOL,
        &[SigType::Pointer, SigType::Pointer, SigType::I64],
        &[SigType::Pointer],
    );
pub(super) static SOAC_RUNTIME_STORE_FIELD_INDEXED_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_STORE_FIELD_INDEXED_SYMBOL,
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
    ],
    &[SigType::I32],
);
pub(super) static SOAC_RUNTIME_STORE_FIELD_INDEXED_INLINE_VALUES_IMPORT: ImportSpec =
    ImportSpec::local(
        SOAC_RUNTIME_STORE_FIELD_INDEXED_INLINE_VALUES_SYMBOL,
        &[
            SigType::Pointer,
            SigType::Pointer,
            SigType::Pointer,
            SigType::I64,
            SigType::Pointer,
        ],
        &[SigType::I32],
    );
pub(super) static SOAC_RUNTIME_COMPARE_COMPACT_ASCII_UNICODE_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_COMPARE_COMPACT_ASCII_UNICODE_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);
pub(super) static SOAC_RUNTIME_BUILTIN_ORD_I64_IMPORT: ImportSpec = ImportSpec::local(
    direct_abi::SOAC_RUNTIME_BUILTIN_ORD_I64_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I64],
);
pub(super) static SOAC_RUNTIME_BUILTIN_CHR_I64_IMPORT: ImportSpec = ImportSpec::local(
    direct_abi::SOAC_RUNTIME_BUILTIN_CHR_I64_SYMBOL,
    &[SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_BUILTIN_LEN_I64_IMPORT: ImportSpec = ImportSpec::local(
    direct_abi::SOAC_RUNTIME_BUILTIN_LEN_I64_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I64],
);
pub(super) static SOAC_RUNTIME_BUILTIN_ITER_OBJECT_IMPORT: ImportSpec = ImportSpec::local(
    direct_abi::SOAC_RUNTIME_BUILTIN_ITER_OBJECT_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_UNPACK_FIXED_IMPORT: ImportSpec = ImportSpec::local(
    direct_abi::SOAC_RUNTIME_UNPACK_FIXED_SYMBOL,
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_SYMBOL,
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I64],
);
pub(super) static DP_JIT_RAISE_I64_OVERFLOW_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_raise_i64_overflow", &[], &[]);
pub(super) static DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_py_call_positional_three",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_PY_CALL_OBJECT_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_py_call_object",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static PY_SOAC_VECTORCALL_CONTEXT_IMPORT: ImportSpec = ImportSpec::new(
    "PySoac_VectorcallWithContext",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static PY_SOAC_OBJECT_CALL_CONTEXT_IMPORT: ImportSpec = ImportSpec::new(
    "PySoac_ObjectCallWithContext",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_MAKE_GENERATOR_INSTANCE_FROM_VECTORCALL_IMPORT: ImportSpec =
    ImportSpec::new(
        "dp_jit_make_generator_instance_from_vectorcall",
        &[
            SigType::Pointer,
            SigType::Pointer,
            SigType::Pointer,
            SigType::Pointer,
        ],
        &[SigType::Pointer],
    );
pub(super) static DP_JIT_ENTER_RECURSIVE_CALL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_enter_recursive_call",
    &[SigType::Pointer],
    &[SigType::I32],
);
pub(super) static PY_THREAD_STATE_GET_UNCHECKED_IMPORT: ImportSpec =
    ImportSpec::new("PyThreadState_GetUnchecked", &[], &[SigType::Pointer]);
pub(super) static PY_HANDLE_PENDING_IMPORT: ImportSpec =
    ImportSpec::new("_Py_HandlePending", &[SigType::Pointer], &[SigType::I32]);
static DP_JIT_LOAD_RUNTIME_OBJ_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_load_runtime_obj",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_LOAD_RUNTIME_OBJ_BY_ID_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_load_runtime_obj_by_id",
    &[SigType::I64],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_PYOBJECT_GETATTR_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_getattr",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static PYOBJECT_GETMETHOD_IMPORT: ImportSpec = ImportSpec::new(
    "_PyObject_GetMethod",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);
pub(super) static DP_JIT_PYOBJECT_SETATTR_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_setattr",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_MATCH_SEALED_FIELD_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_match_sealed_field_capability",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);
pub(super) static DP_JIT_RESOLVE_SEALED_VIRTUAL_METHOD_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_resolve_sealed_virtual_method_capability",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);
pub(super) static SOAC_RUNTIME_PROBE_SEALED_FIELD_IMPORT: ImportSpec = ImportSpec::local(
    "soac_runtime_probe_stable_indexed_field",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::I64,
        SigType::I64,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_LOAD_NATIVE_OBJECT_SLOT_IMPORT: ImportSpec = ImportSpec::local(
    "soac_runtime_load_native_object_slot",
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_PYOBJECT_GETITEM_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_getitem",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_PYOBJECT_SETITEM_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_setitem",
    &[SigType::Pointer, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_PRESERVED_VALUES_PTR_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_preserved_values_ptr",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_DEL_PRESERVED_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_del_preserved",
    &[SigType::Pointer, SigType::I64, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_DEL_PRESERVED_QUIETLY_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_del_preserved_quietly",
    &[SigType::Pointer, SigType::I64, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_PYTYPE_GENERIC_ALLOC_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pytype_generic_alloc",
    &[SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_FINISH_CONSTRUCTOR_INIT_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_finish_constructor_init",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_PYOBJECT_TO_I64_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_pyobject_to_i64",
    &[SigType::Pointer],
    &[SigType::I64],
);
pub(super) static PYNUMBER_ADD_IMPORT: ImportSpec = ImportSpec::new(
    "PyNumber_Add",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static PYNUMBER_SUBTRACT_IMPORT: ImportSpec = ImportSpec::new(
    "PyNumber_Subtract",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static PYNUMBER_MULTIPLY_IMPORT: ImportSpec = ImportSpec::new(
    "PyNumber_Multiply",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static PYNUMBER_AND_IMPORT: ImportSpec = ImportSpec::new(
    "PyNumber_And",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static PYNUMBER_OR_IMPORT: ImportSpec = ImportSpec::new(
    "PyNumber_Or",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static PYNUMBER_XOR_IMPORT: ImportSpec = ImportSpec::new(
    "PyNumber_Xor",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static PYOBJECT_RICHCOMPARE_IMPORT: ImportSpec = ImportSpec::new(
    "PyObject_RichCompare",
    &[SigType::Pointer, SigType::Pointer, SigType::I32],
    &[SigType::Pointer],
);
pub(super) static PYOBJECT_RICHCOMPARE_BOOL_IMPORT: ImportSpec = ImportSpec::new(
    "PyObject_RichCompareBool",
    &[SigType::Pointer, SigType::Pointer, SigType::I32],
    &[SigType::I32],
);
pub(super) static PYUNICODE_COMPARE_IMPORT: ImportSpec = ImportSpec::new(
    "PyUnicode_Compare",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);
pub(super) static PYLONG_FROM_LONGLONG_IMPORT: ImportSpec =
    ImportSpec::new("PyLong_FromLongLong", &[SigType::I64], &[SigType::Pointer]);
pub(super) static DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_record_top_value_sample",
    &[SigType::Pointer, SigType::I64],
    &[],
);
pub(super) static DP_JIT_PROFILE_CALLABLE_FUNCTION_ID_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_profile_callable_function_id",
    &[SigType::Pointer],
    &[SigType::I64],
);
pub(super) static DP_JIT_PROTOCOL_NEXT_FUNCTION_ID_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_protocol_next_function_id",
    &[SigType::Pointer],
    &[SigType::I64],
);
pub(super) static DP_JIT_PROTOCOL_ITER_FUNCTION_ID_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_protocol_iter_function_id",
    &[SigType::Pointer],
    &[SigType::I64],
);
pub(super) static DP_JIT_RAISE_UNBOUND_LOCAL_ERROR_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_raise_unbound_local_error", &[SigType::Pointer], &[]);
pub(super) static DP_JIT_RAISE_MISSING_REQUIRED_ARGUMENT_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_raise_missing_required_argument", &[], &[]);
pub(super) static DP_JIT_RAISE_SUPER_ARG_DELETED_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_raise_super_arg_deleted", &[], &[]);
pub(super) static DP_JIT_MAKE_CELL_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_make_cell", &[SigType::Pointer], &[SigType::Pointer]);
pub(super) static DP_JIT_LOAD_CELL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_load_cell",
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_STORE_CELL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_store_cell",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_TUPLE_NEW_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_TUPLE_NEW_SYMBOL,
    &[SigType::I64],
    &[SigType::Pointer],
);
pub(super) static SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_IMPORT: ImportSpec = ImportSpec::local(
    SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_SYMBOL,
    &[SigType::Pointer, SigType::I64, SigType::Pointer],
    &[],
);
pub(super) static DP_JIT_IS_TRUE_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_is_true", &[SigType::Pointer], &[SigType::I32]);
pub(super) static DP_JIT_RAISE_FROM_EXC_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_raise_from_exc",
    &[SigType::Pointer],
    &[SigType::I32],
);
pub(super) static DP_JIT_RESTORE_RAISED_EXCEPTION_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_restore_raised_exception",
    &[SigType::Pointer],
    &[SigType::I32],
);
pub(super) static DP_JIT_GENERATOR_RETURN_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_generator_return",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_HANDLED_STATE_INIT_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_handled_state_init",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_HANDLED_STATE_SELECT_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_handled_state_select",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::I64,
    ],
    &[SigType::I32],
);
pub(super) static DP_JIT_HANDLED_STATE_RAISED_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_handled_state_raised",
    &[SigType::Pointer, SigType::I64],
    &[],
);
pub(super) static DP_JIT_HANDLED_STATE_FINISH_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_handled_state_finish",
    &[SigType::Pointer, SigType::I64, SigType::Pointer],
    &[],
);
pub(super) static DP_JIT_HANDLED_STATE_RELEASE_RESIDUAL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_handled_state_release_residual",
    &[SigType::Pointer],
    &[],
);
pub(super) static DP_JIT_RETIRE_TERMINAL_ROOTS_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_retire_terminal_roots",
    &[SigType::Pointer],
    &[SigType::I32],
);
pub(super) static DP_JIT_RERAISE_CURRENT_IMPORT: ImportSpec =
    ImportSpec::new("dp_jit_reraise_current", &[], &[]);
pub(super) static DP_JIT_DEOPT_RESUME_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_deopt_resume",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_CHECKED_FUNCTION_METADATA_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_checked_function_metadata",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_VECTORCALL_BIND_DIRECT_ARGS_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_vectorcall_bind_direct_args",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::I64,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_VECTORCALL_COMPILE_FUNCTION_ENV_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_vectorcall_compile_function_env",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_STRICT_FINISH_CALL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_strict_finish_call",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_PREPARE_STRICT_DIRECT_CALL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_prepare_strict_direct_call",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::I32],
);
pub(super) static DP_JIT_FINISH_STRICT_DIRECT_CALL_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_finish_strict_direct_call",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_RETIRE_STRICT_CALL_ARGUMENTS_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_retire_strict_call_arguments",
    &[SigType::Pointer],
    &[],
);
pub(super) static DP_JIT_VECTORCALL_PREVIOUS_FOR_CHANGED_CODE_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_vectorcall_previous_for_changed_code",
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static DP_JIT_DIRECT_COMPILE_FUNCTION_ENV_IMPORT: ImportSpec = ImportSpec::new(
    "dp_jit_direct_compile_function_env",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_IMPORT: ImportSpec = ImportSpec::new(
    SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_SYMBOL,
    &[
        SigType::I64,
        SigType::I64,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_CONSTRUCT_CLASS_IMPORT: ImportSpec = ImportSpec::new(
    CONSTRUCT_CLASS_SYMBOL,
    &[
        SigType::I64,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_PREPARE_CLASS_DECORATOR_IMPORT: ImportSpec = ImportSpec::new(
    crate::strict_class_decorator::PREPARE_CLASS_DECORATOR_SYMBOL,
    &[
        SigType::I64,
        SigType::Pointer,
        SigType::I32,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_PREPARE_CLASS_DECORATOR_UNPACKED_IMPORT: ImportSpec = ImportSpec::new(
    crate::strict_class_decorator::PREPARE_CLASS_DECORATOR_UNPACKED_SYMBOL,
    &[
        SigType::I64,
        SigType::Pointer,
        SigType::I32,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_APPLY_CLASS_DECORATOR_IMPORT: ImportSpec = ImportSpec::new(
    crate::strict_class_decorator::APPLY_CLASS_DECORATOR_SYMBOL,
    &[
        SigType::I64,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_DISCARD_CLASS_DECORATOR_IMPORT: ImportSpec = ImportSpec::new(
    crate::strict_class_decorator::DISCARD_CLASS_DECORATOR_SYMBOL,
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_DISCARD_CLASS_CONSTRUCTION_CAPTURES_IMPORT: ImportSpec = ImportSpec::new(
    crate::strict_function::DISCARD_CLASS_CONSTRUCTION_CAPTURES_SYMBOL,
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_APPLY_FUNCTION_DESCRIPTOR_IMPORT: ImportSpec = ImportSpec::new(
    crate::strict_descriptor::APPLY_FUNCTION_DESCRIPTOR_SYMBOL,
    &[
        SigType::I64,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_NEW_ANNOTATION_SET_IMPORT: ImportSpec =
    ImportSpec::new(NEW_ANNOTATION_SET_SYMBOL, &[], &[SigType::Pointer]);
pub(super) static SOAC_JIT_SETUP_ANNOTATIONS_IMPORT: ImportSpec = ImportSpec::new(
    SETUP_ANNOTATIONS_SYMBOL,
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_CREATE_TYPE_ALIAS_IMPORT: ImportSpec = ImportSpec::new(
    CREATE_TYPE_ALIAS_SYMBOL,
    &[
        SigType::I64,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_CONSTRUCT_TYPE_PARAMETER_SCOPE_IMPORT: ImportSpec = ImportSpec::new(
    CONSTRUCT_TYPE_PARAMETER_SCOPE_SYMBOL,
    &[
        SigType::I64,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_SUBSCRIPT_GENERIC_IMPORT: ImportSpec = ImportSpec::new(
    SUBSCRIPT_GENERIC_SYMBOL,
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_SET_FUNCTION_TYPE_PARAMETERS_IMPORT: ImportSpec = ImportSpec::new(
    SET_FUNCTION_TYPE_PARAMETERS_SYMBOL,
    &[
        SigType::I64,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_CREATE_TYPE_PARAMETER_IMPORT: ImportSpec = ImportSpec::new(
    CREATE_TYPE_PARAMETER_SYMBOL,
    &[
        SigType::I64,
        SigType::I32,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_SET_TYPE_PARAMETER_DEFAULT_IMPORT: ImportSpec = ImportSpec::new(
    SET_TYPE_PARAMETER_DEFAULT_SYMBOL,
    &[
        SigType::I64,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_COMPLETE_FUNCTION_DEFINITION_IMPORT: ImportSpec = ImportSpec::new(
    COMPLETE_FUNCTION_DEFINITION_SYMBOL,
    &[SigType::I64, SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_RECORD_ANNOTATION_IMPORT: ImportSpec = ImportSpec::new(
    RECORD_ANNOTATION_SYMBOL,
    &[SigType::Pointer, SigType::I32],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_CHECK_ANNOTATION_FORMAT_IMPORT: ImportSpec = ImportSpec::new(
    CHECK_ANNOTATION_FORMAT_SYMBOL,
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static SOAC_JIT_RESUME_GENERATOR_IMPORT: ImportSpec = ImportSpec::new(
    direct_abi::SOAC_JIT_RESUME_GENERATOR_SYMBOL,
    &[
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::Pointer],
);

static JIT_RUNTIME_IMPORT_SPECS: &[&ImportSpec] = &[
    &DP_JIT_INCREF_IMPORT,
    &DP_JIT_DECREF_IMPORT,
    &SOAC_RUNTIME_INCREF_APPLIED_IMPORT,
    &SOAC_RUNTIME_DECREF_APPLIED_IMPORT,
    &SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT,
    &SOAC_RUNTIME_LOAD_GLOBAL_IMPORT,
    &SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_IMPORT,
    &SOAC_RUNTIME_LOAD_GLOBAL_SLOW_IMPORT,
    &SOAC_RUNTIME_STORE_GLOBAL_IMPORT,
    &SOAC_RUNTIME_STORE_GLOBAL_INDEXED_IMPORT,
    &SOAC_RUNTIME_STORE_GLOBAL_INDEXED_STOLEN_IMPORT,
    &SOAC_RUNTIME_PROBE_FIELD_INDEXED_IMPORT,
    &SOAC_RUNTIME_PROBE_FIELD_INDEXED_INLINE_VALUES_IMPORT,
    &SOAC_RUNTIME_STORE_FIELD_INDEXED_IMPORT,
    &SOAC_RUNTIME_STORE_FIELD_INDEXED_INLINE_VALUES_IMPORT,
    &SOAC_RUNTIME_COMPARE_COMPACT_ASCII_UNICODE_IMPORT,
    &SOAC_RUNTIME_BUILTIN_ORD_I64_IMPORT,
    &SOAC_RUNTIME_BUILTIN_CHR_I64_IMPORT,
    &SOAC_RUNTIME_BUILTIN_LEN_I64_IMPORT,
    &SOAC_RUNTIME_BUILTIN_ITER_OBJECT_IMPORT,
    &SOAC_RUNTIME_UNPACK_FIXED_IMPORT,
    &SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_IMPORT,
    &DP_JIT_RAISE_I64_OVERFLOW_IMPORT,
    &DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT,
    &DP_JIT_PY_CALL_OBJECT_IMPORT,
    &PY_SOAC_VECTORCALL_CONTEXT_IMPORT,
    &PY_SOAC_OBJECT_CALL_CONTEXT_IMPORT,
    &DP_JIT_MAKE_GENERATOR_INSTANCE_FROM_VECTORCALL_IMPORT,
    &DP_JIT_ENTER_RECURSIVE_CALL_IMPORT,
    &PY_THREAD_STATE_GET_UNCHECKED_IMPORT,
    &PY_HANDLE_PENDING_IMPORT,
    &DP_JIT_LOAD_RUNTIME_OBJ_IMPORT,
    &DP_JIT_LOAD_RUNTIME_OBJ_BY_ID_IMPORT,
    &DP_JIT_PYOBJECT_GETATTR_IMPORT,
    &DP_JIT_MATCH_SEALED_FIELD_IMPORT,
    &DP_JIT_RESOLVE_SEALED_VIRTUAL_METHOD_IMPORT,
    &SOAC_RUNTIME_PROBE_SEALED_FIELD_IMPORT,
    &SOAC_RUNTIME_LOAD_NATIVE_OBJECT_SLOT_IMPORT,
    &PYOBJECT_GETMETHOD_IMPORT,
    &DP_JIT_PYOBJECT_SETATTR_IMPORT,
    &DP_JIT_PYOBJECT_GETITEM_IMPORT,
    &DP_JIT_PYOBJECT_SETITEM_IMPORT,
    &DP_JIT_PRESERVED_VALUES_PTR_IMPORT,
    &DP_JIT_DEL_PRESERVED_IMPORT,
    &DP_JIT_DEL_PRESERVED_QUIETLY_IMPORT,
    &DP_JIT_PYTYPE_GENERIC_ALLOC_IMPORT,
    &DP_JIT_FINISH_CONSTRUCTOR_INIT_IMPORT,
    &DP_JIT_PYOBJECT_TO_I64_IMPORT,
    &PYNUMBER_ADD_IMPORT,
    &PYNUMBER_SUBTRACT_IMPORT,
    &PYNUMBER_MULTIPLY_IMPORT,
    &PYNUMBER_AND_IMPORT,
    &PYNUMBER_OR_IMPORT,
    &PYNUMBER_XOR_IMPORT,
    &PYOBJECT_RICHCOMPARE_IMPORT,
    &PYOBJECT_RICHCOMPARE_BOOL_IMPORT,
    &PYUNICODE_COMPARE_IMPORT,
    &PYLONG_FROM_LONGLONG_IMPORT,
    &DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT,
    &DP_JIT_PROFILE_CALLABLE_FUNCTION_ID_IMPORT,
    &DP_JIT_PROTOCOL_ITER_FUNCTION_ID_IMPORT,
    &DP_JIT_PROTOCOL_NEXT_FUNCTION_ID_IMPORT,
    &DP_JIT_RAISE_UNBOUND_LOCAL_ERROR_IMPORT,
    &DP_JIT_RAISE_MISSING_REQUIRED_ARGUMENT_IMPORT,
    &DP_JIT_RAISE_SUPER_ARG_DELETED_IMPORT,
    &DP_JIT_MAKE_CELL_IMPORT,
    &DP_JIT_LOAD_CELL_IMPORT,
    &DP_JIT_STORE_CELL_IMPORT,
    &SOAC_RUNTIME_TUPLE_NEW_IMPORT,
    &SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_IMPORT,
    &DP_JIT_IS_TRUE_IMPORT,
    &DP_JIT_RAISE_FROM_EXC_IMPORT,
    &DP_JIT_RESTORE_RAISED_EXCEPTION_IMPORT,
    &DP_JIT_GENERATOR_RETURN_IMPORT,
    &DP_JIT_HANDLED_STATE_INIT_IMPORT,
    &DP_JIT_HANDLED_STATE_SELECT_IMPORT,
    &DP_JIT_HANDLED_STATE_RAISED_IMPORT,
    &DP_JIT_HANDLED_STATE_FINISH_IMPORT,
    &DP_JIT_HANDLED_STATE_RELEASE_RESIDUAL_IMPORT,
    &DP_JIT_RETIRE_TERMINAL_ROOTS_IMPORT,
    &DP_JIT_RERAISE_CURRENT_IMPORT,
    &DP_JIT_DEOPT_RESUME_IMPORT,
    &DP_JIT_CHECKED_FUNCTION_METADATA_IMPORT,
    &DP_JIT_VECTORCALL_BIND_DIRECT_ARGS_IMPORT,
    &DP_JIT_VECTORCALL_COMPILE_FUNCTION_ENV_IMPORT,
    &DP_JIT_STRICT_FINISH_CALL_IMPORT,
    &DP_JIT_PREPARE_STRICT_DIRECT_CALL_IMPORT,
    &DP_JIT_FINISH_STRICT_DIRECT_CALL_IMPORT,
    &DP_JIT_RETIRE_STRICT_CALL_ARGUMENTS_IMPORT,
    &DP_JIT_VECTORCALL_PREVIOUS_FOR_CHANGED_CODE_IMPORT,
    &DP_JIT_DIRECT_COMPILE_FUNCTION_ENV_IMPORT,
    &SOAC_JIT_MAKE_FUNCTION_WITH_CLOSURE_IMPORT,
    &SOAC_JIT_CONSTRUCT_CLASS_IMPORT,
    &SOAC_JIT_PREPARE_CLASS_DECORATOR_IMPORT,
    &SOAC_JIT_PREPARE_CLASS_DECORATOR_UNPACKED_IMPORT,
    &SOAC_JIT_APPLY_CLASS_DECORATOR_IMPORT,
    &SOAC_JIT_DISCARD_CLASS_DECORATOR_IMPORT,
    &SOAC_JIT_DISCARD_CLASS_CONSTRUCTION_CAPTURES_IMPORT,
    &SOAC_JIT_COMPLETE_FUNCTION_DEFINITION_IMPORT,
    &SOAC_JIT_APPLY_FUNCTION_DESCRIPTOR_IMPORT,
    &SOAC_JIT_NEW_ANNOTATION_SET_IMPORT,
    &SOAC_JIT_SETUP_ANNOTATIONS_IMPORT,
    &SOAC_JIT_CREATE_TYPE_ALIAS_IMPORT,
    &SOAC_JIT_CONSTRUCT_TYPE_PARAMETER_SCOPE_IMPORT,
    &SOAC_JIT_SUBSCRIPT_GENERIC_IMPORT,
    &SOAC_JIT_SET_FUNCTION_TYPE_PARAMETERS_IMPORT,
    &SOAC_JIT_CREATE_TYPE_PARAMETER_IMPORT,
    &SOAC_JIT_SET_TYPE_PARAMETER_DEFAULT_IMPORT,
    &SOAC_JIT_RECORD_ANNOTATION_IMPORT,
    &SOAC_JIT_CHECK_ANNOTATION_FORMAT_IMPORT,
    &SOAC_JIT_RESUME_GENERATOR_IMPORT,
];

pub(super) struct ModuleFuncImports {
    func_ids_by_internal_id: Vec<Option<FuncId>>,
    import_id_to_symbol: HashMap<u32, &'static str>,
    func_id_to_symbol: HashMap<u32, &'static str>,
}

impl ModuleFuncImports {
    pub(super) fn new() -> Self {
        Self {
            func_ids_by_internal_id: Vec::new(),
            import_id_to_symbol: HashMap::new(),
            func_id_to_symbol: HashMap::new(),
        }
    }

    pub(super) fn debug_symbols(&self) -> &HashMap<u32, &'static str> {
        &self.import_id_to_symbol
    }

    pub(super) fn debug_declared_symbols(&self) -> &HashMap<u32, &'static str> {
        &self.func_id_to_symbol
    }

    pub(super) fn ensure_declared(
        &mut self,
        codegen_env: &mut impl JitCodegenEnv,
        spec: &'static ImportSpec,
    ) -> Result<FuncId, String> {
        let internal_id = spec.internal_id();
        if internal_id >= self.func_ids_by_internal_id.len() {
            self.func_ids_by_internal_id.resize(internal_id + 1, None);
        }
        if let Some(func_id) = self.func_ids_by_internal_id[internal_id] {
            return Ok(func_id);
        }
        let sig = lower_static_signature(codegen_env, spec.signature);
        let func_id = match spec.linkage {
            Linkage::Import => declare_import_fn(codegen_env, spec.symbol, &sig)?,
            Linkage::Local => declare_local_fn(codegen_env, spec.symbol, &sig)?,
            linkage => {
                return Err(format!(
                    "unsupported linkage {linkage:?} for jit call spec {}",
                    spec.symbol
                ));
            }
        };
        self.func_ids_by_internal_id[internal_id] = Some(func_id);
        self.func_id_to_symbol.insert(func_id.as_u32(), spec.symbol);
        if matches!(spec.linkage, Linkage::Import) {
            self.import_id_to_symbol
                .insert(func_id.as_u32(), spec.symbol);
        }
        Ok(func_id)
    }
}

pub(super) fn predeclare_jit_runtime_imports(jit_module: &mut JITModule) -> Result<(), String> {
    let mut imports = ModuleFuncImports::new();
    for spec in JIT_RUNTIME_IMPORT_SPECS
        .iter()
        .chain(intrinsics::OPERATION_IMPORT_SPECS.iter())
    {
        imports.ensure_declared(jit_module, spec)?;
    }
    for (spec, _) in super::native_iterator_runtime::primitive_bindings() {
        imports.ensure_declared(jit_module, spec)?;
    }
    for (spec, _) in super::collection_runtime::primitive_bindings()
        .into_iter()
        .chain(super::call_arguments_runtime::primitive_bindings())
        .chain(super::iteration_runtime::primitive_bindings())
    {
        imports.ensure_declared(jit_module, spec)?;
    }
    for symbol in [
        CpythonTypeSymbol::Function,
        CpythonTypeSymbol::Method,
        CpythonTypeSymbol::Type,
        CpythonTypeSymbol::Long,
        CpythonTypeSymbol::Float,
        CpythonTypeSymbol::List,
        CpythonTypeSymbol::Tuple,
        CpythonTypeSymbol::Unicode,
    ] {
        let _ = declare_type_ptr_import(jit_module, cpython_type_symbol_name(symbol))?;
    }
    Ok(())
}

fn predeclare_reloc_type_ref_import(
    jit_module: &mut JITModule,
    type_ref: &RelocTypeRef,
) -> Result<(), String> {
    if !ensure_reloc_type_symbol_registered(type_ref)? {
        return Ok(());
    }
    let symbol = reloc_type_ref_symbol_name(type_ref);
    let _ = declare_type_ptr_import(jit_module, symbol.as_ref())?;
    Ok(())
}

fn predeclare_reloc_callable_ref_import(
    jit_module: &mut JITModule,
    callable_ref: &RelocCallableRef,
) -> Result<(), String> {
    if !ensure_reloc_callable_symbol_registered(callable_ref)? {
        return Ok(());
    }
    let symbol = reloc_callable_ref_symbol_name(callable_ref);
    let _ = declare_type_ptr_import(jit_module, symbol.as_str())?;
    Ok(())
}

pub(super) fn predeclare_specialization_type_imports(
    jit_module: &mut JITModule,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let opt_v3_planned_fields = profile.opt_v3_indexed_field_access_plans();
    let mut type_refs = HashSet::new();
    for planned in opt_v3_planned_fields {
        let request = opt_v3_indexed_field_runtime_access_request(planned);
        if let Some(specialization) = field_index_specialization_from_opt_v3(&request)? {
            type_refs.insert(specialization.owner_type_ref);
        }
    }
    for type_ref in type_refs {
        predeclare_reloc_type_ref_import(jit_module, &type_ref)?;
    }
    let mut callable_refs = HashSet::new();
    for direct_calls in profile.opt_v3_emitted_direct_calls.values() {
        for plans in direct_calls.values() {
            for plan in plans {
                match &plan.callee {
                    DirectCallCallee::Function => {}
                    DirectCallCallee::Method { method_name }
                    | DirectCallCallee::RuntimeProtocolMethod { method_name, .. } => {
                        let owners = unsafe {
                            crate::lookup_exact_owner_types_for_method(plan.target, method_name)
                        }
                        .map_err(|_| {
                            format!(
                                "failed to resolve owner types for method {} target {}",
                                method_name, plan.target
                            )
                        })?;
                        for owner in owners {
                            let Some(owner_type_ref) = reloc_type_ref_for_type(owner.owner_type)?
                            else {
                                continue;
                            };
                            predeclare_reloc_type_ref_import(jit_module, &owner_type_ref)?;
                            callable_refs.insert(RelocCallableRef::OwnerAttr {
                                owner_type_ref,
                                attr_name: method_name.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
    for callable_ref in callable_refs {
        predeclare_reloc_callable_ref_import(jit_module, &callable_ref)?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn predeclare_typed_direct_call_imports(
    jit_module: &mut JITModule,
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> Result<(), String> {
    struct ImportCollector<'a> {
        jit_module: &'a mut JITModule,
        error: Option<String>,
    }

    impl ImportCollector<'_> {
        fn predeclare_method_guard(
            &mut self,
            method_name: &str,
            guard: &TypedDirectMethodCallGuard,
        ) -> Result<(), String> {
            let Some(owner_type_ref) =
                reloc_type_ref_from_typed_attr_owner_ref(&guard.owner_type_ref)
            else {
                return Ok(());
            };
            predeclare_reloc_type_ref_import(self.jit_module, &owner_type_ref)?;
            predeclare_reloc_callable_ref_import(
                self.jit_module,
                &RelocCallableRef::OwnerAttr {
                    owner_type_ref,
                    attr_name: method_name.to_string(),
                },
            )
        }
    }

    impl Visit<InstrTyped> for ImportCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.error.is_some() {
                return;
            }
            let result = match expr {
                InstrTyped::GuardedCallableCallTyped(_) => Ok(()),
                InstrTyped::GuardedMethodCallTyped(call) => {
                    for guard in &call.method_guards {
                        if let Err(error) =
                            self.predeclare_method_guard(call.method_name.as_str(), guard)
                        {
                            self.error = Some(error);
                            return;
                        }
                    }
                    Ok(())
                }
                InstrTyped::CallTyped(call) => {
                    if let TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                        method_name,
                        method_guards,
                        ..
                    } = &call.access
                    {
                        for guard in method_guards {
                            if let Err(error) =
                                self.predeclare_method_guard(method_name.as_str(), guard)
                            {
                                self.error = Some(error);
                                return;
                            }
                        }
                    }
                    Ok(())
                }
                InstrTyped::DirectCallableCallTyped(call) => match &call.guard {
                    TypedDirectCallableCallGuard::Function(_) => Ok(()),
                },
                InstrTyped::DirectMethodCallTyped(call) => {
                    self.predeclare_method_guard(call.method_name.as_str(), &call.guard)
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                self.error = Some(error);
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut collector = ImportCollector {
        jit_module,
        error: None,
    };
    collector.visit_fn(function);
    if let Some(error) = collector.error {
        Err(error)
    } else {
        Ok(())
    }
}
