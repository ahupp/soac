use super::*;
use soac_blockpy::block_py::{
    BinOp, BinOpKind, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm, Call,
    CallArgPositional, CellLocation, ClosureInit, ClosureSlot, CodegenBlock, CounterSite, Del,
    DelItem, FunctionId, FunctionName, InstrCodegen, InstrResolved, Literal, LiteralValue, Load,
    Meta, ModuleNameGen, NameLocation, NumberLiteral, NumberLiteralValue, Param, ParamKind,
    ParamSpec, ResolvedName, StorageLayout, Store, StringLiteral, WithMeta,
};
use soac_blockpy::passes::{
    CodegenBlockPyPass, instrument_bb_module_with_block_entry_counters,
    instrument_bb_module_with_refcount_counters,
};
mod tests {
    use super::*;
    use pyo3::types::PyAnyMethods;
    use pyo3::{Python, ffi};
    use ruff_python_ast as ast;
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static CAPSULE_DESTROYED: AtomicBool = AtomicBool::new(false);
    static NEXT_TEST_EXT_STAGING_ID: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn test_capsule_destructor(_capsule: *mut ffi::PyObject) {
        CAPSULE_DESTROYED.store(true, Ordering::SeqCst);
    }

    fn test_name(name: &str) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::local(0),
        }
    }

    fn test_global_name(name: &str) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::global(0),
        }
    }

    fn test_runtime_name(name: &str) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::RuntimeName,
        }
    }

    fn test_closure_cell_name(name: &str, slot: u32) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::closure_cell(slot),
        }
    }

    fn test_constant_name(index: u32) -> ResolvedName {
        ResolvedName {
            id: "__dp_constant".into(),
            location: NameLocation::Constant(index),
        }
    }

    fn int_literal(value: i64) -> InstrResolved {
        let value_str = value.to_string();
        let literal = Literal::NumberLiteral(NumberLiteral {
            value: NumberLiteralValue::Int(
                ast::Int::from_str_radix(value_str.as_str(), 10, value_str.as_str())
                    .expect("test integer literal should parse"),
            ),
        });
        InstrResolved::Literal(LiteralValue::new(literal))
    }

    fn string_literal(value: &str) -> InstrResolved {
        let literal = Literal::StringLiteral(StringLiteral {
            value: value.to_string(),
        });
        InstrResolved::Literal(LiteralValue::new(literal))
    }

    fn none_expr() -> InstrCodegen {
        Load::new(test_runtime_name("NONE")).into()
    }

    #[derive(Default)]
    struct TestConstantPool {
        module_constants: Vec<InstrResolved>,
    }

    impl TestConstantPool {
        fn push_literal(&mut self, literal: InstrResolved) -> InstrCodegen {
            let index = u32::try_from(self.module_constants.len())
                .expect("test module constant count should fit in u32");
            self.module_constants.push(literal);
            Load::new(test_constant_name(index)).into()
        }

        fn int_expr(&mut self, value: i64) -> InstrCodegen {
            self.push_literal(int_literal(value))
        }

        fn string_expr(&mut self, value: &str) -> InstrCodegen {
            self.push_literal(string_literal(value))
        }
    }

    fn name_expr(name: ResolvedName) -> InstrCodegen {
        Load::new(name).into()
    }

    fn op_expr(operation: impl Into<InstrCodegen>) -> InstrCodegen {
        operation.into()
    }

    fn expr_stmt(expr: InstrCodegen) -> InstrCodegen {
        expr
    }

    fn with_instr_id(expr: InstrCodegen, instr_id: InstrId) -> InstrCodegen {
        expr.with_meta(Meta {
            instr_id: Some(instr_id),
            ..Meta::synthetic()
        })
    }

    fn repo_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace crate should have a repo-root parent")
    }

    fn ensure_test_extension_staging_dir() -> PathBuf {
        let staging_dir = repo_root().join("target").join("debug").join(format!(
            "test-ext-{}",
            NEXT_TEST_EXT_STAGING_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let source_ext = repo_root()
            .join("target")
            .join("debug")
            .join("lib_soac_ext.so");
        let staged_ext = staging_dir.join("_soac_ext.so");
        std::fs::create_dir_all(&staging_dir).expect("test extension staging dir should exist");
        if staged_ext.exists() {
            std::fs::remove_file(&staged_ext).expect("stale staged _soac_ext should be removable");
        }
        std::os::unix::fs::symlink(&source_ext, &staged_ext)
            .expect("staged _soac_ext symlink should be creatable");
        staging_dir
    }

    fn assign_stmt(target: ResolvedName, value: InstrCodegen) -> InstrCodegen {
        expr_stmt(op_expr(Store::new(target, value)))
    }

    fn delete_stmt(target: ResolvedName) -> InstrCodegen {
        expr_stmt(op_expr(Del::new(target, false)))
    }

    fn ret_term(value: InstrCodegen) -> BlockTerm<InstrCodegen> {
        BlockTerm::Return(value)
    }

    fn raise_term() -> BlockTerm<InstrCodegen> {
        BlockTerm::Raise(soac_blockpy::block_py::TermRaise { exc: None })
    }

    fn test_source_block(
        function: &BlockPyFunction<CodegenBlockPyPass>,
        ops: Vec<InstrCodegen>,
        term: BlockTerm<InstrCodegen>,
    ) -> CodegenBlock {
        CodegenBlock {
            label: function.name_gen.next_block_name(),
            body: ops,
            term,
            params: vec![],
            exc_edge: None,
        }
    }

    fn test_function() -> BlockPyFunction<CodegenBlockPyPass> {
        let module_name_gen = ModuleNameGen::new(0);
        let name_gen = module_name_gen.next_function_name_gen();
        BlockPyFunction {
            function_id: name_gen.function_id(),
            name_gen,
            names: FunctionName::new("test", "test", "test", "test"),
            kind: soac_blockpy::block_py::FunctionKind::Function,
            params: ParamSpec::default(),
            blocks: vec![],
            doc: None,
            storage_layout: None,
            scope: Default::default(),
        }
    }

    fn with_test_blocks(
        mut function: BlockPyFunction<CodegenBlockPyPass>,
        blocks: Vec<CodegenBlock>,
    ) -> BlockPyFunction<CodegenBlockPyPass> {
        function.blocks = blocks;
        function
    }

    fn set_stack_slots(function: &mut BlockPyFunction<CodegenBlockPyPass>, names: &[&str]) {
        function
            .storage_layout
            .get_or_insert_with(StorageLayout::default)
            .set_stack_slots(names.iter().map(|name| (*name).to_string()).collect());
    }

    fn with_single_test_block(
        function: BlockPyFunction<CodegenBlockPyPass>,
        ops: Vec<InstrCodegen>,
        term: BlockTerm<InstrCodegen>,
    ) -> BlockPyFunction<CodegenBlockPyPass> {
        let block = test_source_block(&function, ops, term);
        with_test_blocks(function, vec![block])
    }

    fn render_test_jit_function(
        function: &BlockPyFunction<CodegenBlockPyPass>,
        blocks: &[ObjPtr],
    ) -> String {
        render_test_jit_function_with_module_constants(function, blocks, Vec::new())
    }

    fn render_test_jit_function_with_module_constants(
        function: &BlockPyFunction<CodegenBlockPyPass>,
        blocks: &[ObjPtr],
        module_constants: Vec<InstrResolved>,
    ) -> String {
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants,
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        render_test_jit_function_with_constants(&module, &function, blocks, &module_constants)
    }

    fn render_test_jit_function_with_call_target_specializations(
        module: &BlockPyModule<CodegenBlockPyPass>,
        function: &BlockPyFunction<CodegenBlockPyPass>,
        blocks: &[ObjPtr],
        specializations: &[(InstrId, FunctionId)],
    ) -> String {
        let module_name = "counter_test";
        let function_id = function.function_id.packed();
        let specialization_value = specializations
            .iter()
            .map(|(instr_id, target_function_id)| {
                format!(
                    "{module_name}|{function_id}|{}|{}={}",
                    instr_id.block_label().as_u32(),
                    instr_id.instr_index_in_block(),
                    target_function_id.packed(),
                )
            })
            .collect::<Vec<_>>()
            .join(";");

        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let old_call_target_specializations =
            std::env::var_os("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS");
        let old_operator_specializations = std::env::var_os("DIET_PYTHON_OPERATOR_SPECIALIZATIONS");
        let old_counters_file = std::env::var_os("DIET_PYTHON_COUNTERS_FILE");
        let old_call_target_counters = std::env::var_os("DIET_PYTHON_CALL_TARGET_COUNTERS");
        let old_pythonhome = std::env::var_os("PYTHONHOME");
        let old_pythonpath = std::env::var_os("PYTHONPATH");
        let python_home = vendored_python_home();
        let python_path = std::env::join_paths([
            python_home.join("Lib"),
            vendored_python_build_lib_dir(),
            repo_root().join("soac_py").join("src"),
        ])
        .expect("test PYTHONPATH should join");
        unsafe {
            std::env::set_var(
                "DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS",
                specialization_value.as_str(),
            );
            std::env::remove_var("DIET_PYTHON_OPERATOR_SPECIALIZATIONS");
            std::env::remove_var("DIET_PYTHON_COUNTERS_FILE");
            std::env::remove_var("DIET_PYTHON_CALL_TARGET_COUNTERS");
            std::env::set_var("PYTHONHOME", &python_home);
            std::env::set_var("PYTHONPATH", python_path);
        }

        let rendered = unsafe {
            Python::initialize();
            Python::attach(|py| {
                let shared_state = crate::module_type::build_shared_state_for_testing(
                    py,
                    module.clone(),
                    module_name,
                    "",
                )
                .expect("shared state should build");
                let mut jit_module = new_jit_module().expect("test jit module should construct");
                let module_constant_ptrs = shared_state.module_constant_ptrs();
                let counter_ptrs = shared_state.counter_ptrs();
                let built = build_cranelift_run_bb_specialized_function(
                    &mut jit_module,
                    blocks,
                    &shared_state.lowered_module,
                    function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    &counter_ptrs,
                    Some(shared_state.as_ref()),
                )
                .expect("specialized JIT build should succeed");
                let (clif, _cfg_dot, _vcode_disasm) = render_compiled_clif_and_vcode_disasm(
                    &mut jit_module,
                    built.ctx,
                    &built.import_id_to_symbol,
                    &built.block_annotations,
                )
                .expect("specialized JIT CLIF render should succeed");
                clif
            })
        };

        unsafe {
            match old_call_target_specializations {
                Some(value) => std::env::set_var("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS", value),
                None => std::env::remove_var("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS"),
            }
            match old_operator_specializations {
                Some(value) => std::env::set_var("DIET_PYTHON_OPERATOR_SPECIALIZATIONS", value),
                None => std::env::remove_var("DIET_PYTHON_OPERATOR_SPECIALIZATIONS"),
            }
            match old_counters_file {
                Some(value) => std::env::set_var("DIET_PYTHON_COUNTERS_FILE", value),
                None => std::env::remove_var("DIET_PYTHON_COUNTERS_FILE"),
            }
            match old_call_target_counters {
                Some(value) => std::env::set_var("DIET_PYTHON_CALL_TARGET_COUNTERS", value),
                None => std::env::remove_var("DIET_PYTHON_CALL_TARGET_COUNTERS"),
            }
            match old_pythonhome {
                Some(value) => std::env::set_var("PYTHONHOME", value),
                None => std::env::remove_var("PYTHONHOME"),
            }
            match old_pythonpath {
                Some(value) => std::env::set_var("PYTHONPATH", value),
                None => std::env::remove_var("PYTHONPATH"),
            }
        }

        rendered
    }

    fn render_test_jit_function_with_operator_specializations(
        function: &BlockPyFunction<CodegenBlockPyPass>,
        blocks: &[ObjPtr],
        module_constants: Vec<InstrResolved>,
        operator_specializations: &[(InstrId, u64)],
    ) -> String {
        let module_name = "counter_test";
        let function_id = function.function_id.packed();
        let specialization_value = operator_specializations
            .iter()
            .map(|(instr_id, shape)| {
                format!(
                    "{module_name}|{function_id}|{}|{}={shape}",
                    instr_id.block_label().as_u32(),
                    instr_id.instr_index_in_block(),
                )
            })
            .collect::<Vec<_>>()
            .join(";");

        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let old_operator_specializations = std::env::var_os("DIET_PYTHON_OPERATOR_SPECIALIZATIONS");
        let old_call_target_specializations =
            std::env::var_os("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS");
        let old_counters_file = std::env::var_os("DIET_PYTHON_COUNTERS_FILE");
        let old_call_target_counters = std::env::var_os("DIET_PYTHON_CALL_TARGET_COUNTERS");
        let old_pythonhome = std::env::var_os("PYTHONHOME");
        let old_pythonpath = std::env::var_os("PYTHONPATH");
        let python_home = vendored_python_home();
        let python_path = std::env::join_paths([
            python_home.join("Lib"),
            vendored_python_build_lib_dir(),
            repo_root().join("soac_py").join("src"),
        ])
        .expect("test PYTHONPATH should join");
        unsafe {
            std::env::set_var(
                "DIET_PYTHON_OPERATOR_SPECIALIZATIONS",
                specialization_value.as_str(),
            );
            std::env::remove_var("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS");
            std::env::remove_var("DIET_PYTHON_COUNTERS_FILE");
            std::env::remove_var("DIET_PYTHON_CALL_TARGET_COUNTERS");
            std::env::set_var("PYTHONHOME", &python_home);
            std::env::set_var("PYTHONPATH", python_path);
        }

        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants,
            counter_defs: Vec::new(),
        };

        let rendered = unsafe {
            Python::initialize();
            Python::attach(|py| {
                let shared_state =
                    crate::module_type::build_shared_state_for_testing(py, module, module_name, "")
                        .expect("shared state should build");
                let mut jit_module = new_jit_module().expect("test jit module should construct");
                let module_constant_ptrs = shared_state.module_constant_ptrs();
                let counter_ptrs = shared_state.counter_ptrs();
                let built = build_cranelift_run_bb_specialized_function(
                    &mut jit_module,
                    blocks,
                    &shared_state.lowered_module,
                    function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    &counter_ptrs,
                    Some(shared_state.as_ref()),
                )
                .expect("specialized JIT build should succeed");
                let (clif, _cfg_dot, _vcode_disasm) = render_compiled_clif_and_vcode_disasm(
                    &mut jit_module,
                    built.ctx,
                    &built.import_id_to_symbol,
                    &built.block_annotations,
                )
                .expect("specialized JIT CLIF render should succeed");
                clif
            })
        };

        unsafe {
            match old_operator_specializations {
                Some(value) => std::env::set_var("DIET_PYTHON_OPERATOR_SPECIALIZATIONS", value),
                None => std::env::remove_var("DIET_PYTHON_OPERATOR_SPECIALIZATIONS"),
            }
            match old_call_target_specializations {
                Some(value) => std::env::set_var("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS", value),
                None => std::env::remove_var("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS"),
            }
            match old_counters_file {
                Some(value) => std::env::set_var("DIET_PYTHON_COUNTERS_FILE", value),
                None => std::env::remove_var("DIET_PYTHON_COUNTERS_FILE"),
            }
            match old_call_target_counters {
                Some(value) => std::env::set_var("DIET_PYTHON_CALL_TARGET_COUNTERS", value),
                None => std::env::remove_var("DIET_PYTHON_CALL_TARGET_COUNTERS"),
            }
            match old_pythonhome {
                Some(value) => std::env::set_var("PYTHONHOME", value),
                None => std::env::remove_var("PYTHONHOME"),
            }
            match old_pythonpath {
                Some(value) => std::env::set_var("PYTHONPATH", value),
                None => std::env::remove_var("PYTHONPATH"),
            }
        }

        rendered
    }

    fn render_test_jit_function_with_constants(
        module: &BlockPyModule<CodegenBlockPyPass>,
        function: &BlockPyFunction<CodegenBlockPyPass>,
        blocks: &[ObjPtr],
        module_constants: &crate::module_constants::ModuleCodegenConstants,
    ) -> String {
        unsafe {
            let mut jit_module = new_jit_module().expect("test jit module should construct");
            let module_constant_ptrs = placeholder_module_constant_ptrs(module_constants.len());
            let counter_ptrs = placeholder_counter_ptrs(
                function
                    .blocks
                    .iter()
                    .flat_map(|block| block.body.iter())
                    .filter_map(|expr| match expr {
                        InstrCodegen::IncrementCounter(op) => Some(op.counter_id.0),
                        _ => None,
                    })
                    .max()
                    .map_or(0, |max_counter_id| max_counter_id + 1),
            );
            let built = build_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks,
                module,
                function,
                module_constants,
                &[],
                &module_constant_ptrs,
                &counter_ptrs,
                None,
            )
            .expect("specialized JIT build should succeed");
            let (clif, _cfg_dot, _vcode_disasm) = render_compiled_clif_and_vcode_disasm(
                &mut jit_module,
                built.ctx,
                &built.import_id_to_symbol,
                &built.block_annotations,
            )
            .expect("specialized JIT CLIF render should succeed");
            clif
        }
    }

    fn vendored_python_home() -> std::path::PathBuf {
        repo_root().join("vendor").join("cpython")
    }

    fn vendored_python_build_lib_dir() -> PathBuf {
        let python_home = vendored_python_home();
        let rel_build_dir = std::fs::read_to_string(python_home.join("pybuilddir.txt"))
            .expect("vendored CPython pybuilddir.txt should exist");
        python_home.join(rel_build_dir.trim())
    }

    unsafe fn build_test_module_runtime(
        py: Python<'_>,
        shared_state: std::sync::Arc<crate::module_type::SharedModuleState>,
    ) -> crate::jit::ModuleRuntimeContext {
        let globals_obj = ffi::PyDict_New().cast::<c_void>();
        assert!(
            !globals_obj.is_null(),
            "PyDict_New should produce globals for test runtime"
        );
        let true_obj = ffi::PyBool_FromLong(1).cast::<c_void>();
        let false_obj = ffi::PyBool_FromLong(0).cast::<c_void>();
        let none_obj = py.None().as_ptr().cast::<c_void>();
        ffi::Py_INCREF(none_obj.cast());
        let deleted_obj = py.None().as_ptr().cast::<c_void>();
        ffi::Py_INCREF(deleted_obj.cast());
        let empty_tuple_obj = pyo3::types::PyTuple::empty(py).as_ptr().cast::<c_void>();
        ffi::Py_INCREF(empty_tuple_obj.cast());
        let global_cache = crate::module_globals::ModuleGlobalCache::new(
            globals_obj.cast(),
            shared_state.lowered_module.global_names.as_slice(),
        )
        .expect("test runtime should create module global cache");
        crate::jit::ModuleRuntimeContext {
            vmctx: crate::jit::JitModuleVmCtx {
                shared_module_state: std::sync::Arc::as_ptr(&shared_state),
                globals_obj,
                global_slots: global_cache.slots_ptr().cast::<c_void>(),
                true_obj,
                false_obj,
                none_obj,
                deleted_obj,
                empty_tuple_obj,
            },
            shared_module_state_owner: shared_state,
            global_cache_owner: global_cache,
        }
    }

    unsafe extern "C" fn test_bind_direct_args_stub(
        _callable: ObjPtr,
        _args: *const ObjPtr,
        _nargsf: usize,
        _kwnames: ObjPtr,
        _data_ptr: ObjPtr,
        _out_args: *mut ObjPtr,
        _out_len: i64,
    ) -> i32 {
        1
    }

    fn count_direct_calls_to_runtime_helpers(
        function: &ir::Function,
        helpers: &[ir::UserExternalName],
    ) -> usize {
        let mut count = 0usize;
        for block in function.layout.blocks() {
            for inst in function.layout.block_insts(block) {
                let callee = match function.dfg.insts[inst] {
                    ir::InstructionData::Call { func_ref, .. }
                    | ir::InstructionData::TryCall { func_ref, .. } => Some(func_ref),
                    _ => None,
                };
                let Some(callee) = callee else {
                    continue;
                };
                let ext_func = &function.dfg.ext_funcs[callee];
                let ir::ExternalName::User(name_ref) = &ext_func.name else {
                    continue;
                };
                let user_name = &function.params.user_named_funcs()[*name_ref];
                if helpers.contains(user_name) {
                    count += 1;
                }
            }
        }
        count
    }

    unsafe fn build_runtime_refcount_smoke_context() -> (
        JITModule,
        cranelift_codegen::Context,
        FuncId,
        [ir::UserExternalName; 2],
    ) {
        let mut jit_module = new_jit_module().expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));
        wrapper_signature.returns.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id = declare_local_fn(
            &mut jit_module,
            "jit_runtime_support_smoke_wrapper",
            &wrapper_signature,
        )
        .expect("wrapper function should declare");
        let incref_id = declare_local_fn(
            &mut jit_module,
            SOAC_RUNTIME_INCREF_SYMBOL,
            &refcount_signature,
        )
        .expect("runtime incref support function should be available");
        let decref_id = declare_local_fn(
            &mut jit_module,
            SOAC_RUNTIME_DECREF_SYMBOL,
            &refcount_signature,
        )
        .expect("runtime decref support function should be available");

        let mut ctx = jit_module.make_context();
        ctx.func.name = ir::UserFuncName::user(0, wrapper_id.as_u32());
        ctx.func.signature = wrapper_signature;

        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
            let entry = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.switch_to_block(entry);
            fb.seal_block(entry);

            let incref_ref = jit_module.declare_func_in_func(incref_id, &mut fb.func);
            let decref_ref = jit_module.declare_func_in_func(decref_id, &mut fb.func);
            let arg = fb.block_params(entry)[0];
            fb.ins().call(incref_ref, &[arg]);
            fb.ins().call(decref_ref, &[arg]);
            fb.ins().return_(&[arg]);
            fb.finalize();
        }

        (
            jit_module,
            ctx,
            wrapper_id,
            [
                ir::UserExternalName::new(0, incref_id.as_u32()),
                ir::UserExternalName::new(0, decref_id.as_u32()),
            ],
        )
    }

    unsafe fn build_runtime_refcount_smoke_wrapper()
    -> unsafe extern "C" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void {
        let (mut jit_module, mut ctx, wrapper_id, _) = build_runtime_refcount_smoke_context();

        define_function_with_incremental_cache(
            &mut jit_module,
            wrapper_id,
            &mut ctx,
            "test wrapper function should define",
        )
        .expect("wrapper function should compile");
        jit_module.clear_context(&mut ctx);
        jit_module
            .finalize_definitions()
            .expect("jit module should finalize");

        let code_ptr = jit_module.get_finalized_function(wrapper_id);
        let compiled: unsafe extern "C" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void =
            std::mem::transmute(code_ptr);
        Box::leak(Box::new(jit_module));
        compiled
    }

    unsafe fn build_runtime_decref_wrapper() -> unsafe extern "C" fn(*mut std::ffi::c_void) {
        let mut jit_module = new_jit_module().expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id = declare_local_fn(
            &mut jit_module,
            "jit_runtime_support_decref_wrapper",
            &wrapper_signature,
        )
        .expect("wrapper function should declare");
        let decref_id = declare_local_fn(
            &mut jit_module,
            SOAC_RUNTIME_DECREF_SYMBOL,
            &refcount_signature,
        )
        .expect("runtime decref support function should be available");

        let mut ctx = jit_module.make_context();
        ctx.func.name = ir::UserFuncName::user(0, wrapper_id.as_u32());
        ctx.func.signature = wrapper_signature;

        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
            let entry = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.switch_to_block(entry);
            fb.seal_block(entry);

            let decref_ref = jit_module.declare_func_in_func(decref_id, &mut fb.func);
            let arg = fb.block_params(entry)[0];
            fb.ins().call(decref_ref, &[arg]);
            fb.ins().return_(&[]);
            fb.finalize();
        }

        define_function_with_incremental_cache(
            &mut jit_module,
            wrapper_id,
            &mut ctx,
            "test wrapper function should define",
        )
        .expect("wrapper function should compile");
        jit_module.clear_context(&mut ctx);
        jit_module
            .finalize_definitions()
            .expect("jit module should finalize");

        let code_ptr = jit_module.get_finalized_function(wrapper_id);
        let compiled: unsafe extern "C" fn(*mut std::ffi::c_void) = std::mem::transmute(code_ptr);
        Box::leak(Box::new(jit_module));
        compiled
    }

    #[test]
    fn jit_can_call_runtime_support_clif_function() {
        unsafe {
            let wrapper = build_runtime_refcount_smoke_wrapper();
            let result = wrapper(std::ptr::null_mut());
            assert!(
                result.is_null(),
                "runtime incref/decref smoke wrapper should preserve the null pointer"
            );
        }
    }

    #[test]
    fn jit_runtime_support_inliner_removes_direct_refcount_calls_from_caller() {
        let (mut jit_module, mut ctx, _wrapper_id, helper_names) =
            unsafe { build_runtime_refcount_smoke_context() };
        let before = count_direct_calls_to_runtime_helpers(&ctx.func, &helper_names);
        assert_eq!(
            before, 2,
            "test caller should start with direct incref/decref calls"
        );

        let inlined = inline_runtime_support_calls(
            &mut jit_module,
            &mut ctx,
            "test runtime support inliner should run",
        )
        .expect("runtime support inliner should succeed");
        let after = count_direct_calls_to_runtime_helpers(&ctx.func, &helper_names);

        assert!(
            inlined,
            "runtime support inliner should report at least one inlined call"
        );
        assert_eq!(
            after, 0,
            "runtime support inliner should remove direct incref/decref calls from the caller"
        );
    }

    #[test]
    fn jit_runtime_clif_refcount_roundtrip_preserves_py_long_refcount() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            let wrapper = build_runtime_refcount_smoke_wrapper();
            let python_home = vendored_python_home();
            std::env::set_var("PYTHONHOME", &python_home);
            let python_path = std::env::join_paths([
                python_home.join("Lib"),
                vendored_python_build_lib_dir(),
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .expect("workspace crate should have a repo-root parent")
                    .join("soac_py")
                    .join("src"),
            ])
            .expect("test PYTHONPATH should join");
            std::env::set_var("PYTHONPATH", python_path);
            Python::initialize();
            Python::attach(|_| {
                let obj = ffi::PyLong_FromLongLong(123);
                assert!(
                    !obj.is_null(),
                    "PyLong_FromLongLong should produce a test object"
                );
                let before = ffi::Py_REFCNT(obj);
                let result = wrapper(obj.cast());
                let after = ffi::Py_REFCNT(obj);
                assert_eq!(result, obj.cast(), "wrapper should return the same pointer");
                assert_eq!(
                    after, before,
                    "runtime CLIF incref/decref should preserve PyLong refcount"
                );
                ffi::Py_DECREF(obj);
            });
        }
    }

    #[test]
    fn jit_runtime_clif_decref_can_destroy_py_capsule() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            let wrapper = build_runtime_decref_wrapper();
            let python_home = vendored_python_home();
            std::env::set_var("PYTHONHOME", &python_home);
            let python_path =
                std::env::join_paths([python_home.join("Lib"), vendored_python_build_lib_dir()])
                    .expect("test PYTHONPATH should join");
            std::env::set_var("PYTHONPATH", python_path);
            Python::initialize();
            Python::attach(|_| {
                CAPSULE_DESTROYED.store(false, Ordering::SeqCst);
                let capsule = ffi::PyCapsule_New(
                    std::ptr::dangling_mut::<c_void>(),
                    c"soac.runtime.test".as_ptr(),
                    Some(test_capsule_destructor),
                );
                assert!(
                    !capsule.is_null(),
                    "PyCapsule_New should produce a test object"
                );
                assert_eq!(
                    ffi::Py_REFCNT(capsule),
                    1,
                    "capsule should start with a unique owned reference"
                );

                wrapper(capsule.cast());
                let after = ffi::Py_REFCNT(capsule);

                assert!(
                    CAPSULE_DESTROYED.load(Ordering::SeqCst),
                    "runtime CLIF decref should drive PyCapsule destruction through _Py_Dealloc; refcnt after wrapper = {after}"
                );
            });
        }
    }

    #[test]
    fn jit_vectorcall_trampoline_can_link_runtime_decref_clif() {
        unsafe {
            let compiled = Box::new(CompiledSpecializedRunner {
                _jit_module: new_jit_module().expect("compiled runner jit module should construct"),
                entry: Some(CompiledRunnerEntry::Direct {
                    code_ptr: std::ptr::null(),
                    param_count: 0,
                }),
            });
            let compiled_handle = Box::into_raw(compiled) as ObjPtr;
            let result = compile_cranelift_vectorcall_direct_trampoline(
                test_bind_direct_args_stub,
                1usize as ObjPtr,
                1usize as ObjPtr,
                compiled_handle,
                "jit_runtime_support_vectorcall_smoke",
            );

            match result {
                Ok((trampoline_handle, _entry)) => {
                    free_cranelift_vectorcall_trampoline(trampoline_handle);
                }
                Err(error) => {
                    free_cranelift_run_bb_specialized_cached(compiled_handle);
                    panic!(
                        "vectorcall trampoline should link runtime CLIF refcount helpers: {error}"
                    );
                }
            }

            free_cranelift_run_bb_specialized_cached(compiled_handle);
        }
    }

    #[test]
    fn jit_block_entry_counter_updates_shared_state() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            let python_home = vendored_python_home();
            let repo_root = repo_root();
            let soac_py_src = repo_root.join("soac_py").join("src");
            let ext_staging_dir = ensure_test_extension_staging_dir();
            let pythonpath = std::env::join_paths([
                python_home.join("Lib"),
                vendored_python_build_lib_dir(),
                soac_py_src.clone(),
                ext_staging_dir.clone(),
            ])
            .expect("test PYTHONPATH should join cleanly");
            std::env::set_var("PYTHONHOME", &python_home);
            std::env::set_var("PYTHONPATH", pythonpath);
            Python::initialize();
            Python::attach(|py| {
                let sys = py.import("sys").expect("sys should import");
                sys.getattr("path")
                    .expect("sys.path should exist")
                    .call_method1("insert", (0, ext_staging_dir.to_string_lossy().as_ref()))
                    .expect("sys.path should accept staged _soac_ext");
                sys.getattr("path")
                    .expect("sys.path should exist")
                    .call_method1("insert", (0, soac_py_src.to_string_lossy().as_ref()))
                    .expect("sys.path should accept soac_py/src");
                let mut lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
                    r#"
def f():
    return None
"#,
                )
                .expect("lowering should succeed")
                .codegen_module;
                instrument_bb_module_with_block_entry_counters(&mut lowered);

                let function = lowered
                    .callable_defs
                    .iter()
                    .find(|function| function.names.bind_name == "f")
                    .expect("missing lowered function f")
                    .clone();
                let entry_label = function.entry_block().label;
                let entry_counter_id = lowered
                    .counter_defs
                    .iter()
                    .find_map(|counter| match &counter.site {
                        CounterSite::BlockEntry {
                            function_id,
                            block_label,
                        } if counter.kind == "block_entry"
                            && *function_id == function.function_id
                            && *block_label == entry_label =>
                        {
                            Some(counter.id)
                        }
                        _ => None,
                    })
                    .expect("missing entry counter for lowered function f");

                let shared_state = crate::module_type::build_shared_state_for_testing(
                    py,
                    lowered,
                    "counter_test",
                    "",
                )
                .expect("shared state should build");
                let runtime = build_test_module_runtime(py, shared_state.clone());
                let module_constant_ptrs = shared_state.module_constant_ptrs();
                let counter_ptrs = shared_state.counter_ptrs();
                let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
                let compiled_handle = compile_cranelift_run_bb_specialized_cached(
                    &blocks,
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    &counter_ptrs,
                    Some(shared_state.as_ref()),
                )
                .expect("direct counter test function should compile");
                let (code_ptr, param_count) = compiled_direct_runner_info(compiled_handle)
                    .expect("compiled direct runner should expose entrypoint");
                assert_eq!(param_count, 0, "test function should not take direct args");
                let entry: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
                    std::mem::transmute(code_ptr);

                let result1 = entry(
                    std::ptr::addr_of!(runtime.vmctx).cast_mut().cast(),
                    std::ptr::null_mut(),
                );
                let result2 = entry(
                    std::ptr::addr_of!(runtime.vmctx).cast_mut().cast(),
                    std::ptr::null_mut(),
                );

                assert_eq!(
                    shared_state.counter_value(entry_counter_id),
                    2,
                    "entry counter should reflect the number of completed direct JIT calls"
                );

                ffi::Py_DECREF(result1.cast());
                ffi::Py_DECREF(result2.cast());
                free_cranelift_run_bb_specialized_cached(compiled_handle);
            });
        }
    }

    #[test]
    fn jit_function_scope_refcount_counters_track_runtime_helpers() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            let python_home = vendored_python_home();
            let repo_root = repo_root();
            let soac_py_src = repo_root.join("soac_py").join("src");
            let ext_staging_dir = ensure_test_extension_staging_dir();
            let pythonpath = std::env::join_paths([
                python_home.join("Lib"),
                vendored_python_build_lib_dir(),
                soac_py_src.clone(),
                ext_staging_dir.clone(),
            ])
            .expect("test PYTHONPATH should join cleanly");
            std::env::set_var("PYTHONHOME", &python_home);
            std::env::set_var("PYTHONPATH", pythonpath);
            Python::initialize();
            Python::attach(|py| {
                let sys = py.import("sys").expect("sys should import");
                sys.getattr("path")
                    .expect("sys.path should exist")
                    .call_method1("insert", (0, ext_staging_dir.to_string_lossy().as_ref()))
                    .expect("sys.path should accept staged _soac_ext");
                sys.getattr("path")
                    .expect("sys.path should exist")
                    .call_method1("insert", (0, soac_py_src.to_string_lossy().as_ref()))
                    .expect("sys.path should accept soac_py/src");
                let mut lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
                    r#"
def f(x):
    y = x
    del y
    return None
"#,
                )
                .expect("lowering should succeed")
                .codegen_module;
                instrument_bb_module_with_refcount_counters(&mut lowered, CounterScope::Function)
                    .expect("function-scoped refcount counters should instrument");

                let function = lowered
                    .callable_defs
                    .iter()
                    .find(|function| function.names.bind_name == "f")
                    .expect("missing lowered function f")
                    .clone();
                let incref_counter_id = lowered
                    .counter_defs
                    .iter()
                    .find_map(|counter| match &counter.site {
                        CounterSite::Runtime {
                            function_id: Some(counter_function_id),
                            instr_id: None,
                        } if counter.scope == CounterScope::Function
                            && counter.kind == "runtime_incref"
                            && *counter_function_id == function.function_id =>
                        {
                            Some(counter.id)
                        }
                        _ => None,
                    })
                    .expect("missing function-scoped incref counter for lowered function f");
                let decref_counter_id = lowered
                    .counter_defs
                    .iter()
                    .find_map(|counter| match &counter.site {
                        CounterSite::Runtime {
                            function_id: Some(counter_function_id),
                            instr_id: None,
                        } if counter.scope == CounterScope::Function
                            && counter.kind == "runtime_decref"
                            && *counter_function_id == function.function_id =>
                        {
                            Some(counter.id)
                        }
                        _ => None,
                    })
                    .expect("missing function-scoped decref counter for lowered function f");

                let shared_state = crate::module_type::build_shared_state_for_testing(
                    py,
                    lowered,
                    "counter_test",
                    "",
                )
                .expect("shared state should build");
                let runtime = build_test_module_runtime(py, shared_state.clone());
                let module_constant_ptrs = shared_state.module_constant_ptrs();
                let counter_ptrs = shared_state.counter_ptrs();
                let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
                let compiled_handle = compile_cranelift_run_bb_specialized_cached(
                    &blocks,
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    &counter_ptrs,
                    Some(shared_state.as_ref()),
                )
                .expect("direct refcount counter test function should compile");
                let (code_ptr, param_count) = compiled_direct_runner_info(compiled_handle)
                    .expect("compiled direct runner should expose entrypoint");
                assert_eq!(param_count, 1, "test function should take one direct arg");
                let entry: unsafe extern "C" fn(
                    *mut c_void,
                    *mut c_void,
                    *mut c_void,
                ) -> *mut c_void = std::mem::transmute(code_ptr);

                let result1 = entry(
                    std::ptr::addr_of!(runtime.vmctx).cast_mut().cast(),
                    std::ptr::null_mut(),
                    ffi::PyLong_FromLong(7).cast(),
                );
                let incref_after_first = shared_state.counter_value(incref_counter_id);
                let decref_after_first = shared_state.counter_value(decref_counter_id);
                let result2 = entry(
                    std::ptr::addr_of!(runtime.vmctx).cast_mut().cast(),
                    std::ptr::null_mut(),
                    ffi::PyLong_FromLong(11).cast(),
                );

                assert!(
                    shared_state.counter_value(incref_counter_id) > incref_after_first,
                    "function-scoped incref counter should increase after another direct JIT call"
                );
                assert!(
                    shared_state.counter_value(decref_counter_id) > decref_after_first,
                    "function-scoped decref counter should increase after another direct JIT call"
                );

                ffi::Py_DECREF(result1.cast());
                ffi::Py_DECREF(result2.cast());
                free_cranelift_run_bb_specialized_cached(compiled_handle);
            });
        }
    }

    #[test]
    fn render_specialized_jit_clif_smoke() {
        let blocks = [1usize as ObjPtr, 2usize as ObjPtr, 3usize as ObjPtr];
        let function = test_function();
        let function = with_test_blocks(
            function.clone(),
            vec![
                test_source_block(&function, vec![], raise_term()),
                test_source_block(&function, vec![], raise_term()),
                test_source_block(&function, vec![], raise_term()),
            ],
        );
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            rendered.contains("function"),
            "specialized JIT CLIF render should produce function text:\n{}",
            rendered
        );
    }

    #[test]
    fn render_specialized_jit_clif_annotates_block_headers_with_named_typed_params() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = test_function();
        set_stack_slots(&mut function, &["current", "acc"]);
        let mut source = test_source_block(&function, vec![], ret_term(constants.int_expr(7)));
        source.ensure_param("current", BlockParamRole::AbruptKind);
        source.ensure_param("acc", BlockParamRole::AbruptPayload);
        let function = with_test_blocks(function, vec![source]);
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("; block jit_entry(vmctx: i64, callable: i64)"),
            "rendered CLIF should include named typed params on surviving post-opt block headers:\n{rendered}"
        );
        assert!(
            rendered.contains("; block bb0()"),
            "rendered CLIF should still surface the scope name for optimized blocks:\n{rendered}"
        );
        assert!(
            rendered.contains("block0(v0: i64, v1: i64):"),
            "rendered CLIF should keep the real Cranelift block header for round-tripping:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_operator_calls_use_python_capi() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(BinOp::new(
                BinOpKind::Add,
                constants.int_expr(1),
                constants.int_expr(2),
            ))),
        );
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("call PyNumber_Add"),
            "operator lowering should use PyNumber_Add in rendered CLIF:\n{rendered}"
        );
        assert!(
            !rendered.contains("call dp_jit_py_call_positional_three"),
            "direct operator lowering should avoid generic Python helper calls:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_compare_calls_use_richcompare() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(BinOp::new(
                BinOpKind::Lt,
                constants.int_expr(1),
                constants.int_expr(2),
            ))),
        );
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("call PyObject_RichCompare"),
            "comparison lowering should use PyObject_RichCompare in rendered CLIF:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_exact_int_binop_uses_operator_fast_path() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = test_function();
        let block_label = function.name_gen.next_block_name();
        let instr_id = InstrId::new(block_label, 0);
        let block = CodegenBlock {
            label: block_label,
            body: vec![],
            term: ret_term(with_instr_id(
                op_expr(BinOp::new(
                    BinOpKind::Add,
                    constants.int_expr(1),
                    constants.int_expr(2),
                )),
                instr_id,
            )),
            params: vec![],
            exc_edge: None,
        };
        function.blocks = vec![block];
        let rendered = render_test_jit_function_with_operator_specializations(
            &function,
            &blocks,
            constants.module_constants,
            &[(
                instr_id,
                crate::operator_specialization::pack_binary_shape(
                    crate::operator_specialization::ExactTypeTag::Int,
                    crate::operator_specialization::ExactTypeTag::Int,
                ),
            )],
        );
        assert!(
            rendered.contains("call dp_jit_exact_long_binary_op"),
            "exact-int binop specialization should call the direct helper:\n{rendered}"
        );
        assert!(
            rendered.contains("iconst.i64 257"),
            "exact-int binop specialization should guard on the profiled exact-int shape:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_exact_int_compare_uses_operator_fast_path() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = test_function();
        let block_label = function.name_gen.next_block_name();
        let instr_id = InstrId::new(block_label, 0);
        let block = CodegenBlock {
            label: block_label,
            body: vec![],
            term: ret_term(with_instr_id(
                op_expr(BinOp::new(
                    BinOpKind::Lt,
                    constants.int_expr(1),
                    constants.int_expr(2),
                )),
                instr_id,
            )),
            params: vec![],
            exc_edge: None,
        };
        function.blocks = vec![block];
        let rendered = render_test_jit_function_with_operator_specializations(
            &function,
            &blocks,
            constants.module_constants,
            &[(
                instr_id,
                crate::operator_specialization::pack_binary_shape(
                    crate::operator_specialization::ExactTypeTag::Int,
                    crate::operator_specialization::ExactTypeTag::Int,
                ),
            )],
        );
        assert!(
            rendered.contains("call dp_jit_exact_long_binary_op"),
            "exact-int compare specialization should call the direct helper:\n{rendered}"
        );
        assert!(
            rendered.contains("iconst.i64 257"),
            "exact-int compare specialization should guard on the profiled exact-int shape:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_exact_int_unary_uses_operator_fast_path() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = test_function();
        let block_label = function.name_gen.next_block_name();
        let instr_id = InstrId::new(block_label, 0);
        let block = CodegenBlock {
            label: block_label,
            body: vec![],
            term: ret_term(with_instr_id(
                op_expr(soac_blockpy::block_py::UnaryOp::new(
                    soac_blockpy::block_py::UnaryOpKind::Neg,
                    constants.int_expr(1),
                )),
                instr_id,
            )),
            params: vec![],
            exc_edge: None,
        };
        function.blocks = vec![block];
        let rendered = render_test_jit_function_with_operator_specializations(
            &function,
            &blocks,
            constants.module_constants,
            &[(
                instr_id,
                crate::operator_specialization::pack_unary_shape(
                    crate::operator_specialization::ExactTypeTag::Int,
                ),
            )],
        );
        assert!(
            rendered.contains("call dp_jit_exact_long_unary_op"),
            "exact-int unary specialization should call the direct helper:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_string_literals_use_module_constant_loader() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(constants.string_expr("hello")),
        );
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            !rendered.contains("call dp_jit_load_module_constant"),
            "string literal lowering should not call the module constant hook anymore:\n{rendered}"
        );
        assert!(
            rendered.contains("iconst.i64 4096"),
            "string literal lowering should embed the immortal module constant pointer directly:\n{rendered}"
        );
        assert!(
            !rendered.contains("call dp_jit_decode_literal_bytes"),
            "string literal lowering should not decode literal bytes directly anymore:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_constant_locations_use_module_constant_loader() {
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Load::new(test_constant_name(0)))),
        );
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants: vec![int_literal(7)],
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let rendered =
            render_test_jit_function_with_constants(&module, &function, &blocks, &module_constants);
        assert!(
            !rendered.contains("call dp_jit_load_module_constant"),
            "constant slot lowering should not call the module constant hook anymore:\n{rendered}"
        );
        assert!(
            rendered.contains("iconst.i64 4096"),
            "constant slot lowering should embed the immortal module constant pointer directly:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_pow_calls_use_pynumber_power() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(BinOp::new(
                BinOpKind::Pow,
                constants.int_expr(2),
                constants.int_expr(3),
            ))),
        );
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("call PyNumber_Power"),
            "power lowering should use PyNumber_Power in rendered CLIF:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_inplace_pow_calls_use_pynumber_inplace_power() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(BinOp::new(
                BinOpKind::InplacePow,
                constants.int_expr(2),
                constants.int_expr(3),
            ))),
        );
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("call PyNumber_InPlacePower"),
            "inplace power lowering should use PyNumber_InPlacePower in rendered CLIF:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_allocates_function_state_slots() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function =
            with_single_test_block(test_function(), vec![], ret_term(constants.int_expr(7)));
        set_stack_slots(&mut function, &["x", "y"]);
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.matches("explicit_slot 8").count() >= 2,
            "slot-backed JIT plans should allocate explicit stack slots:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_assignments_sync_function_state_slots() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = with_single_test_block(
            test_function(),
            vec![assign_stmt(test_name("x"), constants.int_expr(7))],
            ret_term(name_expr(test_name("x"))),
        );
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("store.i64")
                || rendered.contains("stack_store")
                || rendered.contains("store notrap"),
            "assignment-backed JIT plans should update mirrored function-state slots:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_global_names_load_from_vmctx_globals() {
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(name_expr(test_global_name("x"))),
        );
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            !rendered.contains("call dp_jit_function_globals")
                && rendered.contains("call dp_jit_load_global_obj")
                && !rendered.contains("call dp_jit_load_module_constant"),
            "global located names should use vmctx-backed globals and pass the name object as an immediate constant:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_load_global_intrinsic_uses_direct_helper() {
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Load::new(test_global_name("x")))),
        );
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            rendered.contains("call dp_jit_load_global_obj"),
            "load_global intrinsic should use the direct JIT helper:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_store_global_intrinsic_uses_direct_helper() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Store::new(
                test_global_name("x"),
                constants.int_expr(3),
            ))),
        );
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("call dp_jit_store_global"),
            "store_global intrinsic should use the direct JIT helper:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_closure_names_use_function_closure_cells() {
        let blocks = [1usize as ObjPtr];
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(name_expr(test_closure_cell_name("x", 2))),
        );
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            rendered.contains("call dp_jit_function_closure_cell")
                && rendered.contains("call dp_jit_load_cell"),
            "closure located names should load through callable-rooted closure cells:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_cell_ref_intrinsic_uses_function_closure_cells() {
        let blocks = [1usize as ObjPtr];
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(soac_blockpy::block_py::CellRef::new(
                CellLocation::Closure(2),
            ))),
        );
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            rendered.contains("call dp_jit_function_closure_cell"),
            "cell_ref intrinsic should use callable-rooted closure cells:\n{rendered}"
        );
        assert!(
            !rendered.contains("call dp_jit_load_cell"),
            "cell_ref intrinsic should return the cell object, not its contents:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_cell_ref_on_captured_source_unwraps_wrapper_cell_once() {
        let blocks = [1usize as ObjPtr];
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(soac_blockpy::block_py::CellRef::new(
                CellLocation::CapturedSource(2),
            ))),
        );
        function.storage_layout = Some(StorageLayout {
            freevars: vec![
                ClosureSlot {
                    logical_name: "_dp_classcell".into(),
                    storage_name: "_dp_classcell".into(),
                    init: ClosureInit::InheritedCapture,
                },
                ClosureSlot {
                    logical_name: "__unused".into(),
                    storage_name: "__unused".into(),
                    init: ClosureInit::InheritedCapture,
                },
                ClosureSlot {
                    logical_name: "_dp_classcell".into(),
                    storage_name: "_dp_classcell".into(),
                    init: ClosureInit::InheritedCapture,
                },
            ],
            cellvars: vec![],
            runtime_cells: vec![],
            stack_slots: Vec::new(),
        });
        set_stack_slots(&mut function, &["_dp_classcell"]);
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            rendered.contains("call dp_jit_function_closure_cell"),
            "captured cell sources should resolve through the callable closure:\n{rendered}"
        );
        assert!(
            !rendered.contains("call dp_jit_load_cell"),
            "__dp_cell_ref on a captured cell source should still return the raw cell object:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_delete_intrinsics_use_direct_helpers() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = with_single_test_block(
            test_function(),
            vec![
                expr_stmt(op_expr(DelItem::new(
                    constants.int_expr(1),
                    constants.int_expr(2),
                ))),
                expr_stmt(op_expr(Del::new(test_global_name("x"), true))),
                expr_stmt(op_expr(Del::new(test_global_name("y"), false))),
                expr_stmt(op_expr(Del::new(test_closure_cell_name("cell", 2), false))),
                expr_stmt(op_expr(Del::new(test_closure_cell_name("cell", 2), true))),
            ],
            ret_term(constants.int_expr(0)),
        );
        set_stack_slots(&mut function, &["cell"]);
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("call dp_jit_pyobject_delitem"),
            "delitem intrinsic should use the direct JIT helper:\n{rendered}"
        );
        assert!(
            rendered.contains("call dp_jit_del_global_quietly"),
            "quiet global delete intrinsic should use the direct JIT helper:\n{rendered}"
        );
        assert!(
            rendered.contains("call dp_jit_del_global"),
            "global delete intrinsic should use the direct JIT helper:\n{rendered}"
        );
        assert!(
            rendered.contains("call dp_jit_del_deref"),
            "del_deref intrinsic should use the direct JIT helper:\n{rendered}"
        );
        assert!(
            rendered.contains("call dp_jit_del_deref_quietly"),
            "del_deref_quietly intrinsic should use the direct JIT helper:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_direct_entry_uses_live_positional_defaults() {
        let blocks = [1usize as ObjPtr];
        let mut function =
            with_single_test_block(test_function(), vec![], ret_term(name_expr(test_name("y"))));
        function.params = ParamSpec {
            params: vec![
                Param {
                    name: "x".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
                Param {
                    name: "y".into(),
                    kind: ParamKind::Any,
                    has_default: true,
                },
            ],
        };
        set_stack_slots(&mut function, &["x", "y"]);
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            rendered.contains("call dp_jit_function_positional_default_obj"),
            "direct entry lowering should source omitted positional defaults from the callable:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_direct_entry_uses_live_kwonly_defaults() {
        let blocks = [1usize as ObjPtr];
        let mut function =
            with_single_test_block(test_function(), vec![], ret_term(name_expr(test_name("x"))));
        function.params = ParamSpec {
            params: vec![Param {
                name: "x".into(),
                kind: ParamKind::KwOnly,
                has_default: true,
            }],
        };
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            rendered.contains("call dp_jit_function_kwonly_default_obj"),
            "direct entry lowering should source omitted kwonly defaults from the callable:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_deleted_name_checks_inline_the_sentinel_compare() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Call::new(
                name_expr(test_runtime_name("load_deleted_name")),
                vec![
                    CallArgPositional::Positional(constants.string_expr("x")),
                    CallArgPositional::Positional(name_expr(test_name("x"))),
                ],
                vec![],
            ))),
        );
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("call dp_jit_raise_deleted_name_error"),
            "deleted-name lowering should keep only the cold-path error helper:\n{rendered}"
        );
        assert!(
            !rendered.contains("call dp_jit_load_deleted_name_obj"),
            "deleted-name lowering should inline the DELETED sentinel check in CLIF:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_constant_runtime_helper_calls_still_specialize() {
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Call::new(
                name_expr(test_constant_name(0)),
                vec![],
                vec![],
            ))),
        );
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            vec![InstrResolved::Load(Load::new(test_runtime_name("globals")))],
        );
        assert!(
            !rendered.contains("call dp_jit_load_runtime_obj")
                && !rendered.contains("call dp_jit_py_vectorcall")
                && !rendered.contains("call dp_jit_py_call_object")
                && !rendered.contains("call dp_jit_py_call_with_kw"),
            "constant-backed runtime helpers should still specialize instead of reloading or generic-calling:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_generic_positional_calls_use_vectorcall() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Call::new(
                name_expr(test_global_name("f")),
                vec![
                    CallArgPositional::Positional(constants.int_expr(1)),
                    CallArgPositional::Positional(constants.int_expr(2)),
                ],
                vec![],
            ))),
        );
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("call dp_jit_py_vectorcall"),
            "generic positional calls should lower through the vectorcall helper:\n{rendered}"
        );
        assert!(
            !rendered.contains("call dp_jit_py_call_positional_three")
                && !rendered.contains("call dp_jit_py_call_object")
                && !rendered.contains("call dp_jit_py_call_with_kw"),
            "generic positional calls should avoid the tuple/kwargs helper path:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_direct_function_calls_use_direct_call_path() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let module_name_gen = ModuleNameGen::new(0);
        let callee_name_gen = module_name_gen.next_function_name_gen();
        let caller_name_gen = module_name_gen.next_function_name_gen();

        let mut callee_function = BlockPyFunction {
            function_id: callee_name_gen.function_id(),
            name_gen: callee_name_gen,
            names: FunctionName::new("callee", "callee", "callee", "callee"),
            kind: soac_blockpy::block_py::FunctionKind::Function,
            params: ParamSpec {
                params: vec![Param {
                    name: "x".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                }],
            },
            blocks: vec![CodegenBlock {
                label: BlockLabel::from_index(0),
                body: vec![],
                term: ret_term(name_expr(test_name("x"))),
                params: vec![],
                exc_edge: None,
            }],
            doc: None,
            storage_layout: None,
            scope: Default::default(),
        };
        set_stack_slots(&mut callee_function, &["x"]);

        let call_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let caller_function = BlockPyFunction {
            function_id: caller_name_gen.function_id(),
            name_gen: caller_name_gen,
            names: FunctionName::new("caller", "caller", "caller", "caller"),
            kind: soac_blockpy::block_py::FunctionKind::Function,
            params: ParamSpec::default(),
            blocks: vec![CodegenBlock {
                label: BlockLabel::from_index(0),
                body: vec![],
                term: ret_term(with_instr_id(
                    op_expr(Call::new(
                        name_expr(test_global_name("callee")),
                        vec![CallArgPositional::Positional(constants.int_expr(1))],
                        vec![],
                    )),
                    call_instr_id,
                )),
                params: vec![],
                exc_edge: None,
            }],
            doc: None,
            storage_layout: None,
            scope: Default::default(),
        };

        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: vec!["callee".into()],
            callable_defs: vec![callee_function.clone(), caller_function.clone()],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        let rendered = render_test_jit_function_with_call_target_specializations(
            &module,
            &caller_function,
            &blocks,
            &[(call_instr_id, callee_function.function_id)],
        );

        assert!(
            rendered.contains("call_indirect"),
            "direct call specialization should emit an indirect call to the compiled target:\n{rendered}"
        );
        assert!(
            rendered
                .contains(format!("iconst.i64 {}", callee_function.function_id.packed()).as_str()),
            "direct call specialization should compare against the profiled target function id:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_type_constructors_use_constructor_fast_path() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let old_call_target_specializations =
            std::env::var_os("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS");
        let old_counters_file = std::env::var_os("DIET_PYTHON_COUNTERS_FILE");
        let old_call_target_counters = std::env::var_os("DIET_PYTHON_CALL_TARGET_COUNTERS");
        let old_pythonhome = std::env::var_os("PYTHONHOME");
        let old_pythonpath = std::env::var_os("PYTHONPATH");
        let python_home = vendored_python_home();
        let python_path = std::env::join_paths([
            python_home.join("Lib"),
            vendored_python_build_lib_dir(),
            repo_root().join("soac_py").join("src"),
        ])
        .expect("test PYTHONPATH should join");
        unsafe {
            std::env::remove_var("DIET_PYTHON_COUNTERS_FILE");
            std::env::remove_var("DIET_PYTHON_CALL_TARGET_COUNTERS");
            std::env::set_var("PYTHONHOME", &python_home);
            std::env::set_var("PYTHONPATH", python_path);
        }

        let rendered = unsafe {
            Python::initialize();
            Python::attach(|py| {
                let mut constants = TestConstantPool::default();
                let module_name_gen = ModuleNameGen::new(0);
                let init_name_gen = module_name_gen.next_function_name_gen();
                let init_function_id = init_name_gen.function_id();
                let caller_name_gen = module_name_gen.next_function_name_gen();
                let caller_function_id = caller_name_gen.function_id();

                let mut init_function = BlockPyFunction {
                    function_id: init_function_id,
                    name_gen: init_name_gen,
                    names: FunctionName::new(
                        "Record.__init__",
                        "Record.__init__",
                        "Record.__init__",
                        "Record.__init__",
                    ),
                    kind: soac_blockpy::block_py::FunctionKind::Function,
                    params: ParamSpec {
                        params: vec![
                            Param {
                                name: "self".into(),
                                kind: ParamKind::Any,
                                has_default: false,
                            },
                            Param {
                                name: "x".into(),
                                kind: ParamKind::Any,
                                has_default: false,
                            },
                        ],
                    },
                    blocks: vec![CodegenBlock {
                        label: BlockLabel::from_index(0),
                        body: vec![],
                        term: ret_term(none_expr()),
                        params: vec![],
                        exc_edge: None,
                    }],
                    doc: None,
                    storage_layout: None,
                    scope: Default::default(),
                };
                set_stack_slots(&mut init_function, &["self", "x"]);

                let call_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
                let caller_function = BlockPyFunction {
                    function_id: caller_function_id,
                    name_gen: caller_name_gen,
                    names: FunctionName::new(
                        "make_record",
                        "make_record",
                        "make_record",
                        "make_record",
                    ),
                    kind: soac_blockpy::block_py::FunctionKind::Function,
                    params: ParamSpec::default(),
                    blocks: vec![CodegenBlock {
                        label: BlockLabel::from_index(0),
                        body: vec![],
                        term: ret_term(with_instr_id(
                            op_expr(Call::new(
                                name_expr(test_global_name("Record")),
                                vec![CallArgPositional::Positional(constants.int_expr(1))],
                                vec![],
                            )),
                            call_instr_id,
                        )),
                        params: vec![],
                        exc_edge: None,
                    }],
                    doc: None,
                    storage_layout: None,
                    scope: Default::default(),
                };

                let module = BlockPyModule {
                    module_name_gen: ModuleNameGen::new(0),
                    global_names: vec!["Record".into()],
                    callable_defs: vec![init_function.clone(), caller_function.clone()],
                    module_constants: constants.module_constants,
                    counter_defs: Vec::new(),
                };
                let specialization_value = format!(
                    "counter_test|{}|{}|{}={}",
                    caller_function.function_id.packed(),
                    call_instr_id.block_label().as_u32(),
                    call_instr_id.instr_index_in_block(),
                    init_function.function_id.packed(),
                );
                std::env::set_var(
                    "DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS",
                    specialization_value,
                );

                let shared_state = crate::module_type::build_shared_state_for_testing(
                    py,
                    module,
                    "counter_test",
                    "",
                )
                .expect("shared state should build");
                let mut runtime = build_test_module_runtime(py, shared_state.clone());
                let globals = pyo3::Bound::<pyo3::PyAny>::from_borrowed_ptr(
                    py,
                    runtime.vmctx.globals_obj.cast(),
                )
                .cast_into::<pyo3::types::PyDict>()
                .expect("runtime globals should be a dict");
                globals
                    .set_item("__name__", "counter_test")
                    .expect("globals should accept __name__");
                let class_source = std::ffi::CString::new(
                    "class Record:\n    def __init__(self, x):\n        self.x = x\n",
                )
                .expect("class source should be CString-compatible");
                let run_result = ffi::PyRun_StringFlags(
                    class_source.as_ptr(),
                    ffi::Py_file_input,
                    globals.as_ptr(),
                    globals.as_ptr(),
                    std::ptr::null_mut(),
                );
                assert!(
                    !run_result.is_null(),
                    "class definition should execute in test globals"
                );
                ffi::Py_DECREF(run_result);
                let cls = globals.get_item("Record").expect("class should exist");
                let owner_type = cls.as_ptr() as *mut ffi::PyTypeObject;
                let init_function_obj =
                    ffi::PyDict_GetItemString((*owner_type).tp_dict, c"__init__".as_ptr());
                assert!(
                    !init_function_obj.is_null(),
                    "class dict should contain __init__"
                );
                ffi::Py_INCREF(init_function_obj);
                let runtime_clone = crate::clone_module_runtime_context(&runtime)
                    .expect("runtime clone should succeed");
                crate::register_clif_vectorcall(
                    init_function_obj,
                    init_function.function_id,
                    runtime_clone,
                )
                .expect("registering __init__ vectorcall should succeed");
                ffi::Py_DECREF(init_function_obj);

                crate::with_active_module_runtime_context(&mut runtime, || {
                    let mut jit_module =
                        new_jit_module().expect("test jit module should construct");
                    let module_constant_ptrs = shared_state.module_constant_ptrs();
                    let counter_ptrs = shared_state.counter_ptrs();
                    let built = build_cranelift_run_bb_specialized_function(
                        &mut jit_module,
                        &[1usize as ObjPtr],
                        &shared_state.lowered_module,
                        &caller_function,
                        &shared_state.codegen_constants,
                        &shared_state.lowered_module.counter_defs,
                        &module_constant_ptrs,
                        &counter_ptrs,
                        Some(shared_state.as_ref()),
                    )
                    .expect("specialized JIT build should succeed");
                    let (clif, _cfg_dot, _vcode_disasm) = render_compiled_clif_and_vcode_disasm(
                        &mut jit_module,
                        built.ctx,
                        &built.import_id_to_symbol,
                        &built.block_annotations,
                    )
                    .expect("specialized JIT CLIF render should succeed");
                    clif
                })
            })
        };

        match old_call_target_specializations {
            Some(value) => unsafe {
                std::env::set_var("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS", value)
            },
            None => unsafe { std::env::remove_var("DIET_PYTHON_CALL_TARGET_SPECIALIZATIONS") },
        }
        match old_counters_file {
            Some(value) => unsafe { std::env::set_var("DIET_PYTHON_COUNTERS_FILE", value) },
            None => unsafe { std::env::remove_var("DIET_PYTHON_COUNTERS_FILE") },
        }
        match old_call_target_counters {
            Some(value) => unsafe { std::env::set_var("DIET_PYTHON_CALL_TARGET_COUNTERS", value) },
            None => unsafe { std::env::remove_var("DIET_PYTHON_CALL_TARGET_COUNTERS") },
        }
        match old_pythonhome {
            Some(value) => unsafe { std::env::set_var("PYTHONHOME", value) },
            None => unsafe { std::env::remove_var("PYTHONHOME") },
        }
        match old_pythonpath {
            Some(value) => unsafe { std::env::set_var("PYTHONPATH", value) },
            None => unsafe { std::env::remove_var("PYTHONPATH") },
        }

        assert!(
            rendered.contains("call dp_jit_pytype_generic_alloc"),
            "constructor specialization should allocate via the constructor fast path:\n{rendered}"
        );
        assert!(
            rendered.contains("call dp_jit_finish_constructor_init"),
            "constructor specialization should validate __init__ results in the fast path:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_delete_stmt_updates_function_state_slots() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = with_single_test_block(
            test_function(),
            vec![delete_stmt(test_name("x"))],
            ret_term(constants.int_expr(0)),
        );
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("store.i64")
                || rendered.contains("stack_store")
                || rendered.contains("store notrap"),
            "delete-backed JIT plans should update mirrored function-state slots:\n{rendered}"
        );
    }
}
