//! Mechanical consumers of the explicit conditional-annotation operations.
//! Neither these helpers nor the resulting ordinary set grant source authority.

use std::collections::HashSet;
use std::ffi::c_int;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyFrozenSet, PyTuple};
use soac_core::block_py::{
    AnnotationProviderKind, CallableSourceRole, ParamKind, TypeParameterKind,
    TypeParameterScopeInputKind,
};

use crate::strict_function::{
    AuthenticatedStrictFunction, authenticate_borrowed_strict_function,
    authenticate_strict_function,
};
use crate::strict_interpreter_source::InterpreterAnnotationCaptureOrigin;
use crate::strict_runtime_unavailable;

type ReplayResolver =
    unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject, c_int) -> *mut ffi::PyObject;

unsafe extern "C" {
    fn PySoac_SetupAnnotations(locals: *mut ffi::PyObject) -> c_int;
    fn PySoac_SetAnnotationReplayResolver(resolver: ReplayResolver) -> c_int;
    fn PySoac_CloneAnnotationReplayCode(
        provider: *mut ffi::PyObject,
        expected_owner: *mut ffi::PyObject,
        verified_code: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_NewTypeAlias(
        name: *mut ffi::PyObject,
        parameters: *mut ffi::PyObject,
        evaluator: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_NewTypeParameter(
        kind: c_int,
        name: *mut ffi::PyObject,
        evaluator: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_SetTypeParameterDefault(
        parameter: *mut ffi::PyObject,
        evaluator: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_MatchesTypeExpression(
        target: *mut ffi::PyObject,
        kind: c_int,
        evaluator: *mut ffi::PyObject,
    ) -> c_int;
    fn PySoac_SubscriptGeneric(parameters: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PySoac_SetFunctionTypeParameters(
        function: *mut ffi::PyObject,
        parameters: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
}

/// Install the one interpreter-owned callback at native extension initialization.
/// Repeated initialization is accepted only for this exact function pointer;
/// Python module attributes and other interpreters cannot replace it.
pub fn initialize_strict_runtime(py: Python<'_>) -> PyResult<()> {
    if unsafe { PySoac_SetAnnotationReplayResolver(annotation_replay_code) } < 0 {
        Err(PyErr::fetch(py))
    } else {
        Ok(())
    }
}

unsafe extern "C" fn annotation_replay_code(
    provider: *mut ffi::PyObject,
    _logical_owner: *mut ffi::PyObject,
    format: c_int,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<Bound<'_, PyAny>> {
        if provider.is_null() || !matches!(format, 3 | 4) {
            return Err(strict_runtime_unavailable(
                py,
                "invalid native annotation replay request",
            ));
        }
        // Native annotationlib supports the actual provider throughout this
        // callback. Do not add a second function edge for common authentication.
        let provider = unsafe { Borrowed::<PyAny>::from_ptr(py, provider) };
        let auth = authenticate_borrowed_strict_function(py, provider)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "annotation replay needs its actual native source owner")
        })?;
        let raw = provider.as_ptr().cast::<ffi::PyFunctionObject>();
        let code = if auth.is_interpreter() {
            let actual = unsafe { Borrowed::<PyAny>::from_ptr(py, (*raw).func_code) };
            if !auth.interpreter_source_authority()?
                || !auth.is_original_interpreter_entry(actual.as_ptr())?
                || auth.native_source()?.code(py, &actual)?.ordinal()
                    != auth.native_code_ordinal()?
            {
                return Err(strict_runtime_unavailable(
                    py,
                    "native annotation replay lost its original code",
                ));
            }
            actual.as_ptr()
        } else {
            auth.module_state()?
                .lookup_original_code(auth.function_id()?)
                .ok_or_else(|| {
                    strict_runtime_unavailable(py, "annotation replay original code is absent")
                })?
                .as_ptr()
        };
        let closure = unsafe { (*raw).func_closure };
        // Keep the original tuple live across allocations/code watchers, so
        // pointer equality cannot accept an ABA replacement of a mutable
        // callback's closure. No closure contents are frozen or copied.
        let _closure_pin =
            (!closure.is_null()).then(|| unsafe { Bound::<PyAny>::from_borrowed_ptr(py, closure) });
        validate_annotation_replay_source(&auth)?;
        revalidate_annotation_replay_function(&auth, code, closure)?;
        // The logical owner is annotationlib context, including legitimate
        // owner=None calls. It neither grants nor replaces provider authority.
        let replay = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                PySoac_CloneAnnotationReplayCode(provider.as_ptr(), auth.owner().as_ptr(), code),
            )
        }?;
        // Code allocation notifies native watchers. Native code also rechecks
        // code/owner; reauthenticate the Rust source and exact closure here.
        revalidate_annotation_replay_function(&auth, code, closure)?;
        Ok(replay)
    }));
    match result {
        Ok(Ok(code)) => code.into_ptr(),
        Ok(Err(error)) => {
            error.restore(py);
            ptr::null_mut()
        }
        Err(_) => {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_SystemError,
                    c"panic in strict annotation replay resolver".as_ptr(),
                )
            };
            ptr::null_mut()
        }
    }
}

fn revalidate_annotation_replay_function(
    original: &AuthenticatedStrictFunction<'_, '_>,
    code: *mut ffi::PyObject,
    closure: *mut ffi::PyObject,
) -> PyResult<()> {
    let function = original.function();
    let py = function.py();
    let current =
        authenticate_borrowed_strict_function(py, function.as_borrowed())?.ok_or_else(|| {
            strict_runtime_unavailable(py, "annotation callback lost its owner during replay")
        })?;
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    if current.owner().as_ptr() != original.owner().as_ptr()
        || unsafe { (*raw).func_code } != code
        || unsafe { (*raw).func_closure } != closure
    {
        return Err(strict_runtime_unavailable(
            py,
            "annotation callback changed during replay preparation",
        ));
    }
    Ok(())
}

/// The public annotationlib operation also accepts user-written callbacks.
/// Their ordinary replay code must preserve the source/closure restrictions
/// throughout the exact native tree. This is not admission
/// of a new source function, a copied owner, or foreign-globals JIT execution.
fn validate_annotation_replay_source(
    callback: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<()> {
    let py = callback.function().py();
    match callback.origin().map(|origin| origin.role) {
        Some(CallableSourceRole::AnnotationProvider) => {
            validated_annotation_capture_schema(callback)?;
        }
        Some(CallableSourceRole::SourceFunction) => {
            validate_source_callback_replay_tree(callback)?;
            validated_native_capture_layout(callback)?;
        }
        _ => {
            return Err(strict_runtime_unavailable(
                py,
                "annotation replay requires an authenticated provider or source callback",
            ));
        }
    }
    Ok(())
}

fn validate_source_callback_replay_tree(
    callback: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<()> {
    if callback.is_interpreter() {
        return validate_native_source_callback_replay_tree(callback);
    }
    let py = callback.function().py();
    let shared = callback.module_state()?;
    let root = shared
        .lookup_original_code(callback.function_id()?)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "annotation callback original code is absent")
        })?
        .bind(py);
    let mut pending = vec![root.clone()];
    let mut seen = HashSet::new();
    while let Some(value) = pending.pop() {
        let actual_type = unsafe { ffi::Py_TYPE(value.as_ptr()) };
        if actual_type != ptr::addr_of_mut!(ffi::PyCode_Type)
            && actual_type != ptr::addr_of_mut!(ffi::PyTuple_Type)
            && actual_type != ptr::addr_of_mut!(ffi::PyFrozenSet_Type)
        {
            continue;
        }
        if !seen.insert(value.as_ptr() as usize) {
            continue;
        }
        if actual_type == ptr::addr_of_mut!(ffi::PyCode_Type) {
            // Pointer correspondence to the independently authenticated native
            // catalogue, never code equality, names, or source-range guesses.
            let mut matches = shared
                .lowered_module
                .callable_defs
                .iter()
                .filter(|function| {
                    shared
                        .lookup_original_code(function.function_id)
                        .is_some_and(|original| original.as_ptr() == value.as_ptr())
                });
            let function = matches.next().ok_or_else(|| {
                strict_runtime_unavailable(
                    py,
                    "annotation replay contains an unrepresented callable or class body",
                )
            })?;
            if matches.next().is_some() || !shared.admits_function(function) {
                return Err(strict_runtime_unavailable(
                    py,
                    "annotation replay code does not have one authenticated source template",
                ));
            }
            match function
                .scope
                .source_origin
                .as_ref()
                .map(|origin| origin.role)
            {
                Some(
                    CallableSourceRole::SourceFunction | CallableSourceRole::AnnotationProvider,
                ) => (),
                _ => {
                    return Err(strict_runtime_unavailable(
                        py,
                        "annotation replay cannot derive a class or generic construction body",
                    ));
                }
            }
            pending.push(value.getattr("co_consts")?);
        } else if actual_type == ptr::addr_of_mut!(ffi::PyTuple_Type) {
            pending.extend(value.cast::<PyTuple>()?.iter());
        } else {
            pending.extend(value.cast::<PyFrozenSet>()?.iter());
        }
    }
    Ok(())
}

/// Validate the immutable actual native constant tree without cloning code,
/// tuple, or function owners. Success uses only borrowed exact-container reads
/// and Rust allocations. The following C clone transaction supplies its own
/// transient code pin before its first code-watcher callback.
fn validate_native_source_callback_replay_tree(
    callback: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<()> {
    use crate::strict_interpreter_source::InterpreterCodeRole;
    let py = callback.function().py();
    if !callback.interpreter_source_authority()? {
        return Err(strict_runtime_unavailable(
            py,
            "ordinary replacement is not an original replay source",
        ));
    }
    let raw = callback.function().as_ptr().cast::<ffi::PyFunctionObject>();
    let root = unsafe { (*raw).func_code };
    if !callback.is_original_interpreter_entry(root)? {
        return Err(strict_runtime_unavailable(
            py,
            "native replay source code changed",
        ));
    }
    let source = callback.native_source()?;
    let mut pending = vec![root];
    let mut seen = HashSet::new();
    while let Some(value) = pending.pop() {
        if !seen.insert(value as usize) {
            continue;
        }
        let actual_type = unsafe { ffi::Py_TYPE(value) };
        if actual_type == ptr::addr_of_mut!(ffi::PyCode_Type) {
            let code = unsafe { Borrowed::<PyAny>::from_ptr(py, value) };
            let selected = source.code(py, &code)?;
            match selected.role() {
                InterpreterCodeRole::SourceFunction
                | InterpreterCodeRole::AsyncSourceFunction
                | InterpreterCodeRole::AnnotationProvider => (),
                _ => {
                    return Err(strict_runtime_unavailable(
                        py,
                        "annotation replay cannot derive a class or generic construction body",
                    ));
                }
            }
            pending.push(unsafe { crate::code_view::view(py, value) }?.consts);
        } else if actual_type == ptr::addr_of_mut!(ffi::PyTuple_Type) {
            let len = unsafe { ffi::PyTuple_Size(value) };
            if len < 0 {
                return Err(PyErr::fetch(py));
            }
            for index in 0..len {
                let item = unsafe { ffi::PyTuple_GetItem(value, index) };
                if item.is_null() {
                    return Err(PyErr::fetch(py));
                }
                pending.push(item);
            }
        } else if actual_type == ptr::addr_of_mut!(ffi::PyFrozenSet_Type) {
            let mut position = 0;
            let mut item = ptr::null_mut();
            let mut hash = 0;
            while unsafe { ffi::_PySet_NextEntry(value, &mut position, &mut item, &mut hash) } != 0
            {
                pending.push(item);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnnotationCaptureKind {
    Lexical,
    ClassDictionary,
    ConditionalAnnotations,
    UnresolvedNativeRole,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AnnotationCaptureSlot {
    pub(crate) logical_name: String,
    pub(crate) cell_index: usize,
    pub(crate) kind: AnnotationCaptureKind,
    /// Some only for a proven native lexical carrier. None preserves the
    /// existing compiled projection, or accompanies a non-lexical native role.
    pub(crate) lexical_scope: Option<soac_contracts::SourceIdentity>,
}

impl AnnotationCaptureSlot {
    pub(crate) fn matches_lexical_binding(
        &self,
        name: &str,
        binding_scope: &soac_contracts::SourceIdentity,
    ) -> bool {
        self.kind == AnnotationCaptureKind::Lexical
            && self.logical_name == name
            && binding_scope.definition_kind == soac_contracts::DefinitionKind::Function
            && self
                .lexical_scope
                .as_ref()
                .is_none_or(|scope| scope == binding_scope)
    }
}

/// A value-only description of this authenticated provider's actual lexical
/// cells. Consumers still own the provider and must inspect its actual cells;
/// this vector is not a capability for another function with equal names.
pub(crate) fn validated_annotation_capture_schema(
    provider: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<Vec<AnnotationCaptureSlot>> {
    let function = provider.function();
    let py = function.py();
    provider.ensure_live()?;
    if !provider
        .origin()
        .is_some_and(|origin| origin.role == CallableSourceRole::AnnotationProvider)
    {
        return Err(strict_runtime_unavailable(
            py,
            "capture schema requires an authenticated annotation provider",
        ));
    }
    if provider.is_interpreter() {
        return native_annotation_capture_schema(provider);
    }
    let blockpy = provider
        .module_state()?
        .lookup_function(provider.function_id()?)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "annotation provider function plan is absent")
        })?;
    let projection = blockpy.scope.annotation_provider.as_ref().ok_or_else(|| {
        strict_runtime_unavailable(py, "annotation provider has no explicit capture projection")
    })?;
    if blockpy.params.len() != 1
        || blockpy.params.params[0].name != projection.kind.parameter_name()
        || blockpy.params.params[0].kind != ParamKind::PosOnly
        || blockpy.params.params[0].has_default
            != (projection.kind != AnnotationProviderKind::Dictionary)
    {
        return Err(strict_runtime_unavailable(
            py,
            "annotation provider public signature is not native",
        ));
    }
    Ok(validated_native_capture_layout(provider)?
        .into_iter()
        .enumerate()
        .map(|(cell_index, logical_name)| {
            let kind = if projection.class_dictionary.as_ref() == Some(&logical_name) {
                AnnotationCaptureKind::ClassDictionary
            } else if projection.conditional_annotations.as_ref() == Some(&logical_name) {
                AnnotationCaptureKind::ConditionalAnnotations
            } else {
                AnnotationCaptureKind::Lexical
            };
            AnnotationCaptureSlot {
                logical_name,
                cell_index,
                kind,
                lexical_scope: None,
            }
        })
        .collect())
}

/// Native roles come from the authenticated original provider's exact FREE
/// ordinal. Class roles additionally join its actual namespace execution; the
/// captured value is still untrusted until the consumer authenticates it.
fn native_annotation_capture_schema(
    provider: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<Vec<AnnotationCaptureSlot>> {
    let names = native_capture_names(provider)?;
    let py = provider.function().py();
    let source = &provider
        .origin()
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "native provider has no original source owner")
        })?
        .definition;
    let facts = provider.verified_module().type_facts().facts();
    // native_capture_names validated this actual code and closure. No Python
    // callback or extra code/provider owner intervenes before the scalar query.
    let raw = provider.function().as_ptr().cast::<ffi::PyFunctionObject>();
    let code = unsafe { Borrowed::<PyAny>::from_ptr(py, (*raw).func_code) };
    let native_source = provider.native_source()?;
    let same_class_execution = |class_definition: &soac_contracts::SourceIdentity| {
        provider.creation_execution().is_some_and(|execution| {
            execution.source() == class_definition
                && execution
                    .matches_source_execution(provider.verified_module(), provider.execution_ref())
        })
    };
    names
        .into_iter()
        .enumerate()
        .map(|(cell_index, logical_name)| {
            let free_ordinal = u32::try_from(cell_index).map_err(|_| {
                strict_runtime_unavailable(py, "native provider FREE ordinal overflows")
            })?;
            let origin = native_source.annotation_capture(py, &code, free_ordinal)?;
            let (kind, lexical_scope) = match origin {
                InterpreterAnnotationCaptureOrigin::Lexical { binding_scope, .. }
                    if facts.nominal_bindings.iter().any(|leaf| {
                        let own = match &leaf.owner {
                            soac_contracts::NominalBindingOwner::Function { function, .. } => {
                                function == source
                            }
                            soac_contracts::NominalBindingOwner::Field { field } => {
                                &field.declaring_class.definition == source
                            }
                        };
                        own && leaf.binding_scope == *binding_scope && leaf.name == logical_name
                    }) =>
                {
                    (AnnotationCaptureKind::Lexical, Some(binding_scope.clone()))
                }
                InterpreterAnnotationCaptureOrigin::ClassDictionary {
                    class_definition, ..
                } if same_class_execution(class_definition) => {
                    (AnnotationCaptureKind::ClassDictionary, None)
                }
                InterpreterAnnotationCaptureOrigin::ConditionalAnnotations {
                    class_definition,
                    ..
                } if same_class_execution(class_definition) => {
                    (AnnotationCaptureKind::ConditionalAnnotations, None)
                }
                // Missing/ambiguous/unproved ancestry and wrong-activation captures
                // never gain a role from spelling or a dictionary-shaped value.
                _ => (AnnotationCaptureKind::UnresolvedNativeRole, None),
            };
            Ok(AnnotationCaptureSlot {
                logical_name,
                cell_index,
                kind,
                lexical_scope,
            })
        })
        .collect()
}

/// No Python allocations, attribute lookup, or code/closure/cell owner copies.
/// The caller must keep the actual provider supported without callbacks.
fn native_capture_names(provider: &AuthenticatedStrictFunction<'_, '_>) -> PyResult<Vec<String>> {
    unsafe extern "C" {
        static mut PyCell_Type: ffi::PyTypeObject;
    }
    let py = provider.function().py();
    if !provider.interpreter_source_authority()? {
        return Err(strict_runtime_unavailable(
            py,
            "ordinary replacement is not a native capture source",
        ));
    }
    let raw = provider.function().as_ptr().cast::<ffi::PyFunctionObject>();
    let code = unsafe { Borrowed::<PyAny>::from_ptr(py, (*raw).func_code) };
    if !provider.is_original_interpreter_entry(code.as_ptr())? {
        return Err(strict_runtime_unavailable(
            py,
            "native provider original code changed",
        ));
    }
    let original = provider.native_source()?.code(py, &code)?;
    if original.ordinal() != provider.native_code_ordinal()? {
        return Err(strict_runtime_unavailable(
            py,
            "native provider code ordinal changed",
        ));
    }
    let names: Vec<_> = original
        .layout()
        .free_variables()
        .map(|(_, _, name)| name.to_owned())
        .collect();
    let closure = unsafe { (*raw).func_closure };
    if closure.is_null() {
        if names.is_empty() {
            return Ok(names);
        }
        return Err(strict_runtime_unavailable(
            py,
            "native provider closure has not been published",
        ));
    }
    if unsafe { ffi::PyTuple_CheckExact(closure) } == 0
        || unsafe { ffi::PyTuple_Size(closure) } != names.len() as isize
    {
        return Err(strict_runtime_unavailable(
            py,
            "native provider closure differs from actual code",
        ));
    }
    for index in 0..names.len() {
        let cell = unsafe { ffi::PyTuple_GetItem(closure, index as isize) };
        if cell.is_null() || unsafe { ffi::Py_TYPE(cell) } != ptr::addr_of_mut!(PyCell_Type) {
            return Err(strict_runtime_unavailable(
                py,
                "native provider capture is not a cell",
            ));
        }
    }
    Ok(names)
}

/// Validate the actual closure against its native and public compiler layouts.
/// This returns only names; it neither gives a source callback a compiler
/// annotation-provider role nor grants access to provider-specific projections.
fn validated_native_capture_layout(
    provider: &AuthenticatedStrictFunction<'_, '_>,
) -> PyResult<Vec<String>> {
    unsafe extern "C" {
        fn PyCode_GetSoacStrictSourceId(code: *mut ffi::PyObject) -> u64;
        fn PyFunction_GetSoacStrictOwner(function: *mut ffi::PyObject) -> *mut ffi::PyObject;
        static mut PyCell_Type: ffi::PyTypeObject;
    }
    let function = provider.function();
    let py = function.py();
    provider.ensure_live()?;
    if provider.is_interpreter() {
        return native_capture_names(provider);
    }
    let blockpy = provider
        .module_state()?
        .lookup_function(provider.function_id()?)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "annotation callback function plan is absent")
        })?;
    let original = provider
        .module_state()?
        .lookup_original_code(provider.function_id()?)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "annotation provider original code is absent")
        })?
        .bind(py);
    let raw = function.as_ptr().cast::<ffi::PyFunctionObject>();
    if unsafe { (*raw).func_code.cast::<ffi::PyObject>() } != original.as_ptr()
        || unsafe { PyCode_GetSoacStrictSourceId(original.as_ptr()) } == 0
    {
        return Err(strict_runtime_unavailable(
            py,
            "annotation provider does not retain its actual verified native code",
        ));
    }
    let closure_pointer = unsafe { (*raw).func_closure };
    let closure = if closure_pointer.is_null() {
        None
    } else {
        Some(
            unsafe { Bound::<PyAny>::from_borrowed_ptr(py, closure_pointer) }
                .cast_into::<PyTuple>()?,
        )
    };
    let names = original.getattr("co_freevars")?.extract::<Vec<String>>()?;
    let captures = blockpy
        .public_storage_layout()
        .map_or(&[][..], |layout| layout.freevars.as_slice());
    if names.len() != captures.len()
        || names
            .iter()
            .zip(captures)
            .any(|(name, slot)| name != &slot.logical_name)
        || closure.as_ref().map_or(0, |closure| closure.len()) != names.len()
    {
        return Err(strict_runtime_unavailable(
            py,
            "annotation provider native and lowered capture layouts differ",
        ));
    }
    for cell_index in 0..names.len() {
        let cell = closure
            .as_ref()
            .expect("nonempty capture layout has a closure")
            .get_item(cell_index)?;
        if unsafe { ffi::Py_TYPE(cell.as_ptr()) } != ptr::addr_of_mut!(PyCell_Type) {
            return Err(strict_runtime_unavailable(
                py,
                "annotation provider capture is not an actual native cell",
            ));
        }
    }
    // Native metadata getters can allocate. Do not return a schema for code
    // or a closure tuple replaced by a callback during that allocation.
    provider.ensure_live()?;
    let owner = unsafe { PyFunction_GetSoacStrictOwner(function.as_ptr()) };
    if owner.is_null() && !unsafe { ffi::PyErr_Occurred() }.is_null() {
        return Err(PyErr::fetch(py));
    }
    if owner != provider.owner().as_ptr()
        || unsafe { (*raw).func_code.cast::<ffi::PyObject>() } != original.as_ptr()
        || unsafe { (*raw).func_closure } != closure_pointer
    {
        return Err(strict_runtime_unavailable(
            py,
            "annotation provider changed during capture validation",
        ));
    }
    Ok(names)
}

pub(crate) const NEW_ANNOTATION_SET_SYMBOL: &str = "soac_jit_new_annotation_set";
pub(crate) const SETUP_ANNOTATIONS_SYMBOL: &str = "soac_jit_setup_annotations";
pub(crate) const RECORD_ANNOTATION_SYMBOL: &str = "soac_jit_record_annotation";
pub(crate) const CHECK_ANNOTATION_FORMAT_SYMBOL: &str = "soac_jit_check_annotation_format";
pub(crate) const CREATE_TYPE_ALIAS_SYMBOL: &str = "soac_jit_create_type_alias";
pub(crate) const CREATE_TYPE_PARAMETER_SYMBOL: &str = "soac_jit_create_type_parameter";
pub(crate) const SET_TYPE_PARAMETER_DEFAULT_SYMBOL: &str = "soac_jit_set_type_parameter_default";
pub(crate) const CONSTRUCT_TYPE_PARAMETER_SCOPE_SYMBOL: &str =
    "soac_jit_construct_type_parameter_scope";
pub(crate) const SUBSCRIPT_GENERIC_SYMBOL: &str = "soac_jit_subscript_generic";
pub(crate) const SET_FUNCTION_TYPE_PARAMETERS_SYMBOL: &str =
    "soac_jit_set_function_type_parameters";

/// A mechanical translation of the explicit IR kind to the pinned native ABI.
pub(crate) const fn type_parameter_kind_tag(kind: TypeParameterKind) -> c_int {
    match kind {
        TypeParameterKind::TypeVar => 0,
        TypeParameterKind::TypeVarBound => 1,
        TypeParameterKind::TypeVarConstraints => 2,
        TypeParameterKind::ParamSpec => 3,
        TypeParameterKind::TypeVarTuple => 4,
    }
}

fn authenticate_type_expression<'py>(
    evaluator: &Bound<'py, PyAny>,
    expected_function: u64,
    kind: AnnotationProviderKind,
    globals: *mut ffi::PyObject,
) -> PyResult<AuthenticatedStrictFunction<'static, 'py>> {
    let py = evaluator.py();
    let auth = authenticate_strict_function(py, evaluator)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "type expression needs its actual native source owner")
    })?;
    let expected_definition = match kind {
        AnnotationProviderKind::TypeAliasValue => soac_contracts::DefinitionKind::TypeAlias,
        AnnotationProviderKind::TypeParameterBound
        | AnnotationProviderKind::TypeParameterConstraints
        | AnnotationProviderKind::TypeParameterDefault => soac_contracts::DefinitionKind::Parameter,
        AnnotationProviderKind::Dictionary => {
            return Err(strict_runtime_unavailable(
                py,
                "dictionary provider is not a type evaluator",
            ));
        }
    };
    let function = auth
        .module_state()?
        .lookup_function(auth.function_id()?)
        .ok_or_else(|| {
            strict_runtime_unavailable(py, "type expression has no explicit compiler plan")
        })?;
    if auth.function_id()?.to_packed_runtime_u64() != expected_function
        || auth.globals()?.as_ptr() != globals
        || !auth.origin().is_some_and(|origin| {
            origin.role == CallableSourceRole::AnnotationProvider
                && origin.definition.definition_kind == expected_definition
        })
        || !function
            .scope
            .annotation_provider
            .as_ref()
            .is_some_and(|plan| plan.kind == kind)
    {
        return Err(strict_runtime_unavailable(
            py,
            "type expression factory and evaluator identity differ",
        ));
    }
    validated_annotation_capture_schema(&auth)?;
    Ok(auth)
}

/// Factories authenticate only the evaluator actually supplied to this creation
/// operation. The post-create native slot check neither seals the target nor
/// leaves a token, registry entry, or extra lifetime edge on it.
unsafe fn create_owned_type_expression<'py>(
    py: Python<'py>,
    expected_function: u64,
    kind: AnnotationProviderKind,
    native_kind: c_int,
    evaluator: *mut ffi::PyObject,
    globals: *mut ffi::PyObject,
    create: impl FnOnce() -> *mut ffi::PyObject,
) -> PyResult<Bound<'py, PyAny>> {
    if evaluator.is_null() || globals.is_null() {
        return Err(strict_runtime_unavailable(
            py,
            "type expression factory is missing an operand",
        ));
    }
    let evaluator = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, evaluator) };
    let auth = authenticate_type_expression(&evaluator, expected_function, kind, globals)?;
    let raw = evaluator.as_ptr().cast::<ffi::PyFunctionObject>();
    let code = unsafe { (*raw).func_code };
    let closure = unsafe { (*raw).func_closure };
    let target = unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, create()) }?;
    // Allocation may invoke GC callbacks. Recheck the exact owned function,
    // its current native code/cells and the private target slot afterwards.
    let current = authenticate_type_expression(&evaluator, expected_function, kind, globals)?;
    if current.owner().as_ptr() != auth.owner().as_ptr()
        || unsafe { (*raw).func_code } != code
        || unsafe { (*raw).func_closure } != closure
    {
        return Err(strict_runtime_unavailable(
            py,
            "type evaluator changed during native creation",
        ));
    }
    match unsafe { PySoac_MatchesTypeExpression(target.as_ptr(), native_kind, evaluator.as_ptr()) }
    {
        1 => Ok(target),
        0 => Err(strict_runtime_unavailable(
            py,
            "created type expression does not own its evaluator",
        )),
        _ => Err(PyErr::fetch(py)),
    }
}

fn type_expression_result<'py>(
    py: Python<'py>,
    operation: impl FnOnce() -> PyResult<Bound<'py, PyAny>>,
) -> *mut ffi::PyObject {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => value.into_ptr(),
        Ok(Err(error)) => {
            error.restore(py);
            ptr::null_mut()
        }
        Err(_) => {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_SystemError,
                    c"panic in strict type expression factory".as_ptr(),
                )
            };
            ptr::null_mut()
        }
    }
}

pub(crate) unsafe extern "C" fn create_type_alias(
    expected_function: u64,
    name: *mut ffi::PyObject,
    parameters: *mut ffi::PyObject,
    evaluator: *mut ffi::PyObject,
    globals: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    type_expression_result(py, || unsafe {
        create_owned_type_expression(
            py,
            expected_function,
            AnnotationProviderKind::TypeAliasValue,
            0,
            evaluator,
            globals,
            || PySoac_NewTypeAlias(name, parameters, evaluator),
        )
    })
}

/// The function operand is created after the two enclosing-scope default
/// containers. Call its normal authenticated SOAC entry; its original native
/// code remains a denied inspection witness, not a fallback execution path.
pub(crate) unsafe extern "C" fn construct_type_parameter_scope(
    expected_function: u64,
    positional_defaults: *mut ffi::PyObject,
    keyword_defaults: *mut ffi::PyObject,
    function: *mut ffi::PyObject,
    globals: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    type_expression_result(py, || {
        if function.is_null() || globals.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "generic scope is missing its actual function/globals",
            ));
        }
        let function = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, function) };
        let auth = authenticate_strict_function(py, &function)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "generic scope requires its actual native source owner")
        })?;
        let blockpy = auth
            .module_state()?
            .lookup_function(auth.function_id()?)
            .ok_or_else(|| strict_runtime_unavailable(py, "generic scope has no compiler plan"))?;
        let projection = blockpy.scope.type_parameter_scope.as_ref().ok_or_else(|| {
            strict_runtime_unavailable(py, "generic scope has no explicit parameter projection")
        })?;
        if auth.function_id()?.to_packed_runtime_u64() != expected_function
            || auth.globals()?.as_ptr() != globals
            || !auth
                .origin()
                .is_some_and(|origin| origin.role == CallableSourceRole::TypeParameterScope)
            || projection.inputs.len() > 2
            || projection.inputs.len()
                != usize::from(!positional_defaults.is_null())
                    + usize::from(!keyword_defaults.is_null())
        {
            return Err(strict_runtime_unavailable(
                py,
                "generic scope construction identity differs",
            ));
        }
        let mut arguments = [ptr::null_mut(); 2];
        for (index, input) in projection.inputs.iter().enumerate() {
            let value = match input.kind {
                TypeParameterScopeInputKind::PositionalDefaults => positional_defaults,
                TypeParameterScopeInputKind::KeywordDefaults => keyword_defaults,
            };
            if value.is_null()
                || unsafe {
                    match input.kind {
                        TypeParameterScopeInputKind::PositionalDefaults => {
                            ffi::PyTuple_CheckExact(value) == 0
                        }
                        TypeParameterScopeInputKind::KeywordDefaults => {
                            ffi::PyDict_CheckExact(value) == 0
                        }
                    }
                }
            {
                return Err(strict_runtime_unavailable(
                    py,
                    "generic scope received a mismatched native default container",
                ));
            }
            arguments[index] = value;
        }
        unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                ffi::PyObject_Vectorcall(
                    function.as_ptr(),
                    arguments.as_ptr(),
                    projection.inputs.len(),
                    ptr::null_mut(),
                ),
            )
        }
    })
}

pub(crate) unsafe extern "C" fn subscript_generic(
    parameters: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    unsafe { PySoac_SubscriptGeneric(parameters) }
}

pub(crate) unsafe extern "C" fn set_function_type_parameters(
    expected_function: u64,
    function: *mut ffi::PyObject,
    parameters: *mut ffi::PyObject,
    globals: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    type_expression_result(py, || {
        if function.is_null() || parameters.is_null() || globals.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "generic function metadata is missing an operand",
            ));
        }
        let function = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, function) };
        let auth = authenticate_strict_function(py, &function)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "generic metadata needs its actual source function")
        })?;
        if auth.function_id()?.to_packed_runtime_u64() != expected_function
            || auth.globals()?.as_ptr() != globals
            || !auth
                .origin()
                .is_some_and(|origin| origin.role == CallableSourceRole::SourceFunction)
        {
            return Err(strict_runtime_unavailable(
                py,
                "generic metadata function identity differs",
            ));
        }
        let result = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                PySoac_SetFunctionTypeParameters(function.as_ptr(), parameters),
            )
        }?;
        let current = authenticate_strict_function(py, &function)?.ok_or_else(|| {
            strict_runtime_unavailable(py, "generic function changed during metadata attachment")
        })?;
        if result.as_ptr() != function.as_ptr()
            || current.owner().as_ptr() != auth.owner().as_ptr()
            || function.getattr("__type_params__")?.as_ptr() != parameters
        {
            return Err(strict_runtime_unavailable(
                py,
                "generic metadata attachment changed its target",
            ));
        }
        Ok(result)
    })
}

pub(crate) unsafe extern "C" fn create_type_parameter(
    expected_function: u64,
    kind: c_int,
    name: *mut ffi::PyObject,
    evaluator: *mut ffi::PyObject,
    globals: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    type_expression_result(py, || {
        let provider_kind = match kind {
            1 => Some(AnnotationProviderKind::TypeParameterBound),
            2 => Some(AnnotationProviderKind::TypeParameterConstraints),
            0 | 3 | 4 => None,
            _ => {
                return Err(strict_runtime_unavailable(
                    py,
                    "invalid native type parameter kind",
                ));
            }
        };
        if let Some(provider_kind) = provider_kind {
            unsafe {
                create_owned_type_expression(
                    py,
                    expected_function,
                    provider_kind,
                    kind,
                    evaluator,
                    globals,
                    || PySoac_NewTypeParameter(kind, name, evaluator),
                )
            }
        } else if expected_function != 0 || !evaluator.is_null() {
            Err(strict_runtime_unavailable(
                py,
                "unbounded type parameter received an evaluator",
            ))
        } else {
            unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(
                    py,
                    PySoac_NewTypeParameter(kind, name, ptr::null_mut()),
                )
            }
        }
    })
}

pub(crate) unsafe extern "C" fn set_type_parameter_default(
    expected_function: u64,
    parameter: *mut ffi::PyObject,
    evaluator: *mut ffi::PyObject,
    globals: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    type_expression_result(py, || unsafe {
        create_owned_type_expression(
            py,
            expected_function,
            AnnotationProviderKind::TypeParameterDefault,
            3,
            evaluator,
            globals,
            || PySoac_SetTypeParameterDefault(parameter, evaluator),
        )
    })
}

pub(crate) unsafe extern "C" fn new_annotation_set() -> *mut ffi::PyObject {
    unsafe { ffi::PySet_New(ptr::null_mut()) }
}

pub(crate) unsafe extern "C" fn setup_annotations(
    namespace: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    if unsafe { PySoac_SetupAnnotations(namespace) } < 0 {
        ptr::null_mut()
    } else {
        unsafe { ffi::Py_NewRef(ffi::Py_None()) }
    }
}

pub(crate) unsafe extern "C" fn record_annotation(
    indices: *mut ffi::PyObject,
    index: u32,
) -> *mut ffi::PyObject {
    let index = unsafe { ffi::PyLong_FromUnsignedLong(index.into()) };
    if index.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { ffi::PySet_Add(indices, index) };
    unsafe { ffi::Py_DECREF(index) };
    if result < 0 {
        ptr::null_mut()
    } else {
        unsafe { ffi::Py_NewRef(ffi::Py_None()) }
    }
}

pub(crate) unsafe extern "C" fn check_annotation_format(
    format: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let value_with_fake_globals = unsafe { ffi::PyLong_FromLong(2) };
    if value_with_fake_globals.is_null() {
        return ptr::null_mut();
    }
    let result =
        unsafe { ffi::PyObject_RichCompareBool(format, value_with_fake_globals, ffi::Py_GT) };
    unsafe { ffi::Py_DECREF(value_with_fake_globals) };
    match result {
        -1 => ptr::null_mut(),
        0 => unsafe { ffi::Py_NewRef(ffi::Py_None()) },
        _ => {
            unsafe { ffi::PyErr_SetNone(ffi::PyExc_NotImplementedError) };
            ptr::null_mut()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::types::{PyDict, PySet};

    #[test]
    fn native_lexical_capture_matches_each_leafs_exact_scope_not_only_name() {
        use soac_contracts::{
            DefinitionKind, ModuleTypeFacts, ResolvedStrictPolicy, SourceDialect, SourceIdentity,
            SourceRange,
        };
        let text = "from __future__ import strict\ndef factory():\n    pass\n";
        let facts = ModuleTypeFacts::new(
            "native_capture_scope",
            text.as_bytes(),
            SourceDialect::SoacStrict,
            ResolvedStrictPolicy::default(),
        )
        .unwrap();
        let scope = SourceIdentity {
            module: facts.module,
            lexical_qualname: "factory".into(),
            source_range: SourceRange::new(
                text.find("def factory").unwrap() as u32,
                text.len() as u32,
            ),
            definition_kind: DefinitionKind::Function,
        };
        let mut other = scope.clone();
        other.source_range.start += 1;
        let native = AnnotationCaptureSlot {
            logical_name: "Alias".into(),
            cell_index: 0,
            kind: AnnotationCaptureKind::Lexical,
            lexical_scope: Some(scope.clone()),
        };
        assert!(native.matches_lexical_binding("Alias", &scope));
        assert!(!native.matches_lexical_binding("Alias", &other));
        assert!(!native.matches_lexical_binding("Other", &scope));
        let mut wrong_kind = scope.clone();
        wrong_kind.definition_kind = DefinitionKind::Class;
        assert!(!native.matches_lexical_binding("Alias", &wrong_kind));

        // The existing compiled projection retains its old lexical behavior.
        // Native construction never emits Lexical with an absent scope.
        let compiled = AnnotationCaptureSlot {
            lexical_scope: None,
            ..native.clone()
        };
        assert!(compiled.matches_lexical_binding("Alias", &scope));
        assert!(compiled.matches_lexical_binding("Alias", &other));
        let unresolved = AnnotationCaptureSlot {
            kind: AnnotationCaptureKind::UnresolvedNativeRole,
            lexical_scope: None,
            ..native
        };
        assert!(!unresolved.matches_lexical_binding("Alias", &scope));
    }

    #[test]
    fn native_annotation_operations_use_real_sets_and_canonical_format_error() {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let indices =
                unsafe { Bound::<PyAny>::from_owned_ptr_or_err(py, new_annotation_set()) }.unwrap();
            assert!(indices.is_exact_instance_of::<PySet>());
            for index in [3, 3, 7] {
                let result = unsafe {
                    Bound::<PyAny>::from_owned_ptr_or_err(
                        py,
                        record_annotation(indices.as_ptr(), index),
                    )
                }
                .unwrap();
                assert!(result.is_none());
            }
            assert_eq!(indices.cast::<PySet>().unwrap().len(), 2);
            assert!(indices.contains(3).unwrap());
            assert!(!indices.contains(2).unwrap());
            let globals = PyDict::new(py);
            let result = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(py, setup_annotations(globals.as_ptr()))
            }
            .unwrap();
            assert!(result.is_none());
            assert!(
                globals
                    .get_item("__annotations__")
                    .unwrap()
                    .unwrap()
                    .is_exact_instance_of::<PyDict>()
            );
            globals.set_item("__annotations__", py.None()).unwrap();
            let result = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(py, setup_annotations(globals.as_ptr()))
            }
            .unwrap();
            assert!(result.is_none());
            assert!(
                globals
                    .get_item("__annotations__")
                    .unwrap()
                    .unwrap()
                    .is_none()
            );
            py.run(c"class Format:\n    def __gt__(self, other):\n        assert other == 2\n        return True\nvalue = Format()", Some(&globals), None).unwrap();
            let format = globals.get_item("value").unwrap().unwrap();
            assert!(unsafe { check_annotation_format(format.as_ptr()) }.is_null());
            let error = PyErr::fetch(py);
            assert!(error.is_instance_of::<pyo3::exceptions::PyNotImplementedError>(py));
            let value = 2u32.into_pyobject(py).unwrap();
            let result = unsafe {
                Bound::<PyAny>::from_owned_ptr_or_err(py, check_annotation_format(value.as_ptr()))
            }
            .unwrap();
            assert!(result.is_none());
        });
    }
}
