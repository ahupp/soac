//! One-way actual-function births and source-owned component publication slots.
//!
//! Slots are allocated before Apply. The native Created callback consumes a
//! slot before exposing a weak function witness. Native publication preserves
//! structural closure completeness; no slot owns a function or selects a
//! parameter/return predicate.

use std::cell::Cell;
use std::ptr;

use pyo3::ffi;
use pyo3::prelude::*;

use crate::strict_runtime_unavailable;

use super::generation::{GeneratedRole, GenerationPlan};
use super::invocation::{Owner, native_invocation};
use super::native;

pub(super) struct Birth {
    reference: usize,
    claimed: Cell<bool>,
}

impl Birth {
    fn reserve(owner: &Owner<'_>) -> PyResult<Self> {
        Ok(Self {
            reference: add_reference(
                owner,
                owner.owner().py().None().into_bound(owner.owner().py()),
            )?,
            claimed: Cell::new(false),
        })
    }

    pub(super) fn claim(&self, owner: &Owner<'_>) -> PyResult<()> {
        if self.claimed.replace(true) {
            return Err(strict_runtime_unavailable(
                owner.owner().py(),
                "generated function birth was replayed",
            ));
        }
        Ok(())
    }

    /// Publish the actual created function through a weak witness. Native
    /// entry independently requires its ordinary closure to be complete.
    pub(super) fn publish(&self, owner: &Owner<'_>, function: *mut ffi::PyObject) -> PyResult<()> {
        let py = owner.owner().py();
        if !self.claimed.get() {
            return Err(strict_runtime_unavailable(
                py,
                "generated birth was not consumed",
            ));
        }
        let weak = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                ffi::PyWeakref_NewRef(function, ptr::null_mut()),
            )?
        };
        owner.bind_reserved_reference(self.reference, weak)
    }

    pub(super) fn function<'py>(&self, owner: &Owner<'py>) -> PyResult<Option<Bound<'py, PyAny>>> {
        let py = owner.owner().py();
        let weak = owner.reference(self.reference)?;
        if weak.is_none() {
            return Ok(None);
        }
        let mut function = ptr::null_mut();
        match unsafe { ffi::PyWeakref_GetRef(weak.as_ptr(), &mut function) } {
            0 => Ok(None),
            1 => Ok(Some(unsafe { Bound::from_owned_ptr(py, function) })),
            _ => Err(PyErr::fetch(py)),
        }
    }

    pub(super) fn matches(
        &self,
        owner: &Owner<'_>,
        function: *mut ffi::PyObject,
        role: u32,
    ) -> PyResult<bool> {
        if function.is_null()
            || !self
                .function(owner)?
                .is_some_and(|actual| actual.as_ptr() == function)
        {
            return Ok(false);
        }
        native::predicate(owner.owner().py(), unsafe {
            native::PyFunction_MatchesSoacDataclassCreation(
                function,
                native_invocation(owner)?.as_ptr(),
                role,
            )
        })
    }
}

pub(super) fn add_reference<'py>(owner: &Owner<'py>, value: Bound<'py, PyAny>) -> PyResult<usize> {
    let index = owner.add_reference(value)?;
    owner.data().active_reference_count.set(index + 1);
    Ok(index)
}

pub(super) struct MethodBirth {
    pub(super) function: Birth,
    pub(super) implementation: Option<Birth>,
    pub(super) annotation: Option<Birth>,
    pub(super) components_adopted: Cell<bool>,
}

pub(super) struct GeneratedMethods {
    pub(super) methods: Vec<MethodBirth>,
    pub(super) repr_decorator: Birth,
}

impl GeneratedMethods {
    pub(super) fn prepare<'py>(owner: &Owner<'py>, plan: &GenerationPlan) -> PyResult<Self> {
        let mut methods = Vec::with_capacity(plan.fragments.len());
        for fragment in &plan.fragments {
            methods.push(MethodBirth {
                function: Birth::reserve(owner)?,
                implementation: if fragment.role == GeneratedRole::Repr {
                    Some(Birth::reserve(owner)?)
                } else {
                    None
                },
                annotation: if fragment.annotation_fields.is_some() {
                    Some(Birth::reserve(owner)?)
                } else {
                    None
                },
                components_adopted: Cell::new(false),
            });
        }
        Ok(Self {
            methods,
            repr_decorator: Birth::reserve(owner)?,
        })
    }
}

pub(super) fn native_role(role: GeneratedRole) -> u32 {
    match role {
        GeneratedRole::FrozenSetattr => native::FROZEN_SETATTR,
        GeneratedRole::FrozenDelattr => native::FROZEN_DELATTR,
        _ => native::FUNCTION_MEMBER,
    }
}

pub(super) fn methods<'a>(owner: &'a Owner<'_>) -> PyResult<&'a GeneratedMethods> {
    owner.data().produced.get().ok_or_else(|| {
        strict_runtime_unavailable(owner.owner().py(), "generated birth plan is absent")
    })
}

pub(super) fn fragment_index(owner: &Owner<'_>, name: *mut ffi::PyObject) -> Option<usize> {
    owner
        .data()
        .plan
        .get()?
        .generation
        .fragments
        .iter()
        .position(|fragment| unsafe { super::catalog::text_is(name, fragment.role.name()) })
}
