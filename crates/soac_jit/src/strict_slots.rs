//! Source-requested object storage, distinct from a dictionary prefix.
//!
//! This preparation names prospective native members; it never assigns their
//! offsets. The native type constructor must bind every name to an actual
//! T_OBJECT_EX member in its solid-base layout before Ready can publish it.

use std::collections::BTreeSet;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString, PyTuple};
use soac_contracts::{ClassTypeFact, DynamicClassReason};

/// Immutable, Rust-only proposal built from the already evaluated namespace.
/// An inherited dictionary remains a dictionary even when a child adds slots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObjectSlotPlan {
    pub(crate) names: Vec<String>,
    pub(crate) declared: bool,
    pub(crate) dictionary: bool,
}

impl ObjectSlotPlan {
    pub(crate) fn prepare<'a, 'py>(
        fact: &ClassTypeFact,
        namespace: &Bound<'py, PyDict>,
        bases: &Bound<'py, PyTuple>,
        inherited: impl IntoIterator<Item = &'a [String]>,
        replacement: bool,
    ) -> PyResult<Result<Self, DynamicClassReason>> {
        let declaration = crate::strict_class::namespace_item(namespace, "__slots__")?;
        if replacement && declaration.is_none() {
            return Ok(Err(DynamicClassReason::ConflictingLayout));
        }
        // The logical checker view can see inherited __slots__ or a
        // dataclass transform's future replacement. Neither determines this
        // construction's dictionary shape. Only its own actual namespace
        // and the physical actual bases do so.
        let mut names = Vec::new();
        for fields in inherited {
            for field in fields {
                if !names.contains(field) {
                    names.push(field.clone());
                }
            }
        }
        let mut dictionary = declaration.is_none()
            || bases.iter().any(|base| {
                let base = base.as_ptr().cast::<ffi::PyTypeObject>();
                // The orchestrator authenticates every actual base first.
                unsafe {
                    (*base).tp_dictoffset != 0
                        || (*base).tp_flags & ffi::Py_TPFLAGS_MANAGED_DICT != 0
                }
            });
        let declared = declaration.is_some();
        if let Some(declaration) = declaration {
            let values = if unsafe { ffi::PyUnicode_CheckExact(declaration.as_ptr()) } != 0 {
                vec![declaration]
            } else if unsafe { ffi::PyTuple_CheckExact(declaration.as_ptr()) } != 0 {
                declaration.cast::<PyTuple>()?.iter().collect()
            } else if unsafe { ffi::PyList_CheckExact(declaration.as_ptr()) } != 0 {
                declaration.cast::<PyList>()?.iter().collect()
            } else if unsafe { ffi::PyDict_CheckExact(declaration.as_ptr()) } != 0 {
                declaration
                    .cast::<PyDict>()?
                    .iter()
                    .map(|(name, _)| name)
                    .collect()
            } else {
                // Do not iterate a user object twice or invent its result.
                // Ordinary construction will perform its actual iteration.
                return Ok(Err(DynamicClassReason::ConflictingLayout));
            };
            let class_name = fact
                .identity
                .lexical_qualname
                .rsplit('.')
                .next()
                .unwrap_or("");
            let mut own = BTreeSet::new();
            for value in values {
                if unsafe { ffi::PyUnicode_CheckExact(value.as_ptr()) } == 0 {
                    return Ok(Err(DynamicClassReason::ConflictingLayout));
                }
                let value = value.cast::<PyString>()?.to_str()?;
                if value == "__dict__" {
                    dictionary = true;
                    continue;
                }
                if value == "__weakref__" {
                    continue;
                }
                let name = mangle_slot_name(class_name, value);
                if !own.insert(name.clone()) || names.contains(&name) {
                    // Re-declaring an inherited member creates a different
                    // physical field with the same spelling. The current
                    // catalog deliberately does not conflate those locations.
                    return Ok(Err(DynamicClassReason::ConflictingLayout));
                }
                names.push(name);
            }
        }
        Ok(Ok(Self {
            names,
            declared,
            dictionary,
        }))
    }
}

fn mangle_slot_name(class_name: &str, name: &str) -> String {
    let class_name = class_name.trim_start_matches('_');
    if name.starts_with("__") && !name.ends_with("__") && !class_name.is_empty() {
        format!("_{class_name}{name}")
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::mangle_slot_name;

    #[test]
    fn private_slot_names_follow_the_actual_class_name() {
        assert_eq!(mangle_slot_name("_Box", "__value"), "_Box__value");
        assert_eq!(mangle_slot_name("___", "__value"), "__value");
        assert_eq!(mangle_slot_name("Box", "__value__"), "__value__");
        assert_eq!(mangle_slot_name("Box", "value"), "value");
    }
}
