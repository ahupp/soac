//! Raw native stack-operation semantics for call preparation and owned operands.
//! No Python helper lookup or callable-admission capability is involved.

use super::imports::{ImportSpec, SigType};
use cranelift_jit::JITBuilder;
use pyo3::ffi;
use soac_core::block_py::{Call, CallArgumentOpKind, StorageLayout, call_has_owned_operand_inputs};
use soac_ir_blockpy::InstrBlockPy;
use soac_ir_typed::{InstrTyped, TypedCall};
use std::mem::size_of;
use std::ptr;

unsafe extern "C" {
    fn _PyList_Extend(
        list: *mut ffi::PyListObject,
        iterable: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn _PyList_AsTupleAndClear(list: *mut ffi::PyListObject) -> *mut ffi::PyObject;
    fn _PyDict_MergeEx(dict: *mut ffi::PyObject, update: *mut ffi::PyObject, override_: i32)
    -> i32;
    fn _PyEval_FormatKwargsError(
        state: *mut ffi::PyThreadState,
        callable: *mut ffi::PyObject,
        update: *mut ffi::PyObject,
    );
    fn _Py_Check_ArgsIterable(
        state: *mut ffi::PyThreadState,
        callable: *mut ffi::PyObject,
        args: *mut ffi::PyObject,
    ) -> i32;
}

pub(super) static UPDATE: ImportSpec = ImportSpec::new(
    "dp_jit_call_argument_update",
    &[
        SigType::I32,
        SigType::Pointer,
        SigType::Pointer,
        SigType::Pointer,
    ],
    &[SigType::I32],
);
pub(super) static FINISH_LIST: ImportSpec = ImportSpec::new(
    "dp_jit_call_argument_finish_list",
    &[SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static NORMALIZE_SINGLETON: ImportSpec = ImportSpec::new(
    "dp_jit_call_argument_normalize_singleton",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::Pointer],
);
pub(super) static CHECK_PREPARED: ImportSpec = ImportSpec::new(
    "dp_jit_call_argument_check_prepared",
    &[SigType::Pointer, SigType::Pointer],
    &[SigType::I32],
);

pub(super) static OWNED_OPERANDS: ImportSpec = ImportSpec::new(
    "dp_jit_call_owned_operands",
    &[SigType::Pointer, SigType::Pointer, SigType::I64],
    &[SigType::Pointer],
);

/// Entry/deopt and typed codegen use the same physical input-ownership rule.
/// This predicate never grants an active source interval or callable admission.
pub(super) fn blockpy_owned_operand_call(
    call: &Call<InstrBlockPy>,
    layout: &StorageLayout,
) -> Result<bool, String> {
    call_has_owned_operand_inputs(
        call.func.as_ref(),
        &call.args,
        !call.keywords.is_empty(),
        call.frame_namespace.is_some(),
        layout,
        |input| matches!(input, InstrBlockPy::Call(_)),
    )
}

pub(super) fn typed_owned_operand_call(
    call: &TypedCall<InstrTyped>,
    layout: &StorageLayout,
) -> Result<bool, String> {
    call.has_owned_operand_inputs(layout)
}

unsafe extern "C" {
    fn PySoac_VectorcallWithContext(
        callable: *mut ffi::PyObject,
        args: *const *mut ffi::PyObject,
        nargsf: usize,
        kwnames: *mut ffi::PyObject,
        globals: *mut ffi::PyObject,
        namespace: *mut ffi::PyObject,
        builtins: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
}

unsafe fn release_owned_call_inputs(inputs: &mut [*mut ffi::PyObject]) {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    for slot in inputs.iter_mut().rev() {
        let value = std::mem::replace(slot, ptr::null_mut());
        unsafe { ffi::Py_XDECREF(value) };
    }
    unsafe { ffi::PyErr_SetRaisedException(error) };
}

/// The call borrows the evaluated operands until it returns. Every input still
/// owns its actual reference, and cleanup consumes each physical slot once on
/// either outcome. CPython's private stack-reference schedule is not reproduced.
unsafe fn call_owned_operands_context(
    globals: *mut ffi::PyObject,
    builtins: *mut ffi::PyObject,
    inputs: &mut [*mut ffi::PyObject],
) -> *mut ffi::PyObject {
    let result = unsafe {
        PySoac_VectorcallWithContext(
            inputs[0],
            inputs.as_ptr().add(1),
            inputs.len() - 1,
            ptr::null_mut(),
            globals,
            ptr::null_mut(),
            builtins,
        )
    };
    unsafe { release_owned_call_inputs(inputs) };
    result
}

/// Inputs are [callable, positional arguments], each an owned evaluated
/// expression. Valid-input returns consume/NULL every slot, including errors.
/// The existing contextual native call carries the actual globals/builtins;
/// no opaque token transport or source-entry record is used.
pub(super) unsafe extern "C" fn dp_jit_call_owned_operands(
    environment: *const crate::FunctionEnvAbiHeader,
    inputs: *mut *mut ffi::PyObject,
    count: usize,
) -> *mut ffi::PyObject {
    if inputs.is_null() || count == 0 || count > isize::MAX as usize / size_of::<usize>() {
        if unsafe { ffi::PyErr_Occurred() }.is_null() {
            unsafe { invalid_phase() };
        }
        return ptr::null_mut();
    }
    let inputs = unsafe { std::slice::from_raw_parts_mut(inputs, count) };
    if !unsafe { ffi::PyErr_Occurred() }.is_null() {
        unsafe { release_owned_call_inputs(inputs) };
        return ptr::null_mut();
    }
    let Some(header) = (unsafe { environment.as_ref() }) else {
        unsafe {
            invalid_phase();
            release_owned_call_inputs(inputs);
        }
        return ptr::null_mut();
    };
    if inputs.iter().any(|value| value.is_null()) {
        unsafe {
            invalid_phase();
            release_owned_call_inputs(inputs);
        }
        return ptr::null_mut();
    }
    unsafe { call_owned_operands_context(header.globals_obj, header.builtins_obj, inputs) }
}

pub(super) const fn update_kind(kind: CallArgumentOpKind) -> i32 {
    match kind {
        CallArgumentOpKind::ExtendPositional => 0,
        CallArgumentOpKind::MergeKeywords => 1,
        CallArgumentOpKind::FinishPositionalList | CallArgumentOpKind::NormalizeSingletonStar => -1,
    }
}

unsafe fn release_preserving_error(value: *mut ffi::PyObject) {
    let error = unsafe { ffi::PyErr_GetRaisedException() };
    unsafe {
        ffi::Py_XDECREF(value);
        ffi::PyErr_SetRaisedException(error);
    }
}

unsafe fn invalid_phase() {
    unsafe {
        ffi::PyErr_SetString(
            ffi::PyExc_SystemError,
            c"invalid native call-argument phase".as_ptr(),
        )
    };
}

/// The update is owned and consumed on either result. The callable and buffer
/// are borrowed from their actual compiler Operand primaries throughout every
/// iterator, mapping, error-formatting, and update-finalizer callback.
pub(super) unsafe fn update_owned(
    kind: CallArgumentOpKind,
    callable: *mut ffi::PyObject,
    buffer: *mut ffi::PyObject,
    update: *mut ffi::PyObject,
) -> i32 {
    unsafe { dp_jit_call_argument_update(update_kind(kind), callable, buffer, update) }
}

unsafe extern "C" fn dp_jit_call_argument_update(
    kind: i32,
    callable: *mut ffi::PyObject,
    buffer: *mut ffi::PyObject,
    update: *mut ffi::PyObject,
) -> i32 {
    if callable.is_null() || buffer.is_null() || update.is_null() {
        unsafe {
            invalid_phase();
            release_preserving_error(update);
        }
        return -1;
    }
    let status = match kind {
        0 if unsafe { ffi::Py_TYPE(buffer) } == ptr::addr_of_mut!(ffi::PyList_Type) => {
            let result = unsafe { _PyList_Extend(buffer.cast(), update) };
            if result.is_null() {
                if unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError) } != 0
                    && unsafe {
                        (*ffi::Py_TYPE(update)).tp_iter.is_none()
                            && ffi::PySequence_Check(update) == 0
                    }
                {
                    unsafe {
                        ffi::PyErr_Clear();
                        ffi::PyErr_Format(
                            ffi::PyExc_TypeError,
                            c"Value after * must be an iterable, not %.200s".as_ptr(),
                            (*ffi::Py_TYPE(update)).tp_name,
                        );
                    }
                }
                -1
            } else {
                0
            }
        }
        1 if unsafe { ffi::Py_TYPE(buffer) } == ptr::addr_of_mut!(ffi::PyDict_Type) => {
            let status = unsafe { _PyDict_MergeEx(buffer, update, 2) };
            if status < 0 {
                unsafe { _PyEval_FormatKwargsError(ffi::PyThreadState_Get(), callable, update) };
            }
            status
        }
        _ => {
            unsafe { invalid_phase() };
            -1
        }
    };
    unsafe { release_preserving_error(update) };
    status
}

/// Caller takes/NULLs the list's primary before entry. The native conversion
/// steals its elements, not the list object, so consume the emptied list here
/// even if tuple allocation failed. No caller may release it a second time.
pub(super) unsafe extern "C" fn dp_jit_call_argument_finish_list(
    list: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    if list.is_null() || unsafe { ffi::Py_TYPE(list) } != ptr::addr_of_mut!(ffi::PyList_Type) {
        unsafe {
            invalid_phase();
            release_preserving_error(list);
        }
        return ptr::null_mut();
    }
    let result = unsafe { _PyList_AsTupleAndClear(list.cast()) };
    unsafe { release_preserving_error(list) };
    result
}

/// CALL_FUNCTION_EX's singleton-star path borrows the raw arguments until
/// conversion succeeds. A failed check/conversion must leave that primary
/// untouched; its later unwind comes after the already prepared keyword dict.
pub(super) unsafe extern "C" fn dp_jit_call_argument_normalize_singleton(
    callable: *mut ffi::PyObject,
    raw: *mut ffi::PyObject,
) -> *mut ffi::PyObject {
    if callable.is_null() || raw.is_null() {
        unsafe { invalid_phase() };
        return ptr::null_mut();
    }
    if unsafe { ffi::Py_TYPE(raw) } == ptr::addr_of_mut!(ffi::PyTuple_Type) {
        return unsafe { ffi::Py_NewRef(raw) };
    }
    if unsafe { _Py_Check_ArgsIterable(ffi::PyThreadState_Get(), callable, raw) } < 0 {
        return ptr::null_mut();
    }
    unsafe { ffi::PySequence_Tuple(raw) }
}

/// Shape validation is not inferred from names or observed types. This guards
/// malformed IR without re-running conversion or checking callable-ness early.
pub(super) unsafe extern "C" fn dp_jit_call_argument_check_prepared(
    arguments: *mut ffi::PyObject,
    keywords: *mut ffi::PyObject,
) -> i32 {
    if arguments.is_null()
        || unsafe { ffi::Py_TYPE(arguments) } != ptr::addr_of_mut!(ffi::PyTuple_Type)
        || (!keywords.is_null()
            && unsafe { ffi::Py_TYPE(keywords) } != ptr::addr_of_mut!(ffi::PyDict_Type))
    {
        unsafe { invalid_phase() };
        -1
    } else {
        0
    }
}

pub(super) fn primitive_bindings() -> [(&'static ImportSpec, *const u8); 5] {
    [
        (&OWNED_OPERANDS, dp_jit_call_owned_operands as *const u8),
        (&UPDATE, dp_jit_call_argument_update as *const u8),
        (&FINISH_LIST, dp_jit_call_argument_finish_list as *const u8),
        (
            &NORMALIZE_SINGLETON,
            dp_jit_call_argument_normalize_singleton as *const u8,
        ),
        (
            &CHECK_PREPARED,
            dp_jit_call_argument_check_prepared as *const u8,
        ),
    ]
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
    use pyo3::types::{PyDict, PyModule};
    use soac_core::block_py::CallArgPositional;
    use soac_ir_typed::TypedCallAccessPlan;

    fn outgoing_name(name: &str, slot: u32) -> soac_core::block_py::ResolvedName {
        soac_core::block_py::ResolvedName {
            id: name.into(),
            location: soac_core::block_py::NameLocation::Local(soac_core::block_py::LocalLocation(
                slot,
            )),
        }
    }

    fn outgoing_layout() -> StorageLayout {
        let mut layout = StorageLayout {
            stack_slots: vec![
                "callee".into(),
                "argument".into(),
                "sent".into(),
                "source".into(),
            ],
            ..StorageLayout::default()
        };
        for index in 0..3 {
            layout.mark_expression_temporary(soac_core::block_py::LocalLocation(index));
        }
        layout
    }

    fn outgoing_take(slot: u32) -> InstrBlockPy {
        InstrBlockPy::TakeOperand(soac_core::block_py::TakeOperand::new(outgoing_name(
            "not_storage_authority",
            slot,
        )))
    }

    #[test]
    fn owned_outgoing_selection_requires_distinct_physical_moves_not_local_spellings() {
        use soac_core::block_py::{CallArgKeyword, FrameNamespace, Load};
        let layout = outgoing_layout();
        let factory = InstrBlockPy::Call(Call::new(
            InstrBlockPy::Load(Load::new(outgoing_name("factory", 3))),
            vec![],
            vec![],
        ));
        let call = Call::new(
            outgoing_take(0),
            vec![
                CallArgPositional::Positional(outgoing_take(1)),
                CallArgPositional::Positional(outgoing_take(2)),
                CallArgPositional::Positional(factory),
            ],
            vec![],
        );
        assert_eq!(blockpy_owned_operand_call(&call, &layout), Ok(true));

        let mut other = call.clone();
        other.func = Box::new(InstrBlockPy::Load(Load::new(outgoing_name(
            "_dp_tmp_not_a_move",
            0,
        ))));
        assert_eq!(blockpy_owned_operand_call(&other, &layout), Ok(false));
        other = call.clone();
        other.args[0] = CallArgPositional::Positional(InstrBlockPy::Load(Load::new(
            outgoing_name("_dp_tmp_not_a_move", 1),
        )));
        assert_eq!(blockpy_owned_operand_call(&other, &layout), Ok(false));
        other = call.clone();
        other.args[0] = CallArgPositional::Positional(outgoing_take(0));
        assert!(blockpy_owned_operand_call(&other, &layout).is_err());
        other.args[0] = CallArgPositional::Positional(outgoing_take(3));
        assert!(blockpy_owned_operand_call(&other, &layout).is_err());

        other = call.clone();
        other.args[0] = CallArgPositional::Starred(outgoing_take(1));
        assert_eq!(blockpy_owned_operand_call(&other, &layout), Ok(false));
        other = call.clone();
        other.keywords.push(CallArgKeyword::Named {
            arg: "value".into(),
            value: outgoing_take(1),
        });
        assert_eq!(blockpy_owned_operand_call(&other, &layout), Ok(false));
        other = call;
        other.frame_namespace = Some(FrameNamespace::ModuleGlobals);
        assert_eq!(blockpy_owned_operand_call(&other, &layout), Ok(false));
    }

    #[test]
    fn owned_outgoing_typed_selection_preserves_other_access_plans() {
        use soac_core::block_py::{Load, TakeOperand};
        let take = |slot| InstrTyped::TakeOperand(TakeOperand::new(outgoing_name("alias", slot)));
        let mut call = TypedCall::generic(
            take(0),
            vec![
                CallArgPositional::Positional(take(1)),
                CallArgPositional::Positional(InstrTyped::CallTyped(TypedCall::generic(
                    InstrTyped::Load(Load::new(outgoing_name("factory", 3))),
                    vec![],
                    vec![],
                ))),
            ],
            vec![],
        );
        let layout = outgoing_layout();
        assert_eq!(typed_owned_operand_call(&call, &layout), Ok(true));
        call.args[0] = CallArgPositional::Positional(take(0));
        assert!(typed_owned_operand_call(&call, &layout).is_err());
        call.args[0] = CallArgPositional::Positional(take(1));
        call.access = TypedCallAccessPlan::GuardedCallable {
            function_guards: vec![],
        };
        assert_eq!(typed_owned_operand_call(&call, &layout), Ok(false));
    }

    const SOURCE: &std::ffi::CStr = cr#"
import weakref
events = []
error = MemoryError('argument conversion failed')
raw_ref = None
def target(*args, **kwargs): return args, kwargs
class Iterable:
    def __iter__(self):
        events.append(('iter',))
        yield 1
    def __del__(self): events.append(('drop-iterable',))
class Mapping:
    def keys(self):
        events.append(('keys',))
        return ['value']
    def __getitem__(self, key):
        events.append(('getitem', key))
        return 3
    def __del__(self): events.append(('drop-mapping',))
class BadIterable:
    def __init__(self):
        global raw_ref
        raw_ref = weakref.ref(self)
    def __iter__(self): raise error
    def __del__(self): events.append(('drop-raw',))
class Payload:
    def __del__(self): events.append(('drop-payload', raw_ref() is not None))
def ordinary_extend(): return target(*Iterable(), 2)
def ordinary_merge(): return target(**Mapping())
def ordinary_duplicate(): return target(**{'value': 0}, **Mapping())
def ordinary_singleton_failure(): return target(*BadIterable(), payload=Payload())
class OwnedCallPayload:
    def __del__(self): events.append(('drop-owned-call',))
def owned_success(*arguments):
    events.append(('owned-call', len(arguments)))
    return arguments[0]
def owned_failure(*arguments):
    events.append(('owned-call', len(arguments)))
    raise error
def callback_events():
    return [event for event in events if event[0] in ('iter', 'keys', 'getitem')]
def update_releases():
    return sorted(event[0] for event in events if event[0].startswith('drop-'))
"#;

    fn python(test: impl FnOnce(Python<'_>, Bound<'_, PyModule>)) {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module =
                PyModule::from_code(py, SOURCE, c"call_arguments.py", c"call_arguments").unwrap();
            test(py, module);
        });
    }

    #[test]
    fn owned_outgoing_preflight_failure_consumes_inputs_and_preserves_primary() {
        python(|py, module| unsafe {
            for pending in [false, true] {
                let events = module.getattr("events").unwrap();
                events.call_method0("clear").unwrap();
                let payload = module.getattr("OwnedCallPayload").unwrap().call0().unwrap();
                let witness = Bound::<pyo3::PyAny>::from_owned_ptr(
                    py,
                    ffi::PyWeakref_NewRef(payload.as_ptr(), ptr::null_mut()),
                );
                let primary = module.getattr("error").unwrap();
                let mut inputs = [
                    module.getattr("target").unwrap().into_ptr(),
                    payload.into_ptr(),
                ];
                if pending {
                    PyErr::from_value(primary.clone()).restore(py);
                }
                assert!(
                    dp_jit_call_owned_operands(ptr::null(), inputs.as_mut_ptr(), inputs.len(),)
                        .is_null()
                );
                let error = PyErr::fetch(py);
                if pending {
                    assert!(error.value(py).is(&primary));
                } else {
                    assert!(error.is_instance_of::<pyo3::exceptions::PySystemError>(py));
                }
                assert!(inputs.iter().all(|value| value.is_null()));
                assert!(witness.call0().unwrap().is_none());
                assert_eq!(
                    events.len().unwrap(),
                    1,
                    "one close for the sole payload primary"
                );
            }
        });
    }

    #[test]
    fn owned_outgoing_context_preserves_result_error_and_consumes_duplicate_inputs_once() {
        python(|py, module| unsafe {
            for argument_count in [1, 9] {
                for fails in [false, true] {
                    let events = module.getattr("events").unwrap();
                    events.call_method0("clear").unwrap();
                    let payload = module.getattr("OwnedCallPayload").unwrap().call0().unwrap();
                    let witness = Bound::<pyo3::PyAny>::from_owned_ptr(
                        py,
                        ffi::PyWeakref_NewRef(payload.as_ptr(), ptr::null_mut()),
                    );
                    let function = if fails {
                        "owned_failure"
                    } else {
                        "owned_success"
                    };
                    let callable = module.getattr(function).unwrap();
                    let builtins = callable.getattr("__builtins__").unwrap();
                    let mut inputs = vec![callable.into_ptr()];
                    for _ in 0..argument_count {
                        inputs.push(payload.clone().into_ptr());
                    }
                    drop(payload);
                    let result = call_owned_operands_context(
                        module.dict().as_ptr(),
                        builtins.as_ptr(),
                        &mut inputs,
                    );
                    assert!(inputs.iter().all(|value| value.is_null()));
                    if fails {
                        assert!(result.is_null());
                        let error = PyErr::fetch(py);
                        assert!(error.value(py).is(&module.getattr("error").unwrap()));
                        // A real traceback legitimately owns bound arguments.
                        error.value(py).setattr("__traceback__", py.None()).unwrap();
                    } else {
                        let result = Bound::<pyo3::PyAny>::from_owned_ptr(py, result);
                        assert!(result.is(&witness.call0().unwrap()));
                        assert!(ffi::PyErr_Occurred().is_null());
                        drop(result);
                    }
                    assert!(witness.call0().unwrap().is_none());
                    assert_eq!(
                        events.len().unwrap(),
                        2,
                        "one invocation and one finalization"
                    );
                    assert_eq!(
                        events
                            .get_item(0)
                            .unwrap()
                            .extract::<(String, usize)>()
                            .unwrap(),
                        ("owned-call".to_string(), argument_count),
                    );
                    assert_eq!(
                        events.get_item(1).unwrap().extract::<(String,)>().unwrap(),
                        ("drop-owned-call".to_string(),),
                    );
                }
            }
        });
    }

    #[test]
    fn call_argument_phases_preserve_update_callbacks_and_release_updates() {
        python(|py, module| unsafe {
            let events = module.getattr("events").unwrap();
            let callable = module.getattr("target").unwrap();
            for (kind, ordinary, update_type) in [
                (
                    CallArgumentOpKind::ExtendPositional,
                    "ordinary_extend",
                    "Iterable",
                ),
                (
                    CallArgumentOpKind::MergeKeywords,
                    "ordinary_merge",
                    "Mapping",
                ),
            ] {
                events.call_method0("clear").unwrap();
                drop(module.getattr(ordinary).unwrap().call0().unwrap());
                let expected_callbacks =
                    module.getattr("callback_events").unwrap().call0().unwrap();
                let expected_releases = module.getattr("update_releases").unwrap().call0().unwrap();
                events.call_method0("clear").unwrap();
                let buffer = Bound::from_owned_ptr_or_err(
                    py,
                    if kind == CallArgumentOpKind::ExtendPositional {
                        ffi::PyList_New(0)
                    } else {
                        ffi::PyDict_New()
                    },
                )
                .unwrap();
                let owners = ffi::Py_REFCNT(buffer.as_ptr());
                let update = module
                    .getattr(update_type)
                    .unwrap()
                    .call0()
                    .unwrap()
                    .into_ptr();
                assert_eq!(
                    update_owned(kind, callable.as_ptr(), buffer.as_ptr(), update),
                    0
                );
                assert_eq!(ffi::Py_REFCNT(buffer.as_ptr()), owners);
                assert!(
                    module
                        .getattr("callback_events")
                        .unwrap()
                        .call0()
                        .unwrap()
                        .eq(&expected_callbacks)
                        .unwrap(),
                    "expansion/merge callback order: {kind:?}"
                );
                assert!(
                    module
                        .getattr("update_releases")
                        .unwrap()
                        .call0()
                        .unwrap()
                        .eq(&expected_releases)
                        .unwrap()
                );
            }
        });
    }

    #[test]
    fn call_argument_duplicate_merge_keeps_native_error_and_consumes_update() {
        python(|py, module| unsafe {
            let events = module.getattr("events").unwrap();
            let expected = module
                .getattr("ordinary_duplicate")
                .unwrap()
                .call0()
                .unwrap_err();
            let expected_message = expected.value(py).str().unwrap().to_string();
            let expected_callbacks = module.getattr("callback_events").unwrap().call0().unwrap();
            let expected_releases = module.getattr("update_releases").unwrap().call0().unwrap();
            drop(expected);
            events.call_method0("clear").unwrap();
            let callable = module.getattr("target").unwrap();
            let buffer = PyDict::new(py);
            buffer.set_item("value", 0).unwrap();
            let update = module
                .getattr("Mapping")
                .unwrap()
                .call0()
                .unwrap()
                .into_ptr();
            assert_eq!(
                update_owned(
                    CallArgumentOpKind::MergeKeywords,
                    callable.as_ptr(),
                    buffer.as_ptr(),
                    update
                ),
                -1
            );
            let actual = PyErr::fetch(py);
            assert!(actual.is_instance_of::<pyo3::exceptions::PyTypeError>(py));
            assert_eq!(
                actual.value(py).str().unwrap().to_string(),
                expected_message
            );
            assert!(
                module
                    .getattr("callback_events")
                    .unwrap()
                    .call0()
                    .unwrap()
                    .eq(&expected_callbacks)
                    .unwrap()
            );
            assert!(
                module
                    .getattr("update_releases")
                    .unwrap()
                    .call0()
                    .unwrap()
                    .eq(&expected_releases)
                    .unwrap()
            );
            assert_eq!(
                buffer
                    .get_item("value")
                    .unwrap()
                    .unwrap()
                    .extract::<i32>()
                    .unwrap(),
                0
            );
        });
    }

    #[test]
    fn call_argument_singleton_failure_leaves_raw_owner_for_keyword_first_unwind() {
        python(|py, module| unsafe {
            let events = module.getattr("events").unwrap();
            let error = module.getattr("error").unwrap();
            let ordinary = module
                .getattr("ordinary_singleton_failure")
                .unwrap()
                .call0()
                .unwrap_err();
            let expected_failed = events.call_method0("copy").unwrap();
            error.setattr("__traceback__", py.None()).unwrap();
            drop(ordinary);
            let expected_closed = events.call_method0("copy").unwrap();
            events.call_method0("clear").unwrap();

            let callable = module.getattr("target").unwrap();
            let raw = module
                .getattr("BadIterable")
                .unwrap()
                .call0()
                .unwrap()
                .into_ptr();
            let keywords = PyDict::new(py);
            keywords
                .set_item(
                    "payload",
                    module.getattr("Payload").unwrap().call0().unwrap(),
                )
                .unwrap();
            let result = dp_jit_call_argument_normalize_singleton(callable.as_ptr(), raw);
            assert!(result.is_null());
            let pending = ffi::PyErr_GetRaisedException();
            assert_eq!(pending, error.as_ptr());
            ffi::Py_DECREF(pending);
            error.setattr("__traceback__", py.None()).unwrap();
            assert_eq!(
                ffi::Py_REFCNT(raw),
                1,
                "failed conversion left the original primary intact"
            );
            assert_eq!(events.len().unwrap(), 0);
            drop(keywords);
            assert!(events.eq(&expected_failed).unwrap());
            ffi::Py_DECREF(raw);
            assert!(events.eq(&expected_closed).unwrap());
        });
    }
}
