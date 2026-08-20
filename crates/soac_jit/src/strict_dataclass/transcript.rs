//! Deterministic text plus the one actual native compiler result.
//!
//! Recompiling an expected body would issue an extra user-visible compile
//! audit event. Instead the fixed native bridge compiles the exact validated
//! text once and hands this callback its fresh root and C-created weak tree.
//! Names select roles only within that already authenticated compiler result.

use std::cell::Cell;
use std::ptr;

use pyo3::ffi;
use pyo3::prelude::*;

use super::catalog::text_is;
use super::code;
use super::generation::{GenerationPlan, LocalOperand};

pub(super) struct GeneratedCode {
    pub(super) source: String,
    pub(super) locals: Vec<(String, LocalOperand)>,
    pub(super) source_count: Cell<usize>,
    pub(super) compiled: Cell<bool>,
    pub(super) exec_entered: Cell<bool>,
    pub(super) factory_entered: Cell<bool>,
    factory_constant: Cell<Option<usize>>,
    method_constants: Vec<Cell<Option<usize>>>,
    pub(super) repr_calls: Cell<Option<[usize; 2]>>,
}

impl GeneratedCode {
    pub(super) fn prepare(_py: Python<'_>, plan: &GenerationPlan) -> PyResult<Self> {
        let mut locals: Vec<(String, LocalOperand)> = Vec::new();
        for fragment in &plan.fragments {
            for (name, meaning) in &fragment.locals {
                if let Some((_, previous)) = locals.iter().find(|(previous, _)| previous == name) {
                    if previous != meaning {
                        return Err(pyo3::exceptions::PyValueError::new_err(
                            "conflicting generated local operand",
                        ));
                    }
                } else {
                    locals.push((name.clone(), *meaning));
                }
            }
        }
        let local_vars = locals
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let functions = plan
            .fragments
            .iter()
            .map(|fragment| fragment.source.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let return_names = if plan.fragments.is_empty() {
            "()".to_owned()
        } else {
            format!(
                "({},)",
                plan.fragments
                    .iter()
                    .map(|fragment| fragment.role.name())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        Ok(Self {
            source: format!(
                "def __create_fn__({local_vars}):\n{functions}\n return {return_names}"
            ),
            locals,
            source_count: Cell::new(0),
            compiled: Cell::new(false),
            exec_entered: Cell::new(false),
            factory_entered: Cell::new(false),
            factory_constant: Cell::new(None),
            method_constants: plan.fragments.iter().map(|_| Cell::new(None)).collect(),
            repr_calls: Cell::new(None),
        })
    }

    pub(super) fn matches_source(&self, value: *mut ffi::PyObject) -> bool {
        unsafe { text_is(value, &self.source) }
    }

    /// All mutable Python source/builder operands have already been checked at
    /// this exact EXEC edge. This callback validates the native result and weak
    /// graph without Python allocation, equality, attribute lookup, or eval.
    pub(super) fn bind_compiled(
        &self,
        py: Python<'_>,
        plan: &GenerationPlan,
        root: *mut ffi::PyObject,
        weak_tree: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        if self.compiled.get() || self.source_count.get() != plan.fragments.len() {
            return Ok(false);
        }
        let Some(root_view) = ordinary_view(py, root)? else {
            return Ok(false);
        };
        if !unsafe { text_is(root_view.name, "<module>") }
            || root_view.argcount != 0
            || root_view.kwonlyargcount != 0
            || root_view.nfreevars != 0
            || !weak_tree_matches(py, weak_tree, root)?
        {
            return Ok(false);
        }
        let Some((factory_index, factory)) = only_code_constant(root_view.consts) else {
            return Ok(false);
        };
        let Some(factory_view) = ordinary_view(py, factory)? else {
            return Ok(false);
        };
        if !unsafe { text_is(factory_view.name, "__create_fn__") }
            || !unsafe { text_is(factory_view.qualname, "__create_fn__") }
            || factory_view.argcount != self.locals.len() as i32
            || factory_view.posonlyargcount != 0
            || factory_view.kwonlyargcount != 0
            || factory_view.nfreevars != 0
        {
            return Ok(false);
        }
        for (index, (name, _)) in self.locals.iter().enumerate() {
            if !unsafe {
                text_is(
                    ffi::PyTuple_GetItem(factory_view.localsplusnames, index as ffi::Py_ssize_t),
                    name,
                )
            } {
                return Ok(false);
            }
        }
        let mut method_count = 0;
        for index in 0..unsafe { ffi::PyTuple_Size(factory_view.consts) } {
            let value = unsafe { ffi::PyTuple_GetItem(factory_view.consts, index) };
            if unsafe { ffi::PyCode_Check(value) } == 0 {
                continue;
            }
            let Some(view) = ordinary_view(py, value)? else {
                return Ok(false);
            };
            let Some(method) = plan
                .fragments
                .iter()
                .position(|fragment| unsafe { text_is(view.name, fragment.role.name()) })
            else {
                return Ok(false);
            };
            if self.method_constants[method].get().is_some() {
                return Ok(false);
            }
            self.method_constants[method].set(Some(index as usize));
            method_count += 1;
        }
        if method_count != plan.fragments.len() {
            return Ok(false);
        }
        let mut line = 1;
        for fragment in &plan.fragments {
            if fragment.role == super::generation::GeneratedRole::Repr {
                let Some(pair) = code::compiled_decorator_calls(
                    py,
                    factory,
                    code::CallSpan::new(line, line, 2, 32),
                )?
                else {
                    return Ok(false);
                };
                self.repr_calls.set(Some(pair));
            }
            line += fragment.source.lines().count() as i32;
        }
        self.factory_constant.set(Some(factory_index));
        self.compiled.set(true);
        Ok(true)
    }

    pub(super) fn matches_root(
        &self,
        py: Python<'_>,
        weak_tree: *mut ffi::PyObject,
        code: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        Ok(self.compiled.get()
            && weak_node_matches(py, weak_tree, code)?
            && ordinary_view(py, code)?.is_some())
    }

    pub(super) fn matches_factory(
        &self,
        py: Python<'_>,
        weak_tree: *mut ffi::PyObject,
        code: *mut ffi::PyObject,
    ) -> PyResult<bool> {
        let Some(index) = self.factory_constant.get() else {
            return Ok(false);
        };
        let Some(child) = weak_child(weak_tree, index) else {
            return Ok(false);
        };
        Ok(weak_node_matches(py, child, code)? && ordinary_view(py, code)?.is_some())
    }

    pub(super) fn method_for_code(
        &self,
        py: Python<'_>,
        weak_tree: *mut ffi::PyObject,
        code: *mut ffi::PyObject,
    ) -> PyResult<Option<usize>> {
        let Some(factory) = self
            .factory_constant
            .get()
            .and_then(|index| weak_child(weak_tree, index))
        else {
            return Ok(None);
        };
        for (index, constant) in self.method_constants.iter().enumerate() {
            let Some(child) = constant.get().and_then(|index| weak_child(factory, index)) else {
                continue;
            };
            if weak_node_matches(py, child, code)? && ordinary_view(py, code)?.is_some() {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }
}

fn ordinary_view(
    py: Python<'_>,
    value: *mut ffi::PyObject,
) -> PyResult<Option<code::RawPySoacCodeView>> {
    if value.is_null() || unsafe { ffi::PyCode_Check(value) } == 0 {
        return Ok(None);
    }
    let view = unsafe { code::view(py, value)? };
    if view.strict_source_id != 0
        || view.flags
            & (ffi::CO_GENERATOR
                | ffi::CO_COROUTINE
                | ffi::CO_ASYNC_GENERATOR
                | ffi::CO_VARARGS
                | ffi::CO_VARKEYWORDS)
            != 0
        || unsafe { ffi::PyTuple_CheckExact(view.consts) } == 0
        || unsafe { ffi::PyTuple_CheckExact(view.localsplusnames) } == 0
    {
        return Ok(None);
    }
    Ok(Some(view))
}

fn only_code_constant(constants: *mut ffi::PyObject) -> Option<(usize, *mut ffi::PyObject)> {
    if constants.is_null() || unsafe { ffi::PyTuple_CheckExact(constants) } == 0 {
        return None;
    }
    let mut result = None;
    for index in 0..unsafe { ffi::PyTuple_Size(constants) } {
        let value = unsafe { ffi::PyTuple_GetItem(constants, index) };
        if unsafe { ffi::PyCode_Check(value) } != 0 {
            if result.is_some() {
                return None;
            }
            result = Some((index as usize, value));
        }
    }
    result
}

fn weak_node_matches(
    py: Python<'_>,
    tree: *mut ffi::PyObject,
    code: *mut ffi::PyObject,
) -> PyResult<bool> {
    if tree.is_null()
        || unsafe { ffi::PyTuple_CheckExact(tree) } == 0
        || unsafe { ffi::PyTuple_Size(tree) } != 2
    {
        return Ok(false);
    }
    let weak = unsafe { ffi::PyTuple_GetItem(tree, 0) };
    if unsafe { ffi::PyWeakref_CheckRefExact(weak) } == 0 {
        return Ok(false);
    }
    let mut value = ptr::null_mut();
    match unsafe { ffi::PyWeakref_GetRef(weak, &mut value) } {
        0 => Ok(false),
        1 => {
            let value = unsafe { Bound::<PyAny>::from_owned_ptr(py, value) };
            Ok(value.as_ptr() == code)
        }
        _ => Err(PyErr::fetch(py)),
    }
}

fn weak_child(tree: *mut ffi::PyObject, index: usize) -> Option<*mut ffi::PyObject> {
    if tree.is_null()
        || unsafe { ffi::PyTuple_CheckExact(tree) } == 0
        || unsafe { ffi::PyTuple_Size(tree) } != 2
    {
        return None;
    }
    let children = unsafe { ffi::PyTuple_GetItem(tree, 1) };
    if unsafe { ffi::PyTuple_CheckExact(children) } == 0 {
        return None;
    }
    let mut found = None;
    for child_index in 0..unsafe { ffi::PyTuple_Size(children) } {
        let entry = unsafe { ffi::PyTuple_GetItem(children, child_index) };
        if unsafe { ffi::PyTuple_CheckExact(entry) } == 0
            || unsafe { ffi::PyTuple_Size(entry) } != 2
        {
            return None;
        }
        let key = unsafe { ffi::PyTuple_GetItem(entry, 0) };
        if unsafe { ffi::PyLong_CheckExact(key) } == 0 {
            return None;
        }
        let actual = unsafe { ffi::PyLong_AsSsize_t(key) };
        if actual < 0 {
            return None;
        }
        if actual as usize == index {
            if found.is_some() {
                return None;
            }
            found = Some(unsafe { ffi::PyTuple_GetItem(entry, 1) });
        }
    }
    found
}

fn weak_tree_matches(
    py: Python<'_>,
    tree: *mut ffi::PyObject,
    code: *mut ffi::PyObject,
) -> PyResult<bool> {
    if !weak_node_matches(py, tree, code)? {
        return Ok(false);
    }
    let view = unsafe { code::view(py, code)? };
    let children = unsafe { ffi::PyTuple_GetItem(tree, 1) };
    if unsafe { ffi::PyTuple_CheckExact(children) } == 0 {
        return Ok(false);
    }
    let mut expected_children = 0;
    for index in 0..unsafe { ffi::PyTuple_Size(view.consts) } {
        let child = unsafe { ffi::PyTuple_GetItem(view.consts, index) };
        if unsafe { ffi::PyCode_Check(child) } == 0 {
            continue;
        }
        let Some(child_tree) = weak_child(tree, index as usize) else {
            return Ok(false);
        };
        if !weak_tree_matches(py, child_tree, child)? {
            return Ok(false);
        }
        expected_children += 1;
    }
    if unsafe { !ffi::PyErr_Occurred().is_null() } {
        return Err(PyErr::fetch(py));
    }
    Ok(expected_children == unsafe { ffi::PyTuple_Size(children) })
}
