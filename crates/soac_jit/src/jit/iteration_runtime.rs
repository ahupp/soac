//! Native iterator stepping with a borrowed compiler-owned receiver.
//!
//! Keep the internal loop's item-or-pending-exception ABI. `PyIter_Next` is
//! deliberately not used: it clears StopIteration instead of passing that
//! exception to the already selected exhaustion continuation.

use super::imports::{ImportSpec, SigType};
use cranelift_jit::JITBuilder;
use pyo3::ffi;
use std::ptr;

pub(super) static STEP: ImportSpec = ImportSpec::new(
    "dp_jit_iterator_step",
    &[SigType::Pointer],
    &[SigType::Pointer],
);

/// The caller pins the exact iterator through a validated Operand slot. This
/// helper neither clones nor consumes that owner, including during callbacks.
pub(super) unsafe fn step_borrowed(iterator: *mut ffi::PyObject) -> *mut ffi::PyObject {
    unsafe { dp_jit_iterator_step(iterator) }
}

unsafe extern "C" fn dp_jit_iterator_step(iterator: *mut ffi::PyObject) -> *mut ffi::PyObject {
    if iterator.is_null() {
        unsafe {
            ffi::PyErr_SetString(ffi::PyExc_SystemError, c"empty iterator Operand".as_ptr());
        }
        return ptr::null_mut();
    }
    if unsafe { ffi::PyIter_Check(iterator) } == 0 {
        unsafe {
            ffi::PyErr_Format(
                ffi::PyExc_TypeError,
                c"'%.200s' object is not an iterator".as_ptr(),
                (*ffi::Py_TYPE(iterator)).tp_name,
            );
        }
        return ptr::null_mut();
    }
    // Match the canonical one-argument builtin_next primitive. Reload the
    // actual type slot on each step; arbitrary user iterators are not stable
    // protocol-method plans merely because the compiler owns their loop slot.
    let next = unsafe { (*ffi::Py_TYPE(iterator)).tp_iternext }
        .expect("PyIter_Check requires a native next slot");
    let item = unsafe { next(iterator) };
    if item.is_null() && unsafe { ffi::PyErr_Occurred() }.is_null() {
        unsafe { ffi::PyErr_SetNone(ffi::PyExc_StopIteration) };
    }
    item
}

pub(super) fn primitive_bindings() -> [(&'static ImportSpec, *const u8); 1] {
    [(&STEP, dp_jit_iterator_step as *const u8)]
}

pub(super) fn register_symbols(builder: &mut JITBuilder) {
    for (spec, address) in primitive_bindings() {
        builder.symbol(spec.symbol, address);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::prelude::*;
    use pyo3::types::{PyList, PyModule, PyTuple};

    const SOURCE: &std::ffi::CStr = c"\
calls = []
token = object()
stop = StopIteration(('native', 7))
error = ValueError('native iterator error')
class Receiver:
    def __init__(self, kind):
        self.kind = kind
    def __iter__(self):
        return self
    def __next__(self):
        calls.append(self.kind)
        if self.kind == 'value':
            return token
        if self.kind == 'stop':
            raise stop
        raise error
";

    #[test]
    fn iterator_step_preserves_callback_result_error_and_releases_temporary_owners() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let module =
                PyModule::from_code(py, SOURCE, c"iterator_step.py", c"iterator_step").unwrap();
            let builtin = py.import("builtins").unwrap().getattr("next").unwrap();
            let receiver = module.getattr("Receiver").unwrap();
            let token = module.getattr("token").unwrap();
            let calls = module
                .getattr("calls")
                .unwrap()
                .cast_into::<PyList>()
                .unwrap();
            for kind in ["value", "stop", "error"] {
                for native in [true, false] {
                    calls.call_method0("clear").unwrap();
                    let iterator = receiver.call1((kind,)).unwrap();
                    let before = ffi::Py_REFCNT(iterator.as_ptr());
                    let result = if native {
                        let arguments = [iterator.as_ptr()];
                        ffi::PyObject_Vectorcall(
                            builtin.as_ptr(),
                            arguments.as_ptr(),
                            1,
                            ptr::null_mut(),
                        )
                    } else {
                        step_borrowed(iterator.as_ptr())
                    };
                    // Fetch before any observer call. Neither the helper nor
                    // test cleanup may overwrite the original pending error.
                    let pending = ffi::PyErr_GetRaisedException();
                    assert_eq!(calls.len(), 1, "one next callback for {kind}");
                    assert_eq!(
                        calls.get_item(0).unwrap().extract::<String>().unwrap(),
                        kind
                    );
                    if kind == "value" {
                        assert!(pending.is_null());
                        assert_eq!(result, token.as_ptr());
                        ffi::Py_DECREF(result);
                    } else {
                        assert!(result.is_null());
                        let expected = module.getattr(kind).unwrap();
                        assert_eq!(pending, expected.as_ptr());
                        ffi::Py_DECREF(pending);
                        expected.setattr("__traceback__", py.None()).unwrap();
                    }
                    assert_eq!(ffi::Py_REFCNT(iterator.as_ptr()), before);
                }
            }
        });
    }

    #[test]
    fn iterator_step_keeps_exhaustion_pending_and_rejects_non_iterator_without_consuming_it() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let tuple = PyTuple::empty(py);
            let iterator =
                Bound::<PyAny>::from_owned_ptr(py, ffi::PyObject_GetIter(tuple.as_ptr()));
            let before = ffi::Py_REFCNT(iterator.as_ptr());
            assert!(step_borrowed(iterator.as_ptr()).is_null());
            let pending = ffi::PyErr_GetRaisedException();
            assert!(!pending.is_null());
            assert_eq!(
                ffi::PyObject_IsInstance(pending, ffi::PyExc_StopIteration),
                1
            );
            ffi::Py_DECREF(pending);
            assert_eq!(ffi::Py_REFCNT(iterator.as_ptr()), before);

            let invalid = PyList::empty(py);
            let before = ffi::Py_REFCNT(invalid.as_ptr());
            assert!(step_borrowed(invalid.as_ptr()).is_null());
            let pending = ffi::PyErr_GetRaisedException();
            assert!(!pending.is_null());
            assert_eq!(ffi::PyObject_IsInstance(pending, ffi::PyExc_TypeError), 1);
            ffi::Py_DECREF(pending);
            assert_eq!(ffi::Py_REFCNT(invalid.as_ptr()), before);
        });
    }
}
