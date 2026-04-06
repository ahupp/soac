use pyo3::ffi;
use pyo3::prelude::*;
use soac_blockpy::block_py::{
    AbruptKind, BlockArg, BlockPyFunction, BlockPyModule, BlockTerm, CallArgKeyword,
    ChildVisitable, InstrCodegen, InstrResolved, Literal, NameLike, NumberLiteralValue,
    ParamDefaultSource, operation as blockpy_intrinsics,
};
use soac_blockpy::passes::CodegenModuleShape;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_int};
use std::ptr;

unsafe extern "C" {
    fn _Py_SetImmortal(op: *mut ffi::PyObject);
    fn PyUnstable_IsImmortal(op: *mut ffi::PyObject) -> c_int;
}

const ALWAYS_REQUIRED_UNICODE_CONSTANTS: &[&str] = &[
    "dict",
    "list",
    "raise_from",
    "tuple_from_iter",
    "append",
    "extend",
    "update",
];

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct ModuleConstantId(pub usize);

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum ModuleConstantValue {
    Unicode(Vec<u8>),
    Bytes(Vec<u8>),
    Int(i64),
    BigInt(String),
    FloatBits(u64),
    RuntimeName(Vec<u8>),
}

#[derive(Debug, Clone, Default)]
pub struct ModuleCodegenConstants {
    values: Vec<ModuleConstantValue>,
    ids: HashMap<ModuleConstantValue, ModuleConstantId>,
}

impl ModuleCodegenConstants {
    pub fn collect_from_module(module: &BlockPyModule<CodegenModuleShape>) -> Self {
        let mut collector = ModuleConstantCollector::default();
        for expr in &module.module_constants {
            collector.constants.push_explicit_constant_expr(expr);
        }
        for name in ALWAYS_REQUIRED_UNICODE_CONSTANTS {
            collector.constants.intern_unicode_bytes(name.as_bytes());
        }
        for function in &module.callable_defs {
            collector.collect_function(function);
        }
        collector.constants
    }

    pub fn collect_from_functions<'a>(
        functions: impl IntoIterator<Item = &'a BlockPyFunction<CodegenModuleShape>>,
    ) -> Self {
        let mut collector = ModuleConstantCollector::default();
        for name in ALWAYS_REQUIRED_UNICODE_CONSTANTS {
            collector.constants.intern_unicode_bytes(name.as_bytes());
        }
        for function in functions {
            collector.collect_function(function);
        }
        collector.constants
    }

    pub fn build_python_constants(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        let mut out = Vec::with_capacity(self.values.len());
        for value in &self.values {
            out.push(match value {
                ModuleConstantValue::Unicode(bytes) => build_unicode_constant(py, bytes)?.unbind(),
                ModuleConstantValue::Bytes(bytes) => {
                    let ptr = unsafe {
                        ffi::PyBytes_FromStringAndSize(
                            bytes.as_ptr() as *const i8,
                            bytes.len() as ffi::Py_ssize_t,
                        )
                    };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
                ModuleConstantValue::Int(value) => {
                    let ptr = unsafe { ffi::PyLong_FromLongLong(*value as std::ffi::c_longlong) };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
                ModuleConstantValue::BigInt(value) => {
                    let value = std::ffi::CString::new(value.as_str())
                        .expect("big int literal should not contain NUL");
                    let mut end_ptr = std::ptr::null_mut();
                    let ptr = unsafe { ffi::PyLong_FromString(value.as_ptr(), &mut end_ptr, 0) };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
                ModuleConstantValue::FloatBits(bits) => {
                    let ptr = unsafe { ffi::PyFloat_FromDouble(f64::from_bits(*bits)) };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
                ModuleConstantValue::RuntimeName(bytes) => {
                    let name = build_unicode_constant(py, bytes)?;
                    let ptr = unsafe { load_runtime_name_owned(name.as_ptr()) };
                    let bound: Bound<'_, PyAny> = unsafe { Bound::from_owned_ptr_or_err(py, ptr)? };
                    bound.unbind()
                }
            });
        }
        mark_constants_immortal(&out);
        Ok(out)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn require_unicode_constant_id(&self, value: &str) -> ModuleConstantId {
        self.require_unicode_constant_id_for_bytes(value.as_bytes())
    }

    pub fn require_unicode_constant_id_for_bytes(&self, value: &[u8]) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::Unicode(value.to_vec()))
            .unwrap_or_else(|| {
                panic!(
                    "missing module unicode constant in codegen pool: {:?}",
                    String::from_utf8_lossy(value)
                )
            })
    }

    pub fn require_bytes_constant_id(&self, value: &[u8]) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::Bytes(value.to_vec()))
            .unwrap_or_else(|| panic!("missing module bytes constant in codegen pool"))
    }

    pub fn require_int_constant_id(&self, value: i64) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::Int(value))
            .unwrap_or_else(|| panic!("missing module int constant in codegen pool: {value}"))
    }

    pub fn require_big_int_constant_id(&self, value: &str) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::BigInt(value.to_string()))
            .unwrap_or_else(|| panic!("missing module big-int constant in codegen pool: {value}"))
    }

    pub fn require_float_constant_id(&self, value: f64) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::FloatBits(value.to_bits()))
            .unwrap_or_else(|| panic!("missing module float constant in codegen pool: {value}"))
    }

    pub fn constant_bytes_value(&self, constant_id: ModuleConstantId) -> Option<&[u8]> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::Bytes(bytes) => Some(bytes.as_slice()),
            ModuleConstantValue::Unicode(_)
            | ModuleConstantValue::Int(_)
            | ModuleConstantValue::BigInt(_)
            | ModuleConstantValue::FloatBits(_)
            | ModuleConstantValue::RuntimeName(_) => None,
        }
    }

    pub fn constant_string_bytes_value(&self, constant_id: ModuleConstantId) -> Option<&[u8]> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::Unicode(bytes) | ModuleConstantValue::Bytes(bytes) => {
                Some(bytes.as_slice())
            }
            ModuleConstantValue::Int(_)
            | ModuleConstantValue::BigInt(_)
            | ModuleConstantValue::FloatBits(_)
            | ModuleConstantValue::RuntimeName(_) => None,
        }
    }

    pub fn constant_string_value(&self, constant_id: ModuleConstantId) -> Option<String> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::Unicode(bytes) | ModuleConstantValue::Bytes(bytes) => {
                String::from_utf8(bytes.clone()).ok()
            }
            ModuleConstantValue::Int(_)
            | ModuleConstantValue::BigInt(_)
            | ModuleConstantValue::FloatBits(_)
            | ModuleConstantValue::RuntimeName(_) => None,
        }
    }

    pub fn constant_runtime_name_value(&self, constant_id: ModuleConstantId) -> Option<&str> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::RuntimeName(bytes) => std::str::from_utf8(bytes).ok(),
            ModuleConstantValue::Unicode(_)
            | ModuleConstantValue::Bytes(_)
            | ModuleConstantValue::Int(_)
            | ModuleConstantValue::BigInt(_)
            | ModuleConstantValue::FloatBits(_) => None,
        }
    }

    fn lookup_id(&self, value: &ModuleConstantValue) -> Option<ModuleConstantId> {
        self.ids.get(value).copied()
    }

    fn push_explicit_constant_expr(&mut self, expr: &InstrResolved) -> ModuleConstantId {
        let value = match expr {
            InstrResolved::Literal(literal) => match literal.as_literal() {
                Literal::StringLiteral(string) => {
                    ModuleConstantValue::Unicode(string.value.as_bytes().to_vec())
                }
                Literal::BytesLiteral(bytes) => ModuleConstantValue::Bytes(bytes.value.clone()),
                Literal::NumberLiteral(number) => match &number.value {
                    NumberLiteralValue::Int(value) => {
                        if let Some(value) = value.as_i64() {
                            ModuleConstantValue::Int(value)
                        } else {
                            ModuleConstantValue::BigInt(value.to_string())
                        }
                    }
                    NumberLiteralValue::Float(value) => {
                        ModuleConstantValue::FloatBits(value.to_bits())
                    }
                },
            },
            InstrResolved::Load(op) if op.name.is_runtime_name() => {
                ModuleConstantValue::RuntimeName(op.name.id_str().as_bytes().to_vec())
            }
            _ => {
                panic!(
                    "unsupported explicit module constant expr after codegen lowering: {expr:?}"
                );
            }
        };
        let id = ModuleConstantId(self.values.len());
        self.values.push(value.clone());
        self.ids.entry(value).or_insert(id);
        id
    }

    fn intern(&mut self, value: ModuleConstantValue) -> ModuleConstantId {
        if let Some(existing) = self.ids.get(&value).copied() {
            return existing;
        }
        let id = ModuleConstantId(self.values.len());
        self.values.push(value.clone());
        self.ids.insert(value, id);
        id
    }

    fn intern_unicode_bytes(&mut self, value: &[u8]) -> ModuleConstantId {
        self.intern(ModuleConstantValue::Unicode(value.to_vec()))
    }

    fn intern_int(&mut self, value: i64) -> ModuleConstantId {
        self.intern(ModuleConstantValue::Int(value))
    }
}

fn build_unicode_constant<'py>(py: Python<'py>, bytes: &[u8]) -> PyResult<Bound<'py, PyAny>> {
    let mut ptr = unsafe {
        ffi::PyUnicode_DecodeUTF8(
            bytes.as_ptr() as *const i8,
            bytes.len() as ffi::Py_ssize_t,
            c"surrogatepass".as_ptr(),
        )
    };
    if ptr.is_null() {
        return unsafe { Bound::from_owned_ptr_or_err(py, ptr) };
    }
    unsafe {
        ffi::PyUnicode_InternInPlace(&mut ptr);
        Bound::from_owned_ptr_or_err(py, ptr)
    }
}

fn mark_constants_immortal(constants: &[Py<PyAny>]) {
    for obj in constants {
        unsafe {
            _Py_SetImmortal(obj.as_ptr());
            debug_assert_ne!(PyUnstable_IsImmortal(obj.as_ptr()), 0);
        }
    }
}

pub(crate) unsafe fn raise_name_error_for_missing_name(name_obj: *mut ffi::PyObject) {
    let repr = ffi::PyObject_Repr(name_obj);
    if !repr.is_null() {
        let repr_utf8 = ffi::PyUnicode_AsUTF8(repr);
        if !repr_utf8.is_null() {
            let repr_text = std::ffi::CStr::from_ptr(repr_utf8).to_string_lossy();
            let message = format!("name {repr_text} is not defined");
            ffi::Py_DECREF(repr);
            if let Ok(c_message) = CString::new(message) {
                ffi::PyErr_SetString(ffi::PyExc_NameError, c_message.as_ptr());
                return;
            }
        } else {
            ffi::PyErr_Clear();
        }
        ffi::Py_DECREF(repr);
    } else {
        ffi::PyErr_Clear();
    }
    ffi::PyErr_SetString(ffi::PyExc_NameError, c"name is not defined".as_ptr());
}

pub(crate) unsafe fn load_runtime_name_owned(name_obj: *mut ffi::PyObject) -> *mut ffi::PyObject {
    if name_obj.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"invalid runtime name constant".as_ptr(),
        );
        return ptr::null_mut();
    }
    let runtime_module_name = c"soac.runtime".as_ptr();
    let mut runtime_obj = ptr::null_mut();
    let modules = ffi::PyImport_GetModuleDict();
    if !modules.is_null() {
        runtime_obj = ffi::PyDict_GetItemString(modules, runtime_module_name);
        if !runtime_obj.is_null() {
            ffi::Py_INCREF(runtime_obj);
        }
    }
    if runtime_obj.is_null() {
        runtime_obj = ffi::PyImport_ImportModule(runtime_module_name);
    }
    if runtime_obj.is_null() {
        return ptr::null_mut();
    }
    let runtime_value = ffi::PyObject_GetAttr(runtime_obj, name_obj);
    ffi::Py_DECREF(runtime_obj);
    if !runtime_value.is_null() {
        return runtime_value;
    }
    if ffi::PyErr_ExceptionMatches(ffi::PyExc_AttributeError) == 0 {
        return ptr::null_mut();
    }
    ffi::PyErr_Clear();
    let is_builtins_name = {
        let name_utf8 = ffi::PyUnicode_AsUTF8(name_obj);
        !name_utf8.is_null() && unsafe { CStr::from_ptr(name_utf8) }.to_bytes() == b"builtins"
    };
    if is_builtins_name {
        return ffi::PyImport_ImportModule(c"builtins".as_ptr());
    }
    let builtins_dict = ffi::PyEval_GetBuiltins();
    if builtins_dict.is_null() {
        ffi::PyErr_SetString(
            ffi::PyExc_RuntimeError,
            c"PyEval_GetBuiltins returned null".as_ptr(),
        );
        return ptr::null_mut();
    }
    let builtin_value = ffi::PyObject_GetItem(builtins_dict as *mut ffi::PyObject, name_obj);
    if !builtin_value.is_null() {
        return builtin_value;
    }
    if ffi::PyErr_ExceptionMatches(ffi::PyExc_KeyError) == 0 {
        return ptr::null_mut();
    }
    ffi::PyErr_Clear();
    raise_name_error_for_missing_name(name_obj);
    ptr::null_mut()
}

#[derive(Default)]
struct ModuleConstantCollector {
    constants: ModuleCodegenConstants,
}

impl ModuleConstantCollector {
    fn collect_function(&mut self, function: &BlockPyFunction<CodegenModuleShape>) {
        for (param, default_source) in function.params.iter_with_default_sources() {
            match default_source {
                Some(ParamDefaultSource::Positional(_)) => {
                    self.constants.intern_unicode_bytes(param.name.as_bytes());
                }
                Some(ParamDefaultSource::KeywordOnly(name)) => {
                    self.constants.intern_unicode_bytes(name.as_bytes());
                }
                None => {}
            }
        }
        for block in &function.blocks {
            for stmt in &block.body {
                self.collect_stmt(stmt);
            }
            self.collect_term(&block.term);
        }
    }

    fn collect_stmt(&mut self, stmt: &InstrCodegen) {
        self.collect_expr(stmt);
    }

    fn collect_term(&mut self, term: &BlockTerm<InstrCodegen>) {
        match term {
            BlockTerm::Jump(edge) => self.collect_block_args(&edge.args),
            BlockTerm::IfTerm(if_term) => self.collect_expr(&if_term.test),
            BlockTerm::BranchTable(branch_table) => self.collect_expr(&branch_table.index),
            BlockTerm::Raise(raise_stmt) => {
                if let Some(exc) = &raise_stmt.exc {
                    self.collect_expr(exc);
                }
            }
            BlockTerm::Return(value) => self.collect_expr(value),
        }
    }

    fn collect_block_args(&mut self, args: &[BlockArg]) {
        for arg in args {
            if let BlockArg::AbruptKind(kind) = arg {
                self.constants.intern_int(abrupt_kind_tag(*kind));
            }
        }
    }

    fn collect_expr(&mut self, expr: &InstrCodegen) {
        match expr {
            InstrCodegen::IncrementCounter(_) => {}
            InstrCodegen::CalleeFunctionId(op) => {
                self.collect_expr(op.value.as_ref());
            }
            InstrCodegen::Call(call) => {
                if let Some(const_bytes) = self.string_constant_bytes_for_specialized_codegen(expr)
                {
                    self.constants.intern_unicode_bytes(const_bytes.as_slice());
                }
                if let Some(delete_name_bytes) = self.deleted_name_arg_bytes(call) {
                    self.constants
                        .intern_unicode_bytes(delete_name_bytes.as_slice());
                }
                self.collect_expr(call.func.as_ref());
                for arg in &call.args {
                    self.collect_expr(arg.expr());
                }
                for keyword in &call.keywords {
                    if let CallArgKeyword::Named { arg, .. } = keyword {
                        self.constants.intern_unicode_bytes(arg.as_str().as_bytes());
                    }
                    self.collect_expr(keyword.expr());
                }
            }
            InstrCodegen::CallDirect(call) => {
                self.collect_expr(call.callable.as_ref());
                for arg in &call.args {
                    self.collect_expr(arg.expr());
                }
                for keyword in &call.keywords {
                    if let CallArgKeyword::Named { arg, .. } = keyword {
                        self.constants.intern_unicode_bytes(arg.as_str().as_bytes());
                    }
                    self.collect_expr(keyword.expr());
                }
            }
            InstrCodegen::GetAttr(op) => {
                if let Some(attr_bytes) =
                    self.string_constant_bytes_for_specialized_codegen(op.attr.as_ref())
                {
                    self.constants.intern_unicode_bytes(attr_bytes.as_slice());
                }
                op.visit_children(self);
            }
            InstrCodegen::SetAttr(op) => {
                if let Some(attr_bytes) =
                    self.string_constant_bytes_for_specialized_codegen(op.attr.as_ref())
                {
                    self.constants.intern_unicode_bytes(attr_bytes.as_slice());
                }
                op.visit_children(self);
            }
            InstrCodegen::Load(op)
                if op.name.location.is_global() || op.name.location.is_runtime_name() =>
            {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrCodegen::Load(_) => {}
            InstrCodegen::Store(op) if op.name.location.is_global() => {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
                op.visit_children(self);
            }
            InstrCodegen::Store(op) => {
                op.visit_children(self);
            }
            InstrCodegen::Del(op) if op.name.location.is_global() => {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrCodegen::BinOp(op) => op.visit_children(self),
            InstrCodegen::UnaryOp(op) => {
                op.visit_children(self);
            }
            InstrCodegen::GetItem(op) => {
                op.visit_children(self);
            }
            InstrCodegen::SetItem(op) => {
                op.visit_children(self);
            }
            InstrCodegen::DelItem(op) => {
                op.visit_children(self);
            }
            InstrCodegen::MakeCell(op) => {
                op.visit_children(self);
            }
            InstrCodegen::MakeFunction(op) => {
                op.visit_children(self);
            }
            InstrCodegen::Del(_) | InstrCodegen::CellRef(_) => {}
        }
    }

    fn deleted_name_arg_bytes(
        &self,
        call: &blockpy_intrinsics::Call<InstrCodegen>,
    ) -> Option<Vec<u8>> {
        if helper_name_for_codegen_expr(call.func.as_ref(), &self.constants)
            != Some("load_deleted_name")
            || call.args.len() != 2
        {
            return None;
        }
        self.string_constant_bytes_for_specialized_codegen(call.args[0].expr())
    }

    fn string_constant_bytes_for_specialized_codegen(
        &self,
        expr: &InstrCodegen,
    ) -> Option<Vec<u8>> {
        match expr {
            InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
                self.constants
                    .constant_string_bytes_value(ModuleConstantId(index as usize))
                    .map(ToOwned::to_owned)
            }),
            InstrCodegen::Call(call) => {
                if helper_name_for_codegen_expr(call.func.as_ref(), &self.constants) != Some("str")
                    || call.args.len() != 1
                    || !call.keywords.is_empty()
                {
                    return None;
                }
                self.string_constant_bytes_for_specialized_codegen(call.args[0].expr())
            }
            _ => None,
        }
    }
}

impl soac_blockpy::block_py::Visit<InstrCodegen> for ModuleConstantCollector {
    fn visit_instr(&mut self, expr: &InstrCodegen) {
        self.collect_expr(expr);
    }
}

fn helper_name_for_codegen_expr<'a>(
    expr: &'a InstrCodegen,
    module_constants: &'a ModuleCodegenConstants,
) -> Option<&'a str> {
    match expr {
        InstrCodegen::Load(op)
            if op.name.location.is_global() || op.name.location.is_runtime_name() =>
        {
            Some(op.name.id.as_str())
        }
        InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constants.constant_runtime_name_value(ModuleConstantId(index as usize))
        }),
        _ => None,
    }
}

fn abrupt_kind_tag(kind: AbruptKind) -> i64 {
    match kind {
        AbruptKind::Fallthrough => 0,
        AbruptKind::Return => 1,
        AbruptKind::Exception => 2,
        AbruptKind::Break => 3,
        AbruptKind::Continue => 4,
    }
}
