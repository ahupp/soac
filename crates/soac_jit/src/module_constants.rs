use pyo3::ffi;
use pyo3::prelude::*;
use soac_core::block_py as blockpy_intrinsics;
use soac_core::block_py::{
    AbruptKind, BlockArg, BlockPyFunction, BlockPyModule, BlockTerm, CallArgKeyword,
    ChildVisitable, NameLike, ParamDefaultSource, RuntimeName,
};
use soac_lowering::block_py::literal::{Literal, NumberLiteralValue};
use soac_lowering::passes::{CodegenModuleShape, InstrCodegen, InstrResolved};
use std::collections::HashMap;

mod materialization;
use materialization::RuntimeNameConstantMode;
pub(crate) use materialization::{
    StaticPyObjectImage, load_runtime_name_owned, load_runtime_name_owned_by_id,
    raise_name_error_for_missing_name,
};

const ALWAYS_REQUIRED_UNICODE_CONSTANTS: &[&str] = &[
    "dict",
    "list",
    "raise_from",
    "tuple_from_iter",
    "append",
    "extend",
    "update",
];
const ALWAYS_REQUIRED_RUNTIME_NAME_CONSTANTS: &[RuntimeName] = &[
    RuntimeName::True,
    RuntimeName::False,
    RuntimeName::None,
    RuntimeName::EmptyTuple,
    RuntimeName::IterComplete,
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
    RuntimeName(RuntimeName),
}

#[derive(Debug, Clone, Default)]
pub struct ModuleCodegenConstants {
    values: Vec<ModuleConstantValue>,
    ids: HashMap<ModuleConstantValue, ModuleConstantId>,
}

impl ModuleCodegenConstants {
    pub fn collect_from_module(module: &BlockPyModule<CodegenModuleShape>) -> Self {
        Self::collect_from_module_with_runtime_prelude(module, true)
    }

    pub fn collect_from_runtime_module(module: &BlockPyModule<CodegenModuleShape>) -> Self {
        Self::collect_from_module_with_runtime_prelude(module, true)
    }

    fn collect_from_module_with_runtime_prelude(
        module: &BlockPyModule<CodegenModuleShape>,
        include_runtime_name_prelude: bool,
    ) -> Self {
        let mut collector = ModuleConstantCollector::default();
        for expr in &module.module_constants {
            collector.constants.push_explicit_constant_expr(expr);
        }
        for name in ALWAYS_REQUIRED_UNICODE_CONSTANTS {
            collector.constants.intern_unicode_bytes(name.as_bytes());
        }
        if include_runtime_name_prelude {
            for name in ALWAYS_REQUIRED_RUNTIME_NAME_CONSTANTS {
                collector.constants.intern_runtime_name(*name);
            }
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
        for name in ALWAYS_REQUIRED_RUNTIME_NAME_CONSTANTS {
            collector.constants.intern_runtime_name(*name);
        }
        for function in functions {
            collector.collect_function(function);
        }
        collector.constants
    }

    pub fn build_python_constants(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        self.build_python_constants_with_runtime_names(
            py,
            RuntimeNameConstantMode::ImportRuntime,
            |_| Ok(None),
        )
    }

    pub fn build_python_constants_for_soac_runtime(
        &self,
        py: Python<'_>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        self.build_python_constants_with_runtime_names(
            py,
            RuntimeNameConstantMode::BootstrapSoacRuntime,
            |_| Ok(None),
        )
    }

    pub(crate) fn build_python_constants_with_static_resolver(
        &self,
        py: Python<'_>,
        is_soac_runtime: bool,
        static_resolver: impl FnMut(ModuleConstantId) -> PyResult<Option<*mut ffi::PyObject>>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let runtime_name_mode = if is_soac_runtime {
            RuntimeNameConstantMode::BootstrapSoacRuntime
        } else {
            RuntimeNameConstantMode::ImportRuntime
        };
        self.build_python_constants_with_runtime_names(py, runtime_name_mode, static_resolver)
    }

    fn build_python_constants_with_runtime_names(
        &self,
        py: Python<'_>,
        runtime_name_mode: RuntimeNameConstantMode,
        static_resolver: impl FnMut(ModuleConstantId) -> PyResult<Option<*mut ffi::PyObject>>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        materialization::build_python_constants(
            self.values.as_slice(),
            py,
            runtime_name_mode,
            static_resolver,
        )
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn static_pyobject_image(
        &self,
        constant_id: ModuleConstantId,
    ) -> Option<StaticPyObjectImage> {
        materialization::static_pyobject_image(self.values.get(constant_id.0)?)
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

    pub fn require_u64_constant_id(&self, value: u64) -> ModuleConstantId {
        if let Ok(value) = i64::try_from(value) {
            self.require_int_constant_id(value)
        } else {
            self.require_big_int_constant_id(value.to_string().as_str())
        }
    }

    pub fn require_big_int_constant_id(&self, value: &str) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::BigInt(value.to_string()))
            .unwrap_or_else(|| panic!("missing module big-int constant in codegen pool: {value}"))
    }

    pub fn require_float_constant_id(&self, value: f64) -> ModuleConstantId {
        self.lookup_id(&ModuleConstantValue::FloatBits(value.to_bits()))
            .unwrap_or_else(|| panic!("missing module float constant in codegen pool: {value}"))
    }

    pub fn require_runtime_name_constant_id(&self, value: &str) -> ModuleConstantId {
        let runtime_name = RuntimeName::from_name(value)
            .unwrap_or_else(|| panic!("unknown runtime-name module constant: {value}"));
        self.lookup_id(&ModuleConstantValue::RuntimeName(runtime_name))
            .unwrap_or_else(|| {
                panic!("missing runtime-name module constant in codegen pool: {value}")
            })
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

    pub fn constant_u64_value(&self, constant_id: ModuleConstantId) -> Option<u64> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::Int(value) if *value >= 0 => Some(*value as u64),
            ModuleConstantValue::BigInt(value) => value.parse().ok(),
            ModuleConstantValue::Unicode(_)
            | ModuleConstantValue::Bytes(_)
            | ModuleConstantValue::Int(_)
            | ModuleConstantValue::FloatBits(_)
            | ModuleConstantValue::RuntimeName(_) => None,
        }
    }

    pub fn constant_i64_value(&self, constant_id: ModuleConstantId) -> Option<i64> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::Int(value) => Some(*value),
            ModuleConstantValue::BigInt(value) => value.parse().ok(),
            ModuleConstantValue::Unicode(_)
            | ModuleConstantValue::Bytes(_)
            | ModuleConstantValue::FloatBits(_)
            | ModuleConstantValue::RuntimeName(_) => None,
        }
    }

    pub fn constant_is_int(&self, constant_id: ModuleConstantId) -> bool {
        matches!(
            self.values.get(constant_id.0),
            Some(ModuleConstantValue::Int(_) | ModuleConstantValue::BigInt(_))
        )
    }

    pub fn constant_runtime_name_value(&self, constant_id: ModuleConstantId) -> Option<&str> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::RuntimeName(name) => Some(name.name()),
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
                ModuleConstantValue::RuntimeName(
                    op.name
                        .runtime_name_id()
                        .expect("runtime-name load should carry a RuntimeName id"),
                )
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

    fn intern_runtime_name(&mut self, value: RuntimeName) -> ModuleConstantId {
        self.intern(ModuleConstantValue::RuntimeName(value))
    }

    fn intern_int(&mut self, value: i64) -> ModuleConstantId {
        self.intern(ModuleConstantValue::Int(value))
    }
}

#[derive(Default)]
struct ModuleConstantCollector {
    constants: ModuleCodegenConstants,
}

fn should_include_in_locals_snapshot(name: &str) -> bool {
    !name.starts_with("_dp_") && name != "__soac__"
}

impl ModuleConstantCollector {
    fn collect_function(&mut self, function: &BlockPyFunction<CodegenModuleShape>) {
        if let Some(storage_layout) = function.storage_layout() {
            for name in storage_layout.stack_slots() {
                if should_include_in_locals_snapshot(name) {
                    self.constants.intern_unicode_bytes(name.as_bytes());
                }
            }
        }
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
        if let Some(storage_layout) = function.storage_layout().as_ref() {
            for name in storage_layout.stack_slots() {
                self.constants.intern_unicode_bytes(name.as_bytes());
                if name.starts_with("_dp_try_abrupt_kind_") {
                    self.constants
                        .intern_int(abrupt_kind_tag(AbruptKind::Fallthrough));
                }
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
            InstrCodegen::DirectFunctionIdGuardTest(op) => {
                self.collect_expr(op.value.as_ref());
            }
            InstrCodegen::DirectCallableTypeVersionGuardTest(op) => {
                self.collect_expr(op.value.as_ref());
            }
            InstrCodegen::DirectReceiverTypeVersionGuardTest(op) => {
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
            InstrCodegen::DirectCallableCall(op) => {
                op.visit_children(self);
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
            InstrCodegen::Load(op) if op.name.local_location().is_some() => {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
                if op.name.id_str().starts_with("_dp_try_abrupt_kind_") {
                    self.constants
                        .intern_int(abrupt_kind_tag(AbruptKind::Fallthrough));
                }
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
            InstrCodegen::Tuple(op) => {
                op.visit_children(self);
            }
            InstrCodegen::MakeCell(op) => {
                op.visit_children(self);
            }
            InstrCodegen::MakeFunctionWithClosure(op) => {
                op.visit_children(self);
            }
            InstrCodegen::Del(_) | InstrCodegen::CellRef(_) => {}
        }
    }

    fn deleted_name_arg_bytes(
        &self,
        call: &blockpy_intrinsics::Call<InstrCodegen>,
    ) -> Option<Vec<u8>> {
        match helper_name_for_codegen_expr(call.func.as_ref(), &self.constants) {
            Some("raise_deleted_name") if call.args.len() == 1 => {}
            _ => return None,
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

impl soac_core::block_py::Visit<InstrCodegen> for ModuleConstantCollector {
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
