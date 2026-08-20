//! Native generator-family protocol and its one-use compiled resume admission.
//!
//! The preserved-state capsule owns the binding. Native generator metadata owns
//! that capsule, while the exact source activation remains the only authority
//! to enter a compiled body. No Python wrapper, TLS permit, or source bytecode
//! fallback participates in this protocol.

use pyo3::{Bound, PyAny, PyErr, PyResult, Python, ffi};
use soac_core::block_py::{FunctionKind, GeneratorResumeDelivery};
use std::ffi::{c_int, c_uint, c_void};
use std::panic::{self, AssertUnwindSafe};
use std::ptr;

const GENERATOR_ABI_VERSION: c_uint = 2;
const OP_SEND: c_int = 1;
const OP_THROW: c_int = 2;
const OP_CLOSE: c_int = 3;
const PYGEN_ERROR: c_int = -1;
const PYGEN_RETURN: c_int = 0;
const PYGEN_NEXT: c_int = 1;

#[repr(C)]
struct RawPySoacGeneratorInput {
    operation: c_int,
    close_on_genexit: c_int,
    arg: *mut ffi::PyObject,
    value: *mut ffi::PyObject,
    traceback: *mut ffi::PyObject,
}

#[derive(Clone, Copy)]
#[repr(i32)]
enum NativeState {
    Unchanged = 0,
    Suspended = 1,
    Closed = 2,
}

#[derive(Clone, Copy)]
#[repr(i32)]
enum NativeSuspension {
    None = 0,
    Direct = 1,
    Delegating = 2,
    AsyncYield = 3,
}

#[repr(C)]
struct RawPySoacGeneratorResult {
    outcome: c_int,
    state: NativeState,
    suspension: NativeSuspension,
    value: *mut ffi::PyObject,
}

#[repr(C)]
struct RawPySoacGeneratorSpec {
    abi_version: c_uint,
    reserved: c_uint,
    bind: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *mut ffi::PyCodeObject,
    ) -> c_int,
    step: unsafe extern "C" fn(
        *mut ffi::PyObject,
        *mut ffi::PyObject,
        *const RawPySoacGeneratorInput,
        *mut RawPySoacGeneratorResult,
    ),
    yield_from: unsafe extern "C" fn(*mut ffi::PyObject) -> *mut ffi::PyObject,
    clear: unsafe extern "C" fn(*mut ffi::PyObject, *mut ffi::PyObject),
}

unsafe extern "C" {
    fn PyGen_NewSoacManaged(
        function: *mut ffi::PyObject,
        source_code: *mut ffi::PyCodeObject,
        name: *mut ffi::PyObject,
        qualname: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
        spec: *const RawPySoacGeneratorSpec,
    ) -> *mut ffi::PyObject;
    fn PyCoro_NewSoacManaged(
        function: *mut ffi::PyObject,
        source_code: *mut ffi::PyCodeObject,
        name: *mut ffi::PyObject,
        qualname: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
        spec: *const RawPySoacGeneratorSpec,
    ) -> *mut ffi::PyObject;
    fn PyAsyncGen_NewSoacManaged(
        function: *mut ffi::PyObject,
        source_code: *mut ffi::PyCodeObject,
        name: *mut ffi::PyObject,
        qualname: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
        spec: *const RawPySoacGeneratorSpec,
    ) -> *mut ffi::PyObject;
    fn PyAsyncGen_WrapSoacYield(value: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyGen_MatchesSoacOwner(generator: *mut ffi::PyObject, owner: *mut ffi::PyObject) -> c_int;
    fn PyGen_MarkSoacManagedTerminal(
        generator: *mut ffi::PyObject,
        owner: *mut ffi::PyObject,
    ) -> c_int;
    fn PyGen_NormalizeSoacThrow(
        typ: *mut ffi::PyObject,
        value: *mut ffi::PyObject,
        traceback: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PyGen_CloseSoacDelegate(delegate: *mut ffi::PyObject) -> c_int;
    fn PyGen_ThrowSoacDelegate(
        delegate: *mut ffi::PyObject,
        close_on_genexit: c_int,
        typ: *mut ffi::PyObject,
        value: *mut ffi::PyObject,
        traceback: *mut ffi::PyObject,
        result: *mut *mut ffi::PyObject,
    ) -> c_int;
    fn _PyGen_FetchStopIterationValue(value: *mut *mut ffi::PyObject) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Prepared,
    Idle,
    Protocol,
    Permitted,
    Admitting,
    Body,
    Retired,
}

/// One compiler-created suspended execution, owned/traversed by its existing
/// preserved-state capsule. Native metadata owns that capsule, not a Python
/// wrapper or an address-keyed side table. Source-code identity is borrowed
/// while the construction caller/native generator owns its reference.
pub(crate) struct Binding {
    function: *mut ffi::PyObject,
    kind: FunctionKind,
    no_default: *mut ffi::PyObject,
    source_code_identity: usize,
    generator: *mut ffi::PyObject,
    yieldfrom_slot: usize,
    phase: Phase,
    started: bool,
    delivery: GeneratorResumeDelivery,
    pending_exception: *mut ffi::PyObject,
    terminal_protocol_error: *mut ffi::PyObject,
}

impl Binding {
    /// The preserved capsule owns this stable slot. Its reference is retired
    /// with the invocation's other function/capture roots at terminal cleanup.
    pub(crate) unsafe fn function_owner_slot(binding: *mut Self) -> *mut *mut ffi::PyObject {
        unsafe { ptr::addr_of_mut!((*binding).function) }
    }

    pub(crate) unsafe fn is_prepared_unbound(binding: *const Self) -> bool {
        unsafe { (*binding).phase == Phase::Prepared && (*binding).generator.is_null() }
    }

    pub(crate) unsafe fn traverse(
        binding: *const Self,
        visit: ffi::visitproc,
        arg: *mut c_void,
    ) -> c_int {
        for object in unsafe {
            [
                (*binding).function,
                (*binding).no_default,
                (*binding).pending_exception,
                (*binding).terminal_protocol_error,
            ]
        } {
            if !object.is_null() {
                let result = unsafe { visit(object, arg) };
                if result != 0 {
                    return result;
                }
            }
        }
        0
    }

    pub(crate) unsafe fn matches_clear(
        binding: *const Self,
        generator: *mut ffi::PyObject,
    ) -> bool {
        // Native makes a generator terminal before invoking clear; its live
        // Matches API deliberately rejects it at this point. A failed attempt
        // to reuse our owner must not retire the original live association.
        !generator.is_null() && unsafe { (*binding).generator == generator }
    }

    unsafe fn clear(binding: *mut Self) {
        let function = unsafe { ptr::replace(&mut (*binding).function, ptr::null_mut()) };
        let no_default = unsafe { ptr::replace(&mut (*binding).no_default, ptr::null_mut()) };
        let exception = unsafe { ptr::replace(&mut (*binding).pending_exception, ptr::null_mut()) };
        let terminal_error =
            unsafe { ptr::replace(&mut (*binding).terminal_protocol_error, ptr::null_mut()) };
        unsafe {
            (*binding).phase = Phase::Retired;
            (*binding).generator = ptr::null_mut();
            (*binding).source_code_identity = 0;
            (*binding).delivery = GeneratorResumeDelivery::Ordinary;
            ffi::Py_XDECREF(function);
            ffi::Py_XDECREF(no_default);
            ffi::Py_XDECREF(exception);
            ffi::Py_XDECREF(terminal_error);
        }
    }
}

impl Drop for Binding {
    fn drop(&mut self) {
        unsafe { Self::clear(self) };
    }
}

unsafe fn binding(owner: *mut ffi::PyObject) -> Result<*mut Binding, ()> {
    let Some(binding) = (unsafe { crate::preserved_state::managed_binding(owner)? }) else {
        unsafe { error(c"generator does not own a managed suspended execution") };
        return Err(());
    };
    Ok(binding)
}

unsafe fn error(message: &std::ffi::CStr) {
    unsafe { ffi::PyErr_SetString(ffi::PyExc_RuntimeError, message.as_ptr()) };
}

unsafe fn match_live(owner: *mut ffi::PyObject, generator: *mut ffi::PyObject) -> Result<(), ()> {
    match unsafe { PyGen_MatchesSoacOwner(generator, owner) } {
        1 => Ok(()),
        0 => {
            unsafe { error(c"generator does not own this preserved state") };
            Err(())
        }
        _ => Err(()),
    }
}

pub(crate) unsafe fn new_generator<'py>(
    py: Python<'py>,
    function: &Bound<'py, PyAny>,
    kind: FunctionKind,
    source_code: &Bound<'py, PyAny>,
    name: &Bound<'py, PyAny>,
    qualname: &Bound<'py, PyAny>,
    preserved: &Bound<'py, PyAny>,
    no_default: Bound<'py, PyAny>,
    yieldfrom_slot: usize,
) -> PyResult<Bound<'py, PyAny>> {
    // The factory has already bound/checked source arguments and attached the
    // actual StrictFunctionCall snapshot. No user-visible metadata supplies
    // this binding and no later current-code/closure reread replaces it.
    let constructor = match kind {
        FunctionKind::Generator => PyGen_NewSoacManaged,
        FunctionKind::Coroutine => PyCoro_NewSoacManaged,
        FunctionKind::AsyncGenerator => PyAsyncGen_NewSoacManaged,
        FunctionKind::Function => {
            unsafe { error(c"ordinary function cannot own a suspended native object") };
            return Err(PyErr::fetch(py));
        }
    };
    let prepared = Binding {
        function: function.clone().into_ptr(),
        kind,
        no_default: no_default.into_ptr(),
        source_code_identity: source_code.as_ptr() as usize,
        generator: ptr::null_mut(),
        yieldfrom_slot,
        phase: Phase::Prepared,
        started: false,
        delivery: GeneratorResumeDelivery::Ordinary,
        pending_exception: ptr::null_mut(),
        terminal_protocol_error: ptr::null_mut(),
    };
    unsafe { crate::preserved_state::install_managed_binding(preserved.as_ptr(), prepared) }
        .map_err(|()| PyErr::fetch(py))?;
    let result = unsafe {
        constructor(
            function.as_ptr(),
            source_code.as_ptr().cast(),
            name.as_ptr(),
            qualname.as_ptr(),
            preserved.as_ptr(),
            &SPEC,
        )
    };
    if result.is_null() {
        let raised = PyErr::fetch(py);
        // Allocation/validation can fail before native invokes bind/clear.
        // Retire only our still-unbound preparation; a bound association can
        // only be retired by its exact native clear callback.
        unsafe { crate::preserved_state::abort_managed_preparation(preserved.as_ptr()) };
        Err(raised)
    } else {
        Ok(unsafe { Bound::from_owned_ptr(py, result) })
    }
}

unsafe extern "C" fn bind(
    owner: *mut ffi::PyObject,
    generator: *mut ffi::PyObject,
    function: *mut ffi::PyObject,
    source_code: *mut ffi::PyCodeObject,
) -> c_int {
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        let state = binding(owner)?;
        if (*state).phase != Phase::Prepared
            || !(*state).generator.is_null()
            || (*state).function != function
            || (*state).source_code_identity != source_code as usize
            || crate::preserved_state::strict_state_is_closed(owner)?
        {
            error(c"managed generator construction lost its prepared source snapshot");
            return Err(());
        }
        match_live(owner, generator)?;
        (*state).generator = generator;
        (*state).phase = Phase::Idle;
        Ok(())
    }));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(())) => -1,
        Err(_) => {
            unsafe { error(c"panic while binding a managed generator") };
            -1
        }
    }
}

/// Consume the sole permit from the native EXECUTING callback before any
/// compiler/admission callback. Calling the public resume helper directly,
/// even with a discovered capsule, cannot run or reenter a managed body.
pub(crate) unsafe fn consume_resume_permit(
    function: *mut ffi::PyObject,
    generator: *mut ffi::PyObject,
    owner: *mut ffi::PyObject,
) -> Result<(), ()> {
    let state = unsafe { binding(owner)? };
    if unsafe {
        (*state).phase != Phase::Permitted
            || (*state).generator != generator
            || (*state).function != function
    } {
        unsafe { error(c"managed generator resume requires its active native step") };
        return Err(());
    }
    unsafe {
        match_live(owner, generator)?;
        (*state).phase = Phase::Admitting;
    }
    Ok(())
}

/// Called immediately before the actual native resume entry. Compilation or
/// snapshot-admission failure before this point leaves a generator unresumed.
pub(crate) unsafe fn mark_body_entry(owner: *mut ffi::PyObject) -> Result<(), ()> {
    let state = unsafe { binding(owner)? };
    if unsafe { (*state).phase != Phase::Admitting } {
        unsafe { error(c"managed generator entered without consuming its resume permit") };
        return Err(());
    }
    unsafe {
        (*state).phase = Phase::Body;
        (*state).started = true;
    }
    Ok(())
}

pub(crate) unsafe fn delivery(owner: *mut ffi::PyObject) -> Result<GeneratorResumeDelivery, ()> {
    let Some(state) = (unsafe { crate::preserved_state::managed_binding(owner)? }) else {
        return Ok(GeneratorResumeDelivery::Ordinary);
    };
    if unsafe { (*state).phase != Phase::Body } {
        unsafe { error(c"managed exception delivery is only visible inside its active body") };
        return Err(());
    }
    Ok(unsafe { (*state).delivery })
}

/// The lowering has first copied the resume operand to an Operand-lifetime
/// temporary and cleared its original control parameter. Transfer our owner
/// into the raised-error indicator, never keep a callback-local PyErr/Bound
/// alive across a resumed source handler.
pub(crate) unsafe fn inject_resume_exception(
    owner: *mut ffi::PyObject,
    exception: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let Ok(state) = (unsafe { binding(owner) }) else {
        return ptr::null_mut();
    };
    if exception.is_null()
        || unsafe {
            (*state).phase != Phase::Body
                || (*state).pending_exception != exception
                || (*state).delivery == GeneratorResumeDelivery::Ordinary
        }
    {
        unsafe { error(c"managed exception injection lost its owned resume operand") };
        return ptr::null_mut();
    }
    let owned = unsafe { ptr::replace(&mut (*state).pending_exception, ptr::null_mut()) };
    // This consumes owned on every path. Its conditional context chaining
    // uses this execution's active item, never the inherited caller item.
    unsafe { crate::preserved_state::inject_managed_exception_owned(owner, owned) };
    ptr::null_mut()
}

/// Runs at the explicit source async-yield operation before suspension. A
/// failed allocation follows that operation's source exception edge; the
/// native step later validates this exact wrapped token without allocating.
pub(crate) unsafe fn wrap_async_yield(value: *mut ffi::PyObject) -> *mut ffi::PyObject {
    unsafe { PyAsyncGen_WrapSoacYield(value) }
}

/// Publish native closed/frame-cleared visibility before *any* terminal
/// handled-item, local, or capture release can run a finalizer. Native retains
/// its owner until step returns; this notification never clears Rust storage.
pub(crate) unsafe fn mark_terminal(owner: *mut ffi::PyObject) -> Result<(), ()> {
    let Some(state) = (unsafe { crate::preserved_state::managed_binding(owner)? }) else {
        return Ok(());
    };
    if unsafe { (*state).phase != Phase::Body || (*state).generator.is_null() } {
        unsafe { error(c"managed generator terminated outside its active body") };
        return Err(());
    }
    if unsafe { PyGen_MarkSoacManagedTerminal((*state).generator, owner) } < 0 {
        return Err(());
    }
    Ok(())
}

/// The compiler's terminal cleanup sites cannot enter another source handler.
/// Publish native closed state before their first possible callback. A native
/// protocol failure is a separate owned exit result, not a replacement for the
/// source exception observed by finalizers during this cleanup.
pub(crate) unsafe fn notify_terminal(owner: *mut ffi::PyObject) {
    if owner.is_null() {
        return;
    }
    let raised = unsafe { ffi::PyErr_GetRaisedException() };
    if let Ok(Some(state)) = unsafe { crate::preserved_state::managed_binding(owner) }
        && unsafe { (*state).terminal_protocol_error.is_null() }
        && unsafe { mark_terminal(owner) }.is_err()
    {
        let failure = unsafe { ffi::PyErr_GetRaisedException() };
        // No allocation or Python callback occurs on a valid native Mark. Do
        // not hold a Rust reference across even its rejected-operation path.
        if let Ok(Some(state)) = unsafe { crate::preserved_state::managed_binding(owner) } {
            if unsafe { (*state).terminal_protocol_error.is_null() } {
                unsafe { (*state).terminal_protocol_error = failure };
            } else {
                unsafe { ffi::Py_XDECREF(failure) };
            }
        } else {
            unsafe { ffi::Py_XDECREF(failure) };
        }
    }
    unsafe { ffi::PyErr_SetRaisedException(raised) };
}

/// Take a deferred native-protocol error exactly once, before native clear can
/// retire the association. A successful source result must never escape with
/// a pending protocol error. Both result and old error are released while the
/// new error is owned locally, so their callbacks cannot overwrite it.
pub(crate) unsafe fn finish_body_result(
    owner: *mut ffi::PyObject,
    result: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    let raised = unsafe { ffi::PyErr_GetRaisedException() };
    let failure = match unsafe { binding(owner) } {
        Ok(state) => unsafe {
            ptr::replace(&mut (*state).terminal_protocol_error, ptr::null_mut())
        },
        Err(()) => unsafe { ffi::PyErr_GetRaisedException() },
    };
    if failure.is_null() {
        unsafe { ffi::PyErr_SetRaisedException(raised) };
        return result;
    }
    unsafe {
        ffi::Py_XDECREF(result);
        ffi::Py_XDECREF(raised);
        ffi::PyErr_SetRaisedException(failure);
    }
    ptr::null_mut()
}

struct ProtocolGuard(*mut ffi::PyObject);

impl Drop for ProtocolGuard {
    fn drop(&mut self) {
        // No Rust reference into binding storage is held over Python calls.
        // An explicit GC clear can remove the binding during a callback.
        let raised = unsafe { ffi::PyErr_GetRaisedException() };
        if let Ok(Some(state)) = unsafe { crate::preserved_state::managed_binding(self.0) } {
            unsafe {
                let exception = ptr::replace(&mut (*state).pending_exception, ptr::null_mut());
                if (*state).phase != Phase::Retired {
                    (*state).phase = Phase::Idle;
                    (*state).delivery = GeneratorResumeDelivery::Ordinary;
                }
                ffi::Py_XDECREF(exception);
            }
        }
        unsafe { ffi::PyErr_SetRaisedException(raised) };
    }
}

struct StepFailure {
    error: PyErr,
    state: NativeState,
}

impl StepFailure {
    fn fetch(py: Python<'_>, state: NativeState) -> Self {
        Self {
            error: PyErr::fetch(py),
            state,
        }
    }
}

enum StepValue<'py> {
    Yield(Bound<'py, PyAny>, NativeSuspension),
    Return(Bound<'py, PyAny>),
}

// Protocol execution is deliberately below, separate from owned binding and
// entry admission. Its delegate callbacks run before the body links its item.

unsafe fn normalize_throw<'py>(
    py: Python<'py>,
    input: &RawPySoacGeneratorInput,
) -> Result<Bound<'py, PyAny>, StepFailure> {
    let exception = unsafe { PyGen_NormalizeSoacThrow(input.arg, input.value, input.traceback) };
    if exception.is_null() {
        Err(StepFailure::fetch(py, NativeState::Unchanged))
    } else {
        Ok(unsafe { Bound::from_owned_ptr(py, exception) })
    }
}

unsafe fn run_body<'py>(
    py: Python<'py>,
    owner: *mut ffi::PyObject,
    generator: *mut ffi::PyObject,
    function: *mut ffi::PyObject,
    no_default: *mut ffi::PyObject,
    send_value: *mut ffi::PyObject,
    exception: Option<Bound<'py, PyAny>>,
    delivery: GeneratorResumeDelivery,
) -> Result<StepValue<'py>, StepFailure> {
    let state =
        unsafe { binding(owner) }.map_err(|()| StepFailure::fetch(py, NativeState::Unchanged))?;
    if unsafe { (*state).phase != Phase::Protocol || !(*state).pending_exception.is_null() } {
        unsafe { error(c"managed generator protocol changed before body admission") };
        return Err(StepFailure::fetch(py, NativeState::Closed));
    }
    unsafe {
        (*state).delivery = delivery;
        (*state).phase = Phase::Permitted;
        (*state).pending_exception = exception.map_or(ptr::null_mut(), Bound::into_ptr);
    }
    // Transfer the sole callback owner into the GC-visible activation. The
    // exceptional-resume helper takes it before the source handler runs.
    let resume_exc = unsafe {
        if (*state).pending_exception.is_null() {
            no_default
        } else {
            (*state).pending_exception
        }
    };
    let kind = unsafe { (*state).kind };
    let result = unsafe {
        if kind == FunctionKind::AsyncGenerator {
            // Native ASend chooses the operation's initial send operand. At
            // later await suspensions it supplies the transport operand; the
            // explicit resume CFG decides which input is consumed.
            crate::resume_async_generator(
                function, generator, owner, send_value, resume_exc, send_value,
            )
        } else {
            crate::resume_generator(function, generator, owner, send_value, resume_exc)
        }
    };
    // Fetch errors before any later cleanup/decref can run a callback. Genuine
    // GeneratorReturn sets the preserved closed slot; an escaping source
    // StopIteration has already gone through PEP 479 instead.
    let raised = result.is_null().then(|| PyErr::fetch(py));
    let closed = unsafe { crate::preserved_state::strict_state_is_closed(owner) };
    if closed.is_err() {
        let inspection_error = PyErr::fetch(py);
        unsafe { ffi::Py_XDECREF(result) };
        return Err(StepFailure {
            error: raised.unwrap_or(inspection_error),
            state: NativeState::Closed,
        });
    }
    if !result.is_null() {
        if closed != Ok(false) {
            unsafe {
                ffi::Py_DECREF(result);
                error(c"managed generator yielded after closing its preserved state");
            }
            return Err(StepFailure::fetch(py, NativeState::Closed));
        }
        if let Err(()) = unsafe { require_consumed_exception(owner) } {
            let failure = StepFailure::fetch(py, NativeState::Closed);
            unsafe { ffi::Py_DECREF(result) };
            return Err(failure);
        }
        let value = unsafe { Bound::from_owned_ptr(py, result) };
        let delegate = unsafe { yield_from_impl(py, owner) }.map_err(|error| StepFailure {
            error,
            state: NativeState::Closed,
        })?;
        // This is the compiler-owned active-delegation slot, not a PC or
        // yielded-value recognizer. Native validates the family's result tag.
        let suspension = if delegate.as_ptr() != unsafe { ffi::Py_None() } {
            NativeSuspension::Delegating
        } else if kind == FunctionKind::AsyncGenerator {
            NativeSuspension::AsyncYield
        } else {
            NativeSuspension::Direct
        };
        return Ok(StepValue::Yield(value, suspension));
    }
    let raised = raised.expect("a NULL resume result was fetched before state inspection");
    if closed == Ok(true) && raised.is_instance_of::<pyo3::exceptions::PyStopIteration>(py) {
        unsafe { require_consumed_exception(owner) }
            .map_err(|()| StepFailure::fetch(py, NativeState::Closed))?;
        raised.restore(py);
        let mut returned = ptr::null_mut();
        if unsafe { _PyGen_FetchStopIterationValue(&mut returned) } < 0 {
            return Err(StepFailure::fetch(py, NativeState::Closed));
        }
        return Ok(StepValue::Return(unsafe {
            Bound::from_owned_ptr(py, returned)
        }));
    }
    let entered = match unsafe { binding(owner) } {
        Ok(state) => unsafe { (*state).phase == Phase::Body },
        Err(()) => {
            unsafe { ffi::PyErr_Clear() };
            true
        }
    };
    Err(StepFailure {
        error: raised,
        state: if entered || closed != Ok(false) {
            NativeState::Closed
        } else {
            NativeState::Unchanged
        },
    })
}

unsafe fn require_consumed_exception(owner: *mut ffi::PyObject) -> Result<(), ()> {
    let state = unsafe { binding(owner)? };
    if unsafe { !(*state).pending_exception.is_null() } {
        unsafe { error(c"managed generator completed a step without delivering its exception") };
        return Err(());
    }
    Ok(())
}

unsafe fn step_impl<'py>(
    py: Python<'py>,
    owner: *mut ffi::PyObject,
    generator: *mut ffi::PyObject,
    input: &RawPySoacGeneratorInput,
) -> Result<StepValue<'py>, StepFailure> {
    let state =
        unsafe { binding(owner) }.map_err(|()| StepFailure::fetch(py, NativeState::Closed))?;
    unsafe { match_live(owner, generator) }
        .map_err(|()| StepFailure::fetch(py, NativeState::Closed))?;
    if unsafe { (*state).generator != generator || (*state).phase != Phase::Idle } {
        unsafe { error(c"managed generator protocol is already active or retired") };
        return Err(StepFailure::fetch(py, NativeState::Closed));
    }
    let started = unsafe { (*state).started };
    // The native operation pins the preserved capsule. Its binding owns these
    // inputs until body admission; the active call then owns the function.
    // Do not clone a protocol-local function reference unnecessarily.
    // Neither pointer is read again after the body returns.
    let function = unsafe { (*state).function };
    let no_default = unsafe { (*state).no_default };
    unsafe { (*state).phase = Phase::Protocol };
    let _guard = ProtocolGuard(owner);

    if input.operation == OP_SEND {
        return unsafe {
            run_body(
                py,
                owner,
                generator,
                function,
                no_default,
                input.arg,
                None,
                GeneratorResumeDelivery::Ordinary,
            )
        };
    }
    if input.operation == OP_THROW && !started {
        let exception = unsafe { normalize_throw(py, input)? };
        PyErr::from_value(exception).restore(py);
        return Err(StepFailure::fetch(py, NativeState::Closed));
    }
    if !matches!(input.operation, OP_THROW | OP_CLOSE) {
        unsafe { error(c"unknown managed generator operation") };
        return Err(StepFailure::fetch(py, NativeState::Unchanged));
    }

    let delegate = unsafe { yield_from_impl(py, owner) }.map_err(|error| StepFailure {
        error,
        state: NativeState::Unchanged,
    })?;
    let has_delegate = delegate.as_ptr() != unsafe { ffi::Py_None() };
    let delivery = if has_delegate {
        GeneratorResumeDelivery::YieldFromException
    } else {
        GeneratorResumeDelivery::DirectRaise
    };
    let closing = input.operation == OP_CLOSE
        || (input.close_on_genexit != 0
            && unsafe { ffi::PyErr_GivenExceptionMatches(input.arg, ffi::PyExc_GeneratorExit) }
                != 0);
    if closing {
        if has_delegate && unsafe { PyGen_CloseSoacDelegate(delegate.as_ptr()) } < 0 {
            let exception = PyErr::fetch(py).into_value(py).into_bound(py).into_any();
            drop(delegate);
            return unsafe {
                run_body(
                    py,
                    owner,
                    generator,
                    function,
                    no_default,
                    ffi::Py_None(),
                    Some(exception),
                    delivery,
                )
            };
        }
        drop(delegate);
        let exception = if input.operation == OP_CLOSE {
            // Native close uses SetNone, not throw normalization: this retains
            // its caller context until the body's own item replaces it.
            unsafe { ffi::PyErr_SetNone(ffi::PyExc_GeneratorExit) };
            PyErr::fetch(py).into_value(py).into_bound(py).into_any()
        } else {
            unsafe { normalize_throw(py, input)? }
        };
        return unsafe {
            run_body(
                py,
                owner,
                generator,
                function,
                no_default,
                ffi::Py_None(),
                Some(exception),
                delivery,
            )
        };
    }

    if has_delegate {
        let mut yielded = ptr::null_mut();
        let status = unsafe {
            PyGen_ThrowSoacDelegate(
                delegate.as_ptr(),
                input.close_on_genexit,
                input.arg,
                input.value,
                input.traceback,
                &mut yielded,
            )
        };
        match status {
            -1 => return Err(StepFailure::fetch(py, NativeState::Unchanged)),
            1 if !yielded.is_null() => {
                return Ok(StepValue::Yield(
                    unsafe { Bound::from_owned_ptr(py, yielded) },
                    NativeSuspension::Delegating,
                ));
            }
            1 => {
                let exception = PyErr::fetch(py).into_value(py).into_bound(py).into_any();
                drop(delegate);
                return unsafe {
                    run_body(
                        py,
                        owner,
                        generator,
                        function,
                        no_default,
                        ffi::Py_None(),
                        Some(exception),
                        delivery,
                    )
                };
            }
            0 => {}
            _ => {
                unsafe { error(c"native generator delegate returned an invalid outcome") };
                return Err(StepFailure::fetch(py, NativeState::Closed));
            }
        }
    }
    drop(delegate);
    let exception = unsafe { normalize_throw(py, input)? };
    unsafe {
        run_body(
            py,
            owner,
            generator,
            function,
            no_default,
            ffi::Py_None(),
            Some(exception),
            delivery,
        )
    }
}

unsafe extern "C" fn step(
    owner: *mut ffi::PyObject,
    generator: *mut ffi::PyObject,
    input: *const RawPySoacGeneratorInput,
    result: *mut RawPySoacGeneratorResult,
) {
    let py = unsafe { Python::assume_attached() };
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        step_impl(py, owner, generator, &*input)
    }));
    let value = match outcome {
        Ok(Ok(StepValue::Yield(value, suspension))) => RawPySoacGeneratorResult {
            outcome: PYGEN_NEXT,
            state: NativeState::Suspended,
            suspension,
            value: value.into_ptr(),
        },
        Ok(Ok(StepValue::Return(value))) => RawPySoacGeneratorResult {
            outcome: PYGEN_RETURN,
            state: NativeState::Closed,
            suspension: NativeSuspension::None,
            value: value.into_ptr(),
        },
        Ok(Err(failure)) => {
            failure.error.restore(py);
            RawPySoacGeneratorResult {
                outcome: PYGEN_ERROR,
                state: failure.state,
                suspension: NativeSuspension::None,
                value: ptr::null_mut(),
            }
        }
        Err(_) => {
            unsafe { error(c"panic while resuming a managed generator") };
            RawPySoacGeneratorResult {
                outcome: PYGEN_ERROR,
                state: NativeState::Closed,
                suspension: NativeSuspension::None,
                value: ptr::null_mut(),
            }
        }
    };
    unsafe { result.write(value) };
}

unsafe fn yield_from_impl<'py>(
    py: Python<'py>,
    owner: *mut ffi::PyObject,
) -> PyResult<Bound<'py, PyAny>> {
    let state = unsafe { binding(owner) }.map_err(|()| PyErr::fetch(py))?;
    let slot = unsafe { (*state).yieldfrom_slot };
    unsafe {
        Bound::from_owned_ptr_or_err(
            py,
            crate::preserved_state::load_preserved_state_owned(owner, slot as i64),
        )
    }
}

unsafe extern "C" fn yield_from(owner: *mut ffi::PyObject) -> *mut ffi::PyObject {
    let py = unsafe { Python::assume_attached() };
    match panic::catch_unwind(AssertUnwindSafe(|| unsafe { yield_from_impl(py, owner) })) {
        Ok(Ok(value)) => value.into_ptr(),
        Ok(Err(error)) => {
            error.restore(py);
            ptr::null_mut()
        }
        Err(_) => {
            unsafe { error(c"panic while inspecting a managed generator delegate") };
            ptr::null_mut()
        }
    }
}

unsafe extern "C" fn clear(owner: *mut ffi::PyObject, generator: *mut ffi::PyObject) {
    // Native has already made this exact generator terminal and preserves the
    // pending error through our callback and its own owner/result decrefs.
    let result = panic::catch_unwind(AssertUnwindSafe(|| unsafe {
        crate::preserved_state::clear_managed_binding(owner, generator)
    }));
    if result.is_err() {
        unsafe {
            error(c"panic while retiring a managed generator");
            ffi::PyErr_WriteUnraisable(owner);
        }
    }
}

static SPEC: RawPySoacGeneratorSpec = RawPySoacGeneratorSpec {
    abi_version: GENERATOR_ABI_VERSION,
    reserved: 0,
    bind,
    step,
    yield_from,
    clear,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    struct FinalizerProbe(Arc<AtomicUsize>);

    unsafe extern "C" fn destroy_probe(capsule: *mut ffi::PyObject) {
        let probe = unsafe {
            ffi::PyCapsule_GetPointer(capsule, c"soac.test.ManagedExitProbe".as_ptr())
                .cast::<FinalizerProbe>()
        };
        if !probe.is_null() {
            let probe = unsafe { Box::from_raw(probe) };
            probe.0.fetch_add(1, Ordering::SeqCst);
            unsafe { error(c"error written by terminal result destructor") };
        }
    }

    fn probe<'py>(py: Python<'py>, count: &Arc<AtomicUsize>) -> Bound<'py, PyAny> {
        let state = Box::into_raw(Box::new(FinalizerProbe(count.clone())));
        let capsule = unsafe {
            ffi::PyCapsule_New(
                state.cast(),
                c"soac.test.ManagedExitProbe".as_ptr(),
                Some(destroy_probe),
            )
        };
        if capsule.is_null() {
            drop(unsafe { Box::from_raw(state) });
        }
        unsafe { Bound::from_owned_ptr_or_err(py, capsule) }.expect("probe capsule")
    }

    unsafe extern "C" fn collect_edge(object: *mut ffi::PyObject, data: *mut c_void) -> c_int {
        unsafe { (&mut *data.cast::<Vec<*mut ffi::PyObject>>()).push(object) };
        0
    }

    #[test]
    fn terminal_protocol_failure_retains_first_error_and_consumes_exit_owners_once() {
        let _lock = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let mut builder = crate::preserved_state::PreservedStateBuilder::with_capacity(1, &[])
                .expect("state allocation");
            builder.push_i64(0);
            let owner = Bound::<PyAny>::from_owned_ptr_or_err(py, builder.into_capsule())
                .expect("state capsule");
            // This unit exercises owned protocol failure, not source admission.
            // The deliberately unbound pair cannot execute any source body.
            crate::preserved_state::attach_strict_resume_state(
                owner.as_ptr(),
                crate::strict_function::StrictSuspendedFunctionSnapshot::snapshot_with_references(
                    py,
                    vec![py.None()],
                ),
                0,
            )
            .unwrap();
            crate::preserved_state::install_managed_binding(
                owner.as_ptr(),
                Binding {
                    function: py.None().into_ptr(),
                    kind: FunctionKind::Generator,
                    no_default: py.None().into_ptr(),
                    source_code_identity: 0,
                    generator: ptr::null_mut(),
                    yieldfrom_slot: 0,
                    phase: Phase::Prepared,
                    started: false,
                    delivery: GeneratorResumeDelivery::Ordinary,
                    pending_exception: ptr::null_mut(),
                    terminal_protocol_error: ptr::null_mut(),
                },
            )
            .unwrap();

            let source_drops = Arc::new(AtomicUsize::new(0));
            let result_drops = Arc::new(AtomicUsize::new(0));
            let source_payload = probe(py, &source_drops);
            let source_error = pyo3::exceptions::PyValueError::new_err((source_payload.unbind(),));
            let source_error_pointer = source_error.value(py).as_ptr();
            let result = probe(py, &result_drops).into_ptr();
            source_error.restore(py);

            notify_terminal(owner.as_ptr());
            let state = binding(owner.as_ptr()).unwrap();
            let failure = (*state).terminal_protocol_error;
            assert!(!failure.is_null());
            let pending = ffi::PyErr_GetRaisedException();
            assert_eq!(pending, source_error_pointer);
            ffi::PyErr_SetRaisedException(pending);
            notify_terminal(owner.as_ptr());
            assert_eq!((*state).terminal_protocol_error, failure);
            let mut edges = Vec::<*mut ffi::PyObject>::new();
            assert_eq!(
                Binding::traverse(state, collect_edge, ptr::from_mut(&mut edges).cast()),
                0
            );
            assert_eq!(edges.iter().filter(|&&object| object == failure).count(), 1);
            assert_eq!(source_drops.load(Ordering::SeqCst), 0);
            assert_eq!(result_drops.load(Ordering::SeqCst), 0);

            assert!(finish_body_result(owner.as_ptr(), result).is_null());
            assert!((*state).terminal_protocol_error.is_null());
            assert_eq!(source_drops.load(Ordering::SeqCst), 1);
            assert_eq!(result_drops.load(Ordering::SeqCst), 1);
            let pending = ffi::PyErr_GetRaisedException();
            assert_eq!(
                pending, failure,
                "destructors cannot replace the first protocol failure"
            );
            ffi::PyErr_SetRaisedException(pending);
            assert!(finish_body_result(owner.as_ptr(), ptr::null_mut()).is_null());
            let pending = ffi::PyErr_GetRaisedException();
            assert_eq!(pending, failure);
            ffi::Py_DECREF(pending);
            drop(owner);
            assert!(ffi::PyErr_Occurred().is_null());
            assert_eq!(source_drops.load(Ordering::SeqCst), 1);
            assert_eq!(result_drops.load(Ordering::SeqCst), 1);
        });
    }
}
