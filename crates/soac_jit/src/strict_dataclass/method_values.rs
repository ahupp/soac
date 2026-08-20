//! Exact generated function environments and owned-component relationships.
//! No shared helper, arbitrary closure value, source factory or class acquires
//! function ownership from these checks.

use pyo3::ffi;
use pyo3::prelude::*;

use super::catalog::{dictionary_value, text_is};
use super::edges::{CodeRole, Template};
use super::generation::{DefaultKind, FieldRole, GeneratedRole, LocalOperand};
use super::invocation::{Owner, native_invocation};
use super::native;
use super::produced::{fragment_index, methods, native_role};
use super::protocol::{closure_value, no_closure, no_defaults, plain_entry};

pub(super) fn template_matches(
    owner: &Owner<'_>,
    function: *mut ffi::PyObject,
    template: Template,
) -> PyResult<bool> {
    let py = owner.owner().py();
    if !plain_entry(function) || !no_defaults(function) {
        return Ok(false);
    }
    let raw = function.cast::<ffi::PyFunctionObject>();
    if !owner
        .data()
        .catalog
        .matches_code(py, owner, CodeRole::Template(template), unsafe {
            (*raw).func_code
        })?
    {
        return Ok(false);
    }
    let Some(parent) = owner
        .data()
        .catalog
        .function(py, owner, template.parent_helper())?
    else {
        return Ok(false);
    };
    let parent = parent.as_ptr().cast::<ffi::PyFunctionObject>();
    let closure = unsafe { (*raw).func_closure };
    Ok(unsafe {
        (*raw).func_globals == (*parent).func_globals
            && (*raw).func_builtins == (*parent).func_builtins
    } && !closure.is_null()
        && unsafe { ffi::PyTuple_CheckExact(closure) } != 0
        && unsafe { ffi::PyTuple_Size(closure) } as usize
            == owner
                .data()
                .catalog
                .recipe(CodeRole::Template(template))
                .closure_len())
}

pub(super) fn repr_decorator_matches(
    owner: &Owner<'_>,
    function: *mut ffi::PyObject,
) -> PyResult<bool> {
    if !methods(owner)?
        .repr_decorator
        .matches(owner, function, native::DECORATOR)?
        || !template_matches(owner, function, Template::ReprDecorator)?
    {
        return Ok(false);
    }
    let recipe = owner
        .data()
        .catalog
        .recipe(CodeRole::Template(Template::ReprDecorator));
    Ok(recipe
        .closure_index("fillvalue")
        .and_then(|index| closure_value(function, index))
        .is_some_and(|value| unsafe { text_is(value, "...") }))
}

fn generated_closure_matches(owner: &Owner<'_>, function: *mut ffi::PyObject) -> PyResult<bool> {
    let raw = function.cast::<ffi::PyFunctionObject>();
    let view = unsafe { super::code::view(owner.owner().py(), (*raw).func_code)? };
    let closure = unsafe { (*raw).func_closure };
    if view.nfreevars == 0 {
        return Ok(no_closure(function));
    }
    if closure.is_null()
        || unsafe { ffi::PyTuple_CheckExact(closure) } == 0
        || unsafe { ffi::PyTuple_Size(closure) } != view.nfreevars as ffi::Py_ssize_t
    {
        return Ok(false);
    }
    let locals = &owner.data().code.get().unwrap().locals;
    for index in 0..view.nfreevars as usize {
        let name = unsafe {
            ffi::PyTuple_GetItem(
                view.localsplusnames,
                (view.nlocalsplus - view.nfreevars) as ffi::Py_ssize_t + index as ffi::Py_ssize_t,
            )
        };
        let Some((_, meaning)) = locals
            .iter()
            .find(|(expected, _)| unsafe { text_is(name, expected) })
        else {
            return Ok(false);
        };
        let Some(actual) = closure_value(function, index) else {
            return Ok(false);
        };
        if !super::operands::local_matches(owner, *meaning, actual)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn generated_defaults_match(
    owner: &Owner<'_>,
    function: *mut ffi::PyObject,
    index: usize,
) -> PyResult<bool> {
    let plan = &owner.data().plan.get().unwrap().generation;
    if plan.fragments[index].role != GeneratedRole::Init {
        return Ok(no_defaults(function));
    }
    let raw = function.cast::<ffi::PyFunctionObject>();
    let defaults = unsafe { (*raw).func_defaults };
    let keywords = unsafe { (*raw).func_kwdefaults };
    let mut positions = 0;
    let mut keyword_count = 0;
    for (field_index, field) in plan
        .fields
        .iter()
        .enumerate()
        .filter(|(_, field)| field.role != FieldRole::ClassVariable && field.init)
    {
        let meaning = match field.default {
            DefaultKind::Missing => continue,
            DefaultKind::Value => LocalOperand::FieldDefault(field_index),
            DefaultKind::Factory => LocalOperand::FactoryMarker,
        };
        let value = if field.kw_only {
            keyword_count += 1;
            let Some(value) = (unsafe { dictionary_value(keywords, &field.name) }) else {
                return Ok(false);
            };
            value
        } else {
            if defaults.is_null()
                || unsafe { ffi::PyTuple_CheckExact(defaults) } == 0
                || positions >= unsafe { ffi::PyTuple_Size(defaults) }
            {
                return Ok(false);
            }
            let value = unsafe { ffi::PyTuple_GetItem(defaults, positions) };
            positions += 1;
            value
        };
        if !super::operands::local_matches(owner, meaning, value)? {
            return Ok(false);
        }
    }
    Ok((if defaults.is_null() {
        positions == 0
    } else {
        unsafe {
            ffi::PyTuple_CheckExact(defaults) != 0 && ffi::PyTuple_Size(defaults) == positions
        }
    }) && (if keywords.is_null() {
        keyword_count == 0
    } else {
        unsafe {
            ffi::PyDict_CheckExact(keywords) != 0 && ffi::PyDict_Size(keywords) == keyword_count
        }
    }))
}

pub(super) fn function_matches(
    owner: &Owner<'_>,
    index: usize,
    function: *mut ffi::PyObject,
    implementation: bool,
) -> PyResult<bool> {
    let py = owner.owner().py();
    let plan = &owner.data().plan.get().unwrap().generation;
    let Some(fragment) = plan.fragments.get(index) else {
        return Ok(false);
    };
    let birth = &methods(owner)?.methods[index];
    let (witness, role) = if implementation {
        let Some(implementation) = &birth.implementation else {
            return Ok(false);
        };
        (implementation, native::REPR_IMPLEMENTATION)
    } else {
        (&birth.function, native_role(fragment.role))
    };
    if !witness.matches(owner, function, role)? {
        return Ok(false);
    }
    if !plain_entry(function) {
        return Ok(false);
    }
    let raw = function.cast::<ffi::PyFunctionObject>();
    if fragment.role == GeneratedRole::Repr && !implementation {
        if !template_matches(owner, function, Template::ReprWrapper)? {
            return Ok(false);
        }
        let recipe = owner
            .data()
            .catalog
            .recipe(CodeRole::Template(Template::ReprWrapper));
        let Some(fill) = recipe
            .closure_index("fillvalue")
            .and_then(|index| closure_value(function, index))
        else {
            return Ok(false);
        };
        let Some(running) = recipe
            .closure_index("repr_running")
            .and_then(|index| closure_value(function, index))
        else {
            return Ok(false);
        };
        let Some(inner) = recipe
            .closure_index("user_function")
            .and_then(|index| closure_value(function, index))
        else {
            return Ok(false);
        };
        if !unsafe { text_is(fill, "...") }
            || unsafe { ffi::PySet_CheckExact(running) } == 0
            || !function_matches(owner, index, inner, true)?
            || unsafe { dictionary_value((*raw).func_dict, "__wrapped__") } != Some(inner)
        {
            return Ok(false);
        }
    } else {
        let tree = owner.reference(owner.data().generated_code)?;
        if owner
            .data()
            .code
            .get()
            .unwrap()
            .method_for_code(py, tree.as_ptr(), unsafe { (*raw).func_code })?
            != Some(index)
            || !unsafe { super::invocation::matches_source_globals(owner, (*raw).func_globals)? }
            || !generated_closure_matches(owner, function)?
        {
            return Ok(false);
        }
        let globals = unsafe { (*raw).func_globals };
        let Some(builtins) = (unsafe { dictionary_value(globals, "__builtins__") }) else {
            return Ok(false);
        };
        let actual = if unsafe { ffi::PyModule_CheckExact(builtins) } != 0 {
            unsafe { ffi::PyModule_GetDict(builtins) }
        } else {
            builtins
        };
        if unsafe { (*raw).func_builtins } != actual {
            return Ok(false);
        }
    }
    Ok(generated_defaults_match(owner, function, index)?
        && unsafe { text_is((*raw).func_name, fragment.role.name()) })
}

pub(super) fn annotation_matches(
    owner: &Owner<'_>,
    index: usize,
    function: *mut ffi::PyObject,
) -> PyResult<bool> {
    let birth = &methods(owner)?.methods[index];
    let Some(annotation) = &birth.annotation else {
        return Ok(false);
    };
    if !annotation.matches(owner, function, native::ANNOTATION_PROVIDER)?
        || !template_matches(owner, function, Template::AnnotationProvider)?
    {
        return Ok(false);
    }
    let recipe = owner
        .data()
        .catalog
        .recipe(CodeRole::Template(Template::AnnotationProvider));
    let value = |name| {
        recipe
            .closure_index(name)
            .and_then(|index| closure_value(function, index))
    };
    let plan = owner.data().plan.get().unwrap();
    let fragment = &plan.generation.fragments[index];
    Ok(
        value("__class__") == Some(super::slots::annotation_class(owner)?)
            && value("return_type") == Some(unsafe { ffi::Py_None() })
            && value("annotation_fields").is_some_and(|actual| {
                fragment.annotation_fields.as_ref().is_some_and(|fields| {
                    super::operands::text_sequence(actual, fields.iter().map(String::as_str))
                })
            }),
    )
}

pub(super) fn component_matches(
    owner: &Owner<'_>,
    method: *mut ffi::PyObject,
    component: *mut ffi::PyObject,
    kind: u32,
    closure_index: ffi::Py_ssize_t,
) -> PyResult<bool> {
    if method.is_null()
        || component.is_null()
        || unsafe { ffi::PyFunction_Check(method) } == 0
        || unsafe { ffi::PyFunction_Check(component) } == 0
    {
        return Ok(false);
    }
    let raw = method.cast::<ffi::PyFunctionObject>();
    let Some(index) = fragment_index(owner, unsafe { (*raw).func_name }) else {
        return Ok(false);
    };
    if !function_matches(owner, index, method, false)? {
        return Ok(false);
    }
    // Both approved component kinds have no defaults/kwdefaults. Therefore
    // native metadata adoption below allocates nothing and invokes only this
    // pure validator; unrelated closure values are never traversed or sealed.
    if !no_defaults(component)
        || !unsafe { (*component.cast::<ffi::PyFunctionObject>()).func_kwdefaults }.is_null()
    {
        return Ok(false);
    }
    match kind {
        native::COMPONENT_ANNOTATE => Ok(closure_index == -1
            && unsafe { (*raw).func_annotate } == component
            && annotation_matches(owner, index, component)?),
        native::COMPONENT_REPR => {
            let expected = owner
                .data()
                .catalog
                .recipe(CodeRole::Template(Template::ReprWrapper))
                .closure_index("user_function");
            Ok(
                expected.is_some_and(|index| index as ffi::Py_ssize_t == closure_index)
                    && closure_value(method, closure_index as usize) == Some(component)
                    && function_matches(owner, index, component, true)?,
            )
        }
        _ => Ok(false),
    }
}

pub(super) fn adopt_components(
    owner: &Owner<'_>,
    index: usize,
    method: *mut ffi::PyObject,
) -> PyResult<()> {
    let py = owner.owner().py();
    let birth = &methods(owner)?.methods[index];
    if birth.components_adopted.get() {
        return Ok(());
    }
    if let Some(annotation) = &birth.annotation {
        let component = annotation.function(owner)?.ok_or_else(|| {
            crate::strict_runtime_unavailable(py, "generated annotation function expired")
        })?;
        native::status(py, unsafe {
            native::PyFunction_AdoptSoacDataclassComponent(
                native_invocation(owner)?.as_ptr(),
                method,
                component.as_ptr(),
                native::COMPONENT_ANNOTATE,
                -1,
            )
        })?;
    }
    if let Some(implementation) = &birth.implementation {
        let component = implementation.function(owner)?.ok_or_else(|| {
            crate::strict_runtime_unavailable(py, "generated repr implementation expired")
        })?;
        let index = owner
            .data()
            .catalog
            .recipe(CodeRole::Template(Template::ReprWrapper))
            .closure_index("user_function")
            .ok_or_else(|| {
                crate::strict_runtime_unavailable(py, "repr component projection is absent")
            })?;
        native::status(py, unsafe {
            native::PyFunction_AdoptSoacDataclassComponent(
                native_invocation(owner)?.as_ptr(),
                method,
                component.as_ptr(),
                native::COMPONENT_REPR,
                index as ffi::Py_ssize_t,
            )
        })?;
    }
    birth.components_adopted.set(true);
    Ok(())
}
