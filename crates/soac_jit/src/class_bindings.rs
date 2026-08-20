//! Decode compiler-owned class recipes while the exact native root is pinned.
//!
//! The wire packet is never an admission token. Code identities are checked
//! against this compilation's final constant tree before any pointer-free
//! metadata is passed to lowering. The same root subsequently enters the
//! authenticated catalog; no Python object is hidden in the lowering sidecar.
//!
//! Native wire v7 retains one code-tree carrier for class bindings and
//! original-function lexical regions. It contains no comprehension lifecycle
//! or scalar reference schedule; no decoded row grants executable authority.

use std::collections::HashSet;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyTuple};
use soac_contracts::SourceRange;
use soac_core::block_py::{
    CLASS_BINDINGS_SCHEMA_VERSION, ClassBindingAccess, ClassBindingAccessContext,
    ClassBindingAccessSelection, ClassBindingCapture, ClassBindingCaptureCreation,
    ClassBindingCodeNode, ClassBindingExport, ClassBindingExportKind, ClassBindingInitialValue,
    ClassBindingInitializer, ClassBindingPhase, ClassBindingRecipe, ClassBindingSlotId,
    NativeCodeId, NativeCompileScopeKind, NativeLocalsPlusKind, NativeLocalsPlusSlot,
    NativeSymbolScopeKind,
};

use crate::strict_runtime_unavailable;

pub(crate) struct NativeCodeNode<'py> {
    pub(crate) code: Bound<'py, PyAny>,
    pub(crate) parent: Option<NativeCodeId>,
}

/// Final `co_consts` preorder, shared by the decoder and catalog matcher.
pub(crate) fn code_tree<'py>(
    py: Python<'py>,
    root: &Bound<'py, PyAny>,
) -> PyResult<Vec<NativeCodeNode<'py>>> {
    let source_id = unsafe { crate::code_view::view(py, root.as_ptr())? }.strict_source_id;
    if source_id == 0 {
        return Err(strict_runtime_unavailable(
            py,
            "native class code root is unauthenticated",
        ));
    }
    let mut pending = vec![(root.clone(), None)];
    let mut seen = HashSet::new();
    let mut nodes = Vec::new();
    while let Some((code, parent)) = pending.pop() {
        if unsafe { ffi::Py_TYPE(code.as_ptr()) } != std::ptr::addr_of_mut!(ffi::PyCode_Type)
            || !seen.insert(code.as_ptr() as usize)
        {
            return Err(strict_runtime_unavailable(
                py,
                "native code tree has a repeated or invalid node",
            ));
        }
        let view = unsafe { crate::code_view::view(py, code.as_ptr())? };
        if view.strict_source_id != source_id {
            return Err(strict_runtime_unavailable(
                py,
                "native code tree contains a foreign source",
            ));
        }
        let constants = exact_tuple(unsafe { Bound::from_borrowed_ptr(py, view.consts) }, None)?;
        let id = NativeCodeId(u32::try_from(nodes.len()).map_err(|_| {
            strict_runtime_unavailable(py, "native code tree exceeds ordinal capacity")
        })?);
        for index in (0..constants.len()).rev() {
            let value = constants.get_item(index)?;
            if unsafe { ffi::Py_TYPE(value.as_ptr()) } == std::ptr::addr_of_mut!(ffi::PyCode_Type) {
                pending.push((value, Some(id)));
            }
        }
        nodes.push(NativeCodeNode { code, parent });
    }
    Ok(nodes)
}

fn exact_tuple(value: Bound<'_, PyAny>, length: Option<usize>) -> PyResult<Bound<'_, PyTuple>> {
    if !value.is_exact_instance_of::<PyTuple>() {
        return Err(strict_runtime_unavailable(
            value.py(),
            "native class metadata requires exact tuples",
        ));
    }
    let tuple = value.cast_into::<PyTuple>()?;
    if length.is_some_and(|length| tuple.len() != length) {
        return Err(strict_runtime_unavailable(
            tuple.py(),
            "native class metadata tuple has the wrong length",
        ));
    }
    Ok(tuple)
}

fn unsigned(value: Bound<'_, PyAny>) -> PyResult<u32> {
    if unsafe { ffi::PyLong_CheckExact(value.as_ptr()) } == 0 {
        return Err(strict_runtime_unavailable(
            value.py(),
            "native class metadata requires exact integer ordinals",
        ));
    }
    value.extract().map_err(|_| {
        strict_runtime_unavailable(value.py(), "native class metadata ordinal is out of range")
    })
}

fn optional_unsigned(value: Bound<'_, PyAny>) -> PyResult<Option<u32>> {
    if value.is_none() {
        Ok(None)
    } else {
        unsigned(value).map(Some)
    }
}

fn tag<T>(value: Bound<'_, PyAny>, decode: fn(u32) -> Option<T>) -> PyResult<T> {
    decode(unsigned(value.clone())?)
        .ok_or_else(|| strict_runtime_unavailable(value.py(), "unknown native class metadata tag"))
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
                        .filter_map(|(i, byte)| (byte == b'\n').then_some(i + 1)),
                )
                .collect(),
        }
    }

    fn offset(&self, py: Python<'_>, line: u32, column: u32) -> PyResult<u32> {
        let index = line.checked_sub(1).ok_or_else(|| {
            strict_runtime_unavailable(py, "native class source line is not one-based")
        })? as usize;
        let start = *self.line_starts.get(index).ok_or_else(|| {
            strict_runtime_unavailable(py, "native class source line is outside the module")
        })?;
        let end = self
            .line_starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.source.len());
        let offset = start
            .checked_add(column as usize)
            .filter(|offset| *offset <= end && self.source.is_char_boundary(*offset))
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "native class source column is invalid")
            })?;
        u32::try_from(offset)
            .map_err(|_| strict_runtime_unavailable(py, "native class source range is too large"))
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
            return Err(strict_runtime_unavailable(
                row.py(),
                "native class source range is reversed",
            ));
        }
        Ok(Some(SourceRange::new(start, end)))
    }

    fn node(
        &self,
        row: Bound<'_, PyAny>,
        index: usize,
        expected: &NativeCodeNode<'_>,
    ) -> PyResult<ClassBindingCodeNode> {
        let row = exact_tuple(row, Some(6))?;
        let id = NativeCodeId(unsigned(row.get_item(0)?)?);
        let parent = optional_unsigned(row.get_item(1)?)?.map(NativeCodeId);
        if id.0 as usize != index
            || parent != expected.parent
            || row.get_item(2)?.as_ptr() != expected.code.as_ptr()
        {
            return Err(strict_runtime_unavailable(
                row.py(),
                "native class metadata differs from its owned code tree",
            ));
        }
        let view = unsafe { crate::code_view::view(row.py(), expected.code.as_ptr())? };
        if view.firstlineno < 1
            || view.nlocalsplus < 0
            || view.nfreevars < 0
            || view.nfreevars > view.nlocalsplus
            || unsafe { ffi::PyTuple_CheckExact(view.localsplusnames) } == 0
            || unsafe { ffi::PyBytes_CheckExact(view.localspluskinds) } == 0
        {
            return Err(strict_runtime_unavailable(
                row.py(),
                "invalid original class localsplus metadata",
            ));
        }
        let names = exact_tuple(
            unsafe { Bound::from_borrowed_ptr(row.py(), view.localsplusnames) },
            Some(view.nlocalsplus as usize),
        )?;
        let kinds = unsafe { Bound::<PyAny>::from_borrowed_ptr(row.py(), view.localspluskinds) }
            .cast_into::<PyBytes>()?;
        if kinds.as_bytes().len() != names.len() {
            return Err(strict_runtime_unavailable(
                row.py(),
                "native localsplus kind and name counts differ",
            ));
        }
        let mut slots = Vec::with_capacity(names.len());
        for (name, kind) in names.iter().zip(kinds.as_bytes()) {
            if unsafe { ffi::PyUnicode_CheckExact(name.as_ptr()) } == 0 {
                return Err(strict_runtime_unavailable(
                    row.py(),
                    "native localsplus name is not an exact string",
                ));
            }
            slots.push(NativeLocalsPlusSlot {
                name: name.extract()?,
                kind: NativeLocalsPlusKind(*kind),
            });
        }
        Ok(ClassBindingCodeNode {
            id,
            parent,
            compile_scope: tag(row.get_item(3)?, NativeCompileScopeKind::from_wire)?,
            symbol_scope: tag(row.get_item(4)?, NativeSymbolScopeKind::from_wire)?,
            first_line: view.firstlineno as u32,
            source_range: self.range(row.get_item(5)?)?,
            slots,
            freevar_count: view.nfreevars as u32,
        })
    }

    /// Each original code node carries semantic seeds and lexical bindings.
    /// No prefix, operation or lifetime schedule is part of the v7 envelope.
    fn scope_envelope<'py>(
        &self,
        row: Bound<'py, PyAny>,
        index: usize,
        node: &ClassBindingCodeNode,
        native: &NativeCodeNode<'_>,
    ) -> PyResult<Bound<'py, PyTuple>> {
        let row = exact_tuple(row, Some(7))?;
        let py = row.py();
        if unsigned(row.get_item(0)?)? as usize != index {
            return Err(strict_runtime_unavailable(
                py,
                "scope recipes are not dense native code ordinals",
            ));
        }
        for field in [1, 2, 3, 4, 5] {
            exact_tuple(row.get_item(field)?, None)?;
        }
        let class = node.compile_scope == NativeCompileScopeKind::Class;
        let actions = row.get_item(6)?;
        if class {
            let actions = exact_tuple(actions, Some(2))?;
            exact_tuple(actions.get_item(0)?, None)?;
            exact_tuple(actions.get_item(1)?, None)?;
        } else if !actions.is_none() {
            return Err(strict_runtime_unavailable(
                py,
                "non-class scope contains class actions",
            ));
        }
        let header = unsafe { crate::code_view::view(py, native.code.as_ptr())? };
        let parameters = usize::try_from(header.argcount)
            .ok()
            .zip(usize::try_from(header.kwonlyargcount).ok())
            .and_then(|(positional, keyword)| positional.checked_add(keyword))
            .and_then(|count| count.checked_add(usize::from(header.flags & ffi::CO_VARARGS != 0)))
            .and_then(|count| {
                count.checked_add(usize::from(header.flags & ffi::CO_VARKEYWORDS != 0))
            })
            .filter(|count| *count <= node.slots.len())
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "invalid native successful-bind parameter count")
            })?;
        if header.posonlyargcount < 0
            || header.posonlyargcount > header.argcount
            || (class && parameters != 0)
        {
            return Err(strict_runtime_unavailable(
                py,
                "class scope has a non-class parameter layout",
            ));
        }
        let seeds = exact_tuple(row.get_item(1)?, Some(node.slots.len()))?;
        for (slot, seed) in seeds.iter().enumerate() {
            let seed = exact_tuple(seed, Some(4))?;
            let parameter = slot < parameters;
            if unsigned(seed.get_item(0)?)? as usize != slot
                || unsigned(seed.get_item(1)?)? != u32::from(node.slots[slot].kind.0)
                || unsigned(seed.get_item(2)?)? != u32::from(parameter)
                || optional_unsigned(seed.get_item(3)?)? != parameter.then_some(slot as u32)
            {
                return Err(strict_runtime_unavailable(
                    py,
                    "scope seed differs from the actual native slot/binder",
                ));
            }
        }
        for owner in exact_tuple(row.get_item(2)?, None)?.iter() {
            exact_tuple(owner, Some(5))?;
        }
        for region in exact_tuple(row.get_item(3)?, None)?.iter() {
            exact_tuple(region, Some(8))?;
        }
        for field in [4, 5] {
            for child in exact_tuple(row.get_item(field)?, None)?.iter() {
                exact_tuple(child, Some(5))?;
            }
        }
        Ok(row)
    }

    fn class_initializers(
        &self,
        py: Python<'_>,
        node: &ClassBindingCodeNode,
        native: &NativeCodeNode<'_>,
        owners: Bound<'_, PyTuple>,
        header_actions: Bound<'_, PyTuple>,
        captures: &[ClassBindingCapture],
        exports: &[ClassBindingExport],
        accesses: &[ClassBindingAccess],
    ) -> PyResult<Vec<ClassBindingInitializer>> {
        use std::collections::{BTreeSet, HashMap};
        let slot_id = |index| ClassBindingSlotId {
            class_code: node.id,
            index,
        };
        // These native IDs resolve header stores. Regional save/cell owners are
        // not SOAC storage obligations; ordinary comprehension helpers own them.
        let mut entry_owners = HashMap::new();
        let mut entry_slots = BTreeSet::new();
        for (index, owner) in owners.iter().enumerate() {
            let owner = exact_tuple(owner, Some(5))?;
            if unsigned(owner.get_item(0)?)? as usize != index {
                return Err(strict_runtime_unavailable(
                    py,
                    "class owner identity is not canonical",
                ));
            }
            let kind = unsigned(owner.get_item(1)?)?;
            if kind != 0 {
                if !matches!(kind, 1 | 2) {
                    return Err(strict_runtime_unavailable(
                        py,
                        "unknown native class owner role",
                    ));
                }
                continue;
            }
            let slot = unsigned(owner.get_item(2)?)?;
            let native_kind = unsigned(owner.get_item(3)?)?;
            if !owner.get_item(4)?.is_none()
                || node
                    .slots
                    .get(slot as usize)
                    .is_none_or(|row| u32::from(row.kind.0) != native_kind)
                || !entry_slots.insert(slot_id(slot))
            {
                return Err(strict_runtime_unavailable(
                    py,
                    "class entry owner differs from actual lexical storage",
                ));
            }
            entry_owners.insert(index as u32, slot_id(slot));
        }
        let header = unsafe { crate::code_view::view(py, native.code.as_ptr())? };
        if header.flags & (ffi::CO_GENERATOR | ffi::CO_COROUTINE | ffi::CO_ASYNC_GENERATOR) != 0 {
            return Err(strict_runtime_unavailable(
                py,
                "class body has a suspended native function kind",
            ));
        }
        let first_free = node
            .slots
            .len()
            .checked_sub(node.freevar_count as usize)
            .ok_or_else(|| {
                strict_runtime_unavailable(py, "class free-variable count exceeds actual slots")
            })?;
        let mut selected = (first_free..node.slots.len())
            .map(|index| slot_id(index as u32))
            .collect::<BTreeSet<_>>();
        selected.extend(captures.iter().map(|capture| capture.source));
        selected.extend(exports.iter().map(|export| export.source));
        selected.extend(accesses.iter().map(|access| access.source));
        let mut stores = Vec::new();
        for action in header_actions.iter() {
            let action = exact_tuple(action, Some(3))?;
            let kind = unsigned(action.get_item(1)?)?;
            if !matches!(kind, 3 | 4) || !action.get_item(2)?.is_none() {
                return Err(strict_runtime_unavailable(
                    py,
                    "class header action is not a native namespace/conditional store",
                ));
            }
            let slot = *entry_owners
                .get(&unsigned(action.get_item(0)?)?)
                .ok_or_else(|| {
                    strict_runtime_unavailable(py, "class header store lacks its actual entry cell")
                })?;
            selected.insert(slot);
            stores.push(ClassBindingInitializer {
                phase: ClassBindingPhase::ClassHeaderComplete,
                slot,
                value: ClassBindingInitialValue::from_wire(kind, None)
                    .expect("checked header role"),
            });
        }
        let mut initializers = Vec::with_capacity(selected.len() + stores.len());
        for slot in selected {
            let kind = node
                .slots
                .get(slot.index as usize)
                .ok_or_else(|| strict_runtime_unavailable(py, "class semantic cell is absent"))?
                .kind;
            if slot.class_code != node.id || !entry_slots.contains(&slot) || !kind.is_cell() {
                return Err(strict_runtime_unavailable(
                    py,
                    "class semantic binding lacks an actual entry cell",
                ));
            }
            let value = if slot.index as usize >= first_free {
                if kind != NativeLocalsPlusKind::FREE {
                    return Err(strict_runtime_unavailable(
                        py,
                        "class FREE cells do not form the actual closure suffix",
                    ));
                }
                ClassBindingInitialValue::IncomingFree {
                    ordinal: slot.index - first_free as u32,
                }
            } else {
                if kind.is_free() {
                    return Err(strict_runtime_unavailable(
                        py,
                        "class FREE cell appears outside its closure suffix",
                    ));
                }
                ClassBindingInitialValue::EmptyCell
            };
            initializers.push(ClassBindingInitializer {
                phase: ClassBindingPhase::ClassEntry,
                slot,
                value,
            });
        }
        initializers.extend(stores);
        Ok(initializers)
    }

    fn recipe(
        &self,
        row: Bound<'_, PyTuple>,
        nodes: &[ClassBindingCodeNode],
        native: &NativeCodeNode<'_>,
    ) -> PyResult<ClassBindingRecipe> {
        let py = row.py();
        let class_code = NativeCodeId(unsigned(row.get_item(0)?)?);
        let node = nodes.get(class_code.0 as usize).ok_or_else(|| {
            strict_runtime_unavailable(py, "class recipe refers to an absent native node")
        })?;
        let slot_id = |index| ClassBindingSlotId { class_code, index };
        let current_slot = |value| -> PyResult<ClassBindingSlotId> {
            let row = exact_tuple(value, Some(2))?;
            if unsigned(row.get_item(0)?)? != 0 {
                return Err(strict_runtime_unavailable(
                    py,
                    "class binding requires an actual lexical cell reference",
                ));
            }
            Ok(slot_id(unsigned(row.get_item(1)?)?))
        };
        // Keep the native packet complete for interpreter construction/source
        // authority. Only the containing class's lexical operations become
        // compiler bindings; eager bodies are lowered through ordinary helpers.
        let mut captures = Vec::new();
        for capture in exact_tuple(row.get_item(4)?, None)?.iter() {
            let capture = exact_tuple(capture, Some(5))?;
            if optional_unsigned(capture.get_item(4)?)?.is_some() {
                continue;
            }
            let child = NativeCodeId(unsigned(capture.get_item(0)?)?);
            let child_node = nodes.get(child.0 as usize).ok_or_else(|| {
                strict_runtime_unavailable(py, "class capture refers to an absent native child")
            })?;
            let creation = ClassBindingCaptureCreation::from_native_marker(
                self.source,
                node,
                child_node,
                self.range(capture.get_item(1)?)?,
            )
            .map_err(|error| strict_runtime_unavailable(py, &error))?;
            captures.push(ClassBindingCapture {
                child,
                creation,
                freevar_ordinal: unsigned(capture.get_item(2)?)?,
                source: current_slot(capture.get_item(3)?)?,
            });
        }
        let actions = exact_tuple(row.get_item(6)?, Some(2))?;
        let exports = exact_tuple(actions.get_item(1)?, None)?
            .iter()
            .map(|export| {
                let export = exact_tuple(export, Some(2))?;
                Ok(ClassBindingExport {
                    kind: tag(export.get_item(0)?, ClassBindingExportKind::from_wire)?,
                    source: current_slot(export.get_item(1)?)?,
                })
            })
            .collect::<PyResult<Vec<_>>>()?;
        let mut accesses = Vec::new();
        for access in exact_tuple(row.get_item(5)?, None)?.iter() {
            let access = exact_tuple(access, Some(5))?;
            if optional_unsigned(access.get_item(4)?)?.is_some() {
                continue;
            }
            accesses.push(ClassBindingAccess {
                source_range: self.range(access.get_item(0)?)?.ok_or_else(|| {
                    strict_runtime_unavailable(py, "native class Name access has no source range")
                })?,
                context: tag(access.get_item(1)?, ClassBindingAccessContext::from_wire)?,
                selection: tag(access.get_item(2)?, ClassBindingAccessSelection::from_wire)?,
                source: current_slot(access.get_item(3)?)?,
            });
        }
        let initializers = self.class_initializers(
            py,
            node,
            native,
            exact_tuple(row.get_item(2)?, None)?,
            exact_tuple(actions.get_item(0)?, None)?,
            &captures,
            &exports,
            &accesses,
        )?;
        Ok(ClassBindingRecipe {
            class_code,
            initializers,
            captures,
            exports,
            accesses,
        })
    }
}

pub(crate) fn decode<'py>(
    py: Python<'py>,
    source: &str,
    native_root: &Bound<'py, PyAny>,
    packet: Bound<'py, PyAny>,
) -> PyResult<soac_lowering::CanonicalClassBindings> {
    let packet = exact_tuple(packet, Some(4))?;
    if unsigned(packet.get_item(0)?)? != CLASS_BINDINGS_SCHEMA_VERSION {
        return Err(strict_runtime_unavailable(
            py,
            "unsupported native class binding schema",
        ));
    }
    let native_nodes = code_tree(py, native_root)?;
    let decoder = Decoder::new(source);
    let node_rows = exact_tuple(packet.get_item(1)?, Some(native_nodes.len()))?;
    let nodes = node_rows
        .iter()
        .zip(&native_nodes)
        .enumerate()
        .map(|(index, (row, expected))| decoder.node(row, index, expected))
        .collect::<PyResult<Vec<_>>>()?;
    let scope_rows = exact_tuple(packet.get_item(2)?, Some(nodes.len()))?;
    let mut recipes = Vec::new();
    for (index, (node, native)) in nodes.iter().zip(&native_nodes).enumerate() {
        let scope = decoder.scope_envelope(scope_rows.get_item(index)?, index, node, native)?;
        if node.compile_scope == NativeCompileScopeKind::Class {
            recipes.push(decoder.recipe(scope, &nodes, native)?);
        }
    }
    soac_lowering::CanonicalClassBindings::from_native_entries(source, nodes, recipes).map_err(
        |error| {
            strict_runtime_unavailable(py, &format!("invalid native class bindings: {error:#}"))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::{PyDict, PyString};

    const CLASS_BINDING_SOURCE: &str = "from __future__ import strict\n\
        def build(outside, condition):\n\
        \x20   class Box:\n\
        \x20       field: int\n\
        \x20       if condition:\n\
        \x20           conditional: str\n\
        \x20       def method(self):\n\
        \x20           return outside, __class__\n\
        \x20   return Box\n";

    /// The real private Details producer is the success oracle. These unit
    /// fixtures do not execute the subject or construct a runtime catalog.
    fn native_class_wire<'py>(
        py: Python<'py>,
        source: &str,
    ) -> (Bound<'py, PyAny>, Bound<'py, PyAny>) {
        unsafe extern "C" {
            fn PySoac_CompileVerifiedSourceDetails(
                source: *const std::ffi::c_char,
                length: ffi::Py_ssize_t,
                filename: *mut ffi::PyObject,
                optimize: std::ffi::c_int,
            ) -> *mut ffi::PyObject;
        }
        let filename = PyString::new(py, "<class-binding-wire-test>");
        let details = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                PySoac_CompileVerifiedSourceDetails(
                    source.as_ptr().cast(),
                    source.len() as ffi::Py_ssize_t,
                    filename.as_ptr(),
                    0,
                ),
            )
            .unwrap()
        };
        let root = details.get_item(0).unwrap();
        let packet = details.get_item(2).unwrap();
        assert_eq!(packet.cast::<PyTuple>().unwrap().len(), 4);
        assert_eq!(
            unsigned(packet.get_item(0).unwrap()).unwrap(),
            CLASS_BINDINGS_SCHEMA_VERSION
        );
        (root, packet)
    }

    fn replace_tuple<'py>(
        value: &Bound<'py, PyAny>,
        index: usize,
        replacement: Bound<'py, PyAny>,
    ) -> Bound<'py, PyAny> {
        let mut items = value.cast::<PyTuple>().unwrap().iter().collect::<Vec<_>>();
        items[index] = replacement;
        PyTuple::new(value.py(), items).unwrap().into_any()
    }

    fn replace_scope_field<'py>(
        packet: &Bound<'py, PyAny>,
        code: NativeCodeId,
        field: usize,
        replacement: Bound<'py, PyAny>,
    ) -> Bound<'py, PyAny> {
        let scopes = packet.get_item(2).unwrap();
        let row = scopes.get_item(code.0 as usize).unwrap();
        let row = replace_tuple(&row, field, replacement);
        replace_tuple(packet, 2, replace_tuple(&scopes, code.0 as usize, row))
    }

    fn wire_int(py: Python<'_>, value: u32) -> Bound<'_, PyAny> {
        value.into_pyobject(py).unwrap().into_any()
    }

    /// Reuse the maintained behavioral subjects byte-for-byte. This fixture
    /// only compiles their original code; it does not run the subject, ty,
    /// startup authentication, or an alternative retained body.
    #[test]
    fn native_class_decoder_requires_owned_tree_and_exact_wire_shape() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let source = "from __future__ import strict\nanswer = 42\n";
            let (root, packet) = native_class_wire(py, source);
            let metadata = decode(py, source, &root, packet.clone()).unwrap();
            assert_eq!(metadata.nodes().len(), 1);
            assert_eq!(metadata.nodes()[0].id, NativeCodeId(0));
            assert_eq!(metadata.class_recipes().len(), 0);

            let locals = PyDict::new(py);
            locals.set_item("root", &root).unwrap();
            locals.set_item("packet", &packet).unwrap();
            for expression in [
                c"list(packet)",
                c"(True, *packet[1:])",
                c"(1, *packet[1:])",
                c"(2, *packet[1:])",
                c"(4, *packet[1:])",
                c"(5, *packet[1:])",
                c"(6, *packet[1:])",
                c"(packet[0] + 1, *packet[1:])",
                c"packet[:3]",
                c"(packet[0], ((0, 0, root, 0, 2, None),), *packet[2:])",
                c"(packet[0], ((1, None, root, 0, 2, None),), *packet[2:])",
                c"(packet[0], ((0, None, root.replace(), 0, 2, None),), *packet[2:])",
                c"(packet[0], ((0, None, root, 0, 2, None), (1, 0, root, 0, 2, None)), *packet[2:])",
                c"(packet[0], ((0, None, root, 99, 2, None),), *packet[2:])",
            ] {
                let corrupt = py.eval(expression, None, Some(&locals)).unwrap();
                assert!(
                    decode(py, source, &root, corrupt).is_err(),
                    "accepted {expression:?}"
                );
            }
            let foreign = py
                .eval(c"compile('answer = 42', '<ordinary>', 'exec')", None, None)
                .unwrap();
            assert!(code_tree(py, &foreign).is_err());
        });
    }

    #[test]
    fn native_class_coordinates_use_utf8_bytes_and_preserve_zero_width() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let decoder = Decoder::new("π = 1\n");
            let valid = PyTuple::new(py, [1, 2, 1, 2]).unwrap();
            assert_eq!(
                decoder.range(valid.into_any()).unwrap(),
                Some(SourceRange::new(2, 2))
            );
            for coordinates in [[1, 1, 1, 2], [0, 0, 1, 2], [1, 2, 1, 1], [1, 0, 1, 8]] {
                assert!(
                    decoder
                        .range(PyTuple::new(py, coordinates).unwrap().into_any())
                        .is_err()
                );
            }
        });
    }
    #[test]
    fn native_class_decoder_distinguishes_body_completion_from_source_creation() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            unsafe extern "C" {
                fn PySoac_CompileVerifiedSourceDetails(
                    source: *const std::ffi::c_char,
                    length: ffi::Py_ssize_t,
                    filename: *mut ffi::PyObject,
                    optimize: std::ffi::c_int,
                ) -> *mut ffi::PyObject;
            }
            for decorator in ["", "    @decorate\n"] {
                let source = format!(
                    "from __future__ import strict\n\
                     def build():\n\
                     {decorator}\
                     \x20   class Box:\n\
                     \x20       field: int\n\
                     \x20       def method(self, value: str):\n\
                     \x20           return value\n\
                     \x20   return Box\n",
                );
                let filename = PyString::new(py, "<class-completion-origin>");
                let details = unsafe {
                    Bound::<PyAny>::from_owned_ptr_or_err(
                        py,
                        PySoac_CompileVerifiedSourceDetails(
                            source.as_ptr().cast(),
                            source.len() as ffi::Py_ssize_t,
                            filename.as_ptr(),
                            0,
                        ),
                    )
                    .unwrap()
                };
                let root = details.get_item(0).unwrap();
                let packet = details.get_item(2).unwrap();
                let metadata = decode(py, &source, &root, packet.clone()).unwrap();
                let recipe = metadata.class_recipes().next().unwrap();
                let parent = metadata.node(recipe.class_code).unwrap();
                let capture_index = recipe
                    .captures
                    .iter()
                    .position(|capture| {
                        matches!(
                            &capture.creation,
                            ClassBindingCaptureCreation::ClassAnnotationBodyCompletion { .. }
                        )
                    })
                    .unwrap();
                let capture = &recipe.captures[capture_index];
                let child = metadata.node(capture.child).unwrap();
                let marker = parent.first_line_marker(&source).unwrap();
                assert_eq!(
                    capture.creation,
                    ClassBindingCaptureCreation::ClassAnnotationBodyCompletion { marker },
                );
                assert_eq!(capture.creation.source_range(), None);
                assert_eq!(
                    child.source_range.unwrap().start,
                    source.find("int\n").unwrap() as u32
                );
                assert!(marker.start < parent.source_range.unwrap().start);
                assert!(
                    recipe.captures.iter().any(|capture| {
                        matches!(
                            &capture.creation,
                            ClassBindingCaptureCreation::SourceRange(_)
                        ) && metadata.node(capture.child).unwrap().compile_scope
                            == NativeCompileScopeKind::Annotations
                    }),
                    "method provider retains its real FunctionDef creation range"
                );

                // A different zero-column line may lie inside the ClassDef,
                // but it is not the native body-completion marker.
                let wrong_line = parent.first_line + 1;
                let wrong_marker = PyTuple::new(py, [wrong_line, 0, wrong_line, 0])
                    .unwrap()
                    .into_any();
                let recipes = packet.get_item(2).unwrap();
                let recipe_index = recipe.class_code.0 as usize;
                let original_recipe = recipes.get_item(recipe_index).unwrap();
                let captures = original_recipe.get_item(4).unwrap();
                let bad_capture =
                    replace_tuple(&captures.get_item(capture_index).unwrap(), 1, wrong_marker);
                let bad_captures = replace_tuple(&captures, capture_index, bad_capture);
                let bad_recipe = replace_tuple(&original_recipe, 4, bad_captures);
                let bad_recipes = replace_tuple(&recipes, recipe_index, bad_recipe);
                let bad_packet = replace_tuple(&packet, 2, bad_recipes);
                assert!(decode(py, &source, &root, bad_packet).is_err());
            }
        });
    }

    #[test]
    fn native_class_decoder_projects_slots_and_header_without_prefix_receipts() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (root, packet) = native_class_wire(py, CLASS_BINDING_SOURCE);
            // The class projection is independent of the operation-origin
            // table consumed by the separate interpreter-admission path.
            let packet = replace_tuple(&packet, 3, py.None().into_bound(py));
            let metadata = decode(py, CLASS_BINDING_SOURCE, &root, packet.clone()).unwrap();
            assert_eq!(metadata.class_recipes().len(), 1);
            let recipe = metadata.class_recipes().next().unwrap();
            let node = metadata.node(recipe.class_code).unwrap();
            let scope = packet
                .get_item(2)
                .unwrap()
                .get_item(node.id.0 as usize)
                .unwrap();
            assert_eq!(scope.cast::<PyTuple>().unwrap().len(), 7);
            let seeds = scope.get_item(1).unwrap().cast_into::<PyTuple>().unwrap();
            assert_eq!(seeds.len(), node.slots.len());
            for (index, seed) in seeds.iter().enumerate() {
                assert_eq!(unsigned(seed.get_item(0).unwrap()).unwrap(), index as u32);
                assert_eq!(
                    unsigned(seed.get_item(1).unwrap()).unwrap(),
                    u32::from(node.slots[index].kind.0)
                );
                assert_eq!(unsigned(seed.get_item(2).unwrap()).unwrap(), 0);
                assert!(seed.get_item(3).unwrap().is_none());
            }
            assert!(node.freevar_count > 0);
            let first_free = node.slots.len() - node.freevar_count as usize;
            let entry = recipe
                .initializers
                .iter()
                .filter(|init| init.phase == ClassBindingPhase::ClassEntry)
                .collect::<Vec<_>>();
            assert_eq!(
                entry.len(),
                node.slots.iter().filter(|slot| slot.kind.is_cell()).count()
            );
            for (slot, native) in node
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.kind.is_cell())
            {
                let initializer = entry
                    .iter()
                    .find(|init| init.slot.index as usize == slot)
                    .unwrap();
                let expected = if native.kind.is_free() {
                    assert_eq!(native.kind, NativeLocalsPlusKind::FREE);
                    ClassBindingInitialValue::IncomingFree {
                        ordinal: (slot - first_free) as u32,
                    }
                } else {
                    ClassBindingInitialValue::EmptyCell
                };
                assert_eq!(initializer.value, expected);
            }
            assert!(
                entry
                    .iter()
                    .any(|init| init.value == ClassBindingInitialValue::EmptyCell)
            );
            for value in [
                ClassBindingInitialValue::NamespaceStore,
                ClassBindingInitialValue::ConditionalSetStore,
            ] {
                assert!(recipe.initializers.iter().any(|init| {
                    init.phase == ClassBindingPhase::ClassHeaderComplete && init.value == value
                }));
            }
            for kind in [
                ClassBindingExportKind::ClassCell,
                ClassBindingExportKind::ClassDictCell,
            ] {
                assert!(recipe.exports.iter().any(|row| row.kind == kind));
            }
            assert!(recipe.captures.iter().any(|row| {
                metadata.node(row.child).unwrap().compile_scope == NativeCompileScopeKind::Function
            }));
        });
    }

    #[test]
    fn native_class_decoder_rejects_changed_entry_slots_and_binder_seeds() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (root, packet) = native_class_wire(py, CLASS_BINDING_SOURCE);
            let metadata = decode(py, CLASS_BINDING_SOURCE, &root, packet.clone()).unwrap();
            let class = metadata.class_recipes().next().unwrap().class_code;
            let scope = packet
                .get_item(2)
                .unwrap()
                .get_item(class.0 as usize)
                .unwrap();
            let seeds = scope.get_item(1).unwrap();
            let first_seed = seeds.get_item(0).unwrap();
            let parameter_seed = replace_tuple(
                &replace_tuple(&first_seed, 2, wire_int(py, 1)),
                3,
                wire_int(py, 0),
            );
            let owners = scope.get_item(2).unwrap();
            let owner = owners.get_item(0).unwrap();
            let mut corrupt = vec![
                replace_scope_field(&packet, class, 1, PyTuple::empty(py).into_any()),
                replace_scope_field(&packet, class, 1, replace_tuple(&seeds, 0, parameter_seed)),
                replace_scope_field(&packet, class, 2, PyTuple::empty(py).into_any()),
            ];
            for (field, value) in [
                (0, wire_int(py, 1)),
                (1, wire_int(py, 0)),
                (3, wire_int(py, 0)),
            ] {
                corrupt.push(replace_scope_field(
                    &packet,
                    class,
                    1,
                    replace_tuple(&seeds, 0, replace_tuple(&first_seed, field, value)),
                ));
            }
            for (field, value) in [(0, 1), (1, 1), (2, u32::MAX), (3, 0), (4, 0)] {
                corrupt.push(replace_scope_field(
                    &packet,
                    class,
                    2,
                    replace_tuple(
                        &owners,
                        0,
                        replace_tuple(&owner, field, wire_int(py, value)),
                    ),
                ));
            }
            for (index, packet) in corrupt.into_iter().enumerate() {
                assert!(
                    decode(py, CLASS_BINDING_SOURCE, &root, packet).is_err(),
                    "accepted changed class entry binding {index}"
                );
            }
        });
    }

    #[test]
    fn native_class_decoder_requires_dense_scope_and_semantic_capture_header_joins() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let (root, packet) = native_class_wire(py, CLASS_BINDING_SOURCE);
            let metadata = decode(py, CLASS_BINDING_SOURCE, &root, packet.clone()).unwrap();
            let class = metadata.class_recipes().next().unwrap().class_code;
            let scopes = packet.get_item(2).unwrap();
            let scope = scopes.get_item(class.0 as usize).unwrap();
            let header = scope.get_item(6).unwrap();
            let truncated = PyTuple::new(py, scope.cast::<PyTuple>().unwrap().iter().take(6))
                .unwrap()
                .into_any();
            let mut corrupt = vec![
                replace_tuple(&packet, 2, PyTuple::empty(py).into_any()),
                replace_scope_field(&packet, class, 0, wire_int(py, 0)),
                replace_scope_field(&packet, class, 6, py.None().into_bound(py)),
                replace_scope_field(&packet, NativeCodeId(0), 6, header.clone()),
                replace_tuple(
                    &packet,
                    2,
                    replace_tuple(&scopes, class.0 as usize, truncated),
                ),
            ];
            let captures = scope.get_item(4).unwrap();
            assert!(!captures.cast::<PyTuple>().unwrap().is_empty());
            let capture = captures.get_item(0).unwrap();
            for field in [0, 4] {
                corrupt.push(replace_scope_field(
                    &packet,
                    class,
                    4,
                    replace_tuple(
                        &captures,
                        0,
                        replace_tuple(&capture, field, wire_int(py, 0)),
                    ),
                ));
            }
            let actions = header.get_item(0).unwrap();
            let action = actions.get_item(0).unwrap();
            let wrong_header = replace_tuple(
                &header,
                0,
                replace_tuple(&actions, 0, replace_tuple(&action, 1, wire_int(py, 1))),
            );
            corrupt.push(replace_scope_field(&packet, class, 6, wrong_header));
            let exports = header.get_item(1).unwrap();
            let export = exports.get_item(0).unwrap();
            let current = export.get_item(1).unwrap();
            let wrong_export = replace_tuple(
                &export,
                1,
                replace_tuple(&current, 1, wire_int(py, u32::MAX)),
            );
            corrupt.push(replace_scope_field(
                &packet,
                class,
                6,
                replace_tuple(&header, 1, replace_tuple(&exports, 0, wrong_export)),
            ));
            for (index, packet) in corrupt.into_iter().enumerate() {
                assert!(
                    decode(py, CLASS_BINDING_SOURCE, &root, packet).is_err(),
                    "accepted changed semantic class join {index}"
                );
            }
        });
    }

    #[test]
    fn native_class_decoder_keeps_nonclass_regions_out_of_class_projection() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let source = "from __future__ import strict\n\
                def collect(items):\n\
                \x20   return [item for item in items]\n\
                def suspended(value):\n\
                \x20   yield value\n\
                class Plain:\n\
                \x20   pass\n";
            let (root, packet) = native_class_wire(py, source);
            let metadata = decode(py, source, &root, packet.clone()).unwrap();
            let scopes = packet.get_item(2).unwrap().cast_into::<PyTuple>().unwrap();
            assert_eq!(scopes.len(), metadata.nodes().len());
            assert!(
                scopes.iter().any(|scope| {
                    !scope
                        .get_item(3)
                        .unwrap()
                        .cast::<PyTuple>()
                        .unwrap()
                        .is_empty()
                        && scope.get_item(6).unwrap().is_none()
                }),
                "real non-class comprehension must be present"
            );
            let native = code_tree(py, &root).unwrap();
            assert!(
                native.iter().enumerate().any(|(index, native)| {
                    let view = unsafe { crate::code_view::view(py, native.code.as_ptr()).unwrap() };
                    view.flags & ffi::CO_GENERATOR != 0
                        && scopes
                            .get_item(index)
                            .unwrap()
                            .get_item(6)
                            .unwrap()
                            .is_none()
                }),
                "real non-class generator remains independent of class projection"
            );
            assert_eq!(metadata.class_recipes().len(), 1);
            let class = metadata.class_recipes().next().unwrap();
            assert!(class.initializers.is_empty());
            assert!(class.captures.is_empty());
        });
    }

    #[test]
    fn native_class_decoder_keeps_eager_iteration_out_of_class_cell_bindings() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            for source in [
                "from __future__ import strict\nclass Box:\n    values = [item for item in (1, 2)]\n",
                "from __future__ import strict\nclass Box:\n    values = [lambda: item for item in (1, 2)]\n",
                "from __future__ import strict\nclass Box:\n    values = {key: value for key, value in ((1, 2), (3, 4))}\n",
                "from __future__ import strict\nclass Box:\n    values = [[lambda: (outer, inner) for inner in (1, 2)] for outer in (3, 4)]\n",
            ] {
                let (root, packet) = native_class_wire(py, source);
                let scopes = packet.get_item(2).unwrap().cast_into::<PyTuple>().unwrap();
                assert!(
                    scopes.iter().any(|scope| {
                        !scope.get_item(6).unwrap().is_none()
                            && !scope
                                .get_item(3)
                                .unwrap()
                                .cast::<PyTuple>()
                                .unwrap()
                                .is_empty()
                    }),
                    "the actual native compiler inlined an eager class comprehension"
                );
                let metadata = decode(py, source, &root, packet).unwrap();
                let class = metadata.class_recipes().next().unwrap();
                assert!(
                    class.initializers.is_empty(),
                    "iteration locals/cells are not class construction cells"
                );
                assert!(
                    class.captures.is_empty(),
                    "body lambdas capture helper cells, not class storage"
                );
                assert!(class.accesses.is_empty());
            }
        });
    }

    #[test]
    fn native_class_decoder_accepts_wide_free_binding_ordinals() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            // Closure ordinals are not bounded by an opcode's operand width.
            let names = (0..260)
                .map(|index| format!("outside{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let source = format!(
                "from __future__ import strict\n\
                 def build({names}):\n\
                 \x20   class Box:\n\
                 \x20       def method(self):\n\
                 \x20           return ({names})\n\
                 \x20   return Box\n"
            );
            let (root, packet) = native_class_wire(py, &source);
            let metadata = decode(py, &source, &root, packet.clone()).unwrap();
            let recipe = metadata.class_recipes().next().unwrap();
            let node = metadata.node(recipe.class_code).unwrap();
            assert_eq!(node.freevar_count, 260);
            let mut ordinals = recipe
                .initializers
                .iter()
                .filter_map(|init| match init.value {
                    ClassBindingInitialValue::IncomingFree { ordinal } => Some(ordinal),
                    _ => None,
                })
                .collect::<Vec<_>>();
            ordinals.sort_unstable();
            assert_eq!(ordinals, (0..260).collect::<Vec<_>>());
            let scope = packet
                .get_item(2)
                .unwrap()
                .get_item(node.id.0 as usize)
                .unwrap();
            let owners = scope.get_item(2).unwrap();
            let last = owners.cast::<PyTuple>().unwrap().len() - 1;
            let wrong_owner =
                replace_tuple(&owners.get_item(last).unwrap(), 2, wire_int(py, u32::MAX));
            let wrong = replace_scope_field(
                &packet,
                node.id,
                2,
                replace_tuple(&owners, last, wrong_owner),
            );
            assert!(decode(py, &source, &root, wrong).is_err());
        });
    }

    #[test]
    fn native_class_decoder_checks_nonclass_binder_seeds_without_class_owners() {
        let _serial = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let source = "from __future__ import strict\n\
                def mixed(first, /, second, *rest, keyword, **extras):\n\
                \x20   return first, second, rest, keyword, extras\n";
            let (root, packet) = native_class_wire(py, source);
            let metadata = decode(py, source, &root, packet.clone()).unwrap();
            assert_eq!(metadata.class_recipes().len(), 0);
            let function = metadata
                .nodes()
                .iter()
                .find(|node| node.compile_scope == NativeCompileScopeKind::Function)
                .unwrap();
            let seeds = packet
                .get_item(2)
                .unwrap()
                .get_item(function.id.0 as usize)
                .unwrap()
                .get_item(1)
                .unwrap();
            assert_eq!(seeds.cast::<PyTuple>().unwrap().len(), 5);
            // Native successful binding orders keyword-only before *args/**kwargs.
            assert_eq!(
                function
                    .slots
                    .iter()
                    .map(|slot| slot.name.as_str())
                    .collect::<Vec<_>>(),
                ["first", "second", "keyword", "rest", "extras"],
            );
            for ordinal in 0..5 {
                let seed = seeds.get_item(ordinal).unwrap();
                assert_eq!(unsigned(seed.get_item(2).unwrap()).unwrap(), 1);
                assert_eq!(
                    unsigned(seed.get_item(3).unwrap()).unwrap() as usize,
                    ordinal
                );
            }
            let seed = seeds.get_item(2).unwrap();
            let bad_seed = replace_tuple(
                &replace_tuple(&seed, 2, wire_int(py, 0)),
                3,
                py.None().into_bound(py),
            );
            let corrupt =
                replace_scope_field(&packet, function.id, 1, replace_tuple(&seeds, 2, bad_seed));
            assert!(decode(py, source, &root, corrupt).is_err());
        });
    }
}
