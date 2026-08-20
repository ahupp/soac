//! Fresh builtin descriptor birth for one source-selected function definition.
//!
//! The compiler supplies the original MakeFunction result directly. No result
//! of an intervening decorator, same-source function, or mutable Python name
//! may substitute for that operand. Runtime selection is optional until the
//! actual class's copied namespace is admitted; adoption is permanent.

use std::ffi::{CStr, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::{Arc, OnceLock};

use pyo3::ffi;
use pyo3::prelude::*;
use soac_contracts::{DecoratorKind, SourceIdentity};
use soac_core::block_py::CallableSourceRole;

use crate::strict_function::{AuthenticatedStrictFunction, authenticate_class_candidate_function};
use crate::strict_namespace::NamespaceExecution;
use crate::strict_state::{StrictStateData, StrictStateRef};
use crate::{FunctionEnv, FunctionEnvAbiHeader, strict_runtime_unavailable};

pub(crate) const APPLY_FUNCTION_DESCRIPTOR_SYMBOL: &str = "soac_jit_apply_function_descriptor";

unsafe extern "C" {
    static mut PyStaticMethod_Type: ffi::PyTypeObject;
    static mut PyClassMethod_Type: ffi::PyTypeObject;
    static mut PyProperty_Type: ffi::PyTypeObject;
    fn PySoac_NewBuiltinDescriptor(
        factory: *mut ffi::PyObject,
        function: *mut ffi::PyObject,
        function_owner: *mut ffi::PyObject,
        code: *mut ffi::PyObject,
        namespace_witness: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_GetDescriptorBirthOwner(descriptor: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PySoac_GetDescriptorBirthId(descriptor: *mut ffi::PyObject) -> u64;
    fn PySoac_MatchesDescriptorBirth(
        descriptor: *mut ffi::PyObject,
        namespace_witness: *mut ffi::PyObject,
        function: *mut ffi::PyObject,
        function_owner: *mut ffi::PyObject,
        code: *mut ffi::PyObject,
    ) -> c_int;
    fn PySoac_AdoptBuiltinDescriptor(
        descriptor: *mut ffi::PyObject,
        namespace_witness: *mut ffi::PyObject,
        function: *mut ffi::PyObject,
        function_owner: *mut ffi::PyObject,
        code: *mut ffi::PyObject,
    ) -> c_int;
    fn PySoac_VectorcallWithContext(
        callable: *mut ffi::PyObject,
        args: *const *mut ffi::PyObject,
        nargs: usize,
        kwnames: *mut ffi::PyObject,
        globals: *mut ffi::PyObject,
        locals: *mut ffi::PyObject,
        builtins: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
}

struct DescriptorBirthData {
    definition: SourceIdentity,
    verified: Arc<crate::VerifiedStrictModule>,
    execution: Arc<NamespaceExecution>,
    // Set exactly once after the original factory returns, with no intervening
    // allocation/callback. A same-source C caller can expose/reuse the owner,
    // but cannot replay this interpreter-owned, non-reused native birth ID.
    birth_id: OnceLock<u64>,
}

// SAFETY: Only immutable Rust source/policy coordinates and a zero-Python-edge
// execution identity are retained. The native descriptor already owns its
// function; this payload adds no function, code, class, globals, or namespace root.
unsafe impl StrictStateData for DescriptorBirthData {
    const TYPE_NAME: &'static CStr = c"soac._StrictDescriptorBirth";
}

fn selected_factory(kind: DecoratorKind) -> Option<*mut ffi::PyObject> {
    match kind {
        DecoratorKind::StaticMethod => Some(ptr::addr_of_mut!(PyStaticMethod_Type).cast()),
        DecoratorKind::ClassMethod => Some(ptr::addr_of_mut!(PyClassMethod_Type).cast()),
        DecoratorKind::Property => Some(ptr::addr_of_mut!(PyProperty_Type).cast()),
        _ => None,
    }
}

fn source_factory(auth: &AuthenticatedStrictFunction<'_, '_>) -> Option<*mut ffi::PyObject> {
    let origin = auth.origin()?;
    if origin.role != CallableSourceRole::SourceFunction || !auth.can_finalize() {
        return None;
    }
    let fact = auth
        .verified_module()
        .type_facts()
        .facts()
        .functions
        .iter()
        .find(|fact| fact.identity == origin.definition)?;
    let [decorator] = fact.decorators.as_slice() else {
        return None;
    };
    if !decorator.uncertainty.is_empty() || !decorator.arguments.is_empty() {
        return None;
    }
    selected_factory(decorator.kind)
}

/// All Python operands remain ordinary caller-owned call operands throughout
/// this helper. An unknown actual factory is invoked once with the original
/// function and acquires no birth, even if it returns a builtin descriptor.
pub(crate) unsafe extern "C" fn apply_function_descriptor(
    function_id: u64,
    environment: *const c_void,
    decorator: *mut ffi::PyObject,
    function: *mut ffi::PyObject,
    namespace: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<Bound<'_, PyAny>> {
        let header =
            unsafe { environment.cast::<FunctionEnvAbiHeader>().as_ref() }.ok_or_else(|| {
                strict_runtime_unavailable(py, "descriptor application has no active frame")
            })?;
        if decorator.is_null() || function.is_null() {
            return Err(strict_runtime_unavailable(
                py,
                "descriptor application has a null operand",
            ));
        }
        let active = unsafe { header.active_strict_call.as_ref() }
            .filter(|active| ptr::eq(active.environment().header(), header))
            .ok_or_else(|| {
                strict_runtime_unavailable(
                    py,
                    "descriptor application has no authenticated activation",
                )
            })?;
        let ordinary = || unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                PySoac_VectorcallWithContext(
                    decorator,
                    [function].as_ptr(),
                    1,
                    ptr::null_mut(),
                    header.globals_obj,
                    namespace,
                    header.builtins_obj,
                ),
            )
        };
        let original = unsafe { Bound::<PyAny>::from_borrowed_ptr(py, function) };
        let Some(auth) = authenticate_class_candidate_function(py, &original)? else {
            return ordinary();
        };
        let Some(execution) = (unsafe { FunctionEnv::namespace_execution_from_raw(environment) })
        else {
            return ordinary();
        };
        let caller = active.captured_owner(py)?;
        let Some(shared) = active.active_module_state() else {
            return Err(strict_runtime_unavailable(
                py,
                "descriptor activation lost its module state",
            ));
        };
        if auth.function_id()?.to_packed_runtime_u64() != function_id
            || source_factory(&auth) != Some(decorator)
            || !Arc::ptr_eq(auth.module_state()?, shared)
            || !auth
                .creation_execution()
                .is_some_and(|created| Arc::ptr_eq(created, &execution))
            || !caller.source().is_some_and(|origin| {
                origin.role == CallableSourceRole::ClassNamespace
                    && &origin.definition == execution.source()
            })
        {
            return ordinary();
        }
        execution.validate_creation(py, shared, auth.globals()?.as_any())?;
        let code = unsafe {
            Bound::<PyAny>::from_borrowed_ptr(
                py,
                (*function.cast::<ffi::PyFunctionObject>()).func_code,
            )
        };
        let witness = StrictStateRef::new(
            py,
            DescriptorBirthData {
                definition: auth
                    .origin()
                    .expect("selected source function")
                    .definition
                    .clone(),
                verified: auth.verified_module().clone(),
                execution: execution.clone(),
                birth_id: OnceLock::new(),
            },
            Vec::new(),
        )?;
        // Allocating the inert witness may collect arbitrary Python objects.
        // Revalidate before the native factory; a changed uncommitted function
        // still uses the one ordinary decorator call, not a stale proposal.
        let Some(current) = authenticate_class_candidate_function(py, &original)? else {
            return ordinary();
        };
        if current.owner().as_ptr() != auth.owner().as_ptr()
            || unsafe { (*function.cast::<ffi::PyFunctionObject>()).func_code } != code.as_ptr()
        {
            return ordinary();
        }
        execution.validate_creation(py, shared, current.globals()?.as_any())?;
        let descriptor = unsafe {
            Bound::<PyAny>::from_owned_ptr_or_err(
                py,
                PySoac_NewBuiltinDescriptor(
                    decorator,
                    function,
                    current.owner().as_ptr(),
                    code.as_ptr(),
                    witness.owner().as_ptr(),
                ),
            )
        }?;
        let identity = unsafe { PySoac_GetDescriptorBirthId(descriptor.as_ptr()) };
        if identity == 0 {
            return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
                strict_runtime_unavailable(py, "new descriptor has no live native birth identity")
            } else {
                PyErr::fetch(py)
            });
        }
        witness.data().birth_id.set(identity).map_err(|_| {
            strict_runtime_unavailable(py, "descriptor producer already recorded a native birth")
        })?;
        Ok(descriptor)
    }));
    match result {
        Ok(Ok(value)) => value.into_ptr(),
        Ok(Err(error)) => {
            error.restore(py);
            ptr::null_mut()
        }
        Err(_) => {
            strict_runtime_unavailable(py, "panic in source descriptor application").restore(py);
            ptr::null_mut()
        }
    }
}

/// The same zero-value-edge birth witness used by the retained backend.
/// All returned value pointers are comparison-only and supported by the real
/// native function operand; the C producer rechecks before dereferencing code.
pub(crate) struct NativeDescriptorSelection<'py> {
    pub(crate) witness: Bound<'py, PyAny>,
    pub(crate) function_owner: *mut ffi::PyObject,
    pub(crate) code: *mut ffi::PyObject,
}

pub(crate) fn prepare_native_descriptor<'py>(
    py: Python<'py>,
    source: &Arc<crate::strict_interpreter_source::StrictInterpreterSource>,
    source_execution: &crate::StrictModuleExecutionRef,
    invocation: &Arc<crate::strict_interpreter::InterpreterInvocationIdentity>,
    namespace: &Arc<NamespaceExecution>,
    definition: &SourceIdentity,
    actual_globals: *mut ffi::PyObject,
    factory: Borrowed<'_, 'py, PyAny>,
    function: Borrowed<'_, 'py, PyAny>,
) -> PyResult<Option<NativeDescriptorSelection<'py>>> {
    let Some(auth) = crate::strict_function::authenticate_borrowed_strict_function(py, function)?
    else {
        return Ok(None);
    };
    let matches = |auth: &AuthenticatedStrictFunction<'_, '_>| -> PyResult<bool> {
        Ok(auth.is_interpreter()
            && auth.interpreter_source_authority()?
            && source_factory(auth) == Some(factory.as_ptr())
            && auth
                .origin()
                .is_some_and(|origin| &origin.definition == definition)
            && Arc::ptr_eq(auth.native_source()?, source)
            && auth.execution().same_execution(source_execution)
            && Arc::ptr_eq(auth.native_birth_execution()?, invocation)
            && auth
                .creation_execution()
                .is_some_and(|created| Arc::ptr_eq(created, namespace))
            && auth.globals()?.as_ptr() == actual_globals)
    };
    if !matches(&auth)? {
        return Ok(None);
    }
    namespace.validate_native_creation(
        py,
        source.verified(),
        source_execution,
        actual_globals as usize,
    )?;
    let function_owner = auth.owner().as_ptr();
    let code = unsafe { (*function.as_ptr().cast::<ffi::PyFunctionObject>()).func_code };
    let witness = StrictStateRef::new(
        py,
        DescriptorBirthData {
            definition: definition.clone(),
            verified: source.verified().clone(),
            execution: namespace.clone(),
            birth_id: OnceLock::new(),
        },
        Vec::new(),
    )?;
    // Metadata allocation may run GC. Reauthenticate the actual still-borrowed
    // function and never transport a stale raw code pointer into a constructor.
    let Some(current) =
        crate::strict_function::authenticate_borrowed_strict_function(py, function)?
    else {
        return Ok(None);
    };
    if !matches(&current)?
        || current.owner().as_ptr() != function_owner
        || unsafe { (*function.as_ptr().cast::<ffi::PyFunctionObject>()).func_code } != code
    {
        return Ok(None);
    }
    namespace.validate_native_creation(
        py,
        source.verified(),
        source_execution,
        actual_globals as usize,
    )?;
    Ok(Some(NativeDescriptorSelection {
        witness: witness.owner().clone(),
        function_owner,
        code,
    }))
}

pub(crate) fn finish_native_descriptor(
    py: Python<'_>,
    source: &Arc<crate::strict_interpreter_source::StrictInterpreterSource>,
    namespace: &Arc<NamespaceExecution>,
    definition: &SourceIdentity,
    metadata: Borrowed<'_, '_, PyAny>,
    result: Option<Borrowed<'_, '_, PyAny>>,
) -> PyResult<()> {
    let witness = StrictStateRef::<DescriptorBirthData>::from_owner(metadata.to_owned())?;
    let data = witness.data();
    if &data.definition != definition
        || !Arc::ptr_eq(&data.verified, source.verified())
        || !Arc::ptr_eq(&data.execution, namespace)
        || data.birth_id.get().is_some()
    {
        return Err(strict_runtime_unavailable(
            py,
            "descriptor completion has a foreign or reused witness",
        ));
    }
    let Some(descriptor) = result else {
        return Ok(());
    };
    let actual_owner = unsafe { PySoac_GetDescriptorBirthOwner(descriptor.as_ptr()) };
    if actual_owner != metadata.as_ptr() {
        return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
            strict_runtime_unavailable(
                py,
                "descriptor completion did not construct its selected birth",
            )
        } else {
            PyErr::fetch(py)
        });
    }
    let identity = unsafe { PySoac_GetDescriptorBirthId(descriptor.as_ptr()) };
    if identity == 0 {
        return Err(if unsafe { ffi::PyErr_Occurred() }.is_null() {
            strict_runtime_unavailable(
                py,
                "descriptor completion has no actual native birth identity",
            )
        } else {
            PyErr::fetch(py)
        });
    }
    data.birth_id.set(identity).map_err(|_| {
        strict_runtime_unavailable(
            py,
            "descriptor completion already consumed its native birth",
        )
    })
}

fn birth_for_component<'py>(
    py: Python<'py>,
    descriptor: &Bound<'py, PyAny>,
    function: &AuthenticatedStrictFunction<'_, 'py>,
    execution: &Arc<NamespaceExecution>,
) -> PyResult<Option<StrictStateRef<'py, DescriptorBirthData>>> {
    let identity = unsafe { PySoac_GetDescriptorBirthId(descriptor.as_ptr()) };
    if identity == 0 {
        return if unsafe { ffi::PyErr_Occurred() }.is_null() {
            Ok(None)
        } else {
            Err(PyErr::fetch(py))
        };
    }
    let owner = unsafe { PySoac_GetDescriptorBirthOwner(descriptor.as_ptr()) };
    if owner.is_null() {
        return if unsafe { ffi::PyErr_Occurred() }.is_null() {
            Ok(None)
        } else {
            Err(PyErr::fetch(py))
        };
    }
    let Some(witness) = StrictStateRef::<DescriptorBirthData>::try_from_owner(unsafe {
        Bound::from_borrowed_ptr(py, owner)
    })?
    else {
        return Ok(None);
    };
    let data = witness.data();
    if !execution.is_completed()
        || data.birth_id.get().copied() != Some(identity)
        || !Arc::ptr_eq(&data.execution, execution)
        || !Arc::ptr_eq(&data.verified, function.verified_module())
        || !function
            .origin()
            .is_some_and(|origin| origin.definition == data.definition)
        || !function
            .creation_execution()
            .is_some_and(|created| Arc::ptr_eq(created, execution))
    {
        return Ok(None);
    }
    let code = unsafe { (*function.function().as_ptr().cast::<ffi::PyFunctionObject>()).func_code };
    match unsafe {
        PySoac_MatchesDescriptorBirth(
            descriptor.as_ptr(),
            witness.owner().as_ptr(),
            function.function().as_ptr(),
            function.owner().as_ptr(),
            code,
        )
    } {
        1 => Ok(Some(witness)),
        0 => Ok(None),
        _ => Err(PyErr::fetch(py)),
    }
}

pub(crate) fn matches_birth(
    py: Python<'_>,
    descriptor: &Bound<'_, PyAny>,
    function: &Bound<'_, PyAny>,
    execution: &Arc<NamespaceExecution>,
) -> PyResult<bool> {
    let Some(function) = authenticate_class_candidate_function(py, function)? else {
        return Ok(false);
    };
    Ok(birth_for_component(py, descriptor, &function, execution)?.is_some())
}

pub(crate) fn adopt(
    py: Python<'_>,
    descriptor: &Bound<'_, PyAny>,
    function: &Bound<'_, PyAny>,
    execution: &Arc<NamespaceExecution>,
) -> PyResult<()> {
    let function = authenticate_class_candidate_function(py, function)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "admitted descriptor lost its source function")
    })?;
    let witness = birth_for_component(py, descriptor, &function, execution)?.ok_or_else(|| {
        strict_runtime_unavailable(py, "admitted descriptor lost its construction birth")
    })?;
    let code = unsafe { (*function.function().as_ptr().cast::<ffi::PyFunctionObject>()).func_code };
    if unsafe {
        PySoac_AdoptBuiltinDescriptor(
            descriptor.as_ptr(),
            witness.owner().as_ptr(),
            function.function().as_ptr(),
            function.owner().as_ptr(),
            code,
        )
    } < 0
    {
        return Err(PyErr::fetch(py));
    }
    Ok(())
}
