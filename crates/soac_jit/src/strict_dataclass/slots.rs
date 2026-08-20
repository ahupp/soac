//! A slots replacement is a second construction, not a retargeted original.
//!
//! An independently prepared physical projection is rechecked at the actual
//! opcode bridge. Shared generated members keep their original birth records;
//! only the selected final type receives permanent weak member witnesses.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::ptr;
use std::sync::Arc;

use crate::strict_runtime_unavailable;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};

use super::adoption::{
    ClassPlan, DataclassSlotsConstruction, MemberKind, MemberPlan, Phase as ClassPhase,
};
use super::catalog::{Helper, StructType, dictionary_value, text_is};
use super::edges::{CodeRole, Edge};
use super::generation::{FieldRole, GenerationPlan};
use super::invocation::{Owner, Phase};
use super::native::{self, Frame};
use super::protocol::{self, Role, require};

unsafe extern "C" {
    fn PyType_GetSoacContractOwner(class: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyType_GetName(class: *mut ffi::PyTypeObject) -> *mut ffi::PyObject;
}

/// Only weak type edges are added to the active invocation's traversed owner.
/// The native bridge's actual operands pin both types while constructing them;
/// later completion/failure cleanup must upgrade these witnesses explicitly.
pub(super) struct Replacement {
    pub(super) plan: Arc<ClassPlan>,
    pub(super) original_weak: Cell<Option<usize>>,
    pub(super) replacement_weak: Cell<Option<usize>>,
    /// Set only after the authenticated root returned the native-associated
    /// replacement. The ordinary stdlib cell repair has then completed.
    pub(super) apply_returned: Cell<bool>,
}

#[derive(Clone, Copy)]
enum NamesShape {
    Absent,
    None,
    String,
    Tuple,
    List,
    Dict,
}

struct BaseSlots {
    weak: usize,
    owner: usize,
    shape: NamesShape,
    names: Vec<String>,
    dictionary_offset: ffi::Py_ssize_t,
    weak_offset: ffi::Py_ssize_t,
}

/// Rust-only names and weak actual base identities. Inherited __slots__ is
/// user-visible metadata, not physical-layout authority: the cold projection
/// compares it with the actual native object-field catalogs before binding.
pub(super) struct SlotsLayout {
    bases: Vec<BaseSlots>,
    names: Vec<String>,
}

fn text(value: *mut ffi::PyObject) -> Option<String> {
    if value.is_null() || unsafe { ffi::PyUnicode_CheckExact(value) } == 0 {
        return None;
    }
    let mut result = String::new();
    for index in 0..unsafe { ffi::PyUnicode_GetLength(value) } {
        result.push(char::from_u32(unsafe {
            ffi::PyUnicode_ReadChar(value, index)
        })?);
    }
    Some(result)
}

fn names(value: *mut ffi::PyObject) -> Option<(NamesShape, Vec<String>)> {
    if value.is_null() {
        return Some((NamesShape::Absent, Vec::new()));
    }
    if value == unsafe { ffi::Py_None() } {
        return Some((NamesShape::None, Vec::new()));
    }
    if unsafe { ffi::PyUnicode_CheckExact(value) } != 0 {
        return Some((NamesShape::String, vec![text(value)?]));
    }
    let (shape, count, get): (NamesShape, _, unsafe extern "C" fn(_, _) -> _) =
        if unsafe { ffi::PyTuple_CheckExact(value) } != 0 {
            (
                NamesShape::Tuple,
                unsafe { ffi::PyTuple_Size(value) },
                ffi::PyTuple_GetItem,
            )
        } else if unsafe { ffi::PyList_CheckExact(value) } != 0 {
            (
                NamesShape::List,
                unsafe { ffi::PyList_Size(value) },
                ffi::PyList_GetItem,
            )
        } else if unsafe { ffi::PyDict_CheckExact(value) } != 0 {
            let mut position = 0;
            let mut key = ptr::null_mut();
            let mut item = ptr::null_mut();
            let mut result = Vec::new();
            while unsafe { ffi::PyDict_Next(value, &mut position, &mut key, &mut item) } != 0 {
                result.push(text(key)?);
            }
            return Some((NamesShape::Dict, result));
        } else {
            // No iterator calls or custom __next__/attribute probes.
            return None;
        };
    let mut result = Vec::new();
    for index in 0..count {
        result.push(text(unsafe { get(value, index) })?);
    }
    Some((shape, result))
}

fn names_match(value: *mut ffi::PyObject, shape: NamesShape, names: &[String]) -> bool {
    match shape {
        NamesShape::Absent => value.is_null(),
        NamesShape::None => value == unsafe { ffi::Py_None() },
        NamesShape::String => names.len() == 1 && unsafe { text_is(value, &names[0]) },
        NamesShape::Tuple | NamesShape::List => {
            !value.is_null()
                && match shape {
                    NamesShape::Tuple => (unsafe { ffi::PyTuple_CheckExact(value) }) != 0,
                    _ => (unsafe { ffi::PyList_CheckExact(value) }) != 0,
                }
                && super::operands::text_sequence(value, names.iter().map(String::as_str))
        }
        NamesShape::Dict => {
            if value.is_null()
                || unsafe { ffi::PyDict_CheckExact(value) } == 0
                || unsafe { ffi::PyDict_Size(value) } as usize != names.len()
            {
                return false;
            }
            let mut position = 0;
            let mut key = ptr::null_mut();
            let mut item = ptr::null_mut();
            let mut expected = names.iter();
            while unsafe { ffi::PyDict_Next(value, &mut position, &mut key, &mut item) } != 0 {
                if expected
                    .next()
                    .is_none_or(|name| !unsafe { text_is(key, name) })
                {
                    return false;
                }
            }
            expected.next().is_none()
        }
    }
}

fn type_dictionary(class: *mut ffi::PyObject) -> Option<*mut ffi::PyObject> {
    if class.is_null() || unsafe { ffi::PyType_Check(class) } == 0 {
        return None;
    }
    let dictionary = unsafe { (*class.cast::<ffi::PyTypeObject>()).tp_dict };
    if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
        return None;
    }
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    while unsafe { ffi::PyDict_Next(dictionary, &mut position, &mut key, &mut value) } != 0 {
        if unsafe { ffi::PyUnicode_CheckExact(key) } == 0 {
            return None;
        }
    }
    Some(dictionary)
}

impl SlotsLayout {
    pub(super) fn prepare(
        owner: &Owner<'_>,
        generation: &GenerationPlan,
        namespace: &Bound<'_, PyDict>,
        bases: &Bound<'_, PyTuple>,
    ) -> PyResult<Option<Self>> {
        if unsafe { dictionary_value(namespace.as_ptr(), "__slots__") }.is_some()
            || generation.fields.iter().any(|field| {
                field.role == FieldRole::Instance
                    && matches!(field.name.as_str(), "__dict__" | "__weakref__")
            })
        {
            return Ok(None);
        }
        let py = owner.owner().py();
        let Some(mro) = super::fields::prospective_mro(bases) else {
            return Ok(None);
        };
        let mut inherited = BTreeSet::new();
        let mut physical = BTreeSet::new();
        let mut projected = Vec::new();
        for base in &mro {
            if base.as_ptr() == ptr::addr_of_mut!(ffi::PyBaseObject_Type).cast() {
                continue;
            }
            let Some(state) = crate::strict_class_state::for_actual_type(py, base)? else {
                return Ok(None);
            };
            let Some(dictionary) = type_dictionary(base.as_ptr()) else {
                return Ok(None);
            };
            let value =
                unsafe { dictionary_value(dictionary, "__slots__") }.unwrap_or(ptr::null_mut());
            let Some((shape, base_names)) = names(value) else {
                return Ok(None);
            };
            let raw = base.as_ptr().cast::<ffi::PyTypeObject>();
            let dictionary_offset = unsafe { (*raw).tp_dictoffset };
            let weak_offset = unsafe { (*raw).tp_weaklistoffset };
            inherited.extend(base_names.iter().cloned());
            if matches!(shape, NamesShape::Absent | NamesShape::None) {
                if dictionary_offset != 0 {
                    inherited.insert("__dict__".to_owned());
                }
                if weak_offset != 0 {
                    inherited.insert("__weakref__".to_owned());
                }
            }
            for name in state.object_fields()?.iter() {
                let Some(name) = text(name.as_ptr()) else {
                    return Ok(None);
                };
                physical.insert(name);
            }
            if dictionary_offset != 0 {
                physical.insert("__dict__".to_owned());
            }
            if weak_offset != 0 {
                physical.insert("__weakref__".to_owned());
            }
            let weak = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    ffi::PyWeakref_NewRef(base.as_ptr(), ptr::null_mut()),
                )?
            };
            projected.push(BaseSlots {
                weak: super::produced::add_reference(owner, weak)?,
                owner: state.owner().as_ptr() as usize,
                shape,
                names: base_names,
                dictionary_offset,
                weak_offset,
            });
        }
        if inherited != physical {
            return Ok(None);
        }
        let mut names: Vec<_> = generation
            .fields
            .iter()
            .filter(|field| field.role == FieldRole::Instance && !inherited.contains(&field.name))
            .map(|field| field.name.clone())
            .collect();
        if owner.data().options.weakref_slot && !inherited.contains("__weakref__") {
            names.push("__weakref__".to_owned());
        }
        let result = Self {
            bases: projected,
            names,
        };
        if !result.validate_bases(owner)? {
            return Ok(None);
        }
        Ok(Some(result))
    }

    pub(super) fn matches_input(
        &self,
        owner: &Owner<'_>,
        namespace: &Bound<'_, PyDict>,
    ) -> PyResult<bool> {
        Ok(
            unsafe { dictionary_value(namespace.as_ptr(), "__slots__") }.is_none()
                && self.validate_bases(owner)?,
        )
    }

    fn validate_bases(&self, owner: &Owner<'_>) -> PyResult<bool> {
        for expected in &self.bases {
            let Some(base) = weak_class(owner, expected.weak)? else {
                return Ok(false);
            };
            let Some(dictionary) = type_dictionary(base.as_ptr()) else {
                return Ok(false);
            };
            let native_owner = unsafe { PyType_GetSoacContractOwner(base.as_ptr()) };
            if native_owner.is_null() && unsafe { !ffi::PyErr_Occurred().is_null() } {
                return Err(PyErr::fetch(owner.owner().py()));
            }
            let raw = base.as_ptr().cast::<ffi::PyTypeObject>();
            if native_owner as usize != expected.owner
                || unsafe { (*raw).tp_dictoffset } != expected.dictionary_offset
                || unsafe { (*raw).tp_weaklistoffset } != expected.weak_offset
                || !names_match(
                    unsafe { dictionary_value(dictionary, "__slots__") }.unwrap_or(ptr::null_mut()),
                    expected.shape,
                    &expected.names,
                )
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn validate_mro(&self, owner: &Owner<'_>, class: *mut ffi::PyObject) -> PyResult<bool> {
        let mro = unsafe { (*class.cast::<ffi::PyTypeObject>()).tp_mro };
        if mro.is_null()
            || unsafe { ffi::PyTuple_CheckExact(mro) } == 0
            || unsafe { ffi::PyTuple_Size(mro) } as usize != self.bases.len() + 2
            || unsafe { ffi::PyTuple_GetItem(mro, 0) } != class
            || unsafe { ffi::PyTuple_GetItem(mro, self.bases.len() as ffi::Py_ssize_t + 1) }
                != ptr::addr_of_mut!(ffi::PyBaseObject_Type).cast()
        {
            return Ok(false);
        }
        for (index, expected) in self.bases.iter().enumerate() {
            if weak_class(owner, expected.weak)?.is_none_or(|base| {
                base.as_ptr() != unsafe { ffi::PyTuple_GetItem(mro, index as ffi::Py_ssize_t + 1) }
            }) {
                return Ok(false);
            }
        }
        self.validate_bases(owner)
    }

    fn validate_slots(
        &self,
        owner: &Owner<'_>,
        slots: *mut ffi::PyObject,
        fields: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        let py = owner.owner().py();
        let mut has_doc = false;
        for name in &self.names {
            if let Some(field) = unsafe { dictionary_value(fields, name) } {
                let field = unsafe { Bound::from_borrowed_ptr(py, field) };
                let Some(doc) =
                    owner
                        .data()
                        .catalog
                        .member(py, owner, &field, StructType::Field, "doc")?
                else {
                    return Ok(false);
                };
                has_doc |= !doc.is_none();
            }
        }
        if !has_doc {
            return Ok(names_match(slots, NamesShape::Tuple, &self.names));
        }
        if !names_match(slots, NamesShape::Dict, &self.names) {
            return Ok(false);
        }
        for name in &self.names {
            let expected = if let Some(field) = unsafe { dictionary_value(fields, name) } {
                let field = unsafe { Bound::from_borrowed_ptr(py, field) };
                let Some(doc) =
                    owner
                        .data()
                        .catalog
                        .member(py, owner, &field, StructType::Field, "doc")?
                else {
                    return Ok(false);
                };
                doc
            } else {
                py.None().into_bound(py)
            };
            if unsafe { dictionary_value(slots, name) } != Some(expected.as_ptr()) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn original_namespace<'py>(owner: &Owner<'py>) -> PyResult<Bound<'py, PyDict>> {
    let plan = protocol::plan(owner)?;
    let class = plan.actual_class.get() as *mut ffi::PyObject;
    require(
        owner,
        protocol::matches_class(owner, class)?,
        "slots original class changed",
    )?;
    let dictionary = type_dictionary(class).ok_or_else(|| {
        strict_runtime_unavailable(owner.owner().py(), "slots original namespace changed")
    })?;
    unsafe { Bound::<PyAny>::from_borrowed_ptr(owner.owner().py(), dictionary) }
        .cast_into::<PyDict>()
        .map_err(Into::into)
}

/// Enter after the ordinary binder, before the helper's first opcode. Its
/// fields argument is the exact completed _process_class field dictionary,
/// not a fresh mapping selected by names or by the callback itself.
pub(super) fn enter(owner: &Owner<'_>, parent: Frame<'_>, child: Frame<'_>) -> PyResult<()> {
    let py = owner.owner().py();
    let recipe = owner
        .data()
        .catalog
        .recipe(CodeRole::Helper(Helper::AddSlots));
    let parent_recipe = owner
        .data()
        .catalog
        .recipe(CodeRole::Helper(Helper::ProcessClass));
    let original = child.parameter(py, recipe, "cls")?;
    let namespace = original_namespace(owner)?;
    require(
        owner,
        owner.data().options.slots
            && owner.data().replacement.get().is_none()
            && child.parameter(py, recipe, "is_frozen")? == boolean(owner.data().options.frozen)
            && child.parameter(py, recipe, "weakref_slot")?
                == boolean(owner.data().options.weakref_slot)
            && child.parameter(py, recipe, "defined_fields")?
                == parent.executing(py, parent_recipe, "fields")?
            && unsafe { dictionary_value(namespace.as_ptr(), "__dataclass_fields__") }
                == Some(child.parameter(py, recipe, "defined_fields")?)
            && protocol::validate_completed(
                owner,
                &unsafe { Bound::from_borrowed_ptr(py, original) },
                &namespace,
            )?
            && owner.data().slots_layout.get().is_some(),
        "slots helper options or declaring fields changed",
    )?;
    require(
        owner,
        owner
            .data()
            .slots_layout
            .get()
            .unwrap()
            .validate_mro(owner, original)?,
        "slots inherited metadata differs from its native physical projection",
    )
}

fn boolean(value: bool) -> *mut ffi::PyObject {
    unsafe {
        if value {
            ffi::Py_True()
        } else {
            ffi::Py_False()
        }
    }
}

fn copied_value(
    dictionary: *mut ffi::PyObject,
    expected_key: *mut ffi::PyObject,
) -> Option<*mut ffi::PyObject> {
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    while unsafe { ffi::PyDict_Next(dictionary, &mut position, &mut key, &mut value) } != 0 {
        if key == expected_key {
            return Some(value);
        }
    }
    None
}

/// Pure, repeatable across the native handle/allocation/Ready boundaries.
/// The dict is exactly the original dict's retained operands plus the
/// independently projected __slots__. No equality or attribute hooks run.
pub(super) fn bridge(
    owner: &Owner<'_>,
    parent: Frame<'_>,
    args: &[*mut ffi::PyObject],
) -> PyResult<()> {
    let py = owner.owner().py();
    let recipe = owner
        .data()
        .catalog
        .recipe(CodeRole::Helper(Helper::AddSlots));
    require(
        owner,
        owner.data().phase.get() == Phase::Applying
            && owner.data().options.slots
            && protocol::active_role(owner, parent)? == Role::Slots
            && parent.instruction().and_then(|offset| {
                owner
                    .data()
                    .catalog
                    .edge(CodeRole::Helper(Helper::AddSlots), offset)
            }) == Some(Edge::NewSlots)
            && args.len() == 5
            && args.iter().all(|value| !value.is_null())
            && args[0] == ptr::addr_of_mut!(ffi::PyType_Type).cast()
            && protocol::matches_class(owner, args[4])?
            && args[4] == parent.executing(py, recipe, "cls")?
            && args[3] == parent.executing(py, recipe, "cls_dict")?
            && unsafe { ffi::PyDict_CheckExact(args[3]) } != 0
            && args[2] == unsafe { (*args[4].cast::<ffi::PyTypeObject>()).tp_bases }
            && parent.executing(py, recipe, "is_frozen")? == boolean(owner.data().options.frozen)
            && parent.executing(py, recipe, "weakref_slot")?
                == boolean(owner.data().options.weakref_slot),
        "slots bridge execution or operands changed",
    )?;
    let name =
        unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, PyType_GetName(args[4].cast()))? };
    require(
        owner,
        args[1] == name.as_ptr(),
        "slots replacement name changed",
    )?;
    let namespace = original_namespace(owner)?;
    let fields = unsafe { dictionary_value(namespace.as_ptr(), "__dataclass_fields__") };
    require(
        owner,
        fields == Some(parent.executing(py, recipe, "defined_fields")?)
            && protocol::validate_completed(
                owner,
                &unsafe { Bound::from_borrowed_ptr(py, args[4]) },
                &namespace,
            )?,
        "slots original generated members or field dictionary changed",
    )?;
    let layout = owner
        .data()
        .slots_layout
        .get()
        .ok_or_else(|| strict_runtime_unavailable(py, "slots physical projection is absent"))?;
    require(
        owner,
        layout.validate_mro(owner, args[4])?,
        "slots original ancestry changed",
    )?;
    let new_slots = unsafe { dictionary_value(args[3], "__slots__") }.ok_or_else(|| {
        strict_runtime_unavailable(py, "slots replacement has no slot declaration")
    })?;
    require(
        owner,
        layout.validate_slots(owner, new_slots, fields.unwrap())?,
        "slots declaration or doc operands changed",
    )?;
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    let mut count = 1; // the one new __slots__ entry
    while unsafe { ffi::PyDict_Next(namespace.as_ptr(), &mut position, &mut key, &mut value) } != 0
    {
        if unsafe { text_is(key, "__slots__") } {
            return Err(strict_runtime_unavailable(
                py,
                "slots original acquired a slot declaration",
            ));
        }
        if unsafe { text_is(key, "__dict__") || text_is(key, "__weakref__") }
            || protocol::plan(owner)?
                .generation
                .fields
                .iter()
                .any(|field| {
                    field.role == FieldRole::Instance && unsafe { text_is(key, &field.name) }
                })
        {
            continue;
        }
        require(
            owner,
            copied_value(args[3], key) == Some(value),
            "slots copied namespace operand changed",
        )?;
        count += 1;
    }
    require(
        owner,
        unsafe { ffi::PyDict_Size(args[3]) } == count,
        "slots copied namespace gained another binding",
    )
}

pub(super) fn prepare<'py>(
    owner: &Owner<'py>,
    producer: Frame<'_>,
    args: &[*mut ffi::PyObject; 5],
) -> PyResult<Bound<'py, PyAny>> {
    bridge(owner, producer, args)?;
    require(
        owner,
        owner.data().replacement.get().is_none(),
        "slots replacement preparation was replayed",
    )?;
    let py = owner.owner().py();
    let original = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, args[4]) };
    let original_plan = Arc::clone(owner.data().plan.get().unwrap());
    let mut replacement_plan = ClassPlan::slots_replacement(&original_plan);
    if owner.data().options.frozen {
        for (name, helper) in [
            ("__getstate__", Helper::GetState),
            ("__setstate__", Helper::SetState),
        ] {
            if unsafe { dictionary_value(args[3], name) }.is_none() {
                replacement_plan.members.push(MemberPlan {
                    name: name.to_owned(),
                    kind: MemberKind::Shared(helper),
                });
            }
        }
    }
    let replacement_plan = Arc::new(replacement_plan);
    // No Python allocation occurs before this one-way reservation. The weak
    // type witness may allocate; a reentrant prepare then sees this record.
    owner
        .data()
        .replacement
        .set(Replacement {
            plan: Arc::clone(&replacement_plan),
            original_weak: Cell::new(None),
            replacement_weak: Cell::new(None),
            apply_returned: Cell::new(false),
        })
        .map_err(|_| strict_runtime_unavailable(py, "slots replacement was already reserved"))?;
    let weak = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            ffi::PyWeakref_NewRef(original.as_ptr(), ptr::null_mut()),
        )?
    };
    owner
        .data()
        .replacement
        .get()
        .unwrap()
        .original_weak
        .set(Some(super::produced::add_reference(owner, weak)?));
    bridge(owner, producer, args)?;
    let proof = DataclassSlotsConstruction {
        plan: replacement_plan,
        original: original_plan,
        invocation_owner: owner.owner().clone(),
    };
    let name = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, args[1]) };
    let bases = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, args[2]) }.cast_into::<PyTuple>()?;
    let namespace =
        unsafe { Bound::<PyAny>::from_borrowed_ptr(py, args[3]) }.cast_into::<PyDict>()?;
    let handle = crate::strict_class::prepare_dataclass_slots_type_handle(
        py,
        producer.as_raw(),
        &original,
        &name,
        &bases,
        &namespace,
        &proof,
    )?;
    bridge(owner, producer, args)?;
    Ok(handle)
}

pub(super) fn matches_construction_owner(
    owner: &Bound<'_, PyAny>,
    replacement: &Arc<ClassPlan>,
    original: &Arc<ClassPlan>,
) -> PyResult<bool> {
    let state = Owner::from_owner(owner.clone())?;
    Ok(state.data().phase.get() == Phase::Applying
        && original.phase.get() == ClassPhase::Bound
        && replacement.phase.get() != ClassPhase::Failed
        && state
            .data()
            .plan
            .get()
            .is_some_and(|plan| Arc::ptr_eq(plan, original))
        && state
            .data()
            .replacement
            .get()
            .is_some_and(|slots| Arc::ptr_eq(&slots.plan, replacement))
        && replacement
            .replacement_of
            .as_ref()
            .is_some_and(|plan| Arc::ptr_eq(plan, original))
        && super::invocation::validate_catalog(&state)?)
}

fn weak_class<'py>(owner: &Owner<'py>, index: usize) -> PyResult<Option<Bound<'py, PyAny>>> {
    let py = owner.owner().py();
    let weak = owner.reference(index)?;
    if weak.is_none() {
        return Ok(None);
    }
    let mut class = ptr::null_mut();
    match unsafe { ffi::PyWeakref_GetRef(weak.as_ptr(), &mut class) } {
        0 => Ok(None),
        1 => {
            let class = unsafe { Bound::<PyAny>::from_owned_ptr(py, class) };
            Ok((unsafe { ffi::PyType_Check(class.as_ptr()) } != 0).then_some(class))
        }
        _ => Err(PyErr::fetch(py)),
    }
}

pub(super) fn bind_class<'py>(
    raw_owner: &Bound<'py, PyAny>,
    plan: &Arc<ClassPlan>,
    class: &Bound<'py, PyAny>,
    class_owner: &Bound<'py, PyAny>,
) -> PyResult<()> {
    let py = class.py();
    let owner = Owner::from_owner(raw_owner.clone())?;
    let original = plan.replacement_of.as_ref().ok_or_else(|| {
        strict_runtime_unavailable(py, "slots class has no declaring construction")
    })?;
    require(
        &owner,
        plan.phase.get() == ClassPhase::Prepared
            && matches_construction_owner(raw_owner, plan, original)?
            && native::predicate(py, unsafe {
                native::PySoac_DataclassMatchesSlotsClass(
                    super::invocation::native_invocation(&owner)?.as_ptr(),
                    class.as_ptr(),
                    class_owner.as_ptr(),
                )
            })?,
        "slots class has no matching native replacement association",
    )?;
    let replacement = owner.data().replacement.get().unwrap();
    require(
        &owner,
        replacement.replacement_weak.get().is_none(),
        "slots class binding was replayed",
    )?;
    let weak = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            py,
            ffi::PyWeakref_NewRef(class.as_ptr(), ptr::null_mut()),
        )?
    };
    let index = super::produced::add_reference(&owner, weak)?;
    require(
        &owner,
        plan.phase.get() == ClassPhase::Prepared
            && matches_construction_owner(raw_owner, plan, original)?
            && native::predicate(py, unsafe {
                native::PySoac_DataclassMatchesSlotsClass(
                    super::invocation::native_invocation(&owner)?.as_ptr(),
                    class.as_ptr(),
                    class_owner.as_ptr(),
                )
            })?,
        "slots class association changed while recording its weak witness",
    )?;
    replacement.replacement_weak.set(Some(index));
    plan.actual_class.set(class.as_ptr() as usize);
    plan.actual_owner.set(class_owner.as_ptr() as usize);
    plan.phase.set(ClassPhase::Bound);
    Ok(())
}

pub(super) fn matches_result(owner: &Owner<'_>, result: &Bound<'_, PyAny>) -> PyResult<bool> {
    let Some(replacement) = owner.data().replacement.get() else {
        return Ok(false);
    };
    let Some(weak) = replacement.replacement_weak.get() else {
        return Ok(false);
    };
    if owner.data().phase.get() != Phase::Applying
        || replacement.plan.phase.get() != ClassPhase::Bound
        || replacement.plan.actual_class.get() != result.as_ptr() as usize
        || weak_class(owner, weak)?.is_none_or(|class| class.as_ptr() != result.as_ptr())
    {
        return Ok(false);
    }
    native::predicate(owner.owner().py(), unsafe {
        native::PySoac_DataclassMatchesSlotsClass(
            super::invocation::native_invocation(owner)?.as_ptr(),
            result.as_ptr(),
            replacement.plan.actual_owner.get() as *mut ffi::PyObject,
        )
    })
}

/// A temporary view of the actual native completion result, not an original
/// type pin. Only the real post-Apply result callback calls consumers while its
/// result token supports the class. Lost proof after Apply is an error, never
/// a fallback to the old comparison-only original address.
pub(super) fn completed_native_result<'py>(
    owner: &Owner<'py>,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    if !matches!(
        &owner.data().source_globals,
        super::invocation::SourceGlobals::Interpreter { .. }
    ) {
        return Ok(None);
    }
    let Some(replacement) = owner.data().replacement.get() else {
        return Ok(None);
    };
    if !replacement.apply_returned.get() {
        return Ok(None);
    }
    let class = replacement
        .replacement_weak
        .get()
        .map(|index| weak_class(owner, index))
        .transpose()?
        .flatten()
        .ok_or_else(|| {
            strict_runtime_unavailable(
                owner.owner().py(),
                "native slots result expired before member completion",
            )
        })?;
    require(
        owner,
        matches_result(owner, &class)?
            && protocol::matches_native_pending_class(owner, &replacement.plan, &class)?,
        "native slots completion lost its actual pending result",
    )?;
    Ok(Some(class))
}

pub(super) fn annotation_class(owner: &Owner<'_>) -> PyResult<*mut ffi::PyObject> {
    if let Some(replacement) = owner.data().replacement.get() {
        if replacement.apply_returned.get() {
            let class = replacement
                .replacement_weak
                .get()
                .map(|index| weak_class(owner, index))
                .transpose()?
                .flatten();
            let Some(class) = class else {
                return Err(strict_runtime_unavailable(
                    owner.owner().py(),
                    "slots annotation class expired",
                ));
            };
            require(
                owner,
                matches_result(owner, &class)?,
                "slots annotation class association changed",
            )?;
            // The root result/paired completion pins the type throughout this
            // callback-free borrowed read. No Python operation follows here.
            return Ok(class.as_ptr());
        }
    }
    Ok(protocol::plan(owner)?.actual_class.get() as *mut ffi::PyObject)
}

pub(super) fn validate_completed(
    owner: &Owner<'_>,
    plan: &Arc<ClassPlan>,
    class: &Bound<'_, PyAny>,
    namespace: &Bound<'_, PyDict>,
) -> PyResult<bool> {
    let Some(replacement) = owner.data().replacement.get() else {
        return Ok(false);
    };
    if !Arc::ptr_eq(plan, &replacement.plan)
        || !replacement.apply_returned.get()
        || !matches_result(owner, class)?
        || type_dictionary(class.as_ptr()) != Some(namespace.as_ptr())
    {
        return Ok(false);
    }
    if matches!(
        &owner.data().source_globals,
        super::invocation::SourceGlobals::Interpreter { .. }
    ) {
        return protocol::validate_native_slots_members(owner, plan, class, namespace);
    }
    if !protocol::matches_retained_pending_class(owner, plan, class)? {
        return Ok(false);
    }
    let Some(original) = replacement
        .original_weak
        .get()
        .map(|index| weak_class(owner, index))
        .transpose()?
        .flatten()
    else {
        return Ok(false);
    };
    let original_namespace = original_namespace(owner)?;
    if !protocol::validate_completed(owner, &original, &original_namespace)? {
        return Ok(false);
    }
    for member in &plan.members {
        let Some(actual) = (unsafe { dictionary_value(namespace.as_ptr(), &member.name) }) else {
            return Ok(false);
        };
        if let MemberKind::Shared(helper @ (Helper::GetState | Helper::SetState)) = member.kind {
            if !owner
                .data()
                .catalog
                .matches_function(owner.owner().py(), owner, helper, actual)?
            {
                return Ok(false);
            }
        } else if unsafe { dictionary_value(original_namespace.as_ptr(), &member.name) }
            != Some(actual)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Failure cleanup observes only a weak replacement; it grants no authority
/// and remains usable after fail_owner dropped all other active graph edges.
pub(crate) fn failed_replacement<'py>(
    raw_owner: &Bound<'py, PyAny>,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    let owner = Owner::from_owner(raw_owner.clone())?;
    owner
        .data()
        .replacement
        .get()
        .and_then(|replacement| replacement.replacement_weak.get())
        .map(|index| weak_class(&owner, index))
        .transpose()
        .map(Option::flatten)
}

pub(super) fn validate_copied_namespace(
    owner: &Bound<'_, PyAny>,
    plan: &Arc<ClassPlan>,
    namespace: &Bound<'_, PyDict>,
) -> PyResult<bool> {
    let Some(original_plan) = &plan.replacement_of else {
        return Ok(false);
    };
    if plan.phase.get() != ClassPhase::Prepared
        || !matches_construction_owner(owner, plan, original_plan)?
    {
        return Ok(false);
    }
    let state = Owner::from_owner(owner.clone())?;
    let Some(replacement) = state.data().replacement.get() else {
        return Ok(false);
    };
    let Some(original_weak) = replacement.original_weak.get() else {
        return Ok(false);
    };
    let Some(original) = weak_class(&state, original_weak)? else {
        return Ok(false);
    };
    let py = original.py();
    let native = matches!(
        &state.data().source_globals,
        super::invocation::SourceGlobals::Interpreter { .. }
    );
    let original_state = if native {
        if !protocol::matches_native_pending_class(&state, original_plan, &original)? {
            return Ok(false);
        }
        crate::strict_class_state::for_constructed_type(py, &original)?
    } else {
        if !protocol::matches_retained_pending_class(&state, original_plan, &original)? {
            return Ok(false);
        }
        crate::strict_class_state::for_constructed_type(py, &original)?
    };
    let Some(original_state) = original_state else {
        return Ok(false);
    };
    if original.as_ptr() as usize != original_plan.actual_class.get()
        || original_state.owner().as_ptr() as usize != original_plan.actual_owner.get()
        || !Arc::ptr_eq(
            original_state.namespace_execution(),
            &original_plan.namespace,
        )
    {
        return Ok(false);
    }
    let dictionary = unsafe { (*original.as_ptr().cast::<ffi::PyTypeObject>()).tp_dict };
    if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
        return Ok(false);
    }
    let original_namespace =
        unsafe { Bound::<PyAny>::from_borrowed_ptr(py, dictionary) }.cast_into::<PyDict>()?;
    if !super::protocol::validate_completed(&state, &original, &original_namespace)? {
        return Ok(false);
    }
    // Original generated functions have already consumed their member roles
    // and been sealed. Copying them into the replacement is identity checking,
    // never a second BeginMember operation or a change to their owner.
    Ok(plan.members.iter().all(|member| unsafe {
        let original = dictionary_value(original_namespace.as_ptr(), &member.name);
        let copied = dictionary_value(namespace.as_ptr(), &member.name);
        original == copied
            && (original.is_some()
                || matches!(
                    member.kind,
                    MemberKind::Shared(Helper::GetState | Helper::SetState)
                ))
    }))
}

#[cfg(test)]
mod tests {
    use pyo3::types::PyModule;

    use super::*;

    #[test]
    fn slots_metadata_is_read_without_iterators_or_key_callbacks() -> PyResult<()> {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"
events = []
class Key(str):
    def __eq__(self, other):
        events.append('equal')
        raise AssertionError
    __hash__ = str.__hash__
class Iterable:
    def __iter__(self):
        events.append('iterate')
        raise AssertionError
tuple_slots = ('left', 'right')
list_slots = ['left', 'right']
dict_slots = {'left': None, 'right': 'doc'}
hostile_key = {Key('left'): None}
hostile_iterable = Iterable()
",
                c"slots_metadata.py",
                c"slots_metadata",
            )?;
            let expected = vec!["left".to_owned(), "right".to_owned()];
            for name in ["tuple_slots", "list_slots", "dict_slots"] {
                let value = module.getattr(name)?;
                let (shape, observed) = names(value.as_ptr()).unwrap();
                assert_eq!(observed, expected);
                assert!(names_match(value.as_ptr(), shape, &expected));
                assert!(!names_match(
                    value.as_ptr(),
                    shape,
                    &["right".to_owned(), "left".to_owned()]
                ));
            }
            for name in ["hostile_key", "hostile_iterable"] {
                assert!(names(module.getattr(name)?.as_ptr()).is_none());
            }
            assert!(!names_match(
                module.getattr("hostile_key")?.as_ptr(),
                NamesShape::Dict,
                &["left".to_owned()]
            ));
            assert_eq!(module.getattr("events")?.len()?, 0);
            Ok(())
        })
    }
}
