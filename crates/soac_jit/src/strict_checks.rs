//! Shared field-value predicates and independently checked optimization guards.
//!
//! Source annotations do not constrain function arguments or return values.
//! Required field policies supply actual resolved nominal targets; optional
//! guards establish only the point-in-time value predicate they execute.

use std::ffi::CString;
use std::ptr::{self, NonNull};

use pyo3::exceptions::PyRuntimeError;
use pyo3::ffi;
use pyo3::prelude::*;
use soac_contracts::{BuiltinType, ClassReference, LiteralValue, StaticType};

/// A field policy or independent guard owns actual resolved targets in
/// GC-traversed state. This lookup only visits borrowed targets; the predicate
/// never caches or owns them. A field resolver is closed over its actual policy
/// and exact authenticated FieldReference.
///
/// # Safety
/// Each visited pointer must be a live type in `py`'s interpreter for the
/// duration of the check and identify the actual authenticated class for this
/// lexical execution. Matching source names alone is insufficient: distinct
/// factory executions can create distinct classes. Resolution must not run
/// Python code, evaluate annotations, or invoke overridable membership hooks.
/// Persistent owning references must remain in GC-traversed native state.
pub unsafe trait StrictNominalTypeResolver {
    /// Return true only when every required source leaf for this target/class
    /// is resolved. An unresolved leaf must not silently remove a union member.
    /// The visitor supplied by a value check is native and callback-free.
    fn visit_targets(
        &self,
        py: Python<'_>,
        class: &ClassReference,
        visitor: &mut dyn FnMut(NonNull<ffi::PyTypeObject>),
    ) -> PyResult<bool>;
}

/// Explicitly unresolved nominal targets; required checks fail closed while
/// optional guards decline. No name-based runtime lookup is substituted.
pub struct UnresolvedStrictNominalTypes;

// SAFETY: This implementation never returns or retains a Python pointer.
unsafe impl StrictNominalTypeResolver for UnresolvedStrictNominalTypes {
    fn visit_targets(
        &self,
        _py: Python<'_>,
        _class: &ClassReference,
        _visitor: &mut dyn FnMut(NonNull<ffi::PyTypeObject>),
    ) -> PyResult<bool> {
        Ok(false)
    }
}

/// Exactness is produced only by an exact predicate. None of these variants
/// establishes machine-width range, a strict receiver, storage, or dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrictValueAcceptance {
    NominalTypeAccepted,
    ExactBuiltinTypeAccepted,
    ExactClassTypeAccepted,
    SingletonAccepted,
    LiteralValueAccepted,
}

#[derive(Debug)]
pub struct StrictValueProof<'contract> {
    value_type: &'contract StaticType,
    acceptance: StrictValueAcceptance,
}

impl StrictValueProof<'_> {
    pub fn value_type(&self) -> &StaticType {
        self.value_type
    }
    pub fn acceptance(&self) -> StrictValueAcceptance {
        self.acceptance
    }
}

fn guard_supported(value_type: &StaticType, depth: usize) -> bool {
    if depth > 64 {
        return false;
    }
    match value_type {
        StaticType::NumericWidening { target, accepted } => match target {
            BuiltinType::Float => accepted
                .iter()
                .copied()
                .eq([BuiltinType::Int, BuiltinType::Float]),
            BuiltinType::Complex => accepted.iter().copied().eq([
                BuiltinType::Int,
                BuiltinType::Float,
                BuiltinType::Complex,
            ]),
            _ => false,
        },
        StaticType::Literal(LiteralValue::FloatBits(_)) => false,
        StaticType::Literal(_) => true,
        StaticType::Union(elements) => {
            !elements.is_empty()
                && elements
                    .iter()
                    .all(|element| guard_supported(element, depth + 1))
        }
        StaticType::Optional(element) => guard_supported(element, depth + 1),
        _ => value_type.has_supported_value_shape(),
    }
}

/// An optional optimization predicate. Unsupported/unresolved contracts and
/// ordinary mismatches return `None`, not a mandatory-language `TypeError`.
/// Genuine native/resolver errors propagate. Passing records only this
/// point-in-time predicate, not an immutable fact about a mutable receiver.
pub fn strict_value_guard<'contract>(
    value: &Bound<'_, PyAny>,
    value_type: &'contract StaticType,
    resolver: &(impl StrictNominalTypeResolver + ?Sized),
) -> PyResult<Option<StrictValueProof<'contract>>> {
    unsafe { guard_value(value.py(), value.as_ptr(), value_type, resolver) }
}

fn targets_resolved(
    py: Python<'_>,
    value_type: &StaticType,
    resolver: &(impl StrictNominalTypeResolver + ?Sized),
) -> PyResult<bool> {
    match value_type {
        StaticType::NominalClass(class) | StaticType::ExactClass(class) => {
            resolver.visit_targets(py, class, &mut |_| {})
        }
        StaticType::Union(elements) => {
            for element in elements {
                if !targets_resolved(py, element, resolver)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        StaticType::Optional(element) => targets_resolved(py, element, resolver),
        _ => Ok(true),
    }
}

unsafe fn guard_value<'contract>(
    py: Python<'_>,
    value: *mut ffi::PyObject,
    value_type: &'contract StaticType,
    resolver: &(impl StrictNominalTypeResolver + ?Sized),
) -> PyResult<Option<StrictValueProof<'contract>>> {
    if !guard_supported(value_type, 0) || !targets_resolved(py, value_type, resolver)? {
        return Ok(None);
    }
    unsafe { matches_value(py, value, value_type, resolver) }.map(|accepted| {
        accepted.map(|acceptance| StrictValueProof {
            value_type,
            acceptance,
        })
    })
}

fn builtin_type(builtin: BuiltinType) -> *mut ffi::PyTypeObject {
    match builtin {
        BuiltinType::Object => ptr::addr_of_mut!(ffi::PyBaseObject_Type),
        BuiltinType::Bool => ptr::addr_of_mut!(ffi::PyBool_Type),
        BuiltinType::Int => ptr::addr_of_mut!(ffi::PyLong_Type),
        BuiltinType::Float => ptr::addr_of_mut!(ffi::PyFloat_Type),
        BuiltinType::Complex => ptr::addr_of_mut!(ffi::PyComplex_Type),
        BuiltinType::Str => ptr::addr_of_mut!(ffi::PyUnicode_Type),
        BuiltinType::Bytes => ptr::addr_of_mut!(ffi::PyBytes_Type),
        BuiltinType::ByteArray => ptr::addr_of_mut!(ffi::PyByteArray_Type),
        BuiltinType::Tuple => ptr::addr_of_mut!(ffi::PyTuple_Type),
        BuiltinType::List => ptr::addr_of_mut!(ffi::PyList_Type),
        BuiltinType::Dict => ptr::addr_of_mut!(ffi::PyDict_Type),
        BuiltinType::Set => ptr::addr_of_mut!(ffi::PySet_Type),
        BuiltinType::FrozenSet => ptr::addr_of_mut!(ffi::PyFrozenSet_Type),
        BuiltinType::Type => ptr::addr_of_mut!(ffi::PyType_Type),
    }
}

unsafe fn is_type(
    value: *mut ffi::PyObject,
    expected: *mut ffi::PyTypeObject,
    subclasses: bool,
) -> bool {
    let actual = unsafe { ffi::Py_TYPE(value) };
    actual == expected || (subclasses && unsafe { ffi::PyType_IsSubtype(actual, expected) } != 0)
}

unsafe fn matches_value(
    py: Python<'_>,
    value: *mut ffi::PyObject,
    value_type: &StaticType,
    resolver: &(impl StrictNominalTypeResolver + ?Sized),
) -> PyResult<Option<StrictValueAcceptance>> {
    use StrictValueAcceptance as A;
    let result = match value_type {
        StaticType::None => (value == unsafe { ffi::Py_None() }).then_some(A::SingletonAccepted),
        StaticType::ExactBuiltin(builtin) => {
            unsafe { is_type(value, builtin_type(*builtin), false) }
                .then_some(A::ExactBuiltinTypeAccepted)
        }
        StaticType::NominalBuiltin {
            builtin,
            allow_subclasses,
        } => unsafe { is_type(value, builtin_type(*builtin), *allow_subclasses) }.then_some(
            if *allow_subclasses {
                A::NominalTypeAccepted
            } else {
                A::ExactBuiltinTypeAccepted
            },
        ),
        StaticType::NumericWidening { accepted, .. } => accepted
            .iter()
            .any(|builtin| unsafe { is_type(value, builtin_type(*builtin), true) })
            .then_some(A::NominalTypeAccepted),
        StaticType::NominalClass(class) | StaticType::ExactClass(class) => {
            let exact = matches!(value_type, StaticType::ExactClass(_));
            let mut accepted = false;
            let mut first_target = None;
            let mut multiple_targets = false;
            let complete = resolver.visit_targets(py, class, &mut |expected| {
                match first_target {
                    None => first_target = Some(expected),
                    Some(first) => multiple_targets |= first != expected,
                }
                accepted |= unsafe { is_type(value, expected.as_ptr(), !exact) };
            })?;
            if !complete {
                return Ok(None);
            }
            accepted.then_some(if exact && !multiple_targets {
                A::ExactClassTypeAccepted
            } else {
                A::NominalTypeAccepted
            })
        }
        StaticType::Optional(element) => (value == unsafe { ffi::Py_None() }
            || unsafe { matches_value(py, value, element, resolver) }?.is_some())
        .then_some(A::NominalTypeAccepted),
        StaticType::Union(elements) => {
            for element in elements {
                if unsafe { matches_value(py, value, element, resolver) }?.is_some() {
                    // The union contract does not promote a matching alternative
                    // into an exact representation or participating receiver.
                    return Ok(Some(A::NominalTypeAccepted));
                }
            }
            None
        }
        StaticType::Literal(literal) => {
            unsafe { literal_matches(py, value, literal) }?.then_some(A::LiteralValueAccepted)
        }
        _ => None,
    };
    Ok(result)
}

// Convert a trusted decimal integer constant to hexadecimal before calling
// CPython. Base-16 parsing avoids dependence on mutable int_max_str_digits and
// neither coerces the checked value nor evaluates Python annotation code.
fn integer_hex(decimal: &str) -> PyResult<CString> {
    let (negative, digits) = decimal
        .strip_prefix('-')
        .map_or((false, decimal), |digits| (true, digits));
    if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(PyRuntimeError::new_err(
            "invalid integer in a strict literal predicate",
        ));
    }
    let mut magnitude = vec![0u8];
    for digit in digits.bytes() {
        let mut carry = u16::from(digit - b'0');
        for byte in &mut magnitude {
            let expanded = u16::from(*byte) * 10 + carry;
            *byte = expanded as u8;
            carry = expanded >> 8;
        }
        if carry != 0 {
            magnitude.push(carry as u8);
        }
    }
    let mut encoded = if negative {
        "-0x".to_owned()
    } else {
        "0x".to_owned()
    };
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in magnitude.iter().rev() {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 15)]));
    }
    Ok(CString::new(encoded).expect("hexadecimal digits contain no NUL"))
}

unsafe fn literal_matches(
    py: Python<'_>,
    value: *mut ffi::PyObject,
    literal: &LiteralValue,
) -> PyResult<bool> {
    match literal {
        LiteralValue::None => Ok(value == unsafe { ffi::Py_None() }),
        LiteralValue::Bool(expected) => Ok(value
            == unsafe {
                if *expected {
                    ffi::Py_True()
                } else {
                    ffi::Py_False()
                }
            }),
        LiteralValue::Int(decimal) => {
            if !unsafe { is_type(value, builtin_type(BuiltinType::Int), false) } {
                return Ok(false);
            }
            let text = integer_hex(decimal)?;
            let expected: Bound<'_, PyAny> = unsafe {
                Bound::from_owned_ptr_or_err(
                    py,
                    ffi::PyLong_FromString(text.as_ptr(), ptr::null_mut(), 16),
                )?
            };
            let comparison =
                unsafe { ffi::PyObject_RichCompareBool(value, expected.as_ptr(), ffi::Py_EQ) };
            if comparison < 0 {
                Err(PyErr::fetch(py))
            } else {
                Ok(comparison != 0)
            }
        }
        LiteralValue::Str(expected) => {
            if !unsafe { is_type(value, builtin_type(BuiltinType::Str), false) } {
                return Ok(false);
            }
            let length = unsafe { ffi::PyUnicode_GetLength(value) };
            if length < 0 {
                return Err(PyErr::fetch(py));
            }
            if length as usize != expected.chars().count() {
                return Ok(false);
            }
            for (index, expected) in expected.chars().enumerate() {
                let actual = unsafe { ffi::PyUnicode_ReadChar(value, index as ffi::Py_ssize_t) };
                if actual == u32::MAX {
                    return Err(PyErr::fetch(py));
                }
                if actual != u32::from(expected) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        LiteralValue::Bytes(expected) => {
            if !unsafe { is_type(value, builtin_type(BuiltinType::Bytes), false) } {
                return Ok(false);
            }
            let mut bytes = ptr::null_mut();
            let mut size = 0;
            if unsafe { ffi::PyBytes_AsStringAndSize(value, &mut bytes, &mut size) } != 0 {
                return Err(PyErr::fetch(py));
            }
            Ok(
                unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), size as usize) }
                    == expected,
            )
        }
        LiteralValue::FloatBits(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::exceptions::PyValueError;
    use pyo3::types::{PyDict, PyDictMethods};
    use soac_contracts::{
        DefinitionKind, Fingerprint, ModuleContentId, SourceIdentity, SourceRange,
        legacy_source_hash,
    };
    use std::collections::BTreeSet;

    const EXTERNAL: &[u8] = b"class Declared:\n    pass\n";

    fn nominal(builtin: BuiltinType) -> StaticType {
        StaticType::NominalBuiltin {
            builtin,
            allow_subclasses: true,
        }
    }

    fn widened_float() -> StaticType {
        StaticType::NumericWidening {
            target: BuiltinType::Float,
            accepted: BTreeSet::from([BuiltinType::Int, BuiltinType::Float]),
        }
    }

    fn class_reference() -> ClassReference {
        ClassReference {
            definition: SourceIdentity {
                module: ModuleContentId::new("external", legacy_source_hash(EXTERNAL)),
                lexical_qualname: "Declared".into(),
                source_range: SourceRange::new(0, EXTERNAL.len() as u32),
                definition_kind: DefinitionKind::Class,
            },
            source_digest: Fingerprint::digest(EXTERNAL),
        }
    }

    fn native_lock() -> std::sync::MutexGuard<'static, ()> {
        let lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        lock
    }

    #[test]
    fn native_membership_does_not_call_python_coercion_or_equality_hooks() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let globals = PyDict::new(py);
            py.run(c"events = []\nclass Integer(int):\n def __int__(self): events.append('int'); raise AssertionError\n def __eq__(self, other): events.append('eq'); raise AssertionError\nclass Floating(float):\n def __float__(self): events.append('float'); raise AssertionError\n", Some(&globals), None)?;
            for expression in [c"True", c"Integer(3)", c"Floating(2.0)", c"10 ** 100"] {
                let value = py.eval(expression, Some(&globals), None)?;
                let contract = widened_float();
                assert_eq!(
                    strict_value_guard(&value, &contract, &UnresolvedStrictNominalTypes)?
                        .unwrap()
                        .acceptance(),
                    StrictValueAcceptance::NominalTypeAccepted
                );
            }
            let subclass = py.eval(c"Integer(1)", Some(&globals), None)?;
            assert!(
                strict_value_guard(
                    &subclass,
                    &StaticType::ExactBuiltin(BuiltinType::Int),
                    &UnresolvedStrictNominalTypes
                )?
                .is_none()
            );
            assert!(
                strict_value_guard(
                    &subclass,
                    &StaticType::Literal(LiteralValue::Int("1".into())),
                    &UnresolvedStrictNominalTypes
                )?
                .is_none()
            );
            assert_eq!(globals.get_item("events")?.unwrap().len()?, 0);
            Ok(())
        })
    }

    struct NominalFixture {
        reference: ClassReference,
        targets: Vec<NonNull<ffi::PyTypeObject>>,
    }

    // SAFETY: Tests bind one declared source to the live class held by their
    // Python globals dictionary, and retain that dictionary for every call.
    unsafe impl StrictNominalTypeResolver for NominalFixture {
        fn visit_targets(
            &self,
            _py: Python<'_>,
            class: &ClassReference,
            visitor: &mut dyn FnMut(NonNull<ffi::PyTypeObject>),
        ) -> PyResult<bool> {
            if class != &self.reference {
                return Ok(false);
            }
            for &target in &self.targets {
                visitor(target);
            }
            Ok(!self.targets.is_empty())
        }
    }

    #[test]
    fn nominal_targets_require_actual_class_identity_and_do_not_grant_layout_proofs() -> PyResult<()>
    {
        let _lock = native_lock();
        Python::attach(|py| {
            let globals = PyDict::new(py);
            py.run(c"import abc\nevents = []\nclass Meta(abc.ABCMeta):\n def __instancecheck__(cls, value): events.append('instancecheck'); return True\nclass Declared(metaclass=Meta): pass\nclass Child(Declared): pass\nclass Virtual: pass\nDeclared.register(Virtual)\nclass Other: pass\nclass Spoof:\n @property\n def __class__(self): events.append('class'); return Declared\nevents.clear()\n", Some(&globals), None)?;
            let target = globals.get_item("Declared")?.unwrap();
            let resolver = NominalFixture {
                reference: class_reference(),
                targets: vec![NonNull::new(target.as_ptr().cast()).unwrap()],
            };
            let contract = StaticType::NominalClass(class_reference());
            let value = py.eval(c"Child()", Some(&globals), None)?;
            assert!(
                strict_value_guard(&value, &contract, &UnresolvedStrictNominalTypes)?.is_none()
            );
            assert_eq!(
                strict_value_guard(&value, &contract, &resolver)?
                    .unwrap()
                    .acceptance(),
                StrictValueAcceptance::NominalTypeAccepted
            );
            let exact = StaticType::ExactClass(class_reference());
            assert!(strict_value_guard(&value, &exact, &resolver)?.is_none());
            let own = py.eval(c"Declared()", Some(&globals), None)?;
            assert_eq!(
                strict_value_guard(&own, &exact, &resolver)?
                    .unwrap()
                    .acceptance(),
                StrictValueAcceptance::ExactClassTypeAccepted
            );
            let other = globals.get_item("Other")?.unwrap();
            let multiple = NominalFixture {
                reference: class_reference(),
                targets: vec![
                    NonNull::new(target.as_ptr().cast()).unwrap(),
                    NonNull::new(other.as_ptr().cast()).unwrap(),
                ],
            };
            // Type normalization can merge two same-source aliases. Accepting
            // either actual type must not claim one unique exact native type.
            assert_eq!(
                strict_value_guard(&own, &exact, &multiple)?
                    .unwrap()
                    .acceptance(),
                StrictValueAcceptance::NominalTypeAccepted
            );
            let spoof = py.eval(c"Spoof()", Some(&globals), None)?;
            assert!(strict_value_guard(&spoof, &contract, &resolver)?.is_none());
            let virtual_subclass = py.eval(c"Virtual()", Some(&globals), None)?;
            assert!(strict_value_guard(&virtual_subclass, &contract, &resolver)?.is_none());
            let moving = py.eval(c"Declared()", Some(&globals), None)?;
            assert!(strict_value_guard(&moving, &contract, &resolver)?.is_some());
            globals.set_item("moving", &moving)?;
            py.run(c"moving.__class__ = Other", Some(&globals), None)?;
            assert!(strict_value_guard(&moving, &contract, &resolver)?.is_none());
            assert_eq!(globals.get_item("events")?.unwrap().len()?, 0);
            Ok(())
        })
    }

    #[test]
    fn literal_guards_use_exact_builtin_values_without_bool_int_conflation() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            for (expression, literal) in [
                (c"True", LiteralValue::Bool(true)),
                (
                    c"-123456789012345678901234567890",
                    LiteralValue::Int("-123456789012345678901234567890".into()),
                ),
                (c"'café😀'", LiteralValue::Str("caf\u{e9}\u{1f600}".into())),
                (c"chr(0xFFFD)", LiteralValue::Str("�".into())),
                (c"r'\\ud800'", LiteralValue::Str(r"\ud800".into())),
                (c"bytes([0, 255])", LiteralValue::Bytes(vec![0, 255])),
                (c"None", LiteralValue::None),
            ] {
                let value = py.eval(expression, None, None)?;
                let contract = StaticType::Literal(literal);
                assert_eq!(
                    strict_value_guard(&value, &contract, &UnresolvedStrictNominalTypes)?
                        .unwrap()
                        .acceptance(),
                    StrictValueAcceptance::LiteralValueAccepted
                );
            }
            let surrogate = py.eval(c"chr(0xD800)", None, None)?;
            for expected in ["�", r"\ud800"] {
                assert!(
                    strict_value_guard(
                        &surrogate,
                        &StaticType::Literal(LiteralValue::Str(expected.into())),
                        &UnresolvedStrictNominalTypes,
                    )?
                    .is_none()
                );
            }
            let boolean = py.eval(c"True", None, None)?;
            assert!(
                strict_value_guard(
                    &boolean,
                    &StaticType::Literal(LiteralValue::Int("1".into())),
                    &UnresolvedStrictNominalTypes
                )?
                .is_none()
            );
            let integer = py.eval(c"1", None, None)?;
            assert!(
                strict_value_guard(
                    &integer,
                    &StaticType::Literal(LiteralValue::Bool(true)),
                    &UnresolvedStrictNominalTypes
                )?
                .is_none()
            );
            let floating = py.eval(c"1.0", None, None)?;
            assert!(
                strict_value_guard(
                    &floating,
                    &StaticType::Literal(LiteralValue::FloatBits(1.0f64.to_bits())),
                    &UnresolvedStrictNominalTypes
                )?
                .is_none()
            );
            Ok(())
        })
    }

    #[test]
    fn unions_remain_dynamic_if_any_member_is_unsupported_or_unresolved() -> PyResult<()> {
        let _lock = native_lock();
        Python::attach(|py| {
            let value = py.eval(c"1", None, None)?;
            for unsupported in [
                StaticType::Any,
                StaticType::Unknown,
                StaticType::StructuralProtocol(soac_contracts::ProtocolFact {
                    definition: None,
                    runtime_checkable: true,
                }),
                StaticType::NominalClass(class_reference()),
            ] {
                let contract = StaticType::Union(vec![nominal(BuiltinType::Int), unsupported]);
                assert!(
                    strict_value_guard(&value, &contract, &UnresolvedStrictNominalTypes)?.is_none()
                );
            }
            let contract = StaticType::Union(vec![StaticType::None, nominal(BuiltinType::Int)]);
            assert_eq!(
                strict_value_guard(&value, &contract, &UnresolvedStrictNominalTypes)?
                    .unwrap()
                    .acceptance(),
                StrictValueAcceptance::NominalTypeAccepted
            );
            let malformed = StaticType::NumericWidening {
                target: BuiltinType::Float,
                accepted: BTreeSet::from([BuiltinType::Str]),
            };
            assert!(
                strict_value_guard(&value, &malformed, &UnresolvedStrictNominalTypes)?.is_none()
            );
            struct Failed;
            // SAFETY: No pointer is returned; this models a native lookup error.
            unsafe impl StrictNominalTypeResolver for Failed {
                fn visit_targets(
                    &self,
                    _py: Python<'_>,
                    _class: &ClassReference,
                    _visitor: &mut dyn FnMut(NonNull<ffi::PyTypeObject>),
                ) -> PyResult<bool> {
                    Err(PyValueError::new_err("native lookup failed"))
                }
            }
            assert!(
                strict_value_guard(
                    &value,
                    &StaticType::NominalClass(class_reference()),
                    &Failed
                )
                .err()
                .unwrap()
                .is_instance_of::<PyValueError>(py)
            );
            Ok(())
        })
    }
}
