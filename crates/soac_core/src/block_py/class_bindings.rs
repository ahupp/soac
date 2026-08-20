//! Source-native class binding semantics, independent of executable authority.
//!
//! Native code IDs belong to one exact original constant tree. The compiler
//! retains only cells required by actual class construction, lexical access,
//! and closure transport. Comprehension iteration state uses ordinary helper
//! scopes; it has no native-frame slot or save/restore correspondence.

use soac_contracts::SourceRange;

/// Version of the native Details wire, not an archived IR-layout epoch.
pub const CLASS_BINDINGS_SCHEMA_VERSION: u32 = 7;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
pub struct NativeCodeId(pub u32);

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
pub struct ClassBindingSlotId {
    pub class_code: NativeCodeId,
    pub index: u32,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
pub struct NativeLocalsPlusKind(pub u8);

impl NativeLocalsPlusKind {
    pub const LOCAL: Self = Self(0x20);
    pub const CELL: Self = Self(0x40);
    pub const FREE: Self = Self(0x80);

    /// Whether this native slot has CELL/FREE storage rather than a raw local.
    pub const fn is_cell(self) -> bool {
        self.0 & (Self::CELL.0 | Self::FREE.0) != 0
    }

    pub const fn is_local(self) -> bool {
        self.0 & Self::LOCAL.0 != 0
    }

    pub const fn is_free(self) -> bool {
        self.0 & Self::FREE.0 != 0
    }

    /// Class bodies have no fast arguments. HIDDEN requires LOCAL; FREE is a
    /// distinct suffix and cannot be combined with LOCAL, CELL, or HIDDEN.
    pub const fn is_valid_class_slot(self) -> bool {
        matches!(self.0, 0x20 | 0x30 | 0x40 | 0x60 | 0x70 | 0x80)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
#[repr(u8)]
pub enum NativeCompileScopeKind {
    Module = 0,
    Class = 1,
    Function = 2,
    AsyncFunction = 3,
    Lambda = 4,
    Comprehension = 5,
    Annotations = 6,
}

impl NativeCompileScopeKind {
    pub const fn from_wire(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Module),
            1 => Some(Self::Class),
            2 => Some(Self::Function),
            3 => Some(Self::AsyncFunction),
            4 => Some(Self::Lambda),
            5 => Some(Self::Comprehension),
            6 => Some(Self::Annotations),
            _ => None,
        }
    }

    pub const fn wire_value(self) -> u8 {
        self as u8
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
#[repr(u8)]
pub enum NativeSymbolScopeKind {
    FunctionBlock = 0,
    ClassBlock = 1,
    ModuleBlock = 2,
    AnnotationBlock = 3,
    TypeAliasBlock = 4,
    TypeParametersBlock = 5,
    TypeVariableBlock = 6,
}

impl NativeSymbolScopeKind {
    pub const fn from_wire(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::FunctionBlock),
            1 => Some(Self::ClassBlock),
            2 => Some(Self::ModuleBlock),
            3 => Some(Self::AnnotationBlock),
            4 => Some(Self::TypeAliasBlock),
            5 => Some(Self::TypeParametersBlock),
            6 => Some(Self::TypeVariableBlock),
            _ => None,
        }
    }

    pub const fn wire_value(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct NativeLocalsPlusSlot {
    pub name: String,
    pub kind: NativeLocalsPlusKind,
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingCodeNode {
    pub id: NativeCodeId,
    pub parent: Option<NativeCodeId>,
    pub compile_scope: NativeCompileScopeKind,
    pub symbol_scope: NativeSymbolScopeKind,
    /// Actual original code first line, including the first decorator when
    /// native compilation uses it. This is not a lexical identity fallback.
    pub first_line: u32,
    pub source_range: Option<SourceRange>,
    pub slots: Vec<NativeLocalsPlusSlot>,
    pub freevar_count: u32,
}

impl ClassBindingCodeNode {
    /// Native artificial entry locations use this exact zero-column marker.
    pub fn first_line_marker(&self, source: &str) -> Option<SourceRange> {
        let line = self.first_line.checked_sub(1)? as usize;
        let offset = std::iter::once(0)
            .chain(
                source
                    .bytes()
                    .enumerate()
                    .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
            )
            .nth(line)?;
        let offset = u32::try_from(offset).ok()?;
        Some(SourceRange::new(offset, offset))
    }

    pub fn freevar_slot(&self, ordinal: u32) -> Option<&NativeLocalsPlusSlot> {
        if ordinal >= self.freevar_count {
            return None;
        }
        let start = self.slots.len().checked_sub(self.freevar_count as usize)?;
        self.slots.get(start + ordinal as usize)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
#[repr(u8)]
pub enum ClassBindingPhase {
    ClassEntry = 0,
    ClassHeaderComplete = 1,
}

impl ClassBindingPhase {
    pub const fn from_wire(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::ClassEntry),
            1 => Some(Self::ClassHeaderComplete),
            _ => None,
        }
    }

    pub const fn wire_value(self) -> u8 {
        self as u8
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
pub enum ClassBindingInitialValue {
    EmptyCell,
    IncomingFree { ordinal: u32 },
    NamespaceStore,
    ConditionalSetStore,
}

impl ClassBindingInitialValue {
    pub const fn from_wire(role: u32, operand: Option<u32>) -> Option<Self> {
        match (role, operand) {
            (1, None) => Some(Self::EmptyCell),
            (2, Some(ordinal)) => Some(Self::IncomingFree { ordinal }),
            (3, None) => Some(Self::NamespaceStore),
            (4, None) => Some(Self::ConditionalSetStore),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingInitializer {
    pub phase: ClassBindingPhase,
    pub slot: ClassBindingSlotId,
    pub value: ClassBindingInitialValue,
}

/// A capture's native creation location is not always an original source site.
/// Wire v1 uses the class code's exact entry marker for its deferred variable
/// annotation provider, although that provider is created at body completion.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum ClassBindingCaptureCreation {
    SourceRange(SourceRange),
    ClassAnnotationBodyCompletion { marker: SourceRange },
    Unavailable,
}

impl ClassBindingCaptureCreation {
    /// Decode the established native v1 marker against actual code metadata.
    /// The caller must already have authenticated both nodes' original code
    /// identities and copied first_line from their native code views.
    pub fn from_native_marker(
        source: &str,
        parent: &ClassBindingCodeNode,
        child: &ClassBindingCodeNode,
        marker: Option<SourceRange>,
    ) -> Result<Self, String> {
        let creation = match marker {
            None => Self::Unavailable,
            Some(marker)
                if child.compile_scope == NativeCompileScopeKind::Annotations
                    && child.symbol_scope == NativeSymbolScopeKind::AnnotationBlock
                    && (marker.start == marker.end || child.first_line == parent.first_line) =>
            {
                Self::ClassAnnotationBodyCompletion { marker }
            }
            Some(range) => Self::SourceRange(range),
        };
        creation.validate(source, parent, child)?;
        Ok(creation)
    }

    /// Only genuine source sites participate in original-expression matching.
    pub fn source_range(&self) -> Option<SourceRange> {
        match self {
            Self::SourceRange(range) => Some(*range),
            Self::ClassAnnotationBodyCompletion { .. } | Self::Unavailable => None,
        }
    }

    pub fn validate(
        &self,
        source: &str,
        parent: &ClassBindingCodeNode,
        child: &ClassBindingCodeNode,
    ) -> Result<(), String> {
        let class_range = parent
            .source_range
            .ok_or("class capture parent has no source range")?;
        let valid_range = |range: SourceRange| {
            range.start <= range.end
                && range.end as usize <= source.len()
                && source.is_char_boundary(range.start as usize)
                && source.is_char_boundary(range.end as usize)
        };
        if child.parent != Some(parent.id) {
            return Err("class capture must target its actual direct native child".into());
        }
        let completion_child = parent.compile_scope == NativeCompileScopeKind::Class
            && parent.symbol_scope == NativeSymbolScopeKind::ClassBlock
            && child.compile_scope == NativeCompileScopeKind::Annotations
            && child.symbol_scope == NativeSymbolScopeKind::AnnotationBlock
            && child.first_line == parent.first_line;
        match self {
            Self::Unavailable if completion_child => {
                Err("class annotation provider has no native body-completion marker".into())
            }
            Self::Unavailable => Ok(()),
            Self::SourceRange(_) if completion_child => Err(
                "class annotation body completion is not an original source creation site".into(),
            ),
            Self::SourceRange(range) => {
                if !valid_range(*range) || !class_range.contains(*range) {
                    return Err(
                        "capture creation lies outside its class or has an invalid range".into(),
                    );
                }
                Ok(())
            }
            Self::ClassAnnotationBodyCompletion { marker } => {
                if parent.compile_scope != NativeCompileScopeKind::Class
                    || parent.symbol_scope != NativeSymbolScopeKind::ClassBlock
                    || child.compile_scope != NativeCompileScopeKind::Annotations
                    || child.symbol_scope != NativeSymbolScopeKind::AnnotationBlock
                    || child.first_line != parent.first_line
                    || parent.first_line_marker(source) != Some(*marker)
                    || !child.source_range.is_some_and(|range| {
                        valid_range(range) && range.start < range.end && class_range.contains(range)
                    })
                {
                    return Err("invalid native class annotation body-completion marker".into());
                }
                Ok(())
            }
        }
    }
}

/// Read the actual current carrier, including after a conditional region.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingCapture {
    pub child: NativeCodeId,
    pub creation: ClassBindingCaptureCreation,
    pub freevar_ordinal: u32,
    pub source: ClassBindingSlotId,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
#[repr(u8)]
pub enum ClassBindingExportKind {
    ClassCell = 0,
    ClassDictCell = 1,
}

impl ClassBindingExportKind {
    pub const fn from_wire(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::ClassCell),
            1 => Some(Self::ClassDictCell),
            _ => None,
        }
    }

    pub const fn wire_value(self) -> u8 {
        self as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingExport {
    pub kind: ClassBindingExportKind,
    pub source: ClassBindingSlotId,
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingRecipe {
    pub class_code: NativeCodeId,
    pub initializers: Vec<ClassBindingInitializer>,
    pub captures: Vec<ClassBindingCapture>,
    pub exports: Vec<ClassBindingExport>,
    pub accesses: Vec<ClassBindingAccess>,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
#[repr(u8)]
pub enum ClassBindingAccessContext {
    Load = 0,
    Store = 1,
    Delete = 2,
}

impl ClassBindingAccessContext {
    pub const fn from_wire(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Load),
            1 => Some(Self::Store),
            2 => Some(Self::Delete),
            _ => None,
        }
    }

    pub const fn wire_value(self) -> u8 {
        self as u8
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[rkyv(derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash))]
#[repr(u8)]
pub enum ClassBindingAccessSelection {
    RawSlot = 0,
    CellValue = 1,
    NamespaceOrCell = 2,
}

impl ClassBindingAccessSelection {
    pub const fn from_wire(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::RawSlot),
            1 => Some(Self::CellValue),
            2 => Some(Self::NamespaceOrCell),
            _ => None,
        }
    }

    pub const fn wire_value(self) -> u8 {
        self as u8
    }
}

/// The selected storage for one actual source Name operation. The native
/// compiler supplies this decision before outlining or renaming can erase it.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingAccess {
    pub source_range: SourceRange,
    pub context: ClassBindingAccessContext,
    pub selection: ClassBindingAccessSelection,
    pub source: ClassBindingSlotId,
}

/// One compiler-owned class cell. Its spelling is a physical binding key;
/// the class-qualified native identity selects its lexical meaning.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingSlotBinding {
    pub slot: ClassBindingSlotId,
    pub binding: String,
}

/// Class construction and lexical cell bindings, not a native-frame inventory.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingScope {
    pub source: soac_contracts::SourceIdentity,
    pub node: ClassBindingCodeNode,
    pub recipe: ClassBindingRecipe,
    pub namespace_binding: String,
    pub slots: Vec<ClassBindingSlotBinding>,
}

impl ClassBindingScope {
    /// The cell itself is allocated or captured by explicit class-entry code.
    pub fn is_current_cell_binding(&self, name: &str) -> bool {
        let mut found = self.slots.iter().filter(|row| row.binding == name);
        let Some(row) = found.next() else {
            return false;
        };
        found.next().is_none()
            && row.slot.class_code == self.node.id
            && self
                .node
                .slots
                .get(row.slot.index as usize)
                .is_some_and(|slot| slot.kind.is_cell())
    }

    pub fn slot_binding(&self, slot: ClassBindingSlotId) -> Option<&str> {
        let mut rows = self.slots.iter().filter(|row| row.slot == slot);
        let row = rows.next()?;
        rows.next().is_none().then_some(row.binding.as_str())
    }
}

/// Physical storage selected for an actual class binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ClassBindingStorage {
    Cell(super::CellLocation),
}

impl ClassBindingStorage {
    pub fn raw_local(self, layout: &super::StorageLayout) -> Option<super::LocalLocation> {
        match self {
            Self::Cell(super::CellLocation::Owned(index)) => {
                let cell = layout.cellvars.get(index as usize)?;
                let mut slots = layout
                    .stack_slots
                    .iter()
                    .enumerate()
                    .filter(|(_, name)| *name == &cell.storage_name);
                let (index, _) = slots.next()?;
                if slots.next().is_some() {
                    return None;
                }
                Some(super::LocalLocation(u32::try_from(index).ok()?))
            }
            Self::Cell(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingSlotProjection {
    pub slot: ClassBindingSlotId,
    pub storage: ClassBindingStorage,
}

/// Storage shared by actual class cell accesses, closure capture and exports.
/// Its rows are not a copy of the native localsplus inventory.
#[derive(Debug, Clone, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ClassBindingProjection {
    pub class_code: NativeCodeId,
    pub namespace: super::LocalLocation,
    pub slots: Vec<ClassBindingSlotProjection>,
}

impl ClassBindingProjection {
    pub fn slot(&self, slot: ClassBindingSlotId) -> Option<&ClassBindingSlotProjection> {
        let mut rows = self.slots.iter().filter(|row| row.slot == slot);
        let row = rows.next()?;
        rows.next().is_none().then_some(row)
    }

    /// Recover an actual class cell's source name for ordinary name errors.
    pub fn source_name_at<'a>(
        &self,
        source: &'a ClassBindingScope,
        layout: &super::StorageLayout,
        location: super::LocalLocation,
    ) -> Option<&'a str> {
        if self.class_code != source.node.id || self.class_code != source.recipe.class_code {
            return None;
        }
        let mut matches = self.slots.iter().filter(|row| {
            row.slot.class_code == self.class_code
                && row.storage.raw_local(layout) == Some(location)
        });
        let row = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        source
            .node
            .slots
            .get(row.slot.index as usize)
            .map(|slot| slot.name.as_str())
    }

    pub fn validate(
        &self,
        source: &ClassBindingScope,
        layout: &super::StorageLayout,
        callable: &super::CallableScopeInfo,
    ) -> Result<(), String> {
        let fail = |message: &str| Err(message.to_owned());
        if self.class_code != source.node.id
            || self.class_code != source.recipe.class_code
            || source.source.definition_kind != soac_contracts::DefinitionKind::Class
            || source.node.compile_scope != NativeCompileScopeKind::Class
            || source.node.symbol_scope != NativeSymbolScopeKind::ClassBlock
            || callable.class_bindings.as_ref() != Some(source)
        {
            return fail("class binding projection changed its original class identity");
        }
        let Some(native_range) = source.node.source_range else {
            return fail("class binding source range is absent");
        };
        if native_range.start < source.source.source_range.start
            || native_range.end != source.source.source_range.end
            || native_range.start >= native_range.end
        {
            return fail("class binding node lies outside its original declaration");
        }
        let Some(origin) = &callable.source_origin else {
            return fail("class binding projection requires its producer origin");
        };
        if origin.role != super::CallableSourceRole::ClassNamespace
            || origin.definition != source.source
        {
            return fail("class binding projection differs from its executable class producer");
        }
        if layout.stack_slots.get(self.namespace.slot() as usize) != Some(&source.namespace_binding)
            || layout.is_expression_temporary(self.namespace)
        {
            return fail("class namespace projection does not select its actual mapping owner");
        }
        let required = source
            .recipe
            .initializers
            .iter()
            .filter(|init| init.phase == ClassBindingPhase::ClassEntry)
            .map(|init| init.slot)
            .collect::<std::collections::BTreeSet<_>>();
        if self.slots.len() != source.slots.len()
            || self.slots.len() != required.len()
            || source.slots.iter().map(|row| row.slot).collect::<Vec<_>>()
                != required.into_iter().collect::<Vec<_>>()
        {
            return fail("class binding projection changed its selected lexical cells");
        }
        let mut used_locations = std::collections::HashSet::from([self.namespace]);
        let mut binding_names =
            std::collections::HashSet::from([source.namespace_binding.as_str()]);
        for (actual, declared) in self.slots.iter().zip(&source.slots) {
            if actual.slot != declared.slot
                || actual.slot.class_code != self.class_code
                || !binding_names.insert(declared.binding.as_str())
            {
                return fail("class cell projection is ambiguous or reordered");
            }
            let Some(native) = source.node.slots.get(actual.slot.index as usize) else {
                return fail("class cell projection refers to absent lexical storage");
            };
            let ClassBindingStorage::Cell(super::CellLocation::Owned(cell_index)) = actual.storage
            else {
                return fail("class cell requires its actual owned carrier");
            };
            if !native.kind.is_cell() {
                return fail("class binding selected a native iteration local instead of a cell");
            }
            let Some(local) = actual.storage.raw_local(layout) else {
                return fail("class cell lacks one owned raw local carrier");
            };
            if !used_locations.insert(local)
                || layout.is_expression_temporary(local)
                || layout.stack_slots.get(local.slot() as usize) != Some(&declared.binding)
            {
                return fail("class cell owner overlaps another owner or operand temporary");
            }
            let cell = &layout.cellvars[cell_index as usize];
            if !callable.owned_cell_source_names.contains(&declared.binding)
                || layout
                    .cellvars
                    .iter()
                    .filter(|row| row.storage_name == declared.binding)
                    .count()
                    != 1
                || cell.storage_name != declared.binding
                || cell.logical_name != declared.binding
                || cell.init != super::ClosureInit::Deferred
            {
                return fail(
                    "class cell does not use its explicit non-allocating owned-cell source",
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class_projection_fixture() -> (
        ClassBindingScope,
        super::super::StorageLayout,
        super::super::CallableScopeInfo,
        ClassBindingProjection,
    ) {
        use super::super::{
            CallableScopeInfo, CallableSourceOrigin, CallableSourceRole, CellLocation, ClosureInit,
            ClosureSlot, LocalLocation, StorageLayout,
        };
        let code = NativeCodeId(2);
        let slot = |index| ClassBindingSlotId {
            class_code: code,
            index,
        };
        let source = soac_contracts::SourceIdentity {
            module: soac_contracts::ModuleContentId::new("class_projection_fixture", 0),
            lexical_qualname: "outer.<locals>.Box".into(),
            source_range: SourceRange::new(0, 80),
            definition_kind: soac_contracts::DefinitionKind::Class,
        };
        let scope = ClassBindingScope {
            source: source.clone(),
            node: ClassBindingCodeNode {
                id: code,
                parent: Some(NativeCodeId(1)),
                compile_scope: NativeCompileScopeKind::Class,
                symbol_scope: NativeSymbolScopeKind::ClassBlock,
                first_line: 1,
                source_range: Some(source.source_range),
                slots: vec![
                    NativeLocalsPlusSlot {
                        name: "iteration_target".into(),
                        kind: NativeLocalsPlusKind(0x30),
                    },
                    NativeLocalsPlusSlot {
                        name: "same".into(),
                        kind: NativeLocalsPlusKind::CELL,
                    },
                    NativeLocalsPlusSlot {
                        name: "same".into(),
                        kind: NativeLocalsPlusKind::FREE,
                    },
                ],
                freevar_count: 1,
            },
            recipe: ClassBindingRecipe {
                class_code: code,
                initializers: vec![
                    ClassBindingInitializer {
                        phase: ClassBindingPhase::ClassEntry,
                        slot: slot(1),
                        value: ClassBindingInitialValue::EmptyCell,
                    },
                    ClassBindingInitializer {
                        phase: ClassBindingPhase::ClassEntry,
                        slot: slot(2),
                        value: ClassBindingInitialValue::IncomingFree { ordinal: 0 },
                    },
                ],
                captures: vec![],
                exports: vec![ClassBindingExport {
                    kind: ClassBindingExportKind::ClassCell,
                    source: slot(1),
                }],
                accesses: vec![],
            },
            namespace_binding: "namespace".into(),
            slots: (1..3)
                .map(|index| ClassBindingSlotBinding {
                    slot: slot(index),
                    binding: format!("raw{index}"),
                })
                .collect(),
        };
        let layout = StorageLayout {
            stack_slots: vec!["namespace".into(), "raw1".into(), "raw2".into()],
            cellvars: ["raw1", "raw2"]
                .into_iter()
                .map(|name| ClosureSlot {
                    logical_name: name.into(),
                    storage_name: name.into(),
                    init: ClosureInit::Deferred,
                })
                .collect(),
            ..Default::default()
        };
        let callable = CallableScopeInfo {
            class_bindings: Some(scope.clone()),
            source_origin: Some(CallableSourceOrigin {
                definition: source,
                role: CallableSourceRole::ClassNamespace,
            }),
            owned_cell_source_names: ["raw1".into(), "raw2".into()].into_iter().collect(),
            ..Default::default()
        };
        let projection = ClassBindingProjection {
            class_code: code,
            namespace: LocalLocation(0),
            slots: vec![
                ClassBindingSlotProjection {
                    slot: slot(1),
                    storage: ClassBindingStorage::Cell(CellLocation::Owned(0)),
                },
                ClassBindingSlotProjection {
                    slot: slot(2),
                    storage: ClassBindingStorage::Cell(CellLocation::Owned(1)),
                },
            ],
        };
        (scope, layout, callable, projection)
    }

    #[test]
    fn class_binding_projection_selects_lexical_cells_not_native_iteration_slots() {
        use super::super::LocalLocation;
        let (scope, layout, callable, projection) = class_projection_fixture();
        projection.validate(&scope, &layout, &callable).unwrap();
        assert_eq!(scope.node.slots.len(), 3);
        assert_eq!(projection.slots.len(), 2);
        assert_eq!(scope.node.slots[1].name, scope.node.slots[2].name);
        for (index, local) in [(1, LocalLocation(1)), (2, LocalLocation(2))] {
            let id = ClassBindingSlotId {
                class_code: scope.node.id,
                index,
            };
            assert_eq!(
                projection.slot(id).unwrap().storage.raw_local(&layout),
                Some(local)
            );
            assert_eq!(
                projection.source_name_at(&scope, &layout, local),
                Some("same")
            );
            assert!(scope.slot_binding(id).is_some());
        }
        assert!(
            projection
                .slot(ClassBindingSlotId {
                    class_code: scope.node.id,
                    index: 0
                })
                .is_none()
        );
        assert!(
            scope
                .slot_binding(ClassBindingSlotId {
                    class_code: NativeCodeId(7),
                    index: 1
                })
                .is_none()
        );
        assert_eq!(
            projection.source_name_at(&scope, &layout, LocalLocation(0)),
            None
        );
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&projection).unwrap();
        assert_eq!(
            rkyv::from_bytes::<ClassBindingProjection, rkyv::rancor::Error>(&bytes).unwrap(),
            projection
        );
        let mut remapped_layout = layout.clone();
        remapped_layout.class_bindings = Some(projection);
        remapped_layout.set_stack_slots(remapped_layout.stack_slots.clone());
        assert!(remapped_layout.class_bindings.is_none());
    }

    #[test]
    fn class_binding_projection_rejects_foreign_missing_and_aliased_lexical_cells() {
        use super::super::{CellLocation, ClosureInit, LocalLocation};
        type Edit = fn(
            &mut super::super::StorageLayout,
            &mut super::super::CallableScopeInfo,
            &mut ClassBindingProjection,
        );
        let edits: &[(&str, Edit)] = &[
            ("private instead of owned cell", |_, _, projection| {
                projection.slots[1].storage = ClassBindingStorage::Cell(CellLocation::Private(0))
            }),
            ("duplicate class cell", |_, _, projection| {
                projection.slots[1].storage = projection.slots[0].storage
            }),
            ("automatic early cell allocation", |layout, _, _| {
                layout.cellvars[0].init = ClosureInit::EmptyCell
            }),
            ("unregistered cell", |_, callable, _| {
                callable.owned_cell_source_names.remove("raw1");
            }),
            ("duplicate owned registry row", |layout, _, _| {
                layout.cellvars.push(layout.cellvars[0].clone())
            }),
            ("cell aliases namespace", |_, _, projection| {
                projection.namespace = LocalLocation(1)
            }),
            ("missing required cell", |_, _, projection| {
                projection.slots.pop();
            }),
            ("foreign source cell", |_, _, projection| {
                projection.slots[0].slot.class_code = NativeCodeId(4)
            }),
            ("wrong executable origin", |_, callable, _| {
                callable.source_origin.as_mut().unwrap().role =
                    super::super::CallableSourceRole::SourceFunction
            }),
        ];
        for &(label, edit) in edits {
            let (scope, mut layout, mut callable, mut projection) = class_projection_fixture();
            edit(&mut layout, &mut callable, &mut projection);
            assert!(
                projection.validate(&scope, &layout, &callable).is_err(),
                "{label}"
            );
        }
    }

    #[test]
    fn class_binding_wire_tags_are_explicit_and_reject_unknown_values() {
        for value in 0..=6 {
            assert_eq!(
                NativeCompileScopeKind::from_wire(value)
                    .unwrap()
                    .wire_value(),
                value as u8
            );
            assert_eq!(
                NativeSymbolScopeKind::from_wire(value)
                    .unwrap()
                    .wire_value(),
                value as u8
            );
        }
        for value in [7, u32::MAX] {
            assert!(NativeCompileScopeKind::from_wire(value).is_none());
            assert!(NativeSymbolScopeKind::from_wire(value).is_none());
        }
        for value in 0..=1 {
            assert_eq!(
                ClassBindingPhase::from_wire(value).unwrap().wire_value(),
                value as u8
            );
            assert_eq!(
                ClassBindingExportKind::from_wire(value)
                    .unwrap()
                    .wire_value(),
                value as u8
            );
        }
        for value in 0..=2 {
            assert_eq!(
                ClassBindingAccessContext::from_wire(value)
                    .unwrap()
                    .wire_value(),
                value as u8
            );
            assert_eq!(
                ClassBindingAccessSelection::from_wire(value)
                    .unwrap()
                    .wire_value(),
                value as u8
            );
        }
        assert!(ClassBindingAccessContext::from_wire(3).is_none());
        assert!(ClassBindingAccessSelection::from_wire(3).is_none());
        assert!(ClassBindingPhase::from_wire(2).is_none());
        assert!(ClassBindingExportKind::from_wire(2).is_none());
        for role in [1, 3, 4] {
            assert!(ClassBindingInitialValue::from_wire(role, None).is_some());
            assert!(ClassBindingInitialValue::from_wire(role, Some(0)).is_none());
        }
        assert_eq!(
            ClassBindingInitialValue::from_wire(2, Some(3)),
            Some(ClassBindingInitialValue::IncomingFree { ordinal: 3 })
        );
        assert!(ClassBindingInitialValue::from_wire(2, None).is_none());
        assert!(ClassBindingInitialValue::from_wire(5, None).is_none());
    }
    #[test]
    fn native_class_capture_completion_marker_preserves_source_identity() {
        let source = "from __future__ import strict\n\
                      def build():\n\
                      \x20   @decorate\n\
                      \x20   class Box:\n\
                      \x20       field: Value\n";
        let class_start = source.find("class Box:").unwrap() as u32;
        let value_start = source.find("Value").unwrap() as u32;
        let parent = ClassBindingCodeNode {
            id: NativeCodeId(2),
            parent: Some(NativeCodeId(1)),
            compile_scope: NativeCompileScopeKind::Class,
            symbol_scope: NativeSymbolScopeKind::ClassBlock,
            first_line: 3,
            source_range: Some(SourceRange::new(class_start, source.len() as u32)),
            slots: vec![],
            freevar_count: 0,
        };
        let child = ClassBindingCodeNode {
            id: NativeCodeId(3),
            parent: Some(parent.id),
            compile_scope: NativeCompileScopeKind::Annotations,
            symbol_scope: NativeSymbolScopeKind::AnnotationBlock,
            first_line: parent.first_line,
            source_range: Some(SourceRange::new(value_start, value_start + 5)),
            slots: vec![],
            freevar_count: 0,
        };
        let marker = parent.first_line_marker(source).unwrap();
        assert!(
            marker.start < class_start,
            "decorator marker precedes the ClassDef"
        );
        let creation =
            ClassBindingCaptureCreation::from_native_marker(source, &parent, &child, Some(marker))
                .unwrap();
        assert_eq!(
            creation,
            ClassBindingCaptureCreation::ClassAnnotationBodyCompletion { marker },
        );
        assert_eq!(creation.source_range(), None);
        assert_eq!(child.source_range.unwrap().start, value_start);

        let false_marker = SourceRange::new(class_start, class_start);
        assert!(
            ClassBindingCaptureCreation::from_native_marker(
                source,
                &parent,
                &child,
                Some(false_marker),
            )
            .is_err()
        );
        assert!(
            ClassBindingCaptureCreation::from_native_marker(
                source,
                &parent,
                &child,
                child.source_range,
            )
            .is_err(),
            "a provider's expression range is not its creation marker"
        );
        assert!(
            ClassBindingCaptureCreation::from_native_marker(source, &parent, &child, None,)
                .is_err(),
            "a missing completion marker is not fabricated as unavailable"
        );

        let mut bad = child.clone();
        bad.parent = Some(NativeCodeId(0));
        assert!(creation.validate(source, &parent, &bad).is_err());
        bad = child.clone();
        bad.compile_scope = NativeCompileScopeKind::Function;
        bad.symbol_scope = NativeSymbolScopeKind::FunctionBlock;
        assert!(creation.validate(source, &parent, &bad).is_err());
        bad = child.clone();
        bad.first_line += 1;
        assert!(creation.validate(source, &parent, &bad).is_err());
    }
}
