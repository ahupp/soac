//! Callback-free checks of the actual operands consumed by generated roles.
//! Field names select entries only after the signed plan and exact native
//! field layout have authenticated the mapping. Values/factories stay ordinary.

use std::ptr;

use pyo3::ffi;
use pyo3::prelude::*;

use crate::strict_runtime_unavailable;

use super::catalog::{Helper, Sentinel, StructType, dictionary_value, text_is};
use super::generation::{GeneratedFragment, GeneratedRole, LocalOperand, Overwrite};
use super::invocation::Owner;
use super::native::Frame;

pub(super) fn field<'py>(owner: &Owner<'py>, index: usize) -> PyResult<Bound<'py, PyAny>> {
    let py = owner.owner().py();
    let plan = owner
        .data()
        .plan
        .get()
        .ok_or_else(|| strict_runtime_unavailable(py, "dataclass field plan is absent"))?;
    let expected = plan
        .generation
        .fields
        .get(index)
        .ok_or_else(|| strict_runtime_unavailable(py, "dataclass field index is absent"))?;
    let dictionary = if matches!(
        &owner.data().source_globals,
        super::invocation::SourceGlobals::Interpreter { .. }
    ) {
        // During native post-Apply completion the actual returned replacement,
        // not the potentially retired original, supports this callback-bound
        // mapping. Retained execution keeps its existing original support.
        super::protocol::actual_class_dictionary(owner)?
    } else {
        let class = plan.actual_class.get() as *mut ffi::PyTypeObject;
        if class.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "dataclass field class is unbound",
            ));
        }
        unsafe { (*class).tp_dict }
    };
    let actual = unsafe { dictionary_value(dictionary, "__dataclass_fields__") }
        .and_then(|fields| unsafe { dictionary_value(fields, &expected.name) })
        .ok_or_else(|| strict_runtime_unavailable(py, "dataclass actual field is absent"))?;
    let actual = unsafe { Bound::from_borrowed_ptr(py, actual) };
    if !super::fields::matches_field(py, &owner.data().catalog, owner, expected, &actual)? {
        return Err(strict_runtime_unavailable(
            py,
            "dataclass actual field changed",
        ));
    }
    Ok(actual)
}

pub(super) fn field_index(
    owner: &Owner<'_>,
    actual: *mut ffi::PyObject,
) -> PyResult<Option<usize>> {
    let plan = owner.data().plan.get().unwrap();
    for index in 0..plan.generation.fields.len() {
        if field(owner, index)?.as_ptr() == actual {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

pub(super) fn local_matches(
    owner: &Owner<'_>,
    meaning: LocalOperand,
    value: *mut ffi::PyObject,
) -> PyResult<bool> {
    let py = owner.owner().py();
    if value.is_null() {
        return Ok(false);
    }
    let catalog = &owner.data().catalog;
    Ok(match meaning {
        LocalOperand::FactoryMarker => {
            catalog.matches_sentinel(py, owner, Sentinel::Factory, value)?
        }
        LocalOperand::ObjectType => value == ptr::addr_of_mut!(ffi::PyBaseObject_Type).cast(),
        LocalOperand::FieldDefault(index) | LocalOperand::FieldFactory(index) => {
            let field = field(owner, index)?;
            let slot = if matches!(meaning, LocalOperand::FieldDefault(_)) {
                "default"
            } else {
                "default_factory"
            };
            catalog
                .member(py, owner, &field, StructType::Field, slot)?
                .is_some_and(|actual| actual.as_ptr() == value)
        }
        LocalOperand::RecursiveRepr => {
            catalog.matches_function(py, owner, Helper::RecursiveRepr, value)?
        }
        LocalOperand::ActualClass => {
            owner.data().plan.get().unwrap().actual_class.get() == value as usize
        }
        LocalOperand::FrozenInstanceError => catalog
            .structure(py, owner, StructType::FrozenError)?
            .is_some_and(|actual| actual.as_ptr() == value),
    })
}

pub(super) fn locals_match(
    owner: &Owner<'_>,
    actual: *mut ffi::PyObject,
    expected: &[(String, LocalOperand)],
) -> PyResult<bool> {
    if actual.is_null()
        || unsafe { ffi::PyDict_CheckExact(actual) } == 0
        || unsafe { ffi::PyDict_Size(actual) } as usize != expected.len()
    {
        return Ok(false);
    }
    let mut position = 0;
    let mut key = ptr::null_mut();
    let mut value = ptr::null_mut();
    for (name, meaning) in expected {
        if unsafe { ffi::PyDict_Next(actual, &mut position, &mut key, &mut value) } == 0
            || !unsafe { text_is(key, name) }
            || !local_matches(owner, *meaning, value)?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn sequence_len(value: *mut ffi::PyObject) -> Option<usize> {
    if value.is_null() {
        None
    } else if unsafe { ffi::PyTuple_CheckExact(value) } != 0 {
        usize::try_from(unsafe { ffi::PyTuple_Size(value) }).ok()
    } else if unsafe { ffi::PyList_CheckExact(value) } != 0 {
        usize::try_from(unsafe { ffi::PyList_Size(value) }).ok()
    } else {
        None
    }
}

pub(super) fn sequence_item(value: *mut ffi::PyObject, index: usize) -> *mut ffi::PyObject {
    unsafe {
        if ffi::PyTuple_CheckExact(value) != 0 {
            ffi::PyTuple_GetItem(value, index as ffi::Py_ssize_t)
        } else {
            ffi::PyList_GetItem(value, index as ffi::Py_ssize_t)
        }
    }
}

pub(super) fn text_sequence<'a>(
    value: *mut ffi::PyObject,
    expected: impl Iterator<Item = &'a str>,
) -> bool {
    let Some(length) = sequence_len(value) else {
        return false;
    };
    let mut count = 0;
    for text in expected {
        if count >= length || !unsafe { text_is(sequence_item(value, count), text) } {
            return false;
        }
        count += 1;
    }
    count == length
}

pub(super) fn builder_request(
    owner: &Owner<'_>,
    frame: Frame<'_>,
    fragment: &GeneratedFragment,
    entering: bool,
) -> PyResult<bool> {
    let py = owner.owner().py();
    let recipe = owner
        .data()
        .catalog
        .recipe(super::edges::CodeRole::Helper(Helper::BuilderAdd));
    let read = |name| {
        if entering {
            frame.parameter(py, recipe, name)
        } else {
            frame.executing(py, recipe, name)
        }
    };
    if !owner
        .data()
        .catalog
        .matches_structure(py, owner, StructType::Builder, read("self")?)?
        || !unsafe { text_is(read("name")?, fragment.role.name()) }
    {
        return Ok(false);
    }
    if entering {
        if !text_sequence(
            read("args")?,
            fragment.parameters.iter().map(String::as_str),
        ) || !text_sequence(
            read("body")?,
            fragment
                .source
                .lines()
                .skip(if fragment.role == GeneratedRole::Repr {
                    2
                } else {
                    1
                }),
        ) {
            return Ok(false);
        }
    }
    let locals = read("locals")?;
    if fragment.locals.is_empty() {
        if locals != unsafe { ffi::Py_None() } {
            return Ok(false);
        }
    } else if !locals_match(owner, locals, &fragment.locals)? {
        return Ok(false);
    }
    let annotations = read("annotation_fields")?;
    if let Some(fields) = &fragment.annotation_fields {
        if !text_sequence(annotations, fields.iter().map(String::as_str)) {
            return Ok(false);
        }
    } else if annotations != unsafe { ffi::Py_None() } {
        return Ok(false);
    }
    let return_type = read("return_type")?;
    if fragment.return_none {
        if return_type != unsafe { ffi::Py_None() } {
            return Ok(false);
        }
    } else if !owner
        .data()
        .catalog
        .matches_sentinel(py, owner, Sentinel::Missing, return_type)?
    {
        return Ok(false);
    }
    let overwrite = read("overwrite_error")?;
    if !match fragment.overwrite {
        Overwrite::Allowed => overwrite == unsafe { ffi::Py_False() },
        Overwrite::Error => overwrite == unsafe { ffi::Py_True() },
        Overwrite::OrderingError => unsafe {
            text_is(overwrite, "Consider using functools.total_ordering")
        },
    } {
        return Ok(false);
    }
    if read("unconditional_add")?
        != unsafe {
            if fragment.unconditional {
                ffi::Py_True()
            } else {
                ffi::Py_False()
            }
        }
    {
        return Ok(false);
    }
    let decorator = read("decorator")?;
    Ok(if fragment.role == GeneratedRole::Repr {
        unsafe { text_is(decorator, "@__dataclasses_recursive_repr()") }
    } else {
        decorator == unsafe { ffi::Py_None() }
    })
}

pub(super) fn factory_values(
    owner: &Owner<'_>,
    frame: Frame<'_>,
    entering: bool,
) -> PyResult<bool> {
    let py = owner.owner().py();
    let code = owner.data().code.get().unwrap();
    for (index, (_, meaning)) in code.locals.iter().enumerate() {
        let value = if entering {
            frame.local(py, index)?
        } else {
            let Some(binding) = super::code::compiled_binding(py, frame.code(), index)? else {
                return Ok(false);
            };
            frame.binding(py, binding)?
        };
        if !local_matches(owner, *meaning, value)? {
            return Ok(false);
        }
    }
    Ok(true)
}
