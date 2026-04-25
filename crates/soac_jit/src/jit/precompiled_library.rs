use super::*;
use std::ffi::{CStr, CString, c_void};
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

static PRECOMPILED_LIBRARY: OnceLock<Result<Option<PrecompiledLibrary>, String>> = OnceLock::new();

fn precompiled_library() -> Result<Option<&'static PrecompiledLibrary>, String> {
    match PRECOMPILED_LIBRARY.get_or_init(load_precompiled_library_from_env) {
        Ok(Some(library)) => Ok(Some(library)),
        Ok(None) => Ok(None),
        Err(error) => Err(error.clone()),
    }
}

fn load_precompiled_library_from_env() -> Result<Option<PrecompiledLibrary>, String> {
    let Some(path) = crate::config::precompiled_library_path()? else {
        return Ok(None);
    };
    promote_current_soac_extension_symbols_for_precompiled_library()?;
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        format!(
            "SOAC_PRECOMPILED_LIBRARY contains an interior NUL byte: {}",
            path.display()
        )
    })?;
    let handle = unsafe {
        libc::dlerror();
        libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL)
    };
    if handle.is_null() {
        return Err(format!(
            "failed to load SOAC_PRECOMPILED_LIBRARY {}: {}",
            path.display(),
            take_dlerror()
        ));
    }
    info!(
        target: "soac_jit_precompiled",
        event = "soac.precompiled_library_load",
        path = %path.display(),
        "soac_precompiled_library_load",
    );
    Ok(Some(PrecompiledLibrary {
        handle: handle as usize,
        path,
    }))
}

fn promote_current_soac_extension_symbols_for_precompiled_library() -> Result<(), String> {
    let mut info = MaybeUninit::<libc::Dl_info>::zeroed();
    let ok = unsafe {
        libc::dladdr(
            specialized_helpers::dp_jit_load_runtime_obj as *const c_void,
            info.as_mut_ptr(),
        )
    };
    if ok == 0 {
        return Err(
            "failed to locate the loaded _soac_ext shared object for SOAC_PRECOMPILED_LIBRARY"
                .to_string(),
        );
    }
    let info = unsafe { info.assume_init() };
    if info.dli_fname.is_null() {
        return Err(
            "dynamic loader did not report a path for the loaded _soac_ext shared object"
                .to_string(),
        );
    }

    let handle = unsafe {
        libc::dlerror();
        libc::dlopen(
            info.dli_fname,
            libc::RTLD_NOW | libc::RTLD_GLOBAL | libc::RTLD_NOLOAD,
        )
    };
    if !handle.is_null() {
        return Ok(());
    }
    let no_load_error = take_dlerror();

    let handle = unsafe {
        libc::dlerror();
        libc::dlopen(info.dli_fname, libc::RTLD_NOW | libc::RTLD_GLOBAL)
    };
    if handle.is_null() {
        return Err(format!(
            "failed to promote _soac_ext symbols for SOAC_PRECOMPILED_LIBRARY (RTLD_NOLOAD error: {}; reopen error: {})",
            no_load_error,
            take_dlerror()
        ));
    }
    Ok(())
}

fn take_dlerror() -> String {
    let error = unsafe { libc::dlerror() };
    if error.is_null() {
        return "unknown dynamic loader error".to_string();
    }
    unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}

#[derive(Debug)]
struct PrecompiledLibrary {
    handle: usize,
    path: PathBuf,
}

pub(crate) struct PrecompiledModuleRuntime {
    deopt_resume_plan: PlannedJitDeoptResumeModule,
    module_constant_ptrs: Vec<usize>,
}

impl PrecompiledModuleRuntime {
    fn module_constant_ptrs(&self) -> Vec<*mut ffi::PyObject> {
        self.module_constant_ptrs
            .iter()
            .map(|ptr| *ptr as *mut ffi::PyObject)
            .collect()
    }
}

impl PrecompiledLibrary {
    fn lookup_code_symbol(&self, symbol: &str) -> Result<Option<*const u8>, String> {
        let c_symbol = CString::new(symbol)
            .map_err(|_| format!("precompiled symbol contains an interior NUL byte: {symbol:?}"))?;
        let ptr = unsafe {
            libc::dlerror();
            libc::dlsym(self.handle as *mut c_void, c_symbol.as_ptr())
        };
        if ptr.is_null() {
            let error = unsafe { libc::dlerror() };
            if error.is_null() {
                return Ok(None);
            }
            let error = unsafe { CStr::from_ptr(error) }.to_string_lossy();
            if error.contains("undefined symbol:") {
                return Ok(None);
            }
            return Err(format!(
                "failed to look up precompiled symbol {symbol:?} in {}: {}",
                self.path.display(),
                error
            ));
        }
        Ok(Some(ptr.cast::<u8>() as *const u8))
    }

    fn lookup_module_constant_slot(
        &self,
        symbol: &str,
    ) -> Result<Option<*mut *mut ffi::PyObject>, String> {
        self.lookup_symbol(symbol)
            .map(|ptr| ptr.map(|ptr| ptr.cast::<*mut ffi::PyObject>()))
    }

    fn lookup_symbol(&self, symbol: &str) -> Result<Option<*mut c_void>, String> {
        let c_symbol = CString::new(symbol)
            .map_err(|_| format!("precompiled symbol contains an interior NUL byte: {symbol:?}"))?;
        let ptr = unsafe {
            libc::dlerror();
            libc::dlsym(self.handle as *mut c_void, c_symbol.as_ptr())
        };
        if ptr.is_null() {
            let error = unsafe { libc::dlerror() };
            if error.is_null() {
                return Ok(None);
            }
            return Err(format!(
                "failed to look up precompiled symbol {symbol:?} in {}: {}",
                self.path.display(),
                unsafe { CStr::from_ptr(error) }.to_string_lossy()
            ));
        }
        Ok(Some(ptr))
    }
}

pub(crate) fn lookup_precompiled_direct_function_handle(
    session: &Arc<crate::session::CompileSession>,
    shared_state: &SharedModuleState,
    function: &BlockPyFunction<CodegenModuleShape>,
) -> Result<Option<Arc<CompiledFunctionHandle>>, String> {
    let Some(library) = precompiled_library()? else {
        return Ok(None);
    };
    if shared_state.source_hash() == 0 {
        return Ok(None);
    }

    let symbol_scope = precompiled_direct_function_symbol_scope_for_shared_state(
        shared_state,
        function.function_id,
    );
    let symbol = direct_function_symbol(function, Some(symbol_scope.as_str()));
    let Some(code_ptr) = library.lookup_code_symbol(symbol.as_str())? else {
        return Ok(None);
    };

    let default_code_ptr = if function_has_default_resolving_direct_entry(function) {
        let default_symbol = default_direct_function_symbol(function, Some(symbol_scope.as_str()));
        library
            .lookup_code_symbol(default_symbol.as_str())?
            .ok_or_else(|| {
                format!(
                    "precompiled library {} has direct entry {symbol:?} but is missing default entry {default_symbol:?}",
                    library.path.display()
                )
            })?
    } else {
        code_ptr
    };

    let runtime = precompiled_module_runtime(library, shared_state)?;
    let function_deopt_resume_plan = runtime
        .deopt_resume_plan
        .function(function.function_id)
        .ok_or_else(|| {
            format!(
                "missing JIT deopt resume plan for precompiled function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
    let module_constant_ptrs = runtime.module_constant_ptrs();
    let deopt_table = Arc::new(RuntimeJitDeoptTable::from_plan(
        function,
        function_deopt_resume_plan,
        module_constant_ptrs.as_slice(),
    )?);
    info!(
        target: "soac_jit_precompiled",
        event = "soac.precompiled_direct_function_hit",
        module = shared_state.module_name.as_str(),
        source_hash = format_args!("0x{:016x}", shared_state.source_hash()),
        function_id = function.function_id.local_function_id().as_u32(),
        qualname = function.names.qualname.as_str(),
        symbol = symbol.as_str(),
        "soac_precompiled_direct_function_hit",
    );
    Ok(Some(Arc::new(CompiledFunctionHandle::from_direct_entry(
        session,
        code_ptr,
        default_code_ptr,
        function.params.len(),
        deopt_table,
        None,
    ))))
}

pub(crate) fn lookup_precompiled_static_module_constant(
    module_name: &str,
    source_hash: u64,
    constant_id: ModuleConstantId,
) -> Result<Option<*mut ffi::PyObject>, String> {
    let Some(library) = precompiled_library()? else {
        return Ok(None);
    };
    if source_hash == 0 {
        return Ok(None);
    }
    let symbol_prefix = module_constant_symbol_prefix_for_module_identity(module_name, source_hash);
    let symbol = module_constant_object_symbol(symbol_prefix.as_str(), constant_id);
    library
        .lookup_code_symbol(symbol.as_str())
        .map(|ptr| ptr.map(|ptr| ptr.cast_mut().cast::<ffi::PyObject>()))
}

fn precompiled_module_runtime(
    library: &PrecompiledLibrary,
    shared_state: &SharedModuleState,
) -> Result<Arc<PrecompiledModuleRuntime>, String> {
    match shared_state
        .precompiled_module_runtime
        .get_or_init(|| build_precompiled_module_runtime(library, shared_state))
    {
        Ok(runtime) => Ok(Arc::clone(runtime)),
        Err(error) => Err(error.clone()),
    }
}

fn build_precompiled_module_runtime(
    library: &PrecompiledLibrary,
    shared_state: &SharedModuleState,
) -> Result<Arc<PrecompiledModuleRuntime>, String> {
    patch_precompiled_module_constant_slots(library, shared_state)?;
    let value_facts = infer_jit_value_facts(&shared_state.lowered_module);
    let deopt_resume_plan =
        plan_jit_module_from_codegen(&shared_state.lowered_module, value_facts)?.deopt_resume;
    let module_constant_ptrs = shared_state
        .module_constant_ptrs()
        .into_iter()
        .map(|ptr| ptr as usize)
        .collect();
    Ok(Arc::new(PrecompiledModuleRuntime {
        deopt_resume_plan,
        module_constant_ptrs,
    }))
}

fn patch_precompiled_module_constant_slots(
    library: &PrecompiledLibrary,
    shared_state: &SharedModuleState,
) -> Result<(), String> {
    let symbol_prefix = module_constant_symbol_prefix_for_shared_state(shared_state);
    for (index, ptr) in shared_state.module_constant_ptrs().into_iter().enumerate() {
        let constant_id = ModuleConstantId(index);
        if shared_state
            .codegen_constants
            .static_pyobject_image(constant_id)
            .is_some()
        {
            continue;
        }
        let symbol = module_constant_object_symbol(symbol_prefix.as_str(), constant_id);
        let Some(slot) = library.lookup_module_constant_slot(symbol.as_str())? else {
            return Err(format!(
                "precompiled library {} is missing module constant slot {symbol:?}",
                library.path.display()
            ));
        };
        unsafe { *slot = ptr };
    }
    Ok(())
}
