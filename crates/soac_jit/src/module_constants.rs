use pyo3::prelude::*;
use soac_core::block_py::literal::{Literal, NumberLiteralValue};
use soac_core::block_py::{
    AbruptKind, BlockArg, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm,
    CallArgKeyword, ChildVisitable, ConstantExpr, NameLike, ParamDefaultSource, RuntimeName,
    StorageLayout,
};
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};
use soac_ir_typed::{InstrTyped, TypedBlockPyModuleShape};
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
    pub fn collect_from_module(module: &BlockPyModule<BlockPyModuleShape>) -> Self {
        Self::collect_from_module_with_runtime_prelude(module, true, true)
    }

    pub fn collect_from_runtime_module(module: &BlockPyModule<BlockPyModuleShape>) -> Self {
        Self::collect_from_module_with_runtime_prelude(module, true, false)
    }

    pub fn collect_from_typed_module(module: &BlockPyModule<TypedBlockPyModuleShape>) -> Self {
        Self::collect_from_typed_module_with_runtime_prelude(module, true, true)
    }

    pub fn collect_from_typed_runtime_module(
        module: &BlockPyModule<TypedBlockPyModuleShape>,
    ) -> Self {
        Self::collect_from_typed_module_with_runtime_prelude(module, true, false)
    }

    fn collect_from_module_with_runtime_prelude(
        module: &BlockPyModule<BlockPyModuleShape>,
        include_runtime_name_prelude: bool,
        runtime_name_load_constants: bool,
    ) -> Self {
        let mut collector = ModuleConstantCollector::new(runtime_name_load_constants);
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

    fn collect_from_typed_module_with_runtime_prelude(
        module: &BlockPyModule<TypedBlockPyModuleShape>,
        include_runtime_name_prelude: bool,
        runtime_name_load_constants: bool,
    ) -> Self {
        let mut collector = ModuleConstantCollector::new(runtime_name_load_constants);
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
            collector.collect_typed_function(function);
        }
        collector.constants
    }

    pub fn collect_from_functions<'a>(
        functions: impl IntoIterator<Item = &'a BlockPyFunction<BlockPyModuleShape>>,
    ) -> Self {
        let mut collector = ModuleConstantCollector::new(true);
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

    pub fn collect_from_typed_functions<'a>(
        functions: impl IntoIterator<Item = &'a BlockPyFunction<TypedBlockPyModuleShape>>,
    ) -> Self {
        let mut collector = ModuleConstantCollector::new(true);
        for name in ALWAYS_REQUIRED_UNICODE_CONSTANTS {
            collector.constants.intern_unicode_bytes(name.as_bytes());
        }
        for name in ALWAYS_REQUIRED_RUNTIME_NAME_CONSTANTS {
            collector.constants.intern_runtime_name(*name);
        }
        for function in functions {
            collector.collect_typed_function(function);
        }
        collector.constants
    }

    pub fn build_python_constants(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        self.build_python_constants_with_runtime_names(py, RuntimeNameConstantMode::ImportRuntime)
    }

    pub fn build_python_constants_for_soac_runtime(
        &self,
        py: Python<'_>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        self.build_python_constants_with_runtime_names(
            py,
            RuntimeNameConstantMode::BootstrapSoacRuntime,
        )
    }

    fn build_python_constants_with_runtime_names(
        &self,
        py: Python<'_>,
        runtime_name_mode: RuntimeNameConstantMode,
    ) -> PyResult<Vec<Py<PyAny>>> {
        materialization::build_python_constants(self.values.as_slice(), py, runtime_name_mode)
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
        self.runtime_name_constant_id(runtime_name)
            .unwrap_or_else(|| {
                panic!("missing runtime-name module constant in codegen pool: {value}")
            })
    }

    pub(crate) fn runtime_name_constant_id(
        &self,
        runtime_name: RuntimeName,
    ) -> Option<ModuleConstantId> {
        self.lookup_id(&ModuleConstantValue::RuntimeName(runtime_name))
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
        self.constant_runtime_name(constant_id)
            .map(RuntimeName::name)
    }

    pub(crate) fn constant_runtime_name(
        &self,
        constant_id: ModuleConstantId,
    ) -> Option<RuntimeName> {
        match self.values.get(constant_id.0)? {
            ModuleConstantValue::RuntimeName(name) => Some(*name),
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

    fn push_explicit_constant_expr(&mut self, expr: &ConstantExpr) -> ModuleConstantId {
        let value = match expr {
            ConstantExpr::Literal(literal) => match literal.as_literal() {
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
            ConstantExpr::RuntimeName(name) => ModuleConstantValue::RuntimeName(*name),
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
    runtime_name_load_constants: bool,
}

fn should_include_in_locals_snapshot(name: &str) -> bool {
    !name.starts_with("_dp_") && name != "__soac__"
}

impl ModuleConstantCollector {
    fn new(runtime_name_load_constants: bool) -> Self {
        Self {
            constants: ModuleCodegenConstants::default(),
            runtime_name_load_constants,
        }
    }

    fn collect_closure_storage_names(&mut self, storage_layout: &StorageLayout) {
        if storage_layout
            .block_parameter_roles
            .iter()
            .any(|binding| binding.role == BlockParamRole::AbruptKind)
        {
            self.constants
                .intern_int(abrupt_kind_tag(AbruptKind::Fallthrough));
        }
        for slot in storage_layout
            .freevars
            .iter()
            .chain(storage_layout.cellvars.iter())
        {
            self.constants
                .intern_unicode_bytes(slot.logical_name.as_bytes());
            self.constants
                .intern_unicode_bytes(slot.storage_name.as_bytes());
        }
        for slot in &storage_layout.preserved_slots {
            self.constants
                .intern_unicode_bytes(slot.logical_name.as_bytes());
            self.constants
                .intern_unicode_bytes(slot.storage_name.as_bytes());
        }
    }

    fn collect_function(&mut self, function: &BlockPyFunction<BlockPyModuleShape>) {
        self.collect_class_binding_names(&function.scope);
        if let Some(storage_layout) = function.storage_layout() {
            self.collect_closure_storage_names(storage_layout);
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
            }
        }
        for block in &function.blocks {
            for stmt in &block.body {
                self.collect_stmt(stmt);
            }
            self.collect_term(&block.term);
        }
    }

    fn collect_typed_function(&mut self, function: &BlockPyFunction<TypedBlockPyModuleShape>) {
        self.collect_class_binding_names(&function.scope);
        if let Some(storage_layout) = function.storage_layout() {
            self.collect_closure_storage_names(storage_layout);
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
            }
        }
        for block in &function.blocks {
            for stmt in &block.body {
                self.collect_typed_stmt(stmt);
            }
            self.collect_typed_term(&block.term);
        }
    }

    fn collect_class_binding_names(&mut self, scope: &soac_core::block_py::CallableScopeInfo) {
        if let Some(bindings) = &scope.class_bindings {
            for slot in &bindings.node.slots {
                self.constants.intern_unicode_bytes(slot.name.as_bytes());
            }
        }
    }

    fn collect_stmt(&mut self, stmt: &InstrBlockPy) {
        self.collect_expr(stmt);
    }

    fn collect_typed_stmt(&mut self, stmt: &InstrTyped) {
        self.collect_typed_expr(stmt);
    }

    fn collect_term(&mut self, term: &BlockTerm<InstrBlockPy>) {
        match term {
            BlockTerm::Jump(edge) => self.collect_block_args(&edge.args),
            BlockTerm::IfTerm(if_term) => self.collect_expr(&if_term.test),
            BlockTerm::BranchTable(branch_table) => self.collect_expr(&branch_table.index),
            BlockTerm::Raise(raise_stmt) => {
                if let Some(exc) = &raise_stmt.exc {
                    self.collect_expr(exc);
                }
            }
            BlockTerm::Return(value) | BlockTerm::GeneratorReturn(value) => {
                self.collect_expr(value)
            }
        }
    }

    fn collect_typed_term(&mut self, term: &BlockTerm<InstrTyped>) {
        match term {
            BlockTerm::Jump(edge) => self.collect_block_args(&edge.args),
            BlockTerm::IfTerm(if_term) => self.collect_typed_expr(&if_term.test),
            BlockTerm::BranchTable(branch_table) => {
                self.collect_typed_expr(&branch_table.index);
            }
            BlockTerm::Raise(raise_stmt) => {
                if let Some(exc) = &raise_stmt.exc {
                    self.collect_typed_expr(exc);
                }
            }
            BlockTerm::Return(value) | BlockTerm::GeneratorReturn(value) => {
                self.collect_typed_expr(value)
            }
        }
    }

    fn collect_block_args(&mut self, args: &[BlockArg]) {
        for arg in args {
            if let BlockArg::AbruptKind(kind) = arg {
                self.constants.intern_int(abrupt_kind_tag(*kind));
            }
        }
    }

    fn collect_expr(&mut self, expr: &InstrBlockPy) {
        match expr {
            InstrBlockPy::IncrementCounter(_) => {}
            InstrBlockPy::Call(call) => {
                if let Some(const_bytes) = self.string_constant_bytes_for_specialized_codegen(expr)
                {
                    self.constants.intern_unicode_bytes(const_bytes.as_slice());
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
            InstrBlockPy::GetAttr(op) => {
                if let Some(attr_bytes) =
                    self.string_constant_bytes_for_specialized_codegen(op.attr.as_ref())
                {
                    self.constants.intern_unicode_bytes(attr_bytes.as_slice());
                }
                op.visit_children(self);
            }
            InstrBlockPy::SetAttr(op) => {
                if let Some(attr_bytes) =
                    self.string_constant_bytes_for_specialized_codegen(op.attr.as_ref())
                {
                    self.constants.intern_unicode_bytes(attr_bytes.as_slice());
                }
                op.visit_children(self);
            }
            InstrBlockPy::Load(op) if op.name.location.is_global() => {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrBlockPy::Load(op) if op.name.runtime_name_id().is_some() => {
                let runtime_name = op
                    .name
                    .runtime_name_id()
                    .expect("runtime-name load should carry a runtime name");
                if self.runtime_name_load_constants {
                    self.constants.intern_runtime_name(runtime_name);
                } else {
                    self.constants
                        .intern_unicode_bytes(runtime_name.name().as_bytes());
                }
            }
            InstrBlockPy::Load(op)
                if op.name.local_location().is_some() || op.name.preserved_location().is_some() =>
            {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrBlockPy::Load(op) => {
                if let Some(binding) = &op.cell_binding {
                    self.constants
                        .intern_unicode_bytes(binding.logical_name.as_str().as_bytes());
                }
            }
            InstrBlockPy::Store(op) if op.name.location.is_global() => {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
                op.visit_children(self);
            }
            InstrBlockPy::Store(op) => {
                op.visit_children(self);
            }
            InstrBlockPy::Del(op)
                if op.name.location.is_global() || op.name.preserved_location().is_some() =>
            {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrBlockPy::BinOp(op) => op.visit_children(self),
            InstrBlockPy::UnaryOp(op) => {
                op.visit_children(self);
            }
            InstrBlockPy::GetItem(op) => {
                op.visit_children(self);
            }
            InstrBlockPy::SetItem(op) => {
                op.visit_children(self);
            }
            InstrBlockPy::DelItem(op) => {
                op.visit_children(self);
            }
            InstrBlockPy::Tuple(op) => {
                op.visit_children(self);
            }
            InstrBlockPy::MakeCell(op) => {
                op.visit_children(self);
            }
            InstrBlockPy::MakeFunctionWithClosure(op) => {
                op.visit_children(self);
            }
            InstrBlockPy::ConstructClass(op) => op.visit_children(self),
            InstrBlockPy::PrepareClassDecorator(op) => {
                // Keyword labels are payloads, not expression children. The
                // preparation emits the same raw keyword call as Call.
                for keyword in &op.keywords {
                    if let CallArgKeyword::Named { arg, .. } = keyword {
                        self.constants.intern_unicode_bytes(arg.as_str().as_bytes());
                    }
                }
                op.visit_children(self);
            }
            InstrBlockPy::ApplyClassDecorator(op) => op.visit_children(self),
            InstrBlockPy::DiscardClassDecorator(op) => op.visit_children(self),
            InstrBlockPy::DiscardClassConstructionCaptures(op) => op.visit_children(self),
            InstrBlockPy::CompleteFunctionDefinition(op) => op.visit_children(self),
            InstrBlockPy::ApplyFunctionDescriptor(op) => op.visit_children(self),
            InstrBlockPy::NewAnnotationSet(op) => op.visit_children(self),
            InstrBlockPy::SetupAnnotations(op) => op.visit_children(self),
            InstrBlockPy::ConstructTypeParameterScope(op) => op.visit_children(self),
            InstrBlockPy::SubscriptGeneric(op) => op.visit_children(self),
            InstrBlockPy::SetFunctionTypeParameters(op) => op.visit_children(self),
            InstrBlockPy::CreateTypeAlias(op) => op.visit_children(self),
            InstrBlockPy::CreateTypeParameter(op) => op.visit_children(self),
            InstrBlockPy::SetTypeParameterDefault(op) => op.visit_children(self),
            InstrBlockPy::CheckAnnotationFormat(op) => op.visit_children(self),
            InstrBlockPy::RecordAnnotation(op) => op.visit_children(self),
            InstrBlockPy::ComprehensionInsert(op) => op.visit_children(self),
            InstrBlockPy::BuildCollection(op) => op.visit_children(self),
            InstrBlockPy::CallArgumentOp(op) => op.visit_children(self),
            InstrBlockPy::PreparedCall(op) => op.visit_children(self),
            InstrBlockPy::Del(_)
            | InstrBlockPy::TakeOperand(_)
            | InstrBlockPy::IteratorStep(_)
            | InstrBlockPy::CellRef(_) => {}
        }
    }

    fn collect_typed_expr(&mut self, expr: &InstrTyped) {
        match expr {
            InstrTyped::IncrementCounter(_) => {}
            InstrTyped::CalleeFunctionId(op) => {
                self.collect_typed_expr(op.value.as_ref());
            }
            InstrTyped::DirectCallGuardTest(op) => {
                op.visit_children(self);
            }
            InstrTyped::CallTyped(call) => {
                if let Some(const_bytes) =
                    self.typed_string_constant_bytes_for_specialized_codegen(expr)
                {
                    self.constants.intern_unicode_bytes(const_bytes.as_slice());
                }
                self.collect_typed_expr(call.func.as_ref());
                for arg in &call.args {
                    self.collect_typed_expr(arg.expr());
                }
                for keyword in &call.keywords {
                    if let CallArgKeyword::Named { arg, .. } = keyword {
                        self.constants.intern_unicode_bytes(arg.as_str().as_bytes());
                    }
                    self.collect_typed_expr(keyword.expr());
                }
            }
            InstrTyped::GuardedCallableCallTyped(call) => {
                call.visit_children(self);
            }
            InstrTyped::GuardedMethodCallTyped(call) => {
                call.visit_children(self);
            }
            InstrTyped::CallDirect(call) => {
                self.collect_typed_expr(call.callable.as_ref());
                for arg in &call.args {
                    self.collect_typed_expr(arg.expr());
                }
                for keyword in &call.keywords {
                    if let CallArgKeyword::Named { arg, .. } = keyword {
                        self.constants.intern_unicode_bytes(arg.as_str().as_bytes());
                    }
                    self.collect_typed_expr(keyword.expr());
                }
            }
            InstrTyped::DirectCallableCallTyped(op) => {
                op.visit_children(self);
            }
            InstrTyped::DirectMethodCallTyped(op) => {
                op.visit_children(self);
            }
            InstrTyped::GetAttrTyped(op) => {
                if let Some(attr_bytes) =
                    self.typed_string_constant_bytes_for_specialized_codegen(op.attr.as_ref())
                {
                    self.constants.intern_unicode_bytes(attr_bytes.as_slice());
                }
                op.visit_children(self);
            }
            InstrTyped::SetAttrTyped(op) => {
                if let Some(attr_bytes) =
                    self.typed_string_constant_bytes_for_specialized_codegen(op.attr.as_ref())
                {
                    self.constants.intern_unicode_bytes(attr_bytes.as_slice());
                }
                op.visit_children(self);
            }
            InstrTyped::Load(op) if op.name.location.is_global() => {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrTyped::Load(op) if op.name.runtime_name_id().is_some() => {
                let runtime_name = op
                    .name
                    .runtime_name_id()
                    .expect("runtime-name load should carry a runtime name");
                if self.runtime_name_load_constants {
                    self.constants.intern_runtime_name(runtime_name);
                } else {
                    self.constants
                        .intern_unicode_bytes(runtime_name.name().as_bytes());
                }
            }
            InstrTyped::Load(op)
                if op.name.local_location().is_some() || op.name.preserved_location().is_some() =>
            {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrTyped::Load(op) => {
                if let Some(binding) = &op.cell_binding {
                    self.constants
                        .intern_unicode_bytes(binding.logical_name.as_str().as_bytes());
                }
            }
            InstrTyped::Store(op) if op.name.location.is_global() => {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
                op.visit_children(self);
            }
            InstrTyped::Store(op) => {
                op.visit_children(self);
            }
            InstrTyped::Del(op)
                if op.name.location.is_global() || op.name.preserved_location().is_some() =>
            {
                self.constants
                    .intern_unicode_bytes(op.name.id_str().as_bytes());
            }
            InstrTyped::BinOp(op) => op.visit_children(self),
            InstrTyped::UnaryOp(op) => op.visit_children(self),
            InstrTyped::Tuple(op) => op.visit_children(self),
            InstrTyped::Truthy(op) => op.visit_children(self),
            InstrTyped::GetItem(op) => op.visit_children(self),
            InstrTyped::SetItem(op) => op.visit_children(self),
            InstrTyped::DelItem(op) => op.visit_children(self),
            InstrTyped::MakeCell(op) => op.visit_children(self),
            InstrTyped::MakeFunctionWithClosure(op) => op.visit_children(self),
            InstrTyped::ConstructClass(op) => op.visit_children(self),
            InstrTyped::PrepareClassDecorator(op) => {
                for keyword in &op.keywords {
                    if let CallArgKeyword::Named { arg, .. } = keyword {
                        self.constants.intern_unicode_bytes(arg.as_str().as_bytes());
                    }
                }
                op.visit_children(self);
            }
            InstrTyped::ApplyClassDecorator(op) => op.visit_children(self),
            InstrTyped::DiscardClassDecorator(op) => op.visit_children(self),
            InstrTyped::DiscardClassConstructionCaptures(op) => op.visit_children(self),
            InstrTyped::CompleteFunctionDefinition(op) => op.visit_children(self),
            InstrTyped::ApplyFunctionDescriptor(op) => op.visit_children(self),
            InstrTyped::NewAnnotationSet(op) => op.visit_children(self),
            InstrTyped::SetupAnnotations(op) => op.visit_children(self),
            InstrTyped::ConstructTypeParameterScope(op) => op.visit_children(self),
            InstrTyped::SubscriptGeneric(op) => op.visit_children(self),
            InstrTyped::SetFunctionTypeParameters(op) => op.visit_children(self),
            InstrTyped::CreateTypeAlias(op) => op.visit_children(self),
            InstrTyped::CreateTypeParameter(op) => op.visit_children(self),
            InstrTyped::SetTypeParameterDefault(op) => op.visit_children(self),
            InstrTyped::CheckAnnotationFormat(op) => op.visit_children(self),
            InstrTyped::RecordAnnotation(op) => op.visit_children(self),
            InstrTyped::ComprehensionInsert(op) => op.visit_children(self),
            InstrTyped::BuildCollection(op) => op.visit_children(self),
            InstrTyped::CallArgumentOp(op) => op.visit_children(self),
            InstrTyped::PreparedCall(op) => op.visit_children(self),
            InstrTyped::Del(_)
            | InstrTyped::TakeOperand(_)
            | InstrTyped::IteratorStep(_)
            | InstrTyped::CellRef(_) => {}
        }
    }

    fn string_constant_bytes_for_specialized_codegen(
        &self,
        expr: &InstrBlockPy,
    ) -> Option<Vec<u8>> {
        match expr {
            InstrBlockPy::Load(op) => op.name.location.as_constant().and_then(|index| {
                self.constants
                    .constant_string_bytes_value(ModuleConstantId(index as usize))
                    .map(ToOwned::to_owned)
            }),
            _ => None,
        }
    }

    fn typed_string_constant_bytes_for_specialized_codegen(
        &self,
        expr: &InstrTyped,
    ) -> Option<Vec<u8>> {
        match expr {
            InstrTyped::Load(op) => op.name.location.as_constant().and_then(|index| {
                self.constants
                    .constant_string_bytes_value(ModuleConstantId(index as usize))
                    .map(ToOwned::to_owned)
            }),
            _ => None,
        }
    }
}

impl soac_core::block_py::Visit<InstrBlockPy> for ModuleConstantCollector {
    fn visit_instr(&mut self, expr: &InstrBlockPy) {
        self.collect_expr(expr);
    }
}

impl soac_core::block_py::Visit<InstrTyped> for ModuleConstantCollector {
    fn visit_instr(&mut self, expr: &InstrTyped) {
        self.collect_typed_expr(expr);
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

#[cfg(test)]
mod tests {
    use super::{ModuleCodegenConstants, ModuleConstantValue, abrupt_kind_tag};
    use soac_contracts::{DefinitionKind, ModuleContentId, SourceIdentity, SourceRange};
    use soac_core::block_py::{
        AbruptKind, BlockTerm, ClosureInit, ClosureSlot, PrepareClassDecorator, PreservedSlot,
        PreservedSlotStorage, StorageLayout,
    };
    use soac_ir_blockpy::InstrBlockPy;
    use soac_ir_typed::lower_blockpy_module_to_typed;

    #[test]
    fn decorator_preparation_collects_keyword_labels_and_all_call_operands() {
        let source = "def run():\n    return factory(argument, eq=value, **options)\n";
        let mut module = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("decorator constant fixture should lower")
            .blockpy_module;
        let function = module
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "run")
            .expect("fixture should contain run");
        let construction_function = function.function_id;
        let result = function
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.term {
                BlockTerm::Return(value) if matches!(value, InstrBlockPy::Call(_)) => Some(value),
                _ => None,
            })
            .expect("fixture should return its ordinary keyword call");
        let InstrBlockPy::Call(call) = result.clone() else {
            unreachable!()
        };
        // This tests the constant collector's resolved operation shape, not
        // runtime admission. No signed/native class authority is manufactured.
        *result = PrepareClassDecorator::new(
            SourceIdentity {
                module: ModuleContentId::new("decorator_constants", 0),
                lexical_qualname: "Item".into(),
                source_range: SourceRange::new(0, source.len() as u32),
                definition_kind: DefinitionKind::Class,
            },
            construction_function,
            call.func,
            call.args,
            call.keywords,
            true,
            call.frame_namespace,
        )
        .into();
        let typed_module = lower_blockpy_module_to_typed(module.clone());
        for (kind, constants) in [
            (
                "BlockPy",
                ModuleCodegenConstants::collect_from_module(&module),
            ),
            (
                "typed BlockPy",
                ModuleCodegenConstants::collect_from_typed_module(&typed_module),
            ),
        ] {
            for name in ["eq", "factory", "argument", "value", "options"] {
                assert!(
                    constants
                        .lookup_id(&ModuleConstantValue::Unicode(name.as_bytes().to_vec()))
                        .is_some(),
                    "{kind} preparation constants must include {name:?}"
                );
            }
        }
    }

    #[test]
    fn source_control_spelling_does_not_create_a_fallthrough_constant() {
        for name in ["ordinary", "_dp_try_abrupt_kind_user"] {
            let source = format!("def f({name}):\n    return {name}\n");
            let module = soac_lowering::lower_python_to_blockpy_for_testing(&source)
                .expect("ordinary local constant fixture should lower")
                .blockpy_module;
            let typed = lower_blockpy_module_to_typed(module.clone());
            for constants in [
                ModuleCodegenConstants::collect_from_module(&module),
                ModuleCodegenConstants::collect_from_typed_module(&typed),
            ] {
                assert!(
                    constants
                        .lookup_id(&ModuleConstantValue::Int(abrupt_kind_tag(
                            AbruptKind::Fallthrough
                        )))
                        .is_none(),
                    "ordinary source spelling is not a control declaration"
                );
            }
        }
    }

    #[test]
    fn closure_and_preserved_storage_names_are_available_to_unbound_codegen() {
        let mut module =
            soac_lowering::lower_python_to_blockpy_for_testing("def run():\n    return 1\n")
                .expect("closure-storage constant fixture should lower")
                .blockpy_module;
        let function = module
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "run")
            .expect("closure-storage fixture should contain run");
        function.storage_layout = Some(StorageLayout {
            generator_resume_abi: None,
            block_parameter_roles: Vec::new(),
            class_bindings: None,
            expression_temporaries: Vec::new(),
            freevars: vec![ClosureSlot {
                logical_name: "captured_value".to_string(),
                storage_name: "_dp_free_captured_value".to_string(),
                init: ClosureInit::InheritedCapture,
            }],
            cellvars: vec![ClosureSlot {
                logical_name: "pool".to_string(),
                storage_name: "_dp_cell_pool".to_string(),
                init: ClosureInit::EmptyCell,
            }],
            preserved_slots: vec![PreservedSlot {
                generator_control: None,
                logical_name: "suspended_value".to_string(),
                storage_name: "_dp_preserved_suspended_value".to_string(),
                init: ClosureInit::Deferred,
                storage: PreservedSlotStorage::PyCellObject,
            }],
            stack_slots: Vec::new(),
        });

        let typed_module = lower_blockpy_module_to_typed(module.clone());
        for (kind, constants) in [
            (
                "BlockPy",
                ModuleCodegenConstants::collect_from_module(&module),
            ),
            (
                "typed BlockPy",
                ModuleCodegenConstants::collect_from_typed_module(&typed_module),
            ),
        ] {
            for name in [
                "captured_value",
                "_dp_free_captured_value",
                "pool",
                "_dp_cell_pool",
                "suspended_value",
                "_dp_preserved_suspended_value",
            ] {
                assert!(
                    constants
                        .lookup_id(&ModuleConstantValue::Unicode(name.as_bytes().to_vec()))
                        .is_some(),
                    "{kind} module constants must include closure/preserved storage name {name:?}"
                );
            }
        }
    }
}
