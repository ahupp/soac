use super::runtime_context::FunctionRuntimeDataLayout;
use super::{
    RuntimeFunctionEntryPlan, RuntimeJitDeoptCursor, RuntimeJitDeoptInvocation,
    RuntimeJitDeoptLocals, specialized_helpers::ObjPtr,
};
use crate::function_instantiation::{
    make_function_in_shared_state, make_function_kind_abi_tag, soac_jit_make_function_with_closure,
};
use crate::handled_exception::{
    HandledExceptionPlan, HandledExceptionRegion, HandledExceptionState, OwnedHandledExceptionState,
};
use crate::module_constants::{ModuleConstantId, load_runtime_name_owned_by_id};
use crate::module_type::SharedModuleState;
use crate::preserved_state;
use crate::session::CompileSession;
use pyo3::ffi;
use pyo3::types::PyAny;
use pyo3::{Bound, PyErr, Python};
use soac_core::block_py::{
    AbruptKind, BinOp, BinOpKind, Block, BlockArg, BlockEdge, BlockLabel, BlockTerm,
    CallArgKeyword, CallArgPositional, CellBindingKind, CellLoadBinding, CellLocation,
    FrameNamespace, LocalLocation, NameLocation, OperandLocation, ParamKind, PreservedLocation,
    RuntimeName, UnaryOp, UnaryOpKind,
};
use soac_core::block_py::{BlockPyFunction, FunctionKind};
use soac_ir_blockpy::{BlockPyModuleShape, InstrBlockPy};
use std::ffi::{c_int, c_void};
use std::ptr;
use std::sync::Arc;

fn block_for_label<'a>(
    function: &'a BlockPyFunction<BlockPyModuleShape>,
    label: BlockLabel,
) -> Option<&'a Block<InstrBlockPy>> {
    if let Some(block) = function.blocks.get(label.index())
        && block.label == label
    {
        return Some(block);
    }

    function.blocks.iter().find(|block| block.label == label)
}

unsafe extern "C" {
    fn PySoac_ObjectCallWithContext(
        callable: *mut ffi::PyObject,
        args: *mut ffi::PyObject,
        kwargs: *mut ffi::PyObject,
        globals: *mut ffi::PyObject,
        namespace: *mut ffi::PyObject,
        builtins: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn PySoac_VectorcallWithContext(
        callable: *mut ffi::PyObject,
        args: *const *mut ffi::PyObject,
        nargsf: usize,
        kwnames: *mut ffi::PyObject,
        globals: *mut ffi::PyObject,
        locals: *mut ffi::PyObject,
        builtins: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject;
    fn _PyStack_UnpackDict(
        tstate: *mut ffi::PyThreadState,
        args: *const *mut ffi::PyObject,
        nargs: ffi::Py_ssize_t,
        kwargs: *mut ffi::PyObject,
        kwnames: *mut *mut ffi::PyObject,
    ) -> *const *mut ffi::PyObject;
    fn _PyStack_UnpackDict_FreeNoDecRef(
        args: *const *mut ffi::PyObject,
        kwnames: *mut ffi::PyObject,
    );
    fn PyCell_New(obj: *mut ffi::PyObject) -> *mut ffi::PyObject;
    fn PyErr_SetRaisedException(exc: *mut ffi::PyObject);
    fn _PyDict_MergeEx(
        mp: *mut ffi::PyObject,
        other: *mut ffi::PyObject,
        override_: c_int,
    ) -> c_int;
    fn _PyEval_FormatKwargsError(
        tstate: *mut ffi::PyThreadState,
        func: *mut ffi::PyObject,
        kwargs: *mut ffi::PyObject,
    );
}

/// The enclosing IR operation selects the invocation boundary. Argument
/// evaluation and cleanup are shared; nested calls never inherit this choice.
#[derive(Clone, Copy)]
enum CallInvocation {
    Ordinary,
    PrepareClassDecorator {
        construction_function: u64,
        environment: *const c_void,
        factory: bool,
    },
}

impl CallInvocation {
    unsafe fn invoke(
        self,
        callable: *mut ffi::PyObject,
        args: *const *mut ffi::PyObject,
        nargs: usize,
        kwnames: *mut ffi::PyObject,
        globals: *mut ffi::PyObject,
        namespace: *mut ffi::PyObject,
        builtins: *mut ffi::PyObject,
    ) -> *mut ffi::PyObject {
        match self {
            Self::Ordinary => unsafe {
                PySoac_VectorcallWithContext(
                    callable, args, nargs, kwnames, globals, namespace, builtins,
                )
            },
            Self::PrepareClassDecorator {
                construction_function,
                environment,
                factory,
            } => unsafe {
                crate::strict_class_decorator::prepare_class_decorator(
                    construction_function,
                    environment,
                    i32::from(factory),
                    callable,
                    args,
                    nargs,
                    kwnames,
                    namespace,
                )
            },
        }
    }
}

/// A completion is not a Python exception until all activation-owned roots
/// have been released. Keeping it explicit also distinguishes a yielded value
/// from the final value of the same suspended callable. Each variant owns its
/// value until it is explicitly transferred to the public return boundary.
enum FrameExecutionOutcome {
    Return(ObjPtr),
    GeneratorReturn(ObjPtr),
}

impl FrameExecutionOutcome {
    fn take_value(&mut self) -> ObjPtr {
        let (Self::Return(value) | Self::GeneratorReturn(value)) = self;
        std::mem::replace(value, ptr::null_mut())
    }

    unsafe fn into_python_result(mut self) -> ObjPtr {
        let value = self.take_value();
        match self {
            Self::Return(_) => value,
            Self::GeneratorReturn(_) => unsafe {
                super::specialized_helpers::dp_jit_generator_return(value)
            },
        }
    }
}

impl Drop for FrameExecutionOutcome {
    fn drop(&mut self) {
        let value = self.take_value();
        if !value.is_null() {
            // Cleanup may run Python finalizers. A later teardown failure must
            // release the successful result once without replacing its error.
            unsafe {
                let error = ffi::PyErr_GetRaisedException();
                ffi::Py_DECREF(value.cast());
                ffi::PyErr_SetRaisedException(error);
            }
        }
    }
}

#[cfg(test)]
mod completion_ownership_tests {
    use super::*;
    use pyo3::types::{PyAnyMethods, PyModule};

    const SOURCE: &std::ffi::CStr = c"\
events = []
class Payload:
    def __del__(self):
        events.append('released')
";

    #[test]
    fn discarded_completion_releases_value_and_preserves_pending_exception() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::from_code(py, SOURCE, c"completion.py", c"completion")
                .expect("completion ownership fixture should compile");
            for (index, generator) in [false, true].into_iter().enumerate() {
                let payload = module
                    .getattr("Payload")
                    .unwrap()
                    .call0()
                    .unwrap()
                    .into_ptr();
                let outcome = if generator {
                    FrameExecutionOutcome::GeneratorReturn(payload.cast())
                } else {
                    FrameExecutionOutcome::Return(payload.cast())
                };
                unsafe {
                    ffi::PyErr_SetString(ffi::PyExc_ValueError, c"pending teardown error".as_ptr());
                    let pending = ffi::PyErr_GetRaisedException();
                    assert!(!pending.is_null());
                    ffi::Py_INCREF(pending);
                    ffi::PyErr_SetRaisedException(pending);
                    drop(outcome);
                    let after = ffi::PyErr_GetRaisedException();
                    assert_eq!(after, pending);
                    ffi::Py_DECREF(after);
                    ffi::Py_DECREF(pending);
                }
                assert_eq!(module.getattr("events").unwrap().len().unwrap(), index + 1);
            }
        });
    }

    #[test]
    fn successful_completion_transfers_its_only_value_owner() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let module = PyModule::from_code(py, SOURCE, c"completion.py", c"completion")
                .expect("completion ownership fixture should compile");
            for (index, generator) in [false, true].into_iter().enumerate() {
                let payload = module
                    .getattr("Payload")
                    .unwrap()
                    .call0()
                    .unwrap()
                    .into_ptr();
                let outcome = if generator {
                    FrameExecutionOutcome::GeneratorReturn(payload.cast())
                } else {
                    FrameExecutionOutcome::Return(payload.cast())
                };
                let returned = unsafe { outcome.into_python_result() };
                if generator {
                    assert!(returned.is_null());
                    let completion = PyErr::fetch(py);
                    assert!(completion.is_instance_of::<pyo3::exceptions::PyStopIteration>(py));
                    assert_eq!(
                        completion.value(py).getattr("value").unwrap().as_ptr(),
                        payload
                    );
                    assert_eq!(module.getattr("events").unwrap().len().unwrap(), index);
                    drop(completion);
                } else {
                    assert_eq!(returned, payload.cast());
                    assert_eq!(module.getattr("events").unwrap().len().unwrap(), index);
                    unsafe { ffi::Py_DECREF(returned.cast()) };
                }
                assert_eq!(module.getattr("events").unwrap().len().unwrap(), index + 1);
            }
        });
    }
}

#[cold]
pub(super) fn execute_deopt_invocation(
    invocation: &RuntimeJitDeoptInvocation<'_>,
) -> Result<ObjPtr, String> {
    let mut frame = BlockPyDeoptFrame::new_deopt(invocation)?;
    let result = frame.execute();
    unsafe { frame.finish_handled_state(&result) };
    let result = unsafe { frame.release_frame_owned_values(result) };
    result.map(|outcome| unsafe { outcome.into_python_result() })
}

#[cold]
#[allow(dead_code)]
pub(crate) unsafe fn run_blockpy_function_from_entry(
    function: &BlockPyFunction<BlockPyModuleShape>,
    context: BlockPyEntryRuntimeContext<'_>,
    positional_args: &[ObjPtr],
) -> Result<ObjPtr, String> {
    unsafe {
        run_blockpy_function_from_vectorcall_entry(
            function,
            context,
            positional_args.as_ptr(),
            positional_args.len(),
            ptr::null_mut(),
        )
    }
}

#[cold]
#[allow(dead_code)]
pub(crate) unsafe fn run_blockpy_function_from_vectorcall_entry(
    function: &BlockPyFunction<BlockPyModuleShape>,
    mut context: BlockPyEntryRuntimeContext<'_>,
    args: *const ObjPtr,
    nargsf: usize,
    kwnames: ObjPtr,
) -> Result<ObjPtr, String> {
    let Some(values) = (unsafe {
        build_entry_local_values(
            context.entry_plan,
            context.function_data_obj,
            context.strict_activation.as_deref().map(|activation| {
                (
                    activation,
                    context
                        .strict_template
                        .as_deref()
                        .expect("strict entry owns its template"),
                )
            }),
            args,
            nargsf,
            kwnames,
        )?
    }) else {
        return Ok(ptr::null_mut());
    };
    if let Some(activation) = context.strict_activation.as_mut() {
        let mut parameters = vec![ptr::null_mut(); context.entry_plan.params().len()];
        for (local, parameter) in context.entry_plan.local_param_indices().iter().enumerate() {
            if let Some(parameter) = parameter {
                parameters[*parameter] = values[local].cast::<ffi::PyObject>();
            }
        }
        let py = unsafe { Python::assume_attached() };
        let template = context
            .strict_template
            .as_ref()
            .expect("strict entry owns its template");
        // The binder may have replaced or cleared the function's metadata.
        // Finish this invocation from the original owning template and call.
        if let Err(error) =
            unsafe { activation.complete_binding(py, template.runtime_data_layout(), &parameters) }
        {
            unsafe { release_entry_local_values(values.as_slice()) };
            error.restore(py);
            return Ok(ptr::null_mut());
        }
    }
    let locals = match RuntimeJitDeoptLocals::from_prevalidated_live_values(
        context.entry_plan.local_bindings(),
        values.as_slice(),
    ) {
        Ok(locals) => locals,
        Err(err) => {
            unsafe { release_entry_local_values(values.as_slice()) };
            return Err(err);
        }
    };
    let mut frame =
        BlockPyDeoptFrame::new_entry(BlockPyEntryFrameSource { function, context }, locals);
    let cursor = RuntimeJitDeoptCursor::at_block_entry(function.entry_block().label);
    let result = unsafe { frame.execute_from_cursor(cursor) };
    unsafe { frame.finish_handled_state(&result) };
    let result = unsafe { frame.release_frame_owned_values(result) };
    result.map(|outcome| unsafe { outcome.into_python_result() })
}

#[allow(dead_code)]
pub(crate) struct BlockPyEntryRuntimeContext<'a> {
    compile_session: Arc<CompileSession>,
    shared_state: Arc<SharedModuleState>,
    globals_obj: ObjPtr,
    builtins_obj: ObjPtr,
    function_data_obj: ObjPtr,
    strict_activation: Option<Box<crate::strict_function::StrictFunctionCall>>,
    strict_template: Option<Arc<crate::FunctionInstantiationTemplate>>,
    entry_plan: &'a RuntimeFunctionEntryPlan,
}

#[allow(dead_code)]
impl<'a> BlockPyEntryRuntimeContext<'a> {
    pub(crate) fn new(
        compile_session: Arc<CompileSession>,
        shared_state: Arc<SharedModuleState>,
        globals_obj: ObjPtr,
        builtins_obj: ObjPtr,
        function_data_obj: ObjPtr,
        entry_plan: &'a RuntimeFunctionEntryPlan,
    ) -> Self {
        Self {
            compile_session,
            shared_state,
            globals_obj,
            builtins_obj,
            function_data_obj,
            strict_activation: None,
            strict_template: None,
            entry_plan,
        }
    }

    pub(crate) fn with_strict_call(
        mut self,
        activation: Box<crate::strict_function::StrictFunctionCall>,
        template: Arc<crate::FunctionInstantiationTemplate>,
    ) -> Self {
        self.globals_obj = activation.environment().globals_obj().cast();
        self.builtins_obj = activation.environment().builtins_obj().cast();
        self.function_data_obj = activation.environment().runtime_objects_ptr().cast();
        self.strict_activation = Some(activation);
        self.strict_template = Some(template);
        self
    }
}

#[allow(dead_code)]
struct BlockPyEntryFrameSource<'a> {
    function: &'a BlockPyFunction<BlockPyModuleShape>,
    context: BlockPyEntryRuntimeContext<'a>,
}

#[allow(dead_code)]
enum BlockPyFrameSource<'inv, 'data> {
    Deopt(&'inv RuntimeJitDeoptInvocation<'data>),
    Entry(BlockPyEntryFrameSource<'inv>),
}

impl<'inv, 'data> BlockPyFrameSource<'inv, 'data> {
    fn strict_activation(&self) -> Option<&crate::strict_function::StrictFunctionCall> {
        match self {
            Self::Deopt(invocation) => unsafe { invocation.strict_activation().as_ref() },
            Self::Entry(entry) => entry.context.strict_activation.as_deref(),
        }
    }

    fn initial_cursor(&self) -> Option<RuntimeJitDeoptCursor> {
        match self {
            Self::Deopt(invocation) => invocation.record().initial_cursor(),
            Self::Entry(entry) => Some(RuntimeJitDeoptCursor::at_block_entry(
                entry.function.entry_block().label,
            )),
        }
    }

    fn function(&self) -> &'inv BlockPyFunction<BlockPyModuleShape> {
        match self {
            Self::Deopt(invocation) => invocation.function(),
            Self::Entry(entry) => entry.function,
        }
    }

    fn globals_obj(&self) -> ObjPtr {
        match self {
            Self::Deopt(invocation) => invocation.globals_obj(),
            Self::Entry(entry) => entry.context.globals_obj,
        }
    }

    fn builtins_obj(&self) -> ObjPtr {
        match self {
            Self::Deopt(invocation) => invocation.builtins_obj(),
            Self::Entry(entry) => entry.context.builtins_obj,
        }
    }

    fn function_data_obj(&self) -> ObjPtr {
        match self {
            Self::Deopt(invocation) => invocation.function_data_obj(),
            Self::Entry(entry) => entry.context.function_data_obj,
        }
    }

    fn module_constant_ptr(&self, constant_index: u32) -> Result<ObjPtr, String> {
        match self {
            Self::Deopt(invocation) => invocation.module_constant_ptr(constant_index),
            Self::Entry(entry) => entry
                .context
                .shared_state
                .module_constant_obj(ModuleConstantId(constant_index as usize))
                .map(|obj| obj.as_ptr().cast())
                .ok_or_else(|| {
                    format!(
                        "entry interpreter for function {} is missing module constant {}",
                        entry.function.function_id, constant_index
                    )
                }),
        }
    }

    unsafe fn runtime_name_owned(&self, runtime_name: RuntimeName) -> ObjPtr {
        match self {
            Self::Deopt(_) => unsafe { load_runtime_name_owned_by_id(runtime_name).cast() },
            Self::Entry(entry) => unsafe {
                entry
                    .context
                    .shared_state
                    .runtime_name_owned_cached(runtime_name)
                    .cast()
            },
        }
    }

    fn static_runtime_name(&self, expression: &InstrBlockPy) -> Option<RuntimeName> {
        let InstrBlockPy::Load(load) = expression else {
            return None;
        };
        match load.name.location {
            NameLocation::RuntimeName(name) => Some(name),
            NameLocation::Constant(index) => match self {
                Self::Deopt(invocation) => invocation.module_constant_runtime_name(index),
                Self::Entry(entry) => entry
                    .context
                    .shared_state
                    .codegen_constants
                    .constant_runtime_name(ModuleConstantId(index as usize)),
            },
            _ => None,
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Deopt(invocation) => invocation.describe(),
            Self::Entry(entry) => format!(
                "function {}, entry interpreter, module {}",
                entry.function.function_id,
                entry.context.shared_state.module_id()
            ),
        }
    }

    fn instantiate_entry_function(
        &self,
        py: Python<'_>,
        function_id: soac_core::block_py::RuntimeFunctionId,
        expected_kind: FunctionKind,
        captures: &Bound<'_, PyAny>,
        param_defaults: &Bound<'_, PyAny>,
        annotate_fn: &Bound<'_, PyAny>,
        module_globals: &Bound<'_, PyAny>,
        class_namespace: Option<&Bound<'_, PyAny>>,
        class_cells: &[Bound<'_, PyAny>],
    ) -> Option<pyo3::PyResult<pyo3::Py<PyAny>>> {
        match self {
            Self::Deopt(_) => None,
            Self::Entry(entry) => Some(make_function_in_shared_state(
                py,
                Arc::clone(&entry.context.compile_session),
                Arc::clone(&entry.context.shared_state),
                function_id,
                expected_kind,
                captures,
                param_defaults,
                annotate_fn,
                module_globals,
                entry
                    .context
                    .strict_activation
                    .as_ref()
                    .and_then(|activation| activation.environment().namespace_execution.as_ref()),
                entry.context.strict_activation.as_deref(),
                class_namespace,
                class_cells,
            )),
        }
    }
}

#[cold]
#[allow(dead_code)]
unsafe fn build_entry_local_values(
    entry_plan: &RuntimeFunctionEntryPlan,
    function_data_obj: ObjPtr,
    strict_call: Option<(
        &crate::strict_function::StrictFunctionCall,
        &crate::FunctionInstantiationTemplate,
    )>,
    args: *const ObjPtr,
    nargsf: usize,
    kwnames: ObjPtr,
) -> Result<Option<Vec<ObjPtr>>, String> {
    if let Some((activation, template)) = strict_call {
        if !ptr::eq(entry_plan, template.entry_plan()) {
            return Err("strict interpreter entry does not own its binding plan".to_string());
        }
        let mut parameters = vec![ptr::null_mut(); entry_plan.params().len()];
        if unsafe {
            crate::bind_function_args_to_output(
                template.binding_plan(),
                activation.environment(),
                Some(activation),
                args.cast(),
                nargsf,
                kwnames.cast(),
                parameters.as_mut_ptr(),
                parameters.len(),
            )
        }
        .is_err()
        {
            return Ok(None);
        }
        return Ok(Some(entry_locals_from_bound_parameters(
            entry_plan,
            parameters.into_iter().map(|value| value.cast()).collect(),
        )));
    }
    let params = entry_plan.params();
    let positional_capacity = entry_plan.positional_capacity();
    let varargs_param = entry_plan.varargs_param();
    let varkw_param = entry_plan.varkw_param();
    let callable_name = entry_plan.callable_name();
    let nargs = unsafe { ffi::PyVectorcall_NARGS(nargsf) as usize };
    let nkw = if kwnames.is_null() {
        0
    } else {
        unsafe { ffi::PyTuple_GET_SIZE(kwnames.cast::<ffi::PyObject>()) as usize }
    };
    if (nargs > 0 || nkw > 0) && args.is_null() {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"missing vectorcall argument array in entry interpreter binding".as_ptr(),
            );
        }
        return Ok(None);
    }

    if varargs_param.is_none() && nargs > positional_capacity {
        unsafe {
            set_entry_type_error(format!(
                "{}() takes {} positional argument{} but {} {} given",
                callable_name,
                positional_capacity,
                if positional_capacity == 1 { "" } else { "s" },
                nargs,
                if nargs == 1 { "was" } else { "were" }
            ));
        }
        return Ok(None);
    }

    let mut param_values = vec![ptr::null_mut(); params.len()];
    let mut assigned = vec![false; params.len()];

    let positional_bound = nargs.min(positional_capacity);
    for position in 0..positional_bound {
        let param_index = entry_plan.positional_param_indices()[position];
        let value = unsafe { *args.add(position) };
        if value.is_null() {
            unsafe {
                release_entry_param_values(param_values.as_mut_slice());
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"null vectorcall positional argument in entry interpreter binding".as_ptr(),
                );
            }
            return Ok(None);
        }
        unsafe {
            ffi::Py_INCREF(value.cast::<ffi::PyObject>());
        }
        param_values[param_index] = value;
        assigned[param_index] = true;
    }

    if let Some(varargs_param) = varargs_param {
        let extras = nargs.saturating_sub(positional_capacity);
        let tuple = unsafe { ffi::PyTuple_New(extras as ffi::Py_ssize_t) };
        if tuple.is_null() {
            unsafe { release_entry_param_values(param_values.as_mut_slice()) };
            return Ok(None);
        }
        for offset in 0..extras {
            let value = unsafe { *args.add(positional_capacity + offset) };
            if value.is_null() {
                unsafe {
                    ffi::Py_DECREF(tuple);
                    release_entry_param_values(param_values.as_mut_slice());
                    ffi::PyErr_SetString(
                        ffi::PyExc_RuntimeError,
                        c"null vectorcall positional vararg in entry interpreter binding".as_ptr(),
                    );
                }
                return Ok(None);
            }
            unsafe {
                ffi::Py_INCREF(value.cast::<ffi::PyObject>());
                if ffi::PyTuple_SetItem(
                    tuple,
                    offset as ffi::Py_ssize_t,
                    value.cast::<ffi::PyObject>(),
                ) != 0
                {
                    ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                    ffi::Py_DECREF(tuple);
                    release_entry_param_values(param_values.as_mut_slice());
                    return Ok(None);
                }
            }
        }
        param_values[varargs_param] = tuple.cast();
        assigned[varargs_param] = true;
    }

    let has_varkw = varkw_param.is_some();
    if let Some(varkw_param) = varkw_param {
        let dict = unsafe { ffi::PyDict_New() };
        if dict.is_null() {
            unsafe { release_entry_param_values(param_values.as_mut_slice()) };
            return Ok(None);
        }
        param_values[varkw_param] = dict.cast();
        assigned[varkw_param] = true;
    }

    for kw_index in 0..nkw {
        let key = unsafe {
            ffi::PyTuple_GetItem(kwnames.cast::<ffi::PyObject>(), kw_index as ffi::Py_ssize_t)
        };
        if key.is_null() {
            unsafe { release_entry_param_values(param_values.as_mut_slice()) };
            return Ok(None);
        }
        let value = unsafe { *args.add(nargs + kw_index) };
        if value.is_null() {
            unsafe {
                release_entry_param_values(param_values.as_mut_slice());
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"null vectorcall keyword argument in entry interpreter binding".as_ptr(),
                );
            }
            return Ok(None);
        }
        let Some(key_name) = (unsafe { entry_keyword_name(key)? }) else {
            unsafe { release_entry_param_values(param_values.as_mut_slice()) };
            return Ok(None);
        };
        if let Some(param_index) = entry_plan.param_index(key_name.as_str()) {
            let param = &params[param_index];
            match param.kind() {
                ParamKind::PosOnly | ParamKind::VarArg => {
                    if !has_varkw {
                        unsafe {
                            release_entry_param_values(param_values.as_mut_slice());
                            set_entry_type_error(format!(
                                "{}() got an unexpected keyword argument '{}'",
                                callable_name, key_name
                            ));
                        }
                        return Ok(None);
                    }
                    if let Some(varkw_param) = varkw_param {
                        let dict = param_values[varkw_param].cast::<ffi::PyObject>();
                        if unsafe { ffi::PyDict_SetItem(dict, key, value.cast::<ffi::PyObject>()) }
                            != 0
                        {
                            unsafe { release_entry_param_values(param_values.as_mut_slice()) };
                            return Ok(None);
                        }
                    }
                }
                ParamKind::Any | ParamKind::KwOnly => {
                    if assigned[param_index] {
                        unsafe {
                            release_entry_param_values(param_values.as_mut_slice());
                            set_entry_type_error(format!(
                                "{}() got multiple values for argument '{}'",
                                callable_name, key_name
                            ));
                        }
                        return Ok(None);
                    }
                    unsafe {
                        ffi::Py_INCREF(value.cast::<ffi::PyObject>());
                    }
                    param_values[param_index] = value;
                    assigned[param_index] = true;
                }
                ParamKind::KwArg => {
                    if let Some(varkw_param) = varkw_param {
                        let dict = param_values[varkw_param].cast::<ffi::PyObject>();
                        if unsafe { ffi::PyDict_SetItem(dict, key, value.cast::<ffi::PyObject>()) }
                            != 0
                        {
                            unsafe { release_entry_param_values(param_values.as_mut_slice()) };
                            return Ok(None);
                        }
                    }
                }
            }
        } else if let Some(varkw_param) = varkw_param {
            let dict = param_values[varkw_param].cast::<ffi::PyObject>();
            if unsafe { ffi::PyDict_SetItem(dict, key, value.cast::<ffi::PyObject>()) } != 0 {
                unsafe { release_entry_param_values(param_values.as_mut_slice()) };
                return Ok(None);
            }
        } else {
            unsafe {
                release_entry_param_values(param_values.as_mut_slice());
                set_entry_type_error(format!(
                    "{}() got an unexpected keyword argument '{}'",
                    callable_name, key_name
                ));
            }
            return Ok(None);
        }
    }

    for (param_index, param) in params.iter().enumerate() {
        if assigned[param_index] {
            continue;
        }
        match param.kind() {
            ParamKind::VarArg | ParamKind::KwArg => {}
            ParamKind::PosOnly | ParamKind::Any | ParamKind::KwOnly => {
                let default = unsafe {
                    load_entry_default_arg_owned(param.default_slot(), function_data_obj)?
                };
                if let Some(default) = default {
                    param_values[param_index] = default;
                    assigned[param_index] = true;
                } else {
                    unsafe {
                        release_entry_param_values(param_values.as_mut_slice());
                        set_entry_type_error(format!(
                            "{}() missing required argument '{}'",
                            callable_name,
                            param.name()
                        ));
                    }
                    return Ok(None);
                }
            }
        }
    }

    Ok(Some(entry_locals_from_bound_parameters(
        entry_plan,
        param_values,
    )))
}

/// Move the binder's owned references into the validated physical local map.
/// Compiled and interpreted strict entry use the same semantic argument binder.
fn entry_locals_from_bound_parameters(
    entry_plan: &RuntimeFunctionEntryPlan,
    mut param_values: Vec<ObjPtr>,
) -> Vec<ObjPtr> {
    let mut values = vec![ptr::null_mut(); entry_plan.local_bindings().len()];
    for (local_index, param_index) in entry_plan.local_param_indices().iter().copied().enumerate() {
        if let Some(param_index) = param_index {
            values[local_index] = param_values[param_index];
            param_values[param_index] = ptr::null_mut();
        } else {
            values[local_index] = ptr::null_mut();
        }
    }
    debug_assert!(param_values.iter().all(|value| value.is_null()));
    values
}

#[cold]
unsafe fn load_entry_default_arg_owned(
    default_slot: Option<usize>,
    function_data_obj: ObjPtr,
) -> Result<Option<ObjPtr>, String> {
    let Some(default_slot) = default_slot else {
        return Ok(None);
    };
    if function_data_obj.is_null() {
        return Ok(None);
    }
    let value = unsafe { *function_data_obj.cast::<ObjPtr>().add(default_slot) };
    if value.is_null() {
        return Ok(None);
    }
    unsafe {
        ffi::Py_INCREF(value.cast::<ffi::PyObject>());
    }
    Ok(Some(value))
}

#[cold]
unsafe fn entry_keyword_name(key: *mut ffi::PyObject) -> Result<Option<String>, String> {
    let mut size = 0;
    let ptr = unsafe { ffi::PyUnicode_AsUTF8AndSize(key, &mut size) };
    if ptr.is_null() {
        return Ok(None);
    }
    let len = usize::try_from(size)
        .map_err(|_| "entry interpreter keyword name had negative UTF-8 size".to_string())?;
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    std::str::from_utf8(bytes)
        .map(|name| Some(name.to_string()))
        .map_err(|_| "entry interpreter keyword name was not valid UTF-8".to_string())
}

#[cold]
unsafe fn set_entry_type_error(message: String) {
    if let Ok(c_message) = std::ffi::CString::new(message) {
        unsafe {
            ffi::PyErr_SetString(ffi::PyExc_TypeError, c_message.as_ptr());
        }
    } else {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_TypeError,
                c"invalid entry interpreter argument binding error".as_ptr(),
            );
        }
    }
}

#[cold]
unsafe fn release_entry_param_values(values: &mut [ObjPtr]) {
    for value in values.iter_mut() {
        if !value.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            *value = ptr::null_mut();
        }
    }
}

#[allow(dead_code)]
unsafe fn release_entry_local_values(values: &[ObjPtr]) {
    for value in values.iter().copied().filter(|value| !value.is_null()) {
        unsafe {
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
    }
}

struct BlockPyDeoptFrame<'inv, 'data> {
    source: BlockPyFrameSource<'inv, 'data>,
    locals: RuntimeJitDeoptLocals<'inv>,
    // The actual resume ABI pins this capsule independently of local liveness.
    // Keep its borrowed identity available after the last control-slot read,
    // including terminal cleanup and a permanent JIT-to-interpreter handoff.
    preserved_state_value: ObjPtr,
    handled_plan: HandledExceptionPlan,
    handled_state: *mut HandledExceptionState,
    handled_owner: Option<Box<OwnedHandledExceptionState>>,
    handled_initialized: bool,
}

impl<'inv, 'data> BlockPyDeoptFrame<'inv, 'data> {
    #[cold]
    fn new_deopt(invocation: &'inv RuntimeJitDeoptInvocation<'data>) -> Result<Self, String> {
        let locals = invocation.materialize_locals()?;
        let mut frame = Self::new_with_locals(BlockPyFrameSource::Deopt(invocation), locals);
        frame.handled_state = invocation.handled_state();
        frame.handled_initialized = true;
        if frame.handled_state.is_null() && frame.handled_plan.len() != 0 {
            return Err(
                "deopt continuation is missing its original handled-state activation".into(),
            );
        }
        Ok(frame)
    }

    #[cold]
    #[allow(dead_code)]
    fn new_entry(
        source: BlockPyEntryFrameSource<'inv>,
        locals: RuntimeJitDeoptLocals<'inv>,
    ) -> Self {
        Self::new_with_locals(BlockPyFrameSource::Entry(source), locals)
    }

    #[cold]
    fn new_with_locals(
        source: BlockPyFrameSource<'inv, 'data>,
        locals: RuntimeJitDeoptLocals<'inv>,
    ) -> Self {
        let preserved_state_value = if source.function().kind != FunctionKind::Function {
            source
                .function()
                .storage_layout()
                .as_ref()
                .and_then(|layout| {
                    layout.generator_resume_parameter(
                        soac_core::block_py::GeneratorResumeParamRole::StateValue,
                    )
                })
                .and_then(|name| locals.get_by_name(name))
                .map_or(ptr::null_mut(), |local| local.value())
        } else {
            ptr::null_mut()
        };
        let handled_plan = match &source {
            BlockPyFrameSource::Deopt(invocation) => invocation.handled_plan().clone(),
            BlockPyFrameSource::Entry(_) => HandledExceptionPlan::for_function(source.function()),
        };
        Self {
            source,
            locals,
            preserved_state_value,
            handled_plan,
            handled_state: ptr::null_mut(),
            handled_owner: None,
            handled_initialized: false,
        }
    }

    unsafe fn activate_handled_state(&mut self) -> Result<bool, String> {
        if self.handled_initialized {
            return Ok(true);
        }
        self.handled_initialized = true;
        if self.source.function().kind != FunctionKind::Function {
            let preserved = self.preserved_state()?;
            let Ok(state) = (unsafe {
                preserved_state::enter_handled_exception_state(preserved.cast(), &self.handled_plan)
            }) else {
                return Ok(false);
            };
            self.handled_state = state;
        } else if self.handled_plan.len() != 0 {
            let Ok(mut owner) = OwnedHandledExceptionState::new(&self.handled_plan, false) else {
                return Ok(false);
            };
            let Ok(state) = (unsafe { OwnedHandledExceptionState::enter(owner.as_mut()) }) else {
                return Ok(false);
            };
            self.handled_state = state;
            self.handled_owner = Some(owner);
        }
        Ok(true)
    }

    fn yielded(&self, result: &Result<FrameExecutionOutcome, String>) -> bool {
        self.source.function().kind != FunctionKind::Function
            && matches!(result, Ok(FrameExecutionOutcome::Return(value)) if !value.is_null())
    }

    unsafe fn finish_handled_state(&mut self, result: &Result<FrameExecutionOutcome, String>) {
        let yielded = self.yielded(result);
        if !yielded {
            unsafe { crate::managed_generator::notify_terminal(self.preserved_state_value.cast()) };
        }
        unsafe { HandledExceptionState::retire_scopes_and_detach(self.handled_state, yielded) };
    }

    unsafe fn transition_handled_state(
        &self,
        block: &Block<InstrBlockPy>,
        incoming: &[(&str, ObjPtr)],
        enter: bool,
    ) -> Result<bool, String> {
        use crate::handled_exception::HandledExceptionTransition;
        let transition = match block.extra.handled_exception {
            soac_core::block_py::HandledExceptionContext::Preserve => return Ok(true),
            soac_core::block_py::HandledExceptionContext::Terminal => {
                unsafe {
                    crate::managed_generator::notify_terminal(self.preserved_state_value.cast())
                };
                unsafe {
                    HandledExceptionState::retire_scopes_and_detach(self.handled_state, false)
                };
                return Ok(true);
            }
            soac_core::block_py::HandledExceptionContext::Unwind => {
                HandledExceptionTransition::Unwind
            }
            soac_core::block_py::HandledExceptionContext::Regions => {
                if enter {
                    HandledExceptionTransition::Enter
                } else {
                    HandledExceptionTransition::Leave
                }
            }
        };
        let regions = block
            .handled_exception_params()
            .map(|param| {
                let value = incoming
                    .iter()
                    .find(|(name, _)| *name == param.name)
                    .map(|(_, value)| *value)
                    .or_else(|| {
                        self.locals
                            .get_by_name(&param.name)
                            .map(|local| local.value())
                    })
                    .ok_or_else(|| {
                        format!(
                            "handled region {} is missing its explicit operand in {}",
                            param.name, block.label,
                        )
                    })?;
                Ok(HandledExceptionRegion {
                    scope: self.handled_plan.scope(&param.name),
                    exception: value.cast(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(unsafe { HandledExceptionState::select(self.handled_state, &regions, transition) } == 0)
    }

    unsafe fn leave_handled_for_target(&self, target: BlockLabel) -> Result<bool, String> {
        let target = block_for_label(self.source.function(), target)
            .ok_or_else(|| format!("missing handled-region target {target}"))?;
        unsafe { self.transition_handled_state(target, &[], false) }
    }

    #[cold]
    unsafe fn execute_runtime_name_deopt(
        &self,
        runtime_name: RuntimeName,
    ) -> Result<ObjPtr, String> {
        Ok(unsafe { self.source.runtime_name_owned(runtime_name) })
    }

    #[cold]
    fn execute(&mut self) -> Result<FrameExecutionOutcome, String> {
        let Some(cursor) = self.source.initial_cursor() else {
            return Err(format!(
                "{}, {}",
                self.source.describe(),
                self.locals.describe()
            ));
        };
        unsafe { self.execute_from_cursor(cursor) }
    }

    #[cold]
    unsafe fn execute_from_cursor(
        &mut self,
        mut cursor: RuntimeJitDeoptCursor,
    ) -> Result<FrameExecutionOutcome, String> {
        if !unsafe { self.activate_handled_state()? } {
            return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
        }
        let function = self.source.function();
        'execute: loop {
            let block_label = cursor.block();
            let start_body_index = cursor.body_index();
            let block = block_for_label(function, block_label).ok_or_else(|| {
                format!(
                    "deopt continuation expected block {block_label} in function {}",
                    function.function_id
                )
            })?;
            let body_tail = block.body.get(start_body_index..).ok_or_else(|| {
                format!(
                    "deopt continuation start body index {start_body_index} is outside block {block_label}"
                )
            })?;
            if !unsafe { self.transition_handled_state(block, &[], true)? } {
                return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
            }
            for instr in body_tail {
                let value = unsafe { self.execute_expr_owned(instr)? };
                if value.is_null() {
                    if let Some(next_cursor) =
                        unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                    {
                        cursor = next_cursor;
                        continue 'execute;
                    }
                    return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
                }
                unsafe {
                    ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                }
            }
            match &block.term {
                BlockTerm::Return(expression) | BlockTerm::GeneratorReturn(expression) => {
                    let value = unsafe { self.execute_expr_owned(expression)? };
                    if value.is_null() {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                    }
                    return Ok(
                        if !value.is_null() && matches!(block.term, BlockTerm::GeneratorReturn(_)) {
                            FrameExecutionOutcome::GeneratorReturn(value)
                        } else {
                            FrameExecutionOutcome::Return(value)
                        },
                    );
                }
                BlockTerm::Jump(edge) => {
                    let Some(next_cursor) = (unsafe {
                        self.execute_jump_edge(edge, OwnedExceptionEdge::new(ptr::null_mut()))?
                    }) else {
                        return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
                    };
                    cursor = next_cursor;
                }
                BlockTerm::IfTerm(if_term) => {
                    let test = unsafe { self.execute_expr_owned(&if_term.test)? };
                    if test.is_null() {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                        return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
                    }
                    let truth = unsafe { ffi::PyObject_IsTrue(test.cast::<ffi::PyObject>()) };
                    unsafe {
                        ffi::Py_DECREF(test.cast::<ffi::PyObject>());
                    }
                    if truth < 0 {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                        return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
                    }
                    let next_block = if truth != 0 {
                        if_term.then_label
                    } else {
                        if_term.else_label
                    };
                    if !unsafe { self.leave_handled_for_target(next_block)? } {
                        return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
                    }
                    cursor = RuntimeJitDeoptCursor::at_block_entry(next_block);
                }
                BlockTerm::BranchTable(branch) => {
                    let index_obj = unsafe { self.execute_expr_owned(&branch.index)? };
                    if index_obj.is_null() {
                        return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
                    }
                    let index =
                        unsafe { ffi::PyLong_AsLongLong(index_obj.cast::<ffi::PyObject>()) };
                    unsafe {
                        ffi::Py_DECREF(index_obj.cast::<ffi::PyObject>());
                    }
                    if index == -1 && !unsafe { ffi::PyErr_Occurred() }.is_null() {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                        return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
                    }
                    let next_block = usize::try_from(index)
                        .ok()
                        .and_then(|index| branch.targets.get(index).copied())
                        .unwrap_or(branch.default_label);
                    if !unsafe { self.leave_handled_for_target(next_block)? } {
                        return Ok(FrameExecutionOutcome::Return(ptr::null_mut()));
                    }
                    cursor = RuntimeJitDeoptCursor::at_block_entry(next_block);
                }
                BlockTerm::Raise(raise) => {
                    let value = unsafe { self.execute_raise_term_owned(raise)? };
                    if value.is_null() {
                        if let Some(next_cursor) =
                            unsafe { self.try_dispatch_exception_edge(block.exc_edge.clone())? }
                        {
                            cursor = next_cursor;
                            continue 'execute;
                        }
                    }
                    return Ok(FrameExecutionOutcome::Return(value));
                }
            }
        }
    }

    #[cold]
    unsafe fn try_dispatch_exception_edge(
        &mut self,
        edge: Option<BlockEdge>,
    ) -> Result<Option<RuntimeJitDeoptCursor>, String> {
        if edge.is_none() || unsafe { ffi::PyErr_Occurred() }.is_null() {
            return Ok(None);
        }
        let current = unsafe { take_current_raised_exception_owned() };
        if current.is_null() {
            return Ok(None);
        }
        let incoming = OwnedExceptionEdge::new(current);
        let edge = edge.expect("edge checked above");
        if let Some(name) = block_for_label(self.source.function(), edge.target).and_then(|block| {
            block
                .handled_exception_params()
                .last()
                .map(|param| param.name.as_str())
        }) {
            unsafe {
                HandledExceptionState::mark_raised(
                    self.handled_state,
                    self.handled_plan.scope(name),
                )
            };
        }
        unsafe { self.execute_jump_edge(&edge, incoming) }
    }

    #[cold]
    unsafe fn execute_jump_edge(
        &mut self,
        edge: &BlockEdge,
        mut incoming_owner: OwnedExceptionEdge,
    ) -> Result<Option<RuntimeJitDeoptCursor>, String> {
        let function = self.source.function();
        let target_block = block_for_label(function, edge.target).ok_or_else(|| {
            format!(
                "deopt continuation expected jump target {} in function {}",
                edge.target, function.function_id
            )
        })?;
        let target_params = &target_block.params;
        if edge.args.len() > target_params.len() {
            return Err(format!(
                "deopt continuation jump to {} has {} explicit args for {} target params",
                edge.target,
                edge.args.len(),
                target_params.len()
            ));
        }
        for param in target_params {
            if self.locals.get_by_name(param.name.as_str()).is_none() {
                return Err(format!(
                    "deopt continuation jump to {} targets param {}, but it was not materialized: {}",
                    edge.target,
                    param.name,
                    self.locals.describe()
                ));
            }
        }

        if incoming_owner
            .values
            .try_reserve_exact(edge.args.len())
            .is_err()
        {
            unsafe { ffi::PyErr_NoMemory() };
            return Ok(None);
        }
        let mut explicit_param_names = Vec::with_capacity(edge.args.len());
        // Omitted edge args mean "keep forwarding the already-materialized local";
        // explicit args overwrite the corresponding target params by position.
        for (param, arg) in target_params.iter().zip(edge.args.iter()) {
            let value = match arg {
                BlockArg::Name(name) => unsafe {
                    self.execute_block_arg_name_owned(name.as_str())?
                },
                BlockArg::None => owned_none(),
                BlockArg::AbruptKind(kind) => unsafe {
                    execute_abrupt_kind_arg_owned(kind.clone())
                },
                BlockArg::CurrentException => unsafe { incoming_owner.current_exception_owned() },
            };
            if value.is_null() {
                return Ok(None);
            }
            explicit_param_names.push(param.name.as_str());
            incoming_owner.values.push(value);
        }

        let incoming = explicit_param_names
            .iter()
            .copied()
            .zip(incoming_owner.values.iter().copied())
            .collect::<Vec<_>>();
        if !unsafe { self.transition_handled_state(target_block, &incoming, false)? } {
            return Ok(None);
        }
        for (param_name, value) in explicit_param_names
            .into_iter()
            .zip(&mut incoming_owner.values)
        {
            let local = self
                .locals
                .get_by_name_mut(param_name)
                .expect("jump target params were prevalidated against materialized locals");
            unsafe {
                local.replace_with_owned_value(std::mem::replace(value, ptr::null_mut()));
            }
        }
        Ok(Some(RuntimeJitDeoptCursor::at_block_entry(edge.target)))
    }

    #[cold]
    unsafe fn execute_block_arg_name_owned(&self, name: &str) -> Result<ObjPtr, String> {
        let Some(local) = self.locals.get_by_name(name) else {
            return Err(format!(
                "deopt continuation jump expected local {name}, but it was not materialized: {}",
                self.locals.describe()
            ));
        };
        let value = local.value();
        if value.is_null() {
            set_deopt_unbound_local_error(name);
            return Ok(ptr::null_mut());
        }
        unsafe {
            ffi::Py_INCREF(value.cast::<ffi::PyObject>());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_expr_owned(&mut self, expr: &InstrBlockPy) -> Result<ObjPtr, String> {
        match expr {
            InstrBlockPy::Load(load) => unsafe {
                self.execute_load_owned(
                    load.name.id.as_str(),
                    load.name.location,
                    load.cell_binding.as_ref(),
                )
            },
            InstrBlockPy::BinOp(binop) => unsafe { self.execute_binop_owned(binop) },
            InstrBlockPy::UnaryOp(unary) => unsafe { self.execute_unary_op_owned(unary) },
            InstrBlockPy::GetAttr(getattr) => unsafe { self.execute_getattr_owned(getattr) },
            InstrBlockPy::GetItem(getitem) => unsafe { self.execute_getitem_owned(getitem) },
            InstrBlockPy::SetAttr(setattr) => unsafe { self.execute_setattr_owned(setattr) },
            InstrBlockPy::SetItem(setitem) => unsafe { self.execute_setitem_owned(setitem) },
            InstrBlockPy::DelItem(delitem) => unsafe { self.execute_delitem_owned(delitem) },
            InstrBlockPy::Tuple(tuple) => unsafe { self.execute_tuple_owned(tuple) },
            InstrBlockPy::Call(call) => unsafe { self.execute_call_owned(call) },
            InstrBlockPy::Store(store) => unsafe { self.execute_store_owned(store) },
            InstrBlockPy::Del(del) => unsafe { self.execute_del_owned(del) },
            InstrBlockPy::TakeOperand(op) => unsafe { self.execute_take_operand(op) },
            InstrBlockPy::IteratorStep(op) => unsafe { self.execute_iterator_step(op) },
            InstrBlockPy::ComprehensionInsert(op) => unsafe {
                self.execute_comprehension_insert(op)
            },
            InstrBlockPy::BuildCollection(op) => unsafe { self.execute_build_collection(op) },
            InstrBlockPy::CallArgumentOp(op) => unsafe { self.execute_call_argument_phase(op) },
            InstrBlockPy::PreparedCall(op) => unsafe { self.execute_prepared_call(op) },
            InstrBlockPy::IncrementCounter(_) => Ok(owned_none()),
            InstrBlockPy::MakeCell(make_cell) => unsafe { self.execute_make_cell_owned(make_cell) },
            InstrBlockPy::NewAnnotationSet(_) => {
                Ok(unsafe { crate::strict_annotation::new_annotation_set() }.cast())
            }
            InstrBlockPy::SetupAnnotations(op) => unsafe {
                if let Some(namespace) = &op.namespace {
                    self.execute_annotation_operand(namespace, |namespace| {
                        crate::strict_annotation::setup_annotations(namespace)
                    })
                } else {
                    Ok(crate::strict_annotation::setup_annotations(
                        self.source.globals_obj().cast(),
                    )
                    .cast())
                }
            },
            InstrBlockPy::CreateTypeAlias(op) => unsafe {
                let globals = self.source.globals_obj().cast::<ffi::PyObject>();
                self.execute_type_expression_operands(op.operands().map(Some), |values| {
                    crate::strict_annotation::create_type_alias(
                        op.evaluator_function.to_packed_runtime_u64(),
                        values[0],
                        values[1],
                        values[2],
                        globals,
                    )
                })
            },
            InstrBlockPy::ConstructTypeParameterScope(op) => unsafe {
                let globals = self.source.globals_obj().cast::<ffi::PyObject>();
                self.execute_type_expression_operands(
                    [
                        op.positional_defaults.as_deref(),
                        op.keyword_defaults.as_deref(),
                        Some(op.scope_function.as_ref()),
                    ],
                    |values| {
                        crate::strict_annotation::construct_type_parameter_scope(
                            op.scope_function_id.to_packed_runtime_u64(),
                            values[0],
                            values[1],
                            values[2],
                            globals,
                        )
                    },
                )
            },
            InstrBlockPy::SubscriptGeneric(op) => unsafe {
                self.execute_annotation_operand(&op.type_parameters, |parameters| {
                    crate::strict_annotation::subscript_generic(parameters)
                })
            },
            InstrBlockPy::SetFunctionTypeParameters(op) => unsafe {
                let globals = self.source.globals_obj().cast::<ffi::PyObject>();
                self.execute_type_expression_operands(op.operands().map(Some), |values| {
                    crate::strict_annotation::set_function_type_parameters(
                        op.function_id.to_packed_runtime_u64(),
                        values[0],
                        values[1],
                        globals,
                    )
                })
            },
            InstrBlockPy::CreateTypeParameter(op) => unsafe {
                let globals = self.source.globals_obj().cast::<ffi::PyObject>();
                self.execute_type_expression_operands(
                    [Some(op.name.as_ref()), op.evaluator.as_deref()],
                    |values| {
                        crate::strict_annotation::create_type_parameter(
                            op.evaluator_function
                                .map_or(0, |id| id.to_packed_runtime_u64()),
                            crate::strict_annotation::type_parameter_kind_tag(op.kind),
                            values[0],
                            values[1],
                            globals,
                        )
                    },
                )
            },
            InstrBlockPy::SetTypeParameterDefault(op) => unsafe {
                let globals = self.source.globals_obj().cast::<ffi::PyObject>();
                self.execute_type_expression_operands(op.operands().map(Some), |values| {
                    crate::strict_annotation::set_type_parameter_default(
                        op.evaluator_function.to_packed_runtime_u64(),
                        values[0],
                        values[1],
                        globals,
                    )
                })
            },
            InstrBlockPy::CheckAnnotationFormat(op) => unsafe {
                self.execute_annotation_operand(&op.format, |format| {
                    crate::strict_annotation::check_annotation_format(format)
                })
            },
            InstrBlockPy::RecordAnnotation(op) => unsafe {
                self.execute_annotation_operand(&op.indices, |indices| {
                    crate::strict_annotation::record_annotation(indices, op.index)
                })
            },
            InstrBlockPy::MakeFunctionWithClosure(make_function) => unsafe {
                self.execute_make_function_with_closure_owned(make_function)
            },
            InstrBlockPy::ConstructClass(construction) => unsafe {
                self.execute_construct_class_owned(construction)
            },
            InstrBlockPy::PrepareClassDecorator(op) => unsafe {
                self.execute_prepare_class_decorator_owned(op)
            },
            InstrBlockPy::ApplyClassDecorator(op) => unsafe {
                let environment = crate::FunctionEnv::environment_from_runtime_objects(
                    self.source.function_data_obj().cast(),
                );
                let module_namespace = self.frame_module_globals(op.frame_namespace.as_ref())?;
                self.execute_type_expression_operands(
                    [
                        Some(op.preparation.as_ref()),
                        Some(op.class.as_ref()),
                        op.frame_namespace
                            .as_ref()
                            .and_then(FrameNamespace::mapping),
                    ],
                    |values| {
                        crate::strict_class_decorator::apply_class_decorator(
                            op.construction_function.to_packed_runtime_u64(),
                            environment,
                            values[0],
                            values[1],
                            module_namespace.map_or(values[2], |namespace| namespace.cast()),
                        )
                    },
                )
            },
            InstrBlockPy::DiscardClassDecorator(op) => unsafe {
                self.execute_annotation_operand(&op.preparation, |preparation| {
                    crate::strict_class_decorator::discard_class_decorator(preparation)
                })
            },
            InstrBlockPy::DiscardClassConstructionCaptures(op) => unsafe {
                self.execute_annotation_operand(&op.function, |preparation| {
                    crate::strict_function::discard_class_construction_captures(preparation)
                })
            },
            InstrBlockPy::ApplyFunctionDescriptor(op) => unsafe {
                let environment = crate::FunctionEnv::environment_from_runtime_objects(
                    self.source.function_data_obj().cast(),
                );
                let module_namespace = self.frame_module_globals(op.frame_namespace.as_ref())?;
                self.execute_type_expression_operands(
                    [
                        Some(op.decorator.as_ref()),
                        Some(op.function.as_ref()),
                        op.frame_namespace
                            .as_ref()
                            .and_then(FrameNamespace::mapping),
                    ],
                    |values| {
                        crate::strict_descriptor::apply_function_descriptor(
                            op.function_id.to_packed_runtime_u64(),
                            environment,
                            values[0],
                            values[1],
                            module_namespace.map_or(values[2], |namespace| namespace.cast()),
                        )
                    },
                )
            },
            InstrBlockPy::CompleteFunctionDefinition(op) => unsafe {
                let globals = self.source.globals_obj().cast::<ffi::PyObject>();
                self.execute_annotation_operand(&op.function, |function| {
                    crate::strict_function::soac_jit_complete_function_definition(
                        op.function_id.to_packed_runtime_u64(),
                        function,
                        globals,
                    )
                })
            },
            InstrBlockPy::CellRef(cell_ref) => unsafe { self.execute_cell_ref_owned(cell_ref) },
        }
    }

    #[cold]
    unsafe fn execute_type_expression_operands<'a>(
        &mut self,
        operands: impl IntoIterator<Item = Option<&'a InstrBlockPy>>,
        operation: impl FnOnce(&[*mut ffi::PyObject]) -> *mut ffi::PyObject,
    ) -> Result<ObjPtr, String> {
        let py = unsafe { Python::assume_attached() };
        let mut owned = Vec::new();
        let mut values = Vec::new();
        for operand in operands {
            let Some(operand) = operand else {
                values.push(ptr::null_mut());
                continue;
            };
            let value = match unsafe { self.execute_expr_owned(operand) } {
                Ok(value) => value,
                Err(error) => {
                    while let Some(value) = owned.pop() {
                        drop(value);
                    }
                    return Err(error);
                }
            };
            if value.is_null() {
                let error = PyErr::fetch(py);
                while let Some(value) = owned.pop() {
                    drop(value);
                }
                error.restore(py);
                return Ok(ptr::null_mut());
            }
            let value = unsafe { Bound::<PyAny>::from_owned_ptr(py, value.cast()) };
            values.push(value.as_ptr());
            owned.push(value);
        }
        let result = operation(&values);
        let error = result.is_null().then(|| PyErr::fetch(py));
        while let Some(value) = owned.pop() {
            drop(value);
        }
        if let Some(error) = error {
            error.restore(py);
        }
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_annotation_operand(
        &mut self,
        operand: &InstrBlockPy,
        operation: impl FnOnce(*mut ffi::PyObject) -> *mut ffi::PyObject,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(operand)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let py = unsafe { Python::assume_attached() };
        let value = unsafe { Bound::<PyAny>::from_owned_ptr(py, value.cast()) };
        let result = operation(value.as_ptr());
        let error = result.is_null().then(|| PyErr::fetch(py));
        drop(value);
        if let Some(error) = error {
            error.restore(py);
        }
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_prepare_class_decorator_owned(
        &mut self,
        preparation: &soac_core::block_py::PrepareClassDecorator<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        if !preparation.factory
            && (!preparation.args.is_empty() || !preparation.keywords.is_empty())
        {
            return Err("bare class decorator preparation has call arguments".into());
        }
        let invocation = CallInvocation::PrepareClassDecorator {
            construction_function: preparation.construction_function.to_packed_runtime_u64(),
            environment: unsafe {
                crate::FunctionEnv::environment_from_runtime_objects(
                    self.source.function_data_obj().cast(),
                )
            },
            factory: preparation.factory,
        };
        unsafe {
            self.execute_call_parts_owned_with_invocation(
                &preparation.decorator,
                &preparation.args,
                &preparation.keywords,
                preparation.frame_namespace.as_ref(),
                invocation,
            )
        }
    }

    #[cold]
    unsafe fn execute_construct_class_owned(
        &mut self,
        construction: &soac_core::block_py::ConstructClass<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        // Owning Bound values keep every evaluated operand visible to Python
        // while later operands and native class callbacks execute. An error
        // releases the already evaluated prefix, never replays construction.
        let py = unsafe { Python::assume_attached() };
        let mut values = Vec::with_capacity(8);
        for operand in construction.operands() {
            let value = match unsafe { self.execute_expr_owned(operand) } {
                Ok(value) => value,
                Err(error) => {
                    while let Some(value) = values.pop() {
                        drop(value);
                    }
                    return Err(error);
                }
            };
            if value.is_null() {
                let error = PyErr::fetch(py);
                while let Some(value) = values.pop() {
                    drop(value);
                }
                error.restore(py);
                return Ok(ptr::null_mut());
            }
            values
                .push(unsafe { Bound::<PyAny>::from_owned_ptr(py, value.cast::<ffi::PyObject>()) });
        }
        let globals = self.source.globals_obj();
        if globals.is_null() {
            while let Some(value) = values.pop() {
                drop(value);
            }
            return Err("class construction has no actual module globals".into());
        }
        let result = unsafe {
            crate::strict_class::soac_jit_construct_class(
                construction.construction_function.to_packed_runtime_u64(),
                crate::FunctionEnv::environment_from_runtime_objects(
                    self.source.function_data_obj().cast(),
                ),
                values[0].as_ptr(),
                values[1].as_ptr(),
                values[2].as_ptr(),
                values[3].as_ptr(),
                values[4].as_ptr(),
                values[5].as_ptr(),
                values[6].as_ptr(),
                values
                    .get(7)
                    .map_or(ptr::null_mut(), |value| value.as_ptr()),
                globals.cast::<ffi::PyObject>(),
            )
        };
        let error = result.is_null().then(|| PyErr::fetch(py));
        while let Some(value) = values.pop() {
            drop(value);
        }
        if let Some(error) = error {
            error.restore(py);
        }
        Ok(result.cast())
    }

    #[cold]
    fn borrow_operand(&self, location: OperandLocation) -> Result<ObjPtr, String> {
        let value = match location {
            OperandLocation::Local(location) => {
                let local = self
                    .locals
                    .get_by_location(location)
                    .ok_or("expression operand was not materialized for deopt")?;
                if !local.has_transferable_nullable_owner() {
                    return Err("expression operand has no owning deopt edge".into());
                }
                local.value()
            }
            OperandLocation::Preserved(location) => {
                let state = self.preserved_state()?;
                let address =
                    unsafe { preserved_state::operand_owner_slot(state.cast(), location) }
                        .map_err(|()| "expression operand lost its preserved deopt owner role")?;
                unsafe { (*address).cast() }
            }
        };
        if value.is_null() {
            let layout = self
                .source
                .function()
                .storage_layout
                .as_ref()
                .ok_or("expression operand has no deopt storage layout")?;
            let name = match location {
                OperandLocation::Local(location) => layout
                    .stack_slots()
                    .get(location.slot() as usize)
                    .map(String::as_str),
                OperandLocation::Preserved(location) => layout
                    .preserved_slot(location.slot())
                    .map(|slot| slot.storage_name.as_str()),
            }
            .ok_or("expression operand has no physical name")?;
            set_deopt_unbound_local_error(name);
        }
        Ok(value)
    }

    #[cold]
    unsafe fn publish_operand_owned(
        &mut self,
        location: OperandLocation,
        new_owned: ObjPtr,
    ) -> Result<ObjPtr, String> {
        match location {
            OperandLocation::Local(location) => {
                let local = self
                    .locals
                    .get_by_location_mut(location)
                    .ok_or("expression operand was not materialized for deopt")?;
                if !local.has_transferable_nullable_owner() {
                    return Err("expression operand has no owning deopt edge".into());
                }
                Ok(local.replace_nullable_owner_without_release(new_owned))
            }
            OperandLocation::Preserved(location) => {
                let state = self.preserved_state()?;
                let address =
                    unsafe { preserved_state::operand_owner_slot(state.cast(), location) }
                        .map_err(|()| "expression operand lost its preserved deopt owner role")?;
                Ok(unsafe { ptr::replace(address, new_owned.cast()).cast() })
            }
        }
    }

    #[cold]
    unsafe fn execute_iterator_step(
        &mut self,
        op: &soac_core::block_py::IteratorStep<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let layout = self
            .source
            .function()
            .storage_layout
            .as_ref()
            .ok_or("iterator step has no deopt storage layout")?;
        let location = op.validate_resolved(layout)?;
        let iterator = self.borrow_operand(location)?;
        if iterator.is_null() {
            return Ok(iterator);
        }
        Ok(unsafe { super::iteration_runtime::step_borrowed(iterator.cast()) }.cast())
    }

    #[cold]
    unsafe fn execute_take_operand(
        &mut self,
        op: &soac_core::block_py::TakeOperand<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let layout = self
            .source
            .function()
            .storage_layout
            .as_ref()
            .ok_or("operand take has no deopt storage layout")?;
        let location = op.validate_resolved(layout)?;
        let value = self.borrow_operand(location)?;
        if value.is_null() {
            return Ok(value);
        }
        unsafe { self.publish_operand_owned(location, ptr::null_mut()) }
    }

    #[cold]
    unsafe fn execute_build_collection(
        &mut self,
        op: &soac_core::block_py::BuildCollection<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        op.validate_shape()?;
        let mut values: Vec<*mut ffi::PyObject> = Vec::with_capacity(op.values.len());
        for input in &op.values {
            match unsafe { self.execute_expr_owned(input) } {
                Ok(value) if !value.is_null() => values.push(value.cast()),
                result => {
                    for value in values.into_iter().rev() {
                        unsafe { release_raw_class_owner(value.cast()) };
                    }
                    return result;
                }
            }
        }
        Ok(unsafe { super::collection_runtime::build_owned(op.kind, &mut values) }.cast())
    }

    #[cold]
    unsafe fn execute_call_argument_phase(
        &mut self,
        op: &soac_core::block_py::CallArgumentOp<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        use soac_core::block_py::CallArgumentOpKind;
        let layout = self
            .source
            .function()
            .storage_layout
            .as_ref()
            .ok_or("call-argument phase has no deopt layout")?;
        let (callable_location, buffer_location) = op.validate_resolved(layout)?;
        let callable = self.borrow_operand(callable_location)?;
        if callable.is_null() {
            return Ok(ptr::null_mut());
        }
        let buffer = self.borrow_operand(buffer_location)?;
        if buffer.is_null() {
            return Ok(ptr::null_mut());
        }
        match op.kind {
            CallArgumentOpKind::ExtendPositional | CallArgumentOpKind::MergeKeywords => {
                let value = unsafe {
                    self.execute_expr_owned(op.value.as_deref().expect("validated update"))?
                };
                if value.is_null() {
                    return Ok(ptr::null_mut());
                }
                let status = unsafe {
                    super::call_arguments_runtime::update_owned(
                        op.kind,
                        callable.cast(),
                        buffer.cast(),
                        value.cast(),
                    )
                };
                if status < 0 {
                    return Ok(ptr::null_mut());
                }
            }
            CallArgumentOpKind::FinishPositionalList
            | CallArgumentOpKind::NormalizeSingletonStar => {
                let value = if op.kind == CallArgumentOpKind::FinishPositionalList {
                    let owned =
                        unsafe { self.publish_operand_owned(buffer_location, ptr::null_mut())? };
                    unsafe {
                        super::call_arguments_runtime::dp_jit_call_argument_finish_list(
                            owned.cast(),
                        )
                    }
                } else {
                    unsafe {
                        super::call_arguments_runtime::dp_jit_call_argument_normalize_singleton(
                            callable.cast(),
                            buffer.cast(),
                        )
                    }
                };
                if value.is_null() {
                    return Ok(ptr::null_mut());
                }
                let old = match unsafe { self.publish_operand_owned(buffer_location, value.cast()) }
                {
                    Ok(old) => old,
                    Err(error) => {
                        unsafe { release_raw_class_owner(value.cast()) };
                        return Err(error);
                    }
                };
                // Every slot publication precedes any callback from the old
                // raw value. Failed singleton conversion never reached here.
                unsafe { release_raw_class_owner(old) };
            }
        }
        Ok(owned_none())
    }

    /// The explicit frame namespace is a borrowed source coordinate, never an
    /// additional owned call operand. Keep the same exact-Load shape as JIT.
    #[cold]
    fn borrowed_prepared_namespace(
        &self,
        namespace: Option<&FrameNamespace<InstrBlockPy>>,
    ) -> Result<ObjPtr, String> {
        match namespace {
            None => Ok(ptr::null_mut()),
            Some(FrameNamespace::ModuleGlobals) => self
                .frame_module_globals(namespace)
                .map(|value| value.expect("module namespace")),
            Some(FrameNamespace::Mapping(mapping)) => {
                let InstrBlockPy::Load(load) = mapping.as_ref() else {
                    return Err("prepared namespace must be an exact resolved mapping Load".into());
                };
                let location = load
                    .name
                    .local_location()
                    .ok_or("prepared namespace must use its represented active local owner")?;
                let value = self
                    .locals
                    .get_by_location(location)
                    .ok_or("prepared namespace owner was not materialized")?
                    .value();
                if value.is_null() {
                    set_deopt_unbound_local_error(load.name.id.as_str());
                }
                Ok(value)
            }
        }
    }

    #[cold]
    unsafe fn execute_prepared_call(
        &mut self,
        op: &soac_core::block_py::PreparedCall<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let layout = self
            .source
            .function()
            .storage_layout
            .as_ref()
            .ok_or("prepared call has no deopt layout")?;
        op.validate_resolved(layout)?;
        let namespace = self.borrowed_prepared_namespace(op.frame_namespace.as_ref())?;
        if op.frame_namespace.is_some() && namespace.is_null() {
            return Ok(ptr::null_mut());
        }
        let mut inputs: Vec<ObjPtr> = Vec::with_capacity(3);
        for input in std::iter::once(op.func.as_ref())
            .chain(std::iter::once(op.arguments.as_ref()))
            .chain(op.keywords.as_deref())
        {
            match unsafe { self.execute_expr_owned(input) } {
                Ok(value) if !value.is_null() => inputs.push(value),
                result => {
                    for value in inputs.into_iter().rev() {
                        unsafe { release_raw_class_owner(value) };
                    }
                    return result;
                }
            }
        }
        let keywords = inputs.get(2).copied().unwrap_or(ptr::null_mut());
        let result = if unsafe {
            super::call_arguments_runtime::dp_jit_call_argument_check_prepared(
                inputs[1].cast(),
                keywords.cast(),
            )
        } < 0
        {
            ptr::null_mut()
        } else {
            unsafe {
                PySoac_ObjectCallWithContext(
                    inputs[0].cast(),
                    inputs[1].cast(),
                    keywords.cast(),
                    self.source.globals_obj().cast(),
                    namespace.cast(),
                    self.source.builtins_obj().cast(),
                )
            }
        };
        // Native CALL_FUNCTION_EX closes kwargs, tuple, then callable. Neither
        // a successful result nor the pending source error may be lost here.
        for value in inputs.into_iter().rev() {
            unsafe { release_raw_class_owner(value) };
        }
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_comprehension_insert(
        &mut self,
        op: &soac_core::block_py::ComprehensionInsert<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let layout = self
            .source
            .function()
            .storage_layout
            .as_ref()
            .ok_or("comprehension insertion has no deopt storage layout")?;
        let location = op.validate_resolved(layout)?;
        let container = self.borrow_operand(location)?;
        if container.is_null() {
            return Ok(ptr::null_mut());
        }
        // This checked borrow stays in its live Operand root. The shared
        // validator rejects a nested take of that root before any evaluation.
        let key = if let Some(key) = &op.key {
            let key = unsafe { self.execute_expr_owned(key)? };
            if key.is_null() {
                return Ok(ptr::null_mut());
            }
            key
        } else {
            ptr::null_mut()
        };
        let value = match unsafe { self.execute_expr_owned(&op.value) } {
            Ok(value) if !value.is_null() => value,
            result => {
                unsafe { release_raw_class_owner(key) };
                return result;
            }
        };
        // The exact native helper consumes both inputs, including hash,
        // equality, allocation, and explicit wrong-container rejection paths.
        let status = unsafe {
            super::collection_runtime::insert_owned(
                op.kind,
                container.cast(),
                key.cast(),
                value.cast(),
            )
        };
        Ok(if status < 0 {
            ptr::null_mut()
        } else {
            owned_none()
        })
    }

    #[cold]
    unsafe fn execute_make_cell_owned(
        &mut self,
        make_cell: &soac_core::block_py::MakeCell<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let Some(initial_value_expr) = make_cell.initial_value.as_ref() else {
            let cell = unsafe { PyCell_New(ptr::null_mut()) };
            return Ok(cell.cast());
        };
        let initial_value = unsafe { self.execute_expr_owned(initial_value_expr)? };
        if initial_value.is_null() {
            return Ok(ptr::null_mut());
        }
        let cell = unsafe { PyCell_New(initial_value.cast::<ffi::PyObject>()) };
        unsafe {
            ffi::Py_DECREF(initial_value.cast::<ffi::PyObject>());
        }
        Ok(cell.cast())
    }

    #[cold]
    unsafe fn execute_make_function_with_closure_owned(
        &mut self,
        make_function: &soac_core::block_py::MakeFunctionWithClosure<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let py = unsafe { Python::assume_attached() };
        let mut owned = Vec::new();
        let mut values = Vec::new();
        for operand in [
            Some(make_function.captures.as_ref()),
            Some(make_function.param_defaults.as_ref()),
            Some(make_function.annotate_fn.as_ref()),
            make_function.class_namespace.as_deref(),
        ]
        .into_iter()
        .chain(make_function.creation_cells.iter().map(Some))
        {
            let Some(operand) = operand else {
                values.push(ptr::null_mut());
                continue;
            };
            let value = match unsafe { self.execute_expr_owned(operand) } {
                Ok(value) => value,
                Err(error) => {
                    while let Some(value) = owned.pop() {
                        drop(value);
                    }
                    return Err(error);
                }
            };
            if value.is_null() {
                let error = PyErr::fetch(py);
                while let Some(value) = owned.pop() {
                    drop(value);
                }
                error.restore(py);
                return Ok(ptr::null_mut());
            }
            let value = unsafe { Bound::<PyAny>::from_owned_ptr(py, value.cast()) };
            values.push(value.as_ptr());
            owned.push(value);
        }
        let globals = self.source.globals_obj();
        if globals.is_null() {
            while let Some(value) = owned.pop() {
                drop(value);
            }
            return Err("deopt continuation expected module globals for make_function".to_string());
        }
        let result = if let Some(result) = unsafe {
            self.execute_entry_make_function_with_closure(
                make_function,
                values[0].cast(),
                values[1].cast(),
                values[2].cast(),
                globals,
                values[3],
                &values[4..],
            )
        } {
            result
        } else {
            unsafe {
                soac_jit_make_function_with_closure(
                    make_function.function_id().to_packed_runtime_u64(),
                    make_function_kind_abi_tag(make_function.kind),
                    values[0],
                    values[1],
                    values[2],
                    globals.cast::<ffi::PyObject>(),
                    crate::FunctionEnv::environment_from_runtime_objects(
                        self.source.function_data_obj(),
                    ),
                    values[3],
                    values[4..].as_ptr(),
                    values.len() - 4,
                )
                .cast()
            }
        };
        let error = result.is_null().then(|| PyErr::fetch(py));
        while let Some(value) = owned.pop() {
            drop(value);
        }
        if let Some(error) = error {
            error.restore(py);
        }
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_entry_make_function_with_closure(
        &self,
        make_function: &soac_core::block_py::MakeFunctionWithClosure<InstrBlockPy>,
        captures: ObjPtr,
        param_defaults: ObjPtr,
        annotate_fn: ObjPtr,
        globals: ObjPtr,
        class_namespace: *mut ffi::PyObject,
        class_cells: &[*mut ffi::PyObject],
    ) -> Option<ObjPtr> {
        let py = Python::assume_attached();
        let captures = unsafe { Bound::from_borrowed_ptr(py, captures.cast::<ffi::PyObject>()) };
        let param_defaults =
            unsafe { Bound::from_borrowed_ptr(py, param_defaults.cast::<ffi::PyObject>()) };
        let annotate_fn =
            unsafe { Bound::from_borrowed_ptr(py, annotate_fn.cast::<ffi::PyObject>()) };
        let globals = unsafe { Bound::from_borrowed_ptr(py, globals.cast::<ffi::PyObject>()) };
        let class_namespace = (!class_namespace.is_null())
            .then(|| unsafe { Bound::from_borrowed_ptr(py, class_namespace) });
        let class_cells = class_cells
            .iter()
            .map(|cell| unsafe { Bound::from_borrowed_ptr(py, *cell) })
            .collect::<Vec<_>>();
        match self.source.instantiate_entry_function(
            py,
            make_function.function_id(),
            make_function.kind,
            &captures,
            &param_defaults,
            &annotate_fn,
            &globals,
            class_namespace.as_ref(),
            &class_cells,
        ) {
            Some(Ok(func)) => Some(func.into_ptr().cast()),
            Some(Err(err)) => {
                err.restore(py);
                Some(ptr::null_mut())
            }
            None => None,
        }
    }

    #[cold]
    unsafe fn execute_cell_ref_owned(
        &self,
        cell_ref: &soac_core::block_py::CellRef,
    ) -> Result<ObjPtr, String> {
        unsafe { self.execute_raw_cell_object_for_location_owned(cell_ref.location, "cell_ref") }
    }

    #[cold]
    unsafe fn execute_raw_cell_object_for_location_owned(
        &self,
        location: CellLocation,
        debug_name: &str,
    ) -> Result<ObjPtr, String> {
        match location {
            CellLocation::Owned(slot) => unsafe {
                self.execute_owned_raw_cell_object_for_slot_owned(slot, debug_name)
            },
            CellLocation::Preserved(slot) => unsafe {
                self.execute_preserved_raw_cell_object_for_slot_owned(slot, debug_name)
            },
            CellLocation::Closure(slot) | CellLocation::CapturedSource(slot) => unsafe {
                self.execute_environment_raw_cell_object_for_slot_owned(slot, false, debug_name)
            },
            CellLocation::Private(slot) => unsafe {
                self.execute_environment_raw_cell_object_for_slot_owned(slot, true, debug_name)
            },
        }
    }

    #[cold]
    unsafe fn execute_owned_raw_cell_object_for_slot_owned(
        &self,
        slot: u32,
        debug_name: &str,
    ) -> Result<ObjPtr, String> {
        let function = self.source.function();
        let layout = function.storage_layout.as_ref().ok_or_else(|| {
            format!(
                "deopt continuation expected storage layout for owned {debug_name} slot {slot} in function {}",
                function.function_id
            )
        })?;
        let closure_slot = layout.owned_slot(slot).ok_or_else(|| {
            format!(
                "deopt continuation expected owned {debug_name} slot {slot} in function {} storage layout",
                function.function_id
            )
        })?;
        let location = layout
            .stack_slots()
            .iter()
            .position(|name| name == &closure_slot.storage_name)
            .and_then(|index| u32::try_from(index).ok())
            .map(LocalLocation)
            .ok_or_else(|| {
                format!(
                    "deopt continuation expected owned {debug_name} slot {slot} storage {} in function {} stack slots",
                    closure_slot.storage_name, function.function_id
                )
            })?;
        let Some(local) = self.locals.get_by_location(location) else {
            return Err(format!(
                "deopt continuation expected owned {debug_name} slot {slot} via local {location:?}, but locals were {}",
                self.locals.describe()
            ));
        };
        let value = local.value();
        if value.is_null() {
            set_deopt_unbound_local_error(closure_slot.storage_name.as_str());
            return Ok(ptr::null_mut());
        }
        unsafe {
            ffi::Py_INCREF(value.cast::<ffi::PyObject>());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_preserved_raw_cell_object_for_slot_owned(
        &self,
        slot: u32,
        debug_name: &str,
    ) -> Result<ObjPtr, String> {
        let state = self.preserved_state()?;
        let cell =
            unsafe { preserved_state::load_preserved_state_owned(state.cast(), i64::from(slot)) };
        if cell.is_null() {
            return Err(format!(
                "deopt continuation expected non-null preserved cell slot {slot} for {debug_name}"
            ));
        }
        Ok(cell.cast())
    }

    #[cold]
    unsafe fn execute_environment_raw_cell_object_for_slot_owned(
        &self,
        slot: u32,
        private: bool,
        debug_name: &str,
    ) -> Result<ObjPtr, String> {
        let function_data = self.source.function_data_obj();
        if function_data.is_null() {
            return Err(format!(
                "deopt continuation expected function data for closure {debug_name} slot {slot}"
            ));
        }
        let function = self.source.function();
        let runtime_layout = FunctionRuntimeDataLayout::from_function(function);
        let length = if private {
            runtime_layout.private_cell_len()
        } else {
            runtime_layout.closure_len()
        };
        if slot as usize >= length {
            return Err(format!(
                "deopt continuation expected closure {debug_name} slot {slot} in function {} with {} closure slots",
                function.function_id,
                runtime_layout.closure_len()
            ));
        }
        let data_slot = if private {
            runtime_layout.private_cell_slot(slot as usize)
        } else {
            runtime_layout.closure_cell_slot(slot as usize)
        };
        let raw_cell = unsafe { *function_data.cast::<ObjPtr>().add(data_slot) };
        if raw_cell.is_null() {
            return Err(format!(
                "deopt continuation expected non-null closure {debug_name} slot {slot} in function {}",
                function.function_id
            ));
        }
        unsafe {
            ffi::Py_INCREF(raw_cell.cast::<ffi::PyObject>());
        }
        Ok(raw_cell)
    }

    #[cold]
    unsafe fn execute_binop_owned(
        &mut self,
        binop: &BinOp<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let left = unsafe { self.execute_expr_owned(&binop.left)? };
        if left.is_null() {
            return Ok(ptr::null_mut());
        }
        let right = match unsafe { self.execute_expr_owned(&binop.right) } {
            Ok(right) => right,
            Err(error) => {
                unsafe { ffi::Py_DECREF(left.cast::<ffi::PyObject>()) };
                return Err(error);
            }
        };
        if right.is_null() {
            unsafe {
                ffi::Py_DECREF(left.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let result = unsafe { execute_binop_kind_owned(binop.kind, left, right) };
        unsafe {
            // Binary IR retains source order, including needle/container for
            // membership. CPython pops the right operand before the left.
            ffi::Py_DECREF(right.cast::<ffi::PyObject>());
            ffi::Py_DECREF(left.cast::<ffi::PyObject>());
        }
        result
    }

    #[cold]
    unsafe fn execute_unary_op_owned(
        &mut self,
        unary: &UnaryOp<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let operand = unsafe { self.execute_expr_owned(&unary.operand)? };
        if operand.is_null() {
            return Ok(ptr::null_mut());
        }
        let result = unsafe { execute_unary_op_kind_owned(unary.kind, operand)? };
        unsafe {
            ffi::Py_DECREF(operand.cast::<ffi::PyObject>());
        }
        Ok(result)
    }

    #[cold]
    unsafe fn execute_getattr_owned(
        &mut self,
        getattr: &soac_core::block_py::GetAttr<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(&getattr.value)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let attr = unsafe { self.execute_expr_owned(&getattr.attr)? };
        if attr.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let result = unsafe {
            ffi::PyObject_GetAttr(value.cast::<ffi::PyObject>(), attr.cast::<ffi::PyObject>())
        };
        unsafe {
            ffi::Py_DECREF(attr.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_getitem_owned(
        &mut self,
        getitem: &soac_core::block_py::GetItem<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(&getitem.value)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let index = unsafe { self.execute_expr_owned(&getitem.index)? };
        if index.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let result = unsafe {
            ffi::PyObject_GetItem(value.cast::<ffi::PyObject>(), index.cast::<ffi::PyObject>())
        };
        unsafe {
            ffi::Py_DECREF(index.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_setattr_owned(
        &mut self,
        setattr: &soac_core::block_py::SetAttr<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        struct OwnedSetterInputs([ObjPtr; 3]);
        impl Drop for OwnedSetterInputs {
            fn drop(&mut self) {
                let values = std::mem::replace(&mut self.0, [ptr::null_mut(); 3]);
                unsafe {
                    let error = ffi::PyErr_GetRaisedException();
                    // STORE_ATTR closes its receiver before its replacement.
                    // Attribute-name ownership is internal to the operation.
                    for value in values {
                        ffi::Py_XDECREF(value.cast::<ffi::PyObject>());
                    }
                    ffi::PyErr_SetRaisedException(error);
                }
            }
        }

        let mut inputs = OwnedSetterInputs([ptr::null_mut(); 3]);
        inputs.0[0] = unsafe { self.execute_expr_owned(&setattr.value)? };
        if inputs.0[0].is_null() {
            return Ok(ptr::null_mut());
        }
        inputs.0[1] = unsafe { self.execute_expr_owned(&setattr.attr)? };
        if inputs.0[1].is_null() {
            return Ok(ptr::null_mut());
        }
        inputs.0[2] = unsafe { self.execute_expr_owned(&setattr.replacement)? };
        if inputs.0[2].is_null() {
            return Ok(ptr::null_mut());
        }
        let rc = unsafe {
            ffi::PyObject_SetAttr(
                inputs.0[0].cast::<ffi::PyObject>(),
                inputs.0[1].cast::<ffi::PyObject>(),
                inputs.0[2].cast::<ffi::PyObject>(),
            )
        };
        drop(inputs);
        if rc != 0 {
            return Ok(ptr::null_mut());
        }
        Ok(owned_none())
    }

    #[cold]
    unsafe fn execute_setitem_owned(
        &mut self,
        setitem: &soac_core::block_py::SetItem<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(&setitem.value)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let index = unsafe { self.execute_expr_owned(&setitem.index)? };
        if index.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let replacement = unsafe { self.execute_expr_owned(&setitem.replacement)? };
        if replacement.is_null() {
            unsafe {
                ffi::Py_DECREF(index.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let rc = unsafe {
            ffi::PyObject_SetItem(
                value.cast::<ffi::PyObject>(),
                index.cast::<ffi::PyObject>(),
                replacement.cast::<ffi::PyObject>(),
            )
        };
        let inputs = [value, index, replacement];
        for index in soac_core::block_py::SetItem::<InstrBlockPy>::INPUT_RELEASE_ORDER {
            unsafe { ffi::Py_DECREF(inputs[index].cast::<ffi::PyObject>()) };
        }
        if rc != 0 {
            return Ok(ptr::null_mut());
        }
        Ok(owned_none())
    }

    #[cold]
    unsafe fn execute_delitem_owned(
        &mut self,
        delitem: &soac_core::block_py::DelItem<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(&delitem.value)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let index = unsafe { self.execute_expr_owned(&delitem.index)? };
        if index.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let rc = unsafe {
            ffi::PyObject_DelItem(value.cast::<ffi::PyObject>(), index.cast::<ffi::PyObject>())
        };
        unsafe {
            ffi::Py_DECREF(index.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
        if rc != 0 {
            return Ok(ptr::null_mut());
        }
        Ok(owned_none())
    }

    #[cold]
    unsafe fn execute_call_owned(
        &mut self,
        call: &soac_core::block_py::Call<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        if let Some(layout) = self.source.function().storage_layout.as_ref()
            && super::call_arguments_runtime::blockpy_owned_operand_call(call, layout)?
        {
            return unsafe { self.execute_owned_operand_call(call) };
        }
        unsafe {
            self.execute_call_parts_owned(
                &call.func,
                &call.args,
                &call.keywords,
                call.frame_namespace.as_ref(),
            )
        }
    }

    /// This path is selected only for real TakeOperand/fresh-call-result inputs.
    /// The runtime helper consumes them; the old borrowed-call cleanup must not
    /// see these raw transport values again.
    #[cold]
    unsafe fn execute_owned_operand_call(
        &mut self,
        call: &soac_core::block_py::Call<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let mut inputs: Vec<*mut ffi::PyObject> = Vec::with_capacity(call.args.len() + 1);
        for input in std::iter::once(call.func.as_ref()).chain(call.args.iter().map(|arg| {
            let CallArgPositional::Positional(input) = arg else {
                unreachable!("selected owned call is positional")
            };
            input
        })) {
            match unsafe { self.execute_expr_owned(input) } {
                Ok(value) if !value.is_null() => inputs.push(value.cast()),
                result => {
                    for value in inputs.into_iter().rev() {
                        unsafe { release_raw_class_owner(value.cast()) };
                    }
                    return result;
                }
            }
        }
        let activation = self
            .source
            .strict_activation()
            .expect("owned call selection has its actual source activation");
        let result = unsafe {
            super::call_arguments_runtime::dp_jit_call_owned_operands(
                activation.environment().header(),
                inputs.as_mut_ptr(),
                inputs.len(),
            )
        };
        debug_assert!(inputs.iter().all(|value| value.is_null()));
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_tuple_owned(
        &mut self,
        tuple_expr: &soac_core::block_py::Tuple<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let tuple_len = match ffi::Py_ssize_t::try_from(tuple_expr.values.len()) {
            Ok(tuple_len) => tuple_len,
            Err(_) => {
                return Err(format!(
                    "deopt continuation tuple has too many values: {}",
                    tuple_expr.values.len()
                ));
            }
        };
        let tuple = unsafe { ffi::PyTuple_New(tuple_len) };
        if tuple.is_null() {
            return Ok(ptr::null_mut());
        }
        for (index, expr) in tuple_expr.values.iter().enumerate() {
            let value = unsafe { self.execute_expr_owned(expr)? };
            if value.is_null() {
                unsafe {
                    ffi::Py_DECREF(tuple);
                }
                return Ok(ptr::null_mut());
            }
            let index = ffi::Py_ssize_t::try_from(index)
                .expect("tuple value index should fit after tuple length conversion");
            if unsafe { ffi::PyTuple_SetItem(tuple, index, value.cast::<ffi::PyObject>()) } != 0 {
                unsafe {
                    ffi::Py_DECREF(tuple);
                }
                return Ok(ptr::null_mut());
            }
        }
        Ok(tuple.cast())
    }

    fn frame_module_globals(
        &self,
        namespace: Option<&FrameNamespace<InstrBlockPy>>,
    ) -> Result<Option<ObjPtr>, String> {
        if !matches!(namespace, Some(FrameNamespace::ModuleGlobals)) {
            return Ok(None);
        }
        let globals = self.source.globals_obj();
        if globals.is_null() {
            return Err("module-frame call is missing its defining globals".into());
        }
        Ok(Some(globals))
    }

    #[cold]
    unsafe fn execute_call_parts_owned(
        &mut self,
        callable_expr: &InstrBlockPy,
        positional_args: &[CallArgPositional<InstrBlockPy>],
        keyword_args: &[CallArgKeyword<InstrBlockPy>],
        frame_namespace: Option<&FrameNamespace<InstrBlockPy>>,
    ) -> Result<ObjPtr, String> {
        unsafe {
            self.execute_call_parts_owned_with_invocation(
                callable_expr,
                positional_args,
                keyword_args,
                frame_namespace,
                CallInvocation::Ordinary,
            )
        }
    }

    #[cold]
    unsafe fn execute_call_parts_owned_with_invocation(
        &mut self,
        callable_expr: &InstrBlockPy,
        positional_args: &[CallArgPositional<InstrBlockPy>],
        keyword_args: &[CallArgKeyword<InstrBlockPy>],
        frame_namespace: Option<&FrameNamespace<InstrBlockPy>>,
        invocation: CallInvocation,
    ) -> Result<ObjPtr, String> {
        let namespace = match frame_namespace {
            Some(FrameNamespace::ModuleGlobals) => {
                let globals = self
                    .frame_module_globals(frame_namespace)?
                    .expect("module context");
                Some(unsafe {
                    Bound::<PyAny>::from_borrowed_ptr(Python::assume_attached(), globals.cast())
                })
            }
            Some(FrameNamespace::Mapping(namespace)) => {
                if !matches!(namespace.as_ref(), InstrBlockPy::Load(_)) {
                    return Err("class-frame operand must be a resolved namespace binding".into());
                }
                let value = unsafe { self.execute_expr_owned(namespace)? };
                if value.is_null() {
                    return Ok(ptr::null_mut());
                }
                Some(unsafe {
                    Bound::<PyAny>::from_owned_ptr(Python::assume_attached(), value.cast())
                })
            }
            None => None,
        };
        if matches!(invocation, CallInvocation::Ordinary)
            && positional_args.is_empty()
            && keyword_args.is_empty()
            && self.source.static_runtime_name(callable_expr) == Some(RuntimeName::Globals)
        {
            return Ok(unsafe { self.entry_globals_owned() });
        }

        if matches!(invocation, CallInvocation::Ordinary)
            && keyword_args.is_empty()
            && let [
                CallArgPositional::Positional(value_expr),
                CallArgPositional::Positional(arity_expr),
            ] = positional_args
            && self.source.static_runtime_name(callable_expr) == Some(RuntimeName::UnpackFixed)
        {
            let value = unsafe { self.execute_expr_owned(value_expr)? };
            if value.is_null() {
                return Ok(ptr::null_mut());
            }
            let arity_obj = unsafe { self.execute_expr_owned(arity_expr)? };
            if arity_obj.is_null() {
                unsafe { ffi::Py_DECREF(value.cast::<ffi::PyObject>()) };
                return Ok(ptr::null_mut());
            }
            let arity = unsafe { ffi::PyLong_AsLongLong(arity_obj.cast::<ffi::PyObject>()) };
            unsafe { ffi::Py_DECREF(arity_obj.cast::<ffi::PyObject>()) };
            if arity == -1 && !unsafe { ffi::PyErr_Occurred() }.is_null() {
                unsafe { ffi::Py_DECREF(value.cast::<ffi::PyObject>()) };
                return Ok(ptr::null_mut());
            }
            let result = unsafe {
                super::specialized_helpers::dp_jit_unpack_fixed_slow(
                    ffi::PyThreadState_Get().cast(),
                    value,
                    arity,
                )
            };
            unsafe { ffi::Py_DECREF(value.cast::<ffi::PyObject>()) };
            return Ok(result);
        }

        let callable = unsafe { self.execute_expr_owned(callable_expr)? };
        if callable.is_null() {
            return Ok(ptr::null_mut());
        }
        let implicit_super = matches!(invocation, CallInvocation::Ordinary)
            && self.source.static_runtime_name(callable_expr) == Some(RuntimeName::CallSuper)
            && keyword_args.is_empty()
            && matches!(
                positional_args,
                [
                    CallArgPositional::Positional(_),
                    CallArgPositional::Positional(_),
                    CallArgPositional::Positional(_)
                ]
            );
        let mut args = Vec::with_capacity(positional_args.len());
        for (index, arg) in positional_args.iter().enumerate() {
            match arg {
                CallArgPositional::Positional(expr) => {
                    let value = if implicit_super && index == 2 {
                        unsafe { self.execute_implicit_super_instance_owned(expr)? }
                    } else {
                        unsafe { self.execute_expr_owned(expr)? }
                    };
                    if value.is_null() {
                        unsafe {
                            release_owned_values(args);
                            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                        }
                        return Ok(ptr::null_mut());
                    }
                    args.push(value);
                }
                CallArgPositional::Starred(expr) => {
                    let value = unsafe { self.execute_expr_owned(expr)? };
                    if value.is_null() {
                        unsafe {
                            release_owned_values(args);
                            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                        }
                        return Ok(ptr::null_mut());
                    }
                    let tuple = unsafe { ffi::PySequence_Tuple(value.cast::<ffi::PyObject>()) };
                    unsafe {
                        ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                    }
                    if tuple.is_null() {
                        unsafe {
                            release_owned_values(args);
                            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                        }
                        return Ok(ptr::null_mut());
                    }
                    let tuple_len = unsafe { ffi::PyTuple_Size(tuple) };
                    if tuple_len < 0 {
                        unsafe {
                            ffi::Py_DECREF(tuple);
                            release_owned_values(args);
                            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                        }
                        return Ok(ptr::null_mut());
                    }
                    for index in 0..tuple_len {
                        let item = unsafe { ffi::PyTuple_GetItem(tuple, index) };
                        if item.is_null() {
                            unsafe {
                                ffi::Py_DECREF(tuple);
                                release_owned_values(args);
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        unsafe {
                            ffi::Py_INCREF(item);
                        }
                        args.push(item.cast());
                    }
                    unsafe {
                        ffi::Py_DECREF(tuple);
                    }
                }
            };
        }

        let args_len = match ffi::Py_ssize_t::try_from(args.len()) {
            Ok(args_len) => args_len,
            Err(_) => {
                unsafe {
                    release_owned_values(args);
                    ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                }
                return Err(format!(
                    "deopt continuation call has too many positional args: {}",
                    positional_args.len()
                ));
            }
        };
        if !keyword_args.is_empty()
            && positional_args
                .iter()
                .all(|arg| matches!(arg, CallArgPositional::Positional(_)))
            && keyword_args
                .iter()
                .all(|arg| matches!(arg, CallArgKeyword::Named { .. }))
        {
            return unsafe {
                self.execute_named_call_owned(
                    callable,
                    args,
                    keyword_args,
                    namespace.as_ref(),
                    invocation,
                )
            };
        }
        let kwargs = if keyword_args.is_empty() {
            ptr::null_mut()
        } else {
            let kwargs = unsafe { ffi::PyDict_New() };
            if kwargs.is_null() {
                unsafe {
                    release_owned_values(args.into_iter().rev());
                    ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                }
                return Ok(ptr::null_mut());
            }
            for keyword in keyword_args {
                match keyword {
                    CallArgKeyword::Named { arg, value } => {
                        let value = unsafe { self.execute_expr_owned(value)? };
                        if value.is_null() {
                            unsafe {
                                ffi::Py_DECREF(kwargs);
                                release_owned_values(args.into_iter().rev());
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let name_len = match ffi::Py_ssize_t::try_from(arg.as_str().len()) {
                            Ok(name_len) => name_len,
                            Err(_) => {
                                unsafe {
                                    ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                                    ffi::Py_DECREF(kwargs);
                                    release_owned_values(args.into_iter().rev());
                                    ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                                }
                                return Err(format!(
                                    "deopt continuation keyword name {:?} is too large to materialize as PyUnicode",
                                    arg.as_str()
                                ));
                            }
                        };
                        let key = unsafe {
                            ffi::PyUnicode_FromStringAndSize(arg.as_str().as_ptr().cast(), name_len)
                        };
                        if key.is_null() {
                            unsafe {
                                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                                ffi::Py_DECREF(kwargs);
                                release_owned_values(args.into_iter().rev());
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let one_keyword = unsafe { ffi::PyDict_New() };
                        if one_keyword.is_null() {
                            unsafe {
                                ffi::Py_DECREF(key);
                                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                                ffi::Py_DECREF(kwargs);
                                release_owned_values(args.into_iter().rev());
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let rc = unsafe {
                            ffi::PyDict_SetItem(one_keyword, key, value.cast::<ffi::PyObject>())
                        };
                        unsafe {
                            ffi::Py_DECREF(key);
                            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                        }
                        if rc != 0 {
                            unsafe {
                                ffi::Py_DECREF(one_keyword);
                                ffi::Py_DECREF(kwargs);
                                release_owned_values(args.into_iter().rev());
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let merged = unsafe {
                            merge_kwargs_or_format_error(
                                callable.cast::<ffi::PyObject>(),
                                kwargs,
                                one_keyword,
                            )
                        };
                        unsafe {
                            ffi::Py_DECREF(one_keyword);
                        }
                        if !merged {
                            unsafe {
                                ffi::Py_DECREF(kwargs);
                                release_owned_values(args.into_iter().rev());
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                    }
                    CallArgKeyword::Starred(value_expr) => {
                        let value = unsafe { self.execute_expr_owned(value_expr)? };
                        if value.is_null() {
                            unsafe {
                                ffi::Py_DECREF(kwargs);
                                release_owned_values(args.into_iter().rev());
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                        let merged = unsafe {
                            merge_kwargs_or_format_error(
                                callable.cast::<ffi::PyObject>(),
                                kwargs,
                                value.cast::<ffi::PyObject>(),
                            )
                        };
                        unsafe {
                            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
                        }
                        if !merged {
                            unsafe {
                                ffi::Py_DECREF(kwargs);
                                release_owned_values(args.into_iter().rev());
                                ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
                            }
                            return Ok(ptr::null_mut());
                        }
                    }
                }
            }
            kwargs
        };
        // Keep positional operands in the native stack representation. A staging
        // Python tuple is observable to a METH_VARARGS callee such as
        // gc.get_referrers, whose own required argument tuple is created by the
        // native vectorcall fallback.
        let mut keyword_names = ptr::null_mut();
        let native_args = if !kwargs.is_null() && unsafe { ffi::PyDict_Size(kwargs) } != 0 {
            // The native helper preserves keyword identities and error behavior.
            // Positional entries are borrowed; only the appended keyword values
            // and the returned names tuple own references. Failure frees its
            // allocation and references internally and leaves keyword_names NULL.
            unsafe {
                _PyStack_UnpackDict(
                    ffi::PyThreadState_Get(),
                    args.as_ptr().cast(),
                    args_len,
                    kwargs,
                    &mut keyword_names,
                )
            }
        } else {
            args.as_ptr().cast()
        };
        let result = if native_args.is_null() {
            ptr::null_mut()
        } else {
            unsafe {
                invocation.invoke(
                    callable.cast(),
                    native_args,
                    args.len(),
                    keyword_names,
                    self.source.globals_obj().cast(),
                    namespace.as_ref().map_or(ptr::null_mut(), Bound::as_ptr),
                    self.source.builtins_obj().cast(),
                )
            }
        };
        unsafe {
            let error = ffi::PyErr_GetRaisedException();
            if !keyword_names.is_null() {
                let keyword_count = ffi::PyTuple_Size(keyword_names);
                for index in 0..keyword_count {
                    ffi::Py_DECREF(*native_args.add(args.len() + index as usize));
                }
                _PyStack_UnpackDict_FreeNoDecRef(native_args, keyword_names);
            }
            if !kwargs.is_null() {
                ffi::Py_DECREF(kwargs);
            }
            // Retire our owned positional inputs while preserving the call's
            // exception. This does not model CPython's transient owners.
            release_owned_values(args.into_iter().rev());
            ffi::Py_DECREF(callable.cast::<ffi::PyObject>());
            PyErr_SetRaisedException(error);
        }
        Ok(result.cast())
    }

    /// Plain CALL_KW operands stay in a native vector, not a Python mapping.
    /// Unpacking calls retain their separate native mapping semantics above.
    #[cold]
    unsafe fn execute_named_call_owned(
        &mut self,
        callable: ObjPtr,
        mut arguments: Vec<ObjPtr>,
        keywords: &[CallArgKeyword<InstrBlockPy>],
        namespace: Option<&Bound<'_, PyAny>>,
        invocation: CallInvocation,
    ) -> Result<ObjPtr, String> {
        let positional_count = arguments.len();
        for keyword in keywords {
            let CallArgKeyword::Named { value, .. } = keyword else {
                unreachable!("plain named call cannot contain an unpacking operand");
            };
            let value = match unsafe { self.execute_expr_owned(value) } {
                Ok(value) if !value.is_null() => value,
                result => {
                    unsafe { release_call_inputs(callable, arguments, ptr::null_mut()) };
                    return result;
                }
            };
            arguments.push(value);
        }
        let keyword_count = match ffi::Py_ssize_t::try_from(keywords.len()) {
            Ok(count) => count,
            Err(_) => {
                unsafe { release_call_inputs(callable, arguments, ptr::null_mut()) };
                return Err("deopt continuation has too many named call operands".into());
            }
        };
        let names = unsafe { ffi::PyTuple_New(keyword_count) };
        if names.is_null() {
            unsafe { release_call_inputs(callable, arguments, ptr::null_mut()) };
            return Ok(ptr::null_mut());
        }
        for (index, keyword) in keywords.iter().enumerate() {
            let CallArgKeyword::Named { arg, .. } = keyword else {
                unreachable!("plain named call cannot contain an unpacking operand");
            };
            let name_length = match ffi::Py_ssize_t::try_from(arg.as_str().len()) {
                Ok(length) => length,
                Err(_) => {
                    unsafe { release_call_inputs(callable, arguments, names) };
                    return Err("deopt continuation keyword name is too large".into());
                }
            };
            let name = unsafe {
                ffi::PyUnicode_FromStringAndSize(arg.as_str().as_ptr().cast(), name_length)
            };
            // PyTuple_SetItem steals name, including its error path.
            if name.is_null()
                || unsafe { ffi::PyTuple_SetItem(names, index as ffi::Py_ssize_t, name) } != 0
            {
                unsafe { release_call_inputs(callable, arguments, names) };
                return Ok(ptr::null_mut());
            }
        }
        let result = unsafe {
            invocation.invoke(
                callable.cast(),
                arguments.as_ptr().cast(),
                positional_count,
                names,
                self.source.globals_obj().cast(),
                namespace.map_or(ptr::null_mut(), Bound::as_ptr),
                self.source.builtins_obj().cast(),
            )
        };
        unsafe { release_call_inputs(callable, arguments, names) };
        Ok(result.cast())
    }

    #[cold]
    unsafe fn execute_implicit_super_instance_owned(
        &mut self,
        expression: &InstrBlockPy,
    ) -> Result<ObjPtr, String> {
        // CallSuper's third operand is the compiler-inserted first argument,
        // not a source-level read. Match the compiled path's deleted-local
        // check without converting unrelated user-load or callback exceptions.
        if let InstrBlockPy::Load(load) = expression
            && let Some(location) = load.name.local_location()
            && let Some(local) = self.locals.get_by_location(location)
            && local.value().is_null()
        {
            unsafe { super::specialized_helpers::dp_jit_raise_super_arg_deleted() };
            return Ok(ptr::null_mut());
        }
        unsafe { self.execute_expr_owned(expression) }
    }

    #[cold]
    unsafe fn entry_globals_owned(&self) -> ObjPtr {
        let globals = self.source.globals_obj();
        if globals.is_null() {
            unsafe {
                ffi::PyErr_SetString(
                    ffi::PyExc_RuntimeError,
                    c"entry interpreter globals() has no module globals".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        unsafe {
            ffi::Py_INCREF(globals.cast::<ffi::PyObject>());
        }
        globals
    }

    #[cold]
    unsafe fn execute_raise_term_owned(
        &mut self,
        raise: &soac_core::block_py::TermRaise<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        let Some(exc_expr) = &raise.exc else {
            unsafe { crate::handled_exception::reraise_current() };
            return Ok(ptr::null_mut());
        };
        let exc = unsafe { self.execute_expr_owned(exc_expr)? };
        if exc.is_null() {
            return Ok(ptr::null_mut());
        }
        if raise.disposition.is_normalized() {
            unsafe {
                crate::handled_exception::restore_raised_exception(exc.cast());
                ffi::Py_DECREF(exc.cast());
            }
        } else {
            unsafe {
                set_raise_exception_owned(exc);
            }
        }
        Ok(ptr::null_mut())
    }

    #[cold]
    unsafe fn execute_load_owned(
        &mut self,
        name: &str,
        location: NameLocation,
        cell_binding: Option<&CellLoadBinding>,
    ) -> Result<ObjPtr, String> {
        match location {
            NameLocation::Local(location) => self.execute_return_local(name, location),
            NameLocation::Preserved(location) => unsafe {
                self.execute_preserved_load_owned(location)
            },
            NameLocation::Global(slot) => unsafe {
                self.execute_return_global(name, i64::from(slot.slot()))
            },
            NameLocation::GlobalName => unsafe { self.execute_return_global(name, -1) },
            NameLocation::RuntimeName(runtime_name) => unsafe {
                self.execute_runtime_name_deopt(runtime_name)
            },
            NameLocation::Constant(constant_index) => self.execute_module_constant(constant_index),
            NameLocation::Cell(location) => unsafe {
                let binding = cell_binding.ok_or_else(|| {
                    format!("resolved cell load {name:?} has no original source binding",)
                })?;
                self.execute_cell_load_owned(name, location, binding)
            },
        }
    }

    #[cold]
    fn preserved_state(&self) -> Result<ObjPtr, String> {
        let state_name = self
            .source
            .function()
            .storage_layout()
            .as_ref()
            .and_then(|layout| {
                layout.generator_resume_parameter(
                    soac_core::block_py::GeneratorResumeParamRole::StateValue,
                )
            })
            .ok_or("deopt continuation has no generator state parameter role")?;
        let state = self.locals.get_by_name(state_name).ok_or_else(|| {
            format!(
                "deopt continuation expected its resolved generator state parameter for preserved state: {}",
                self.locals.describe()
            )
        })?;
        let value = state.value();
        if value.is_null() {
            return Err("deopt continuation found null generator resume state".to_string());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_preserved_load_owned(
        &self,
        location: PreservedLocation,
    ) -> Result<ObjPtr, String> {
        let state = self.preserved_state()?;
        Ok(unsafe {
            preserved_state::load_preserved_state_owned(state.cast(), i64::from(location.slot()))
                .cast()
        })
    }

    #[cold]
    unsafe fn execute_cell_load_owned(
        &self,
        name: &str,
        location: CellLocation,
        binding: &CellLoadBinding,
    ) -> Result<ObjPtr, String> {
        let cell = unsafe { self.execute_raw_cell_object_for_location_owned(location, name)? };
        if cell.is_null() {
            return Ok(ptr::null_mut());
        }
        let logical_name = binding.logical_name.as_str();
        let name_obj = unsafe {
            ffi::PyUnicode_FromStringAndSize(
                logical_name.as_ptr().cast(),
                logical_name.len() as isize,
            )
        };
        if name_obj.is_null() {
            unsafe { ffi::Py_DECREF(cell.cast::<ffi::PyObject>()) };
            return Ok(ptr::null_mut());
        }
        let value = unsafe {
            super::specialized_helpers::dp_jit_load_cell(
                cell,
                name_obj.cast(),
                i64::from(binding.kind == CellBindingKind::Capture),
            )
        };
        unsafe {
            ffi::Py_DECREF(name_obj);
            ffi::Py_DECREF(cell.cast::<ffi::PyObject>());
        }
        Ok(value)
    }

    #[cold]
    fn execute_return_local(&self, name: &str, location: LocalLocation) -> Result<ObjPtr, String> {
        let local = self.locals.get_by_location(location).ok_or_else(|| {
            format!(
                "deopt continuation expected local {name} at {location:?}, but it was not materialized: {}",
                self.locals.describe()
            )
        })?;
        debug_assert_eq!(
            local.binding().name.as_str(),
            name,
            "deopt continuation local slot {location:?} should be bound to {name}"
        );
        let value = local.value();
        if value.is_null() {
            let function = self.source.function();
            let name = super::owned_slots::local_diagnostic_name(
                &function.scope,
                function.storage_layout.as_ref(),
                location,
                name,
            );
            set_deopt_unbound_local_error(name);
            return Ok(ptr::null_mut());
        }
        unsafe { ffi::Py_INCREF(value.cast::<ffi::PyObject>()) };
        Ok(value)
    }

    #[cold]
    fn execute_module_constant(&self, constant_index: u32) -> Result<ObjPtr, String> {
        let value = self.source.module_constant_ptr(constant_index)?;
        if value.is_null() {
            return Err(format!(
                "deopt continuation expected non-null module constant {constant_index}"
            ));
        }
        unsafe { ffi::Py_INCREF(value.cast::<ffi::PyObject>()) };
        Ok(value)
    }

    #[cold]
    unsafe fn execute_del_owned(
        &mut self,
        del: &soac_core::block_py::Del<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        match del.name.location {
            NameLocation::Local(location) => unsafe {
                self.execute_local_del_owned(del.name.id.as_str(), location, del.quietly)
            },
            NameLocation::Global(_) | NameLocation::GlobalName => unsafe {
                self.execute_global_del_owned(del.name.id.as_str(), del.quietly)
            },
            NameLocation::Cell(location) => unsafe {
                self.execute_cell_del_owned(del.name.id.as_str(), location, del.quietly)
            },
            NameLocation::Preserved(location) => unsafe {
                self.execute_preserved_del_owned(del.name.id.as_str(), location, del.quietly)
            },
            location => Err(format!(
                "deopt continuation does not support deleting {location:?} for {:?}",
                del.name.id.as_str()
            )),
        }
    }

    #[cold]
    unsafe fn execute_local_del_owned(
        &mut self,
        name: &str,
        location: LocalLocation,
        quietly: bool,
    ) -> Result<ObjPtr, String> {
        let function = self.source.function();
        let diagnostic_name = super::owned_slots::local_diagnostic_name(
            &function.scope,
            function.storage_layout.as_ref(),
            location,
            name,
        );
        let Some(local) = self.locals.get_by_location_mut(location) else {
            if !quietly {
                set_deopt_unbound_local_error(diagnostic_name);
                return Ok(ptr::null_mut());
            }
            return unsafe { self.execute_runtime_name_deopt(RuntimeName::None) };
        };
        debug_assert_eq!(
            local.binding().name.as_str(),
            name,
            "deopt continuation local slot {location:?} should be bound to {name}"
        );
        if local.value().is_null() {
            if !quietly {
                set_deopt_unbound_local_error(diagnostic_name);
                return Ok(ptr::null_mut());
            }
        } else {
            unsafe {
                local.delete_value();
            }
        }
        unsafe { self.execute_runtime_name_deopt(RuntimeName::None) }
    }

    #[cold]
    unsafe fn execute_cell_del_owned(
        &self,
        name: &str,
        location: CellLocation,
        quietly: bool,
    ) -> Result<ObjPtr, String> {
        let cell = unsafe { self.execute_raw_cell_object_for_location_owned(location, name)? };
        if cell.is_null() {
            return Ok(ptr::null_mut());
        }
        let result = if quietly {
            unsafe { super::specialized_helpers::dp_jit_del_deref_quietly(cell) }
        } else {
            unsafe { super::specialized_helpers::dp_jit_del_deref(cell) }
        };
        unsafe {
            ffi::Py_DECREF(cell.cast::<ffi::PyObject>());
        }
        Ok(result)
    }

    #[cold]
    unsafe fn execute_preserved_del_owned(
        &self,
        name: &str,
        location: PreservedLocation,
        quietly: bool,
    ) -> Result<ObjPtr, String> {
        let state = self.preserved_state()?;
        let name_len = ffi::Py_ssize_t::try_from(name.len()).map_err(|_| {
            format!("preserved-delete deopt name {name:?} is too large to materialize as PyUnicode")
        })?;
        let name_obj = unsafe { ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name_len) };
        if name_obj.is_null() {
            return Ok(ptr::null_mut());
        }
        let result = if quietly {
            unsafe {
                super::specialized_helpers::dp_jit_del_preserved_quietly(
                    state,
                    i64::from(location.slot()),
                    name_obj.cast(),
                )
            }
        } else {
            unsafe {
                super::specialized_helpers::dp_jit_del_preserved(
                    state,
                    i64::from(location.slot()),
                    name_obj.cast(),
                )
            }
        };
        unsafe {
            ffi::Py_DECREF(name_obj);
        }
        Ok(result)
    }

    #[cold]
    unsafe fn execute_global_del_owned(&self, name: &str, quietly: bool) -> Result<ObjPtr, String> {
        let globals_obj = self.source.globals_obj();
        if globals_obj.is_null() {
            return Err(format!(
                "deopt continuation expected module globals for global delete {name:?}"
            ));
        }
        let name_len = ffi::Py_ssize_t::try_from(name.len()).map_err(|_| {
            format!("global-delete deopt name {name:?} is too large to materialize as PyUnicode")
        })?;
        let name_obj = unsafe { ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name_len) };
        if name_obj.is_null() {
            return Ok(ptr::null_mut());
        }
        let result = if quietly {
            unsafe {
                super::specialized_helpers::dp_jit_del_global_quietly(
                    globals_obj,
                    name_obj.cast::<c_void>(),
                    -1,
                )
            }
        } else {
            unsafe {
                super::specialized_helpers::dp_jit_del_global(
                    globals_obj,
                    name_obj.cast::<c_void>(),
                    -1,
                )
            }
        };
        unsafe { ffi::Py_DECREF(name_obj) };
        Ok(result)
    }

    #[cold]
    unsafe fn execute_store_owned(
        &mut self,
        store: &soac_core::block_py::Store<InstrBlockPy>,
    ) -> Result<ObjPtr, String> {
        match store.name.location {
            NameLocation::Local(location) => unsafe {
                self.execute_local_store_owned(
                    store.name.id.as_str(),
                    location,
                    store.value.as_ref(),
                )
            },
            NameLocation::Global(_) | NameLocation::GlobalName => unsafe {
                self.execute_global_store_owned(store.name.id.as_str(), store.value.as_ref())
            },
            NameLocation::Cell(location) => unsafe {
                self.execute_cell_store_owned(
                    store.name.id.as_str(),
                    location,
                    store.value.as_ref(),
                )
            },
            NameLocation::Preserved(location) => unsafe {
                self.execute_preserved_store_owned(location, store.value.as_ref())
            },
            location => Err(format!(
                "deopt continuation does not support storing {location:?} for {:?}",
                store.name.id.as_str()
            )),
        }
    }

    #[cold]
    unsafe fn execute_preserved_store_owned(
        &mut self,
        location: PreservedLocation,
        value_expr: &InstrBlockPy,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(value_expr)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let state = self.preserved_state()?;
        let result = unsafe {
            preserved_state::store_preserved_state(
                state.cast(),
                i64::from(location.slot()),
                value.cast(),
            )
            .cast()
        };
        unsafe {
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
        Ok(result)
    }

    #[cold]
    unsafe fn execute_local_store_owned(
        &mut self,
        name: &str,
        location: LocalLocation,
        value_expr: &InstrBlockPy,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(value_expr)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let Some(local) = self.locals.get_by_location_mut(location) else {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Err(format!(
                "deopt continuation expected local {name} at {location:?} for store, but it was not materialized: {}",
                self.locals.describe()
            ));
        };
        debug_assert_eq!(
            local.binding().name.as_str(),
            name,
            "deopt continuation local slot {location:?} should be bound to {name}"
        );
        unsafe {
            local.replace_with_owned_value(value);
            ffi::Py_INCREF(value.cast::<ffi::PyObject>());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_cell_store_owned(
        &mut self,
        name: &str,
        location: CellLocation,
        value_expr: &InstrBlockPy,
    ) -> Result<ObjPtr, String> {
        let value = unsafe { self.execute_expr_owned(value_expr)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let cell = unsafe { self.execute_raw_cell_object_for_location_owned(location, name)? };
        if cell.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        let result = unsafe { super::specialized_helpers::dp_jit_store_cell(cell, value) };
        unsafe {
            ffi::Py_DECREF(cell.cast::<ffi::PyObject>());
        }
        if result.is_null() {
            unsafe {
                ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            }
            return Ok(ptr::null_mut());
        }
        unsafe {
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_global_store_owned(
        &mut self,
        name: &str,
        value_expr: &InstrBlockPy,
    ) -> Result<ObjPtr, String> {
        let globals_obj = self.source.globals_obj();
        if globals_obj.is_null() {
            return Err(format!(
                "deopt continuation expected module globals for global store {name:?}"
            ));
        }
        let value = unsafe { self.execute_expr_owned(value_expr)? };
        if value.is_null() {
            return Ok(ptr::null_mut());
        }
        let name_len = match ffi::Py_ssize_t::try_from(name.len()) {
            Ok(name_len) => name_len,
            Err(_) => {
                unsafe { ffi::Py_DECREF(value.cast::<ffi::PyObject>()) };
                return Err(format!(
                    "global-store deopt name {name:?} is too large to materialize as PyUnicode"
                ));
            }
        };
        let name_obj = unsafe { ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name_len) };
        if name_obj.is_null() {
            unsafe { ffi::Py_DECREF(value.cast::<ffi::PyObject>()) };
            return Ok(ptr::null_mut());
        }
        let rc = unsafe {
            ffi::PyObject_SetItem(
                globals_obj.cast::<ffi::PyObject>(),
                name_obj,
                value.cast::<ffi::PyObject>(),
            )
        };
        unsafe { ffi::Py_DECREF(name_obj) };
        if rc != 0 {
            unsafe { ffi::Py_DECREF(value.cast::<ffi::PyObject>()) };
            return Ok(ptr::null_mut());
        }
        Ok(value)
    }

    #[cold]
    unsafe fn execute_return_global(
        &self,
        name: &str,
        expected_index: i64,
    ) -> Result<ObjPtr, String> {
        let globals_obj = self.source.globals_obj();
        if globals_obj.is_null() {
            return Err(format!(
                "deopt continuation expected module globals for return-global {name:?}"
            ));
        }
        let name_len = ffi::Py_ssize_t::try_from(name.len()).map_err(|_| {
            format!("return-global deopt name {name:?} is too large to materialize as PyUnicode")
        })?;
        let name_obj = unsafe { ffi::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name_len) };
        if name_obj.is_null() {
            return Ok(ptr::null_mut());
        }
        let result = unsafe {
            super::specialized_helpers::soac_runtime_load_global_slow(
                globals_obj,
                self.source.builtins_obj(),
                name_obj.cast::<c_void>(),
                expected_index,
            )
        };
        unsafe { ffi::Py_DECREF(name_obj) };
        Ok(result)
    }

    #[cold]
    unsafe fn release_frame_owned_values(
        &mut self,
        mut result: Result<FrameExecutionOutcome, String>,
    ) -> Result<FrameExecutionOutcome, String> {
        let terminal = !self.yielded(&result);
        unsafe {
            self.locals.release_frame_owned_values();
        }
        if terminal && let Some(activation) = self.source.strict_activation() {
            let environment =
                activation.environment().header() as *const crate::FunctionEnvAbiHeader;
            let status = unsafe {
                super::specialized_helpers::dp_jit_retire_terminal_roots(
                    environment.cast_mut().cast(),
                )
            };
            if status != 0 {
                // A failed internal ownership invariant must not leak a
                // successful body result or return it with an error pending.
                let previous = std::mem::replace(
                    &mut result,
                    Ok(FrameExecutionOutcome::Return(ptr::null_mut())),
                );
                drop(previous);
            }
        }
        unsafe { HandledExceptionState::release_residual(self.handled_state) };
        result
    }
}

unsafe fn execute_abrupt_kind_arg_owned(kind: AbruptKind) -> ObjPtr {
    unsafe { ffi::PyLong_FromLongLong(super::abrupt_kind_tag(kind)).cast() }
}

/// The raised-error snapshot belongs to one edge handoff, not the source
/// frame. Argument owners are transferred only as their destination is
/// installed; a partial evaluation or admission failure releases the rest.
struct OwnedExceptionEdge {
    raised: ObjPtr,
    values: Vec<ObjPtr>,
}

impl OwnedExceptionEdge {
    fn new(raised: ObjPtr) -> Self {
        Self {
            raised,
            values: Vec::new(),
        }
    }

    unsafe fn current_exception_owned(&mut self) -> ObjPtr {
        if self.raised.is_null() {
            self.raised = unsafe { take_current_raised_exception_owned() };
            if self.raised.is_null() {
                return ptr::null_mut();
            }
        }
        unsafe {
            ffi::Py_INCREF(self.raised.cast::<ffi::PyObject>());
        }
        self.raised
    }
}

impl Drop for OwnedExceptionEdge {
    fn drop(&mut self) {
        unsafe {
            let error = ffi::PyErr_GetRaisedException();
            for value in self.values.drain(..).rev() {
                ffi::Py_XDECREF(value.cast());
            }
            ffi::Py_XDECREF(std::mem::replace(&mut self.raised, ptr::null_mut()).cast());
            ffi::PyErr_SetRaisedException(error);
        }
    }
}

unsafe fn take_current_raised_exception_owned() -> ObjPtr {
    let tstate = unsafe { ffi::PyThreadState_Get() };
    let current_exception_slot = unsafe {
        tstate
            .cast::<u8>()
            .add(super::PY_THREAD_STATE_CURRENT_EXCEPTION_OFFSET as usize)
            .cast::<*mut ffi::PyObject>()
    };
    let current_exception = unsafe { *current_exception_slot };
    if current_exception.is_null() {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_RuntimeError,
                c"No active exception to reraise".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    unsafe {
        *current_exception_slot = ptr::null_mut();
    }
    current_exception.cast()
}

fn owned_none() -> ObjPtr {
    unsafe {
        let none = ffi::Py_None();
        ffi::Py_INCREF(none);
        none.cast()
    }
}

pub(super) unsafe fn release_raw_class_owner(value: ObjPtr) {
    if !value.is_null() {
        unsafe {
            let error = ffi::PyErr_GetRaisedException();
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
            ffi::PyErr_SetRaisedException(error);
        }
    }
}

unsafe fn release_owned_values(values: impl IntoIterator<Item = ObjPtr>) {
    for value in values {
        unsafe {
            ffi::Py_DECREF(value.cast::<ffi::PyObject>());
        }
    }
}

unsafe fn release_call_inputs(
    callable: ObjPtr,
    arguments: Vec<ObjPtr>,
    keyword_names: *mut ffi::PyObject,
) {
    // Match _Py_VectorCall_StackRefSteal: names, then all operands in reverse,
    // then callable. A deallocator must not replace the original call error.
    unsafe {
        let error = ffi::PyErr_GetRaisedException();
        ffi::Py_XDECREF(keyword_names);
        release_owned_values(arguments.into_iter().rev());
        ffi::Py_DECREF(callable.cast());
        PyErr_SetRaisedException(error);
    }
}

unsafe fn set_raise_exception_owned(exc: ObjPtr) {
    let exc = exc.cast::<ffi::PyObject>();
    unsafe {
        if ffi::PyExceptionClass_Check(exc) != 0 {
            ffi::PyErr_SetObject(exc, ptr::null_mut());
            ffi::Py_DECREF(exc);
        } else if ffi::PyExceptionInstance_Check(exc) != 0 {
            let exc_type = ffi::PyExceptionInstance_Class(exc);
            ffi::PyErr_SetObject(exc_type, exc);
            ffi::Py_DECREF(exc);
        } else {
            ffi::Py_DECREF(exc);
            ffi::PyErr_SetString(
                ffi::PyExc_TypeError,
                c"exceptions must derive from BaseException".as_ptr(),
            );
        }
    }
}

unsafe fn merge_kwargs_or_format_error(
    callable: *mut ffi::PyObject,
    kwargs: *mut ffi::PyObject,
    update: *mut ffi::PyObject,
) -> bool {
    unsafe {
        if _PyDict_MergeEx(kwargs, update, 2) == 0 {
            return true;
        }
        _PyEval_FormatKwargsError(ffi::PyThreadState_Get(), callable, update);
    }
    false
}

#[cold]
unsafe fn execute_unary_op_kind_owned(
    kind: UnaryOpKind,
    operand: ObjPtr,
) -> Result<ObjPtr, String> {
    let operand = operand.cast::<ffi::PyObject>();
    let result = unsafe {
        match kind {
            UnaryOpKind::Pos => ffi::PyNumber_Positive(operand),
            UnaryOpKind::Neg => ffi::PyNumber_Negative(operand),
            UnaryOpKind::Invert => ffi::PyNumber_Invert(operand),
            UnaryOpKind::Not | UnaryOpKind::Truth => {
                let truth = ffi::PyObject_IsTrue(operand);
                if truth < 0 {
                    ptr::null_mut()
                } else {
                    let bool_value = if kind == UnaryOpKind::Not {
                        truth == 0
                    } else {
                        truth != 0
                    };
                    ffi::PyBool_FromLong(bool_value as libc::c_long)
                }
            }
        }
    };
    Ok(result.cast())
}

#[cold]
unsafe fn execute_binop_kind_owned(
    kind: BinOpKind,
    left: ObjPtr,
    right: ObjPtr,
) -> Result<ObjPtr, String> {
    let left = left.cast::<ffi::PyObject>();
    let right = right.cast::<ffi::PyObject>();
    let result = unsafe {
        match kind {
            BinOpKind::Add => ffi::PyNumber_Add(left, right),
            BinOpKind::Sub => ffi::PyNumber_Subtract(left, right),
            BinOpKind::Mul => ffi::PyNumber_Multiply(left, right),
            BinOpKind::MatMul => ffi::PyNumber_MatrixMultiply(left, right),
            BinOpKind::TrueDiv => ffi::PyNumber_TrueDivide(left, right),
            BinOpKind::FloorDiv => ffi::PyNumber_FloorDivide(left, right),
            BinOpKind::Mod => ffi::PyNumber_Remainder(left, right),
            BinOpKind::Pow => ffi::PyNumber_Power(left, right, ffi::Py_None()),
            BinOpKind::LShift => ffi::PyNumber_Lshift(left, right),
            BinOpKind::RShift => ffi::PyNumber_Rshift(left, right),
            BinOpKind::Or => ffi::PyNumber_Or(left, right),
            BinOpKind::Xor => ffi::PyNumber_Xor(left, right),
            BinOpKind::And => ffi::PyNumber_And(left, right),
            BinOpKind::Eq => ffi::PyObject_RichCompare(left, right, ffi::Py_EQ),
            BinOpKind::Ne => ffi::PyObject_RichCompare(left, right, ffi::Py_NE),
            BinOpKind::Lt => ffi::PyObject_RichCompare(left, right, ffi::Py_LT),
            BinOpKind::Le => ffi::PyObject_RichCompare(left, right, ffi::Py_LE),
            BinOpKind::Gt => ffi::PyObject_RichCompare(left, right, ffi::Py_GT),
            BinOpKind::Ge => ffi::PyObject_RichCompare(left, right, ffi::Py_GE),
            BinOpKind::Contains => {
                let contains = ffi::PySequence_Contains(right, left);
                if contains < 0 {
                    ptr::null_mut()
                } else {
                    ffi::PyBool_FromLong((contains != 0) as libc::c_long)
                }
            }
            BinOpKind::Is => ffi::PyBool_FromLong((left == right) as libc::c_long),
            BinOpKind::InplaceAdd => ffi::PyNumber_InPlaceAdd(left, right),
            BinOpKind::InplaceSub => ffi::PyNumber_InPlaceSubtract(left, right),
            BinOpKind::InplaceMul => ffi::PyNumber_InPlaceMultiply(left, right),
            BinOpKind::InplaceMatMul => ffi::PyNumber_InPlaceMatrixMultiply(left, right),
            BinOpKind::InplaceTrueDiv => ffi::PyNumber_InPlaceTrueDivide(left, right),
            BinOpKind::InplaceFloorDiv => ffi::PyNumber_InPlaceFloorDivide(left, right),
            BinOpKind::InplaceMod => ffi::PyNumber_InPlaceRemainder(left, right),
            BinOpKind::InplacePow => ffi::PyNumber_InPlacePower(left, right, ffi::Py_None()),
            BinOpKind::InplaceLShift => ffi::PyNumber_InPlaceLshift(left, right),
            BinOpKind::InplaceRShift => ffi::PyNumber_InPlaceRshift(left, right),
            BinOpKind::InplaceOr => ffi::PyNumber_InPlaceOr(left, right),
            BinOpKind::InplaceXor => ffi::PyNumber_InPlaceXor(left, right),
            BinOpKind::InplaceAnd => ffi::PyNumber_InPlaceAnd(left, right),
        }
    };
    Ok(result.cast())
}

#[cold]
fn set_deopt_unbound_local_error(name: &str) {
    let message =
        format!("cannot access local variable {name:?} where it is not associated with a value");
    if let Ok(c_message) = std::ffi::CString::new(message) {
        unsafe {
            ffi::PyErr_SetString(ffi::PyExc_UnboundLocalError, c_message.as_ptr());
        }
    } else {
        unsafe {
            ffi::PyErr_SetString(
                ffi::PyExc_UnboundLocalError,
                b"cannot access local variable before assignment\0"
                    .as_ptr()
                    .cast(),
            );
        }
    }
}
