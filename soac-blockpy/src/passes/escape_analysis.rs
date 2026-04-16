use crate::block_py::pretty::BlockPyPrettyPrint;
use crate::block_py::{
    instr_any, BlockPyFunction, BlockPyModule, BlockTerm, CallArgPositional, FunctionId, HasMeta,
    InstrCodegen, InstrCodegenOp, InstrId, InstrResolved, Literal, LocalLocation, NameLike,
    NameLocation,
};
use crate::CodegenModuleShape;
use std::collections::{HashMap, HashSet};

#[derive(
    Clone, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct EscapeSummaryModule {
    pub functions: HashMap<crate::block_py::FunctionId, FunctionEscapeSummary>,
}

impl EscapeSummaryModule {
    pub fn function(
        &self,
        function_id: crate::block_py::FunctionId,
    ) -> Option<&FunctionEscapeSummary> {
        self.functions.get(&function_id)
    }

    pub fn non_escaping_constructor(
        &self,
        function_id: crate::block_py::FunctionId,
    ) -> Option<&NonEscapingConstructorSummary> {
        self.function(function_id)
            .and_then(|summary| summary.non_escaping_constructor.as_ref())
    }

    pub fn straightline_field_initializer(
        &self,
        function_id: crate::block_py::FunctionId,
    ) -> Option<&FieldInitializerConstructorSummary> {
        self.function(function_id)
            .and_then(|summary| summary.straightline_field_initializer.as_ref())
    }

    pub fn remap_function_ids(
        &mut self,
        remap: impl Fn(crate::block_py::FunctionId) -> crate::block_py::FunctionId + Copy,
    ) {
        self.functions = std::mem::take(&mut self.functions)
            .into_iter()
            .map(|(function_id, mut summary)| {
                summary.remap_function_ids(remap);
                (remap(function_id), summary)
            })
            .collect();
    }
}

impl BlockPyPrettyPrint for EscapeSummaryModule {
    fn pretty_print(&self) -> String {
        let mut function_ids = self.functions.keys().copied().collect::<Vec<_>>();
        function_ids.sort_by_key(|function_id| function_id.packed());
        let mut out = String::new();
        for function_id in function_ids {
            let summary = self
                .functions
                .get(&function_id)
                .expect("function id was collected from this summary map");
            if summary.non_escaping_constructor.is_none()
                && summary.straightline_field_initializer.is_none()
                && summary.non_escaping_constructor_allocations.is_empty()
            {
                continue;
            }
            out.push_str(&format!("{function_id}:\n"));
            if let Some(constructor) = &summary.non_escaping_constructor {
                out.push_str(&format!(
                    "  non_escaping_constructor self={} fields={}\n",
                    constructor.self_name,
                    render_field_stores(&constructor.field_stores),
                ));
            }
            if let Some(constructor) = &summary.straightline_field_initializer {
                out.push_str(&format!(
                    "  straightline_field_initializer self={} fields={}\n",
                    constructor.self_name,
                    render_field_stores(&constructor.field_stores),
                ));
            }
            for allocation in &summary.non_escaping_constructor_allocations {
                out.push_str(&format!(
                    "  non_escaping_constructor_allocation local={} constructor={} reads={} writes={}\n",
                    allocation.local_name,
                    allocation.constructor_function_id,
                    render_field_accesses(&allocation.field_reads),
                    render_field_accesses(&allocation.field_writes),
                ));
            }
        }
        if out.is_empty() {
            "; no constructor escape summaries\n".to_string()
        } else {
            out
        }
    }
}

fn render_field_stores(stores: &[ConstructorFieldStore]) -> String {
    stores
        .iter()
        .map(|store| format!("{}={}", store.field_name, render_field_value(&store.value)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_field_accesses(accesses: &[ConstructorFieldAccess]) -> String {
    accesses
        .iter()
        .map(|access| access.field_name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_field_value(value: &ConstructorFieldValue) -> String {
    match value {
        ConstructorFieldValue::Param { name, index, .. } => format!("param#{index}:{name}"),
        ConstructorFieldValue::Local { name, .. } => format!("local:{name}"),
        ConstructorFieldValue::Constant { description } => format!("const:{description}"),
        ConstructorFieldValue::Other => "other".to_string(),
    }
}

#[derive(
    Clone, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct FunctionEscapeSummary {
    pub non_escaping_constructor: Option<NonEscapingConstructorSummary>,
    pub straightline_field_initializer: Option<FieldInitializerConstructorSummary>,
    pub non_escaping_constructor_allocations: Vec<NonEscapingConstructorAllocationSummary>,
}

impl FunctionEscapeSummary {
    fn remap_function_ids(
        &mut self,
        remap: impl Fn(crate::block_py::FunctionId) -> crate::block_py::FunctionId + Copy,
    ) {
        for allocation in &mut self.non_escaping_constructor_allocations {
            allocation.constructor_function_id = remap(allocation.constructor_function_id);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct NonEscapingConstructorSummary {
    pub self_name: String,
    pub self_location: Option<LocalLocation>,
    pub field_stores: Vec<ConstructorFieldStore>,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FieldInitializerConstructorSummary {
    pub self_name: String,
    pub self_location: Option<LocalLocation>,
    pub field_stores: Vec<ConstructorFieldStore>,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ConstructorFieldStore {
    pub field_name: String,
    pub value: ConstructorFieldValue,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct NonEscapingConstructorAllocationSummary {
    pub local_name: String,
    pub local_location: LocalLocation,
    pub constructor_function_id: FunctionId,
    pub call_instr_id: Option<InstrId>,
    pub field_reads: Vec<ConstructorFieldAccess>,
    pub field_writes: Vec<ConstructorFieldAccess>,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ConstructorFieldAccess {
    pub field_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ConstructorFieldValue {
    Param {
        name: String,
        index: usize,
        location: Option<LocalLocation>,
    },
    Local {
        name: String,
        location: Option<LocalLocation>,
    },
    Constant {
        description: String,
    },
    Other,
}

pub fn summarize_module_escapes(module: &BlockPyModule<CodegenModuleShape>) -> EscapeSummaryModule {
    let mut functions = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                FunctionEscapeSummary {
                    non_escaping_constructor: summarize_non_escaping_constructor(module, function),
                    straightline_field_initializer: summarize_straightline_field_initializer(
                        module, function,
                    ),
                    non_escaping_constructor_allocations: Vec::new(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let straightline_constructor_ids = functions
        .iter()
        .filter_map(|(function_id, summary)| {
            summary
                .straightline_field_initializer
                .as_ref()
                .map(|_| *function_id)
        })
        .collect::<HashSet<_>>();
    for function in &module.callable_defs {
        let allocations = summarize_non_escaping_constructor_allocations(
            module,
            function,
            &straightline_constructor_ids,
        );
        if let Some(summary) = functions.get_mut(&function.function_id) {
            summary.non_escaping_constructor_allocations = allocations;
        }
    }
    EscapeSummaryModule { functions }
}

fn summarize_non_escaping_constructor_allocations(
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
    straightline_constructor_ids: &HashSet<FunctionId>,
) -> Vec<NonEscapingConstructorAllocationSummary> {
    let mut allocations = Vec::new();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let InstrCodegenOp::Store(store) = instr else {
                continue;
            };
            let Some(local_location) = store.name.location.as_local() else {
                continue;
            };
            let InstrCodegenOp::CallDirect(call) = store.value.as_ref() else {
                continue;
            };
            if !straightline_constructor_ids.contains(&call.function_id) {
                continue;
            }
            let Some(summary) = summarize_constructor_allocation_uses_in_block(
                module,
                local_location,
                store.name.id_str().to_string(),
                call.function_id,
                call.meta().instr_id,
                &block.body[instr_index + 1..],
                &block.term,
            ) else {
                continue;
            };
            allocations.push(summary);
        }
    }
    allocations
}

fn summarize_constructor_allocation_uses_in_block(
    module: &BlockPyModule<CodegenModuleShape>,
    local_location: LocalLocation,
    local_name: String,
    constructor_function_id: FunctionId,
    call_instr_id: Option<InstrId>,
    remaining_body: &[InstrCodegen],
    term: &BlockTerm<InstrCodegen>,
) -> Option<NonEscapingConstructorAllocationSummary> {
    let mut summary = NonEscapingConstructorAllocationSummary {
        local_name,
        local_location,
        constructor_function_id,
        call_instr_id,
        field_reads: Vec::new(),
        field_writes: Vec::new(),
    };
    for instr in remaining_body {
        if let InstrCodegenOp::Store(store) = instr {
            if store.name.location.as_local() == Some(local_location) {
                break;
            }
        }
        if !record_allowed_constructor_local_use(module, local_location, instr, &mut summary) {
            return None;
        }
    }
    if !record_allowed_constructor_local_use_in_term(module, local_location, term, &mut summary) {
        return None;
    }
    if summary.field_reads.is_empty() && summary.field_writes.is_empty() {
        return None;
    }
    Some(summary)
}

fn record_allowed_constructor_local_use_in_term(
    module: &BlockPyModule<CodegenModuleShape>,
    local_location: LocalLocation,
    term: &BlockTerm<InstrCodegen>,
    summary: &mut NonEscapingConstructorAllocationSummary,
) -> bool {
    match term {
        BlockTerm::Return(value) => {
            record_allowed_constructor_local_use(module, local_location, value, summary)
        }
        BlockTerm::IfTerm(term) => {
            record_allowed_constructor_local_use(module, local_location, &term.test, summary)
        }
        BlockTerm::BranchTable(term) => {
            record_allowed_constructor_local_use(module, local_location, &term.index, summary)
        }
        BlockTerm::Raise(term) => term.exc.as_ref().is_none_or(|exc| {
            record_allowed_constructor_local_use(module, local_location, exc, summary)
        }),
        BlockTerm::Jump(_) => true,
    }
}

fn record_allowed_constructor_local_use(
    module: &BlockPyModule<CodegenModuleShape>,
    local_location: LocalLocation,
    instr: &InstrCodegen,
    summary: &mut NonEscapingConstructorAllocationSummary,
) -> bool {
    match instr {
        InstrCodegenOp::GetAttr(getattr)
            if is_local_load(&getattr.value, local_location)
                && !instr_uses_local(&getattr.attr, local_location) =>
        {
            let Some(field_name) = constant_string(module, &getattr.attr) else {
                return false;
            };
            summary
                .field_reads
                .push(ConstructorFieldAccess { field_name });
            true
        }
        InstrCodegenOp::SetAttr(setattr)
            if is_local_load(&setattr.value, local_location)
                && !instr_uses_local(&setattr.attr, local_location)
                && !instr_uses_local(&setattr.replacement, local_location) =>
        {
            let Some(field_name) = constant_string(module, &setattr.attr) else {
                return false;
            };
            summary
                .field_writes
                .push(ConstructorFieldAccess { field_name });
            true
        }
        _ => !instr_uses_local(instr, local_location),
    }
}

fn is_local_load(instr: &InstrCodegen, local_location: LocalLocation) -> bool {
    matches!(
        instr,
        InstrCodegenOp::Load(load) if load.name.location.as_local() == Some(local_location)
    )
}

fn instr_uses_local(instr: &InstrCodegen, local_location: LocalLocation) -> bool {
    instr_any(instr, |child| match child {
        InstrCodegenOp::Load(load) => load.name.location.as_local() == Some(local_location),
        _ => false,
    })
}

fn constant_string(
    module: &BlockPyModule<CodegenModuleShape>,
    instr: &InstrCodegen,
) -> Option<String> {
    let InstrCodegenOp::Load(load) = instr else {
        return None;
    };
    let constant_index = load.name.location.as_constant()? as usize;
    match module.module_constants.get(constant_index)? {
        InstrResolved::Literal(value) => match value.as_literal() {
            Literal::StringLiteral(value) => Some(value.value.clone()),
            Literal::BytesLiteral(_) | Literal::NumberLiteral(_) => None,
        },
        _ => None,
    }
}

fn summarize_non_escaping_constructor(
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
) -> Option<NonEscapingConstructorSummary> {
    summarize_constructor_with_mode(module, function, ConstructorSummaryMode::NonEscaping).map(
        |summary| NonEscapingConstructorSummary {
            self_name: summary.self_name,
            self_location: summary.self_location,
            field_stores: summary.field_stores,
        },
    )
}

fn summarize_straightline_field_initializer(
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
) -> Option<FieldInitializerConstructorSummary> {
    summarize_constructor_with_mode(
        module,
        function,
        ConstructorSummaryMode::StraightlineFieldInitializer,
    )
    .map(|summary| FieldInitializerConstructorSummary {
        self_name: summary.self_name,
        self_location: summary.self_location,
        field_stores: summary.field_stores,
    })
}

fn summarize_constructor_with_mode(
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
    mode: ConstructorSummaryMode,
) -> Option<ConstructorSummary> {
    if !function.names.qualname.ends_with(".__init__") {
        return None;
    }
    let self_param = function.params.params.first()?;
    if self_param.name != "self" {
        return None;
    }

    let mut summary = ConstructorBuilder {
        module,
        function,
        mode,
        self_name: self_param.name.clone(),
        self_location: None,
        aliases: HashMap::new(),
        field_stores: Vec::new(),
        rejected: None,
    };
    summary.scan_function();
    summary.finish().map(|summary| ConstructorSummary {
        self_name: summary.self_name,
        self_location: summary.self_location,
        field_stores: summary.field_stores,
    })
}

struct ConstructorSummary {
    self_name: String,
    self_location: Option<LocalLocation>,
    field_stores: Vec<ConstructorFieldStore>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConstructorSummaryMode {
    NonEscaping,
    StraightlineFieldInitializer,
}

struct ConstructorBuilder<'a> {
    module: &'a BlockPyModule<CodegenModuleShape>,
    function: &'a BlockPyFunction<CodegenModuleShape>,
    mode: ConstructorSummaryMode,
    self_name: String,
    self_location: Option<LocalLocation>,
    aliases: HashMap<LocalLocation, ValueAlias>,
    field_stores: Vec<ConstructorFieldStore>,
    rejected: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ValueAlias {
    SelfObject,
    Param {
        name: String,
        index: usize,
        location: Option<LocalLocation>,
    },
    Local {
        name: String,
        location: Option<LocalLocation>,
    },
    Constant {
        description: String,
    },
}

impl ConstructorBuilder<'_> {
    fn scan_function(&mut self) {
        if self.mode == ConstructorSummaryMode::StraightlineFieldInitializer
            && self.function.blocks.len() != 1
        {
            self.reject("straightline field initializer must have exactly one block");
            return;
        }
        for block in &self.function.blocks {
            for instr in &block.body {
                self.scan_instr(instr);
                if self.rejected.is_some() {
                    return;
                }
            }
            self.scan_term(&block.term);
            if self.rejected.is_some() {
                return;
            }
        }
    }

    fn scan_instr(&mut self, instr: &InstrCodegen) {
        match instr {
            InstrCodegenOp::Store(store) => {
                let Some(location) = store.name.location.as_local() else {
                    if self.instr_uses_self_or_alias(&store.value) {
                        self.reject("self used in non-local store value");
                    } else if self.mode == ConstructorSummaryMode::StraightlineFieldInitializer {
                        self.reject("straightline field initializer used non-local store");
                    }
                    return;
                };
                if let Some(alias) = self.value_alias(&store.value) {
                    self.aliases.insert(location, alias);
                } else {
                    self.aliases.remove(&location);
                    if self.instr_uses_self_or_alias(&store.value) {
                        self.reject("self used in non-aliasable local store value");
                    } else if self.mode == ConstructorSummaryMode::StraightlineFieldInitializer {
                        self.reject("straightline field initializer used unsupported store value");
                    }
                }
            }
            InstrCodegenOp::Del(del) => {
                if let Some(location) = del.name.location.as_local() {
                    self.aliases.remove(&location);
                }
                if del.name.id_str() == self.self_name {
                    self.reject("deleted self");
                }
            }
            InstrCodegenOp::SetAttr(setattr) if self.is_self_alias(&setattr.value) => {
                if self.instr_uses_self_or_alias(&setattr.attr)
                    || self.instr_uses_self_or_alias(&setattr.replacement)
                {
                    self.reject("self used in SetAttr attr or replacement");
                    return;
                }
                let Some(field_name) = self.constant_string(&setattr.attr) else {
                    self.reject("SetAttr on self used a non-constant field name");
                    return;
                };
                self.field_stores.push(ConstructorFieldStore {
                    field_name,
                    value: self.field_value(&setattr.replacement),
                });
            }
            _ if instr_uses_self(instr, self.self_name.as_str()) => {
                self.reject("direct self use outside field store");
            }
            _ if self.instr_uses_self_alias(instr) => {
                self.reject("self alias use outside field store");
            }
            _ if self.mode == ConstructorSummaryMode::StraightlineFieldInitializer => {
                self.reject("straightline field initializer used unsupported instruction");
            }
            _ => {}
        }
    }

    fn scan_term(&mut self, term: &BlockTerm<InstrCodegen>) {
        if self.mode == ConstructorSummaryMode::StraightlineFieldInitializer {
            match term {
                BlockTerm::Return(value) if self.is_runtime_none(value) => return,
                BlockTerm::Return(_) => {
                    self.reject("straightline field initializer returned non-None value");
                }
                BlockTerm::Jump(_)
                | BlockTerm::IfTerm(_)
                | BlockTerm::BranchTable(_)
                | BlockTerm::Raise(_) => {
                    self.reject("straightline field initializer used non-return terminator");
                }
            }
            return;
        }
        let uses_self = match term {
            BlockTerm::Jump(_) => false,
            BlockTerm::IfTerm(term) => self.instr_uses_self_or_alias(&term.test),
            BlockTerm::BranchTable(term) => self.instr_uses_self_or_alias(&term.index),
            BlockTerm::Raise(term) => term
                .exc
                .as_ref()
                .is_some_and(|exc| self.instr_uses_self_or_alias(exc)),
            BlockTerm::Return(value) => self.instr_uses_self_or_alias(value),
        };
        if uses_self {
            self.reject("self used in terminator");
        }
    }

    fn finish(self) -> Option<NonEscapingConstructorSummary> {
        if self.rejected.is_some() || self.field_stores.is_empty() {
            return None;
        }
        Some(NonEscapingConstructorSummary {
            self_name: self.self_name,
            self_location: self.self_location,
            field_stores: self.field_stores,
        })
    }

    fn reject(&mut self, reason: impl Into<String>) {
        if self.rejected.is_none() {
            self.rejected = Some(reason.into());
        }
    }

    fn is_self_alias(&mut self, instr: &InstrCodegen) -> bool {
        matches!(self.value_alias(instr), Some(ValueAlias::SelfObject))
    }

    fn value_alias(&mut self, instr: &InstrCodegen) -> Option<ValueAlias> {
        let InstrCodegenOp::Load(load) = instr else {
            return self.checked_load_deleted_alias(instr);
        };
        if load.name.id_str() == self.self_name {
            if let NameLocation::Local(location) = load.name.location {
                match self.self_location {
                    Some(existing) if existing != location => return None,
                    Some(_) => {}
                    None => self.self_location = Some(location),
                }
                return Some(ValueAlias::SelfObject);
            }
            return None;
        }
        if let Some(location) = load.name.location.as_local() {
            if let Some(alias) = self.aliases.get(&location) {
                return Some(alias.clone());
            }
        }
        if let Some(index) = self.function.params.param_index(load.name.id_str()) {
            return Some(ValueAlias::Param {
                name: load.name.id_str().to_string(),
                index,
                location: load.name.location.as_local(),
            });
        }
        match load.name.location {
            NameLocation::Local(location) => Some(ValueAlias::Local {
                name: load.name.id_str().to_string(),
                location: Some(location),
            }),
            NameLocation::Constant(_) => Some(ValueAlias::Constant {
                description: load.name.id_str().to_string(),
            }),
            NameLocation::GlobalName
            | NameLocation::Global(_)
            | NameLocation::RuntimeName
            | NameLocation::Cell(_) => None,
        }
    }

    fn checked_load_deleted_alias(&mut self, instr: &InstrCodegen) -> Option<ValueAlias> {
        let InstrCodegenOp::Call(call) = instr else {
            return None;
        };
        if self.constant_load_name(&call.func)? != "load_deleted_name" {
            return None;
        }
        let [CallArgPositional::Positional(_name), CallArgPositional::Positional(value)] =
            call.args.as_slice()
        else {
            return None;
        };
        self.value_alias(value)
    }

    fn constant_string(&self, instr: &InstrCodegen) -> Option<String> {
        let InstrCodegenOp::Load(load) = instr else {
            return None;
        };
        let constant_index = load.name.location.as_constant()? as usize;
        match self.module.module_constants.get(constant_index)? {
            InstrResolved::Literal(value) => match value.as_literal() {
                Literal::StringLiteral(value) => Some(value.value.clone()),
                Literal::BytesLiteral(_) | Literal::NumberLiteral(_) => None,
            },
            _ => None,
        }
    }

    fn constant_load_name(&self, instr: &InstrCodegen) -> Option<String> {
        let InstrCodegenOp::Load(load) = instr else {
            return None;
        };
        let constant_index = load.name.location.as_constant()? as usize;
        match self.module.module_constants.get(constant_index)? {
            InstrResolved::Load(load) => Some(load.name.id_str().to_string()),
            _ => None,
        }
    }

    fn is_runtime_none(&self, instr: &InstrCodegen) -> bool {
        let InstrCodegenOp::Load(load) = instr else {
            return false;
        };
        if load.name.location == NameLocation::RuntimeName && load.name.id_str() == "NONE" {
            return true;
        }
        let Some(constant_index) = load.name.location.as_constant() else {
            return false;
        };
        matches!(
            self.module.module_constants.get(constant_index as usize),
            Some(InstrResolved::Load(load))
                if load.name.location == NameLocation::RuntimeName && load.name.id_str() == "NONE"
        )
    }

    fn field_value(&self, instr: &InstrCodegen) -> ConstructorFieldValue {
        match self.value_alias_readonly(instr) {
            Some(ValueAlias::Param {
                name,
                index,
                location,
            }) => ConstructorFieldValue::Param {
                name,
                index,
                location,
            },
            Some(ValueAlias::Local { name, location }) => {
                ConstructorFieldValue::Local { name, location }
            }
            Some(ValueAlias::Constant { description }) => {
                ConstructorFieldValue::Constant { description }
            }
            Some(ValueAlias::SelfObject) | None => ConstructorFieldValue::Other,
        }
    }

    fn value_alias_readonly(&self, instr: &InstrCodegen) -> Option<ValueAlias> {
        let InstrCodegenOp::Load(load) = instr else {
            return None;
        };
        if let Some(location) = load.name.location.as_local() {
            if let Some(alias) = self.aliases.get(&location) {
                return Some(alias.clone());
            }
        }
        if let Some(index) = self.function.params.param_index(load.name.id_str()) {
            return Some(ValueAlias::Param {
                name: load.name.id_str().to_string(),
                index,
                location: load.name.location.as_local(),
            });
        }
        match load.name.location {
            NameLocation::Local(location) => Some(ValueAlias::Local {
                name: load.name.id_str().to_string(),
                location: Some(location),
            }),
            NameLocation::Constant(_) => Some(ValueAlias::Constant {
                description: load.name.id_str().to_string(),
            }),
            NameLocation::GlobalName
            | NameLocation::Global(_)
            | NameLocation::RuntimeName
            | NameLocation::Cell(_) => None,
        }
    }

    fn instr_uses_self_or_alias(&self, instr: &InstrCodegen) -> bool {
        instr_uses_self(instr, self.self_name.as_str()) || self.instr_uses_self_alias(instr)
    }

    fn instr_uses_self_alias(&self, instr: &InstrCodegen) -> bool {
        instr_any(instr, |child| match child {
            InstrCodegenOp::Load(load) => load
                .name
                .location
                .as_local()
                .and_then(|location| self.aliases.get(&location))
                .is_some_and(|alias| matches!(alias, ValueAlias::SelfObject)),
            _ => false,
        })
    }
}

fn instr_uses_self(instr: &InstrCodegen, self_name: &str) -> bool {
    instr_any(instr, |child| match child {
        InstrCodegenOp::Load(load) => load.name.id_str() == self_name,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::{CallArgPositional, CallDirect};
    use crate::lower_python_to_blockpy_for_testing;

    fn lowered(source: &str) -> BlockPyModule<CodegenModuleShape> {
        lower_python_to_blockpy_for_testing(source)
            .expect("transform should succeed")
            .codegen_module
    }

    fn function_by_qualname<'a>(
        module: &'a BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> &'a BlockPyFunction<CodegenModuleShape> {
        module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing function {qualname}; got {module:?}"))
    }

    fn function_by_qualname_mut<'a>(
        module: &'a mut BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> &'a mut BlockPyFunction<CodegenModuleShape> {
        module
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing function {qualname}"))
    }

    fn rewrite_first_box_call_as_direct(module: &mut BlockPyModule<CodegenModuleShape>) {
        let constructor_id = function_by_qualname(module, "Box.__init__").function_id;
        let function = function_by_qualname_mut(module, "make");
        let Some(InstrCodegenOp::Store(store)) = function.blocks[0].body.first_mut() else {
            panic!("make should start with a store");
        };
        let InstrCodegenOp::Call(call) = store.value.as_ref() else {
            panic!("store value should start as a generic call");
        };
        let args: Vec<CallArgPositional<InstrCodegen>> = call
            .args
            .iter()
            .map(|arg| match arg {
                CallArgPositional::Positional(value) => {
                    CallArgPositional::Positional(value.clone())
                }
                CallArgPositional::Starred(value) => CallArgPositional::Starred(value.clone()),
            })
            .collect();
        *store.value = InstrCodegen::CallDirect(CallDirect::new(
            (*call.func).clone(),
            constructor_id,
            args,
            call.keywords.clone(),
        ));
    }

    #[test]
    fn summarizes_field_only_constructor_as_non_escaping() {
        let module = lowered(
            r#"
class IterRange:
    def __init__(self, start, stop, step, /):
        self.current = start
        self.stop = stop
        self.step = step
"#,
        );
        let function = function_by_qualname(&module, "IterRange.__init__");
        let escapes = summarize_module_escapes(&module);
        let summary = escapes
            .non_escaping_constructor(function.function_id)
            .unwrap_or_else(|| {
                panic!(
                    "constructor should not escape self: constants={:#?} function={function:#?}",
                    module.module_constants
                )
            })
            .clone();

        let fields = summary
            .field_stores
            .iter()
            .map(|store| store.field_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(fields, ["current", "stop", "step"]);
        assert_eq!(
            summary.field_stores[0].value,
            ConstructorFieldValue::Param {
                name: "start".to_string(),
                index: 1,
                location: Some(LocalLocation(1)),
            }
        );
        assert_eq!(
            escapes
                .straightline_field_initializer(function.function_id)
                .unwrap_or_else(|| panic!(
                    "straightline field initializer should be summarized: function={function:#?}"
                ))
                .field_stores,
            summary.field_stores
        );
    }

    #[test]
    fn rejects_constructor_that_passes_self_to_unknown_call() {
        let module = lowered(
            r#"
def leak(value):
    pass

class Box:
    def __init__(self, value):
        leak(self)
        self.value = value
"#,
        );
        let function = function_by_qualname(&module, "Box.__init__");
        assert!(summarize_module_escapes(&module)
            .non_escaping_constructor(function.function_id)
            .is_none());
    }

    #[test]
    fn non_escaping_constructor_with_control_flow_is_not_a_field_initializer() {
        let module = lowered(
            r#"
class Box:
    def __init__(self, value):
        if value is None:
            value = 0
        self.value = value
"#,
        );
        let function = function_by_qualname(&module, "Box.__init__");
        let escapes = summarize_module_escapes(&module);
        assert!(escapes
            .non_escaping_constructor(function.function_id)
            .is_some());
        assert!(escapes
            .straightline_field_initializer(function.function_id)
            .is_none());
    }

    #[test]
    fn rejects_constructor_that_returns_self() {
        let module = lowered(
            r#"
class Box:
    def __init__(self, value):
        self.value = value
        return self
"#,
        );
        let function = function_by_qualname(&module, "Box.__init__");
        assert!(summarize_module_escapes(&module)
            .non_escaping_constructor(function.function_id)
            .is_none());
    }

    #[test]
    fn summarizes_local_constructor_allocation_used_for_field_read() {
        let mut module = lowered(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def make(x):
    box = Box(x)
    return box.value
"#,
        );
        rewrite_first_box_call_as_direct(&mut module);
        let make_id = function_by_qualname(&module, "make").function_id;

        let escapes = summarize_module_escapes(&module);
        let allocations = &escapes
            .function(make_id)
            .expect("make should have an escape summary")
            .non_escaping_constructor_allocations;

        assert_eq!(allocations.len(), 1);
        assert_eq!(allocations[0].local_name, "box");
        assert_eq!(
            allocations[0].field_reads,
            vec![ConstructorFieldAccess {
                field_name: "value".to_string()
            }]
        );
        assert!(allocations[0].field_writes.is_empty());
    }

    #[test]
    fn rejects_local_constructor_allocation_returned_directly() {
        let mut module = lowered(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def make(x):
    box = Box(x)
    return box
"#,
        );
        rewrite_first_box_call_as_direct(&mut module);
        let make_id = function_by_qualname(&module, "make").function_id;

        let escapes = summarize_module_escapes(&module);

        assert!(escapes
            .function(make_id)
            .expect("make should have an escape summary")
            .non_escaping_constructor_allocations
            .is_empty());
    }
}
