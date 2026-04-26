use super::deopt::RuntimeJitDeoptTable;
use super::specialized_helpers::ObjPtr;
use std::sync::Arc;

struct CompiledSpecializedRunner {
    _session: Arc<crate::session::CompileSession>,
    entry: Option<CompiledRunnerEntry>,
    direct_deopt_table: Option<Arc<RuntimeJitDeoptTable>>,
}

pub(crate) struct CompiledFunctionHandle {
    handle: ObjPtr,
    stats: Option<JitCodegenStats>,
}

pub(crate) struct DirectFunctionCompileResult {
    pub(crate) handle: Arc<CompiledFunctionHandle>,
    pub(crate) compiled: bool,
    pub(crate) stats: Option<JitCodegenStats>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct JitCodegenStats {
    pub(crate) clif_block_count: usize,
    pub(crate) clif_inst_count: usize,
    pub(crate) machine_code_size_bytes: usize,
    pub(crate) machine_code_block_count: usize,
    pub(crate) machine_code_edge_count: usize,
}

// The handle points to an immutable compiled runner after construction. The code memory is kept
// alive by the runner owner, and the raw handle is freed only when the final Arc drops this wrapper.
unsafe impl Send for CompiledFunctionHandle {}
unsafe impl Sync for CompiledFunctionHandle {}

impl CompiledFunctionHandle {
    pub(super) fn from_direct_entry(
        session: &Arc<crate::session::CompileSession>,
        code_ptr: *const u8,
        default_code_ptr: *const u8,
        param_count: usize,
        deopt_table: Arc<RuntimeJitDeoptTable>,
        stats: Option<JitCodegenStats>,
    ) -> Self {
        Self {
            handle: new_compiled_direct_runner_handle(
                session,
                code_ptr,
                default_code_ptr,
                param_count,
                deopt_table,
            ),
            stats,
        }
    }

    pub(crate) fn jit_stats(&self) -> Option<&JitCodegenStats> {
        self.stats.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn direct_runner_info(&self) -> Result<(*const u8, *const u8, usize), String> {
        compiled_direct_runner_info(self.handle)
    }

    pub(crate) fn direct_code_ptr(&self) -> Result<ObjPtr, String> {
        compiled_direct_code_ptr(self.handle)
    }

    pub(crate) fn default_direct_code_ptr(&self) -> Result<ObjPtr, String> {
        compiled_default_direct_code_ptr(self.handle)
    }

    pub(crate) fn direct_deopt_table_ptr(&self) -> Result<ObjPtr, String> {
        compiled_direct_deopt_table_ptr(self.handle)
    }
}

impl Drop for CompiledFunctionHandle {
    fn drop(&mut self) {
        unsafe { free_cranelift_run_bb_specialized_cached(self.handle) };
        self.handle = std::ptr::null_mut();
    }
}

pub(crate) type VectorcallEntryFn =
    unsafe extern "C" fn(ObjPtr, *const ObjPtr, usize, ObjPtr) -> ObjPtr;

#[derive(Clone, Copy)]
enum CompiledRunnerEntry {
    Direct {
        code_ptr: *const u8,
        default_code_ptr: *const u8,
        param_count: usize,
    },
}

fn new_compiled_direct_runner_handle(
    session: &Arc<crate::session::CompileSession>,
    code_ptr: *const u8,
    default_code_ptr: *const u8,
    param_count: usize,
    deopt_table: Arc<RuntimeJitDeoptTable>,
) -> ObjPtr {
    Box::into_raw(Box::new(CompiledSpecializedRunner {
        _session: Arc::clone(session),
        entry: Some(CompiledRunnerEntry::Direct {
            code_ptr,
            default_code_ptr,
            param_count,
        }),
        direct_deopt_table: Some(deopt_table),
    })) as ObjPtr
}

fn compiled_direct_runner_info(
    compiled_handle: ObjPtr,
) -> Result<(*const u8, *const u8, usize), String> {
    if compiled_handle.is_null() {
        return Err("invalid null compiled handle for direct vectorcall trampoline".to_string());
    }
    let compiled = unsafe { &*(compiled_handle as *const CompiledSpecializedRunner) };
    debug_assert!(
        compiled.direct_deopt_table.is_some(),
        "compiled direct handle should carry a deopt table"
    );
    match compiled.entry {
        Some(CompiledRunnerEntry::Direct {
            code_ptr,
            default_code_ptr,
            param_count,
        }) => Ok((code_ptr, default_code_ptr, param_count)),
        None => Err("invalid compiled handle without entrypoint".to_string()),
    }
}

fn compiled_direct_code_ptr(compiled_handle: ObjPtr) -> Result<ObjPtr, String> {
    compiled_direct_runner_info(compiled_handle).map(|(code_ptr, _, _)| code_ptr as ObjPtr)
}

fn compiled_default_direct_code_ptr(compiled_handle: ObjPtr) -> Result<ObjPtr, String> {
    compiled_direct_runner_info(compiled_handle)
        .map(|(_, default_code_ptr, _)| default_code_ptr as ObjPtr)
}

fn compiled_direct_deopt_table_ptr(compiled_handle: ObjPtr) -> Result<ObjPtr, String> {
    if compiled_handle.is_null() {
        return Err("invalid null compiled handle for direct deopt table pointer".to_string());
    }
    let compiled = unsafe { &*(compiled_handle as *const CompiledSpecializedRunner) };
    compiled
        .direct_deopt_table
        .as_ref()
        .map(|table| Arc::as_ptr(table) as ObjPtr)
        .ok_or_else(|| "compiled direct handle does not carry a deopt table".to_string())
}

unsafe fn free_cranelift_run_bb_specialized_cached(compiled_handle: ObjPtr) {
    if compiled_handle.is_null() {
        return;
    }
    let _ = Box::from_raw(compiled_handle as *mut CompiledSpecializedRunner);
}
