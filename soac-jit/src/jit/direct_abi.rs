#![allow(dead_code)]
//! Staged descriptor surface for typed direct-call ABIs. The first consumer is
//! runtime builtins, but the shape intentionally includes regular Python
//! function entries so the later direct-call migration uses the same model.

use super::typed_value::ValueOwnership;
use soac_blockpy::block_py::FunctionId;
use soac_blockpy::passes::PyExactType;

pub(super) const SOAC_RUNTIME_BUILTIN_ORD_I64_SYMBOL: &str = "soac_runtime_builtin_ord_i64";
pub(super) const SOAC_RUNTIME_BUILTIN_CHR_I64_SYMBOL: &str = "soac_runtime_builtin_chr_i64";
pub(super) const SOAC_RUNTIME_BUILTIN_LEN_I64_SYMBOL: &str = "soac_runtime_builtin_len_i64";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum DirectTargetId {
    PythonFunction(FunctionId),
    RuntimePrimitive(RuntimePrimitiveId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum RuntimePrimitiveId {
    BuiltinOrdI64,
    BuiltinChrI64,
    BuiltinLenI64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DirectEntry {
    ProcessJitPythonFunction,
    RuntimeSymbol(&'static str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HiddenArgAbi {
    FunctionEnv,
    ThreadState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ArgOwnership {
    BorrowedOk,
    OwnedConsumed,
    OwnedPreserved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ParamAbi {
    PyObject {
        ownership: ArgOwnership,
    },
    I64 {
        py_long_coercion: Option<PyLongI64Coercion>,
    },
    I32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PyLongI64Coercion {
    Saturating,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResultAbi {
    PyObject {
        ownership: ValueOwnership,
        exact_type: Option<PyExactType>,
    },
    I64,
    I32,
    NoValue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ErrorAbi {
    CannotRaise,
    CurrentException,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DirectCallAbi {
    pub hidden_args: &'static [HiddenArgAbi],
    pub params: &'static [ParamAbi],
    pub result: ResultAbi,
    pub error: ErrorAbi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DirectCallCost {
    pub runtime: u16,
    pub code_size: u16,
}

impl DirectCallCost {
    pub const fn new(runtime: u16, code_size: u16) -> Self {
        Self { runtime, code_size }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DirectCallableDesc {
    pub target: DirectTargetId,
    pub entry: DirectEntry,
    pub abi: DirectCallAbi,
    pub cost: DirectCallCost,
}

const TSTATE_HIDDEN_ARGS: &[HiddenArgAbi] = &[HiddenArgAbi::ThreadState];
const ORD_PARAMS: &[ParamAbi] = &[ParamAbi::PyObject {
    ownership: ArgOwnership::BorrowedOk,
}];
const LEN_PARAMS: &[ParamAbi] = ORD_PARAMS;
const CHR_PARAMS: &[ParamAbi] = &[ParamAbi::I64 {
    py_long_coercion: Some(PyLongI64Coercion::Saturating),
}];

pub(super) const BUILTIN_ORD_I64_DESC: DirectCallableDesc = DirectCallableDesc {
    target: DirectTargetId::RuntimePrimitive(RuntimePrimitiveId::BuiltinOrdI64),
    entry: DirectEntry::RuntimeSymbol(SOAC_RUNTIME_BUILTIN_ORD_I64_SYMBOL),
    abi: DirectCallAbi {
        hidden_args: TSTATE_HIDDEN_ARGS,
        params: ORD_PARAMS,
        result: ResultAbi::I64,
        error: ErrorAbi::CurrentException,
    },
    cost: DirectCallCost::new(8, 4),
};

pub(super) const BUILTIN_CHR_I64_DESC: DirectCallableDesc = DirectCallableDesc {
    target: DirectTargetId::RuntimePrimitive(RuntimePrimitiveId::BuiltinChrI64),
    entry: DirectEntry::RuntimeSymbol(SOAC_RUNTIME_BUILTIN_CHR_I64_SYMBOL),
    abi: DirectCallAbi {
        hidden_args: TSTATE_HIDDEN_ARGS,
        params: CHR_PARAMS,
        result: ResultAbi::PyObject {
            ownership: ValueOwnership::Owned,
            exact_type: Some(PyExactType::Str),
        },
        error: ErrorAbi::CurrentException,
    },
    cost: DirectCallCost::new(10, 5),
};

pub(super) const BUILTIN_LEN_I64_DESC: DirectCallableDesc = DirectCallableDesc {
    target: DirectTargetId::RuntimePrimitive(RuntimePrimitiveId::BuiltinLenI64),
    entry: DirectEntry::RuntimeSymbol(SOAC_RUNTIME_BUILTIN_LEN_I64_SYMBOL),
    abi: DirectCallAbi {
        hidden_args: TSTATE_HIDDEN_ARGS,
        params: LEN_PARAMS,
        result: ResultAbi::I64,
        error: ErrorAbi::CurrentException,
    },
    cost: DirectCallCost::new(8, 4),
};

pub(super) fn runtime_primitive_desc(primitive: RuntimePrimitiveId) -> &'static DirectCallableDesc {
    match primitive {
        RuntimePrimitiveId::BuiltinOrdI64 => &BUILTIN_ORD_I64_DESC,
        RuntimePrimitiveId::BuiltinChrI64 => &BUILTIN_CHR_I64_DESC,
        RuntimePrimitiveId::BuiltinLenI64 => &BUILTIN_LEN_I64_DESC,
    }
}

pub(super) fn runtime_primitive_for_builtin_name(name: &str) -> Option<RuntimePrimitiveId> {
    match name {
        "ord" => Some(RuntimePrimitiveId::BuiltinOrdI64),
        "chr" => Some(RuntimePrimitiveId::BuiltinChrI64),
        "len" => Some(RuntimePrimitiveId::BuiltinLenI64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ord_descriptor_accepts_borrowed_pyobject_and_returns_i64() {
        let desc = runtime_primitive_desc(RuntimePrimitiveId::BuiltinOrdI64);
        assert_eq!(
            desc.entry,
            DirectEntry::RuntimeSymbol(SOAC_RUNTIME_BUILTIN_ORD_I64_SYMBOL)
        );
        assert_eq!(desc.abi.hidden_args, &[HiddenArgAbi::ThreadState]);
        assert_eq!(
            desc.abi.params,
            &[ParamAbi::PyObject {
                ownership: ArgOwnership::BorrowedOk
            }]
        );
        assert_eq!(desc.abi.result, ResultAbi::I64);
        assert_eq!(desc.abi.error, ErrorAbi::CurrentException);
    }

    #[test]
    fn chr_descriptor_accepts_i64_and_returns_owned_pyobject() {
        let desc = runtime_primitive_desc(RuntimePrimitiveId::BuiltinChrI64);
        assert_eq!(
            desc.entry,
            DirectEntry::RuntimeSymbol(SOAC_RUNTIME_BUILTIN_CHR_I64_SYMBOL)
        );
        assert_eq!(desc.abi.hidden_args, &[HiddenArgAbi::ThreadState]);
        assert_eq!(
            desc.abi.params,
            &[ParamAbi::I64 {
                py_long_coercion: Some(PyLongI64Coercion::Saturating)
            }]
        );
        assert_eq!(
            desc.abi.result,
            ResultAbi::PyObject {
                ownership: ValueOwnership::Owned,
                exact_type: Some(PyExactType::Str)
            }
        );
        assert_eq!(desc.abi.error, ErrorAbi::CurrentException);
    }

    #[test]
    fn len_descriptor_accepts_borrowed_pyobject_and_returns_i64() {
        let desc = runtime_primitive_desc(RuntimePrimitiveId::BuiltinLenI64);
        assert_eq!(
            desc.entry,
            DirectEntry::RuntimeSymbol(SOAC_RUNTIME_BUILTIN_LEN_I64_SYMBOL)
        );
        assert_eq!(desc.abi.hidden_args, &[HiddenArgAbi::ThreadState]);
        assert_eq!(
            desc.abi.params,
            &[ParamAbi::PyObject {
                ownership: ArgOwnership::BorrowedOk
            }]
        );
        assert_eq!(desc.abi.result, ResultAbi::I64);
        assert_eq!(desc.abi.error, ErrorAbi::CurrentException);
    }

    #[test]
    fn builtin_name_lookup_maps_ord_and_chr_to_runtime_primitives() {
        assert_eq!(
            runtime_primitive_for_builtin_name("ord"),
            Some(RuntimePrimitiveId::BuiltinOrdI64)
        );
        assert_eq!(
            runtime_primitive_for_builtin_name("chr"),
            Some(RuntimePrimitiveId::BuiltinChrI64)
        );
        assert_eq!(
            runtime_primitive_for_builtin_name("len"),
            Some(RuntimePrimitiveId::BuiltinLenI64)
        );
        assert_eq!(runtime_primitive_for_builtin_name("sum"), None);
    }
}
