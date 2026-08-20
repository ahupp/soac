//! Actual Field projections for a signed dataclass generation plan.
//!
//! Source facts specify names, kinds and default categories. Actual canonical
//! Field slots specify the ordinary generator flags. Neither an annotation
//! callback nor a descriptor is evaluated to decide participation. Later
//! producer edges recheck the actual fields against these Rust-only values.

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyTuple};
use soac_contracts::{AnnotationOrigin, ClassTypeFact, DataclassOptions, DefaultFact, FieldKind};

use super::catalog::{HelperCatalog, References, Sentinel, StructType, dictionary_value, text_is};
use super::generation::{DefaultKind, FieldRole, GenerationField};

fn exact_bool(value: *mut ffi::PyObject) -> Option<bool> {
    if value == unsafe { ffi::Py_True() } {
        Some(true)
    } else if value == unsafe { ffi::Py_False() } {
        Some(false)
    } else {
        None
    }
}

pub(super) struct FieldProjection {
    pub(super) fields: Vec<GenerationField>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FieldValues {
    default: DefaultKind,
    init: bool,
    repr: bool,
    compare: bool,
    hash: Option<bool>,
    kw_only: bool,
}

impl FieldValues {
    fn with_name(self, name: String, role: FieldRole) -> GenerationField {
        GenerationField {
            name,
            role,
            default: self.default,
            init: self.init,
            repr: self.repr,
            compare: self.compare,
            hash: self.hash,
            kw_only: self.kw_only,
        }
    }

    fn matches(self, field: &GenerationField) -> bool {
        self.default == field.default
            && self.init == field.init
            && self.repr == field.repr
            && self.compare == field.compare
            && self.hash == field.hash
            && self.kw_only == field.kw_only
    }
}

impl FieldProjection {
    pub(super) fn capture<'py>(
        py: Python<'py>,
        catalog: &HelperCatalog,
        references: &impl References<'py>,
        fact: &ClassTypeFact,
        options: &DataclassOptions,
        namespace: &Bound<'py, PyDict>,
        bases: &Bound<'py, PyTuple>,
    ) -> PyResult<Option<Self>> {
        let mut fields = Vec::new();
        let Some(field_class) = catalog.structure(py, references, StructType::Field)? else {
            return Ok(None);
        };
        for field in &fact.instance_fields {
            // Inferred assignments in a user-written __init__ are instance
            // layout proposals, not dataclasses' annotated field declarations.
            if field.annotation_origin != AnnotationOrigin::Explicit {
                continue;
            }
            let role = match field.field_kind {
                FieldKind::InstanceField
                | FieldKind::CallableInstanceField
                | FieldKind::ShadowableClassDefault => FieldRole::Instance,
                FieldKind::ClassVariable => FieldRole::ClassVariable,
                FieldKind::InitOnly => FieldRole::InitOnly,
                _ => return Ok(None),
            };
            let expected_default = match &field.default {
                DefaultFact::Missing => DefaultKind::Missing,
                DefaultFact::Value { .. } => DefaultKind::Value,
                DefaultFact::Factory { .. } => DefaultKind::Factory,
                DefaultFact::Unknown => return Ok(None),
            };
            let inherited = field.declaring_class.definition != fact.identity;
            let actual = if inherited {
                let Some(actual) = inherited_field(py, bases, &field.name)? else {
                    return Ok(None);
                };
                Some(actual.value)
            } else {
                unsafe { dictionary_value(namespace.as_ptr(), &field.name) }
                    .map(|value| unsafe { Bound::<PyAny>::from_borrowed_ptr(py, value) })
            };
            let canonical_field = actual.as_ref().is_some_and(|value| {
                // This is only an exact native type/slot-layout test. A
                // recognized invalid catalog was rejected above, not converted
                // into an ordinary field default.
                unsafe { ffi::Py_TYPE(value.as_ptr()) }.cast::<ffi::PyObject>()
                    == field_class.as_ptr()
            });
            let projected = if canonical_field || inherited {
                let Some(actual) = actual else {
                    return Ok(None);
                };
                let Some(projected) = project_field(
                    py,
                    catalog,
                    references,
                    &actual,
                    &field.name,
                    role,
                    options.kw_only,
                    inherited,
                )?
                else {
                    return Ok(None);
                };
                projected.with_name(field.name.clone(), role)
            } else {
                if actual.as_ref().is_some_and(|value| unsafe {
                    let kind = ffi::Py_TYPE(value.as_ptr());
                    (*kind).tp_descr_get.is_some() || (*kind).tp_descr_set.is_some()
                }) {
                    return Ok(None);
                }
                GenerationField {
                    name: field.name.clone(),
                    role,
                    default: if actual.is_some() {
                        DefaultKind::Value
                    } else {
                        DefaultKind::Missing
                    },
                    init: true,
                    kw_only: role != FieldRole::ClassVariable && options.kw_only,
                    repr: true,
                    compare: true,
                    hash: None,
                }
            };
            if projected.default != expected_default {
                return Ok(None);
            }
            fields.push(projected);
        }
        Ok(Some(Self { fields }))
    }
}

/// Read the current actual Field against an already-selected Rust projection.
/// This path runs at native producer callbacks and allocates no field names,
/// tuples, dictionaries, or projection vectors.
pub(super) fn matches_field<'py>(
    py: Python<'py>,
    catalog: &HelperCatalog,
    references: &impl References<'py>,
    expected: &GenerationField,
    actual: &Bound<'py, PyAny>,
) -> PyResult<bool> {
    Ok(project_field(
        py,
        catalog,
        references,
        actual,
        &expected.name,
        expected.role,
        expected.kw_only,
        true,
    )?
    .is_some_and(|values| values.matches(expected)))
}

fn project_field<'py>(
    py: Python<'py>,
    catalog: &HelperCatalog,
    references: &impl References<'py>,
    actual: &Bound<'py, PyAny>,
    name: &str,
    role: FieldRole,
    default_keyword_only: bool,
    resolved: bool,
) -> PyResult<Option<FieldValues>> {
    let member = |name| catalog.member(py, references, actual, StructType::Field, name);
    if !catalog.matches_structure(py, references, StructType::Field, actual.as_ptr())? {
        return Ok(None);
    }
    if resolved {
        let Some(actual_name) = member("name")? else {
            return Ok(None);
        };
        let Some(kind) = member("_field_type")? else {
            return Ok(None);
        };
        let expected_kind = match role {
            FieldRole::Instance => Sentinel::Field,
            FieldRole::InitOnly => Sentinel::InitVar,
            FieldRole::ClassVariable => Sentinel::ClassVar,
        };
        if !unsafe { text_is(actual_name.as_ptr(), name) }
            || !catalog.matches_sentinel(py, references, expected_kind, kind.as_ptr())?
        {
            return Ok(None);
        }
    }
    let (Some(default), Some(factory)) = (member("default")?, member("default_factory")?) else {
        return Ok(None);
    };
    let has_default =
        !catalog.matches_sentinel(py, references, Sentinel::Missing, default.as_ptr())?;
    let has_factory =
        !catalog.matches_sentinel(py, references, Sentinel::Missing, factory.as_ptr())?;
    if has_default && has_factory {
        return Ok(None);
    }
    let mut flags = [false; 3];
    for (slot, name) in flags.iter_mut().zip(["init", "repr", "compare"]) {
        let Some(value) = member(name)? else {
            return Ok(None);
        };
        let Some(value) = exact_bool(value.as_ptr()) else {
            return Ok(None);
        };
        *slot = value;
    }
    let Some(hash) = member("hash")? else {
        return Ok(None);
    };
    let hash = if hash.is_none() {
        None
    } else {
        let Some(value) = exact_bool(hash.as_ptr()) else {
            return Ok(None);
        };
        Some(value)
    };
    let Some(keyword_only) = member("kw_only")? else {
        return Ok(None);
    };
    let keyword_only =
        if catalog.matches_sentinel(py, references, Sentinel::Missing, keyword_only.as_ptr())? {
            if role == FieldRole::ClassVariable {
                false
            } else if resolved {
                return Ok(None);
            } else {
                default_keyword_only
            }
        } else {
            let Some(value) = exact_bool(keyword_only.as_ptr()) else {
                return Ok(None);
            };
            value
        };
    Ok(Some(FieldValues {
        default: if has_factory {
            DefaultKind::Factory
        } else if has_default {
            DefaultKind::Value
        } else {
            DefaultKind::Missing
        },
        init: flags[0],
        repr: flags[1],
        compare: flags[2],
        hash,
        kw_only: keyword_only,
    }))
}

/// Actual ordinary generator operands, not a field-binding capability. The
/// contributor is the native MRO entry whose resolved mapping wins the
/// stdlib's reversed-MRO overwrite; mapping_owner owns that actual dictionary.
pub(super) struct InheritedField<'py> {
    pub(super) contributor: Bound<'py, PyAny>,
    pub(super) value: Bound<'py, PyAny>,
    mapping_owner: Bound<'py, PyAny>,
}

pub(super) fn inherited_field<'py>(
    py: Python<'py>,
    bases: &Bound<'py, PyTuple>,
    name: &str,
) -> PyResult<Option<InheritedField<'py>>> {
    let Some(mro) = prospective_mro(bases) else {
        return Ok(None);
    };
    let Some(selected) = lookup_mro_field(py, &mro, name) else {
        return Ok(None);
    };
    let Some(contributor) = crate::strict_class_state::for_actual_type(py, &selected.contributor)?
    else {
        return Ok(None);
    };
    let Some(mapping_owner) =
        crate::strict_class_state::for_actual_type(py, &selected.mapping_owner)?
    else {
        return Ok(None);
    };
    // Apply can be complete while the enclosing source module still awaits
    // sealing. Such a base has an installed class/field contract but grants no
    // finalized method/layout optimization capability here.
    if contributor.pending_dataclass() || mapping_owner.pending_dataclass() {
        return Ok(None);
    }
    let Some(proof) = mapping_owner.dataclass_namespace()? else {
        return Ok(None);
    };
    let dictionary =
        unsafe { (*selected.mapping_owner.as_ptr().cast::<ffi::PyTypeObject>()).tp_dict };
    if dictionary.is_null() || unsafe { ffi::PyDict_CheckExact(dictionary) } == 0 {
        return Ok(None);
    }
    let dictionary =
        unsafe { Bound::<PyAny>::from_borrowed_ptr(py, dictionary) }.cast_into::<PyDict>()?;
    if !proof.validate(
        &dictionary,
        crate::strict_class::ClassNamespacePhase::Adopted,
    )? {
        return Ok(None);
    }
    Ok(Some(selected))
}

/// C3 over the already-existing native base MROs. This does not allocate a
/// Python class, call a metaclass, or evaluate an attribute. Inconsistent or
/// non-native inputs decline before the ordinary construction is attempted.
pub(super) fn prospective_mro<'py>(bases: &Bound<'py, PyTuple>) -> Option<Vec<Bound<'py, PyAny>>> {
    let mut sequences = Vec::with_capacity(bases.len() + 1);
    let mut direct = Vec::with_capacity(bases.len());
    for base in bases.iter() {
        if unsafe { ffi::PyType_Check(base.as_ptr()) } == 0
            || direct
                .iter()
                .any(|previous: &Bound<'py, PyAny>| previous.as_ptr() == base.as_ptr())
        {
            return None;
        }
        let mro = unsafe { (*base.as_ptr().cast::<ffi::PyTypeObject>()).tp_mro };
        if mro.is_null() || unsafe { ffi::PyTuple_CheckExact(mro) } == 0 {
            return None;
        }
        let mut sequence = Vec::with_capacity(unsafe { ffi::PyTuple_Size(mro) } as usize);
        for index in 0..unsafe { ffi::PyTuple_Size(mro) } {
            let actual = unsafe { ffi::PyTuple_GetItem(mro, index) };
            if unsafe { ffi::PyType_Check(actual) } == 0 {
                return None;
            }
            sequence.push(unsafe { Bound::<PyAny>::from_borrowed_ptr(bases.py(), actual) });
        }
        if sequence
            .first()
            .is_none_or(|head| head.as_ptr() != base.as_ptr())
        {
            return None;
        }
        direct.push(base);
        sequences.push(sequence);
    }
    sequences.push(direct);
    let mut positions = vec![0; sequences.len()];
    let mut result = Vec::new();
    loop {
        let mut remaining = false;
        let mut next = None;
        for (sequence, &position) in sequences.iter().zip(&positions) {
            let Some(head) = sequence.get(position) else {
                continue;
            };
            remaining = true;
            if sequences.iter().zip(&positions).all(|(other, &start)| {
                !other
                    .iter()
                    .skip(start + 1)
                    .any(|tail| tail.as_ptr() == head.as_ptr())
            }) {
                next = Some(head.clone());
                break;
            }
        }
        let Some(next) = next else {
            return (!remaining).then_some(result);
        };
        for (sequence, position) in sequences.iter().zip(&mut positions) {
            if sequence
                .get(*position)
                .is_some_and(|head| head.as_ptr() == next.as_ptr())
            {
                *position += 1;
            }
        }
        result.push(next);
    }
}

fn lookup_mro_field<'py>(
    py: Python<'py>,
    mro: &[Bound<'py, PyAny>],
    name: &str,
) -> Option<InheritedField<'py>> {
    for contributor in mro {
        let ancestors = unsafe { (*contributor.as_ptr().cast::<ffi::PyTypeObject>()).tp_mro };
        if ancestors.is_null() || unsafe { ffi::PyTuple_CheckExact(ancestors) } == 0 {
            return None;
        }
        for index in 0..unsafe { ffi::PyTuple_Size(ancestors) } {
            let class = unsafe { ffi::PyTuple_GetItem(ancestors, index) };
            if unsafe { ffi::PyType_Check(class) } == 0 {
                return None;
            }
            let dictionary = unsafe { (*class.cast::<ffi::PyTypeObject>()).tp_dict };
            let Some(fields) = (unsafe { dictionary_value(dictionary, "__dataclass_fields__") })
            else {
                continue;
            };
            if fields == unsafe { ffi::Py_None() } {
                break;
            }
            if unsafe { ffi::PyDict_CheckExact(fields) } == 0 {
                return None;
            }
            if let Some(value) = unsafe { dictionary_value(fields, name) } {
                return Some(InheritedField {
                    contributor: contributor.clone(),
                    mapping_owner: unsafe { Bound::from_borrowed_ptr(py, class) },
                    value: unsafe { Bound::from_borrowed_ptr(py, value) },
                });
            }
            // getattr stops at the first actual mapping even when the field
            // is absent there. The next outer native MRO entry may contribute it.
            break;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use pyo3::types::PyModule;

    use super::*;

    #[test]
    fn dataclass_inherited_field_projection_uses_actual_c3_and_mapping_contributor() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let source = PyModule::from_code(
                py,
                c"
from dataclasses import dataclass
@dataclass
class Root:
    value: int = 1
@dataclass
class Left(Root):
    pass
@dataclass
class Right:
    value: int = 2
del Left.__dataclass_fields__['value']
class Intermediate(Left):
    pass
@dataclass
class Combined(Intermediate, Right):
    pass
",
                c"<dataclass actual MRO>",
                c"dataclass_actual_mro",
            )
            .unwrap();
            let actual = source.getattr("Combined").unwrap();
            let bases = actual
                .getattr("__bases__")
                .unwrap()
                .cast_into::<PyTuple>()
                .unwrap();
            let expected = actual
                .getattr("__mro__")
                .unwrap()
                .cast_into::<PyTuple>()
                .unwrap();
            let projected = prospective_mro(&bases).unwrap();
            assert_eq!(projected.len() + 1, expected.len());
            for (actual, expected) in projected.iter().zip(expected.iter().skip(1)) {
                assert_eq!(actual.as_ptr(), expected.as_ptr());
            }
            let selected = lookup_mro_field(py, &projected, "value").unwrap();
            let root = source.getattr("Root").unwrap();
            assert_eq!(selected.contributor.as_ptr(), root.as_ptr());
            assert_eq!(selected.mapping_owner.as_ptr(), root.as_ptr());
            assert_eq!(
                selected.value.as_ptr(),
                actual
                    .getattr("__dataclass_fields__")
                    .unwrap()
                    .get_item("value")
                    .unwrap()
                    .as_ptr()
            );
            let left = source.getattr("Left").unwrap();
            assert!(prospective_mro(&PyTuple::new(py, [&root, &left]).unwrap()).is_none());
            assert!(prospective_mro(&PyTuple::new(py, [&left, &left]).unwrap()).is_none());
        });
    }

    #[test]
    fn dataclass_field_projection_checks_exact_flags_without_truth_callbacks() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = super::super::StdlibRecipes::load(py).unwrap();
            let dataclasses = PyModule::import(py, "dataclasses").unwrap();
            let root = dataclasses.getattr("dataclass").unwrap();
            let captured = HelperCatalog::capture(py, &root, &recipes)
                .unwrap()
                .unwrap();
            let source = PyModule::from_code(
                py,
                c"
from dataclasses import dataclass, field
events = []
@dataclass
class Example:
    value: int = field(default=1, init=False, repr=False, compare=False, hash=True, kw_only=True)
class TruthTrap:
    def __bool__(self):
        events.append('bool')
        return True
actual = Example.__dataclass_fields__['value']
trap = TruthTrap()
",
                c"<dataclass field projection>",
                c"dataclass_field_projection",
            )
            .unwrap();
            let actual = source.getattr("actual").unwrap();
            let expected = GenerationField {
                name: "value".into(),
                role: FieldRole::Instance,
                default: DefaultKind::Value,
                init: false,
                kw_only: true,
                repr: false,
                compare: false,
                hash: Some(true),
            };
            assert!(
                matches_field(
                    py,
                    &captured.catalog,
                    &captured.references,
                    &expected,
                    &actual
                )
                .unwrap()
            );
            for name in ["init", "repr", "compare", "hash", "kw_only"] {
                let previous = actual.getattr(name).unwrap();
                actual
                    .setattr(name, source.getattr("trap").unwrap())
                    .unwrap();
                let matched = matches_field(
                    py,
                    &captured.catalog,
                    &captured.references,
                    &expected,
                    &actual,
                );
                actual.setattr(name, previous).unwrap();
                assert!(!matched.unwrap());
            }
            assert_eq!(source.getattr("events").unwrap().len().unwrap(), 0);
        });
    }

    #[test]
    fn dataclass_field_projection_distinguishes_unresolved_and_resolved_keyword_defaults() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let recipes = super::super::StdlibRecipes::load(py).unwrap();
            let dataclasses = PyModule::import(py, "dataclasses").unwrap();
            let root = dataclasses.getattr("dataclass").unwrap();
            let captured = HelperCatalog::capture(py, &root, &recipes)
                .unwrap()
                .unwrap();
            let field = dataclasses.getattr("field").unwrap().call0().unwrap();
            let project = |role, resolved| {
                project_field(
                    py,
                    &captured.catalog,
                    &captured.references,
                    &field,
                    "value",
                    role,
                    true,
                    resolved,
                )
            };
            assert!(
                project(FieldRole::Instance, false)
                    .unwrap()
                    .unwrap()
                    .kw_only
            );
            assert!(
                !project(FieldRole::ClassVariable, false)
                    .unwrap()
                    .unwrap()
                    .kw_only
            );
            field.setattr("name", "value").unwrap();
            field
                .setattr("_field_type", dataclasses.getattr("_FIELD").unwrap())
                .unwrap();
            // MISSING is only a pre-generation default. A resolved instance
            // field must carry the generator's actual bool, not an inferred one.
            assert!(project(FieldRole::Instance, true).unwrap().is_none());
            field.setattr("kw_only", false).unwrap();
            assert!(!project(FieldRole::Instance, true).unwrap().unwrap().kw_only);
        });
    }
}
