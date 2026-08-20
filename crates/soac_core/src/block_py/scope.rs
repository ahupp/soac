use super::{FunctionName, LocalLocation, is_internal_symbol};
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

/// Private preserved activation controls. Names are not semantic identities.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum GeneratorControlRole {
    ProgramCounter,
    IsClosed,
    Delegate,
}

impl GeneratorControlRole {
    pub const ALL: [Self; 3] = [Self::ProgramCounter, Self::IsClosed, Self::Delegate];

    pub fn initial_value(self) -> ClosureInit {
        match self {
            Self::ProgramCounter => ClosureInit::RuntimePcUnstarted,
            Self::IsClosed => ClosureInit::RuntimeZero,
            Self::Delegate => ClosureInit::RuntimeNone,
        }
    }

    pub fn storage(self) -> PreservedSlotStorage {
        match self {
            Self::ProgramCounter | Self::IsClosed => PreservedSlotStorage::I64,
            Self::Delegate => PreservedSlotStorage::PyObjectOrNull,
        }
    }
}

/// Semantic positions of the existing private four/five-argument resume ABI.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum GeneratorResumeParamRole {
    SelfValue,
    StateValue,
    SendValue,
    ResumeExc,
    TransportSent,
}

impl GeneratorResumeParamRole {
    pub fn for_kind(kind: super::FunctionKind) -> &'static [Self] {
        match kind {
            super::FunctionKind::Function => &[],
            super::FunctionKind::Generator | super::FunctionKind::Coroutine => &[
                Self::SelfValue,
                Self::StateValue,
                Self::SendValue,
                Self::ResumeExc,
            ],
            super::FunctionKind::AsyncGenerator => &[
                Self::SelfValue,
                Self::StateValue,
                Self::SendValue,
                Self::ResumeExc,
                Self::TransportSent,
            ],
        }
    }

    pub fn is_preserved_owner(self) -> bool {
        matches!(self, Self::SelfValue | Self::StateValue)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct GeneratorResumeParamBinding {
    pub role: GeneratorResumeParamRole,
    pub name: String,
}

/// Executable resume parameters only; public source arguments remain unchanged.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct GeneratorResumeAbi {
    pub params: Vec<GeneratorResumeParamBinding>,
}

impl GeneratorResumeAbi {
    pub fn parameter(&self, role: GeneratorResumeParamRole) -> Option<&str> {
        let mut matches = self
            .params
            .iter()
            .filter(|parameter| parameter.role == role);
        let parameter = matches.next()?;
        matches.next().is_none().then_some(parameter.name.as_str())
    }

    pub fn role_for_name(&self, name: &str) -> Option<GeneratorResumeParamRole> {
        let mut matches = self
            .params
            .iter()
            .filter(|parameter| parameter.name == name);
        let parameter = matches.next()?;
        matches.next().is_none().then_some(parameter.role)
    }

    pub fn validate(
        &self,
        kind: super::FunctionKind,
        body_params: &super::ParamSpec,
    ) -> Result<(), String> {
        let roles = GeneratorResumeParamRole::for_kind(kind);
        if roles.is_empty() || self.params.len() != roles.len() || body_params.len() != roles.len()
        {
            return Err(
                "generator resume ABI disagrees with executable function kind/arity".into(),
            );
        }
        let mut names = HashSet::new();
        for ((binding, parameter), expected_role) in
            self.params.iter().zip(body_params.iter()).zip(roles)
        {
            if binding.role != *expected_role
                || binding.name != parameter.name
                || parameter.kind != super::ParamKind::PosOnly
                || parameter.has_default
                || !names.insert(binding.name.as_str())
            {
                return Err(
                    "generator resume ABI has duplicate, redirected, or reordered parameters"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

/// A producer-selected control value's resolved physical slot. This remains
/// valid after optimization removes or clones the original block parameters.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct ResolvedBlockParameterRole {
    pub location: super::NameLocation,
    pub role: super::BlockParamRole,
}

/// Private owning storage for an evaluated expression operand. Unlike a source
/// binding, its lifetime ends when it is consumed, including after suspension.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum OperandLocation {
    Local(LocalLocation),
    Preserved(super::PreservedLocation),
}

impl OperandLocation {
    pub const fn from_name_location(location: super::NameLocation) -> Option<Self> {
        match location {
            super::NameLocation::Local(location) => Some(Self::Local(location)),
            super::NameLocation::Preserved(location) => Some(Self::Preserved(location)),
            _ => None,
        }
    }

    pub const fn name_location(self) -> super::NameLocation {
        match self {
            Self::Local(location) => super::NameLocation::Local(location),
            Self::Preserved(location) => super::NameLocation::Preserved(location),
        }
    }

    pub const fn local_location(self) -> Option<LocalLocation> {
        match self {
            Self::Local(location) => Some(location),
            Self::Preserved(_) => None,
        }
    }

    pub const fn preserved_location(self) -> Option<super::PreservedLocation> {
        match self {
            Self::Preserved(location) => Some(location),
            Self::Local(_) => None,
        }
    }
}

impl From<LocalLocation> for OperandLocation {
    fn from(location: LocalLocation) -> Self {
        Self::Local(location)
    }
}

impl From<super::PreservedLocation> for OperandLocation {
    fn from(location: super::PreservedLocation) -> Self {
        Self::Preserved(location)
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct StorageLayout {
    pub class_bindings: Option<super::ClassBindingProjection>,
    pub block_parameter_roles: Vec<ResolvedBlockParameterRole>,
    pub generator_resume_abi: Option<GeneratorResumeAbi>,
    pub freevars: Vec<ClosureSlot>,
    pub cellvars: Vec<ClosureSlot>,
    // Per-instance activation state that survives suspension. Most slots hold
    // private locals; PyCellObject slots own lexical cells that the removed
    // generator factory frame used to own.
    pub preserved_slots: Vec<PreservedSlot>,
    pub stack_slots: Vec<String>,
    /// Compiler-owned expression values, in acquisition order. Unlike source
    /// locals, these own operand-stack lifetimes and unwind newest first.
    pub expression_temporaries: Vec<OperandLocation>,
}

impl StorageLayout {
    pub fn generator_control_slot(
        &self,
        role: GeneratorControlRole,
    ) -> Option<super::PreservedLocation> {
        let mut matches = self
            .preserved_slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.generator_control == Some(role));
        let (index, _) = matches.next()?;
        matches
            .next()
            .is_none()
            .then_some(super::PreservedLocation(u32::try_from(index).ok()?))
    }

    pub fn generator_resume_parameter(&self, role: GeneratorResumeParamRole) -> Option<&str> {
        self.generator_resume_abi.as_ref()?.parameter(role)
    }

    pub fn generator_resume_local(&self, role: GeneratorResumeParamRole) -> Option<LocalLocation> {
        let name = self.generator_resume_parameter(role)?;
        let mut matches = self
            .stack_slots
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.as_str() == name);
        let (index, _) = matches.next()?;
        matches
            .next()
            .is_none()
            .then_some(LocalLocation(u32::try_from(index).ok()?))
    }

    /// Validate roles before any runtime or optimizer consumer selects a slot.
    /// An unmarked source name can never satisfy a control/ABI obligation.
    pub fn validate_generator_roles(&self) -> Result<(), String> {
        let mut controls = HashSet::new();
        for slot in &self.preserved_slots {
            if let Some(role) = slot.generator_control {
                if !controls.insert(role) {
                    return Err("duplicate generator preserved-control role".into());
                }
                if slot.storage != role.storage() || slot.init != role.initial_value() {
                    return Err(
                        "generator preserved-control role has an incompatible representation/init"
                            .into(),
                    );
                }
            }
        }
        if (!controls.is_empty() || self.generator_resume_abi.is_some())
            && controls.len() != GeneratorControlRole::ALL.len()
        {
            return Err("generator layout does not define all preserved-control roles".into());
        }
        if let Some(abi) = &self.generator_resume_abi {
            let mut roles = HashSet::new();
            let mut names = HashSet::new();
            for binding in &abi.params {
                if !roles.insert(binding.role) || !names.insert(binding.name.as_str()) {
                    return Err("duplicate generator resume parameter role/name".into());
                }
                if self.preserved_slots.iter().any(|slot| {
                    slot.logical_name == binding.name || slot.storage_name == binding.name
                }) {
                    return Err("generator resume ABI aliases preserved state".into());
                }
                if self.generator_resume_local(binding.role).is_none() {
                    return Err("generator resume parameter has no unique local storage".into());
                }
            }
        }
        Ok(())
    }

    pub fn block_parameter_roles_at(
        &self,
        location: super::NameLocation,
    ) -> impl Iterator<Item = super::BlockParamRole> + '_ {
        self.block_parameter_roles
            .iter()
            .filter(move |binding| binding.location == location)
            .map(|binding| binding.role)
    }

    pub fn record_block_parameter_role(
        &mut self,
        location: super::NameLocation,
        role: super::BlockParamRole,
    ) {
        let binding = ResolvedBlockParameterRole { location, role };
        if !self.block_parameter_roles.contains(&binding) {
            self.block_parameter_roles.push(binding);
        }
    }

    pub fn validate_block_parameter_declarations<'a>(
        &self,
        parameters: impl IntoIterator<Item = &'a super::BlockParam>,
    ) -> Result<(), String> {
        use super::{BlockParamRole, NameLocation};
        for parameter in parameters {
            if parameter.role == BlockParamRole::Value {
                continue;
            }
            let mut slots = self
                .stack_slots()
                .iter()
                .enumerate()
                .filter(|(_, name)| *name == &parameter.name);
            let (slot, _) = slots
                .next()
                .ok_or("control parameter has no resolved local")?;
            if slots.next().is_some() {
                return Err("control parameter has ambiguous resolved local storage".into());
            }
            let location =
                NameLocation::local(u32::try_from(slot).map_err(|_| "local index overflow")?);
            if !self.block_parameter_roles_at(location).any(|role| {
                role == parameter.role
                    || matches!(
                        (role, parameter.role),
                        (
                            BlockParamRole::Exception,
                            BlockParamRole::EnclosingException
                        ) | (
                            BlockParamRole::EnclosingException,
                            BlockParamRole::Exception
                        ) | (
                            BlockParamRole::AbruptPayload,
                            BlockParamRole::EnclosingAbruptPayload
                        ) | (
                            BlockParamRole::EnclosingAbruptPayload,
                            BlockParamRole::AbruptPayload
                        )
                    )
            }) {
                return Err("control parameter lost its resolved physical role".into());
            }
        }
        Ok(())
    }

    pub fn validate_block_parameter_roles(&self) -> Result<(), String> {
        use super::{BlockParamRole, NameLocation};
        fn compatible(left: BlockParamRole, right: BlockParamRole) -> bool {
            left == right
                || matches!(
                    (left, right),
                    (
                        BlockParamRole::Exception,
                        BlockParamRole::EnclosingException
                    ) | (
                        BlockParamRole::EnclosingException,
                        BlockParamRole::Exception
                    ) | (
                        BlockParamRole::AbruptPayload,
                        BlockParamRole::EnclosingAbruptPayload
                    ) | (
                        BlockParamRole::EnclosingAbruptPayload,
                        BlockParamRole::AbruptPayload
                    )
                )
        }

        let mut seen = HashSet::new();
        for binding in &self.block_parameter_roles {
            if binding.role == BlockParamRole::Value || !seen.insert(*binding) {
                return Err("duplicate or non-control resolved block-parameter role".into());
            }
            match binding.location {
                NameLocation::Local(location)
                    if (location.slot() as usize) < self.stack_slots.len() => {}
                NameLocation::Preserved(location) => {
                    let slot = self
                        .preserved_slot(location.slot())
                        .ok_or("block-parameter role has an absent preserved slot")?;
                    let valid_representation = match binding.role {
                        BlockParamRole::AbruptKind => {
                            slot.storage == PreservedSlotStorage::I64
                                && slot.init == ClosureInit::RuntimeAbruptKindFallthrough
                        }
                        BlockParamRole::Exception
                        | BlockParamRole::EnclosingException
                        | BlockParamRole::AbruptPayload
                        | BlockParamRole::EnclosingAbruptPayload => {
                            slot.storage == PreservedSlotStorage::PyObjectOrNull
                        }
                        BlockParamRole::Value | BlockParamRole::GeneratorResume(_) => false,
                    };
                    if !valid_representation || slot.generator_control.is_some() {
                        return Err(
                            "block-parameter role has an incompatible preserved representation"
                                .into(),
                        );
                    }
                }
                _ => {
                    return Err(
                        "block-parameter role requires an allocated local or preserved slot".into(),
                    );
                }
            }
            if self.block_parameter_roles.iter().any(|other| {
                other.location == binding.location && !compatible(binding.role, other.role)
            }) {
                return Err("incompatible control roles share a physical slot".into());
            }
            if let Some(class) = &self.class_bindings {
                let aliases_current = class.slots.iter().any(|slot| {
                    slot.storage
                        .raw_local(self)
                        .is_some_and(|local| binding.location == NameLocation::Local(local))
                });
                if binding.location == NameLocation::Local(class.namespace) || aliases_current {
                    return Err("block-parameter transport aliases a native class owner".into());
                }
            }
            if let NameLocation::Local(location) = binding.location {
                let storage = &self.stack_slots[location.slot() as usize];
                if self
                    .cellvars
                    .iter()
                    .chain(&self.freevars)
                    .any(|cell| &cell.storage_name == storage)
                {
                    return Err("block-parameter transport aliases a lexical cell owner".into());
                }
            }
        }
        Ok(())
    }

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
        self.block_parameter_roles.clear();
        // Reassigning local storage does not reassign the suspended payload.
        self.expression_temporaries
            .retain(|location| matches!(location, OperandLocation::Preserved(_)));
        // Physical class projections must be rebuilt after slot reassignment.
        self.class_bindings = None;
    }

    pub fn ensure_stack_slot(&mut self, name: impl Into<String>) {
        let name = name.into();
        if self.stack_slots.iter().any(|existing| existing == &name) {
            return;
        }
        self.stack_slots.push(name);
    }

    pub fn mark_expression_temporary(&mut self, location: impl Into<OperandLocation>) {
        let location = location.into();
        assert!(
            match location {
                OperandLocation::Local(location) =>
                    (location.slot() as usize) < self.stack_slots.len(),
                OperandLocation::Preserved(location) =>
                    (location.slot() as usize) < self.preserved_slots.len(),
            },
            "expression temporary must have allocated owning storage"
        );
        if !self.is_expression_temporary(location) {
            self.expression_temporaries.push(location);
        }
    }

    pub fn is_expression_temporary(&self, location: impl Into<OperandLocation>) -> bool {
        self.expression_temporaries.contains(&location.into())
    }

    /// Expression operands are released before frame locals, in reverse
    /// acquisition order. Keep the existing source-local order unchanged.
    pub fn local_cleanup_order_key(&self, location: LocalLocation) -> (bool, u32) {
        match self
            .expression_temporaries
            .iter()
            .position(|slot| *slot == OperandLocation::Local(location))
        {
            Some(rank) => (
                false,
                u32::MAX - u32::try_from(rank).expect("temporary rank fits u32"),
            ),
            None => (true, location.slot()),
        }
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
    pub generator_control: Option<GeneratorControlRole>,
    pub logical_name: String,
    pub storage_name: String,
    pub init: ClosureInit,
    pub storage: PreservedSlotStorage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PreservedSlotStorage {
    // Raw Python-object pointer. Null is reserved for the future unbound state.
    PyObjectOrNull,
    // Raw Python cell object pointer for a generator-owned lexical cell.
    PyCellObject,
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
    RuntimeZero,
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
    pub projection: CellCaptureProjection,
}

/// How a creation site obtains an already selected lexical cell. A class
/// namespace receives its dictionary cell as an ordinary argument; taking a
/// reference to that argument's own storage would introduce an extra cell and
/// would no longer match the original annotation provider's closure.
#[derive(
    Debug, Clone, Copy, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum CellCaptureProjection {
    #[default]
    CellReference,
    CellObject,
}

/// The public native provider has one positional-only native format parameter.
/// Its body uses a distinct compiler binding so an annotation which refers
/// to the source name `format` still resolves in its original lexical scope.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    Eq,
    PartialEq,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum AnnotationProviderKind {
    #[default]
    Dictionary,
    TypeAliasValue,
    TypeParameterBound,
    TypeParameterConstraints,
    TypeParameterDefault,
}

impl AnnotationProviderKind {
    /// The pinned compiler renames the dictionary provider's synthetic
    /// parameter for inspection, but leaves type evaluators' `.format` intact.
    pub fn parameter_name(self) -> &'static str {
        match self {
            Self::Dictionary => "format",
            Self::TypeAliasValue
            | Self::TypeParameterBound
            | Self::TypeParameterConstraints
            | Self::TypeParameterDefault => ".format",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct AnnotationProviderScope {
    pub kind: AnnotationProviderKind,
    /// The original native start line selected before AST rewriting. Function
    /// dictionary providers start at def/async; class providers share the class
    /// code's first line, including its decorators. Lazy expressions retain
    /// their own source start.
    pub native_first_line: u32,
    /// Exact native annotation-scope location for a lazy type expression.
    /// It comes from the original AST, never from a generated helper name.
    pub native_range: Option<soac_contracts::SourceRange>,
    pub body_format_parameter: String,
    pub class_dictionary: Option<String>,
    pub class_dictionary_binding: Option<String>,
    pub conditional_annotations: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum TypeParameterScopeInputKind {
    PositionalDefaults,
    KeywordDefaults,
}

impl TypeParameterScopeInputKind {
    pub fn native_parameter_name(self) -> &'static str {
        match self {
            Self::PositionalDefaults => ".defaults",
            Self::KeywordDefaults => ".kwdefaults",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TypeParameterScopeInput {
    pub kind: TypeParameterScopeInputKind,
    pub body_parameter: String,
}

/// An explicitly lowered native generic-declaration scope. The source origin
/// owns its declaration identity; these fields describe its actual argument
/// and inherited class-dictionary projections, not an execution capability.
#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct TypeParameterScope {
    pub native_qualname: String,
    /// Full signed declaration span, including any decorators.
    pub native_range: soac_contracts::SourceRange,
    /// Exact native wrapper instruction span, from the original def/async/class
    /// header through the declaration end. Decorators only affect first_line.
    pub native_header_range: soac_contracts::SourceRange,
    pub native_first_line: u32,
    pub inputs: Vec<TypeParameterScopeInput>,
    pub class_dictionary: Option<String>,
    pub class_dictionary_binding: Option<String>,
}

/// How the explicit function-construction operand represents defaults.
/// NativeContainers preserves the two already evaluated native containers;
/// its meaning is never inferred from the shape of arbitrary Python values.
#[derive(
    Debug, Clone, Copy, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum FunctionDefaultsProjection {
    #[default]
    FlatValues,
    NativeContainers,
}

/// The semantic owner of a lexical cell. Equal names in different source
/// scopes are different bindings; a class dictionary does not shadow this key.
#[derive(
    Debug, Clone, Eq, PartialEq, Ord, PartialOrd, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct LexicalCellBinding {
    pub scope: soac_contracts::SourceIdentity,
    pub name: String,
}

/// One selected cell and the complete signed field leaves requiring it.
/// Indices refer to the verified module's canonical nominal-binding vector.
#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct LexicalCellCapture {
    pub binding: LexicalCellBinding,
    pub nominal_binding_indices: Vec<u32>,
}

/// A source function's native closure follows ordinary pre-seal replacement
/// and post-binder COPY_FREE_VARS semantics. Only otherwise uncaptured cells
/// use the separate compiler-owned environment. Namespace helpers always use
/// the latter through their exact one-use construction handle.
#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct LexicalCaptureProjection {
    pub cell: LexicalCellCapture,
    pub native_closure: Option<String>,
}

#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PrivateLexicalScope {
    pub creator: super::CallableSourceOrigin,
    pub captures: Vec<LexicalCaptureProjection>,
}

impl PrivateLexicalScope {
    pub fn private_captures(&self) -> impl Iterator<Item = &LexicalCellCapture> {
        self.captures
            .iter()
            .filter(|capture| capture.native_closure.is_none())
            .map(|capture| &capture.cell)
    }

    pub fn private_cell_index(&self, binding: &LexicalCellBinding) -> Option<u32> {
        self.private_captures()
            .position(|capture| &capture.binding == binding)
            .map(|index| u32::try_from(index).expect("private cell index fits source size"))
    }
}

/// Private, one-construction capture projection. This is not part of the
/// helper's public closure and does not authorize a class from source alone.
/// The MakeFunction operation supplies cells from this actual active producer
/// and pairs the fresh helper with its already-created namespace function.
#[derive(Debug, Clone, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassConstructionScope {
    pub producer: super::CallableSourceOrigin,
    pub namespace_function: super::RuntimeFunctionId,
    pub captures: Vec<LexicalCellCapture>,
}

#[derive(Debug, Clone, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CallableScopeInfo {
    pub class_bindings: Option<super::ClassBindingScope>,
    pub source_origin: Option<super::CallableSourceOrigin>,
    pub generator_expression_code: Option<super::GeneratorExpressionCode>,
    pub annotation_provider: Option<AnnotationProviderScope>,
    pub type_parameter_scope: Option<TypeParameterScope>,
    pub class_construction: Option<ClassConstructionScope>,
    /// Private forwarding/projection only, never native co_freevars or a
    /// callable argument. Its creation edge and signed leaves are validated.
    pub private_lexical: Option<PrivateLexicalScope>,
    pub creation_defaults: FunctionDefaultsProjection,
    pub names: FunctionName,
    pub scope_kind: CallableScopeKind,
    pub bindings: HashMap<String, BindingKind>,
    pub local_defs: HashSet<String>,
    pub cell_storage_names: HashMap<String, String>,
    pub cell_capture_source_names: HashMap<String, String>,
    pub cell_capture_projections: HashMap<String, CellCaptureProjection>,
    /// Compiler-selected value bindings for a cell whose native logical name
    /// can also occur in source. These aliases do not introduce extra cells.
    pub cell_value_aliases: HashMap<String, String>,
    pub owned_cell_source_names: HashSet<String>,
    pub scope_internal_names: HashSet<String>,
    pub type_param_names: HashSet<String>,
    pub effective_load_bindings: HashMap<String, EffectiveBinding>,
    pub effective_store_bindings: HashMap<String, EffectiveBinding>,
}

impl CallableScopeInfo {
    pub fn native_class_dictionary_binding(&self) -> Option<&str> {
        self.annotation_provider
            .as_ref()
            .and_then(|provider| provider.class_dictionary_binding.as_deref())
            .or_else(|| {
                self.type_parameter_scope
                    .as_ref()
                    .and_then(|scope| scope.class_dictionary_binding.as_deref())
            })
    }
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

    pub fn cell_capture_projection(&self, name: &str) -> CellCaptureProjection {
        self.cell_capture_projections
            .get(name)
            .copied()
            .unwrap_or_default()
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
                projection: self.cell_capture_projection(logical_name.as_str()),
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
