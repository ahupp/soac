use crate::passes::{BlockPyModuleShape, InstrBlockPy};
use soac_core::block_py::PrettyPrint;
use soac_core::block_py::literal::Literal;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, BlockTerm, ConstantExpr, LocalLocation, NameLike, NameLocation,
    RuntimeName, instr_any,
};
use std::collections::HashMap;

#[derive(
    Clone, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct EscapeSummaryModule {
    pub functions: HashMap<soac_core::block_py::RuntimeFunctionId, FunctionEscapeSummary>,
}

impl EscapeSummaryModule {
    pub fn function(
        &self,
        function_id: soac_core::block_py::RuntimeFunctionId,
    ) -> Option<&FunctionEscapeSummary> {
        self.functions.get(&function_id)
    }

    pub fn non_escaping_constructor(
        &self,
        function_id: soac_core::block_py::RuntimeFunctionId,
    ) -> Option<&NonEscapingConstructorSummary> {
        self.function(function_id)
            .and_then(|summary| summary.non_escaping_constructor.as_ref())
    }

    pub fn straightline_field_initializer(
        &self,
        function_id: soac_core::block_py::RuntimeFunctionId,
    ) -> Option<&FieldInitializerConstructorSummary> {
        self.function(function_id)
            .and_then(|summary| summary.straightline_field_initializer.as_ref())
    }

    pub fn remap_function_ids(
        &mut self,
        remap: impl Fn(soac_core::block_py::RuntimeFunctionId) -> soac_core::block_py::RuntimeFunctionId
        + Copy,
    ) {
        self.functions = std::mem::take(&mut self.functions)
            .into_iter()
            .map(|(function_id, summary)| (remap(function_id), summary))
            .collect();
    }
}

impl PrettyPrint for EscapeSummaryModule {
    fn fmt_pretty(&self, printer: &mut soac_core::block_py::PrettyPrinter<'_>) -> std::fmt::Result {
        let mut function_ids = self.functions.keys().copied().collect::<Vec<_>>();
        function_ids.sort_by_key(|function_id| function_id.to_packed_runtime_u64());
        let mut out = String::new();
        for function_id in function_ids {
            let summary = self
                .functions
                .get(&function_id)
                .expect("function id was collected from this summary map");
            if summary.non_escaping_constructor.is_none()
                && summary.straightline_field_initializer.is_none()
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
        }
        if out.is_empty() {
            std::fmt::Write::write_str(printer, "; no constructor escape summaries\n")
        } else {
            std::fmt::Write::write_str(printer, &out)
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

pub fn summarize_module_escapes(module: &BlockPyModule<BlockPyModuleShape>) -> EscapeSummaryModule {
    let functions = module
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
                },
            )
        })
        .collect::<HashMap<_, _>>();
    EscapeSummaryModule { functions }
}

fn summarize_non_escaping_constructor(
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
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
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
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

pub fn straightline_field_initializer_rejection_reason(
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> Option<String> {
    summarize_constructor_with_mode_result(
        module,
        function,
        ConstructorSummaryMode::StraightlineFieldInitializer,
    )
    .err()
}

fn summarize_constructor_with_mode(
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    mode: ConstructorSummaryMode,
) -> Option<ConstructorSummary> {
    summarize_constructor_with_mode_result(module, function, mode).ok()
}

fn summarize_constructor_with_mode_result(
    module: &BlockPyModule<BlockPyModuleShape>,
    function: &BlockPyFunction<BlockPyModuleShape>,
    mode: ConstructorSummaryMode,
) -> Result<ConstructorSummary, String> {
    if !function.names.qualname.ends_with(".__init__") {
        return Err("function qualname is not a constructor __init__".to_string());
    }
    let self_param = function
        .params
        .params
        .first()
        .ok_or_else(|| "constructor has no self parameter".to_string())?;
    if self_param.name != "self" {
        return Err(format!(
            "constructor first parameter is {}, not self",
            self_param.name
        ));
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
    summary.finish()
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
    module: &'a BlockPyModule<BlockPyModuleShape>,
    function: &'a BlockPyFunction<BlockPyModuleShape>,
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

    fn scan_instr(&mut self, instr: &InstrBlockPy) {
        match instr {
            InstrBlockPy::Store(store) => {
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
            InstrBlockPy::Del(del) => {
                if let Some(location) = del.name.location.as_local() {
                    self.aliases.remove(&location);
                }
                if del.name.id_str() == self.self_name {
                    self.reject("deleted self");
                }
            }
            InstrBlockPy::SetAttr(setattr) if self.is_self_alias(&setattr.value) => {
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

    fn scan_term(&mut self, term: &BlockTerm<InstrBlockPy>) {
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

    fn finish(self) -> Result<ConstructorSummary, String> {
        if let Some(reason) = self.rejected {
            return Err(reason);
        }
        if self.field_stores.is_empty() {
            return Err("constructor has no self field stores".to_string());
        }
        Ok(ConstructorSummary {
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

    fn is_self_alias(&mut self, instr: &InstrBlockPy) -> bool {
        matches!(self.value_alias(instr), Some(ValueAlias::SelfObject))
    }

    fn value_alias(&mut self, instr: &InstrBlockPy) -> Option<ValueAlias> {
        let InstrBlockPy::Load(load) = instr else {
            return None;
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
            | NameLocation::RuntimeName(_)
            | NameLocation::Cell(_) => None,
        }
    }

    fn constant_string(&self, instr: &InstrBlockPy) -> Option<String> {
        let InstrBlockPy::Load(load) = instr else {
            return None;
        };
        let constant_index = load.name.location.as_constant()? as usize;
        match self.module.module_constants.get(constant_index)? {
            ConstantExpr::Literal(value) => match value.as_literal() {
                Literal::StringLiteral(value) => Some(value.value.clone()),
                Literal::BytesLiteral(_) | Literal::NumberLiteral(_) => None,
            },
            _ => None,
        }
    }

    fn is_runtime_none(&self, instr: &InstrBlockPy) -> bool {
        let InstrBlockPy::Load(load) = instr else {
            return false;
        };
        if load.name.id_str() == "NONE"
            && matches!(
                load.name.location,
                NameLocation::RuntimeName(_) | NameLocation::GlobalName | NameLocation::Global(_)
            )
        {
            return true;
        }
        if load.name.is_runtime_symbol("NONE") {
            return true;
        }
        let Some(constant_index) = load.name.location.as_constant() else {
            return false;
        };
        matches!(
            self.module.module_constants.get(constant_index as usize),
            Some(ConstantExpr::RuntimeName(RuntimeName::None))
        )
    }

    fn field_value(&self, instr: &InstrBlockPy) -> ConstructorFieldValue {
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

    fn value_alias_readonly(&self, instr: &InstrBlockPy) -> Option<ValueAlias> {
        let InstrBlockPy::Load(load) = instr else {
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
            | NameLocation::RuntimeName(_)
            | NameLocation::Cell(_) => None,
        }
    }

    fn instr_uses_self_or_alias(&self, instr: &InstrBlockPy) -> bool {
        instr_uses_self(instr, self.self_name.as_str()) || self.instr_uses_self_alias(instr)
    }

    fn instr_uses_self_alias(&self, instr: &InstrBlockPy) -> bool {
        instr_any(instr, |child| match child {
            InstrBlockPy::Load(load) => load
                .name
                .location
                .as_local()
                .and_then(|location| self.aliases.get(&location))
                .is_some_and(|alias| matches!(alias, ValueAlias::SelfObject)),
            _ => false,
        })
    }
}

fn instr_uses_self(instr: &InstrBlockPy, self_name: &str) -> bool {
    instr_any(instr, |child| match child {
        InstrBlockPy::Load(load) => load.name.id_str() == self_name,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_core::block_py::ModuleNameGen;
    use soac_core::pass_tracker::RecordingPassTracker;
    use soac_lowering::{
        LoweringOptions, lower_python_to_blockpy_for_testing,
        lower_python_to_blockpy_with_tracker_and_options,
    };

    fn lowered(source: &str) -> BlockPyModule<BlockPyModuleShape> {
        lower_python_to_blockpy_for_testing(source)
            .expect("transform should succeed")
            .blockpy_module
    }

    fn lowered_with_runtime_names_as_globals(source: &str) -> BlockPyModule<BlockPyModuleShape> {
        lower_python_to_blockpy_with_tracker_and_options(
            source,
            ModuleNameGen::new(0),
            RecordingPassTracker::new(),
            LoweringOptions {
                runtime_names_as_globals: true,
            },
        )
        .expect("transform should succeed")
        .blockpy_module
    }

    fn function_by_qualname<'a>(
        module: &'a BlockPyModule<BlockPyModuleShape>,
        qualname: &str,
    ) -> &'a BlockPyFunction<BlockPyModuleShape> {
        module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing function {qualname}; got {module:?}"))
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
    fn summarizes_runtime_iterrange_init_as_field_initializer() {
        let module = lowered(include_str!("../../../../soac_py/src/soac/runtime.py"));
        let function = function_by_qualname(&module, "IterRange.__init__");
        let escapes = summarize_module_escapes(&module);
        let summary = escapes
            .straightline_field_initializer(function.function_id)
            .unwrap_or_else(|| {
                panic!(
                    "runtime IterRange.__init__ should be summarized as a straightline field initializer:\nfunction={function:#?}\nescapes={}",
                    escapes.pretty_print()
                )
            });
        let fields = summary
            .field_stores
            .iter()
            .map(|store| store.field_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(fields, ["current", "stop", "step"]);
    }

    #[test]
    fn summarizes_runtime_iterrange_init_with_runtime_names_as_globals() {
        let module = lowered_with_runtime_names_as_globals(include_str!(
            "../../../../soac_py/src/soac/runtime.py"
        ));
        let function = function_by_qualname(&module, "IterRange.__init__");
        let escapes = summarize_module_escapes(&module);
        let summary = escapes
            .straightline_field_initializer(function.function_id)
            .unwrap_or_else(|| {
                panic!(
                    "runtime IterRange.__init__ should summarize when runtime helper loads are global names:\nfunction={function:#?}\nescapes={}",
                    escapes.pretty_print()
                )
            });
        let fields = summary
            .field_stores
            .iter()
            .map(|store| store.field_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(fields, ["current", "stop", "step"]);
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
        assert!(
            summarize_module_escapes(&module)
                .non_escaping_constructor(function.function_id)
                .is_none()
        );
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
        assert!(
            escapes
                .non_escaping_constructor(function.function_id)
                .is_some()
        );
        assert!(
            escapes
                .straightline_field_initializer(function.function_id)
                .is_none()
        );
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
        assert!(
            summarize_module_escapes(&module)
                .non_escaping_constructor(function.function_id)
                .is_none()
        );
    }
}
