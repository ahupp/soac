use super::{FunctionName, is_internal_symbol};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum BindingTarget {
    Local,
    ModuleGlobal,
    ClassNamespace,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum CellBindingKind {
    Owner,
    Capture,
}

#[derive(
    Debug, Clone, Copy, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum BindingKind {
    #[default]
    Local,
    Global,
    Cell(CellBindingKind),
}

#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct StorageLayout {
    pub freevars: Vec<ClosureSlot>,
    pub cellvars: Vec<ClosureSlot>,
    // Private activation state that survives suspension but is not a Python
    // lexical cell.
    pub preserved_slots: Vec<PreservedSlot>,
    pub stack_slots: Vec<String>,
}

impl StorageLayout {
    pub fn freevar_slot(&self, slot: u32) -> Option<&ClosureSlot> {
        self.freevars.get(slot as usize)
    }

    pub fn owned_slot(&self, slot: u32) -> Option<&ClosureSlot> {
        self.cellvars.get(slot as usize)
    }

    pub fn preserved_slot(&self, slot: u32) -> Option<&PreservedSlot> {
        self.preserved_slots.get(slot as usize)
    }

    pub fn has_freevar_storage_name(&self, storage_name: &str) -> bool {
        self.freevars
            .iter()
            .any(|slot| slot.storage_name == storage_name)
    }

    pub fn has_cellvar_storage_name(&self, storage_name: &str) -> bool {
        self.cellvars
            .iter()
            .any(|slot| slot.storage_name == storage_name)
    }

    pub fn has_storage_name(&self, storage_name: &str) -> bool {
        self.has_freevar_storage_name(storage_name)
            || self.has_cellvar_storage_name(storage_name)
            || self
                .preserved_slots
                .iter()
                .any(|slot| slot.storage_name == storage_name)
    }

    pub fn stack_slots(&self) -> &[String] {
        &self.stack_slots
    }

    pub fn set_stack_slots(&mut self, stack_slots: Vec<String>) {
        self.stack_slots = stack_slots;
    }

    pub fn ensure_stack_slot(&mut self, name: impl Into<String>) {
        let name = name.into();
        if self.stack_slots.iter().any(|existing| existing == &name) {
            return;
        }
        self.stack_slots.push(name);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClosureSlot {
    pub logical_name: String,
    pub storage_name: String,
    pub init: ClosureInit,
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PreservedSlot {
    pub logical_name: String,
    pub storage_name: String,
    pub init: ClosureInit,
    pub storage: PreservedSlotStorage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PreservedSlotStorage {
    // Raw Python-object pointer. Null is reserved for the future unbound state.
    PyObjectOrNull,
    // Raw machine integer for compiler-private always-initialized state.
    I64,
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ClosureInit {
    InheritedCapture,
    Parameter,
    EmptyCell,
    RuntimePcUnstarted,
    RuntimeAbruptKindFallthrough,
    RuntimeNone,
    Deferred,
}

#[derive(
    Debug, Clone, Copy, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum CallableScopeKind {
    #[default]
    Function,
    Class,
    Module,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ClassBodyFallback {
    Global,
    Cell,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum EffectiveBinding {
    Local,
    Global,
    Cell(CellBindingKind),
    ClassBody(ClassBodyFallback),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BindingPurpose {
    Load,
    Store,
}

#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CellCaptureBinding {
    pub logical_name: String,
    pub source_name: String,
}

#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CallableScopeInfo {
    pub names: FunctionName,
    pub scope_kind: CallableScopeKind,
    pub bindings: HashMap<String, BindingKind>,
    pub local_defs: HashSet<String>,
    pub cell_storage_names: HashMap<String, String>,
    pub cell_capture_source_names: HashMap<String, String>,
    pub owned_cell_source_names: HashSet<String>,
    pub scope_internal_names: HashSet<String>,
    pub type_param_names: HashSet<String>,
    pub effective_load_bindings: HashMap<String, EffectiveBinding>,
    pub effective_store_bindings: HashMap<String, EffectiveBinding>,
}

pub fn derive_effective_binding_for_name(
    name: &str,
    binding: BindingKind,
    scope_kind: CallableScopeKind,
    type_param_names: &HashSet<String>,
    purpose: BindingPurpose,
    honor_internal_name: bool,
) -> EffectiveBinding {
    if is_internal_symbol(name) && !honor_internal_name {
        return EffectiveBinding::Local;
    }
    match purpose {
        BindingPurpose::Load => match (scope_kind, binding) {
            (CallableScopeKind::Class, BindingKind::Cell(_)) => {
                EffectiveBinding::ClassBody(ClassBodyFallback::Cell)
            }
            (CallableScopeKind::Class, BindingKind::Local)
            | (CallableScopeKind::Class, BindingKind::Global) => {
                EffectiveBinding::ClassBody(ClassBodyFallback::Global)
            }
            (_, BindingKind::Global) => EffectiveBinding::Global,
            (_, BindingKind::Cell(kind)) => EffectiveBinding::Cell(kind),
            (_, BindingKind::Local) => EffectiveBinding::Local,
        },
        BindingPurpose::Store => {
            if scope_kind == CallableScopeKind::Class && type_param_names.contains(name) {
                return match binding {
                    BindingKind::Local => EffectiveBinding::Local,
                    BindingKind::Global => EffectiveBinding::Global,
                    BindingKind::Cell(kind) => EffectiveBinding::Cell(kind),
                };
            }
            match (scope_kind, binding) {
                (CallableScopeKind::Class, BindingKind::Local) => {
                    EffectiveBinding::ClassBody(ClassBodyFallback::Global)
                }
                (_, BindingKind::Global) => EffectiveBinding::Global,
                (_, BindingKind::Cell(kind)) => EffectiveBinding::Cell(kind),
                (_, BindingKind::Local) => EffectiveBinding::Local,
            }
        }
    }
}

fn cell_name(name: &str) -> String {
    format!("_dp_cell_{name}")
}

impl CallableScopeInfo {
    pub fn honors_internal_binding(&self, name: &str) -> bool {
        !is_internal_symbol(name) || self.scope_internal_names.contains(name)
    }

    pub fn binding_kind(&self, name: &str) -> Option<BindingKind> {
        self.bindings.get(name).copied()
    }

    pub fn has_local_def(&self, name: &str) -> bool {
        self.local_defs.contains(name)
    }

    pub fn effective_binding(
        &self,
        name: &str,
        purpose: BindingPurpose,
    ) -> Option<EffectiveBinding> {
        match purpose {
            BindingPurpose::Load => self.effective_load_bindings.get(name).copied(),
            BindingPurpose::Store => self.effective_store_bindings.get(name).copied(),
        }
    }

    pub fn insert_binding(
        &mut self,
        name: impl Into<String>,
        binding: BindingKind,
        honor_internal_name: bool,
        cell_storage_name: Option<String>,
    ) {
        self.insert_binding_with_cell_names(
            name,
            binding,
            honor_internal_name,
            cell_storage_name.clone(),
            cell_storage_name,
        );
    }

    pub fn insert_binding_with_cell_names(
        &mut self,
        name: impl Into<String>,
        binding: BindingKind,
        honor_internal_name: bool,
        cell_storage_name: Option<String>,
        cell_capture_source_name: Option<String>,
    ) {
        let name = name.into();
        self.bindings.insert(name.clone(), binding);
        if let Some(cell_storage_name) = cell_storage_name {
            self.cell_storage_names
                .insert(name.clone(), cell_storage_name);
        }
        if let Some(cell_capture_source_name) = cell_capture_source_name {
            self.cell_capture_source_names
                .insert(name.clone(), cell_capture_source_name);
        }
        if honor_internal_name {
            self.scope_internal_names.insert(name.clone());
        }
        self.effective_load_bindings.insert(
            name.clone(),
            derive_effective_binding_for_name(
                name.as_str(),
                binding,
                self.scope_kind,
                &self.type_param_names,
                BindingPurpose::Load,
                honor_internal_name,
            ),
        );
        self.effective_store_bindings.insert(
            name.clone(),
            derive_effective_binding_for_name(
                name.as_str(),
                binding,
                self.scope_kind,
                &self.type_param_names,
                BindingPurpose::Store,
                honor_internal_name,
            ),
        );
    }

    pub fn resolved_load_binding_kind(&self, name: &str) -> BindingKind {
        if let Some(binding) = self.binding_kind(name) {
            if self.honors_internal_binding(name) {
                return binding;
            }
        }
        if is_internal_symbol(name) {
            return BindingKind::Local;
        }
        BindingKind::Global
    }

    pub fn is_cell_binding(&self, name: &str) -> bool {
        matches!(self.binding_kind(name), Some(BindingKind::Cell(_)))
    }

    pub fn cell_storage_name(&self, name: &str) -> String {
        self.cell_storage_names
            .get(name)
            .cloned()
            .unwrap_or_else(|| cell_name(name))
    }

    pub fn cell_capture_source_name(&self, name: &str) -> String {
        self.cell_capture_source_names
            .get(name)
            .cloned()
            .unwrap_or_else(|| cell_name(name))
    }

    pub fn cell_ref_source_name(&self, name: &str) -> String {
        if self.is_cell_binding(name) {
            self.cell_storage_name(name)
        } else {
            self.cell_capture_source_name(name)
        }
    }

    pub fn logical_name_for_cell_capture_source(&self, storage_name: &str) -> Option<String> {
        self.cell_capture_source_names
            .iter()
            .find_map(|(logical_name, current_storage_name)| {
                (current_storage_name == storage_name).then(|| logical_name.clone())
            })
            .or_else(|| self.logical_name_for_cell_storage(storage_name))
    }

    pub fn binding_target_for_name(&self, name: &str, purpose: BindingPurpose) -> BindingTarget {
        if let Some(binding) = self.effective_binding(name, purpose) {
            if self.honors_internal_binding(name) {
                return match binding {
                    EffectiveBinding::Global => BindingTarget::ModuleGlobal,
                    EffectiveBinding::ClassBody(_) => BindingTarget::ClassNamespace,
                    EffectiveBinding::Local | EffectiveBinding::Cell(_) => BindingTarget::Local,
                };
            }
        }
        if is_internal_symbol(name) {
            return BindingTarget::Local;
        }
        match self.effective_binding(name, purpose) {
            Some(EffectiveBinding::Global) => BindingTarget::ModuleGlobal,
            Some(EffectiveBinding::ClassBody(_)) => BindingTarget::ClassNamespace,
            _ => BindingTarget::Local,
        }
    }

    pub fn owned_cell_storage_names(&self) -> HashSet<String> {
        let mut names = self
            .bindings
            .iter()
            .filter_map(|(name, binding)| {
                matches!(binding, BindingKind::Cell(CellBindingKind::Owner))
                    .then(|| self.cell_storage_name(name.as_str()))
            })
            .collect::<HashSet<_>>();
        names.extend(self.owned_cell_source_names.iter().cloned());
        names
    }

    pub fn captured_cell_logical_names(&self) -> Vec<String> {
        let mut names = self
            .bindings
            .iter()
            .filter_map(|(name, binding)| {
                matches!(binding, BindingKind::Cell(CellBindingKind::Capture)).then(|| name.clone())
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    pub fn captured_cell_bindings(&self) -> Vec<CellCaptureBinding> {
        self.captured_cell_logical_names()
            .into_iter()
            .map(|logical_name| CellCaptureBinding {
                source_name: self.cell_capture_source_name(logical_name.as_str()),
                logical_name,
            })
            .collect()
    }

    pub fn local_cell_storage_names(&self) -> HashSet<String> {
        if !matches!(self.scope_kind, CallableScopeKind::Function) {
            return HashSet::new();
        }
        self.owned_cell_storage_names()
    }

    pub fn logical_name_for_cell_storage(&self, storage_name: &str) -> Option<String> {
        if self.owned_cell_source_names.contains(storage_name) {
            return Some(storage_name.to_string());
        }
        if let Some(logical_name) = storage_name.strip_prefix("_dp_cell_") {
            return Some(logical_name.to_string());
        }
        self.cell_storage_names
            .iter()
            .find_map(|(logical_name, current_storage_name)| {
                (current_storage_name == storage_name).then(|| logical_name.clone())
            })
    }
}
