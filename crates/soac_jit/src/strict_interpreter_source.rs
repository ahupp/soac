//! Original native source correspondence for the interpreter backend.
//!
//! This module does not import the compiler IR, invoke lowering, select a JIT
//! plan, or apply scope/lifetime recipes. It joins the exact native Details
//! result to the original parsed bytes and already verified checker identities.
//! The annotation extension reads only native capture/current-slot/header/export
//! scalars and refuses role inference for region-reused carriers. It does not
//! validate lifetime execution plans or filter unrelated lifetime gaps.
//! No result is a function-birth, runtime-class, captured-activation, or boundary
//! grant; runtime consumers must still authenticate the actual objects.
//!
//! The final map owns Rust data only. Its stored code addresses are never
//! dereferenced: every lookup first receives and validates a caller-owned live
//! exact code object. The native source ID is unique within its interpreter.
//! Ordinary replacement code must be handled by the caller's explicit native
//! source-authority decision, not by catching an adapter error as permission.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString, PyTuple};
use ruff_python_ast::token::{TokenKind, Tokens};
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};
use soac_contracts::{
    DefinitionKind, FunctionKind, ModuleTypeFacts, ParameterKind, SourceIdentity, SourceRange,
};

use crate::code_view::{RawPySoacCodeView, view};
use crate::{VerifiedStrictModule, strict_runtime_unavailable};

// Existing versioned compile.h/code.h/pycore_code.h ABI tags, not a second wire.
const WIRE_VERSION: u32 = soac_core::block_py::CLASS_BINDINGS_SCHEMA_VERSION;
const ARG_POS: u8 = 0x02;
const ARG_KW: u8 = 0x04;
const ARG_VAR: u8 = 0x08;
const ARG_MASK: u8 = ARG_POS | ARG_KW | ARG_VAR;
const HIDDEN: u8 = 0x10;
const LOCAL: u8 = 0x20;
const CELL: u8 = 0x40;
const FREE: u8 = 0x80;
const STORAGE_MASK: u8 = LOCAL | CELL | FREE;
const CO_VARARGS: i32 = 0x0004;
const CO_VARKEYWORDS: i32 = 0x0008;
const CO_GENERATOR: i32 = 0x0020;
const CO_COROUTINE: i32 = 0x0080;
const CO_ASYNC_GENERATOR: i32 = 0x0200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterpreterCodeRole {
    Module,
    SourceFunction,
    AsyncSourceFunction,
    ClassNamespace,
    AnnotationProvider,
    TypeParameterScope,
    Lambda,
    Comprehension,
    TypeAlias,
    TypeVariable,
}

impl InterpreterCodeRole {
    fn from_wire(scope: u32, symbol: u32) -> Option<Self> {
        Some(match (scope, symbol) {
            (0, 2) => Self::Module,
            (1, 1) => Self::ClassNamespace,
            (2, 0) => Self::SourceFunction,
            (3, 0) => Self::AsyncSourceFunction,
            (4, 0) => Self::Lambda,
            (5, 0) => Self::Comprehension,
            (6, 3) => Self::AnnotationProvider,
            (6, 4) => Self::TypeAlias,
            (6, 5) => Self::TypeParameterScope,
            (6, 6) => Self::TypeVariable,
            _ => return None,
        })
    }

    fn is_definition(self) -> bool {
        matches!(
            self,
            Self::SourceFunction | Self::AsyncSourceFunction | Self::ClassNamespace
        )
    }
}

/// Read-only native parameter data; the optional source index is the original
/// source signature order, not a transformed-local or runtime-owner identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterParameter {
    pub(crate) native_index: u32,
    pub(crate) native_name: String,
    pub(crate) kind: ParameterKind,
    pub(crate) source_index: Option<u32>,
    pub(crate) source_name: Option<String>,
}

/// Actual native localsplus slots, not inferred lexical captures or cell values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterLocal {
    pub(crate) index: u32,
    pub(crate) name: String,
    pub(crate) kind: u8,
    pub(crate) free_ordinal: Option<u32>,
}

/// Scalar snapshot of the exact original code header. Crate-visible fields
/// contain data, not a way to construct a validated source map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterNativeLayout {
    pub(crate) flags: i32,
    pub(crate) positional_count: u32,
    pub(crate) positional_only_count: u32,
    pub(crate) keyword_only_count: u32,
    pub(crate) parameters: Vec<InterpreterParameter>,
    pub(crate) locals: Vec<InterpreterLocal>,
}

impl InterpreterNativeLayout {
    pub fn free_variables(&self) -> impl Iterator<Item = (u32, u32, &str)> {
        self.locals.iter().filter_map(|slot| {
            slot.free_ordinal
                .map(|ordinal| (ordinal, slot.index, slot.name.as_str()))
        })
    }

    fn read(py: Python<'_>, code: &RawPySoacCodeView) -> PyResult<Self> {
        let count =
            |n: i32| u32::try_from(n).map_err(|_| invalid(py, "negative native code count"));
        let positional_count = count(code.argcount)?;
        let positional_only_count = count(code.posonlyargcount)?;
        let keyword_only_count = count(code.kwonlyargcount)?;
        let nlocals = count(code.nlocals)?;
        let ncellvars = count(code.ncellvars)?;
        let nfreevars = count(code.nfreevars)?;
        let nlocalsplus = count(code.nlocalsplus)?;
        if positional_only_count > positional_count || code.code_units < 0 {
            return Err(invalid(py, "inconsistent native signature or code size"));
        }
        let names = exact_tuple(
            unsafe { Bound::from_borrowed_ptr(py, code.localsplusnames) },
            None,
        )?;
        let kinds = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, code.localspluskinds) };
        if !kinds.is_exact_instance_of::<PyBytes>() {
            return Err(invalid(py, "native localsplus kinds are not exact bytes"));
        }
        let kinds = kinds.cast_into::<PyBytes>()?;
        if names.len() != nlocalsplus as usize || kinds.as_bytes().len() != names.len() {
            return Err(invalid(py, "native localsplus cardinality mismatch"));
        }
        let parameter_count = positional_count
            .checked_add(keyword_only_count)
            .and_then(|n| n.checked_add(u32::from(code.flags & CO_VARARGS != 0)))
            .and_then(|n| n.checked_add(u32::from(code.flags & CO_VARKEYWORDS != 0)))
            .ok_or_else(|| invalid(py, "native parameter count overflows"))?;
        if nlocals > nlocalsplus || parameter_count > nlocals || ncellvars > nlocalsplus {
            return Err(invalid(
                py,
                "native parameter/local/cell counts exceed actual slots",
            ));
        }
        let mut parameter_kinds = Vec::with_capacity(parameter_count as usize);
        parameter_kinds.extend(std::iter::repeat_n(
            ParameterKind::PositionalOnly,
            positional_only_count as usize,
        ));
        parameter_kinds.extend(std::iter::repeat_n(
            ParameterKind::PositionalOrKeyword,
            (positional_count - positional_only_count) as usize,
        ));
        parameter_kinds.extend(std::iter::repeat_n(
            ParameterKind::KeywordOnly,
            keyword_only_count as usize,
        ));
        if code.flags & CO_VARARGS != 0 {
            parameter_kinds.push(ParameterKind::VarArgs);
        }
        if code.flags & CO_VARKEYWORDS != 0 {
            parameter_kinds.push(ParameterKind::VarKeywords);
        }
        if parameter_kinds.len() > nlocals as usize {
            return Err(invalid(py, "native parameter prefix exceeds LOCAL slots"));
        }
        let free_start = nlocalsplus
            .checked_sub(nfreevars)
            .ok_or_else(|| invalid(py, "native FREE count exceeds localsplus"))?;
        if nlocals > free_start {
            return Err(invalid(py, "native LOCAL and FREE ranges overlap"));
        }
        let mut locals = Vec::with_capacity(names.len());
        let mut cells = 0u32;
        let mut parameters = Vec::with_capacity(parameter_kinds.len());
        for (index, &kind) in kinds.as_bytes().iter().enumerate() {
            let index = index as u32;
            let storage = kind & STORAGE_MASK;
            let expected_storage = if index < nlocals {
                storage == LOCAL || storage == (LOCAL | CELL)
            } else if index < free_start {
                storage == CELL
            } else {
                storage == FREE
            };
            if kind & !(ARG_MASK | HIDDEN | STORAGE_MASK) != 0 || !expected_storage {
                return Err(invalid(py, "invalid native localsplus kind/domain"));
            }
            cells += u32::from(kind & CELL != 0);
            let name = exact_text(names.get_item(index as usize)?)?;
            let expected_arg = parameter_kinds.get(index as usize).copied();
            if kind & ARG_MASK != expected_arg.map_or(0, argument_bits) {
                return Err(invalid(
                    py,
                    "native parameter bits disagree with the actual header",
                ));
            }
            if let Some(parameter_kind) = expected_arg {
                parameters.push(InterpreterParameter {
                    native_index: index,
                    native_name: name.clone(),
                    kind: parameter_kind,
                    source_index: None,
                    source_name: None,
                });
            }
            locals.push(InterpreterLocal {
                index,
                name,
                kind,
                free_ordinal: (index >= free_start).then(|| index - free_start),
            });
        }
        if cells != ncellvars {
            return Err(invalid(
                py,
                "native CELL count disagrees with localsplus kinds",
            ));
        }
        Ok(Self {
            flags: code.flags,
            positional_count,
            positional_only_count,
            keyword_only_count,
            parameters,
            locals,
        })
    }

    fn attach_source_parameters(
        &mut self,
        py: Python<'_>,
        source: Option<&[SourceParameter]>,
    ) -> PyResult<()> {
        let Some(source) = source else {
            return Ok(());
        };
        if source.len() != self.parameters.len() {
            return Err(invalid(
                py,
                "original source/native parameter count mismatch",
            ));
        }
        // CPython places keyword-only entries before both variadic slots.
        // Source spelling is retained separately (native private-name mangling
        // is compiler-owned); it never supplies physical slot authority.
        let native_order = source
            .iter()
            .filter(|p| {
                matches!(
                    p.kind,
                    ParameterKind::PositionalOnly | ParameterKind::PositionalOrKeyword
                )
            })
            .chain(
                source
                    .iter()
                    .filter(|p| p.kind == ParameterKind::KeywordOnly),
            )
            .chain(source.iter().filter(|p| p.kind == ParameterKind::VarArgs))
            .chain(
                source
                    .iter()
                    .filter(|p| p.kind == ParameterKind::VarKeywords),
            );
        for (native, original) in self.parameters.iter_mut().zip(native_order) {
            if native.kind != original.kind {
                return Err(invalid(
                    py,
                    "original source/native parameter kind mismatch",
                ));
            }
            native.source_index = Some(original.index);
            native.source_name = Some(original.name.clone());
        }
        Ok(())
    }
}

fn argument_bits(kind: ParameterKind) -> u8 {
    match kind {
        ParameterKind::PositionalOnly => ARG_POS,
        ParameterKind::PositionalOrKeyword => ARG_POS | ARG_KW,
        ParameterKind::KeywordOnly => ARG_KW,
        ParameterKind::VarArgs => ARG_POS | ARG_VAR,
        ParameterKind::VarKeywords => ARG_KW | ARG_VAR,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterStoreTarget {
    Fast(u32),
    Cell(u32),
    Name(u32),
    Global(u32),
}

/// Preserved unsupported/context receipt, never a uniform execution policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterDefinitionGap {
    pub(crate) reason: u32,
    pub(crate) instruction_ordinal: Option<u32>,
    pub(crate) lane: Option<u8>,
    pub(crate) opcode: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterDefinitionStore {
    pub(crate) source: SourceIdentity,
    pub(crate) role: InterpreterCodeRole,
    pub(crate) body_code_ordinal: u32,
    pub(crate) native_origin: SourceRange,
    pub(crate) instruction_ordinal: u32,
    pub(crate) lane: u8,
    pub(crate) form: u32,
    pub(crate) target: InterpreterStoreTarget,
    pub(crate) gaps: Vec<InterpreterDefinitionGap>,
}

/// Original source role at an actual native CALL; this never identifies the
/// runtime callable/receiver/result by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterCallRole {
    SourceExpression,
    Decorator {
        index: u32,
        expression_range: SourceRange,
    },
    ClassConstruction {
        class_body_ordinal: u32,
    },
    GenericScopeInvocation {
        scope_ordinal: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterCallOrigin {
    pub(crate) source_definition: SourceIdentity,
    pub(crate) original_range: SourceRange,
    pub(crate) role: InterpreterCallRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterCallForm {
    Positional,
    Keywords,
    Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterCallChannel {
    Null,
    MethodSelfOrNull,
    LeadingArgument,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterPositionalKind {
    Vector,
    ExpandedEmpty,
    SoleStarDeferred,
    ExpandedDirectTuple,
    ExpandedListAtFirstStar,
    ExpandedListBeforeArguments,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterPositionalEntryKind {
    Source,
    Star,
    GenericBaseInjected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterPositionalEntry {
    pub(crate) kind: InterpreterPositionalEntryKind,
    pub(crate) source_range: Option<SourceRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterKeywordKind {
    None,
    NamesTuple,
    ExpandedGroups,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterKeywordEntryKind {
    Named,
    Mapping,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterKeywordEntry {
    pub(crate) kind: InterpreterKeywordEntryKind,
    pub(crate) source_range: SourceRange,
    pub(crate) native_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpreterKeywordMapStyle {
    BuildMap,
    MapAdd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterKeywordGroup {
    pub(crate) kind: InterpreterKeywordEntryKind,
    pub(crate) first: u32,
    pub(crate) count: u32,
    pub(crate) map_style: Option<InterpreterKeywordMapStyle>,
}

/// Exact value-only projection of native InputLayout, including the actual
/// emitter's allocation/group choices. It is not an opcode program or a source
/// recipe to reconstruct at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterCallInputLayout {
    pub(crate) channel: InterpreterCallChannel,
    pub(crate) preloaded_value_count: u32,
    pub(crate) positional_kind: InterpreterPositionalKind,
    pub(crate) positional_entries: Vec<InterpreterPositionalEntry>,
    pub(crate) keyword_kind: InterpreterKeywordKind,
    pub(crate) keyword_names: Option<Vec<String>>,
    pub(crate) keyword_entries: Vec<InterpreterKeywordEntry>,
    pub(crate) keyword_groups: Vec<InterpreterKeywordGroup>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterCallGap {
    pub(crate) reason: u32,
    pub(crate) instruction_ordinal: Option<u32>,
    pub(crate) lane: Option<u8>,
    pub(crate) opcode: Option<u32>,
    pub(crate) context_unavailable: bool,
}

/// Retain zero-emission and guarded/lowered alternatives without manufacturing
/// a physical CALL. The same native original-call inventory is used for lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterCallOriginStatus {
    pub(crate) origin: InterpreterCallOrigin,
    pub(crate) instruction_ordinals: Vec<u32>,
    pub(crate) gaps: Vec<InterpreterCallGap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpreterCallReceipt {
    pub(crate) origin: InterpreterCallOrigin,
    pub(crate) instruction_ordinal: u32,
    pub(crate) native_byte_offset: Option<u32>,
    pub(crate) form: InterpreterCallForm,
    /// None for CALL_FUNCTION_EX: its zero opcode operand is not zero values.
    pub(crate) native_value_argument_count: Option<u32>,
    pub(crate) input: InterpreterCallInputLayout,
    pub(crate) gaps: Vec<InterpreterCallGap>,
}

impl InterpreterCallReceipt {
    pub fn source_definition(&self) -> &SourceIdentity {
        &self.origin.source_definition
    }

    /// Required class_call already checked this role. Source-expression
    /// __build_class__ calls cannot acquire this child association.
    pub fn class_body_ordinal(&self) -> Option<u32> {
        match self.origin.role {
            InterpreterCallRole::ClassConstruction { class_body_ordinal } => {
                Some(class_body_ordinal)
            }
            _ => None,
        }
    }
    pub fn generic_scope_ordinal(&self) -> Option<u32> {
        match self.origin.role {
            InterpreterCallRole::GenericScopeInvocation { scope_ordinal } => Some(scope_ordinal),
            _ => None,
        }
    }
}

/// Scalar origin of one actual annotation-provider FREE cell. This describes
/// the native compiler edge, not the current cell value, a namespace instance,
/// or permission to assume a value type. Runtime consumers must
/// still authenticate the original provider and its actual captured activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InterpreterAnnotationCaptureOrigin {
    Lexical {
        /// Terminal source-function CELL, not necessarily the provider's
        /// immediate native parent when ordinary FREE slots forward it.
        parent_ordinal: u32,
        parent_slot: u32,
        binding_scope: SourceIdentity,
    },
    ClassDictionary {
        class_ordinal: u32,
        class_definition: SourceIdentity,
        class_slot: u32,
    },
    ConditionalAnnotations {
        class_ordinal: u32,
        class_definition: SourceIdentity,
        class_slot: u32,
    },
    Unresolved(InterpreterAnnotationCaptureUnresolved),
}

/// A supported original native body can have unproved special-cell provenance.
/// None of these reasons is an ordinary-execution rejection or an exemption
/// from the caller's mandatory annotation/nominal checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InterpreterAnnotationCaptureUnresolved {
    MissingCapture,
    AmbiguousCapture,
    CreationSiteUnavailable,
    RegionalCapture,
    ReusedCarrier,
    ForwardedFree,
    UnrepresentedParent,
    UnprovenClassCell,
    ClassDictionaryNotExported,
}

#[derive(Debug)]
pub struct InterpreterCode {
    ordinal: u32,
    parent: Option<u32>,
    address: usize,
    origin: OriginalOrigin,
    native_range: Option<SourceRange>,
    layout: InterpreterNativeLayout,
    instruction_count: u32,
    byte_size: usize,
    native_names: Vec<String>,
    definition_stores: BTreeMap<(u32, u8), InterpreterDefinitionStore>,
    unsupported_definition_sites: HashSet<(u32, u8)>,
    calls: BTreeMap<u32, InterpreterCallReceipt>,
    call_origins: Vec<InterpreterCallOriginStatus>,
    unsupported_call_sites: HashSet<u32>,
    annotation_captures: Vec<InterpreterAnnotationCaptureOrigin>,
}

impl InterpreterCode {
    pub const fn ordinal(&self) -> u32 {
        self.ordinal
    }

    pub const fn parent_ordinal(&self) -> Option<u32> {
        self.parent
    }

    pub const fn role(&self) -> InterpreterCodeRole {
        self.origin.role
    }

    /// For an implicit provider this is its explicit original owner, not a
    /// fabricated source declaration for the compiler-created helper.
    pub fn source(&self) -> &SourceIdentity {
        &self.origin.source
    }

    pub const fn native_range(&self) -> Option<SourceRange> {
        self.native_range
    }

    pub const fn expression_range(&self) -> Option<SourceRange> {
        self.origin.expression_range
    }

    pub fn layout(&self) -> &InterpreterNativeLayout {
        &self.layout
    }

    /// Same relevant native Call rows, including blocked, zero-emission and
    /// guarded/lowered alternatives. No row here grants runtime object identity.
    pub fn call_origins(&self) -> &[InterpreterCallOriginStatus] {
        &self.call_origins
    }

    /// Native name-table data for an already selected original operation.
    /// A name alone never selects a source definition.
    pub fn native_name(&self, index: u32) -> Option<&str> {
        self.native_names.get(index as usize).map(String::as_str)
    }
}

/// Construct only at the trusted native Details call site. This immutable map
/// has no Python primaries and supplies correspondence data, never adoption.
pub struct StrictInterpreterSource {
    verified: Arc<VerifiedStrictModule>,
    source_id: u64,
    codes: Vec<InterpreterCode>,
    by_address: HashMap<usize, u32>,
}

impl StrictInterpreterSource {
    /// The caller must pass the root/bindings from the SAME just-returned
    /// PySoac_CompileVerifiedSourceDetails invocation on verified.source() and
    /// verified.source_path(). A source-ID tag alone does not prove that call.
    pub fn from_native_details<'py>(
        py: Python<'py>,
        verified: Arc<VerifiedStrictModule>,
        root: &Bound<'py, PyAny>,
        bindings: &Bound<'py, PyAny>,
    ) -> PyResult<Self> {
        if current_interpreter() != verified.interpreter_id() {
            return Err(invalid(
                py,
                "verified source belongs to another interpreter",
            ));
        }
        let source = std::str::from_utf8(verified.source())
            .map_err(|_| invalid(py, "verified source is not UTF-8"))?;
        let decoder = Decoder::new(source);
        let originals = OriginalCatalog::collect(py, source, verified.type_facts().facts())?;
        let packet = exact_tuple(bindings.clone(), Some(4))?;
        if unsigned(packet.get_item(0)?)? != WIRE_VERSION {
            return Err(invalid(py, "requires native compile schema7"));
        }
        let rows = exact_tuple(packet.get_item(1)?, None)?;
        // The selective capture decoder reads only semantic provenance.
        // Source/owner identities and actual reused slots constrain annotation
        // roles; no native lifecycle or temporary-reference schedule is needed.
        let recipes = exact_tuple(packet.get_item(2)?, Some(rows.len()))?;
        let operations = exact_tuple(packet.get_item(3)?, Some(rows.len()))?;
        let tree = native_tree(py, root)?;
        if rows.len() != tree.len() || rows.is_empty() {
            return Err(invalid(
                py,
                "native node table differs from the actual final tree",
            ));
        }
        let source_id = unsafe { view(py, root.as_ptr())? }.strict_source_id;
        let mut codes = Vec::with_capacity(rows.len());
        let mut first_lines = Vec::with_capacity(rows.len());
        let mut by_address = HashMap::with_capacity(rows.len());
        for (ordinal, (code, parent)) in tree.iter().enumerate() {
            let row = exact_tuple(rows.get_item(ordinal)?, Some(6))?;
            if unsigned(row.get_item(0)?)? as usize != ordinal
                || optional_unsigned(row.get_item(1)?)? != *parent
                || row.get_item(2)?.as_ptr() != code.as_ptr()
            {
                return Err(invalid(
                    py,
                    "native node ID/parent/code does not match the actual tree",
                ));
            }
            let role = InterpreterCodeRole::from_wire(
                unsigned(row.get_item(3)?)?,
                unsigned(row.get_item(4)?)?,
            )
            .ok_or_else(|| invalid(py, "unrepresented native scope/symtable role"))?;
            let native_range = decoder.range(row.get_item(5)?)?;
            let original = originals.match_code(py, role, native_range)?;
            let header = unsafe { view(py, code.as_ptr())? };
            first_lines.push(
                u32::try_from(header.firstlineno)
                    .ok()
                    .filter(|line| *line > 0)
                    .ok_or_else(|| invalid(py, "native code has no positive first-line marker"))?,
            );
            let filename = exact_text(unsafe { Bound::from_borrowed_ptr(py, header.filename) })?;
            if verified.source_path().to_str() != Some(filename.as_str()) {
                return Err(invalid(
                    py,
                    "native code filename differs from verified source",
                ));
            }
            let mut layout = InterpreterNativeLayout::read(py, &header)?;
            layout.attach_source_parameters(py, original.parameters.as_deref())?;
            check_function_kind(py, &layout, original, verified.type_facts().facts())?;
            let native_names =
                exact_tuple(unsafe { Bound::from_borrowed_ptr(py, header.names) }, None)?
                    .iter()
                    .map(exact_text)
                    .collect::<PyResult<Vec<_>>>()?;
            let byte_size = usize::try_from(header.code_units)
                .ok()
                .and_then(|units| units.checked_mul(2))
                .ok_or_else(|| invalid(py, "native code size overflows"))?;
            let annotation_captures = if role == InterpreterCodeRole::AnnotationProvider {
                vec![
                    InterpreterAnnotationCaptureOrigin::Unresolved(
                        InterpreterAnnotationCaptureUnresolved::MissingCapture,
                    );
                    layout.free_variables().count()
                ]
            } else {
                Vec::new()
            };
            let address = code.as_ptr() as usize;
            by_address.insert(address, ordinal as u32);
            codes.push(InterpreterCode {
                ordinal: ordinal as u32,
                parent: *parent,
                address,
                origin: original.origin.clone(),
                native_range,
                layout,
                instruction_count: 0,
                byte_size,
                native_names,
                definition_stores: BTreeMap::new(),
                unsupported_definition_sites: HashSet::new(),
                calls: BTreeMap::new(),
                call_origins: Vec::new(),
                unsupported_call_sites: HashSet::new(),
                annotation_captures,
            });
        }
        for code in &codes {
            originals.validate_parent(py, code, &codes)?;
        }
        // All temporary Python tree references are released after construction.
        // Operation decoding copies only consumed immutable scalar data.
        decode_definition_stores(py, &decoder, &originals, &mut codes, operations.clone())?;
        decode_calls(py, &decoder, &originals, &mut codes, operations)?;
        decode_annotation_captures(py, &decoder, &mut codes, &first_lines, &recipes)?;
        Ok(Self {
            verified,
            source_id,
            codes,
            by_address,
        })
    }

    pub fn verified(&self) -> &Arc<VerifiedStrictModule> {
        &self.verified
    }

    pub const fn source_id(&self) -> u64 {
        self.source_id
    }

    pub fn code(
        &self,
        py: Python<'_>,
        caller_owned_code: &Bound<'_, PyAny>,
    ) -> PyResult<&InterpreterCode> {
        if current_interpreter() != self.verified.interpreter_id() {
            return Err(invalid(py, "source map used in another interpreter"));
        }
        let actual = unsafe { view(py, caller_owned_code.as_ptr())? };
        if actual.strict_source_id == 0 || actual.strict_source_id != self.source_id {
            return Err(invalid(
                py,
                "caller code has a foreign native source identity",
            ));
        }
        let address = caller_owned_code.as_ptr() as usize;
        let ordinal = self
            .by_address
            .get(&address)
            .ok_or_else(|| invalid(py, "caller code is not an actual retained source node"))?;
        let code = &self.codes[*ordinal as usize];
        debug_assert_eq!(code.address, address);
        Ok(code)
    }

    /// Callback-free role lookup in the actual original provider's native FREE
    /// ordinal order. No cell is read and no stored code pointer is dereferenced.
    /// Unresolved must not authorize a special role or skip a required check.
    pub(crate) fn annotation_capture(
        &self,
        py: Python<'_>,
        caller_owned_provider_code: &Bound<'_, PyAny>,
        free_ordinal: u32,
    ) -> PyResult<&InterpreterAnnotationCaptureOrigin> {
        let code = self.code(py, caller_owned_provider_code)?;
        if code.role() != InterpreterCodeRole::AnnotationProvider {
            return Err(invalid(
                py,
                "capture role query requires an original annotation provider",
            ));
        }
        code.annotation_captures
            .get(free_ordinal as usize)
            .ok_or_else(|| invalid(py, "annotation capture FREE ordinal is out of bounds"))
    }

    /// A miss is only absence of an original definition receipt at this exact
    /// physical site. It is not proof that a Python value is ordinary/unowned.
    /// No supplied name/value or Python attribute is touched, even on a miss.
    pub fn definition_store(
        &self,
        py: Python<'_>,
        caller_owned_code: &Bound<'_, PyAny>,
        instruction_ordinal: u32,
        lane: u8,
    ) -> PyResult<Option<&InterpreterDefinitionStore>> {
        let code = self.code(py, caller_owned_code)?;
        if instruction_ordinal >= code.instruction_count || lane > 1 {
            return Err(invalid(py, "native Store callback site is out of bounds"));
        }
        let key = (instruction_ordinal, lane);
        if code.unsupported_definition_sites.contains(&key) {
            return Err(invalid(
                py,
                "native definition publication has no supported exact receipt",
            ));
        }
        Ok(code.definition_stores.get(&key))
    }

    /// Callback-free lookup by the actual native frame code and final ordinal.
    /// None means no selected source/class/decorator/generic CALL at this physical site;
    /// it is not permission to adopt an unknown result. Missing/unsupported
    /// physical provenance is an error, never an ordinary fallback.
    pub fn call(
        &self,
        py: Python<'_>,
        caller_owned_code: &Bound<'_, PyAny>,
        instruction_ordinal: u32,
    ) -> PyResult<Option<&InterpreterCallReceipt>> {
        let code = self.code(py, caller_owned_code)?;
        if instruction_ordinal >= code.instruction_count {
            return Err(invalid(py, "native CALL callback ordinal is out of bounds"));
        }
        if code.unsupported_call_sites.contains(&instruction_ordinal) {
            return Err(invalid(
                py,
                "native CALL has no complete exact source/input receipt",
            ));
        }
        Ok(code.calls.get(&instruction_ordinal))
    }

    pub fn class_call(
        &self,
        py: Python<'_>,
        caller_owned_code: &Bound<'_, PyAny>,
        instruction_ordinal: u32,
    ) -> PyResult<&InterpreterCallReceipt> {
        let receipt = self
            .call(py, caller_owned_code, instruction_ordinal)?
            .ok_or_else(|| invalid(py, "class construction lacks its exact native CALL receipt"))?;
        if !matches!(
            receipt.origin.role,
            InterpreterCallRole::ClassConstruction { .. }
        ) {
            return Err(invalid(
                py,
                "actual CALL is not an original native class construction",
            ));
        }
        Ok(receipt)
    }

    pub fn generic_scope_call(
        &self,
        py: Python<'_>,
        caller_owned_code: &Bound<'_, PyAny>,
        instruction_ordinal: u32,
    ) -> PyResult<&InterpreterCallReceipt> {
        let receipt = self
            .call(py, caller_owned_code, instruction_ordinal)?
            .ok_or_else(|| invalid(py, "generic scope lacks its exact native CALL receipt"))?;
        if !matches!(
            receipt.origin.role,
            InterpreterCallRole::GenericScopeInvocation { .. }
        ) {
            return Err(invalid(
                py,
                "actual CALL is not an original generic-scope invocation",
            ));
        }
        Ok(receipt)
    }

    pub fn decorator_call(
        &self,
        py: Python<'_>,
        caller_owned_code: &Bound<'_, PyAny>,
        instruction_ordinal: u32,
    ) -> PyResult<&InterpreterCallReceipt> {
        let receipt = self
            .call(py, caller_owned_code, instruction_ordinal)?
            .ok_or_else(|| {
                invalid(
                    py,
                    "decorator application lacks its exact native CALL receipt",
                )
            })?;
        if !matches!(receipt.origin.role, InterpreterCallRole::Decorator { .. }) {
            return Err(invalid(
                py,
                "actual CALL is not an original native decorator application",
            ));
        }
        Ok(receipt)
    }
}

fn current_interpreter() -> i64 {
    unsafe { ffi::PyInterpreterState_GetID(ffi::PyInterpreterState_Get()) }
}

fn invalid(py: Python<'_>, message: &str) -> PyErr {
    strict_runtime_unavailable(py, &format!("interpreter source metadata: {message}"))
}

fn exact_tuple(value: Bound<'_, PyAny>, length: Option<usize>) -> PyResult<Bound<'_, PyTuple>> {
    if !value.is_exact_instance_of::<PyTuple>() {
        return Err(invalid(value.py(), "requires exact tuples"));
    }
    let tuple = value.cast_into::<PyTuple>()?;
    if length.is_some_and(|length| tuple.len() != length) {
        return Err(invalid(tuple.py(), "tuple arity mismatch"));
    }
    Ok(tuple)
}

fn unsigned(value: Bound<'_, PyAny>) -> PyResult<u32> {
    if unsafe { ffi::PyLong_CheckExact(value.as_ptr()) } == 0 {
        return Err(invalid(value.py(), "requires exact integer ordinals"));
    }
    value
        .extract()
        .map_err(|_| invalid(value.py(), "integer ordinal out of range"))
}

fn optional_unsigned(value: Bound<'_, PyAny>) -> PyResult<Option<u32>> {
    if value.is_none() {
        Ok(None)
    } else {
        unsigned(value).map(Some)
    }
}

fn exact_text(value: Bound<'_, PyAny>) -> PyResult<String> {
    if !value.is_exact_instance_of::<PyString>() {
        return Err(invalid(value.py(), "requires exact native text"));
    }
    value
        .extract()
        .map_err(|_| invalid(value.py(), "native text is not UTF-8"))
}

fn native_tree<'py>(
    py: Python<'py>,
    root: &Bound<'py, PyAny>,
) -> PyResult<Vec<(Bound<'py, PyAny>, Option<u32>)>> {
    let source_id = unsafe { view(py, root.as_ptr())? }.strict_source_id;
    if source_id == 0 {
        return Err(invalid(
            py,
            "root is not a native verified-source code object",
        ));
    }
    let mut pending = vec![(root.clone(), None)];
    let mut seen = HashSet::new();
    let mut tree = Vec::new();
    while let Some((code, parent)) = pending.pop() {
        if unsafe { ffi::Py_TYPE(code.as_ptr()) } != std::ptr::addr_of_mut!(ffi::PyCode_Type)
            || !seen.insert(code.as_ptr() as usize)
        {
            return Err(invalid(
                py,
                "final code tree has an invalid or repeated node",
            ));
        }
        let header = unsafe { view(py, code.as_ptr())? };
        if header.strict_source_id != source_id {
            return Err(invalid(py, "final code tree contains a foreign source ID"));
        }
        let ordinal = u32::try_from(tree.len())
            .map_err(|_| invalid(py, "final code tree exceeds wire ordinal capacity"))?;
        let constants = exact_tuple(unsafe { Bound::from_borrowed_ptr(py, header.consts) }, None)?;
        for index in (0..constants.len()).rev() {
            let value = constants.get_item(index)?;
            if unsafe { ffi::Py_TYPE(value.as_ptr()) } == std::ptr::addr_of_mut!(ffi::PyCode_Type) {
                pending.push((value, Some(ordinal)));
            }
        }
        tree.push((code, parent));
    }
    Ok(tree)
}

struct Decoder<'a> {
    source: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> Decoder<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            line_starts: std::iter::once(0)
                .chain(
                    source
                        .bytes()
                        .enumerate()
                        .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
                )
                .collect(),
        }
    }

    fn offset(&self, py: Python<'_>, line: u32, column: u32) -> PyResult<u32> {
        let index =
            line.checked_sub(1)
                .ok_or_else(|| invalid(py, "source line is not one-based"))? as usize;
        let start = *self
            .line_starts
            .get(index)
            .ok_or_else(|| invalid(py, "source line is outside original bytes"))?;
        let mut end = self
            .line_starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.source.len());
        if end > start && self.source.as_bytes()[end - 1] == b'\n' {
            end -= 1;
        }
        if end > start && self.source.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        let offset = start
            .checked_add(column as usize)
            .filter(|offset| *offset <= end && self.source.is_char_boundary(*offset))
            .ok_or_else(|| invalid(py, "source column is not a bounded UTF-8 byte coordinate"))?;
        u32::try_from(offset).map_err(|_| invalid(py, "source offset exceeds wire capacity"))
    }

    fn range(&self, value: Bound<'_, PyAny>) -> PyResult<Option<SourceRange>> {
        if value.is_none() {
            return Ok(None);
        }
        let row = exact_tuple(value, Some(4))?;
        let start = self.offset(
            row.py(),
            unsigned(row.get_item(0)?)?,
            unsigned(row.get_item(1)?)?,
        )?;
        let end = self.offset(
            row.py(),
            unsigned(row.get_item(2)?)?,
            unsigned(row.get_item(3)?)?,
        )?;
        if start > end {
            return Err(invalid(row.py(), "source range is reversed"));
        }
        Ok(Some(SourceRange::new(start, end)))
    }

    fn required_range(&self, value: Bound<'_, PyAny>) -> PyResult<SourceRange> {
        self.range(value.clone())?
            .filter(|range| range.start < range.end)
            .ok_or_else(|| {
                invalid(
                    value.py(),
                    "original operation requires a nonempty source range",
                )
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OriginalOrigin {
    role: InterpreterCodeRole,
    source: SourceIdentity,
    expression_range: Option<SourceRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OriginalParent {
    Exact(OriginalOrigin),
    // Eager comprehensions may be inlined or retained by this actual native
    // compilation. Only the retained final tree chooses the physical parent;
    // this does not infer their frame/lifetime/storage representation.
    EagerComprehension {
        origin: OriginalOrigin,
        outer: Box<OriginalParent>,
    },
}

impl OriginalParent {
    fn matches(&self, parent: &OriginalOrigin, codes: &[InterpreterCode]) -> bool {
        match self {
            Self::Exact(expected) => parent == expected,
            Self::EagerComprehension { origin, outer } => {
                if codes.iter().any(|code| &code.origin == origin) {
                    parent == origin
                } else {
                    outer.matches(parent, codes)
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct SourceParameter {
    index: u32,
    name: String,
    kind: ParameterKind,
}

struct OriginalCode {
    origin: OriginalOrigin,
    parent: Option<OriginalParent>,
    native_ranges: Vec<Option<SourceRange>>,
    parameters: Option<Vec<SourceParameter>>,
    decorators: Vec<SourceRange>,
    class_inputs: Option<OriginalArguments>,
}

#[derive(Clone)]
struct OriginalArguments {
    positional: Vec<InterpreterPositionalEntry>,
    keywords: Vec<InterpreterKeywordEntry>,
    method_channel_possible: bool,
}

impl OriginalArguments {
    fn from_ast(arguments: Option<&ast::Arguments>, method_channel_possible: bool) -> Self {
        Self {
            positional: arguments.map_or_else(Vec::new, |args| {
                args.args
                    .iter()
                    .map(|arg| InterpreterPositionalEntry {
                        kind: if matches!(arg, Expr::Starred(_)) {
                            InterpreterPositionalEntryKind::Star
                        } else {
                            InterpreterPositionalEntryKind::Source
                        },
                        source_range: Some(source_range(
                            if sole_unparenthesized_generator(args).is_some() {
                                args.range
                            } else {
                                arg.range()
                            },
                        )),
                    })
                    .collect()
            }),
            keywords: arguments.map_or_else(Vec::new, |args| {
                args.keywords
                    .iter()
                    .map(|keyword| InterpreterKeywordEntry {
                        kind: if keyword.arg.is_some() {
                            InterpreterKeywordEntryKind::Named
                        } else {
                            InterpreterKeywordEntryKind::Mapping
                        },
                        source_range: source_range(keyword.range),
                        native_name: keyword.arg.as_ref().map(|name| name.as_str().to_owned()),
                    })
                    .collect()
            }),
            method_channel_possible,
        }
    }
}

// CPython parses primary genexp, so a sole unparenthesized generator owns
// the CALL's delimiter pair. Ruff records that exact pair on Arguments and
// keeps the generator's own range interior. Use the parser's grammar flag and
// delimiter range, never guessed adjacent bytes or a source-containment search.
fn sole_unparenthesized_generator(arguments: &ast::Arguments) -> Option<&ast::ExprGenerator> {
    match arguments.args.as_ref() {
        [Expr::Generator(generator)]
            if !generator.parenthesized && arguments.keywords.is_empty() =>
        {
            Some(generator)
        }
        _ => None,
    }
}

struct OriginalSourceCall {
    range: SourceRange,
    owner: SourceIdentity,
    parent: OriginalParent,
    arguments: OriginalArguments,
}

struct OriginalCatalog {
    codes: Vec<OriginalCode>,
    calls: Vec<OriginalSourceCall>,
}

impl OriginalCatalog {
    fn collect(py: Python<'_>, source: &str, facts: &ModuleTypeFacts) -> PyResult<Self> {
        let parsed = ruff_python_parser::parse_module(source)
            .map_err(|error| invalid(py, &format!("original source parse failed: {error}")))?;
        let module = OriginalOrigin {
            role: InterpreterCodeRole::Module,
            source: facts.module_body_identity(),
            expression_range: None,
        };
        let mut collector = OriginalCollector {
            facts,
            tokens: parsed.tokens(),
            catalog: Self {
                codes: vec![OriginalCode {
                    origin: module.clone(),
                    parent: None,
                    native_ranges: vec![None],
                    parameters: None,
                    decorators: Vec::new(),
                    class_inputs: None,
                }],
                calls: Vec::new(),
            },
            current: OriginalParent::Exact(module.clone()),
            lexical_owner: module.source.clone(),
            lexical_path: Vec::new(),
            annotation_owner: Some(module),
            error: None,
        };
        collector.visit_body(&parsed.syntax().body);
        if let Some(error) = collector.error {
            return Err(invalid(py, &error));
        }
        let catalog = collector.catalog;
        // A signed proposal is not permission to guess source correspondence.
        for identity in facts
            .classes
            .iter()
            .map(|f| &f.identity)
            .chain(facts.functions.iter().map(|f| &f.identity))
        {
            let matching = catalog
                .codes
                .iter()
                .filter(|code| {
                    &code.origin.source == identity
                        && matches!(
                            code.origin.role,
                            InterpreterCodeRole::ClassNamespace
                                | InterpreterCodeRole::SourceFunction
                                | InterpreterCodeRole::AsyncSourceFunction
                                | InterpreterCodeRole::Lambda
                        )
                })
                .count();
            if matching != 1 {
                return Err(invalid(
                    py,
                    "checker definition does not match exactly one original AST declaration",
                ));
            }
        }
        Ok(catalog)
    }

    fn match_code(
        &self,
        py: Python<'_>,
        role: InterpreterCodeRole,
        native_range: Option<SourceRange>,
    ) -> PyResult<&OriginalCode> {
        let mut matches = self.codes.iter().filter(|candidate| {
            candidate.origin.role == role && candidate.native_ranges.contains(&native_range)
        });
        let result = matches.next().ok_or_else(|| invalid(py, &format!(
            "native code has no represented original AST role/origin: {role:?} at {native_range:?}",
        )))?;
        if matches.next().is_some() {
            return Err(invalid(
                py,
                &format!(
                    "native code has ambiguous original AST role/origin: {role:?} at {native_range:?}",
                ),
            ));
        }
        Ok(result)
    }

    fn validate_parent(
        &self,
        py: Python<'_>,
        code: &InterpreterCode,
        codes: &[InterpreterCode],
    ) -> PyResult<()> {
        let original = self
            .codes
            .iter()
            .find(|item| item.origin == code.origin)
            .ok_or_else(|| invalid(py, "native code lost its original AST association"))?;
        match (&original.parent, code.parent) {
            (None, None) if code.ordinal == 0 && code.role() == InterpreterCodeRole::Module => {
                Ok(())
            }
            (Some(expected), Some(parent))
                if expected.matches(&codes[parent as usize].origin, codes) =>
            {
                Ok(())
            }
            _ => Err(invalid(
                py,
                "native parent differs from the exact original source scope",
            )),
        }
    }

    fn definition(
        &self,
        py: Python<'_>,
        kind: u32,
        native_range: SourceRange,
    ) -> PyResult<&OriginalCode> {
        let role = match kind {
            1 => InterpreterCodeRole::SourceFunction,
            2 => InterpreterCodeRole::AsyncSourceFunction,
            3 => InterpreterCodeRole::ClassNamespace,
            _ => {
                return Err(invalid(
                    py,
                    "nondefinition requested as a source definition",
                ));
            }
        };
        self.match_code(py, role, Some(native_range))
    }
}

struct OriginalCollector<'a> {
    facts: &'a ModuleTypeFacts,
    tokens: &'a Tokens,
    catalog: OriginalCatalog,
    current: OriginalParent,
    lexical_owner: SourceIdentity,
    lexical_path: Vec<String>,
    annotation_owner: Option<OriginalOrigin>,
    error: Option<String>,
}

impl<'a> OriginalCollector<'a> {
    fn identity(&self, name: &str, range: TextRange, kind: DefinitionKind) -> SourceIdentity {
        let mut path = self.lexical_path.clone();
        path.push(name.to_owned());
        SourceIdentity {
            module: self.facts.module.clone(),
            lexical_qualname: path.join("."),
            source_range: source_range(range),
            definition_kind: kind,
        }
    }

    fn add(
        &mut self,
        role: InterpreterCodeRole,
        source: SourceIdentity,
        expression_range: Option<SourceRange>,
        native_range: SourceRange,
        parent: OriginalParent,
        parameters: Option<Vec<SourceParameter>>,
    ) -> OriginalOrigin {
        let origin = OriginalOrigin {
            role,
            source,
            expression_range,
        };
        if let Some(previous) = self
            .catalog
            .codes
            .iter_mut()
            .find(|item| item.origin == origin)
        {
            if previous.parent.as_ref() != Some(&parent) || previous.parameters.is_some() {
                self.error =
                    Some("duplicate or incompatible original native scope association".into());
            } else if !previous.native_ranges.contains(&Some(native_range)) {
                // One module/class provider spans multiple originally visited
                // annotations; its first native origin is not a body envelope.
                previous.native_ranges.push(Some(native_range));
            }
        } else {
            self.catalog.codes.push(OriginalCode {
                origin: origin.clone(),
                parent: Some(parent),
                native_ranges: vec![Some(native_range)],
                parameters,
                decorators: Vec::new(),
                class_inputs: None,
            });
        }
        origin
    }

    fn definition_shape(
        &mut self,
        origin: &OriginalOrigin,
        decorators: &[ast::Decorator],
        class_inputs: Option<OriginalArguments>,
    ) {
        let original = self
            .catalog
            .codes
            .iter_mut()
            .find(|item| &item.origin == origin)
            .expect("definition was just added to the original catalog");
        original.decorators = decorators
            .iter()
            .map(|decorator| source_range(decorator.expression.range()))
            .collect();
        original.class_inputs = class_inputs;
    }

    fn header(
        &mut self,
        range: TextRange,
        decorators: &[ast::Decorator],
        name: TextRange,
        function: bool,
    ) -> SourceRange {
        let after = decorators.last().map_or(range.start(), |d| d.range().end());
        let header = self.tokens.iter().find(|token| {
            token.range().start() >= after
                && token.range().start() < name.start()
                && if function {
                    matches!(token.kind(), TokenKind::Async | TokenKind::Def)
                } else {
                    token.kind() == TokenKind::Class
                }
        });
        match header {
            Some(header) => SourceRange::new(header.range().start().to_u32(), range.end().to_u32()),
            None => {
                self.error =
                    Some("original declaration lacks its exact parser header token".into());
                source_range(range)
            }
        }
    }

    fn with_current(&mut self, current: OriginalParent, visit: impl FnOnce(&mut Self)) {
        let old = std::mem::replace(&mut self.current, current);
        visit(self);
        self.current = old;
    }

    fn source_body(
        &mut self,
        origin: OriginalOrigin,
        path: String,
        annotations: bool,
        body: &'a [Stmt],
    ) {
        let old_current =
            std::mem::replace(&mut self.current, OriginalParent::Exact(origin.clone()));
        let old_owner = std::mem::replace(&mut self.lexical_owner, origin.source.clone());
        let old_annotations =
            std::mem::replace(&mut self.annotation_owner, annotations.then_some(origin));
        self.lexical_path.push(path);
        self.visit_body(body);
        self.lexical_path.pop();
        self.annotation_owner = old_annotations;
        self.lexical_owner = old_owner;
        self.current = old_current;
    }

    fn defaults(&mut self, parameters: &'a ast::Parameters) {
        for parameter in parameters
            .posonlyargs
            .iter()
            .chain(&parameters.args)
            .chain(&parameters.kwonlyargs)
        {
            if let Some(default) = &parameter.default {
                self.visit_expr(default);
            }
        }
    }

    fn annotations(&mut self, parameters: &'a ast::Parameters, returns: Option<&'a Expr>) {
        for parameter in parameters.posonlyargs.iter().chain(&parameters.args) {
            if let Some(annotation) = parameter.annotation() {
                self.visit_expr(annotation);
            }
        }
        if let Some(parameter) = &parameters.vararg {
            if let Some(annotation) = &parameter.annotation {
                self.visit_expr(annotation);
            }
        }
        for parameter in &parameters.kwonlyargs {
            if let Some(annotation) = parameter.annotation() {
                self.visit_expr(annotation);
            }
        }
        if let Some(parameter) = &parameters.kwarg {
            if let Some(annotation) = &parameter.annotation {
                self.visit_expr(annotation);
            }
        }
        if let Some(returns) = returns {
            self.visit_expr(returns);
        }
    }

    fn type_parameters(
        &mut self,
        owner: &SourceIdentity,
        header: SourceRange,
        parameters: &'a ast::TypeParams,
    ) -> OriginalOrigin {
        let wrapper = self.add(
            InterpreterCodeRole::TypeParameterScope,
            owner.clone(),
            None,
            header,
            self.current.clone(),
            None,
        );
        for parameter in parameters {
            let (name, bound, default) = match parameter {
                ast::TypeParam::TypeVar(p) => {
                    (p.name.as_str(), p.bound.as_deref(), p.default.as_deref())
                }
                ast::TypeParam::TypeVarTuple(p) => (p.name.as_str(), None, p.default.as_deref()),
                ast::TypeParam::ParamSpec(p) => (p.name.as_str(), None, p.default.as_deref()),
            };
            let source = SourceIdentity {
                module: owner.module.clone(),
                lexical_qualname: format!("{}.{}", owner.lexical_qualname, name),
                source_range: source_range(parameter.range()),
                definition_kind: DefinitionKind::Parameter,
            };
            for expression in bound.into_iter().chain(default) {
                let range = source_range(expression.range());
                let evaluator = self.add(
                    InterpreterCodeRole::TypeVariable,
                    source.clone(),
                    Some(range),
                    range,
                    OriginalParent::Exact(wrapper.clone()),
                    None,
                );
                self.with_current(OriginalParent::Exact(evaluator), |this| {
                    this.visit_expr(expression)
                });
            }
        }
        wrapper
    }

    fn comprehension(
        &mut self,
        range: TextRange,
        native_range: TextRange,
        generators: &'a [ast::Comprehension],
        body: &[&'a Expr],
        eager: bool,
    ) {
        let origin = self.add(
            InterpreterCodeRole::Comprehension,
            self.lexical_owner.clone(),
            Some(source_range(range)),
            source_range(native_range),
            self.current.clone(),
            None,
        );
        let Some((first, rest)) = generators.split_first() else {
            self.error = Some("original comprehension has no generator".into());
            return;
        };
        self.visit_expr(&first.iter);
        let inner = if eager {
            OriginalParent::EagerComprehension {
                origin,
                outer: Box::new(self.current.clone()),
            }
        } else {
            OriginalParent::Exact(origin)
        };
        self.with_current(inner, |this| {
            this.visit_expr(&first.target);
            for condition in &first.ifs {
                this.visit_expr(condition);
            }
            for generator in rest {
                this.visit_expr(&generator.iter);
                this.visit_expr(&generator.target);
                for condition in &generator.ifs {
                    this.visit_expr(condition);
                }
            }
            for expression in body {
                this.visit_expr(*expression);
            }
        });
    }
}

impl<'a> Visitor<'a> for OriginalCollector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if self.error.is_some() {
            return;
        }
        match stmt {
            Stmt::FunctionDef(function) => {
                let source = self.identity(
                    function.name.as_str(),
                    function.range,
                    DefinitionKind::Function,
                );
                let header = self.header(
                    function.range,
                    &function.decorator_list,
                    function.name.range(),
                    true,
                );
                for decorator in &function.decorator_list {
                    self.visit_decorator(decorator);
                }
                self.defaults(&function.parameters);
                let parent = if let Some(parameters) = &function.type_params {
                    OriginalParent::Exact(self.type_parameters(&source, header, parameters))
                } else {
                    self.current.clone()
                };
                let role = if function.is_async {
                    InterpreterCodeRole::AsyncSourceFunction
                } else {
                    InterpreterCodeRole::SourceFunction
                };
                let body = self.add(
                    role,
                    source.clone(),
                    None,
                    header,
                    parent.clone(),
                    Some(source_parameters(&function.parameters)),
                );
                self.definition_shape(&body, &function.decorator_list, None);
                // A provider's native parent is the creation scope, not the
                // source function body whose annotations it describes.
                if has_annotations(&function.parameters, function.returns.as_deref()) {
                    let provider = self.add(
                        InterpreterCodeRole::AnnotationProvider,
                        source,
                        None,
                        header,
                        parent,
                        None,
                    );
                    self.with_current(OriginalParent::Exact(provider), |this| {
                        this.annotations(&function.parameters, function.returns.as_deref());
                    });
                }
                self.source_body(
                    body,
                    format!("{}.<locals>", function.name),
                    false,
                    &function.body,
                );
            }
            Stmt::ClassDef(class) => {
                let source = self.identity(class.name.as_str(), class.range, DefinitionKind::Class);
                let header = self.header(
                    class.range,
                    &class.decorator_list,
                    class.name.range(),
                    false,
                );
                for decorator in &class.decorator_list {
                    self.visit_decorator(decorator);
                }
                let parent = if let Some(parameters) = &class.type_params {
                    OriginalParent::Exact(self.type_parameters(&source, header, parameters))
                } else {
                    self.current.clone()
                };
                self.with_current(parent.clone(), |this| {
                    if let Some(arguments) = &class.arguments {
                        this.visit_arguments(arguments);
                    }
                });
                let body = self.add(
                    InterpreterCodeRole::ClassNamespace,
                    source,
                    None,
                    header,
                    parent,
                    None,
                );
                self.definition_shape(
                    &body,
                    &class.decorator_list,
                    Some(OriginalArguments::from_ast(
                        class.arguments.as_deref(),
                        false,
                    )),
                );
                self.source_body(body, class.name.to_string(), true, &class.body);
            }
            Stmt::TypeAlias(alias) => {
                let Expr::Name(name) = alias.name.as_ref() else {
                    self.error = Some("original type alias has a non-name declaration".into());
                    return;
                };
                let source =
                    self.identity(name.id.as_str(), alias.range, DefinitionKind::TypeAlias);
                let range = source_range(alias.range);
                let parent = if let Some(parameters) = &alias.type_params {
                    OriginalParent::Exact(self.type_parameters(&source, range, parameters))
                } else {
                    self.current.clone()
                };
                let evaluator = self.add(
                    InterpreterCodeRole::TypeAlias,
                    source,
                    None,
                    range,
                    parent,
                    None,
                );
                self.with_current(OriginalParent::Exact(evaluator), |this| {
                    this.visit_expr(&alias.value)
                });
            }
            Stmt::AnnAssign(assignment) => {
                self.visit_expr(&assignment.target);
                if let Some(value) = &assignment.value {
                    self.visit_expr(value);
                }
                if let Some(owner) = self.annotation_owner.clone() {
                    let provider = self.add(
                        InterpreterCodeRole::AnnotationProvider,
                        owner.source.clone(),
                        None,
                        source_range(assignment.annotation.range()),
                        OriginalParent::Exact(owner),
                        None,
                    );
                    self.with_current(OriginalParent::Exact(provider), |this| {
                        this.visit_expr(&assignment.annotation);
                    });
                }
                // Function-local variable annotations are not native evaluator
                // bodies. Do not create helper/source authority from their text.
            }
            _ => visitor::walk_stmt(self, stmt),
        }
    }

    fn visit_arguments(&mut self, arguments: &'a ast::Arguments) {
        if self.error.is_some() {
            return;
        }
        if let Some(generator) = sole_unparenthesized_generator(arguments) {
            self.comprehension(
                generator.range,
                arguments.range,
                &generator.generators,
                &[&generator.elt],
                false,
            );
        } else {
            visitor::walk_arguments(self, arguments);
        }
    }

    fn visit_expr(&mut self, expression: &'a Expr) {
        if self.error.is_some() {
            return;
        }
        match expression {
            Expr::Call(call) => {
                let owner = match &self.current {
                    OriginalParent::Exact(origin)
                    | OriginalParent::EagerComprehension { origin, .. } => origin.source.clone(),
                };
                self.catalog.calls.push(OriginalSourceCall {
                    range: source_range(call.range()),
                    owner,
                    parent: self.current.clone(),
                    arguments: OriginalArguments::from_ast(
                        Some(&call.arguments),
                        matches!(call.func.as_ref(), Expr::Attribute(_)),
                    ),
                });
                visitor::walk_expr(self, expression);
            }
            Expr::Lambda(lambda) => {
                let source = self.identity("<lambda>", lambda.range, DefinitionKind::Lambda);
                if let Some(parameters) = &lambda.parameters {
                    self.defaults(parameters);
                }
                let parameters = lambda
                    .parameters
                    .as_deref()
                    .map(source_parameters)
                    .unwrap_or_default();
                let body = self.add(
                    InterpreterCodeRole::Lambda,
                    source.clone(),
                    None,
                    source_range(lambda.range),
                    self.current.clone(),
                    Some(parameters),
                );
                let previous_owner = std::mem::replace(&mut self.lexical_owner, source);
                self.lexical_path.push("<lambda>".into());
                self.with_current(OriginalParent::Exact(body), |this| {
                    this.visit_expr(&lambda.body)
                });
                self.lexical_path.pop();
                self.lexical_owner = previous_owner;
            }
            Expr::Generator(generator) => {
                if !generator.parenthesized {
                    self.error = Some(
                        "unparenthesized generator lacks its original argument delimiters".into(),
                    );
                    return;
                }
                self.comprehension(
                    generator.range,
                    generator.range,
                    &generator.generators,
                    &[&generator.elt],
                    false,
                );
            }
            Expr::ListComp(comprehension) => self.comprehension(
                comprehension.range,
                comprehension.range,
                &comprehension.generators,
                &[&comprehension.elt],
                true,
            ),
            Expr::SetComp(comprehension) => self.comprehension(
                comprehension.range,
                comprehension.range,
                &comprehension.generators,
                &[&comprehension.elt],
                true,
            ),
            Expr::DictComp(comprehension) => {
                let mut elements = Vec::with_capacity(2);
                if let Some(key) = comprehension.key.as_deref() {
                    elements.push(key);
                }
                elements.push(comprehension.value.as_ref());
                self.comprehension(
                    comprehension.range,
                    comprehension.range,
                    &comprehension.generators,
                    &elements,
                    true,
                );
            }
            _ => visitor::walk_expr(self, expression),
        }
    }
}

fn source_range(range: TextRange) -> SourceRange {
    SourceRange::new(range.start().to_u32(), range.end().to_u32())
}

fn source_parameters(parameters: &ast::Parameters) -> Vec<SourceParameter> {
    let mut result = Vec::new();
    let mut push = |name: &str, kind| {
        let index = result.len() as u32;
        result.push(SourceParameter {
            index,
            name: name.to_owned(),
            kind,
        });
    };
    for parameter in &parameters.posonlyargs {
        push(parameter.name().as_str(), ParameterKind::PositionalOnly);
    }
    for parameter in &parameters.args {
        push(
            parameter.name().as_str(),
            ParameterKind::PositionalOrKeyword,
        );
    }
    if let Some(parameter) = &parameters.vararg {
        push(parameter.name.as_str(), ParameterKind::VarArgs);
    }
    for parameter in &parameters.kwonlyargs {
        push(parameter.name().as_str(), ParameterKind::KeywordOnly);
    }
    if let Some(parameter) = &parameters.kwarg {
        push(parameter.name.as_str(), ParameterKind::VarKeywords);
    }
    result
}

fn has_annotations(parameters: &ast::Parameters, returns: Option<&Expr>) -> bool {
    returns.is_some()
        || parameters
            .iter_non_variadic_params()
            .any(|p| p.annotation().is_some())
        || parameters
            .vararg
            .as_ref()
            .is_some_and(|p| p.annotation.is_some())
        || parameters
            .kwarg
            .as_ref()
            .is_some_and(|p| p.annotation.is_some())
}

fn check_function_kind(
    py: Python<'_>,
    layout: &InterpreterNativeLayout,
    original: &OriginalCode,
    facts: &ModuleTypeFacts,
) -> PyResult<()> {
    if !matches!(
        original.origin.role,
        InterpreterCodeRole::SourceFunction
            | InterpreterCodeRole::AsyncSourceFunction
            | InterpreterCodeRole::Lambda
    ) {
        return Ok(());
    }
    let kind = match layout.flags & (CO_GENERATOR | CO_COROUTINE | CO_ASYNC_GENERATOR) {
        0 => FunctionKind::Synchronous,
        CO_GENERATOR => FunctionKind::Generator,
        CO_COROUTINE => FunctionKind::Coroutine,
        CO_ASYNC_GENERATOR => FunctionKind::AsyncGenerator,
        _ => {
            return Err(invalid(
                py,
                "native function has conflicting generator/coroutine flags",
            ));
        }
    };
    let is_async = matches!(kind, FunctionKind::Coroutine | FunctionKind::AsyncGenerator);
    if is_async != (original.origin.role == InterpreterCodeRole::AsyncSourceFunction) {
        return Err(invalid(
            py,
            "native function async flags disagree with original syntax",
        ));
    }
    if let Some(fact) = facts
        .functions
        .iter()
        .find(|fact| fact.identity == original.origin.source)
    {
        if fact.function_kind != kind {
            return Err(invalid(
                py,
                "checker/native original function kind mismatch",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct BindingOrigin {
    kind: u32,
    range: SourceRange,
    phase: u32,
}

impl BindingOrigin {
    fn is_definition(self) -> bool {
        matches!(self.kind, 1..=3)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoreOperand {
    domain: u32,
    index: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StorePhysical {
    form: u32,
    first: StoreOperand,
    second: Option<StoreOperand>,
}

struct StoreEmission {
    ordinal: u32,
    lane: u8,
    physical: StorePhysical,
    context_missing: bool,
}

fn binding_origin(decoder: &Decoder<'_>, value: Bound<'_, PyAny>) -> PyResult<BindingOrigin> {
    let row = exact_tuple(value, Some(4))?;
    let py = row.py();
    let kind = unsigned(row.get_item(0)?)?;
    let range = decoder.required_range(row.get_item(1)?)?;
    let phase = unsigned(row.get_item(2)?)?;
    if kind > 12 || phase > 3 || (matches!(kind, 1..=3) && phase != 0) {
        return Err(invalid(py, "invalid original Store kind/phase"));
    }
    let detail = row.get_item(3)?;
    if kind == 10 {
        let leaves = exact_tuple(detail, None)?;
        if leaves.is_empty() {
            return Err(invalid(py, "pattern Store lacks original leaves"));
        }
        let mut seen = HashSet::new();
        for leaf in leaves.iter() {
            let leaf = exact_tuple(leaf, Some(2))?;
            let kind = unsigned(leaf.get_item(0)?)?;
            let span = decoder.required_range(leaf.get_item(1)?)?;
            if kind > 2 || !range.contains(span) || !seen.insert((kind, span)) {
                return Err(invalid(py, "invalid original pattern capture detail"));
            }
        }
    } else if !detail.is_none() {
        return Err(invalid(py, "unexpected original Store detail"));
    }
    Ok(BindingOrigin { kind, range, phase })
}

/// Validate shape/coordinates only. The interpreter owns the actual cleanup
/// continuation; missing context does not make a physical definition ambiguous.
fn context_missing(decoder: &Decoder<'_>, value: Bound<'_, PyAny>) -> PyResult<bool> {
    if value.is_none() {
        return Ok(true);
    }
    let context = exact_tuple(value, None)?;
    for row in context.iter() {
        let row = exact_tuple(row, Some(6))?;
        let owner = unsigned(row.get_item(0)?)?;
        decoder.required_range(row.get_item(1)?)?;
        let item = optional_unsigned(row.get_item(2)?)?;
        let entry = unsigned(row.get_item(3)?)?;
        let transfer = decoder.range(row.get_item(4)?)?;
        let payload = unsigned(row.get_item(5)?)?;
        if owner > 5
            || entry > 4
            || payload > 2
            || (owner >= 4) != item.is_some()
            || (entry >= 2) != transfer.is_some()
        {
            return Err(invalid(row.py(), "invalid native continuation row"));
        }
        // Return can carry NO_PAYLOAD or RETURN_VALUE: the native compiler may
        // delay a constant value until after cleanup. Never infer it from AST.
    }
    Ok(false)
}

fn store_operand(code: &InterpreterCode, value: Bound<'_, PyAny>) -> PyResult<StoreOperand> {
    let row = exact_tuple(value, Some(2))?;
    let domain = unsigned(row.get_item(0)?)?;
    let index = optional_unsigned(row.get_item(1)?)?;
    let valid = match (domain, index) {
        (0, Some(index)) => (index as usize) < code.layout.locals.len(),
        (1, Some(index)) => (index as usize) < code.native_names.len(),
        (2, None) => true,
        _ => false,
    };
    if !valid {
        return Err(invalid(
            row.py(),
            "native Store operand exceeds its exact code domain",
        ));
    }
    Ok(StoreOperand { domain, index })
}

fn store_emission(
    decoder: &Decoder<'_>,
    code: &InterpreterCode,
    value: Bound<'_, PyAny>,
) -> PyResult<StoreEmission> {
    let row = exact_tuple(value, Some(6))?;
    let py = row.py();
    let ordinal = unsigned(row.get_item(0)?)?;
    let form = unsigned(row.get_item(1)?)?;
    let first = store_operand(code, row.get_item(2)?)?;
    let second_value = row.get_item(3)?;
    let second = if second_value.is_none() {
        None
    } else {
        Some(store_operand(code, second_value)?)
    };
    let lane = unsigned(row.get_item(4)?)?;
    let missing = context_missing(decoder, row.get_item(5)?)?;
    let paired = matches!(form, 1 | 2);
    if ordinal >= code.instruction_count
        || form > 14
        || lane > 1
        || (lane == 1 && form != 1)
        || paired != second.is_some()
    {
        return Err(invalid(py, "invalid native Store ordinal/form/lane"));
    }
    let expected_domain = match form {
        0..=3 | 9 | 10 => 0,
        4..=6 | 11..=13 => 1,
        7 | 8 | 14 => 2,
        _ => unreachable!(),
    };
    if first.domain != expected_domain || second.is_some_and(|operand| operand.domain != 0) {
        return Err(invalid(
            py,
            "native Store form disagrees with operand domains",
        ));
    }
    if matches!(form, 0..=3 | 9 | 10) {
        let selected = if lane == 0 { first } else { second.unwrap() };
        let slot = &code.layout.locals[selected.index.unwrap() as usize];
        let has_required_kind = if matches!(form, 3 | 10) {
            slot.kind & (CELL | FREE) != 0
        } else {
            slot.kind & LOCAL != 0
        };
        if !has_required_kind {
            return Err(invalid(
                py,
                "native Store selects an incompatible localsplus slot",
            ));
        }
    }
    Ok(StoreEmission {
        ordinal,
        lane: lane as u8,
        physical: StorePhysical {
            form,
            first,
            second,
        },
        context_missing: missing,
    })
}

fn definition_target(py: Python<'_>, emission: &StoreEmission) -> PyResult<InterpreterStoreTarget> {
    let selected = if emission.lane == 0 {
        emission.physical.first
    } else {
        emission.physical.second.unwrap()
    };
    let index = selected
        .index
        .ok_or_else(|| invalid(py, "source definition has no native binding slot/name"))?;
    Ok(match emission.physical.form {
        0..=2 => InterpreterStoreTarget::Fast(index),
        3 => InterpreterStoreTarget::Cell(index),
        4 => InterpreterStoreTarget::Name(index),
        5 => InterpreterStoreTarget::Global(index),
        _ => {
            return Err(invalid(
                py,
                "source definition is not a native name publication",
            ));
        }
    })
}

fn definition_body<'a>(
    py: Python<'_>,
    original: &OriginalCode,
    publisher: u32,
    codes: &'a [InterpreterCode],
) -> PyResult<Option<&'a InterpreterCode>> {
    let mut candidates = codes.iter().filter(|candidate| {
        if candidate.origin != original.origin {
            return false;
        }
        let Some(parent) = candidate.parent else {
            return false;
        };
        parent == publisher || {
            let wrapper = &codes[parent as usize];
            wrapper.role() == InterpreterCodeRole::TypeParameterScope
                && wrapper.source() == &original.origin.source
                && wrapper.parent == Some(publisher)
        }
    });
    let result = candidates.next();
    if candidates.next().is_some() {
        return Err(invalid(
            py,
            "definition publication has ambiguous retained native body code",
        ));
    }
    Ok(result)
}

fn operation_gaps(
    decoder: &Decoder<'_>,
    code: &InterpreterCode,
    node_count: usize,
    value: Bound<'_, PyAny>,
) -> PyResult<(
    HashMap<BindingOrigin, Vec<InterpreterDefinitionGap>>,
    HashSet<(u32, u8)>,
)> {
    let rows = exact_tuple(value, None)?;
    let mut definitions: HashMap<BindingOrigin, Vec<InterpreterDefinitionGap>> = HashMap::new();
    let mut missing_store = HashSet::new();
    for row in rows.iter() {
        let row = exact_tuple(row, Some(6))?;
        let py = row.py();
        let reason = unsigned(row.get_item(0)?)?;
        let source = row.get_item(1)?;
        let mut definition = None;
        if !source.is_none() {
            let origin = exact_tuple(source, Some(2))?;
            match unsigned(origin.get_item(0)?)? {
                0 => {
                    decoder.required_range(origin.get_item(1)?)?;
                }
                1 => {
                    let origin = binding_origin(decoder, origin.get_item(1)?)?;
                    if origin.is_definition() {
                        definition = Some(origin);
                    }
                }
                2 => {
                    // No CALL selection is performed. Validate only the origin
                    // value's exact wire domain; a later decorator adapter must
                    // consume the actual CALL/input receipts independently.
                    let call = exact_tuple(origin.get_item(1)?, Some(3))?;
                    let kind = unsigned(call.get_item(0)?)?;
                    decoder.required_range(call.get_item(1)?)?;
                    let detail = optional_unsigned(call.get_item(2)?)?;
                    if kind > 9
                        || (matches!(kind, 2..=4)
                            && !detail.is_some_and(|id| (id as usize) < node_count))
                    {
                        return Err(invalid(py, "invalid native CALL gap origin"));
                    }
                }
                _ => return Err(invalid(py, "unknown native operation gap family")),
            }
        }
        let ordinal = optional_unsigned(row.get_item(2)?)?;
        let lane = optional_unsigned(row.get_item(3)?)?;
        let opcode = optional_unsigned(row.get_item(4)?)?;
        context_missing(decoder, row.get_item(5)?)?;
        if reason > 11
            || ordinal.is_some() != lane.is_some()
            || ordinal.is_some() != opcode.is_some()
            || ordinal.is_some_and(|ordinal| ordinal >= code.instruction_count)
            || lane.is_some_and(|lane| lane > 1)
            || opcode.is_some_and(|opcode| opcode > 255)
        {
            return Err(invalid(py, "invalid native operation gap location/domain"));
        }
        if let Some(origin) = definition {
            if !matches!(reason, 0 | 1 | 4 | 9)
                || (matches!(reason, 0 | 4) && ordinal.is_some())
                || (matches!(reason, 1 | 9) && ordinal.is_none())
            {
                return Err(invalid(py, "invalid definition-specific native gap"));
            }
            definitions
                .entry(origin)
                .or_default()
                .push(InterpreterDefinitionGap {
                    reason,
                    instruction_ordinal: ordinal,
                    lane: lane.map(|lane| lane as u8),
                    opcode,
                });
        } else if reason == 3 {
            if let (Some(ordinal), Some(lane)) = (ordinal, lane) {
                missing_store.insert((ordinal, lane as u8));
            } else {
                return Err(invalid(
                    py,
                    "missing native Store origin has no physical site",
                ));
            }
        }
    }
    Ok((definitions, missing_store))
}

fn decode_definition_stores(
    py: Python<'_>,
    decoder: &Decoder<'_>,
    originals: &OriginalCatalog,
    codes: &mut [InterpreterCode],
    tables: Bound<'_, PyTuple>,
) -> PyResult<()> {
    // The native producer serializes OperationTables in final CodeNode order.
    // Do this count/name proof before any instruction ordinal is interpreted.
    for (index, code) in codes.iter_mut().enumerate() {
        let table = exact_tuple(tables.get_item(index)?, Some(7))?;
        let ordinal = unsigned(table.get_item(0)?)?;
        let instructions = unsigned(table.get_item(1)?)?;
        let byte_size = unsigned(table.get_item(2)?)? as usize;
        let names = exact_tuple(table.get_item(3)?, Some(code.native_names.len()))?;
        if ordinal != code.ordinal
            || instructions == 0
            || instructions as usize > code.byte_size / 2
            || byte_size != code.byte_size
        {
            return Err(invalid(
                py,
                "native operation table code/count/size mismatch",
            ));
        }
        for (actual, expected) in names.iter().zip(&code.native_names) {
            if exact_text(actual)? != *expected {
                return Err(invalid(
                    py,
                    "operation table names differ from actual native code",
                ));
            }
        }
        // Store/CALL origin containers remain mandatory for actual source
        // publication and call authority; scalar read schedules are absent.
        exact_tuple(table.get_item(4)?, None)?;
        exact_tuple(table.get_item(5)?, None)?;
        exact_tuple(table.get_item(6)?, None)?;
        code.instruction_count = instructions;
    }

    let mut covered_bodies = HashSet::new();
    for index in 0..codes.len() {
        let code = &codes[index];
        let table = exact_tuple(tables.get_item(index)?, Some(7))?;
        let (mut gaps, missing_store) =
            operation_gaps(decoder, code, codes.len(), table.get_item(6)?)?;
        let stores = exact_tuple(table.get_item(4)?, None)?;
        let mut occupied = HashSet::new();
        let mut physical_forms: HashMap<u32, StorePhysical> = HashMap::new();
        let mut definition_origins = HashSet::new();
        let mut selected = BTreeMap::new();
        let mut unsupported = HashSet::new();
        for row in stores.iter() {
            let row = exact_tuple(row, Some(2))?;
            let origin = binding_origin(decoder, row.get_item(0)?)?;
            let emissions = exact_tuple(row.get_item(1)?, None)?;
            let mut decoded = Vec::with_capacity(emissions.len());
            let mut previous = None;
            for emission in emissions.iter() {
                let emission = store_emission(decoder, code, emission)?;
                let key = (emission.ordinal, emission.lane);
                if previous.is_some_and(|previous| previous >= key)
                    || !occupied.insert(key)
                    || missing_store.contains(&key)
                {
                    return Err(invalid(
                        py,
                        "duplicate/conflicting native Store physical site",
                    ));
                }
                previous = Some(key);
                if let Some(prior) = physical_forms.insert(emission.ordinal, emission.physical) {
                    if prior != emission.physical {
                        return Err(invalid(
                            py,
                            "paired native Store lanes disagree on instruction operands/form",
                        ));
                    }
                }
                decoded.push(emission);
            }
            if !origin.is_definition() {
                // This is not a negative value/owner fact. The callback can
                // proceed with its ordinary namespace semantics without looking
                // at a user key, callable, name, profile or Python attribute.
                continue;
            }
            if !definition_origins.insert(origin) {
                return Err(invalid(py, "duplicate original definition Store row"));
            }
            let original = originals.definition(py, origin.kind, origin.range)?;
            let body = definition_body(py, original, code.ordinal, codes)?;
            let issues = gaps.remove(&origin).unwrap_or_default();
            if decoded.is_empty() {
                if !issues.iter().any(|gap| matches!(gap.reason, 0 | 1)) {
                    return Err(invalid(
                        py,
                        "missing definition Store coverage has no native receipt",
                    ));
                }
            } else if issues.iter().any(|gap| gap.reason == 0) {
                return Err(invalid(
                    py,
                    "retained definition Store contradicts an eliminated origin",
                ));
            }
            if let Some(body) = body {
                covered_bodies.insert(body.ordinal);
            } else if !decoded.is_empty() || issues.iter().any(|gap| gap.reason == 1) {
                return Err(invalid(
                    py,
                    "actual definition site has no retained original native body",
                ));
            }
            let divergent = decoded.first().is_some_and(|first| {
                decoded
                    .iter()
                    .any(|other| first.physical != other.physical || first.lane != other.lane)
            });
            if divergent && !issues.iter().any(|gap| gap.reason == 4) {
                return Err(invalid(
                    py,
                    "conflicting native definition forms lost their gap receipt",
                ));
            }
            for issue in &issues {
                if issue.reason == 1 {
                    let key = (issue.instruction_ordinal.unwrap(), issue.lane.unwrap());
                    if occupied.contains(&key) || !unsupported.insert(key) {
                        return Err(invalid(
                            py,
                            "unsupported definition site has conflicting physical proof",
                        ));
                    }
                }
                if issue.reason == 9
                    && !decoded.iter().any(|emission| {
                        Some(emission.ordinal) == issue.instruction_ordinal
                            && Some(emission.lane) == issue.lane
                            && emission.context_missing
                    })
                {
                    return Err(invalid(
                        py,
                        "missing-context receipt has no matching definition emission",
                    ));
                }
            }
            for emission in decoded {
                if emission.context_missing
                    && !issues.iter().any(|gap| {
                        gap.reason == 9
                            && gap.instruction_ordinal == Some(emission.ordinal)
                            && gap.lane == Some(emission.lane)
                    })
                {
                    return Err(invalid(
                        py,
                        "unavailable definition context lacks its explicit receipt",
                    ));
                }
                let body =
                    body.ok_or_else(|| invalid(py, "definition body association is absent"))?;
                let target = definition_target(py, &emission)?;
                selected.insert(
                    (emission.ordinal, emission.lane),
                    InterpreterDefinitionStore {
                        source: original.origin.source.clone(),
                        role: original.origin.role,
                        body_code_ordinal: body.ordinal,
                        native_origin: origin.range,
                        instruction_ordinal: emission.ordinal,
                        lane: emission.lane,
                        form: emission.physical.form,
                        target,
                        // Gap4/9 remain data. Native owns the exact current control
                        // path and retirement; no uniform-shape policy is selected.
                        gaps: issues.clone(),
                    },
                );
            }
        }
        if !gaps.is_empty() {
            return Err(invalid(
                py,
                "native definition gap lost its original Store row",
            ));
        }
        codes[index].definition_stores = selected;
        codes[index].unsupported_definition_sites = unsupported;
    }
    for code in codes.iter().filter(|code| code.role().is_definition()) {
        if !covered_bodies.contains(&code.ordinal) {
            return Err(invalid(
                py,
                "retained original definition has no native publication-origin row",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CallWireOrigin {
    kind: u32,
    range: SourceRange,
    detail: Option<u32>,
}

fn call_wire_origin(
    decoder: &Decoder<'_>,
    code: &InterpreterCode,
    codes: &[InterpreterCode],
    value: Bound<'_, PyAny>,
) -> PyResult<CallWireOrigin> {
    let row = exact_tuple(value, Some(3))?;
    let py = row.py();
    let kind = unsigned(row.get_item(0)?)?;
    let range = decoder.required_range(row.get_item(1)?)?;
    let detail = optional_unsigned(row.get_item(2)?)?;
    let valid_detail = match kind {
        0 | 9 => detail.is_none(),
        1 | 5..=8 => detail.is_some(),
        2..=4 => detail.is_some_and(|ordinal| {
            codes.get(ordinal as usize).is_some_and(|child| {
                child.parent == Some(code.ordinal)
                    && child.native_range == Some(range)
                    && match kind {
                        2 => child.role() == InterpreterCodeRole::ClassNamespace,
                        3 => child.role() == InterpreterCodeRole::TypeParameterScope,
                        4 => child.role() == InterpreterCodeRole::Comprehension,
                        _ => false,
                    }
            })
        }),
        _ => false,
    };
    if !valid_detail {
        return Err(invalid(
            py,
            "native CALL role/detail is not its exact direct child/index domain",
        ));
    }
    Ok(CallWireOrigin {
        kind,
        range,
        detail,
    })
}

enum OriginalCallInputs<'a> {
    Source(&'a OriginalArguments),
    Decorator,
    Class {
        arguments: &'a OriginalArguments,
        generic: bool,
    },
    GenericScope {
        parameter_count: u32,
    },
}

fn original_call<'a>(
    py: Python<'_>,
    wire: CallWireOrigin,
    code: &InterpreterCode,
    codes: &[InterpreterCode],
    originals: &'a OriginalCatalog,
) -> PyResult<Option<(InterpreterCallOrigin, OriginalCallInputs<'a>)>> {
    match wire.kind {
        0 => {
            let mut candidates = originals.calls.iter().filter(|call| {
                call.range == wire.range && call.parent.matches(&code.origin, codes)
            });
            let original = candidates.next().ok_or_else(|| {
                invalid(py, "source CALL lacks its exact original expression/parent")
            })?;
            if candidates.next().is_some() {
                return Err(invalid(
                    py,
                    "source CALL has ambiguous original expression ownership",
                ));
            }
            Ok(Some((
                InterpreterCallOrigin {
                    source_definition: original.owner.clone(),
                    original_range: wire.range,
                    role: InterpreterCallRole::SourceExpression,
                },
                OriginalCallInputs::Source(&original.arguments),
            )))
        }
        1 => {
            let mut candidates = originals.codes.iter().filter(|original| {
                original.origin.role.is_definition()
                    && original.native_ranges.contains(&Some(wire.range))
            });
            let original = candidates
                .next()
                .ok_or_else(|| invalid(py, "decorator CALL lacks its exact original definition"))?;
            if candidates.next().is_some() {
                return Err(invalid(
                    py,
                    "decorator CALL has ambiguous original definition",
                ));
            }
            let mut parent = original.parent.as_ref().ok_or_else(|| {
                invalid(py, "decorated definition lacks its original creation scope")
            })?;
            // Decorators apply outside the generic wrapper, after that wrapper
            // returns the newly created definition. Resolve this through the
            // SAME original scope relation, including a dead wrapper with no ID.
            if let OriginalParent::Exact(wrapper) = parent {
                if wrapper.role == InterpreterCodeRole::TypeParameterScope {
                    parent = originals
                        .codes
                        .iter()
                        .find(|item| &item.origin == wrapper)
                        .and_then(|item| item.parent.as_ref())
                        .ok_or_else(|| {
                            invalid(
                                py,
                                "decorated generic definition lost its original outer scope",
                            )
                        })?;
                }
            }
            if !parent.matches(&code.origin, codes) {
                return Err(invalid(
                    py,
                    "decorator CALL belongs to a different original creation scope",
                ));
            }
            let index = wire.detail.unwrap();
            let expression_range = *original.decorators.get(index as usize).ok_or_else(|| {
                invalid(
                    py,
                    "decorator CALL index exceeds the exact original declaration",
                )
            })?;
            Ok(Some((
                InterpreterCallOrigin {
                    source_definition: original.origin.source.clone(),
                    original_range: wire.range,
                    role: InterpreterCallRole::Decorator {
                        index,
                        expression_range,
                    },
                },
                OriginalCallInputs::Decorator,
            )))
        }
        2 => {
            let child = &codes[wire.detail.unwrap() as usize];
            let original = originals.definition(py, 3, wire.range)?;
            if child.source() != &original.origin.source {
                return Err(invalid(
                    py,
                    "class CALL child differs from the exact original class",
                ));
            }
            let arguments = original
                .class_inputs
                .as_ref()
                .ok_or_else(|| invalid(py, "class CALL lacks its original bases/keywords"))?;
            let generic = matches!(&original.parent,
                Some(OriginalParent::Exact(parent)) if parent.role == InterpreterCodeRole::TypeParameterScope);
            Ok(Some((
                InterpreterCallOrigin {
                    source_definition: original.origin.source.clone(),
                    original_range: wire.range,
                    role: InterpreterCallRole::ClassConstruction {
                        class_body_ordinal: child.ordinal,
                    },
                },
                OriginalCallInputs::Class { arguments, generic },
            )))
        }
        3 => {
            let child = &codes[wire.detail.unwrap() as usize];
            let original = originals.match_code(
                py,
                InterpreterCodeRole::TypeParameterScope,
                Some(wire.range),
            )?;
            if child.source() != &original.origin.source {
                return Err(invalid(
                    py,
                    "generic CALL child differs from its original definition",
                ));
            }
            let parameter_count = u32::try_from(child.layout.parameters.len())
                .map_err(|_| invalid(py, "generic scope parameter count overflows"))?;
            if child
                .layout
                .parameters
                .iter()
                .any(|parameter| parameter.kind != ParameterKind::PositionalOrKeyword)
            {
                return Err(invalid(
                    py,
                    "generic wrapper has an unrepresented native parameter channel",
                ));
            }
            Ok(Some((
                InterpreterCallOrigin {
                    source_definition: original.origin.source.clone(),
                    original_range: wire.range,
                    role: InterpreterCallRole::GenericScopeInvocation {
                        scope_ordinal: child.ordinal,
                    },
                },
                OriginalCallInputs::GenericScope { parameter_count },
            )))
        }
        // Other actual compiler roles stay nonselected; no helper is relabelled
        // as a source expression, definition or factory.
        _ => Ok(None),
    }
}

fn call_input_layout(
    decoder: &Decoder<'_>,
    value: Bound<'_, PyAny>,
) -> PyResult<InterpreterCallInputLayout> {
    let row = exact_tuple(value, Some(4))?;
    let py = row.py();
    let channel = match unsigned(row.get_item(0)?)? {
        0 => InterpreterCallChannel::Null,
        1 => InterpreterCallChannel::MethodSelfOrNull,
        2 => InterpreterCallChannel::LeadingArgument,
        _ => return Err(invalid(py, "unknown native CALL input channel")),
    };
    let preloaded_value_count = unsigned(row.get_item(1)?)?;
    let positional = exact_tuple(row.get_item(2)?, Some(2))?;
    let positional_kind = match unsigned(positional.get_item(0)?)? {
        0 => InterpreterPositionalKind::Vector,
        1 => InterpreterPositionalKind::ExpandedEmpty,
        2 => InterpreterPositionalKind::SoleStarDeferred,
        3 => InterpreterPositionalKind::ExpandedDirectTuple,
        4 => InterpreterPositionalKind::ExpandedListAtFirstStar,
        5 => InterpreterPositionalKind::ExpandedListBeforeArguments,
        _ => return Err(invalid(py, "unknown native positional preparation")),
    };
    let entries = exact_tuple(positional.get_item(1)?, None)?;
    let mut positional_entries = Vec::with_capacity(entries.len());
    let mut generic_base = false;
    for (index, entry) in entries.iter().enumerate() {
        let entry = exact_tuple(entry, Some(2))?;
        let kind = match unsigned(entry.get_item(0)?)? {
            0 => InterpreterPositionalEntryKind::Source,
            1 => InterpreterPositionalEntryKind::Star,
            2 => InterpreterPositionalEntryKind::GenericBaseInjected,
            _ => return Err(invalid(py, "unknown original positional entry")),
        };
        let span = decoder.range(entry.get_item(1)?)?;
        if kind == InterpreterPositionalEntryKind::GenericBaseInjected {
            if span.is_some() || generic_base || index + 1 != entries.len() {
                return Err(invalid(
                    py,
                    "compiler generic base is not one explicit trailing input",
                ));
            }
            generic_base = true;
        } else if !span.is_some_and(|range| range.start < range.end) {
            return Err(invalid(
                py,
                "source positional input lacks its original range",
            ));
        }
        positional_entries.push(InterpreterPositionalEntry {
            kind,
            source_range: span,
        });
    }
    let stars = positional_entries
        .iter()
        .filter(|entry| entry.kind == InterpreterPositionalEntryKind::Star)
        .count();
    let empty = preloaded_value_count == 0 && positional_entries.is_empty();
    let positional_valid = match positional_kind {
        InterpreterPositionalKind::Vector => stars == 0,
        InterpreterPositionalKind::ExpandedEmpty => empty,
        InterpreterPositionalKind::SoleStarDeferred => {
            preloaded_value_count == 0 && positional_entries.len() == 1 && stars == 1
        }
        InterpreterPositionalKind::ExpandedDirectTuple => !empty && stars == 0,
        InterpreterPositionalKind::ExpandedListAtFirstStar => stars != 0,
        InterpreterPositionalKind::ExpandedListBeforeArguments => !empty,
    };
    if !positional_valid {
        return Err(invalid(
            py,
            "native positional preparation contradicts its actual inputs",
        ));
    }

    let keywords = exact_tuple(row.get_item(3)?, Some(4))?;
    let keyword_kind = match unsigned(keywords.get_item(0)?)? {
        0 => InterpreterKeywordKind::None,
        1 => InterpreterKeywordKind::NamesTuple,
        2 => InterpreterKeywordKind::ExpandedGroups,
        _ => return Err(invalid(py, "unknown native keyword preparation")),
    };
    let names = keywords.get_item(1)?;
    let keyword_names = if names.is_none() {
        None
    } else {
        Some(
            exact_tuple(names, None)?
                .iter()
                .map(exact_text)
                .collect::<PyResult<Vec<_>>>()?,
        )
    };
    let entries = exact_tuple(keywords.get_item(2)?, None)?;
    let mut keyword_entries = Vec::with_capacity(entries.len());
    let mut named = HashSet::new();
    for entry in entries.iter() {
        let entry = exact_tuple(entry, Some(3))?;
        let kind = match unsigned(entry.get_item(0)?)? {
            0 => InterpreterKeywordEntryKind::Named,
            1 => InterpreterKeywordEntryKind::Mapping,
            _ => return Err(invalid(py, "unknown original keyword entry")),
        };
        let source_range = decoder.required_range(entry.get_item(1)?)?;
        let name = entry.get_item(2)?;
        let native_name = if name.is_none() {
            None
        } else {
            Some(exact_text(name)?)
        };
        if (kind == InterpreterKeywordEntryKind::Named) != native_name.is_some()
            || native_name
                .as_ref()
                .is_some_and(|name| !named.insert(name.clone()))
        {
            return Err(invalid(
                py,
                "native keyword entry has missing/duplicate/wrong-kind name",
            ));
        }
        keyword_entries.push(InterpreterKeywordEntry {
            kind,
            source_range,
            native_name,
        });
    }
    let groups = exact_tuple(keywords.get_item(3)?, None)?;
    let mut keyword_groups = Vec::with_capacity(groups.len());
    let mut covered = 0usize;
    let mut previous_named = false;
    for group in groups.iter() {
        let group = exact_tuple(group, Some(4))?;
        let kind = match unsigned(group.get_item(0)?)? {
            0 => InterpreterKeywordEntryKind::Named,
            1 => InterpreterKeywordEntryKind::Mapping,
            _ => return Err(invalid(py, "unknown native keyword group")),
        };
        let first = unsigned(group.get_item(1)?)?;
        let count = unsigned(group.get_item(2)?)?;
        let map_style = match optional_unsigned(group.get_item(3)?)? {
            None => None,
            Some(0) => Some(InterpreterKeywordMapStyle::BuildMap),
            Some(1) => Some(InterpreterKeywordMapStyle::MapAdd),
            _ => return Err(invalid(py, "unknown native keyword map construction")),
        };
        let end = (first as usize)
            .checked_add(count as usize)
            .ok_or_else(|| invalid(py, "native keyword group overflows"))?;
        if first as usize != covered
            || count == 0
            || end > keyword_entries.len()
            || keyword_entries[first as usize..end]
                .iter()
                .any(|entry| entry.kind != kind)
            || match kind {
                InterpreterKeywordEntryKind::Named => map_style.is_none() || previous_named,
                InterpreterKeywordEntryKind::Mapping => count != 1 || map_style.is_some(),
            }
        {
            return Err(invalid(
                py,
                "native keyword groups do not cover original contiguous inputs",
            ));
        }
        covered = end;
        previous_named = kind == InterpreterKeywordEntryKind::Named;
        keyword_groups.push(InterpreterKeywordGroup {
            kind,
            first,
            count,
            map_style,
        });
    }
    let keyword_valid = match keyword_kind {
        InterpreterKeywordKind::None => {
            keyword_names.is_none() && keyword_entries.is_empty() && keyword_groups.is_empty()
        }
        InterpreterKeywordKind::NamesTuple => {
            !keyword_entries.is_empty()
                && keyword_groups.is_empty()
                && keyword_entries
                    .iter()
                    .all(|entry| entry.kind == InterpreterKeywordEntryKind::Named)
                && keyword_names.as_ref().is_some_and(|names| {
                    names.len() == keyword_entries.len()
                        && names
                            .iter()
                            .zip(&keyword_entries)
                            .all(|(name, entry)| entry.native_name.as_ref() == Some(name))
                })
        }
        InterpreterKeywordKind::ExpandedGroups => {
            keyword_names.is_none()
                && !keyword_entries.is_empty()
                && covered == keyword_entries.len()
        }
    };
    if !keyword_valid {
        return Err(invalid(
            py,
            "native keyword plan lost its exact names/group payload",
        ));
    }
    Ok(InterpreterCallInputLayout {
        channel,
        preloaded_value_count,
        positional_kind,
        positional_entries,
        keyword_kind,
        keyword_names,
        keyword_entries,
        keyword_groups,
    })
}

fn validate_call_source_inputs(
    py: Python<'_>,
    original: &OriginalCallInputs<'_>,
    input: &InterpreterCallInputLayout,
) -> PyResult<()> {
    let (arguments, prefix, generic, channel_valid) = match original {
        OriginalCallInputs::Source(arguments) => (
            Some(*arguments),
            0,
            false,
            input.channel == InterpreterCallChannel::Null
                || (input.channel == InterpreterCallChannel::MethodSelfOrNull
                    && arguments.method_channel_possible),
        ),
        OriginalCallInputs::Decorator => (
            None,
            0,
            false,
            input.channel == InterpreterCallChannel::LeadingArgument,
        ),
        OriginalCallInputs::Class { arguments, generic } => (
            Some(*arguments),
            2,
            *generic,
            input.channel == InterpreterCallChannel::Null,
        ),
        OriginalCallInputs::GenericScope { parameter_count } => (
            None,
            parameter_count.saturating_sub(1),
            false,
            input.channel
                == if *parameter_count == 0 {
                    InterpreterCallChannel::Null
                } else {
                    InterpreterCallChannel::LeadingArgument
                },
        ),
    };
    if input.preloaded_value_count != prefix || !channel_valid {
        return Err(invalid(
            py,
            "native CALL channel/prefix disagrees with its actual source role",
        ));
    }
    let mut positional = arguments.map_or_else(Vec::new, |args| args.positional.clone());
    if generic {
        positional.push(InterpreterPositionalEntry {
            kind: InterpreterPositionalEntryKind::GenericBaseInjected,
            source_range: None,
        });
    }
    if positional != input.positional_entries {
        return Err(invalid(
            py,
            "native CALL positional inputs are not the exact original arguments",
        ));
    }
    let keywords = arguments.map_or(&[][..], |args| args.keywords.as_slice());
    // Native names are the compiler's exact payload (including its identifier
    // normalization). Original source association is by range/kind, never a
    // second mangling/normalization or spelling-based target selection.
    if keywords.len() != input.keyword_entries.len()
        || keywords
            .iter()
            .zip(&input.keyword_entries)
            .any(|(original, native)| {
                original.kind != native.kind || original.source_range != native.source_range
            })
        || (matches!(
            original,
            OriginalCallInputs::Decorator | OriginalCallInputs::GenericScope { .. }
        ) && (input.positional_kind != InterpreterPositionalKind::Vector
            || input.keyword_kind != InterpreterKeywordKind::None))
    {
        return Err(invalid(
            py,
            "native CALL keyword/source role association is incomplete",
        ));
    }
    Ok(())
}

struct DecodedCall {
    ordinal: u32,
    offset: Option<u32>,
    form: InterpreterCallForm,
    count: Option<u32>,
    input: InterpreterCallInputLayout,
    context_missing: bool,
}

fn call_emission(
    decoder: &Decoder<'_>,
    code: &InterpreterCode,
    value: Bound<'_, PyAny>,
) -> PyResult<DecodedCall> {
    let row = exact_tuple(value, Some(6))?;
    let py = row.py();
    let ordinal = unsigned(row.get_item(0)?)?;
    let offset = optional_unsigned(row.get_item(1)?)?;
    let form = match unsigned(row.get_item(2)?)? {
        0 => InterpreterCallForm::Positional,
        1 => InterpreterCallForm::Keywords,
        2 => InterpreterCallForm::Expanded,
        _ => return Err(invalid(py, "unknown native CALL form")),
    };
    let count = optional_unsigned(row.get_item(3)?)?;
    let input = call_input_layout(decoder, row.get_item(4)?)?;
    let context_missing = context_missing(decoder, row.get_item(5)?)?;
    if ordinal >= code.instruction_count
        || offset.is_some_and(|offset| offset as usize >= code.byte_size || offset % 2 != 0)
    {
        return Err(invalid(
            py,
            "native CALL physical receipt exceeds its exact code",
        ));
    }
    let valid_form = match form {
        InterpreterCallForm::Positional => {
            input.positional_kind == InterpreterPositionalKind::Vector
                && input.keyword_kind == InterpreterKeywordKind::None
        }
        InterpreterCallForm::Keywords => {
            input.positional_kind == InterpreterPositionalKind::Vector
                && input.keyword_kind == InterpreterKeywordKind::NamesTuple
        }
        InterpreterCallForm::Expanded => {
            input.positional_kind != InterpreterPositionalKind::Vector
                && input.keyword_kind != InterpreterKeywordKind::NamesTuple
                && input.channel == InterpreterCallChannel::Null
        }
    };
    let expected = input
        .preloaded_value_count
        .checked_add(
            u32::try_from(input.positional_entries.len())
                .map_err(|_| invalid(py, "native positional input count overflows"))?,
        )
        .and_then(|count| count.checked_add(input.keyword_entries.len().try_into().ok()?));
    if !valid_form
        || match form {
            InterpreterCallForm::Expanded => count.is_some(),
            _ => expected.is_none() || count != expected,
        }
    {
        return Err(invalid(
            py,
            "native CALL form/value count contradicts its input recipe",
        ));
    }
    Ok(DecodedCall {
        ordinal,
        offset,
        form,
        count,
        input,
        context_missing,
    })
}

fn call_gaps(
    decoder: &Decoder<'_>,
    code: &InterpreterCode,
    codes: &[InterpreterCode],
    value: Bound<'_, PyAny>,
) -> PyResult<(
    HashMap<CallWireOrigin, Vec<InterpreterCallGap>>,
    HashSet<u32>,
)> {
    let rows = exact_tuple(value, None)?;
    let py = rows.py();
    let mut origins: HashMap<CallWireOrigin, Vec<InterpreterCallGap>> = HashMap::new();
    let mut missing = HashSet::new();
    for row in rows.iter() {
        let row = exact_tuple(row, Some(6))?;
        let reason = unsigned(row.get_item(0)?)?;
        let source = row.get_item(1)?;
        let origin = if source.is_none() {
            None
        } else {
            let source = exact_tuple(source, Some(2))?;
            if unsigned(source.get_item(0)?)? != 2 {
                continue;
            }
            Some(call_wire_origin(decoder, code, codes, source.get_item(1)?)?)
        };
        if origin.is_none() && reason != 5 {
            continue;
        }
        let ordinal = optional_unsigned(row.get_item(2)?)?;
        let lane = optional_unsigned(row.get_item(3)?)?;
        let opcode = optional_unsigned(row.get_item(4)?)?;
        let context_unavailable = context_missing(decoder, row.get_item(5)?)?;
        let physical = matches!(reason, 1 | 5 | 6 | 9 | 10);
        if !matches!(reason, 0 | 1 | 4..=10)
            || physical != ordinal.is_some()
            || physical != lane.is_some()
            || physical != opcode.is_some()
            || ordinal.is_some_and(|ordinal| ordinal >= code.instruction_count)
            || lane.is_some_and(|lane| lane > 1 || (reason != 1 && lane != 0))
            || opcode.is_some_and(|opcode| opcode > 255)
            || (matches!(reason, 0 | 4 | 9) && !context_unavailable)
            || (reason == 5) != origin.is_none()
        {
            return Err(invalid(
                py,
                "invalid native CALL gap domain or physical provenance",
            ));
        }
        if let Some(origin) = origin {
            origins.entry(origin).or_default().push(InterpreterCallGap {
                reason,
                instruction_ordinal: ordinal,
                lane: lane.map(|lane| lane as u8),
                opcode,
                context_unavailable,
            });
        } else {
            missing.insert(ordinal.unwrap());
        }
    }
    Ok((origins, missing))
}

fn decode_calls(
    py: Python<'_>,
    decoder: &Decoder<'_>,
    originals: &OriginalCatalog,
    codes: &mut [InterpreterCode],
    tables: Bound<'_, PyTuple>,
) -> PyResult<()> {
    let mut classes = HashSet::new();
    let mut generic_scopes = HashSet::new();
    let mut decorators = HashSet::new();
    for index in 0..codes.len() {
        let code = &codes[index];
        let table = exact_tuple(tables.get_item(index)?, Some(7))?;
        let (mut gaps, mut unsupported) = call_gaps(decoder, code, codes, table.get_item(6)?)?;
        let calls = exact_tuple(table.get_item(5)?, None)?;
        let mut origins = HashSet::new();
        let mut occupied = HashSet::new();
        let mut unsupported_origins = HashSet::new();
        let mut offsets = BTreeMap::new();
        let mut selected = BTreeMap::new();
        let mut statuses = Vec::new();
        // Cross-family ambiguity must not turn a Store callback location into
        // a class/decorator CALL. Store rows were already validated by core.
        let stores = exact_tuple(table.get_item(4)?, None)?;
        let mut store_ordinals = HashSet::new();
        for row in stores.iter() {
            let row = exact_tuple(row, Some(2))?;
            for emission in exact_tuple(row.get_item(1)?, None)?.iter() {
                store_ordinals.insert(unsigned(exact_tuple(emission, Some(6))?.get_item(0)?)?);
            }
        }
        for row in calls.iter() {
            let row = exact_tuple(row, Some(2))?;
            let wire = call_wire_origin(decoder, code, codes, row.get_item(0)?)?;
            if !origins.insert(wire) {
                return Err(invalid(py, "duplicate native original CALL row"));
            }
            let original = original_call(py, wire, code, codes, originals)?;
            let issues = gaps.remove(&wire).unwrap_or_default();
            let emissions = exact_tuple(row.get_item(1)?, None)?;
            let mut decoded = Vec::with_capacity(emissions.len());
            let mut previous = None;
            for emission in emissions.iter() {
                let emission = call_emission(decoder, code, emission)?;
                if previous.is_some_and(|ordinal| ordinal >= emission.ordinal)
                    || !occupied.insert(emission.ordinal)
                    || unsupported.contains(&emission.ordinal)
                    || unsupported_origins.contains(&emission.ordinal)
                    || store_ordinals.contains(&emission.ordinal)
                {
                    return Err(invalid(
                        py,
                        "duplicate/conflicting native CALL physical site",
                    ));
                }
                previous = Some(emission.ordinal);
                if let Some(offset) = emission.offset {
                    offsets.insert(emission.ordinal, offset);
                }
                if let Some((_, inputs)) = &original {
                    validate_call_source_inputs(py, inputs, &emission.input)?;
                }
                decoded.push(emission);
            }
            if decoded.is_empty() {
                if !issues.iter().any(|gap| matches!(gap.reason, 0 | 1 | 7 | 8)) {
                    return Err(invalid(
                        py,
                        "missing native CALL coverage has no explicit receipt",
                    ));
                }
            } else if issues.iter().any(|gap| gap.reason == 0) {
                return Err(invalid(
                    py,
                    "retained native CALL contradicts eliminated source receipt",
                ));
            }
            let divergent = decoded.first().is_some_and(|first| {
                decoded.iter().any(|other| {
                    first.form != other.form
                        || first.count != other.count
                        || first.input != other.input
                })
            });
            if divergent && !issues.iter().any(|gap| gap.reason == 4) {
                return Err(invalid(
                    py,
                    "divergent native CALL inputs lost their original gap",
                ));
            }
            for issue in &issues {
                if matches!(issue.reason, 6 | 9 | 10)
                    && !decoded.iter().any(|emission| {
                        Some(emission.ordinal) == issue.instruction_ordinal
                            && issue.context_unavailable == emission.context_missing
                            && (issue.reason != 6 || emission.offset.is_none())
                            && (issue.reason != 9 || emission.context_missing)
                    })
                {
                    return Err(invalid(
                        py,
                        "native CALL gap has no matching exact emission",
                    ));
                }
                if issue.reason == 1 {
                    let ordinal = issue.instruction_ordinal.unwrap();
                    if occupied.contains(&ordinal) || !unsupported_origins.insert(ordinal) {
                        return Err(invalid(
                            py,
                            "unsupported native CALL conflicts with a retained physical site",
                        ));
                    }
                }
            }
            for emission in &decoded {
                if emission.context_missing
                    && !issues.iter().any(|gap| {
                        gap.reason == 9 && gap.instruction_ordinal == Some(emission.ordinal)
                    })
                    || emission.offset.is_none()
                        && !issues.iter().any(|gap| {
                            gap.reason == 6 && gap.instruction_ordinal == Some(emission.ordinal)
                        })
                {
                    return Err(invalid(
                        py,
                        "incomplete native CALL receipt lost its explicit gap",
                    ));
                }
            }
            let Some((origin, _)) = original else {
                continue;
            };
            match &origin.role {
                InterpreterCallRole::ClassConstruction { class_body_ordinal } => {
                    if !classes.insert(*class_body_ordinal) {
                        return Err(invalid(
                            py,
                            "retained class body has ambiguous native construction origins",
                        ));
                    }
                }
                InterpreterCallRole::Decorator { index, .. } => {
                    if !decorators.insert((code.ordinal, origin.source_definition.clone(), *index))
                    {
                        return Err(invalid(
                            py,
                            "original decorator has ambiguous native application origins",
                        ));
                    }
                }
                InterpreterCallRole::GenericScopeInvocation { scope_ordinal } => {
                    if !generic_scopes.insert(*scope_ordinal) {
                        return Err(invalid(
                            py,
                            "retained generic scope has ambiguous native invocation origins",
                        ));
                    }
                }
                InterpreterCallRole::SourceExpression => {}
            }
            for gap in &issues {
                if matches!(gap.reason, 1 | 6 | 10) {
                    unsupported.insert(gap.instruction_ordinal.unwrap());
                }
            }
            statuses.push(InterpreterCallOriginStatus {
                origin: origin.clone(),
                instruction_ordinals: decoded.iter().map(|emission| emission.ordinal).collect(),
                gaps: issues.clone(),
            });
            for emission in decoded {
                selected.insert(
                    emission.ordinal,
                    InterpreterCallReceipt {
                        origin: origin.clone(),
                        instruction_ordinal: emission.ordinal,
                        native_byte_offset: emission.offset,
                        form: emission.form,
                        native_value_argument_count: emission.count,
                        input: emission.input,
                        // Physical selection needs no uniform JIT policy. Preserve
                        // gap4/9 and guarded/lowered alternatives without regranting
                        // any missing input or runtime operand authority.
                        gaps: issues.clone(),
                    },
                );
            }
        }
        if !gaps.is_empty() {
            return Err(invalid(py, "native CALL gap lost its original row"));
        }
        let mut previous_offset = None;
        for offset in offsets.values() {
            if previous_offset.is_some_and(|previous| previous >= *offset) {
                return Err(invalid(
                    py,
                    "native CALL offsets do not follow exact instruction ordinals",
                ));
            }
            previous_offset = Some(*offset);
        }
        codes[index].calls = selected;
        codes[index].call_origins = statuses;
        codes[index].unsupported_call_sites = unsupported;
    }
    for code in codes
        .iter()
        .filter(|code| code.role() == InterpreterCodeRole::TypeParameterScope)
    {
        if !generic_scopes.contains(&code.ordinal) {
            return Err(invalid(
                py,
                "retained generic scope lost its exact native invocation row",
            ));
        }
    }
    for code in codes.iter().filter(|code| code.role().is_definition()) {
        if code.role() == InterpreterCodeRole::ClassNamespace && !classes.contains(&code.ordinal) {
            return Err(invalid(
                py,
                "retained original class has no exact native CLASS call row",
            ));
        }
        let original = originals
            .codes
            .iter()
            .find(|item| item.origin == code.origin)
            .ok_or_else(|| invalid(py, "definition lost its original decorator declaration"))?;
        let mut publisher = code.parent.unwrap();
        if codes[publisher as usize].role() == InterpreterCodeRole::TypeParameterScope {
            publisher = codes[publisher as usize]
                .parent
                .ok_or_else(|| invalid(py, "generic definition wrapper has no publisher"))?;
        }
        for index in 0..original.decorators.len() {
            if !decorators.contains(&(publisher, code.source().clone(), index as u32)) {
                return Err(invalid(
                    py,
                    "retained definition lost an original decorator CALL row",
                ));
            }
        }
    }
    Ok(())
}

/// Temporary scalar view of the already emitted scope rows. It is discarded
/// after filling each provider's native FREE-ordinal result. No lifetime recipe,
/// second owner identity, Python code pin, or runtime namespace enters the map.
struct NativeAnnotationScope {
    header_roles: BTreeMap<u32, u32>,
    exports: BTreeMap<u32, u32>,
    reused_slots: HashSet<u32>,
    captures: BTreeMap<(u32, u32), Vec<NativeAnnotationCapture>>,
}

#[derive(Clone, Copy)]
struct NativeAnnotationOwner {
    kind: u32,
    slot: u32,
    region: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NativeAnnotationCapture {
    child: u32,
    free_ordinal: u32,
    slot: u32,
    region: Option<u32>,
    creation: Option<SourceRange>,
}

fn capture_current_slot(py: Python<'_>, value: Bound<'_, PyAny>) -> PyResult<u32> {
    let row = exact_tuple(value, Some(2))?;
    if unsigned(row.get_item(0)?)? != 0 {
        return Err(invalid(py, "capture requires the actual current-slot tag"));
    }
    unsigned(row.get_item(1)?)
}

fn capture_local<'a>(
    py: Python<'_>,
    parent: &'a InterpreterCode,
    slot: u32,
) -> PyResult<&'a InterpreterLocal> {
    parent
        .layout
        .locals
        .get(slot as usize)
        .ok_or_else(|| invalid(py, "capture/owner slot is outside actual native localsplus"))
}

fn decode_annotation_scope(
    py: Python<'_>,
    decoder: &Decoder<'_>,
    parent: &InterpreterCode,
    codes: &[InterpreterCode],
    row: &Bound<'_, PyTuple>,
) -> PyResult<NativeAnnotationScope> {
    let regions = exact_tuple(row.get_item(3)?, None)?;
    for (index, region) in regions.iter().enumerate() {
        let region = exact_tuple(region, Some(8))?;
        if unsigned(region.get_item(0)?)? as usize != index
            || optional_unsigned(region.get_item(1)?)?.is_some_and(|outer| outer as usize >= index)
        {
            return Err(invalid(
                py,
                "capture region IDs/parents are not the actual dense scope inventory",
            ));
        }
    }
    let owners = exact_tuple(row.get_item(2)?, None)?;
    let mut parsed_owners = Vec::with_capacity(owners.len());
    let mut entries = vec![None; parent.layout.locals.len()];
    let mut reused_slots = HashSet::new();
    for (index, owner) in owners.iter().enumerate() {
        let owner = exact_tuple(owner, Some(5))?;
        let kind = unsigned(owner.get_item(1)?)?;
        let slot = unsigned(owner.get_item(2)?)?;
        let local = capture_local(py, parent, slot)?;
        let region = optional_unsigned(owner.get_item(4)?)?;
        if unsigned(owner.get_item(0)?)? as usize != index
            || unsigned(owner.get_item(3)?)? != u32::from(local.kind)
            || region.is_some_and(|region| region as usize >= regions.len())
        {
            return Err(invalid(
                py,
                "capture owner differs from its actual native slot/region",
            ));
        }
        match (kind, region) {
            (0, None) => {
                if entries[slot as usize].replace(index as u32).is_some() {
                    return Err(invalid(py, "native slot has duplicate Entry owners"));
                }
            }
            (1, Some(_)) if local.kind & (CELL | FREE) != 0 => {
                reused_slots.insert(slot);
            }
            (2, Some(_)) if local.kind & LOCAL != 0 => {
                reused_slots.insert(slot);
            }
            _ => {
                return Err(invalid(
                    py,
                    "capture owner kind has no valid native slot/region domain",
                ));
            }
        }
        parsed_owners.push(NativeAnnotationOwner { kind, slot, region });
    }
    if entries.iter().any(Option::is_none) {
        return Err(invalid(
            py,
            "native capture scope lost an actual slot's Entry owner",
        ));
    }
    // A carrier touched by a real region cannot independently establish
    // class-dictionary annotation authority, regardless of source ordering.
    // No execution-layout or lifetime-restoration proof is required.
    let mut region_owners = HashSet::new();
    for (index, region) in regions.iter().enumerate() {
        let region = exact_tuple(region, Some(8))?;
        for op in exact_tuple(region.get_item(6)?, None)?.iter() {
            let op = exact_tuple(op, Some(3))?;
            let kind = unsigned(op.get_item(0)?)?;
            let slot = unsigned(op.get_item(1)?)?;
            let owner_id = unsigned(op.get_item(2)?)?;
            let owner = parsed_owners
                .get(owner_id as usize)
                .ok_or_else(|| invalid(py, "capture region operation has no native owner"))?;
            if !matches!((kind, owner.kind), (0, 2) | (1, 1))
                || owner.slot != slot
                || owner.region != Some(index as u32)
                || !region_owners.insert(owner_id)
            {
                return Err(invalid(
                    py,
                    "capture region operation differs from its qualified owner",
                ));
            }
            reused_slots.insert(slot);
        }
    }
    if parsed_owners
        .iter()
        .enumerate()
        .any(|(index, owner)| owner.kind != 0 && !region_owners.contains(&(index as u32)))
    {
        return Err(invalid(
            py,
            "regional capture owner lost its actual entry operation",
        ));
    }

    let mut header_roles = BTreeMap::new();
    let mut exports = BTreeMap::new();
    let actions = row.get_item(6)?;
    if parent.role() == InterpreterCodeRole::ClassNamespace {
        let actions = exact_tuple(actions, Some(2))?;
        let mut roles = HashSet::new();
        for header in exact_tuple(actions.get_item(0)?, None)?.iter() {
            let header = exact_tuple(header, Some(3))?;
            let owner = parsed_owners
                .get(unsigned(header.get_item(0)?)? as usize)
                .ok_or_else(|| invalid(py, "class header refers to an absent native owner"))?;
            let role = unsigned(header.get_item(1)?)?;
            let local = capture_local(py, parent, owner.slot)?;
            if owner.kind != 0
                || owner.region.is_some()
                || local.kind & CELL == 0
                || local.kind & FREE != 0
                || !matches!(role, 3 | 4)
                || !header.get_item(2)?.is_none()
                || !roles.insert(role)
                || header_roles.insert(owner.slot, role).is_some()
            {
                return Err(invalid(
                    py,
                    "class capture header is not a unique native Entry/CELL role",
                ));
            }
        }
        let mut export_slots = HashSet::new();
        for export in exact_tuple(actions.get_item(1)?, None)?.iter() {
            let export = exact_tuple(export, Some(2))?;
            let role = unsigned(export.get_item(0)?)?;
            let slot = capture_current_slot(py, export.get_item(1)?)?;
            let local = capture_local(py, parent, slot)?;
            if role > 1
                || local.kind & CELL == 0
                || local.kind & FREE != 0
                || exports.insert(role, slot).is_some()
                || !export_slots.insert(slot)
            {
                return Err(invalid(
                    py,
                    "class capture export is not a unique actual native CELL",
                ));
            }
        }
    } else if !actions.is_none() {
        return Err(invalid(
            py,
            "non-class capture scope has class header/export actions",
        ));
    }

    let mut captures: BTreeMap<(u32, u32), Vec<NativeAnnotationCapture>> = BTreeMap::new();
    let mut seen = HashSet::new();
    for capture in exact_tuple(row.get_item(4)?, None)?.iter() {
        let capture = exact_tuple(capture, Some(5))?;
        let child = unsigned(capture.get_item(0)?)?;
        let child_code = codes
            .get(child as usize)
            .filter(|child| child.parent == Some(parent.ordinal))
            .ok_or_else(|| invalid(py, "capture is not the parent's actual direct native child"))?;
        let free_ordinal = unsigned(capture.get_item(2)?)?;
        let (_, _, free_name) = child_code
            .layout
            .free_variables()
            .find(|(ordinal, _, _)| *ordinal == free_ordinal)
            .ok_or_else(|| {
                invalid(
                    py,
                    "capture is outside the child's actual native FREE order",
                )
            })?;
        let slot = capture_current_slot(py, capture.get_item(3)?)?;
        let local = capture_local(py, parent, slot)?;
        // Equality corroborates an already selected physical edge; it never
        // finds a carrier or class role by spelling. Equal-spelling slots stay
        // separate, and a raw FREE is not inferred to be a lexical CELL.
        if local.kind & (CELL | FREE) == 0 || local.name != free_name {
            return Err(invalid(
                py,
                "capture current slot disagrees with the actual child FREE entry",
            ));
        }
        let region = optional_unsigned(capture.get_item(4)?)?;
        if region.is_some_and(|region| region as usize >= regions.len()) {
            return Err(invalid(py, "capture refers to a foreign native region"));
        }
        let capture = NativeAnnotationCapture {
            child,
            free_ordinal,
            slot,
            region,
            creation: decoder.range(capture.get_item(1)?)?,
        };
        if !seen.insert(capture.clone()) {
            return Err(invalid(
                py,
                "duplicate native capture row was not canonicalized",
            ));
        }
        // A provider can capture a FREE forwarded through ordinary source
        // functions/classes. Retain those exact intermediate child edges too;
        // this temporary graph never becomes a runtime owner inventory.
        captures
            .entry((child, free_ordinal))
            .or_default()
            .push(capture);
    }
    Ok(NativeAnnotationScope {
        header_roles,
        exports,
        reused_slots,
        captures,
    })
}

fn annotation_creation_available(
    py: Python<'_>,
    decoder: &Decoder<'_>,
    parent: &InterpreterCode,
    child: &InterpreterCode,
    first_lines: &[u32],
    creation: Option<SourceRange>,
) -> PyResult<bool> {
    let Some(creation) = creation else {
        return Ok(false);
    };
    if child.source().definition_kind == DefinitionKind::Class {
        // codegen_body emits the class's own annotation closure at its actual
        // first-line zero-column body-completion marker, not at the annotation
        // expression or an invented source helper. Decorators can move that
        // first line before the ClassDef header. A method provider is different.
        let first = first_lines[parent.ordinal as usize];
        let offset = decoder.offset(py, first, 0)?;
        if parent.role() != InterpreterCodeRole::ClassNamespace
            || child.source() != parent.source()
            || first_lines[child.ordinal as usize] != first
            || creation != SourceRange::new(offset, offset)
        {
            return Err(invalid(
                py,
                "class annotation capture lost its exact native body-completion marker",
            ));
        }
    } else if child.source().definition_kind != DefinitionKind::Function
        || child.native_range != Some(creation)
        || creation.start == creation.end
    {
        return Err(invalid(
            py,
            "function annotation capture lost its original definition creation site",
        ));
    }
    Ok(true)
}

fn native_capture_creation_available(
    py: Python<'_>,
    decoder: &Decoder<'_>,
    parent: &InterpreterCode,
    child: &InterpreterCode,
    first_lines: &[u32],
    creation: Option<SourceRange>,
) -> PyResult<bool> {
    if child.role() == InterpreterCodeRole::AnnotationProvider {
        return annotation_creation_available(py, decoder, parent, child, first_lines, creation);
    }
    // These are the actual codegen_function_body/codegen_class_body closure
    // sites. Generic/lambda/comprehension creation is not inferred from a source
    // envelope or a matching name.
    if !matches!(
        child.role(),
        InterpreterCodeRole::SourceFunction
            | InterpreterCodeRole::AsyncSourceFunction
            | InterpreterCodeRole::ClassNamespace
    ) {
        return Ok(false);
    }
    let Some(creation) = creation else {
        return Ok(false);
    };
    if child.native_range != Some(creation) || creation.start == creation.end {
        return Err(invalid(
            py,
            "forwarded capture lost its original definition creation site",
        ));
    }
    Ok(true)
}

fn resolve_annotation_capture(
    py: Python<'_>,
    decoder: &Decoder<'_>,
    codes: &[InterpreterCode],
    first_lines: &[u32],
    scopes: &[Option<NativeAnnotationScope>],
    provider: &InterpreterCode,
    free: u32,
) -> PyResult<InterpreterAnnotationCaptureOrigin> {
    use InterpreterAnnotationCaptureOrigin as Origin;
    use InterpreterAnnotationCaptureUnresolved as Unresolved;

    let mut child = provider;
    let mut free = free;
    let mut forwarded = false;
    loop {
        let Some(parent) = child.parent.map(|ordinal| &codes[ordinal as usize]) else {
            return Ok(Origin::Unresolved(Unresolved::UnrepresentedParent));
        };
        let evidence = scopes[parent.ordinal as usize]
            .as_ref()
            .ok_or_else(|| invalid(py, "annotation capture lost its decoded native ancestor"))?;
        let candidates = evidence.captures.get(&(child.ordinal, free));
        let capture = match candidates.map(Vec::as_slice).unwrap_or_default() {
            [] => return Ok(Origin::Unresolved(Unresolved::MissingCapture)),
            [capture] => capture,
            candidates => {
                for candidate in candidates {
                    native_capture_creation_available(
                        py,
                        decoder,
                        parent,
                        child,
                        first_lines,
                        candidate.creation,
                    )?;
                }
                return Ok(Origin::Unresolved(Unresolved::AmbiguousCapture));
            }
        };
        if !native_capture_creation_available(
            py,
            decoder,
            parent,
            child,
            first_lines,
            capture.creation,
        )? {
            return Ok(Origin::Unresolved(Unresolved::CreationSiteUnavailable));
        }
        if capture.region.is_some() {
            return Ok(Origin::Unresolved(Unresolved::RegionalCapture));
        }
        if evidence.reused_slots.contains(&capture.slot) {
            return Ok(Origin::Unresolved(Unresolved::ReusedCarrier));
        }
        let selected = capture_local(py, parent, capture.slot)?;
        if selected.kind & FREE != 0 {
            if !matches!(
                parent.role(),
                InterpreterCodeRole::SourceFunction
                    | InterpreterCodeRole::AsyncSourceFunction
                    | InterpreterCodeRole::ClassNamespace
            ) {
                return Ok(Origin::Unresolved(Unresolved::ForwardedFree));
            }
            free = selected
                .free_ordinal
                .ok_or_else(|| invalid(py, "forwarded native FREE has no physical FREE ordinal"))?;
            // The actual native tree was authenticated before this decoder;
            // every step follows its strict ancestor, never a name-selected node.
            child = parent;
            forwarded = true;
            continue;
        }
        return Ok(match parent.role() {
            InterpreterCodeRole::SourceFunction | InterpreterCodeRole::AsyncSourceFunction => {
                Origin::Lexical {
                    parent_ordinal: parent.ordinal,
                    parent_slot: capture.slot,
                    binding_scope: parent.source().clone(),
                }
            }
            InterpreterCodeRole::ClassNamespace if !forwarded => {
                match evidence.header_roles.get(&capture.slot) {
                    Some(3) if evidence.exports.get(&1) == Some(&capture.slot) => {
                        Origin::ClassDictionary {
                            class_ordinal: parent.ordinal,
                            class_definition: parent.source().clone(),
                            class_slot: capture.slot,
                        }
                    }
                    Some(3) => Origin::Unresolved(Unresolved::ClassDictionaryNotExported),
                    Some(4) => Origin::ConditionalAnnotations {
                        class_ordinal: parent.ordinal,
                        class_definition: parent.source().clone(),
                        class_slot: capture.slot,
                    },
                    _ => Origin::Unresolved(Unresolved::UnprovenClassCell),
                }
            }
            // This bounded slice forwards lexical function cells only. It does
            // not give a nested provider its ancestor's special namespace role.
            _ => Origin::Unresolved(Unresolved::UnrepresentedParent),
        });
    }
}

fn decode_annotation_captures(
    py: Python<'_>,
    decoder: &Decoder<'_>,
    codes: &mut [InterpreterCode],
    first_lines: &[u32],
    recipes: &Bound<'_, PyTuple>,
) -> PyResult<()> {
    let providers: Vec<_> = codes
        .iter()
        .filter(|code| {
            code.role() == InterpreterCodeRole::AnnotationProvider
                && !code.annotation_captures.is_empty()
        })
        .map(|code| code.ordinal)
        .collect();
    let mut needed = HashSet::new();
    for provider in &providers {
        let mut parent = codes[*provider as usize].parent;
        if parent.is_none() {
            return Err(invalid(
                py,
                "annotation provider has no original native parent",
            ));
        }
        while let Some(ordinal) = parent {
            if !needed.insert(ordinal) {
                break; // This entire earlier ancestor chain was already selected.
            }
            parent = codes[ordinal as usize].parent;
        }
    }
    let mut scopes: Vec<_> = (0..codes.len()).map(|_| None).collect();
    for (index, row) in recipes.iter().enumerate() {
        let row = exact_tuple(row, Some(7))?;
        if unsigned(row.get_item(0)?)? as usize != index {
            return Err(invalid(
                py,
                "annotation scope row is not qualified by its actual code ordinal",
            ));
        }
        if needed.contains(&(index as u32)) {
            scopes[index] = Some(decode_annotation_scope(
                py,
                decoder,
                &codes[index],
                codes,
                &row,
            )?);
        }
    }
    let mut resolved: Vec<_> = codes
        .iter()
        .map(|code| code.annotation_captures.clone())
        .collect();
    for ordinal in providers {
        let provider = &codes[ordinal as usize];
        for (free, _, _) in provider.layout.free_variables() {
            resolved[ordinal as usize][free as usize] = resolve_annotation_capture(
                py,
                decoder,
                codes,
                first_lines,
                &scopes,
                provider,
                free,
            )?;
        }
    }
    for (code, captures) in codes.iter_mut().zip(resolved) {
        code.annotation_captures = captures;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use pyo3::types::{PyBool, PyList, PyModule};
    use soac_contracts::{
        AnnotationOrigin, ArtifactEnvironment, ArtifactExpectations, ArtifactSigningKey,
        CallableSignature, ConservativeAnalysis, Fingerprint, FunctionTypeFact,
        ModuleArtifactIndex, PythonVersion, ResolvedStrictPolicy, SourceDialect, StaticType,
        TypeArtifactManifest, encode_module_shard, sign_manifest, verify_manifest,
    };

    use super::*;

    // Metadata/native-kernel fixture only. The real signature/shard verifier
    // and original file bytes are used, but these test facts are NOT a real
    // checker/startup witness or a function-owner/adoption substitute.
    struct Fixture {
        directory: PathBuf,
        verified: Arc<VerifiedStrictModule>,
    }

    impl Fixture {
        fn new(py: Python<'_>, source: &str) -> Self {
            Self::from_facts(py, source, blank_facts(source))
        }

        fn from_facts(py: Python<'_>, source: &str, facts: ModuleTypeFacts) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let directory = std::env::temp_dir().join(format!(
                "soac-interpreter-source-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("interpreter_source_fixture.py");
            std::fs::write(&path, source).unwrap();
            let path = path.canonicalize().unwrap();
            let stamp =
                Fingerprint::digest(b"interpreter-source native metadata fixture; not startup");
            let environment = ArtifactEnvironment {
                ty_revision: "native-metadata-fixture".into(),
                checker_source_fingerprint: stamp,
                exporter_revision: "native-metadata-fixture".into(),
                python_version: PythonVersion {
                    major: 3,
                    minor: 15,
                },
                python_platform: "linux".into(),
                cpython_abi_fingerprint: stamp,
                normalized_project_policy: stamp,
                resolved_typechecker_configuration: stamp,
                import_search_path: stamp,
                typeshed_fingerprint: stamp,
                installed_stub_fingerprint: stamp,
                installed_dependency_fingerprint: stamp,
                analysis: ConservativeAnalysis::default(),
            };
            let shard = encode_module_shard(&facts).unwrap();
            let manifest = TypeArtifactManifest::new(
                environment.clone(),
                vec![ModuleArtifactIndex::from_shard(&shard).unwrap()],
            )
            .unwrap();
            let signing = ArtifactSigningKey::from_bytes(&[91; 32]);
            let manifest = verify_manifest(
                &sign_manifest(&manifest, &signing).unwrap(),
                &signing.trust_anchor(),
                &ArtifactExpectations {
                    generation: manifest.generation,
                    environment,
                },
            )
            .unwrap();
            let facts = manifest
                .verify_module(
                    &facts.module.module_name,
                    source.as_bytes(),
                    &facts.language_policy,
                    &[],
                    shard.bytes(),
                )
                .unwrap();
            let verified = Arc::new(
                VerifiedStrictModule::from_verified_test_facts(
                    py,
                    path,
                    Arc::from(source.as_bytes()),
                    facts,
                )
                .unwrap(),
            );
            Self {
                directory,
                verified,
            }
        }

        fn compile<'py>(&self, py: Python<'py>) -> (Bound<'py, PyAny>, Bound<'py, PyAny>) {
            unsafe extern "C" {
                fn PySoac_CompileVerifiedSourceDetails(
                    source: *const std::ffi::c_char,
                    length: ffi::Py_ssize_t,
                    filename: *mut ffi::PyObject,
                    optimize: std::ffi::c_int,
                ) -> *mut ffi::PyObject;
            }
            let filename = PyString::new(py, self.verified.source_path().to_str().unwrap());
            let details = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    PySoac_CompileVerifiedSourceDetails(
                        self.verified.source().as_ptr().cast(),
                        self.verified.source().len().try_into().unwrap(),
                        filename.as_ptr(),
                        -1,
                    ),
                )
            }
            .unwrap();
            let details = exact_tuple(details, Some(3)).unwrap();
            (details.get_item(0).unwrap(), details.get_item(2).unwrap())
        }

        fn decode<'py>(
            &self,
            py: Python<'py>,
            root: &Bound<'py, PyAny>,
            bindings: &Bound<'py, PyAny>,
        ) -> PyResult<StrictInterpreterSource> {
            StrictInterpreterSource::from_native_details(py, self.verified.clone(), root, bindings)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn native_lock() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        guard
    }

    fn blank_facts(source: &str) -> ModuleTypeFacts {
        ModuleTypeFacts::new(
            "interpreter_source_fixture",
            source.as_bytes(),
            SourceDialect::SoacStrict,
            ResolvedStrictPolicy {
                strict_assign: true,
                checked_attr: true,
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn direct_function_fact(source: &str, name: &str) -> FunctionTypeFact {
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        let function = parsed
            .syntax()
            .body
            .iter()
            .find_map(|stmt| match stmt {
                Stmt::FunctionDef(function) if function.name.as_str() == name => Some(function),
                _ => None,
            })
            .unwrap();
        FunctionTypeFact {
            identity: SourceIdentity {
                module: blank_facts(source).module,
                lexical_qualname: name.into(),
                source_range: source_range(function.range),
                definition_kind: DefinitionKind::Function,
            },
            function_kind: if function.is_async {
                FunctionKind::Coroutine
            } else {
                FunctionKind::Synchronous
            },
            signature: CallableSignature {
                parameters: Vec::new(),
                return_type: StaticType::Unknown,
                return_annotation_origin: AnnotationOrigin::Absent,
                uncertainty: BTreeSet::new(),
            },
            decorators: Vec::new(),
            uncertainty: BTreeSet::new(),
        }
    }

    fn one<'a>(
        source: &'a StrictInterpreterSource,
        role: InterpreterCodeRole,
        name: &str,
    ) -> &'a InterpreterCode {
        let mut matching = source
            .codes
            .iter()
            .filter(|code| code.role() == role && code.source().lexical_qualname == name);
        let code = matching.next().unwrap();
        assert!(
            matching.next().is_none(),
            "fixture expected one original native role"
        );
        code
    }

    fn tuple(value: Bound<'_, PyAny>) -> Bound<'_, PyTuple> {
        exact_tuple(value, None).unwrap()
    }

    fn number(py: Python<'_>, value: u32) -> Bound<'_, PyAny> {
        value.into_pyobject(py).unwrap().into_any()
    }

    fn replace<'py>(
        row: &Bound<'py, PyTuple>,
        index: usize,
        value: Bound<'py, PyAny>,
    ) -> Bound<'py, PyTuple> {
        PyTuple::new(
            row.py(),
            row.iter()
                .enumerate()
                .map(|(i, old)| if i == index { value.clone() } else { old }),
        )
        .unwrap()
    }

    fn definition_row(table: &Bound<'_, PyTuple>) -> usize {
        tuple(table.get_item(4).unwrap())
            .iter()
            .position(|row| {
                let origin = tuple(tuple(row).get_item(0).unwrap());
                unsigned(origin.get_item(0).unwrap()).unwrap() == 1
            })
            .unwrap()
    }

    fn change_module_store<'py>(
        packet: &Bound<'py, PyTuple>,
        row_index: usize,
        store: Bound<'py, PyAny>,
    ) -> Bound<'py, PyTuple> {
        let tables = tuple(packet.get_item(3).unwrap());
        let table = tuple(tables.get_item(0).unwrap());
        let stores = tuple(table.get_item(4).unwrap());
        let stores = replace(&stores, row_index, store);
        let table = replace(&table, 4, stores.into_any());
        replace(packet, 3, replace(&tables, 0, table.into_any()).into_any())
    }

    #[test]
    fn interpreter_source_module_and_definition_sites_are_exact_not_alias_names() {
        let _guard = native_lock();
        Python::attach(|py| {
            let source = "from __future__ import strict\nvalue = 1\nalias = None\ndef plain():\n    return value\nalias = plain\n";
            let mut facts = blank_facts(source);
            let fact = direct_function_fact(source, "plain");
            facts.functions.push(fact.clone());
            let fixture = Fixture::from_facts(py, source, facts);
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            assert!(Arc::ptr_eq(map.verified(), &fixture.verified));
            let module = map.code(py, &root).unwrap();
            assert_eq!(module.role(), InterpreterCodeRole::Module);
            assert_eq!(
                module.source(),
                &fixture.verified.type_facts().facts().module_body_identity()
            );
            let plain = one(&map, InterpreterCodeRole::SourceFunction, "plain");
            assert_eq!(plain.source(), &fact.identity);
            let packet = tuple(bindings);
            let table = tuple(tuple(packet.get_item(3).unwrap()).get_item(0).unwrap());
            let mut definitions = 0;
            let mut ordinary = 0;
            for row in tuple(table.get_item(4).unwrap()).iter() {
                let row = tuple(row);
                let kind = unsigned(tuple(row.get_item(0).unwrap()).get_item(0).unwrap()).unwrap();
                for emission in tuple(row.get_item(1).unwrap()).iter() {
                    let emission = tuple(emission);
                    let ordinal = unsigned(emission.get_item(0).unwrap()).unwrap();
                    let lane = unsigned(emission.get_item(4).unwrap()).unwrap() as u8;
                    let found = map.definition_store(py, &root, ordinal, lane).unwrap();
                    if kind == 1 {
                        let found = found.unwrap();
                        assert_eq!(found.source, fact.identity);
                        assert_eq!(found.body_code_ordinal, plain.ordinal());
                        assert_eq!(found.role, InterpreterCodeRole::SourceFunction);
                        assert!(matches!(found.target, InterpreterStoreTarget::Name(_)));
                        definitions += 1;
                    } else {
                        assert!(
                            found.is_none(),
                            "ordinary aliases/imports are not definitions"
                        );
                        ordinary += 1;
                    }
                }
            }
            assert_eq!(definitions, 1);
            assert!(ordinary >= 3);
        });
    }

    #[test]
    fn interpreter_source_decorated_nested_and_async_origins_use_original_ast() {
        let _guard = native_lock();
        Python::attach(|py| {
            let source = "from __future__ import strict\ndef decorate(value):\n    return value\n@decorate\nclass Subject:\n    @decorate\n    def method(self, value: int) -> int:\n        return value\ndef factory(value):\n    class Product:\n        def read(self):\n            return value\n    return Product\nasync def later(value):\n    return value\n";
            let fixture = Fixture::new(py, source);
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let class = one(&map, InterpreterCodeRole::ClassNamespace, "Subject");
            assert!(class.source().source_range.start < class.native_range().unwrap().start);
            let method = one(&map, InterpreterCodeRole::SourceFunction, "Subject.method");
            assert!(method.source().source_range.start < method.native_range().unwrap().start);
            assert_eq!(method.parent_ordinal(), Some(class.ordinal()));
            let provider = one(
                &map,
                InterpreterCodeRole::AnnotationProvider,
                "Subject.method",
            );
            assert_eq!(provider.source(), method.source());
            assert_eq!(provider.parent_ordinal(), method.parent_ordinal());
            let factory = one(&map, InterpreterCodeRole::SourceFunction, "factory");
            let product = one(
                &map,
                InterpreterCodeRole::ClassNamespace,
                "factory.<locals>.Product",
            );
            assert_eq!(product.parent_ordinal(), Some(factory.ordinal()));
            let read = one(
                &map,
                InterpreterCodeRole::SourceFunction,
                "factory.<locals>.Product.read",
            );
            assert_eq!(read.parent_ordinal(), Some(product.ordinal()));
            assert!(
                read.layout()
                    .free_variables()
                    .any(|(_, _, name)| name == "value")
            );
            let later = one(&map, InterpreterCodeRole::AsyncSourceFunction, "later");
            assert_ne!(later.layout().flags & CO_COROUTINE, 0);
            // Same original code is source data, not a per-call factory owner.
            let tree = native_tree(py, &root).unwrap();
            assert_eq!(
                map.code(py, &tree[read.ordinal() as usize].0)
                    .unwrap()
                    .source(),
                read.source()
            );
        });
    }

    #[test]
    fn interpreter_source_native_parameter_order_keeps_cells_unused_args_and_free_slots() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef outer(captured):\n    def f(first, /, named, unused, *rest, kwonly, **keywords):\n        return lambda: (first, named, kwonly, captured)\n    return f\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let f = one(
                &map,
                InterpreterCodeRole::SourceFunction,
                "outer.<locals>.f",
            );
            let parameters = &f.layout().parameters;
            let names: Vec<_> = parameters.iter().map(|p| p.native_name.as_str()).collect();
            assert_eq!(
                names,
                ["first", "named", "unused", "kwonly", "rest", "keywords"]
            );
            assert_eq!(
                parameters
                    .iter()
                    .map(|p| p.source_index.unwrap())
                    .collect::<Vec<_>>(),
                [0, 1, 2, 4, 3, 5]
            );
            assert_eq!(
                parameters.iter().map(|p| p.kind).collect::<Vec<_>>(),
                [
                    ParameterKind::PositionalOnly,
                    ParameterKind::PositionalOrKeyword,
                    ParameterKind::PositionalOrKeyword,
                    ParameterKind::KeywordOnly,
                    ParameterKind::VarArgs,
                    ParameterKind::VarKeywords,
                ]
            );
            for index in [0, 1, 3] {
                let parameter = &parameters[index];
                assert_ne!(
                    f.layout().locals[parameter.native_index as usize].kind & CELL,
                    0
                );
            }
            let captures = f.layout().free_variables().collect::<Vec<_>>();
            assert_eq!(captures.len(), 1);
            assert_eq!(captures[0].0, 0);
            assert_eq!(captures[0].2, "captured");
            assert_ne!(f.layout().locals[captures[0].1 as usize].kind & FREE, 0);
        });
    }

    #[test]
    fn interpreter_source_native_signature_rejects_mismatched_actual_header_and_argument_bits() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef f(first, /, named, *rest, kwonly, **keywords):\n    return lambda: (first, kwonly)\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let f = one(&map, InterpreterCodeRole::SourceFunction, "f");
            let tree = native_tree(py, &root).unwrap();
            let code = &tree[f.ordinal() as usize].0;
            // Fault-injected scalar views remain private validation inputs,
            // never code objects, owner grants or an alternative code catalogue.
            let mut wrong_count = unsafe { view(py, code.as_ptr()).unwrap() };
            wrong_count.posonlyargcount = wrong_count.argcount + 1;
            assert!(InterpreterNativeLayout::read(py, &wrong_count).is_err());
            let mut wrong_variadic = unsafe { view(py, code.as_ptr()).unwrap() };
            wrong_variadic.flags ^= CO_VARARGS;
            assert!(InterpreterNativeLayout::read(py, &wrong_variadic).is_err());
            let mut wrong_bits = unsafe { view(py, code.as_ptr()).unwrap() };
            let actual =
                unsafe { Bound::<PyAny>::from_borrowed_ptr(py, wrong_bits.localspluskinds) };
            let mut kinds = actual.cast::<PyBytes>().unwrap().as_bytes().to_vec();
            kinds[0] = (kinds[0] & !ARG_MASK) | ARG_KW;
            let kinds = PyBytes::new(py, &kinds);
            wrong_bits.localspluskinds = kinds.as_ptr();
            assert!(InterpreterNativeLayout::read(py, &wrong_bits).is_err());
            let actual = unsafe { view(py, code.as_ptr()).unwrap() };
            assert!(InterpreterNativeLayout::read(py, &actual).is_ok());
        });
    }

    #[test]
    fn interpreter_source_equal_spelling_local_and_free_are_not_merged() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef outer(shared):\n    class C:\n        value = shared\n        values = [shared for shared in (1, 2)]\n    return C\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let class = one(
                &map,
                InterpreterCodeRole::ClassNamespace,
                "outer.<locals>.C",
            );
            let slots = class
                .layout()
                .locals
                .iter()
                .filter(|slot| slot.name == "shared")
                .collect::<Vec<_>>();
            assert_eq!(slots.len(), 2);
            assert_ne!(slots[0].index, slots[1].index);
            assert!(
                slots
                    .iter()
                    .any(|slot| slot.kind & LOCAL != 0 && slot.free_ordinal.is_none())
            );
            assert!(
                slots
                    .iter()
                    .any(|slot| slot.kind & FREE != 0 && slot.free_ordinal.is_some())
            );
        });
    }

    #[test]
    fn interpreter_source_global_and_nonlocal_definition_stores_keep_lexical_identity() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef publish():\n    global Escaped\n    class Escaped:\n        pass\n    return Escaped\ndef outer():\n    target: object = None\n    def create():\n        nonlocal target\n        class target:\n            pass\n        return target\n    return create\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let publish = one(&map, InterpreterCodeRole::SourceFunction, "publish");
            let escaped = one(
                &map,
                InterpreterCodeRole::ClassNamespace,
                "publish.<locals>.Escaped",
            );
            let global = publish
                .definition_stores
                .values()
                .find(|site| site.source == *escaped.source())
                .unwrap();
            assert!(matches!(global.target, InterpreterStoreTarget::Global(_)));
            assert_eq!(global.body_code_ordinal, escaped.ordinal());
            let create = one(
                &map,
                InterpreterCodeRole::SourceFunction,
                "outer.<locals>.create",
            );
            let target = one(
                &map,
                InterpreterCodeRole::ClassNamespace,
                "outer.<locals>.create.<locals>.target",
            );
            let nonlocal = create
                .definition_stores
                .values()
                .find(|site| site.source == *target.source())
                .unwrap();
            let InterpreterStoreTarget::Cell(index) = nonlocal.target else {
                panic!("requires actual native STORE_DEREF, not a fabricated CELL owner");
            };
            assert_ne!(create.layout().locals[index as usize].kind & FREE, 0);
            assert_eq!(nonlocal.body_code_ordinal, target.ordinal());
        });
    }

    #[test]
    fn interpreter_source_annotation_providers_preserve_real_owner_and_later_annotations() {
        let _guard = native_lock();
        Python::attach(|py| {
            let source = "from __future__ import strict\nfirst: int\nsecond: str\nclass Owner:\n    first: int\n    second: str\n    def method(self, value: int) -> str:\n        return str(value)\n";
            let fixture = Fixture::new(py, source);
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let module_provider = map
                .codes
                .iter()
                .find(|code| {
                    code.role() == InterpreterCodeRole::AnnotationProvider
                        && code.source().definition_kind == DefinitionKind::Module
                })
                .unwrap();
            assert_eq!(module_provider.parent_ordinal(), Some(0));
            let class = one(&map, InterpreterCodeRole::ClassNamespace, "Owner");
            let provider = one(&map, InterpreterCodeRole::AnnotationProvider, "Owner");
            assert_eq!(provider.source(), class.source());
            assert_eq!(provider.parent_ordinal(), Some(class.ordinal()));
            let parsed = ruff_python_parser::parse_module(source).unwrap();
            let class_ast = parsed
                .syntax()
                .body
                .iter()
                .find_map(|stmt| match stmt {
                    Stmt::ClassDef(class) => Some(class),
                    _ => None,
                })
                .unwrap();
            let later = class_ast
                .body
                .iter()
                .filter_map(|stmt| match stmt {
                    Stmt::AnnAssign(assignment) => {
                        Some(source_range(assignment.annotation.range()))
                    }
                    _ => None,
                })
                .nth(1)
                .unwrap();
            assert!(!provider.native_range().unwrap().contains(later));
            assert!(provider.source().source_range.contains(later));
            assert!(
                provider
                    .layout()
                    .free_variables()
                    .any(|(_, _, name)| name == "__classdict__")
            );
            let method = one(&map, InterpreterCodeRole::SourceFunction, "Owner.method");
            let annotations = one(
                &map,
                InterpreterCodeRole::AnnotationProvider,
                "Owner.method",
            );
            assert_eq!(annotations.source(), method.source());
            assert_eq!(annotations.layout().positional_only_count, 1);
        });
    }

    #[test]
    fn interpreter_source_type_parameter_alias_and_bound_default_roles_remain_distinct() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ntype Alias[T: int = int] = list[T]\ndef generic[T: int = int](value: T) -> T:\n    return value\nclass Container[T: int = int]:\n    def get(self, value: T) -> T:\n        return value\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            for (name, body_role) in [
                ("Alias", InterpreterCodeRole::TypeAlias),
                ("generic", InterpreterCodeRole::SourceFunction),
                ("Container", InterpreterCodeRole::ClassNamespace),
            ] {
                let wrapper = one(&map, InterpreterCodeRole::TypeParameterScope, name);
                let body = one(&map, body_role, name);
                assert_eq!(body.parent_ordinal(), Some(wrapper.ordinal()));
                assert_eq!(body.source(), wrapper.source());
                let evaluators = map
                    .codes
                    .iter()
                    .filter(|code| {
                        code.role() == InterpreterCodeRole::TypeVariable
                            && code.parent_ordinal() == Some(wrapper.ordinal())
                    })
                    .collect::<Vec<_>>();
                assert_eq!(evaluators.len(), 2);
                assert_ne!(
                    evaluators[0].expression_range(),
                    evaluators[1].expression_range()
                );
                for evaluator in evaluators {
                    assert_eq!(
                        evaluator.source().definition_kind,
                        DefinitionKind::Parameter
                    );
                    assert_eq!(evaluator.source().lexical_qualname, format!("{name}.T"));
                    assert!(
                        evaluator
                            .source()
                            .source_range
                            .contains(evaluator.expression_range().unwrap())
                    );
                }
            }
            let provider = one(&map, InterpreterCodeRole::AnnotationProvider, "generic");
            let wrapper = one(&map, InterpreterCodeRole::TypeParameterScope, "generic");
            assert_eq!(provider.parent_ordinal(), Some(wrapper.ordinal()));
            assert_eq!(provider.source(), wrapper.source());
        });
    }

    #[test]
    fn interpreter_source_finally_definition_copies_keep_every_actual_physical_site() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef outer(flag):\n    try:\n        if flag:\n            return 1\n        return 2\n    finally:\n        def value():\n            return 3\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let outer = one(&map, InterpreterCodeRole::SourceFunction, "outer");
            let value = one(
                &map,
                InterpreterCodeRole::SourceFunction,
                "outer.<locals>.value",
            );
            let tree = native_tree(py, &root).unwrap();
            let sites = outer
                .definition_stores
                .values()
                .filter(|site| site.source == *value.source())
                .collect::<Vec<_>>();
            assert!(
                sites.len() >= 2,
                "requires real native finally copies, not invented clone order"
            );
            let mut keys = HashSet::new();
            for site in sites {
                assert!(keys.insert((site.instruction_ordinal, site.lane)));
                let found = map
                    .definition_store(
                        py,
                        &tree[outer.ordinal() as usize].0,
                        site.instruction_ordinal,
                        site.lane,
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(found, site);
                assert_eq!(found.body_code_ordinal, value.ordinal());
            }
        });
    }

    #[test]
    fn interpreter_source_replacement_finally_context_keeps_exact_definition_receipt() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef outer():\n    try:\n        try:\n            return 1\n        finally:\n            return 2\n    finally:\n        def value():\n            return 3\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let outer = one(&map, InterpreterCodeRole::SourceFunction, "outer");
            let value = one(
                &map,
                InterpreterCodeRole::SourceFunction,
                "outer.<locals>.value",
            );
            let tree = native_tree(py, &root).unwrap();
            let sites = outer
                .definition_stores
                .values()
                .filter(|site| site.source == *value.source())
                .collect::<Vec<_>>();
            assert!(
                sites
                    .iter()
                    .any(|site| site.gaps.iter().any(|gap| gap.reason == 9)),
                "requires actual native unavailable-context evidence"
            );
            for site in sites {
                let actual = map
                    .definition_store(
                        py,
                        &tree[outer.ordinal() as usize].0,
                        site.instruction_ordinal,
                        site.lane,
                    )
                    .unwrap()
                    .unwrap();
                assert_eq!(actual, site);
                assert_eq!(actual.body_code_ordinal, value.ordinal());
            }
        });
    }

    #[test]
    fn interpreter_source_rejects_wrong_tree_schema_and_callback_capable_wire_values() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef plain():\n    return 1\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let nodes = tuple(packet.get_item(1).unwrap());
            let child = tuple(nodes.get_item(1).unwrap());
            let replacement = root.call_method0("replace").unwrap();
            let malformed = [
                replace(&packet, 0, number(py, 4)).into_any(),
                replace(&packet, 0, number(py, 5)).into_any(),
                replace(&packet, 3, py.None().into_bound(py)).into_any(),
                replace(&packet, 0, PyBool::new(py, true).to_owned().into_any()).into_any(),
                PyTuple::new(py, packet.iter().take(3)).unwrap().into_any(),
                replace(
                    &packet,
                    1,
                    replace(&nodes, 1, replace(&child, 0, number(py, 99)).into_any()).into_any(),
                )
                .into_any(),
                replace(
                    &packet,
                    1,
                    replace(&nodes, 1, replace(&child, 1, number(py, 1)).into_any()).into_any(),
                )
                .into_any(),
                replace(
                    &packet,
                    1,
                    replace(
                        &nodes,
                        1,
                        replace(&child, 2, replacement.clone()).into_any(),
                    )
                    .into_any(),
                )
                .into_any(),
                replace(
                    &packet,
                    1,
                    replace(&nodes, 1, replace(&child, 3, number(py, 99)).into_any()).into_any(),
                )
                .into_any(),
                replace(
                    &packet,
                    1,
                    replace(
                        &nodes,
                        1,
                        replace(&child, 5, py.None().into_bound(py)).into_any(),
                    )
                    .into_any(),
                )
                .into_any(),
            ];
            for malformed in malformed {
                assert!(fixture.decode(py, &root, &malformed).is_err());
            }
            let probes = PyModule::from_code(py,
                c"calls = []\nclass TupleProbe(tuple):\n def __iter__(self): calls.append('iter'); raise AssertionError\n def __len__(self): calls.append('len'); raise AssertionError\nclass IntProbe(int):\n def __int__(self): calls.append('int'); raise AssertionError\nclass TextProbe(str):\n def __eq__(self, other): calls.append('eq'); raise AssertionError\n def __str__(self): calls.append('str'); raise AssertionError\n",
                c"<interpreter source exact-type probes>", c"interpreter_source_probes").unwrap();
            let tuple_probe = probes
                .getattr("TupleProbe")
                .unwrap()
                .call1((&packet,))
                .unwrap();
            assert!(fixture.decode(py, &root, &tuple_probe).is_err());
            let integer = probes
                .getattr("IntProbe")
                .unwrap()
                .call1((WIRE_VERSION,))
                .unwrap();
            assert!(
                fixture
                    .decode(py, &root, &replace(&packet, 0, integer).into_any())
                    .is_err()
            );
            let tables = tuple(packet.get_item(3).unwrap());
            let table = tuple(tables.get_item(0).unwrap());
            let names = tuple(table.get_item(3).unwrap());
            let text = probes
                .getattr("TextProbe")
                .unwrap()
                .call1((names.get_item(0).unwrap(),))
                .unwrap();
            let table = replace(&table, 3, replace(&names, 0, text).into_any());
            let packet = replace(&packet, 3, replace(&tables, 0, table.into_any()).into_any());
            assert!(fixture.decode(py, &root, &packet.into_any()).is_err());
            assert_eq!(
                probes
                    .getattr("calls")
                    .unwrap()
                    .cast::<PyList>()
                    .unwrap()
                    .len(),
                0
            );
        });
    }

    #[test]
    fn interpreter_source_rejects_missing_conflicting_and_out_of_domain_definition_receipts() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef plain():\n    return 1\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let tables = tuple(packet.get_item(3).unwrap());
            let table = tuple(tables.get_item(0).unwrap());
            let stores = tuple(table.get_item(4).unwrap());
            let index = definition_row(&table);
            let store = tuple(stores.get_item(index).unwrap());
            let origin = tuple(store.get_item(0).unwrap());
            let emissions = tuple(store.get_item(1).unwrap());
            let emission = tuple(emissions.get_item(0).unwrap());
            let operand = tuple(emission.get_item(2).unwrap());
            let bad_emissions = [
                replace(&emission, 0, table.get_item(1).unwrap()),
                replace(&emission, 4, number(py, 2)),
                replace(
                    &emission,
                    2,
                    replace(&operand, 1, number(py, u32::MAX)).into_any(),
                ),
            ];
            for bad in bad_emissions {
                let bad_store =
                    replace(&store, 1, replace(&emissions, 0, bad.into_any()).into_any());
                let packet = change_module_store(&packet, index, bad_store.into_any());
                assert!(fixture.decode(py, &root, &packet.into_any()).is_err());
            }
            let twice = PyTuple::new(
                py,
                emissions
                    .iter()
                    .chain(std::iter::once(emission.into_any()))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let duplicated = change_module_store(
                &packet,
                index,
                replace(&store, 1, twice.into_any()).into_any(),
            );
            assert!(fixture.decode(py, &root, &duplicated.into_any()).is_err());
            for changed_origin in [
                replace(&origin, 2, number(py, 1)),
                replace(&origin, 0, number(py, 0)),
            ] {
                let changed = change_module_store(
                    &packet,
                    index,
                    replace(&store, 0, changed_origin.into_any()).into_any(),
                );
                assert!(fixture.decode(py, &root, &changed.into_any()).is_err());
            }
            let missing = PyTuple::new(
                py,
                stores
                    .iter()
                    .enumerate()
                    .filter_map(|(i, row)| (i != index).then_some(row))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let table = replace(&table, 4, missing.into_any());
            let missing = replace(&packet, 3, replace(&tables, 0, table.into_any()).into_any());
            assert!(fixture.decode(py, &root, &missing.into_any()).is_err());
        });
    }

    #[test]
    fn interpreter_source_preserves_gap4_without_requiring_uniform_jit_store_policy() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef plain():\n    return 1\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let tables = tuple(packet.get_item(3).unwrap());
            let table = tuple(tables.get_item(0).unwrap());
            let stores = tuple(table.get_item(4).unwrap());
            let store = tuple(stores.get_item(definition_row(&table)).unwrap());
            let gap_origin = PyTuple::new(py, [number(py, 1), store.get_item(0).unwrap()]).unwrap();
            // Value-only preservation control, NOT an actual native divergence
            // witness or authority to execute modified metadata.
            let gap = PyTuple::new(
                py,
                [
                    number(py, 4),
                    gap_origin.into_any(),
                    py.None().into_bound(py),
                    py.None().into_bound(py),
                    py.None().into_bound(py),
                    py.None().into_bound(py),
                ],
            )
            .unwrap();
            let gaps = tuple(table.get_item(6).unwrap());
            let gaps = PyTuple::new(
                py,
                gaps.iter()
                    .chain(std::iter::once(gap.into_any()))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let table = replace(&table, 6, gaps.into_any());
            let changed = replace(&packet, 3, replace(&tables, 0, table.into_any()).into_any());
            let map = fixture.decode(py, &root, &changed.into_any()).unwrap();
            let emission = tuple(tuple(store.get_item(1).unwrap()).get_item(0).unwrap());
            let found = map
                .definition_store(
                    py,
                    &root,
                    unsigned(emission.get_item(0).unwrap()).unwrap(),
                    unsigned(emission.get_item(4).unwrap()).unwrap() as u8,
                )
                .unwrap()
                .unwrap();
            assert_eq!(found.gaps.len(), 1);
            assert_eq!(found.gaps[0].reason, 4);
            assert_eq!(found.source.lexical_qualname, "plain");
        });
    }

    #[test]
    fn interpreter_source_eliminated_definition_does_not_invent_a_body_or_site() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\nif False:\n    def absent():\n        return 1\nvalue = 2\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            assert!(
                !map.codes
                    .iter()
                    .any(|code| code.source().lexical_qualname == "absent")
            );
            let module = map.code(py, &root).unwrap();
            assert!(module.definition_stores.is_empty());
            let tables = tuple(tuple(bindings).get_item(3).unwrap());
            let table = tuple(tables.get_item(0).unwrap());
            assert!(
                tuple(table.get_item(6).unwrap())
                    .iter()
                    .any(|row| { unsigned(tuple(row).get_item(0).unwrap()).unwrap() == 0 }),
                "requires the native elimination receipt"
            );
        });
    }

    #[test]
    fn interpreter_source_rejects_another_compilation_and_has_no_permanent_python_code_pins() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef plain():\n    return 1\n",
            );
            let (root, bindings) = fixture.compile(py);
            let child = native_tree(py, &root).unwrap().get(1).unwrap().0.clone();
            let weakref = py.import("weakref").unwrap().getattr("ref").unwrap();
            let weak_root = weakref.call1((&root,)).unwrap();
            let weak_child = weakref.call1((&child,)).unwrap();
            let before_root = unsafe { ffi::Py_REFCNT(root.as_ptr()) };
            let before_child = unsafe { ffi::Py_REFCNT(child.as_ptr()) };
            let map = fixture.decode(py, &root, &bindings).unwrap();
            assert_eq!(unsafe { ffi::Py_REFCNT(root.as_ptr()) }, before_root);
            assert_eq!(unsafe { ffi::Py_REFCNT(child.as_ptr()) }, before_child);
            assert_eq!(
                map.code(py, &child).unwrap().role(),
                InterpreterCodeRole::SourceFunction
            );
            let (other_root, other_bindings) = fixture.compile(py);
            assert!(map.code(py, &other_root).is_err());
            assert!(fixture.decode(py, &root, &other_bindings).is_err());
            assert!(
                map.code(py, &root.call_method0("replace").unwrap())
                    .is_err()
            );
            drop(bindings);
            drop(child);
            drop(root);
            assert!(weak_root.call0().unwrap().is_none());
            assert!(weak_child.call0().unwrap().is_none());
            assert_ne!(map.source_id(), 0);
            drop(other_bindings);
            drop(other_root);
        });
    }

    #[test]
    fn interpreter_source_signed_wrong_definition_and_invalid_utf8_coordinates_are_rejected() {
        let _guard = native_lock();
        Python::attach(|py| {
            let source = "from __future__ import strict\ndef plain():\n    return 1\n";
            let mut facts = blank_facts(source);
            let mut fact = direct_function_fact(source, "plain");
            fact.identity.source_range.start += 1;
            facts.functions.push(fact);
            let fixture = Fixture::from_facts(py, source, facts);
            let (root, bindings) = fixture.compile(py);
            assert!(fixture.decode(py, &root, &bindings).is_err());
            let decoder = Decoder::new("πx = 1\n");
            assert!(decoder.offset(py, 1, 1).is_err());
            assert_eq!(decoder.offset(py, 1, 2).unwrap(), 2);
            assert!(decoder.offset(py, 1, 9).is_err());
            assert!(decoder.offset(py, 0, 0).is_err());
            assert!(decoder.offset(py, 99, 0).is_err());
        });
    }

    fn call_row_index(table: &Bound<'_, PyTuple>, kind: u32) -> usize {
        tuple(table.get_item(5).unwrap())
            .iter()
            .position(|row| {
                let origin = tuple(tuple(row).get_item(0).unwrap());
                unsigned(origin.get_item(0).unwrap()).unwrap() == kind
            })
            .unwrap()
    }

    fn change_call_row<'py>(
        packet: &Bound<'py, PyTuple>,
        code_index: usize,
        row_index: usize,
        call: Bound<'py, PyAny>,
    ) -> Bound<'py, PyTuple> {
        let tables = tuple(packet.get_item(3).unwrap());
        let table = tuple(tables.get_item(code_index).unwrap());
        let calls = tuple(table.get_item(5).unwrap());
        let table = replace(&table, 5, replace(&calls, row_index, call).into_any());
        replace(
            packet,
            3,
            replace(&tables, code_index, table.into_any()).into_any(),
        )
    }

    fn add_call_gap<'py>(
        packet: &Bound<'py, PyTuple>,
        code_index: usize,
        origin: Bound<'py, PyAny>,
        reason: u32,
        physical: Option<(u32, u32)>,
        context: Bound<'py, PyAny>,
    ) -> Bound<'py, PyTuple> {
        let py = packet.py();
        let source = PyTuple::new(py, [number(py, 2), origin]).unwrap();
        let gap = PyTuple::new(
            py,
            [
                number(py, reason),
                source.into_any(),
                physical.map_or_else(
                    || py.None().into_bound(py),
                    |(ordinal, _)| number(py, ordinal),
                ),
                physical.map_or_else(|| py.None().into_bound(py), |_| number(py, 0)),
                physical.map_or_else(
                    || py.None().into_bound(py),
                    |(_, opcode)| number(py, opcode),
                ),
                context,
            ],
        )
        .unwrap();
        let tables = tuple(packet.get_item(3).unwrap());
        let table = tuple(tables.get_item(code_index).unwrap());
        let gaps = tuple(table.get_item(6).unwrap());
        let gaps = PyTuple::new(
            py,
            gaps.iter()
                .chain(std::iter::once(gap.into_any()))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let table = replace(&table, 6, gaps.into_any());
        replace(
            packet,
            3,
            replace(&tables, code_index, table.into_any()).into_any(),
        )
    }

    #[test]
    fn interpreter_call_class_receipt_is_not_an_explicit_build_class_expression() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\nclass Declared:\n    pass\nmade = __build_class__(lambda: None, 'Ordinary')\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let module = map.code(py, &root).unwrap();
            let class = one(&map, InterpreterCodeRole::ClassNamespace, "Declared");
            let call = module
                .calls
                .values()
                .find(|call| {
                    matches!(
                        call.origin.role,
                        InterpreterCallRole::ClassConstruction { .. }
                    )
                })
                .unwrap();
            let required = map.class_call(py, &root, call.instruction_ordinal).unwrap();
            assert_eq!(required.class_body_ordinal(), Some(class.ordinal()));
            assert_eq!(required.source_definition(), class.source());
            assert_eq!(required.input.channel, InterpreterCallChannel::Null);
            assert_eq!(required.input.preloaded_value_count, 2);
            assert_eq!(required.native_value_argument_count, Some(2));
            let explicit = module
                .calls
                .values()
                .find(|call| call.origin.role == InterpreterCallRole::SourceExpression)
                .unwrap();
            assert!(
                map.class_call(py, &root, explicit.instruction_ordinal)
                    .is_err()
            );
            assert!(
                map.decorator_call(py, &root, explicit.instruction_ordinal)
                    .is_err()
            );
            assert!(map.class_call(py, &root, module.instruction_count).is_err());
            assert_eq!(map.call(py, &root, 0).unwrap(), None);
        });
    }

    #[test]
    fn interpreter_call_decorator_order_and_factory_expressions_have_distinct_original_roles() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef mark(value):\n    return lambda cls: cls\n@mark(0)\n@mark(1)\nclass C:\n    pass\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let module = map.code(py, &root).unwrap();
            let class = one(&map, InterpreterCodeRole::ClassNamespace, "C");
            let mut indices = Vec::new();
            for call in module.calls.values() {
                if let InterpreterCallRole::Decorator {
                    index,
                    expression_range,
                } = call.origin.role
                {
                    indices.push(index);
                    let actual = map
                        .decorator_call(py, &root, call.instruction_ordinal)
                        .unwrap();
                    assert_eq!(actual.source_definition(), class.source());
                    assert_eq!(
                        actual.input.channel,
                        InterpreterCallChannel::LeadingArgument
                    );
                    assert_eq!(actual.input.preloaded_value_count, 0);
                    assert_eq!(actual.native_value_argument_count, Some(0));
                    assert!(actual.input.positional_entries.is_empty());
                    let factory = module
                        .calls
                        .values()
                        .find(|candidate| {
                            candidate.origin.role == InterpreterCallRole::SourceExpression
                                && candidate.origin.original_range == expression_range
                        })
                        .unwrap();
                    assert!(factory.instruction_ordinal < actual.instruction_ordinal);
                    assert!(
                        map.decorator_call(py, &root, factory.instruction_ordinal)
                            .is_err()
                    );
                    assert!(factory.class_body_ordinal().is_none());
                }
            }
            assert_eq!(indices, [1, 0]);
        });
    }

    #[test]
    fn interpreter_call_generic_scope_pair_uses_actual_direct_child_and_native_default_channels() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef decorate(value):\n    return value\n@decorate\nclass Generic[T](Base):\n    pass\n@decorate\ndef function[T](x=left, *, y=right):\n    return x, y\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let module = map.code(py, &root).unwrap();
            let tree = native_tree(py, &root).unwrap();
            let class = one(&map, InterpreterCodeRole::ClassNamespace, "Generic");
            let scope = one(&map, InterpreterCodeRole::TypeParameterScope, "Generic");
            let invocation = module
                .calls
                .values()
                .find(|call| call.generic_scope_ordinal() == Some(scope.ordinal()))
                .unwrap();
            let invocation = map
                .generic_scope_call(py, &root, invocation.instruction_ordinal)
                .unwrap();
            assert_eq!(invocation.source_definition(), class.source());
            assert_eq!(invocation.input.channel, InterpreterCallChannel::Null);
            assert_eq!(invocation.native_value_argument_count, Some(0));
            assert_eq!(class.parent_ordinal(), invocation.generic_scope_ordinal());
            let inner = scope
                .calls
                .values()
                .find(|call| call.class_body_ordinal() == Some(class.ordinal()))
                .unwrap();
            let actual_inner = map
                .class_call(
                    py,
                    &tree[scope.ordinal() as usize].0,
                    inner.instruction_ordinal,
                )
                .unwrap();
            assert_eq!(
                actual_inner.source_definition(),
                invocation.source_definition()
            );
            assert_eq!(
                actual_inner.input.positional_entries.last().unwrap().kind,
                InterpreterPositionalEntryKind::GenericBaseInjected
            );
            assert!(
                actual_inner
                    .input
                    .positional_entries
                    .last()
                    .unwrap()
                    .source_range
                    .is_none()
            );
            assert!(
                map.class_call(py, &root, invocation.instruction_ordinal)
                    .is_err()
            );
            assert!(module.calls.values().any(|call| {
                matches!(call.origin.role, InterpreterCallRole::Decorator { .. })
                    && call.source_definition() == class.source()
            }));
            let function_scope = one(&map, InterpreterCodeRole::TypeParameterScope, "function");
            let defaults = module
                .calls
                .values()
                .find(|call| call.generic_scope_ordinal() == Some(function_scope.ordinal()))
                .unwrap();
            let defaults = map
                .generic_scope_call(py, &root, defaults.instruction_ordinal)
                .unwrap();
            assert_eq!(function_scope.layout().parameters.len(), 2);
            assert_eq!(
                defaults.input.channel,
                InterpreterCallChannel::LeadingArgument
            );
            assert_eq!(defaults.input.preloaded_value_count, 1);
            assert_eq!(defaults.native_value_argument_count, Some(1));
            assert!(defaults.input.positional_entries.is_empty());
        });
    }

    #[test]
    fn interpreter_call_expanded_inputs_keep_native_allocation_groups_and_unknown_value_count() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef invoke(target, item, iterable, mapping):\n    target(item)\n    target(named=item)\n    target(*iterable)\n    target(item, **mapping)\n    target(item, *iterable, first=item, **mapping, last=item)\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let invoke = one(&map, InterpreterCodeRole::SourceFunction, "invoke");
            let tree = native_tree(py, &root).unwrap();
            let forms = invoke
                .calls
                .values()
                .map(|call| call.form)
                .collect::<Vec<_>>();
            assert_eq!(
                forms,
                [
                    InterpreterCallForm::Positional,
                    InterpreterCallForm::Keywords,
                    InterpreterCallForm::Expanded,
                    InterpreterCallForm::Expanded,
                    InterpreterCallForm::Expanded,
                ]
            );
            let positional = invoke
                .calls
                .values()
                .map(|call| call.input.positional_kind)
                .collect::<Vec<_>>();
            assert_eq!(
                positional,
                [
                    InterpreterPositionalKind::Vector,
                    InterpreterPositionalKind::Vector,
                    InterpreterPositionalKind::SoleStarDeferred,
                    InterpreterPositionalKind::ExpandedDirectTuple,
                    InterpreterPositionalKind::ExpandedListAtFirstStar,
                ]
            );
            for call in invoke.calls.values() {
                if call.form == InterpreterCallForm::Expanded {
                    assert!(call.native_value_argument_count.is_none());
                }
                if call.gaps.iter().any(|gap| matches!(gap.reason, 1 | 6 | 10)) {
                    // Native may fold preparation. Metadata stays visible but
                    // an unavailable actual input recipe never gains admission.
                    assert!(
                        map.call(
                            py,
                            &tree[invoke.ordinal() as usize].0,
                            call.instruction_ordinal
                        )
                        .is_err()
                    );
                } else {
                    assert_eq!(
                        map.call(
                            py,
                            &tree[invoke.ordinal() as usize].0,
                            call.instruction_ordinal
                        )
                        .unwrap(),
                        Some(call)
                    );
                }
            }
            let direct = invoke.calls.values().next().unwrap();
            assert!(
                map.call(
                    py,
                    &tree[invoke.ordinal() as usize].0,
                    direct.instruction_ordinal
                )
                .unwrap()
                .is_some()
            );
            let grouped = invoke.calls.values().last().unwrap();
            assert_eq!(
                grouped
                    .input
                    .keyword_groups
                    .iter()
                    .map(|group| (group.kind, group.first, group.count))
                    .collect::<Vec<_>>(),
                [
                    (InterpreterKeywordEntryKind::Named, 0, 1),
                    (InterpreterKeywordEntryKind::Mapping, 1, 1),
                    (InterpreterKeywordEntryKind::Named, 2, 1),
                ]
            );
            assert_eq!(
                grouped.input.keyword_entries[0].native_name.as_deref(),
                Some("first")
            );
            assert_eq!(
                grouped.input.keyword_entries[2].native_name.as_deref(),
                Some("last")
            );
        });
    }

    #[test]
    fn interpreter_call_large_expanded_source_retains_before_arguments_preparation() {
        let _guard = native_lock();
        Python::attach(|py| {
            let arguments = (0..40)
                .map(|index| format!("value{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let source = format!(
                "from __future__ import strict\ndef invoke(target):\n    return target({arguments})\n"
            );
            let fixture = Fixture::new(py, &source);
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let invoke = one(&map, InterpreterCodeRole::SourceFunction, "invoke");
            let call = invoke.calls.values().next().unwrap();
            assert_eq!(
                call.input.positional_kind,
                InterpreterPositionalKind::ExpandedListBeforeArguments
            );
            assert_eq!(call.input.positional_entries.len(), 40);
            assert!(
                call.input
                    .positional_entries
                    .iter()
                    .all(|entry| entry.kind == InterpreterPositionalEntryKind::Source)
            );
            assert_eq!(call.form, InterpreterCallForm::Expanded);
            assert!(call.native_value_argument_count.is_none());
        });
    }

    #[test]
    fn interpreter_call_nonselected_helpers_and_guarded_lowered_alternatives_stay_explicit() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef invoke(manager, values):\n    with manager:\n        return list(value for value in values)\nclass C(Base):\n    def method(self):\n        return super().method()\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let tree = native_tree(py, &root).unwrap();
            let packet = tuple(bindings);
            let tables = tuple(packet.get_item(3).unwrap());
            let mut helpers = 0;
            for (index, code) in map.codes.iter().enumerate() {
                let table = tuple(tables.get_item(index).unwrap());
                for row in tuple(table.get_item(5).unwrap()).iter() {
                    let row = tuple(row);
                    let kind =
                        unsigned(tuple(row.get_item(0).unwrap()).get_item(0).unwrap()).unwrap();
                    if kind >= 4 {
                        for emission in tuple(row.get_item(1).unwrap()).iter() {
                            let ordinal = unsigned(tuple(emission).get_item(0).unwrap()).unwrap();
                            assert!(map.call(py, &tree[index].0, ordinal).unwrap().is_none());
                            helpers += 1;
                        }
                    }
                }
                for status in code.call_origins() {
                    if status.gaps.iter().any(|gap| gap.reason == 8)
                        && status.instruction_ordinals.is_empty()
                    {
                        assert!(matches!(
                            status.origin.role,
                            InterpreterCallRole::SourceExpression
                        ));
                    }
                }
            }
            assert!(
                helpers > 0,
                "requires actual nonselected native helper CALLs"
            );
            let invoke = one(&map, InterpreterCodeRole::SourceFunction, "invoke");
            assert!(
                invoke
                    .call_origins()
                    .iter()
                    .any(|status| status.gaps.iter().any(|gap| gap.reason == 7))
            );
            let method = one(&map, InterpreterCodeRole::SourceFunction, "C.method");
            assert!(method.call_origins().iter().any(|status| {
                status.instruction_ordinals.is_empty()
                    && status.gaps.iter().any(|gap| gap.reason == 8)
            }));
        });
    }

    #[test]
    fn interpreter_call_missing_class_decorator_and_generic_rows_refuse() {
        let _guard = native_lock();
        Python::attach(|py| {
            for (source, kind) in [
                ("from __future__ import strict\nclass C:\n    pass\n", 2),
                (
                    "from __future__ import strict\n@decorate\nclass C:\n    pass\n",
                    1,
                ),
                ("from __future__ import strict\nclass C[T]:\n    pass\n", 3),
            ] {
                let fixture = Fixture::new(py, source);
                let (root, bindings) = fixture.compile(py);
                let packet = tuple(bindings);
                let tables = tuple(packet.get_item(3).unwrap());
                let table = tuple(tables.get_item(0).unwrap());
                let index = call_row_index(&table, kind);
                let calls = tuple(table.get_item(5).unwrap());
                let calls = PyTuple::new(
                    py,
                    calls
                        .iter()
                        .enumerate()
                        .filter_map(|(i, row)| (i != index).then_some(row))
                        .collect::<Vec<_>>(),
                )
                .unwrap();
                let table = replace(&table, 5, calls.into_any());
                let changed = replace(&packet, 3, replace(&tables, 0, table.into_any()).into_any());
                assert!(fixture.decode(py, &root, &changed.into_any()).is_err());
            }
        });
    }

    #[test]
    fn interpreter_call_wrong_child_decorator_index_or_source_role_refuses() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\n@decorate\nclass First:\n    pass\nclass Second:\n    pass\n",
            );
            let (root, bindings) = fixture.compile(py);
            let map = fixture.decode(py, &root, &bindings).unwrap();
            let packet = tuple(bindings);
            let table = tuple(tuple(packet.get_item(3).unwrap()).get_item(0).unwrap());
            let calls = tuple(table.get_item(5).unwrap());
            let class_index = call_row_index(&table, 2);
            let class = tuple(calls.get_item(class_index).unwrap());
            let origin = tuple(class.get_item(0).unwrap());
            let other = one(&map, InterpreterCodeRole::ClassNamespace, "Second");
            let wrong = replace(
                &class,
                0,
                replace(&origin, 2, number(py, other.ordinal())).into_any(),
            );
            assert!(
                fixture
                    .decode(
                        py,
                        &root,
                        &change_call_row(&packet, 0, class_index, wrong.into_any()).into_any()
                    )
                    .is_err()
            );
            let wrong_kind = replace(&origin, 0, number(py, 0));
            let wrong_kind = replace(&wrong_kind, 2, py.None().into_bound(py));
            let wrong = replace(&class, 0, wrong_kind.into_any());
            assert!(
                fixture
                    .decode(
                        py,
                        &root,
                        &change_call_row(&packet, 0, class_index, wrong.into_any()).into_any()
                    )
                    .is_err()
            );
            let decorator_index = call_row_index(&table, 1);
            let decorator = tuple(calls.get_item(decorator_index).unwrap());
            let origin = tuple(decorator.get_item(0).unwrap());
            let wrong = replace(
                &decorator,
                0,
                replace(&origin, 2, number(py, 99)).into_any(),
            );
            assert!(
                fixture
                    .decode(
                        py,
                        &root,
                        &change_call_row(&packet, 0, decorator_index, wrong.into_any()).into_any()
                    )
                    .is_err()
            );
        });
    }

    #[test]
    fn interpreter_call_duplicate_and_store_conflicting_physical_sites_refuse() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(py, "from __future__ import strict\nclass C:\n    pass\n");
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let table = tuple(tuple(packet.get_item(3).unwrap()).get_item(0).unwrap());
            let index = call_row_index(&table, 2);
            let row = tuple(tuple(table.get_item(5).unwrap()).get_item(index).unwrap());
            let emissions = tuple(row.get_item(1).unwrap());
            let emission = tuple(emissions.get_item(0).unwrap());
            let duplicate = PyTuple::new(
                py,
                [emission.clone().into_any(), emission.clone().into_any()],
            )
            .unwrap();
            let bad = replace(&row, 1, duplicate.into_any());
            assert!(
                fixture
                    .decode(
                        py,
                        &root,
                        &change_call_row(&packet, 0, index, bad.into_any()).into_any()
                    )
                    .is_err()
            );
            let store = tuple(tuple(table.get_item(4).unwrap()).get_item(0).unwrap());
            let store_emission = tuple(tuple(store.get_item(1).unwrap()).get_item(0).unwrap());
            let bad_emission = replace(&emission, 0, store_emission.get_item(0).unwrap());
            let bad = replace(
                &row,
                1,
                replace(&emissions, 0, bad_emission.into_any()).into_any(),
            );
            assert!(
                fixture
                    .decode(
                        py,
                        &root,
                        &change_call_row(&packet, 0, index, bad.into_any()).into_any()
                    )
                    .is_err()
            );
        });
    }

    #[test]
    fn interpreter_call_corrupt_input_counts_channels_and_original_ranges_refuse() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\nclass C(Base, metaclass=Meta):\n    pass\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let table = tuple(tuple(packet.get_item(3).unwrap()).get_item(0).unwrap());
            let index = call_row_index(&table, 2);
            let row = tuple(tuple(table.get_item(5).unwrap()).get_item(index).unwrap());
            let emissions = tuple(row.get_item(1).unwrap());
            let emission = tuple(emissions.get_item(0).unwrap());
            let input = tuple(emission.get_item(4).unwrap());
            let positional = tuple(input.get_item(2).unwrap());
            let entries = tuple(positional.get_item(1).unwrap());
            let entry = tuple(entries.get_item(0).unwrap());
            let no_source = replace(&entry, 1, py.None().into_bound(py));
            let bad_position = replace(
                &positional,
                1,
                replace(&entries, 0, no_source.into_any()).into_any(),
            );
            let bad = [
                replace(&emission, 3, number(py, 99)),
                replace(&emission, 4, replace(&input, 0, number(py, 2)).into_any()),
                replace(&emission, 4, replace(&input, 1, number(py, 1)).into_any()),
                replace(
                    &emission,
                    4,
                    replace(&input, 2, bad_position.into_any()).into_any(),
                ),
            ];
            for bad in bad {
                let bad = replace(&row, 1, replace(&emissions, 0, bad.into_any()).into_any());
                assert!(
                    fixture
                        .decode(
                            py,
                            &root,
                            &change_call_row(&packet, 0, index, bad.into_any()).into_any()
                        )
                        .is_err()
                );
            }
        });
    }

    #[test]
    fn interpreter_call_input_gap_blocks_the_exact_site_but_gap4_and9_are_not_jit_requirements() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(py, "from __future__ import strict\nclass C:\n    pass\n");
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let table = tuple(tuple(packet.get_item(3).unwrap()).get_item(0).unwrap());
            let index = call_row_index(&table, 2);
            let row = tuple(tuple(table.get_item(5).unwrap()).get_item(index).unwrap());
            let emissions = tuple(row.get_item(1).unwrap());
            let emission = tuple(emissions.get_item(0).unwrap());
            let ordinal = unsigned(emission.get_item(0).unwrap()).unwrap();
            // Exact native-supplied offset, used only to preserve a real opcode
            // in the test mutation; no offset/name scan or production selection.
            let offset = unsigned(emission.get_item(1).unwrap()).unwrap() as usize;
            let code_bytes = root.getattr("co_code").unwrap();
            let opcode = u32::from(code_bytes.cast::<PyBytes>().unwrap().as_bytes()[offset]);
            // Scalar-only modified-wire controls; these are not native-positive
            // divergence/input-loss witnesses or authority for runtime execution.
            let missing_input = add_call_gap(
                &packet,
                0,
                row.get_item(0).unwrap(),
                10,
                Some((ordinal, opcode)),
                emission.get_item(5).unwrap(),
            );
            let map = fixture
                .decode(py, &root, &missing_input.into_any())
                .unwrap();
            assert!(map.class_call(py, &root, ordinal).is_err());
            assert!(
                map.code(py, &root)
                    .unwrap()
                    .call_origins()
                    .iter()
                    .any(|status| status.gaps.iter().any(|gap| gap.reason == 10))
            );
            let no_context = replace(&emission, 5, py.None().into_bound(py));
            let row_no_context = replace(
                &row,
                1,
                replace(&emissions, 0, no_context.into_any()).into_any(),
            );
            let changed = change_call_row(&packet, 0, index, row_no_context.into_any());
            assert!(
                fixture
                    .decode(py, &root, &changed.clone().into_any())
                    .is_err()
            );
            let changed = add_call_gap(
                &changed,
                0,
                row.get_item(0).unwrap(),
                9,
                Some((ordinal, opcode)),
                py.None().into_bound(py),
            );
            let changed = add_call_gap(
                &changed,
                0,
                row.get_item(0).unwrap(),
                4,
                None,
                py.None().into_bound(py),
            );
            let map = fixture.decode(py, &root, &changed.into_any()).unwrap();
            let receipt = map.class_call(py, &root, ordinal).unwrap();
            assert!(receipt.gaps.iter().any(|gap| gap.reason == 9));
            assert!(receipt.gaps.iter().any(|gap| gap.reason == 4));
            assert!(receipt.class_body_ordinal().is_some());
        });
    }

    fn annotation_scope<'py>(packet: &Bound<'py, PyTuple>, ordinal: u32) -> Bound<'py, PyTuple> {
        tuple(
            tuple(packet.get_item(2).unwrap())
                .get_item(ordinal as usize)
                .unwrap(),
        )
    }

    fn annotation_code<'py>(packet: &Bound<'py, PyTuple>, ordinal: u32) -> Bound<'py, PyAny> {
        tuple(
            tuple(packet.get_item(1).unwrap())
                .get_item(ordinal as usize)
                .unwrap(),
        )
        .get_item(2)
        .unwrap()
    }

    fn change_annotation_scope<'py>(
        packet: &Bound<'py, PyTuple>,
        ordinal: u32,
        scope: Bound<'py, PyTuple>,
    ) -> Bound<'py, PyTuple> {
        let scopes = tuple(packet.get_item(2).unwrap());
        replace(
            packet,
            2,
            replace(&scopes, ordinal as usize, scope.into_any()).into_any(),
        )
    }

    fn annotation_edges<'py>(
        packet: &Bound<'py, PyTuple>,
        parent: u32,
        child: u32,
    ) -> Vec<(usize, Bound<'py, PyTuple>)> {
        tuple(annotation_scope(packet, parent).get_item(4).unwrap())
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let row = tuple(row);
                (unsigned(row.get_item(0).unwrap()).unwrap() == child).then_some((index, row))
            })
            .collect()
    }

    fn annotation_header_slot(packet: &Bound<'_, PyTuple>, class: u32, role: u32) -> u32 {
        let scope = annotation_scope(packet, class);
        let actions = tuple(scope.get_item(6).unwrap());
        let matching: Vec<_> = tuple(actions.get_item(0).unwrap())
            .iter()
            .filter_map(|row| {
                let row = tuple(row);
                (unsigned(row.get_item(1).unwrap()).unwrap() == role).then_some(row)
            })
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "fixture requires the actual native header role"
        );
        let owner = unsigned(matching[0].get_item(0).unwrap()).unwrap();
        let owner = tuple(
            tuple(scope.get_item(2).unwrap())
                .get_item(owner as usize)
                .unwrap(),
        );
        assert_eq!(unsigned(owner.get_item(1).unwrap()).unwrap(), 0);
        unsigned(owner.get_item(2).unwrap()).unwrap()
    }

    fn annotation_edge_slot(edge: &Bound<'_, PyTuple>) -> u32 {
        unsigned(tuple(edge.get_item(3).unwrap()).get_item(1).unwrap()).unwrap()
    }

    fn annotation_edge_for_free<'py>(
        packet: &Bound<'py, PyTuple>,
        parent: u32,
        child: u32,
        free: u32,
    ) -> Bound<'py, PyTuple> {
        let mut matching = annotation_edges(packet, parent, child)
            .into_iter()
            .filter(|(_, edge)| unsigned(edge.get_item(2).unwrap()).unwrap() == free);
        let (_, edge) = matching
            .next()
            .expect("fixture needs its exact native capture edge");
        assert!(
            matching.next().is_none(),
            "fixture needs a unique native capture edge"
        );
        edge
    }

    fn assert_annotation_lexical_origin(
        py: Python<'_>,
        source: &StrictInterpreterSource,
        code: &Bound<'_, PyAny>,
        free: u32,
        parent: u32,
        slot: u32,
    ) {
        assert!(matches!(source.annotation_capture(py, code, free).unwrap(),
            InterpreterAnnotationCaptureOrigin::Lexical {
                parent_ordinal, parent_slot, binding_scope,
            } if *parent_ordinal == parent && *parent_slot == slot
                && binding_scope == source.codes[parent as usize].source()));
    }

    #[test]
    fn interpreter_annotation_capture_class_roles_follow_actual_header_export_and_provider() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef decorate(value):\n    return value\n@decorate\nclass Owner:\n    Alias = int\n    field: Alias\n    def method(self, value: Alias) -> Alias:\n        return value\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let class = one(&map, InterpreterCodeRole::ClassNamespace, "Owner");
            let slot = annotation_header_slot(&packet, class.ordinal(), 3);
            let actions = tuple(
                annotation_scope(&packet, class.ordinal())
                    .get_item(6)
                    .unwrap(),
            );
            let exports = tuple(actions.get_item(1).unwrap());
            assert!(exports.iter().any(|row| {
                let row = tuple(row);
                unsigned(row.get_item(0).unwrap()).unwrap() == 1
                    && unsigned(tuple(row.get_item(1).unwrap()).get_item(1).unwrap()).unwrap()
                        == slot
            }));
            for name in ["Owner", "Owner.method"] {
                let provider = one(&map, InterpreterCodeRole::AnnotationProvider, name);
                assert_eq!(provider.parent_ordinal(), Some(class.ordinal()));
                let code = annotation_code(&packet, provider.ordinal());
                let edges = annotation_edges(&packet, class.ordinal(), provider.ordinal());
                assert_eq!(edges.len(), 1);
                let edge = &edges[0].1;
                assert_eq!(annotation_edge_slot(edge), slot);
                let free = unsigned(edge.get_item(2).unwrap()).unwrap();
                assert_eq!(
                    map.annotation_capture(py, &code, free).unwrap(),
                    &InterpreterAnnotationCaptureOrigin::ClassDictionary {
                        class_ordinal: class.ordinal(),
                        class_definition: class.source().clone(),
                        class_slot: slot,
                    }
                );
                if name == "Owner.method" {
                    assert_ne!(
                        provider.source(),
                        class.source(),
                        "a method provider belongs to the method, not a fabricated class declaration"
                    );
                }
            }
        });
    }

    #[test]
    fn interpreter_annotation_capture_conditional_set_remains_distinct_from_class_dictionary() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\nclass Owner:\n    Alias = int\n    if enabled:\n        field: Alias\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let class = one(&map, InterpreterCodeRole::ClassNamespace, "Owner");
            let provider = one(&map, InterpreterCodeRole::AnnotationProvider, "Owner");
            let dictionary = annotation_header_slot(&packet, class.ordinal(), 3);
            let conditional = annotation_header_slot(&packet, class.ordinal(), 4);
            assert_ne!(dictionary, conditional);
            let code = annotation_code(&packet, provider.ordinal());
            let edges = annotation_edges(&packet, class.ordinal(), provider.ordinal());
            assert_eq!(edges.len(), 2);
            for (_, edge) in edges {
                let slot = annotation_edge_slot(&edge);
                let free = unsigned(edge.get_item(2).unwrap()).unwrap();
                let expected = if slot == dictionary {
                    InterpreterAnnotationCaptureOrigin::ClassDictionary {
                        class_ordinal: class.ordinal(),
                        class_definition: class.source().clone(),
                        class_slot: slot,
                    }
                } else {
                    assert_eq!(slot, conditional);
                    InterpreterAnnotationCaptureOrigin::ConditionalAnnotations {
                        class_ordinal: class.ordinal(),
                        class_definition: class.source().clone(),
                        class_slot: slot,
                    }
                };
                assert_eq!(map.annotation_capture(py, &code, free).unwrap(), &expected);
            }
        });
    }

    #[test]
    fn interpreter_annotation_capture_lexical_names_and_dictionary_literals_do_not_grant_special_roles()
     {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef factory(__classdict__):\n    __conditional_annotations__ = {'T': int}\n    def target(value: __classdict__) -> __conditional_annotations__:\n        return value\n    return target\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let parent = one(&map, InterpreterCodeRole::SourceFunction, "factory");
            let provider = one(
                &map,
                InterpreterCodeRole::AnnotationProvider,
                "factory.<locals>.target",
            );
            let code = annotation_code(&packet, provider.ordinal());
            let edges = annotation_edges(&packet, parent.ordinal(), provider.ordinal());
            assert_eq!(edges.len(), 2);
            assert!(
                annotation_scope(&packet, parent.ordinal())
                    .get_item(6)
                    .unwrap()
                    .is_none()
            );
            for (_, edge) in edges {
                let slot = annotation_edge_slot(&edge);
                assert_ne!(parent.layout().locals[slot as usize].kind & CELL, 0);
                let free = unsigned(edge.get_item(2).unwrap()).unwrap();
                assert_eq!(
                    map.annotation_capture(py, &code, free).unwrap(),
                    &InterpreterAnnotationCaptureOrigin::Lexical {
                        parent_ordinal: parent.ordinal(),
                        parent_slot: slot,
                        binding_scope: parent.source().clone(),
                    }
                );
            }
            let scope = annotation_scope(&packet, parent.ordinal());
            let false_class_actions = PyTuple::new(
                py,
                [PyTuple::empty(py).into_any(), PyTuple::empty(py).into_any()],
            )
            .unwrap();
            let changed = change_annotation_scope(
                &packet,
                parent.ordinal(),
                replace(&scope, 6, false_class_actions.into_any()),
            );
            assert!(fixture.decode(py, &root, &changed.into_any()).is_err());
        });
    }

    #[test]
    fn interpreter_annotation_capture_proven_forwarding_keeps_generic_cells_unresolved() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\ndef outer(T):\n    class Owner:\n        def method(self, value: T) -> T:\n            return value\n    return Owner\ndef generic[T](value: T) -> T:\n    return value\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let class = one(
                &map,
                InterpreterCodeRole::ClassNamespace,
                "outer.<locals>.Owner",
            );
            let provider = one(
                &map,
                InterpreterCodeRole::AnnotationProvider,
                "outer.<locals>.Owner.method",
            );
            let code = annotation_code(&packet, provider.ordinal());
            let mut forwarded = 0;
            for (_, edge) in annotation_edges(&packet, class.ordinal(), provider.ordinal()) {
                let slot = annotation_edge_slot(&edge);
                if class.layout().locals[slot as usize].kind & FREE != 0 {
                    forwarded += 1;
                    let free = unsigned(edge.get_item(2).unwrap()).unwrap();
                    let outer = one(&map, InterpreterCodeRole::SourceFunction, "outer");
                    let class_free = class.layout().locals[slot as usize].free_ordinal.unwrap();
                    let upstream = annotation_edge_for_free(
                        &packet,
                        outer.ordinal(),
                        class.ordinal(),
                        class_free,
                    );
                    let terminal = annotation_edge_slot(&upstream);
                    assert_ne!(outer.layout().locals[terminal as usize].kind & CELL, 0);
                    assert_eq!(outer.layout().locals[terminal as usize].kind & FREE, 0);
                    assert_annotation_lexical_origin(
                        py,
                        &map,
                        &code,
                        free,
                        outer.ordinal(),
                        terminal,
                    );
                }
            }
            assert_eq!(
                forwarded, 1,
                "requires a real parent FREE, not an invented lexical CELL"
            );
            let provider = one(&map, InterpreterCodeRole::AnnotationProvider, "generic");
            let parent = one(&map, InterpreterCodeRole::TypeParameterScope, "generic");
            assert_eq!(provider.parent_ordinal(), Some(parent.ordinal()));
            let code = annotation_code(&packet, provider.ordinal());
            let free: Vec<_> = provider.layout().free_variables().collect();
            assert_eq!(free.len(), 1);
            assert_eq!(
                map.annotation_capture(py, &code, free[0].0).unwrap(),
                &InterpreterAnnotationCaptureOrigin::Unresolved(
                    InterpreterAnnotationCaptureUnresolved::UnrepresentedParent
                )
            );
        });
    }

    #[test]
    fn interpreter_annotation_capture_follows_decorated_function_and_class_free_edges() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                concat!(
                    "from __future__ import strict
",
                    "def decorate(value):
    return value
",
                    "def outer(T):
",
                    "    @decorate
    def middle():
",
                    "        @decorate
        class Owner:
",
                    "            field: T
",
                    "        return Owner
",
                    "    return middle
",
                ),
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let outer = one(&map, InterpreterCodeRole::SourceFunction, "outer");
            let middle = one(
                &map,
                InterpreterCodeRole::SourceFunction,
                "outer.<locals>.middle",
            );
            let class = one(
                &map,
                InterpreterCodeRole::ClassNamespace,
                "outer.<locals>.middle.<locals>.Owner",
            );
            let provider = one(
                &map,
                InterpreterCodeRole::AnnotationProvider,
                "outer.<locals>.middle.<locals>.Owner",
            );
            let (free, _, _) = provider
                .layout()
                .free_variables()
                .find(|(_, _, name)| *name == "T")
                .unwrap();
            let direct =
                annotation_edge_for_free(&packet, class.ordinal(), provider.ordinal(), free);
            let class_slot = &class.layout().locals[annotation_edge_slot(&direct) as usize];
            let class_free = class_slot
                .free_ordinal
                .expect("class must actually forward a FREE");
            let class_edge =
                annotation_edge_for_free(&packet, middle.ordinal(), class.ordinal(), class_free);
            let middle_slot = &middle.layout().locals[annotation_edge_slot(&class_edge) as usize];
            let middle_free = middle_slot
                .free_ordinal
                .expect("middle must actually forward a FREE");
            let middle_edge =
                annotation_edge_for_free(&packet, outer.ordinal(), middle.ordinal(), middle_free);
            let terminal = annotation_edge_slot(&middle_edge);
            assert_ne!(outer.layout().locals[terminal as usize].kind & CELL, 0);
            assert_eq!(outer.layout().locals[terminal as usize].kind & FREE, 0);
            // Decorated definition identity is not its native closure site.
            assert_ne!(class.source().source_range, class.native_range().unwrap());
            assert_ne!(middle.source().source_range, middle.native_range().unwrap());
            assert_annotation_lexical_origin(
                py,
                &map,
                &annotation_code(&packet, provider.ordinal()),
                free,
                outer.ordinal(),
                terminal,
            );
        });
    }

    #[test]
    fn interpreter_annotation_capture_nested_classes_forward_only_the_exact_lexical_cell() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                concat!(
                    "from __future__ import strict
",
                    "def outer(T):
",
                    "    class First:
",
                    "        class Second:
",
                    "            field: T
",
                    "    return First
",
                ),
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let outer = one(&map, InterpreterCodeRole::SourceFunction, "outer");
            let first = one(
                &map,
                InterpreterCodeRole::ClassNamespace,
                "outer.<locals>.First",
            );
            let second = one(
                &map,
                InterpreterCodeRole::ClassNamespace,
                "outer.<locals>.First.Second",
            );
            let provider = one(
                &map,
                InterpreterCodeRole::AnnotationProvider,
                "outer.<locals>.First.Second",
            );
            let (free, _, _) = provider
                .layout()
                .free_variables()
                .find(|(_, _, name)| *name == "T")
                .unwrap();
            let direct =
                annotation_edge_for_free(&packet, second.ordinal(), provider.ordinal(), free);
            let second_free = second.layout().locals[annotation_edge_slot(&direct) as usize]
                .free_ordinal
                .unwrap();
            let second_edge =
                annotation_edge_for_free(&packet, first.ordinal(), second.ordinal(), second_free);
            let first_free = first.layout().locals[annotation_edge_slot(&second_edge) as usize]
                .free_ordinal
                .unwrap();
            let first_edge =
                annotation_edge_for_free(&packet, outer.ordinal(), first.ordinal(), first_free);
            let terminal = annotation_edge_slot(&first_edge);
            assert_ne!(outer.layout().locals[terminal as usize].kind & CELL, 0);
            assert_annotation_lexical_origin(
                py,
                &map,
                &annotation_code(&packet, provider.ordinal()),
                free,
                outer.ordinal(),
                terminal,
            );
        });
    }

    #[test]
    fn interpreter_annotation_capture_lost_intermediate_proof_never_grants_lexical_scope() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                concat!(
                    "from __future__ import strict
",
                    "def outer(T, U):
",
                    "    unrelated = [item for item in (1, 2)]
",
                    "    class Owner:
",
                    "        first: T
",
                    "        second: U
",
                    "    return Owner
",
                ),
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let before = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let outer = one(&before, InterpreterCodeRole::SourceFunction, "outer");
            let class = one(
                &before,
                InterpreterCodeRole::ClassNamespace,
                "outer.<locals>.Owner",
            );
            let provider = one(
                &before,
                InterpreterCodeRole::AnnotationProvider,
                "outer.<locals>.Owner",
            );
            let code = annotation_code(&packet, provider.ordinal());
            let (free, _, _) = provider
                .layout()
                .free_variables()
                .find(|(_, _, name)| *name == "T")
                .unwrap();
            let direct =
                annotation_edge_for_free(&packet, class.ordinal(), provider.ordinal(), free);
            let class_free = class.layout().locals[annotation_edge_slot(&direct) as usize]
                .free_ordinal
                .unwrap();
            let scope = annotation_scope(&packet, outer.ordinal());
            let captures = tuple(scope.get_item(4).unwrap());
            assert!(!tuple(scope.get_item(3).unwrap()).is_empty());
            let (index, edge) = annotation_edges(&packet, outer.ordinal(), class.ordinal())
                .into_iter()
                .find(|(_, edge)| unsigned(edge.get_item(2).unwrap()).unwrap() == class_free)
                .unwrap();
            let terminal = annotation_edge_slot(&edge);
            assert_annotation_lexical_origin(py, &before, &code, free, outer.ordinal(), terminal);

            let missing = PyTuple::new(
                py,
                captures
                    .iter()
                    .enumerate()
                    .filter_map(|(i, row)| (i != index).then_some(row))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let unknown_site = replace(
                &captures,
                index,
                replace(&edge, 1, py.None().into_bound(py)).into_any(),
            );
            let regional = replace(
                &captures,
                index,
                replace(&edge, 4, number(py, 0)).into_any(),
            );
            let ambiguous = PyTuple::new(
                py,
                captures
                    .iter()
                    .chain(std::iter::once(replace(&edge, 4, number(py, 0)).into_any()))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            for (changed, reason) in [
                (
                    missing,
                    InterpreterAnnotationCaptureUnresolved::MissingCapture,
                ),
                (
                    unknown_site,
                    InterpreterAnnotationCaptureUnresolved::CreationSiteUnavailable,
                ),
                (
                    regional,
                    InterpreterAnnotationCaptureUnresolved::RegionalCapture,
                ),
                (
                    ambiguous,
                    InterpreterAnnotationCaptureUnresolved::AmbiguousCapture,
                ),
            ] {
                let changed = change_annotation_scope(
                    &packet,
                    outer.ordinal(),
                    replace(&scope, 4, changed.into_any()),
                );
                let map = fixture.decode(py, &root, &changed.into_any()).unwrap();
                assert_eq!(
                    map.annotation_capture(py, &code, free).unwrap(),
                    &InterpreterAnnotationCaptureOrigin::Unresolved(reason)
                );
            }

            // Scalar corruptions must reject, not borrow another child's/cell's
            // identity or accept the provider's zero-length completion marker
            // as the intervening ClassDef creation site.
            let other = annotation_edges(&packet, outer.ordinal(), class.ordinal())
                .into_iter()
                .find(|(_, candidate)| annotation_edge_slot(candidate) != terminal)
                .unwrap()
                .1;
            for corrupt in [
                replace(&edge, 0, number(py, provider.ordinal())),
                replace(
                    &edge,
                    2,
                    number(py, class.layout().free_variables().count() as u32),
                ),
                replace(&edge, 3, other.get_item(3).unwrap()),
                replace(&edge, 1, direct.get_item(1).unwrap()),
            ] {
                let changed = change_annotation_scope(
                    &packet,
                    outer.ordinal(),
                    replace(
                        &scope,
                        4,
                        replace(&captures, index, corrupt.into_any()).into_any(),
                    ),
                );
                assert!(fixture.decode(py, &root, &changed.into_any()).is_err());
            }
        });
    }

    #[test]
    fn interpreter_annotation_capture_reused_terminal_cell_is_not_forwarded_lexical_authority() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                concat!(
                    "from __future__ import strict
",
                    "def outer(T):
",
                    "    callbacks = [lambda: T for T in (1,)]
",
                    "    class Owner:
",
                    "        field: T
",
                    "    return Owner
",
                ),
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let outer = one(&map, InterpreterCodeRole::SourceFunction, "outer");
            let class = one(
                &map,
                InterpreterCodeRole::ClassNamespace,
                "outer.<locals>.Owner",
            );
            let provider = one(
                &map,
                InterpreterCodeRole::AnnotationProvider,
                "outer.<locals>.Owner",
            );
            let (free, _, _) = provider
                .layout()
                .free_variables()
                .find(|(_, _, name)| *name == "T")
                .unwrap();
            let direct =
                annotation_edge_for_free(&packet, class.ordinal(), provider.ordinal(), free);
            let class_free = class.layout().locals[annotation_edge_slot(&direct) as usize]
                .free_ordinal
                .unwrap();
            let upstream =
                annotation_edge_for_free(&packet, outer.ordinal(), class.ordinal(), class_free);
            let terminal = annotation_edge_slot(&upstream);
            let scope = annotation_scope(&packet, outer.ordinal());
            let touched = tuple(scope.get_item(3).unwrap()).iter().any(|region| {
                tuple(tuple(region).get_item(6).unwrap())
                    .iter()
                    .any(|operation| {
                        unsigned(tuple(operation).get_item(1).unwrap()).unwrap() == terminal
                    })
            });
            assert!(
                touched,
                "requires actual region reuse of the exact terminal carrier"
            );
            assert_eq!(
                map.annotation_capture(py, &annotation_code(&packet, provider.ordinal()), free)
                    .unwrap(),
                &InterpreterAnnotationCaptureOrigin::Unresolved(
                    InterpreterAnnotationCaptureUnresolved::ReusedCarrier
                )
            );
        });
    }

    #[test]
    fn interpreter_annotation_capture_generic_free_forwarding_remains_unproved() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                concat!(
                    "from __future__ import strict\n",
                    "def outer(T):\n",
                    "    def generic[U](value: T) -> T:\n",
                    "        return value\n",
                    "    return generic\n",
                ),
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let generic = one(
                &map,
                InterpreterCodeRole::TypeParameterScope,
                "outer.<locals>.generic",
            );
            let provider = one(
                &map,
                InterpreterCodeRole::AnnotationProvider,
                "outer.<locals>.generic",
            );
            let (free, _, _) = provider
                .layout()
                .free_variables()
                .find(|(_, _, name)| *name == "T")
                .unwrap();
            let edge =
                annotation_edge_for_free(&packet, generic.ordinal(), provider.ordinal(), free);
            assert_ne!(
                generic.layout().locals[annotation_edge_slot(&edge) as usize].kind & FREE,
                0
            );
            assert_eq!(
                map.annotation_capture(py, &annotation_code(&packet, provider.ordinal()), free)
                    .unwrap(),
                &InterpreterAnnotationCaptureOrigin::Unresolved(
                    InterpreterAnnotationCaptureUnresolved::ForwardedFree
                )
            );
        });
    }

    #[test]
    fn interpreter_annotation_capture_region_reuse_is_not_header_cell_authority() {
        let _guard = native_lock();
        Python::attach(|py| {
            // Existing native coupled-source controls, inspected without executing
            // any source body or pretending this is authenticated startup.
            for source in [
                "from __future__ import strict\ndef build(marker):\n    class Owner:\n        values = [lambda: __classdict__ for __classdict__ in (marker,)]\n        field: int\n    return Owner\n",
                "from __future__ import strict\ndef build(marker, enabled):\n    class Owner:\n        values = [lambda: __conditional_annotations__ for __conditional_annotations__ in (marker,)]\n        if enabled:\n            field: int\n    return Owner\n",
            ] {
                let fixture = Fixture::new(py, source);
                let (root, bindings) = fixture.compile(py);
                let packet = tuple(bindings);
                let map = fixture
                    .decode(py, &root, &packet.clone().into_any())
                    .unwrap();
                let class = one(
                    &map,
                    InterpreterCodeRole::ClassNamespace,
                    "build.<locals>.Owner",
                );
                let provider = one(
                    &map,
                    InterpreterCodeRole::AnnotationProvider,
                    "build.<locals>.Owner",
                );
                let code = annotation_code(&packet, provider.ordinal());
                let regions = tuple(
                    annotation_scope(&packet, class.ordinal())
                        .get_item(3)
                        .unwrap(),
                );
                assert!(!regions.is_empty());
                let mut reused = 0;
                for (_, edge) in annotation_edges(&packet, class.ordinal(), provider.ordinal()) {
                    let slot = annotation_edge_slot(&edge);
                    let touched = regions.iter().any(|region| {
                        tuple(tuple(region).get_item(6).unwrap())
                            .iter()
                            .any(|op| unsigned(tuple(op).get_item(1).unwrap()).unwrap() == slot)
                    });
                    if touched {
                        reused += 1;
                        let free = unsigned(edge.get_item(2).unwrap()).unwrap();
                        assert_eq!(
                            map.annotation_capture(py, &code, free).unwrap(),
                            &InterpreterAnnotationCaptureOrigin::Unresolved(
                                InterpreterAnnotationCaptureUnresolved::ReusedCarrier
                            )
                        );
                    }
                }
                assert!(reused > 0, "requires actual native selected-carrier reuse");
            }
        });
    }

    #[test]
    fn interpreter_annotation_capture_unrelated_regions_do_not_filter_native_roles() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\nclass Owner:\n    values = [item for item in (1, 2)]\n    field: int\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let before = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let class = one(&before, InterpreterCodeRole::ClassNamespace, "Owner");
            let provider = one(&before, InterpreterCodeRole::AnnotationProvider, "Owner");
            let code = annotation_code(&packet, provider.ordinal());
            let edge = &annotation_edges(&packet, class.ordinal(), provider.ordinal())[0].1;
            let free = unsigned(edge.get_item(2).unwrap()).unwrap();
            let expected = InterpreterAnnotationCaptureOrigin::ClassDictionary {
                class_ordinal: class.ordinal(),
                class_definition: class.source().clone(),
                class_slot: annotation_header_slot(&packet, class.ordinal(), 3),
            };
            assert_eq!(
                before.annotation_capture(py, &code, free).unwrap(),
                &expected
            );
            let scope = annotation_scope(&packet, class.ordinal());
            assert!(!tuple(scope.get_item(3).unwrap()).is_empty());
            assert_eq!(scope.len(), 7);
            for region in tuple(scope.get_item(3).unwrap()).iter().map(tuple) {
                assert_eq!(region.len(), 8);
                for operation in tuple(region.get_item(6).unwrap()).iter().map(tuple) {
                    assert_ne!(
                        unsigned(operation.get_item(1).unwrap()).unwrap(),
                        annotation_header_slot(&packet, class.ordinal(), 3),
                        "the unrelated iteration carrier is not the dictionary-cell owner"
                    );
                }
            }
        });
    }

    #[test]
    fn interpreter_annotation_capture_missing_or_ambiguous_evidence_never_grants_a_role() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\nclass Owner:\n    values = [item for item in (1, 2)]\n    field: int\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let before = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let class = one(&before, InterpreterCodeRole::ClassNamespace, "Owner");
            let provider = one(&before, InterpreterCodeRole::AnnotationProvider, "Owner");
            let code = annotation_code(&packet, provider.ordinal());
            let scope = annotation_scope(&packet, class.ordinal());
            let captures = tuple(scope.get_item(4).unwrap());
            let edges = annotation_edges(&packet, class.ordinal(), provider.ordinal());
            assert_eq!(edges.len(), 1);
            let (index, edge) = &edges[0];
            let free = unsigned(edge.get_item(2).unwrap()).unwrap();
            let missing = PyTuple::new(
                py,
                captures
                    .iter()
                    .enumerate()
                    .filter_map(|(i, row)| (i != *index).then_some(row))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let unknown_site = replace(
                &captures,
                *index,
                replace(edge, 1, py.None().into_bound(py)).into_any(),
            );
            let regional = replace(
                &captures,
                *index,
                replace(edge, 4, number(py, 0)).into_any(),
            );
            let ambiguous = PyTuple::new(
                py,
                captures
                    .iter()
                    .chain(std::iter::once(replace(edge, 4, number(py, 0)).into_any()))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            for (captures, reason) in [
                (
                    missing,
                    InterpreterAnnotationCaptureUnresolved::MissingCapture,
                ),
                (
                    unknown_site,
                    InterpreterAnnotationCaptureUnresolved::CreationSiteUnavailable,
                ),
                (
                    regional,
                    InterpreterAnnotationCaptureUnresolved::RegionalCapture,
                ),
                (
                    ambiguous,
                    InterpreterAnnotationCaptureUnresolved::AmbiguousCapture,
                ),
            ] {
                // Modified tuple controls can only lose scalar evidence. They
                // never acquire execution authority or a fictional native site.
                let changed = change_annotation_scope(
                    &packet,
                    class.ordinal(),
                    replace(&scope, 4, captures.into_any()),
                );
                let map = fixture.decode(py, &root, &changed.into_any()).unwrap();
                assert_eq!(
                    map.annotation_capture(py, &code, free).unwrap(),
                    &InterpreterAnnotationCaptureOrigin::Unresolved(reason)
                );
            }
            let actions = tuple(scope.get_item(6).unwrap());
            for (field, reason) in [
                (0, InterpreterAnnotationCaptureUnresolved::UnprovenClassCell),
                (
                    1,
                    InterpreterAnnotationCaptureUnresolved::ClassDictionaryNotExported,
                ),
            ] {
                let actions = replace(&actions, field, PyTuple::empty(py).into_any());
                let changed = change_annotation_scope(
                    &packet,
                    class.ordinal(),
                    replace(&scope, 6, actions.into_any()),
                );
                let map = fixture.decode(py, &root, &changed.into_any()).unwrap();
                assert_eq!(
                    map.annotation_capture(py, &code, free).unwrap(),
                    &InterpreterAnnotationCaptureOrigin::Unresolved(reason)
                );
            }
        });
    }

    #[test]
    fn interpreter_annotation_capture_corrupt_owner_header_current_and_export_rows_refuse() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\nclass Owner:\n    if enabled:\n        field: int\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let class = one(&map, InterpreterCodeRole::ClassNamespace, "Owner");
            let provider = one(&map, InterpreterCodeRole::AnnotationProvider, "Owner");
            let scope = annotation_scope(&packet, class.ordinal());
            let actions = tuple(scope.get_item(6).unwrap());
            let headers = tuple(actions.get_item(0).unwrap());
            let header = tuple(headers.get_item(0).unwrap());
            let owner_index = unsigned(header.get_item(0).unwrap()).unwrap() as usize;
            let owners = tuple(scope.get_item(2).unwrap());
            let owner = tuple(owners.get_item(owner_index).unwrap());
            let captures = tuple(scope.get_item(4).unwrap());
            let edges = annotation_edges(&packet, class.ordinal(), provider.ordinal());
            assert_eq!(edges.len(), 2);
            let (edge_index, edge) = &edges[0];
            let current = tuple(edge.get_item(3).unwrap());
            let other_slot = annotation_edge_slot(&edges[1].1);
            assert_ne!(annotation_edge_slot(edge), other_slot);
            let exports = tuple(actions.get_item(1).unwrap());
            let export = tuple(exports.get_item(0).unwrap());
            let mut corrupt = vec![
                replace(&scope, 0, number(py, 0)),
                replace(&scope, 6, py.None().into_bound(py)),
            ];
            for bad_owner in [
                replace(&owner, 0, number(py, owners.len() as u32)),
                replace(&owner, 1, number(py, 1)),
                replace(&owner, 2, number(py, class.layout().locals.len() as u32)),
                replace(&owner, 3, number(py, FREE as u32)),
            ] {
                corrupt.push(replace(
                    &scope,
                    2,
                    replace(&owners, owner_index, bad_owner.into_any()).into_any(),
                ));
            }
            for bad_header in [
                replace(&header, 0, number(py, owners.len() as u32)),
                replace(&header, 1, number(py, 99)),
                replace(&header, 2, number(py, 0)),
            ] {
                let actions = replace(
                    &actions,
                    0,
                    replace(&headers, 0, bad_header.into_any()).into_any(),
                );
                corrupt.push(replace(&scope, 6, actions.into_any()));
            }
            let duplicate = PyTuple::new(
                py,
                headers
                    .iter()
                    .chain(std::iter::once(header.clone().into_any()))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            corrupt.push(replace(
                &scope,
                6,
                replace(&actions, 0, duplicate.into_any()).into_any(),
            ));
            let bad_export = replace(&export, 0, number(py, 9));
            corrupt.push(replace(
                &scope,
                6,
                replace(
                    &actions,
                    1,
                    replace(&exports, 0, bad_export.into_any()).into_any(),
                )
                .into_any(),
            ));
            for bad_edge in [
                replace(edge, 0, number(py, 0)),
                replace(
                    edge,
                    2,
                    number(py, provider.layout().free_variables().count() as u32),
                ),
                replace(edge, 2, PyBool::new(py, false).to_owned().into_any()),
                replace(edge, 3, replace(&current, 0, number(py, 1)).into_any()),
                replace(
                    edge,
                    3,
                    replace(&current, 1, number(py, other_slot)).into_any(),
                ),
                replace(
                    edge,
                    3,
                    replace(&current, 1, number(py, class.layout().locals.len() as u32)).into_any(),
                ),
                replace(edge, 4, number(py, 0)),
            ] {
                corrupt.push(replace(
                    &scope,
                    4,
                    replace(&captures, *edge_index, bad_edge.into_any()).into_any(),
                ));
            }
            let duplicate = PyTuple::new(
                py,
                captures
                    .iter()
                    .chain(std::iter::once(edge.clone().into_any()))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            corrupt.push(replace(&scope, 4, duplicate.into_any()));
            for scope in corrupt {
                let changed = change_annotation_scope(&packet, class.ordinal(), scope);
                assert!(fixture.decode(py, &root, &changed.into_any()).is_err());
            }
        });
    }

    #[test]
    fn interpreter_annotation_capture_creation_and_same_named_classes_keep_exact_identity() {
        let _guard = native_lock();
        Python::attach(|py| {
            let fixture = Fixture::new(
                py,
                "from __future__ import strict\nclass Owner:\n    field: int\nclass Owner:\n    field: str\n",
            );
            let (root, bindings) = fixture.compile(py);
            let packet = tuple(bindings);
            let map = fixture
                .decode(py, &root, &packet.clone().into_any())
                .unwrap();
            let classes: Vec<_> = map
                .codes
                .iter()
                .filter(|code| code.role() == InterpreterCodeRole::ClassNamespace)
                .collect();
            assert_eq!(classes.len(), 2);
            assert_ne!(classes[0].source(), classes[1].source());
            let mut provider_ordinals = Vec::new();
            for class in &classes {
                let provider = map
                    .codes
                    .iter()
                    .find(|code| {
                        code.role() == InterpreterCodeRole::AnnotationProvider
                            && code.parent_ordinal() == Some(class.ordinal())
                    })
                    .unwrap();
                provider_ordinals.push(provider.ordinal());
                let code = annotation_code(&packet, provider.ordinal());
                let edges = annotation_edges(&packet, class.ordinal(), provider.ordinal());
                assert_eq!(edges.len(), 1);
                let (index, edge) = &edges[0];
                let free = unsigned(edge.get_item(2).unwrap()).unwrap();
                assert_eq!(
                    map.annotation_capture(py, &code, free).unwrap(),
                    &InterpreterAnnotationCaptureOrigin::ClassDictionary {
                        class_ordinal: class.ordinal(),
                        class_definition: class.source().clone(),
                        class_slot: annotation_header_slot(&packet, class.ordinal(), 3),
                    }
                );
                let scope = annotation_scope(&packet, class.ordinal());
                let captures = tuple(scope.get_item(4).unwrap());
                // The provider's expression span is real, but is NOT the
                // compiler class-body-completion creation marker.
                let node = tuple(
                    tuple(packet.get_item(1).unwrap())
                        .get_item(provider.ordinal() as usize)
                        .unwrap(),
                );
                let bad = replace(edge, 1, node.get_item(5).unwrap());
                let changed = change_annotation_scope(
                    &packet,
                    class.ordinal(),
                    replace(
                        &scope,
                        4,
                        replace(&captures, *index, bad.into_any()).into_any(),
                    ),
                );
                assert!(fixture.decode(py, &root, &changed.into_any()).is_err());
                assert!(
                    map.annotation_capture(
                        py,
                        &code,
                        provider.layout().free_variables().count() as u32
                    )
                    .is_err()
                );
            }
            let first = classes[0].ordinal();
            let scope = annotation_scope(&packet, first);
            let captures = tuple(scope.get_item(4).unwrap());
            let edges = annotation_edges(&packet, first, provider_ordinals[0]);
            let bad = replace(&edges[0].1, 0, number(py, provider_ordinals[1]));
            let changed = change_annotation_scope(
                &packet,
                first,
                replace(
                    &scope,
                    4,
                    replace(&captures, edges[0].0, bad.into_any()).into_any(),
                ),
            );
            assert!(fixture.decode(py, &root, &changed.into_any()).is_err());
            assert!(map.annotation_capture(py, &root, 0).is_err());
            let (_, foreign) = fixture.compile(py);
            let foreign = tuple(foreign);
            assert!(
                map.annotation_capture(py, &annotation_code(&foreign, provider_ordinals[0]), 0)
                    .is_err()
            );
        });
    }

    fn generator_excerpt_range(source: &str, excerpt: &str) -> SourceRange {
        let mut matches = source.match_indices(excerpt);
        let (start, _) = matches.next().expect("fixture excerpt is present");
        assert!(matches.next().is_none(), "fixture excerpt must be unique");
        SourceRange::new(
            start.try_into().unwrap(),
            (start + excerpt.len()).try_into().unwrap(),
        )
    }

    fn generator_excerpt_wire_span<'py>(
        py: Python<'py>,
        source: &str,
        excerpt: &str,
    ) -> Bound<'py, PyAny> {
        let range = generator_excerpt_range(source, excerpt);
        let coordinate = |offset: u32| {
            let prefix = &source.as_bytes()[..offset as usize];
            let line = prefix.iter().filter(|byte| **byte == b'\n').count() + 1;
            let column = prefix
                .iter()
                .rposition(|byte| *byte == b'\n')
                .map_or(prefix.len(), |newline| prefix.len() - newline - 1);
            (u32::try_from(line).unwrap(), u32::try_from(column).unwrap())
        };
        let (line, column) = coordinate(range.start);
        let (end_line, end_column) = coordinate(range.end);
        PyTuple::new(py, [line, column, end_line, end_column])
            .unwrap()
            .into_any()
    }

    #[test]
    fn interpreter_source_generator_argument_ranges_follow_native_grammar() {
        let _guard = native_lock();
        Python::attach(|py| {
            // Each pair is (original Ruff expression, actual CPython genexp).
            // The independent native compile product must agree with these
            // source excerpts; this is not a rendered-IR or AST-only assertion.
            let cases = [
                (
                    "def invoke(values, target):\n    return list(value for value in values)\n",
                    vec![("value for value in values", "(value for value in values)")],
                    vec![(0, "(value for value in values)")],
                ),
                (
                    "def invoke(values, target):\n    return list((value for value in values))\n",
                    vec![("(value for value in values)", "(value for value in values)")],
                    vec![(0, "(value for value in values)")],
                ),
                (
                    "def invoke(values, target):\n    return list(((value for value in values)))\n",
                    vec![("(value for value in values)", "(value for value in values)")],
                    vec![(0, "(value for value in values)")],
                ),
                (
                    "def invoke(values, target):\n    return list((value) for value in values)\n",
                    vec![(
                        "(value) for value in values",
                        "((value) for value in values)",
                    )],
                    vec![(0, "((value) for value in values)")],
                ),
                (
                    "def invoke(values, target):\n    return ((list))(value for value in values)\n",
                    vec![("value for value in values", "(value for value in values)")],
                    vec![(0, "(value for value in values)")],
                ),
                (
                    "def invoke(values, target):\n    return list(  # argument delimiter\n        élément for élément in values  # last token\n    )\n",
                    vec![(
                        "élément for élément in values",
                        "(  # argument delimiter\n        élément for élément in values  # last token\n    )",
                    )],
                    vec![(
                        0,
                        "(  # argument delimiter\n        élément for élément in values  # last token\n    )",
                    )],
                ),
                (
                    "def invoke(groups, target):\n    return list(tuple(value for value in values) for values in groups)\n",
                    vec![
                        (
                            "tuple(value for value in values) for values in groups",
                            "(tuple(value for value in values) for values in groups)",
                        ),
                        ("value for value in values", "(value for value in values)"),
                    ],
                    vec![
                        (0, "(tuple(value for value in values) for values in groups)"),
                        (0, "(value for value in values)"),
                    ],
                ),
                (
                    "def invoke(values, target):\n    return target(*(value for value in values))\n",
                    vec![("(value for value in values)", "(value for value in values)")],
                    vec![(1, "*(value for value in values)")],
                ),
                (
                    "def invoke(values, target):\n    return target(items=(value for value in values))\n",
                    vec![("(value for value in values)", "(value for value in values)")],
                    vec![],
                ),
                (
                    "def invoke(values, target):\n    return target((value for value in values), 97)\n",
                    vec![("(value for value in values)", "(value for value in values)")],
                    vec![(0, "(value for value in values)"), (0, "97")],
                ),
                (
                    "async def invoke(values, target):\n    return list(value async for value in values)\n",
                    vec![(
                        "value async for value in values",
                        "(value async for value in values)",
                    )],
                    vec![(0, "(value async for value in values)")],
                ),
            ];
            for (body, generators, arguments) in cases {
                let source = format!("from __future__ import strict\n{body}");
                let fixture = Fixture::new(py, &source);
                let (root, bindings) = fixture.compile(py);
                let map = fixture.decode(py, &root, &bindings).unwrap();
                let actual: BTreeSet<_> = map
                    .codes
                    .iter()
                    .filter(|code| code.role() == InterpreterCodeRole::Comprehension)
                    .map(|code| {
                        let original = code.expression_range().unwrap();
                        let native = code.native_range().unwrap();
                        assert_eq!(code.source().lexical_qualname, "invoke");
                        (original.start, original.end, native.start, native.end)
                    })
                    .collect();
                let expected: BTreeSet<_> = generators
                    .into_iter()
                    .map(|(original, native)| {
                        let original = generator_excerpt_range(&source, original);
                        let native = generator_excerpt_range(&source, native);
                        (original.start, original.end, native.start, native.end)
                    })
                    .collect();
                assert_eq!(
                    actual, expected,
                    "exact original/native generator correspondence"
                );
                let actual_arguments: BTreeSet<_> = map
                    .codes
                    .iter()
                    .flat_map(|code| code.calls.values())
                    .filter(|call| call.origin.role == InterpreterCallRole::SourceExpression)
                    .flat_map(|call| &call.input.positional_entries)
                    .map(|entry| {
                        let kind = match entry.kind {
                            InterpreterPositionalEntryKind::Source => 0,
                            InterpreterPositionalEntryKind::Star => 1,
                            InterpreterPositionalEntryKind::GenericBaseInjected => {
                                panic!("a source expression has no injected class base")
                            }
                        };
                        let range = entry.source_range.unwrap();
                        (kind, range.start, range.end)
                    })
                    .collect();
                let expected_arguments: BTreeSet<_> = arguments
                    .into_iter()
                    .map(|(kind, excerpt)| {
                        let range = generator_excerpt_range(&source, excerpt);
                        (kind, range.start, range.end)
                    })
                    .collect();
                assert_eq!(
                    actual_arguments, expected_arguments,
                    "actual original CALL input ranges"
                );
            }
        });
    }

    #[test]
    fn interpreter_source_generator_code_ranges_refuse_interior_and_outer_call_envelopes() {
        let _guard = native_lock();
        Python::attach(|py| {
            for (expression, bad_excerpts) in [
                (
                    "list(value for value in values)",
                    vec![
                        "value for value in values",
                        "list(value for value in values)",
                    ],
                ),
                (
                    "list((value for value in values))",
                    vec![
                        "value for value in values",
                        "((value for value in values))",
                        "list((value for value in values))",
                    ],
                ),
            ] {
                let source = format!(
                    "from __future__ import strict\ndef invoke(values):\n    return {expression}\n"
                );
                let fixture = Fixture::new(py, &source);
                let (root, bindings) = fixture.compile(py);
                let map = fixture.decode(py, &root, &bindings).unwrap();
                let generator = map
                    .codes
                    .iter()
                    .find(|code| code.role() == InterpreterCodeRole::Comprehension)
                    .unwrap();
                let packet = tuple(bindings);
                let nodes = tuple(packet.get_item(1).unwrap());
                let node = tuple(nodes.get_item(generator.ordinal() as usize).unwrap());
                for excerpt in bad_excerpts {
                    let changed_node =
                        replace(&node, 5, generator_excerpt_wire_span(py, &source, excerpt));
                    let changed = replace(
                        &packet,
                        1,
                        replace(
                            &nodes,
                            generator.ordinal() as usize,
                            changed_node.into_any(),
                        )
                        .into_any(),
                    );
                    assert!(
                        fixture.decode(py, &root, &changed.into_any()).is_err(),
                        "only the grammar-owned native delimiter range may match"
                    );
                }
            }
        });
    }

    #[test]
    fn interpreter_call_generator_input_ranges_refuse_interior_and_outer_call_envelopes() {
        let _guard = native_lock();
        Python::attach(|py| {
            for (expression, bad_excerpts) in [
                (
                    "list(value for value in values)",
                    vec![
                        "value for value in values",
                        "list(value for value in values)",
                    ],
                ),
                (
                    "list((value for value in values))",
                    vec![
                        "value for value in values",
                        "((value for value in values))",
                        "list((value for value in values))",
                    ],
                ),
            ] {
                let source = format!(
                    "from __future__ import strict\ndef invoke(values):\n    return {expression}\n"
                );
                let fixture = Fixture::new(py, &source);
                let (root, bindings) = fixture.compile(py);
                let map = fixture.decode(py, &root, &bindings).unwrap();
                let invoke = one(&map, InterpreterCodeRole::SourceFunction, "invoke");
                let packet = tuple(bindings);
                let tables = tuple(packet.get_item(3).unwrap());
                let table = tuple(tables.get_item(invoke.ordinal() as usize).unwrap());
                let index = call_row_index(&table, 0);
                let row = tuple(tuple(table.get_item(5).unwrap()).get_item(index).unwrap());
                let emissions = tuple(row.get_item(1).unwrap());
                assert_eq!(emissions.len(), 1);
                let emission = tuple(emissions.get_item(0).unwrap());
                let input = tuple(emission.get_item(4).unwrap());
                let positional = tuple(input.get_item(2).unwrap());
                let entries = tuple(positional.get_item(1).unwrap());
                assert_eq!(entries.len(), 1);
                let entry = tuple(entries.get_item(0).unwrap());
                for excerpt in bad_excerpts {
                    let bad_entry =
                        replace(&entry, 1, generator_excerpt_wire_span(py, &source, excerpt));
                    let bad_entries = replace(&entries, 0, bad_entry.into_any());
                    let bad_positional = replace(&positional, 1, bad_entries.into_any());
                    let bad_input = replace(&input, 2, bad_positional.into_any());
                    let bad_emission = replace(&emission, 4, bad_input.into_any());
                    let bad_row = replace(
                        &row,
                        1,
                        replace(&emissions, 0, bad_emission.into_any()).into_any(),
                    );
                    let changed = change_call_row(
                        &packet,
                        invoke.ordinal() as usize,
                        index,
                        bad_row.into_any(),
                    );
                    assert!(
                        fixture.decode(py, &root, &changed.into_any()).is_err(),
                        "a valid code row cannot authorize a mismatched CALL input span"
                    );
                }
            }
        });
    }

    // Original sources from the 13 unrepresented retained-plan witnesses.
    // Native compiler data only: never an executable SOAC lifecycle recipe.
    const CLASS_SCOPE_SOURCES: &[(&str, &str)] = &[
        (
            "missing_access",
            r#"from __future__ import strict
def factory(value):
    class C:
        seen = value
        items = [item for item in (1,)]
    return C
"#,
        ),
        (
            "class_cell_method",
            r#"from __future__ import strict
class C:
    callbacks = [lambda: __class__ for __class__ in (7,)]
    def method(self):
        return __class__
"#,
        ),
        (
            "equal_name_cell_free",
            r#"from __future__ import strict
def factory(value, enabled):
    class C:
        result = [([lambda: value for value in (7,)] if enabled else None, value) for unused in (0,)]
    return C
"#,
        ),
        (
            "finally_region",
            r#"from __future__ import strict
class C:
    try:
        checkpoint()
    finally:
        callbacks = [lambda: item for item in (1,)]
"#,
        ),
        (
            "namespace_and_free",
            r#"from __future__ import strict
def factory(value):
    class C:
        value = 'namespace'
        seen = value
        callbacks = [lambda: value for unused in (0,)]
        del value
    return C
"#,
        ),
        (
            "raw_iterable_prefix",
            r#"from __future__ import strict
class C:
    result = sink(prefix(), [lambda: item for item in source()], later())
"#,
        ),
        (
            "unreachable_region",
            r#"from __future__ import strict
class C:
    raise ValueError('before region')
    ignored = [lambda: item for item in (1,)]
"#,
        ),
        (
            "compile_only_unicode",
            r#"from __future__ import strict
def outer(value):
    class Café:
        readers = [lambda: value for value in (1,)]
    return Café
raise AssertionError('the fixture must only compile source')
"#,
        ),
        (
            "nearest_class_owner",
            r#"from __future__ import strict
def build(marker):
    class Box:
        values = [lambda: __class__ for __class__ in (marker,)]
        def read(self):
            return __class__
    return Box
"#,
        ),
        (
            "list_kind",
            r#"from __future__ import strict
class C:
    result = [item for item in source()]
"#,
        ),
        (
            "set_kind",
            r#"from __future__ import strict
class C:
    result = {item for item in source()}
"#,
        ),
        (
            "dict_kind",
            r#"from __future__ import strict
class C:
    result = {item: item for item in source()}
"#,
        ),
        (
            "export_with_method",
            r#"from __future__ import strict
def build(marker):
    class Box:
        callbacks = [lambda: __class__ for __class__ in (marker,)]
        def method(self):
            return __class__
    return Box
"#,
        ),
        (
            "export_without_method",
            r#"from __future__ import strict
def build(marker):
    class Box:
        callbacks = [lambda: __class__ for __class__ in (marker,)]
    return Box
"#,
        ),
        (
            "classdict_provider",
            r#"from __future__ import strict
def build(marker):
    class Box:
        values = [lambda: __classdict__ for __classdict__ in (marker,)]
        field: int
    return Box
"#,
        ),
        (
            "conditional_provider",
            r#"from __future__ import strict
def build(marker, condition):
    class Box:
        values = [lambda: __conditional_annotations__ for __conditional_annotations__ in (marker,)]
        if condition:
            field: int
    return Box
"#,
        ),
        (
            "lambda_names",
            r#"from __future__ import strict
module_list = [lambda: module_index for module_index in range(2)]
module_set = {lambda: set_index for set_index in range(2)}
module_dict = {dict_index: lambda: dict_index for dict_index in range(2)}
module_generator = (lambda: generator_index for generator_index in range(2))
generator_input = (item for item in (lambda: range(2))())
nested = lambda: (lambda: "nested")
class ModuleClass:
    values = [lambda: module_class_index for module_class_index in range(2)]
def factory():
    local_list = [lambda: local_index for local_index in range(2)]
    class Owner:
        values = [lambda: class_index for class_index in range(2)]
        generated = (lambda: class_generator for class_generator in range(2))
        nested = lambda: (lambda: "class_nested")
    return Owner
"#,
        ),
    ];

    fn with_class_scope_source<'py>(
        py: Python<'py>,
        name: &str,
        check: impl FnOnce(&str, &StrictInterpreterSource, &Bound<'py, PyTuple>),
    ) {
        let source = CLASS_SCOPE_SOURCES
            .iter()
            .find_map(|(candidate, source)| (*candidate == name).then_some(*source))
            .expect("original class scope source");
        let fixture = Fixture::new(py, source);
        let (root, bindings) = fixture.compile(py);
        let native = fixture
            .decode(py, &root, &bindings)
            .unwrap_or_else(|error| panic!("{name}: native source catalogue: {error}"));
        // Both actual owners stay in scope throughout the inspection. No body
        // is executed and no native region becomes a retained executable plan.
        check(source, &native, &tuple(bindings));
    }

    fn native_packet_row<'py>(
        packet: &Bound<'py, PyTuple>,
        group: usize,
        ordinal: u32,
    ) -> Bound<'py, PyTuple> {
        tuple(
            tuple(packet.get_item(group).unwrap())
                .get_item(ordinal as usize)
                .unwrap(),
        )
    }

    fn native_u32(row: &Bound<'_, PyTuple>, index: usize) -> u32 {
        unsigned(row.get_item(index).unwrap()).unwrap()
    }

    #[test]
    fn interpreter_class_scope_original_trees_roles_and_qualnames_are_native_data() {
        let _guard = native_lock();
        Python::attach(|py| {
            for (name, _) in CLASS_SCOPE_SOURCES {
                with_class_scope_source(py, name, |source, native, packet| {
                    let classes = native
                        .codes
                        .iter()
                        .filter(|code| code.role() == InterpreterCodeRole::ClassNamespace)
                        .map(|code| code.source().lexical_qualname.as_str())
                        .collect::<BTreeSet<_>>();
                    let expected: BTreeSet<_> = match *name {
                        "missing_access" | "equal_name_cell_free" | "namespace_and_free" => {
                            ["factory.<locals>.C"].into_iter().collect()
                        }
                        "export_with_method"
                        | "export_without_method"
                        | "classdict_provider"
                        | "conditional_provider"
                        | "nearest_class_owner" => ["build.<locals>.Box"].into_iter().collect(),
                        "compile_only_unicode" => ["outer.<locals>.Café"].into_iter().collect(),
                        "lambda_names" => ["ModuleClass", "factory.<locals>.Owner"]
                            .into_iter()
                            .collect(),
                        _ => ["C"].into_iter().collect(),
                    };
                    assert_eq!(classes, expected, "{name}");
                    let nodes = tuple(packet.get_item(1).unwrap());
                    assert_eq!(nodes.len(), native.codes.len(), "{name}");
                    assert_ne!(native.source_id(), 0);
                    assert_eq!(native.codes[0].role(), InterpreterCodeRole::Module);
                    assert_eq!(native.codes[0].parent_ordinal(), None);
                    for code in &native.codes {
                        let row = native_packet_row(packet, 1, code.ordinal());
                        let actual = row.get_item(2).unwrap();
                        assert_eq!(native.code(py, &actual).unwrap().ordinal(), code.ordinal());
                        assert_eq!(
                            InterpreterCodeRole::from_wire(
                                native_u32(&row, 3),
                                native_u32(&row, 4)
                            ),
                            Some(code.role()),
                            "{name}"
                        );
                        if let Some(parent) = code.parent_ordinal() {
                            assert!(parent < code.ordinal(), "{name}");
                        }
                    }
                    if *name == "compile_only_unicode" {
                        let class = one(
                            native,
                            InterpreterCodeRole::ClassNamespace,
                            "outer.<locals>.Café",
                        );
                        let range = class.source().source_range;
                        assert!(
                            source[range.start as usize..range.end as usize]
                                .starts_with("class Café:")
                        );
                        let actual = native_packet_row(packet, 1, class.ordinal())
                            .get_item(2)
                            .unwrap();
                        assert_eq!(
                            actual
                                .getattr("co_name")
                                .unwrap()
                                .extract::<String>()
                                .unwrap(),
                            "Café"
                        );
                        // SOURCE still ends in its original raising sentinel.
                        // Fixture::compile returning proves it was not executed.
                    }
                });
            }

            with_class_scope_source(py, "lambda_names", |source, native, packet| {
                for (expression, lexical, qualname) in [
                    ("lambda: module_index", "<lambda>", "<lambda>"),
                    ("lambda: set_index", "<lambda>", "<lambda>"),
                    ("lambda: dict_index", "<lambda>", "<lambda>"),
                    ("lambda: generator_index", "<lambda>", "<genexpr>.<lambda>"),
                    ("lambda: range(2)", "<lambda>", "<lambda>"),
                    (
                        "lambda: \"nested\"",
                        "<lambda>.<lambda>",
                        "<lambda>.<locals>.<lambda>",
                    ),
                    (
                        "lambda: module_class_index",
                        "ModuleClass.<lambda>",
                        "ModuleClass.<lambda>",
                    ),
                    (
                        "lambda: local_index",
                        "factory.<locals>.<lambda>",
                        "factory.<locals>.<lambda>",
                    ),
                    (
                        "lambda: class_index",
                        "factory.<locals>.Owner.<lambda>",
                        "factory.<locals>.Owner.<lambda>",
                    ),
                    (
                        "lambda: class_generator",
                        "factory.<locals>.Owner.<lambda>",
                        "factory.<locals>.Owner.<genexpr>.<lambda>",
                    ),
                    (
                        "lambda: \"class_nested\"",
                        "factory.<locals>.Owner.<lambda>.<lambda>",
                        "factory.<locals>.Owner.<lambda>.<locals>.<lambda>",
                    ),
                ] {
                    let start = source.find(expression).unwrap() as u32;
                    let wanted = SourceRange::new(start, start + expression.len() as u32);
                    let matching = native
                        .codes
                        .iter()
                        .filter(|code| {
                            code.role() == InterpreterCodeRole::Lambda
                                && code.source().source_range == wanted
                        })
                        .collect::<Vec<_>>();
                    let [code] = matching.as_slice() else {
                        panic!("one actual original lambda at {wanted:?}")
                    };
                    assert_eq!(code.source().lexical_qualname, lexical);
                    let actual = native_packet_row(packet, 1, code.ordinal())
                        .get_item(2)
                        .unwrap();
                    assert_eq!(
                        actual
                            .getattr("co_qualname")
                            .unwrap()
                            .extract::<String>()
                            .unwrap(),
                        qualname
                    );
                    assert_eq!(
                        actual
                            .getattr("co_name")
                            .unwrap()
                            .extract::<String>()
                            .unwrap(),
                        "<lambda>"
                    );
                }
            });
        });
    }

    #[test]
    fn interpreter_class_scope_current_slots_accesses_and_exports_are_exact() {
        let _guard = native_lock();
        Python::attach(|py| {
            for (name, _) in CLASS_SCOPE_SOURCES {
                with_class_scope_source(py, name, |source, native, packet| {
                    let decoder = Decoder::new(source);
                    for class in native
                        .codes
                        .iter()
                        .filter(|code| code.role() == InterpreterCodeRole::ClassNamespace)
                    {
                        let scope = native_packet_row(packet, 2, class.ordinal());
                        let regions = tuple(scope.get_item(3).unwrap());
                        let captures = tuple(scope.get_item(4).unwrap());
                        let owners = tuple(scope.get_item(2).unwrap());
                        let actions = tuple(scope.get_item(6).unwrap());
                        let mut header_roles = BTreeMap::new();
                        for header in tuple(actions.get_item(0).unwrap()).iter() {
                            let header = tuple(header);
                            let owner =
                                tuple(owners.get_item(native_u32(&header, 0) as usize).unwrap());
                            assert_eq!(native_u32(&owner, 1), 0);
                            assert!(owner.get_item(4).unwrap().is_none());
                            assert!(matches!(native_u32(&header, 1), 3 | 4));
                            header_roles.insert(native_u32(&owner, 2), native_u32(&header, 1));
                        }
                        let mut exports = BTreeMap::new();
                        for export in tuple(actions.get_item(1).unwrap()).iter() {
                            let export = tuple(export);
                            let slot =
                                capture_current_slot(py, export.get_item(1).unwrap()).unwrap();
                            assert_eq!(
                                class.layout.locals[slot as usize].kind & (CELL | FREE),
                                CELL
                            );
                            assert!(exports.insert(native_u32(&export, 0), slot).is_none());
                        }
                        let mut edges = Vec::new();
                        let mut provider_cells = Vec::new();
                        for capture in captures.iter() {
                            let capture = tuple(capture);
                            let child = &native.codes[native_u32(&capture, 0) as usize];
                            assert_eq!(child.parent_ordinal(), Some(class.ordinal()), "{name}");
                            let free = child
                                .layout
                                .free_variables()
                                .find(|(ordinal, _, _)| *ordinal == native_u32(&capture, 2))
                                .expect("actual child FREE ordinal");
                            let slot =
                                capture_current_slot(py, capture.get_item(3).unwrap()).unwrap();
                            let current = &class.layout.locals[slot as usize];
                            assert_ne!(current.kind & (CELL | FREE), 0, "{name}");
                            assert_eq!(current.name, free.2, "{name}");
                            let region = optional_unsigned(capture.get_item(4).unwrap()).unwrap();
                            if let Some(region) = region {
                                assert!((region as usize) < regions.len(), "{name}");
                            }
                            let creation = decoder
                                .range(capture.get_item(1).unwrap())
                                .unwrap()
                                .unwrap();
                            if child.role() == InterpreterCodeRole::AnnotationProvider {
                                let header_role = header_roles.get(&slot).copied();
                                let reused = regions.iter().map(tuple).any(|region| {
                                    tuple(region.get_item(6).unwrap())
                                        .iter()
                                        .map(tuple)
                                        .any(|operation| native_u32(&operation, 1) == slot)
                                });
                                let context = format!(
                                    "{name}: class={} child={} free={} name={:?} slot={} kind={:#04x} region={region:?} header={header_role:?} reused={reused} exports={exports:?}",
                                    class.ordinal(),
                                    child.ordinal(),
                                    free.0,
                                    free.2,
                                    slot,
                                    current.kind,
                                );
                                assert_eq!(child.source(), class.source(), "{context}");
                                assert!(region.is_none(), "{context}");
                                assert_eq!(current.kind & (CELL | FREE), CELL, "{context}");
                                assert_eq!(creation.start, creation.end, "{context}");
                                assert!(
                                    creation.start < class.source().source_range.start,
                                    "{context}"
                                );
                                let actual = native_packet_row(packet, 1, child.ordinal())
                                    .get_item(2)
                                    .unwrap();
                                let class_actual = native_packet_row(packet, 1, class.ordinal())
                                    .get_item(2)
                                    .unwrap();
                                assert_eq!(
                                    actual
                                        .getattr("co_firstlineno")
                                        .unwrap()
                                        .extract::<u32>()
                                        .unwrap(),
                                    class_actual
                                        .getattr("co_firstlineno")
                                        .unwrap()
                                        .extract::<u32>()
                                        .unwrap(),
                                    "{context}"
                                );
                                let expression =
                                    child.native_range().expect("actual provider expression");
                                assert_eq!(
                                    &source[expression.start as usize..expression.end as usize],
                                    "int",
                                    "{context}"
                                );
                                // A provider FREE edge is not a class-header grant.
                                // These original sources also capture a region-reused
                                // CELL: one has no header role; one has role4 but is
                                // still not a proved, unchanged conditional-set owner.
                                let expected = match (header_role, reused) {
                                    (Some(3), false) => {
                                        assert_eq!(exports.get(&1), Some(&slot), "{context}");
                                        InterpreterAnnotationCaptureOrigin::ClassDictionary {
                                            class_ordinal: class.ordinal(),
                                            class_definition: class.source().clone(),
                                            class_slot: slot,
                                        }
                                    }
                                    (None, true) | (Some(4), true) => {
                                        InterpreterAnnotationCaptureOrigin::Unresolved(
                                            InterpreterAnnotationCaptureUnresolved::ReusedCarrier,
                                        )
                                    }
                                    _ => panic!("unexpected original provider edge: {context}"),
                                };
                                assert_eq!(
                                    native.annotation_capture(py, &actual, free.0).unwrap(),
                                    &expected,
                                    "{context}"
                                );
                                provider_cells.push((header_role, reused));
                            } else {
                                assert_eq!(creation, child.source().source_range, "{name}");
                            }
                            edges.push((child.role(), slot, region));
                        }
                        if *name == "classdict_provider" {
                            assert!(
                                header_roles.is_empty() && exports.is_empty(),
                                "{name}: {header_roles:?} {exports:?}"
                            );
                            assert_eq!(provider_cells, [(None, true)], "{name}");
                        }
                        if *name == "conditional_provider" {
                            assert_eq!(header_roles.len(), 2, "{name}: {header_roles:?}");
                            assert_eq!(exports.len(), 1, "{name}: {exports:?}");
                            assert_eq!(
                                provider_cells,
                                [(Some(3), false), (Some(4), true)],
                                "{name}"
                            );
                        }
                        if matches!(
                            *name,
                            "class_cell_method" | "nearest_class_owner" | "export_with_method"
                        ) {
                            let lambda = edges
                                .iter()
                                .find(|(role, _, _)| *role == InterpreterCodeRole::Lambda)
                                .unwrap();
                            let method = edges
                                .iter()
                                .find(|(role, _, _)| *role == InterpreterCodeRole::SourceFunction)
                                .unwrap();
                            assert_eq!(lambda.1, method.1);
                            assert!(lambda.2.is_some() && method.2.is_none());
                            assert_eq!(exports.get(&0), Some(&method.1));
                        }
                        if *name == "export_without_method" {
                            assert_eq!(edges.len(), 1);
                            assert!(edges[0].2.is_some());
                            assert!(!exports.contains_key(&0));
                        }
                        let accesses = tuple(scope.get_item(5).unwrap());
                        if *name == "missing_access" {
                            let selections = accesses
                                .iter()
                                .map(|access| native_u32(&tuple(access), 2))
                                .collect::<BTreeSet<_>>();
                            assert!(selections.contains(&0), "actual regional raw LOCAL access");
                            assert!(selections.contains(&2), "actual namespace-or-FREE access");
                        }
                        if *name == "equal_name_cell_free" {
                            let start = source.find(", value) for unused").unwrap() as u32 + 2;
                            let wanted = SourceRange::new(start, start + 5);
                            let matching = accesses
                                .iter()
                                .map(tuple)
                                .filter(|access| {
                                    decoder.range(access.get_item(0).unwrap()).unwrap()
                                        == Some(wanted)
                                })
                                .collect::<Vec<_>>();
                            let [access] = matching.as_slice() else {
                                panic!("one actual post-conditional Name access")
                            };
                            assert_eq!(native_u32(access, 1), 0);
                            assert_eq!(native_u32(access, 2), 1);
                            let slot =
                                capture_current_slot(py, access.get_item(3).unwrap()).unwrap();
                            assert_eq!(class.layout.locals[slot as usize].kind & FREE, FREE);
                            let equal_names = class
                                .layout
                                .locals
                                .iter()
                                .filter(|slot| slot.name == "value")
                                .collect::<Vec<_>>();
                            assert_eq!(equal_names.len(), 2);
                            assert!(equal_names.iter().any(|slot| slot.kind & CELL != 0));
                        }
                        if *name == "namespace_and_free" {
                            assert_eq!(edges.len(), 1);
                            assert_eq!(class.layout.locals[edges[0].1 as usize].kind & FREE, FREE);
                            let load = source.find("seen = value").unwrap() as u32 + 7;
                            let load = SourceRange::new(load, load + 5);
                            assert!(
                                !accesses.iter().map(tuple).any(|access| {
                                    decoder.range(access.get_item(0).unwrap()).unwrap()
                                        == Some(load)
                                }),
                                "namespace read must not be reclassified as a lexical slot read"
                            );
                            let stores = tuple(
                                native_packet_row(packet, 3, class.ordinal())
                                    .get_item(4)
                                    .unwrap(),
                            );
                            for (text, offset, phase, form) in
                                [("value = 'namespace'", 0, 0, 4), ("del value", 4, 1, 11)]
                            {
                                let start = source.find(text).unwrap() as u32 + offset;
                                let wanted = SourceRange::new(start, start + 5);
                                let matching = stores
                                    .iter()
                                    .map(tuple)
                                    .filter(|store| {
                                        let origin = tuple(store.get_item(0).unwrap());
                                        native_u32(&origin, 0) == 0
                                            && native_u32(&origin, 2) == phase
                                            && decoder.range(origin.get_item(1).unwrap()).unwrap()
                                                == Some(wanted)
                                    })
                                    .collect::<Vec<_>>();
                                let [store] = matching.as_slice() else {
                                    panic!("one actual namespace binding/delete origin")
                                };
                                let emissions = tuple(store.get_item(1).unwrap());
                                assert_eq!(emissions.len(), 1);
                                let emission = tuple(emissions.get_item(0).unwrap());
                                assert_eq!(native_u32(&emission, 1), form);
                                let operand = tuple(emission.get_item(2).unwrap());
                                assert_eq!(native_u32(&operand, 0), 1, "native names domain");
                                assert_eq!(
                                    class.native_name(native_u32(&operand, 1)),
                                    Some("value")
                                );
                            }
                        }
                    }
                });
            }
        });
    }

    #[test]
    fn interpreter_class_scope_regions_keep_semantic_bindings_including_unreachable_source() {
        let _guard = native_lock();
        Python::attach(|py| {
            for (name, _) in CLASS_SCOPE_SOURCES {
                with_class_scope_source(py, name, |source, native, packet| {
                    let decoder = Decoder::new(source);
                    for class in native
                        .codes
                        .iter()
                        .filter(|code| code.role() == InterpreterCodeRole::ClassNamespace)
                    {
                        let scope = native_packet_row(packet, 2, class.ordinal());
                        assert_eq!(scope.len(), 7);
                        let owners = tuple(scope.get_item(2).unwrap());
                        let regions = tuple(scope.get_item(3).unwrap());
                        assert!(
                            !regions.is_empty(),
                            "{name}: original regional source retained"
                        );
                        for (index, region) in regions.iter().map(tuple).enumerate() {
                            assert_eq!(region.len(), 8);
                            assert_eq!(native_u32(&region, 0), index as u32);
                            if let Some(parent) =
                                optional_unsigned(region.get_item(1).unwrap()).unwrap()
                            {
                                assert!((parent as usize) < index);
                            }
                            let range =
                                decoder.range(region.get_item(3).unwrap()).unwrap().unwrap();
                            let outer =
                                decoder.range(region.get_item(4).unwrap()).unwrap().unwrap();
                            assert!(range.start <= outer.start && outer.end <= range.end);
                            for operation in tuple(region.get_item(6).unwrap()).iter().map(tuple) {
                                let slot = native_u32(&operation, 1);
                                let owner = tuple(
                                    owners.get_item(native_u32(&operation, 2) as usize).unwrap(),
                                );
                                assert!(matches!(
                                    (native_u32(&operation, 0), native_u32(&owner, 1)),
                                    (0, 2) | (1, 1)
                                ));
                                assert_eq!(native_u32(&owner, 2), slot);
                                assert_eq!(
                                    native_u32(&owner, 3),
                                    u32::from(class.layout.locals[slot as usize].kind)
                                );
                                assert_eq!(
                                    optional_unsigned(owner.get_item(4).unwrap()).unwrap(),
                                    Some(index as u32)
                                );
                            }
                            for binding in tuple(region.get_item(7).unwrap()).iter().map(tuple) {
                                let origin = tuple(binding.get_item(2).unwrap());
                                let original =
                                    decoder.range(origin.get_item(1).unwrap()).unwrap().unwrap();
                                assert!(range.start <= original.start && original.end <= range.end);
                            }
                            if matches!(*name, "list_kind" | "set_kind" | "dict_kind") {
                                let expected = match *name {
                                    "list_kind" => 0,
                                    "set_kind" => 1,
                                    _ => 2,
                                };
                                assert_eq!(native_u32(&region, 2), expected);
                            }
                            if *name == "raw_iterable_prefix" {
                                let operations = native_packet_row(packet, 3, class.ordinal());
                                assert_eq!(operations.len(), 7);
                                let calls = tuple(operations.get_item(5).unwrap());
                                assert!(
                                    calls.iter().map(tuple).any(|call| {
                                        let origin = tuple(call.get_item(0).unwrap());
                                        native_u32(&origin, 0) == 0
                                            && decoder.range(origin.get_item(1).unwrap()).unwrap()
                                                == Some(outer)
                                    }),
                                    "the original outer iterable CALL remains authenticated source data"
                                );
                            }
                        }
                    }
                });
            }
        });
    }
}
