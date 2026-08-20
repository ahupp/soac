//! Authenticated positional source-body calls with ordinary argument binding.
//!
//! Target and entry guards establish callable identity, never argument or return
//! types. The captured activation owns its ordinary binder/environment lifetime.

use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::mem::offset_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;

use crate::strict_function::StrictFunctionCall;
use pyo3::ffi;
use pyo3::prelude::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StrictFunctionCallStatistics {
    pub direct_body_calls: u64,
    pub fixed_body_calls: u64,
}

#[derive(Default)]
pub(crate) struct StrictCallCounters(Cell<StrictFunctionCallStatistics>);

impl StrictCallCounters {
    pub(crate) fn direct_body(&self, fixed_target: bool) {
        let mut counts = self.0.get();
        counts.direct_body_calls = counts.direct_body_calls.saturating_add(1);
        if fixed_target {
            counts.fixed_body_calls = counts.fixed_body_calls.saturating_add(1);
        }
        self.0.set(counts);
    }

    pub(crate) fn snapshot(&self) -> StrictFunctionCallStatistics {
        self.0.get()
    }
}

#[repr(C)]
pub(crate) struct RawStrictPreparedCall {
    pub environment: *mut c_void,
    pub entry: *const u8,
    pub activation: *mut c_void,
}

impl RawStrictPreparedCall {
    pub(crate) const ENVIRONMENT_OFFSET: usize = offset_of!(Self, environment);
    pub(crate) const ENTRY_OFFSET: usize = offset_of!(Self, entry);
    pub(crate) const ACTIVATION_OFFSET: usize = offset_of!(Self, activation);
}

struct PreparedCallOwner {
    arguments: *mut *mut ffi::PyObject,
    count: usize,
    output: *mut RawStrictPreparedCall,
    armed: bool,
}

impl Drop for PreparedCallOwner {
    fn drop(&mut self) {
        if self.armed {
            unsafe {
                let activation = ptr::replace(&mut (*self.output).activation, ptr::null_mut());
                dp_jit_retire_strict_call_arguments(activation);
                crate::cleanup_output_args(self.arguments, self.count);
                if !activation.is_null() {
                    drop(Box::from_raw(activation.cast::<StrictFunctionCall>()));
                }
                (*self.output).entry = ptr::null();
                (*self.output).environment = ptr::null_mut();
            }
        }
    }
}

/// Prepare the fixed positional body ABI of the already captured function.
/// The output capacity is the full bound arity, independently of the supplied
/// argument count; the ordinary binder reads current defaults and validates arity.
/// A null expected entry means no prior method lookup supplied a witness, not
/// permission to bypass current public/private entry authentication.
/// Return 0 before binding for unsupported shapes/public overrides; -1 retains
/// the actual binding error and must never replay the call; 1 owns the
/// output arguments and activation until `dp_jit_finish_strict_direct_call`.
///
/// # Safety
/// All inputs/outputs are generated native-stack buffers of the stated sizes;
/// The callee and arguments
/// remain owned by the caller throughout. The native recursion guard must run
/// before preparation, just as it does at the public source-owned trampoline.
pub(crate) unsafe extern "C" fn dp_jit_prepare_strict_direct_call(
    callable: *mut ffi::PyObject,
    args: *const *mut ffi::PyObject,
    nargs: usize,
    out_capacity: usize,
    expected_entry: *const c_void,
    expected_body: *const u8,
    out_args: *mut *mut ffi::PyObject,
    out: *mut RawStrictPreparedCall,
) -> c_int {
    let py = unsafe { Python::assume_attached() };
    let result = catch_unwind(AssertUnwindSafe(|| -> PyResult<c_int> {
        unsafe {
            if callable.is_null()
                || out.is_null()
                || out_args.is_null()
                || out_capacity > i64::MAX as usize
                || (nargs != 0 && args.is_null())
            {
                return Err(crate::strict_runtime_unavailable(
                    py,
                    "invalid strict direct-call buffers",
                ));
            }
            out.write(RawStrictPreparedCall {
                environment: ptr::null_mut(),
                entry: ptr::null(),
                activation: ptr::null_mut(),
            });
            let function = Bound::<PyAny>::from_borrowed_ptr(py, callable);
            let Some(checked_entry) = crate::private_checked_vectorcall_entry(&function)? else {
                return Ok(0);
            };
            let public_entry = (*callable.cast::<ffi::PyFunctionObject>()).vectorcall;
            if (!expected_entry.is_null() && checked_entry as *const c_void != expected_entry)
                || !public_entry.is_some_and(|public| ptr::fn_addr_eq(public, checked_entry))
            {
                return Ok(0);
            }
            let data = crate::py_function_jit_extra(callable).map_err(|()| PyErr::fetch(py))?;
            // This successful ownership query and the payload reads below
            // contain no callback; binding takes its own owning snapshot.
            let entry = (*data).function_env.direct_code_ptr();
            let plan = (*data).function_template.binding_plan();
            if entry.is_null()
                || !plan.has_fixed_positional_body_abi()
                || plan.param_count() != out_capacity
            {
                return Ok(0);
            }
            let environment = crate::bind_direct_args_from_vectorcall(
                callable.cast(),
                args.cast(),
                nargs,
                ptr::null_mut(),
                data.cast(),
                out_args.cast(),
                out_capacity as i64,
                ptr::addr_of_mut!((*out).activation),
            );
            if environment.is_null() {
                return Err(PyErr::fetch(py));
            }
            // Binding transferred ownership before any fallible observation below.
            // Errors and panics must consume it, never replay or leak the call.
            let mut owned = PreparedCallOwner {
                arguments: out_args,
                count: out_capacity,
                output: out,
                armed: true,
            };
            if (*out).activation.is_null() {
                return Err(crate::strict_runtime_unavailable(
                    py,
                    "strict direct call lost its activation",
                ));
            }
            let activation = &*(*out).activation.cast::<StrictFunctionCall>();
            // Use the body pinned by the actual activation, not a pointer
            // observed before binding. Its compiled-handle owner must match
            // the environment even if compilation metadata was refreshed.
            let entry = activation.environment().direct_code_ptr();
            if entry.is_null() {
                return Err(crate::strict_runtime_unavailable(
                    py,
                    "prepared strict activation has no native body",
                ));
            }
            (*out).environment = environment;
            (*out).entry = entry;
            activation.record_direct_body(py, entry == expected_body)?;
            owned.armed = false;
            Ok(1)
        }
    }));
    match result {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            error.restore(py);
            -1
        }
        Err(_) => {
            crate::strict_runtime_unavailable(py, "panic preparing strict direct call").restore(py);
            -1
        }
    }
}

/// Consume one successful preparation after its body, preserving binder cleanup
/// and the body exception.
pub(crate) unsafe extern "C" fn dp_jit_finish_strict_direct_call(
    activation: *mut c_void,
    arguments: *mut *mut ffi::PyObject,
    count: usize,
    result: *mut c_void,
) -> *mut c_void {
    unsafe {
        dp_jit_retire_strict_call_arguments(activation);
        crate::cleanup_output_args(arguments, count);
        crate::strict_function::strict_finish_call(activation, result)
    }
}

/// Retire the active binder view before releasing its owned arguments.
/// Null denotes the ordinary non-strict trampoline.
pub(crate) unsafe extern "C" fn dp_jit_retire_strict_call_arguments(activation: *mut c_void) {
    if let Some(activation) = unsafe { activation.cast::<StrictFunctionCall>().as_mut() } {
        activation.retire_bound_arguments();
    }
}
