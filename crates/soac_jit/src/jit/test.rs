use super::*;
use soac_core::block_py::IncrementCounter;
use soac_core::block_py::literal::{
    Literal, LiteralValue, NumberLiteral, NumberLiteralValue, StringLiteral,
};
use soac_core::block_py::{
    AbruptKind, BinOp, BinOpKind, BlockArg, BlockEdge, BlockLabel, BlockParam, BlockParamRole,
    BlockPyFunction, BlockPyModule, BlockTerm, Call, CallArgKeyword, CallArgPositional, CallDirect,
    CalleeFunctionId, CellLocation, CellRef, ChildVisitable, ClosureInit, ClosureSlot, CounterDef,
    CounterSite, Del, DelItem, FunctionExecutionMode, FunctionKind, FunctionName, GetAttr, GetItem,
    HasMeta, HasSemanticInstrId, Load, LocalFunctionId, LocalLocation, MakeCell, Meta,
    ModuleNameGen, NameLike, NameLocation, Param, ParamKind, ParamSpec, ResolvedName,
    RuntimeFunctionId, RuntimeName, SerializedFunctionDebugName, SerializedFunctionId,
    SerializedIdentityTables, SerializedModuleId, SerializedModuleIdentity, SetAttr, SetItem,
    StorageLayout, Store, Tuple, UnaryOp, UnaryOpKind, Visit, VisitMut, WithMeta,
};
use soac_lowering::passes::{
    CodegenModuleShape, InstrCodegen, InstrResolved, validate_codegen_instr_ids,
};
mod tests {
    use super::*;
    use crate::jit::direct_abi::RuntimePrimitiveId;
    use cranelift_codegen::cursor::Cursor;
    use pyo3::types::{PyAnyMethods, PyDict, PyDictMethods, PyModule, PyModuleMethods, PyTuple};
    use pyo3::{Bound, Py, PyAny, PyErr, PyResult, Python, ffi};
    use ruff_python_ast as ast;
    use soac_core::pass_tracker::NoopPassTracker;
    use soac_core::profile::{
        CounterDumpKeyLayout, CounterDumpRecord, CounterDumpRow, CounterDumpTypeKey,
        CounterDumpTypeKeyLayout, CounterDumpTypeTableEntry, write_counter_dump_records,
    };
    use soac_driver::codegen_cache::{
        CachedCodegenModuleMetadata, PythonModuleCacheSource, module_optimization_plan_v3_path,
        module_optimized_codegen_v3_path, pre_optimization_module_cache_identity,
        store_codegen_module_cache,
    };
    use soac_instrument::{
        CounterInstrumentationConfig, ExplicitCounterPlacement, InstrumentationConfig,
        RefcountCounterMode, instrument_codegen_module_with_tracker,
    };
    use soac_opt::alternatives_v3::AlternativeCatalog;
    use soac_opt::artifacts_v3::{ExactIntBranchV3Artifacts, write_optimization_artifacts_v3};
    use soac_opt::emit_v3::{MechanicalIndexedFieldGuard, MechanicalModuleEmission};
    use soac_opt::passes::{
        TypedInstrExtra, TypedPlannedResult as PlannedResult,
        lower_typed_function_if_tests_to_truthy,
    };
    use soac_opt::pipeline_v3::{
        plan_and_emit_function_exact_int_branches_v3_with_module_constants,
        plan_and_emit_module_v3_from_raw_evidence,
    };
    use soac_opt::plan::{FunctionProfileEvidence, ProfileEvidenceStore};
    use soac_opt::plan_v3::{
        CallBodyKind, CallBodyPlan, Cost, DirectCallArgPlan as PlanV3DirectCallArgPlan,
        DirectCallArgSource as PlanV3DirectCallArgSource, DirectCallSpecializationPlan,
        ExactListItemAccessKind as PlanV3ExactListItemAccessKind,
        ExactListItemFallbackKind as PlanV3ExactListItemFallbackKind,
        ExactListItemGuardKind as PlanV3ExactListItemGuardKind,
        ExactListItemShape as PlanV3ExactListItemShape, FunctionPlanIdentity,
        IndexedFieldAccessKind, IndexedFieldFallbackKind, IndexedFieldFallbackPlan,
        IndexedFieldGuardKind, IndexedFieldGuardPlan, IndexedFieldOwnerType,
        IndexedFieldSpecializationPlan, IndexedGlobalAccessKind, IndexedGlobalFallbackKind,
        IndexedGlobalFallbackPlan, IndexedGlobalGuardKind, IndexedGlobalGuardPlan,
        IndexedGlobalSpecializationPlan, ModuleOptimizationPlanV3, ModulePlanIdentity,
    };
    use std::collections::{HashMap, VecDeque};
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    unsafe extern "C" {
        fn PyCell_New(obj: *mut ffi::PyObject) -> *mut ffi::PyObject;
    }

    fn typed_v3_env_config() -> SoacEnvConfig {
        SoacEnvConfig::default()
            .with_runtime_optimization_pipeline(RuntimeOptimizationPipeline::TypedV3)
    }

    fn legacy_counter_instrumentation_config(
        call_targets: bool,
        block_entry: bool,
    ) -> InstrumentationConfig {
        InstrumentationConfig {
            trace: None,
            counters: CounterInstrumentationConfig {
                call_targets,
                locality: block_entry,
                profiled_cold_blocks: block_entry,
                refcounts: RefcountCounterMode::Disabled,
            },
            explicit_counter_placement: ExplicitCounterPlacement::Codegen,
            deopt_entry_counters: false,
            specialization_runtime_logging: false,
        }
    }

    fn instrument_module_with_legacy_call_target_counters(
        module: &mut BlockPyModule<CodegenModuleShape>,
    ) {
        let instrumented = instrument_codegen_module_with_tracker(
            module.clone(),
            &legacy_counter_instrumentation_config(true, false),
            &mut NoopPassTracker::new(),
        )
        .expect("legacy call-target counter instrumentation should succeed");
        *module = instrumented;
    }

    fn instrument_module_with_legacy_block_entry_counters(
        module: &mut BlockPyModule<CodegenModuleShape>,
    ) {
        let instrumented = instrument_codegen_module_with_tracker(
            module.clone(),
            &legacy_counter_instrumentation_config(false, true),
            &mut NoopPassTracker::new(),
        )
        .expect("legacy block-entry counter instrumentation should succeed");
        *module = instrumented;
    }

    fn runtime_branch_counter_for(
        counter_defs: &[CounterDef],
        function_id: RuntimeFunctionId,
        instr_id: InstrId,
        kind: &str,
        branch: &str,
    ) -> (CounterId, soac_core::block_py::CounterBranchId) {
        counter_defs
            .iter()
            .find_map(|counter| match &counter.site {
                CounterSite::Runtime {
                    function_id: Some(counter_function_id),
                    instr_id: Some(counter_instr_id),
                } if counter.kind == kind
                    && *counter_function_id == function_id
                    && *counter_instr_id == instr_id =>
                {
                    counter
                        .branch_id(branch)
                        .map(|branch_id| (counter.id, branch_id))
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!("missing {kind}.{branch} counter for {function_id} at {instr_id}")
            })
    }

    fn test_v3_call_body(kind: CallBodyKind) -> CallBodyPlan {
        CallBodyPlan {
            kind,
            cost: match kind {
                CallBodyKind::DirectCall => Cost {
                    hot_path: 8,
                    miss_path: 2,
                    deopt: 0,
                    materialization: 0,
                    ownership: 1,
                    code_size: 2,
                    compile: 1,
                },
                CallBodyKind::Inline => Cost {
                    hot_path: 2,
                    miss_path: 2,
                    deopt: 0,
                    materialization: 0,
                    ownership: 0,
                    code_size: 6,
                    compile: 4,
                },
            },
            inline_target: None,
            reason: format!("test {kind:?} call body"),
        }
    }

    fn test_v3_inline_call_body() -> CallBodyPlan {
        test_v3_call_body(CallBodyKind::Inline)
    }

    unsafe fn test_dp_jit_deopt_resume(
        deopt_table: ObjPtr,
        globals_obj: ObjPtr,
        record_ordinal: i64,
        live_values: ObjPtr,
        live_value_count: i64,
    ) -> ObjPtr {
        unsafe {
            crate::jit::specialized_helpers::dp_jit_deopt_resume(
                deopt_table,
                globals_obj,
                std::ptr::null_mut(),
                record_ordinal,
                live_values,
                live_value_count,
            )
        }
    }

    unsafe fn test_dp_jit_deopt_resume_with_function_data(
        deopt_table: ObjPtr,
        globals_obj: ObjPtr,
        function_data_obj: ObjPtr,
        record_ordinal: i64,
        live_values: ObjPtr,
        live_value_count: i64,
    ) -> ObjPtr {
        unsafe {
            crate::jit::specialized_helpers::dp_jit_deopt_resume(
                deopt_table,
                globals_obj,
                function_data_obj,
                record_ordinal,
                live_values,
                live_value_count,
            )
        }
    }

    static CAPSULE_DESTROYED: AtomicBool = AtomicBool::new(false);
    static NEXT_TEST_WORK_DIR_ID: AtomicUsize = AtomicUsize::new(0);

    struct ForceEntryInterpreterVectorcallGuard {
        previous: bool,
    }

    impl ForceEntryInterpreterVectorcallGuard {
        fn new() -> Self {
            Self {
                previous: crate::force_entry_interpreter_vectorcall_for_tests(true),
            }
        }
    }

    impl Drop for ForceEntryInterpreterVectorcallGuard {
        fn drop(&mut self) {
            crate::force_entry_interpreter_vectorcall_for_tests(self.previous);
        }
    }

    #[repr(C)]
    struct RawPyDictKeysObject {
        dk_refcnt: isize,
        dk_log2_size: u8,
        dk_log2_index_bytes: u8,
        dk_kind: u8,
        dk_version: u32,
        dk_usable: isize,
        dk_nentries: isize,
    }

    #[repr(C)]
    struct RawPyDictUnicodeEntry {
        me_key: *mut ffi::PyObject,
        me_value: *mut ffi::PyObject,
    }

    unsafe extern "C" {
        fn PyThreadState_GetUnchecked() -> *mut ffi::PyThreadState;
    }
    #[test]
    fn cranelift_function_name_is_stable_from_logical_name() {
        assert_eq!(
            stable_cranelift_function_hash(b""),
            0xcbf29ce484222325,
            "empty FNV-1a hash should stay stable"
        );
        assert_eq!(
            stable_cranelift_function_name("direct:pkg.mod.fn:2"),
            stable_cranelift_function_name("direct:pkg.mod.fn:2")
        );
        assert_ne!(
            stable_cranelift_function_name("direct:pkg.mod.fn:2"),
            stable_cranelift_function_name("direct:pkg.mod.fn:3")
        );
    }

    fn block_successor_targets(function: &ir::Function, block: ir::Block) -> Vec<ir::Block> {
        function
            .layout
            .last_inst(block)
            .into_iter()
            .flat_map(|inst| {
                function.dfg.insts[inst]
                    .branch_destination(&function.dfg.jump_tables, &function.dfg.exception_tables)
                    .iter()
                    .map(|destination| destination.block(&function.dfg.value_lists))
            })
            .collect()
    }

    fn single_block_successor_args(function: &ir::Function, block: ir::Block) -> Vec<ir::BlockArg> {
        let inst = function
            .layout
            .last_inst(block)
            .expect("block should end in a branch");
        let destinations = function.dfg.insts[inst]
            .branch_destination(&function.dfg.jump_tables, &function.dfg.exception_tables);
        assert_eq!(destinations.len(), 1);
        destinations[0].args(&function.dfg.value_lists).collect()
    }

    fn build_noncritical_trivial_jump_function() -> (ir::Function, ir::Block, ir::Block) {
        let mut function = ir::Function::new();
        function
            .signature
            .params
            .push(ir::AbiParam::new(ir::types::I64));
        function
            .signature
            .returns
            .push(ir::AbiParam::new(ir::types::I64));
        let entry;
        let target;
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut function, &mut builder_ctx);
            entry = fb.create_block();
            let forwarder = fb.create_block();
            target = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.append_block_param(forwarder, ir::types::I64);
            fb.append_block_param(target, ir::types::I64);

            fb.switch_to_block(entry);
            let entry_value = fb.block_params(entry)[0];
            fb.ins()
                .jump(forwarder, &[ir::BlockArg::Value(entry_value)]);

            fb.switch_to_block(forwarder);
            let forwarded_value = fb.block_params(forwarder)[0];
            fb.ins().nop();
            fb.ins()
                .jump(target, &[ir::BlockArg::Value(forwarded_value)]);

            fb.switch_to_block(target);
            let result = fb.block_params(target)[0];
            fb.ins().return_(&[result]);

            fb.seal_all_blocks();
            fb.finalize();
        }
        (function, entry, target)
    }

    fn build_chained_trivial_jump_function() -> (ir::Function, ir::Block, ir::Block) {
        let mut function = ir::Function::new();
        function
            .signature
            .returns
            .push(ir::AbiParam::new(ir::types::I64));
        let entry;
        let target;
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut function, &mut builder_ctx);
            entry = fb.create_block();
            let first_forwarder = fb.create_block();
            let second_forwarder = fb.create_block();
            target = fb.create_block();

            fb.switch_to_block(entry);
            fb.ins().jump(first_forwarder, &[]);

            fb.switch_to_block(first_forwarder);
            fb.ins().nop();
            fb.ins().jump(second_forwarder, &[]);

            fb.switch_to_block(second_forwarder);
            fb.ins().nop();
            fb.ins().jump(target, &[]);

            fb.switch_to_block(target);
            let result = fb.ins().iconst(ir::types::I64, 1);
            fb.ins().return_(&[result]);

            fb.seal_all_blocks();
            fb.finalize();
        }
        (function, entry, target)
    }

    fn build_critical_trivial_jump_function() -> ir::Function {
        let mut function = ir::Function::new();
        function
            .signature
            .params
            .push(ir::AbiParam::new(ir::types::I64));
        function
            .signature
            .params
            .push(ir::AbiParam::new(ir::types::I64));
        function
            .signature
            .returns
            .push(ir::AbiParam::new(ir::types::I64));
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut function, &mut builder_ctx);
            let entry = fb.create_block();
            let forwarder = fb.create_block();
            let other = fb.create_block();
            let target = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.append_block_param(forwarder, ir::types::I64);
            fb.append_block_param(other, ir::types::I64);
            fb.append_block_param(target, ir::types::I64);

            fb.switch_to_block(entry);
            let lhs = fb.block_params(entry)[0];
            let rhs = fb.block_params(entry)[1];
            fb.ins().brif(
                lhs,
                forwarder,
                &[ir::BlockArg::Value(lhs)],
                other,
                &[ir::BlockArg::Value(rhs)],
            );

            fb.switch_to_block(forwarder);
            let forwarded_value = fb.block_params(forwarder)[0];
            fb.ins().nop();
            fb.ins()
                .jump(target, &[ir::BlockArg::Value(forwarded_value)]);

            fb.switch_to_block(other);
            let other_value = fb.block_params(other)[0];
            fb.ins().jump(target, &[ir::BlockArg::Value(other_value)]);

            fb.switch_to_block(target);
            let result = fb.block_params(target)[0];
            fb.ins().return_(&[result]);

            fb.seal_all_blocks();
            fb.finalize();
        }
        function
    }

    fn build_partially_noncritical_trivial_jump_function() -> (
        ir::Function,
        ir::Block,
        ir::Block,
        ir::Block,
        ir::Block,
        ir::Block,
    ) {
        let mut function = ir::Function::new();
        function
            .signature
            .params
            .push(ir::AbiParam::new(ir::types::I64));
        function
            .signature
            .returns
            .push(ir::AbiParam::new(ir::types::I64));
        let direct;
        let branch;
        let forwarder;
        let other;
        let target;
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut function, &mut builder_ctx);
            let entry = fb.create_block();
            direct = fb.create_block();
            branch = fb.create_block();
            forwarder = fb.create_block();
            other = fb.create_block();
            target = fb.create_block();
            fb.append_block_params_for_function_params(entry);

            fb.switch_to_block(entry);
            let cond = fb.block_params(entry)[0];
            fb.ins().brif(cond, direct, &[], branch, &[]);

            fb.switch_to_block(direct);
            fb.ins().jump(forwarder, &[]);

            fb.switch_to_block(branch);
            fb.ins().brif(cond, forwarder, &[], other, &[]);

            fb.switch_to_block(forwarder);
            fb.ins().jump(target, &[]);

            fb.switch_to_block(other);
            fb.ins().jump(target, &[]);

            fb.switch_to_block(target);
            let result = fb.ins().iconst(ir::types::I64, 1);
            fb.ins().return_(&[result]);

            fb.seal_all_blocks();
            fb.finalize();
        }
        (function, direct, branch, forwarder, other, target)
    }

    fn build_non_param_jump_arg_trivial_jump_function() -> ir::Function {
        let mut function = ir::Function::new();
        function
            .signature
            .returns
            .push(ir::AbiParam::new(ir::types::I64));
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut function, &mut builder_ctx);
            let entry = fb.create_block();
            let forwarder = fb.create_block();
            let target = fb.create_block();
            fb.append_block_param(forwarder, ir::types::I64);
            fb.append_block_param(target, ir::types::I64);

            fb.switch_to_block(entry);
            let forwarded = fb.ins().iconst(ir::types::I64, 1);
            let non_param = fb.ins().iconst(ir::types::I64, 2);
            fb.ins().jump(forwarder, &[ir::BlockArg::Value(forwarded)]);

            fb.switch_to_block(forwarder);
            fb.ins().nop();
            fb.ins().jump(target, &[ir::BlockArg::Value(non_param)]);

            fb.switch_to_block(target);
            let result = fb.block_params(target)[0];
            fb.ins().return_(&[result]);

            fb.seal_all_blocks();
            fb.finalize();
        }
        function
    }

    fn build_successor_use_trivial_jump_function() -> ir::Function {
        let mut function = ir::Function::new();
        function
            .signature
            .params
            .push(ir::AbiParam::new(ir::types::I64));
        function
            .signature
            .returns
            .push(ir::AbiParam::new(ir::types::I64));
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut function, &mut builder_ctx);
            let entry = fb.create_block();
            let forwarder = fb.create_block();
            let target = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.append_block_param(forwarder, ir::types::I64);

            fb.switch_to_block(entry);
            let entry_value = fb.block_params(entry)[0];
            fb.ins()
                .jump(forwarder, &[ir::BlockArg::Value(entry_value)]);

            fb.switch_to_block(forwarder);
            let forwarded_value = fb.block_params(forwarder)[0];
            fb.ins().nop();
            fb.ins().jump(target, &[]);

            fb.switch_to_block(target);
            let result = fb.ins().iadd_imm(forwarded_value, 1);
            fb.ins().return_(&[result]);

            fb.seal_all_blocks();
            fb.finalize();
        }
        function
    }

    fn build_successor_branch_arg_trivial_jump_function() -> ir::Function {
        let mut function = ir::Function::new();
        function
            .signature
            .params
            .push(ir::AbiParam::new(ir::types::I64));
        function
            .signature
            .returns
            .push(ir::AbiParam::new(ir::types::I64));
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut function, &mut builder_ctx);
            let entry = fb.create_block();
            let forwarder = fb.create_block();
            let target = fb.create_block();
            let done = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.append_block_param(forwarder, ir::types::I64);
            fb.append_block_param(done, ir::types::I64);

            fb.switch_to_block(entry);
            let entry_value = fb.block_params(entry)[0];
            fb.ins()
                .jump(forwarder, &[ir::BlockArg::Value(entry_value)]);

            fb.switch_to_block(forwarder);
            let forwarded_value = fb.block_params(forwarder)[0];
            fb.ins().nop();
            fb.ins().jump(target, &[]);

            fb.switch_to_block(target);
            fb.ins().jump(done, &[ir::BlockArg::Value(forwarded_value)]);

            fb.switch_to_block(done);
            let result = fb.block_params(done)[0];
            fb.ins().return_(&[result]);

            fb.seal_all_blocks();
            fb.finalize();
        }
        function
    }

    #[test]
    fn normalize_trivial_jump_block_with_nop_before_terminator() {
        let (mut function, entry, target) = build_noncritical_trivial_jump_function();
        let entry_arg = function.dfg.block_params(entry)[0];

        assert_eq!(function.layout.blocks().count(), 3);
        let stats = normalize_postopt_clif_for_inspection(&mut function);
        assert_eq!(stats.removed_blocks, 1);
        assert_eq!(function.layout.blocks().count(), 2);
        assert_eq!(block_successor_targets(&function, entry), vec![target]);
        assert_eq!(
            single_block_successor_args(&function, entry),
            vec![ir::BlockArg::Value(entry_arg)]
        );
    }

    #[test]
    fn normalize_trivial_jump_blocks_iterates_to_fixpoint() {
        let (mut function, entry, target) = build_chained_trivial_jump_function();

        assert_eq!(function.layout.blocks().count(), 4);
        let stats = normalize_postopt_clif_for_inspection(&mut function);
        assert_eq!(stats.removed_blocks, 2);
        assert_eq!(function.layout.blocks().count(), 2);
        assert_eq!(block_successor_targets(&function, entry), vec![target]);
    }

    #[test]
    fn normalize_trivial_jump_block_keeps_critical_edges_split() {
        let mut function = build_critical_trivial_jump_function();

        assert_eq!(function.layout.blocks().count(), 4);
        let stats = normalize_postopt_clif_for_inspection(&mut function);
        assert_eq!(stats.removed_blocks, 0);
        assert_eq!(stats.redirected_edges, 0);
        assert_eq!(function.layout.blocks().count(), 4);
    }

    #[test]
    fn normalize_trivial_jump_block_threads_safe_predecessor_edges() {
        let (mut function, direct, branch, forwarder, other, target) =
            build_partially_noncritical_trivial_jump_function();

        assert_eq!(function.layout.blocks().count(), 6);
        let stats = normalize_postopt_clif_for_inspection(&mut function);
        assert_eq!(
            stats.removed_blocks, 0,
            "the split block should remain for the critical conditional edge"
        );
        assert_eq!(
            stats.redirected_edges, 1,
            "only the safe direct predecessor edge should be rewritten"
        );
        assert_eq!(function.layout.blocks().count(), 6);
        assert_eq!(block_successor_targets(&function, direct), vec![target]);
        assert_eq!(
            block_successor_targets(&function, branch),
            vec![forwarder, other],
            "the conditional predecessor should still use the split block for its critical edge"
        );
    }

    #[test]
    fn normalize_trivial_jump_block_skips_non_param_forwarding() {
        let mut function = build_non_param_jump_arg_trivial_jump_function();

        assert_eq!(function.layout.blocks().count(), 3);
        let stats = normalize_postopt_clif_for_inspection(&mut function);
        assert_eq!(stats.removed_blocks, 0);
        assert_eq!(stats.redirected_edges, 0);
        assert_eq!(function.layout.blocks().count(), 3);
    }

    #[test]
    fn normalize_trivial_jump_block_skips_successor_param_uses() {
        let mut function = build_successor_use_trivial_jump_function();

        assert_eq!(function.layout.blocks().count(), 3);
        let stats = normalize_postopt_clif_for_inspection(&mut function);
        assert_eq!(stats.removed_blocks, 0);
        assert_eq!(stats.redirected_edges, 0);
        assert_eq!(function.layout.blocks().count(), 3);
    }

    #[test]
    fn normalize_trivial_jump_block_skips_successor_branch_arg_uses() {
        let mut function = build_successor_branch_arg_trivial_jump_function();

        assert_eq!(function.layout.blocks().count(), 4);
        let stats = normalize_postopt_clif_for_inspection(&mut function);
        assert_eq!(stats.removed_blocks, 0);
        assert_eq!(stats.redirected_edges, 0);
        assert_eq!(function.layout.blocks().count(), 4);
    }

    #[test]
    fn direct_function_module_identity_is_stable() {
        let mut first = String::new();
        push_direct_function_module_identity(&mut first, "pkg.mod", 0x1234);
        let mut second = String::new();
        push_direct_function_module_identity(&mut second, "pkg.mod", 0x1234);
        let mut different_hash = String::new();
        push_direct_function_module_identity(&mut different_hash, "pkg.mod", 0x1235);
        let mut different_module = String::new();
        push_direct_function_module_identity(&mut different_module, "pkg.other", 0x1234);
        assert_eq!(first, second);
        assert_eq!(first, "706b672e6d6f64:0000000000001234");
        assert_ne!(first, different_hash);
        assert_ne!(first, different_module);
    }

    #[test]
    fn shared_module_symbol_identity_is_stable() {
        let mut first = String::new();
        push_shared_module_symbol_identity(&mut first, "pkg.mod", 0x1234, None);
        let mut second = String::new();
        push_shared_module_symbol_identity(&mut second, "pkg.mod", 0x1234, None);
        let mut different_hash = String::new();
        push_shared_module_symbol_identity(&mut different_hash, "pkg.mod", 0x1235, None);
        let mut zero_hash_first = String::new();
        push_shared_module_symbol_identity(&mut zero_hash_first, "pkg.mod", 0, Some(7));
        let mut zero_hash_second = String::new();
        push_shared_module_symbol_identity(&mut zero_hash_second, "pkg.mod", 0, Some(8));
        assert_eq!(first, second);
        assert_eq!(first, "706b672e6d6f64_0000000000001234");
        assert_ne!(first, different_hash);
        assert_eq!(zero_hash_first, "706b672e6d6f64_0000000000000000_inst_7");
        assert_eq!(zero_hash_second, "706b672e6d6f64_0000000000000000_inst_8");
        assert_ne!(zero_hash_first, zero_hash_second);
    }

    #[test]
    fn shared_module_instance_symbols_separate_duplicate_module_identities() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def f():
    return None
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            let first = crate::module_type::build_shared_state_for_testing(
                py,
                lowered.clone(),
                "counter_test",
                "",
            )
            .expect("first shared state should build");
            let second =
                crate::module_type::build_shared_state_for_testing(py, lowered, "counter_test", "")
                    .expect("second shared state should build");

            assert_ne!(first.storage_instance_key(), second.storage_instance_key());
            assert_ne!(
                module_constant_symbol_prefix_for_shared_state(first.as_ref()),
                module_constant_symbol_prefix_for_shared_state(second.as_ref())
            );
            assert_ne!(
                direct_function_symbol_scope_for_shared_state(
                    first.as_ref(),
                    RuntimeFunctionId::from_raw_parts(0, 1)
                ),
                direct_function_symbol_scope_for_shared_state(
                    second.as_ref(),
                    RuntimeFunctionId::from_raw_parts(0, 1)
                )
            );
        });
    }

    #[test]
    fn local_module_constant_data_symbols_are_instance_scoped() {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let first = test_module(ModuleNameGen::new(0), vec![test_function()]);
        let second = test_module(ModuleNameGen::new(0), vec![test_function()]);
        let first_ptr = 0x1_0000usize as *mut ffi::PyObject;
        let second_ptr = 0x2_0000usize as *mut ffi::PyObject;

        declare_module_constant_object_data(&mut jit_module, &first, &[first_ptr])
            .expect("first module constant object data should declare");
        declare_module_constant_object_data(&mut jit_module, &second, &[second_ptr])
            .expect("second module constant object data should declare");

        let first_prefix =
            module_constant_symbol_prefix_for_instance(&first, std::ptr::addr_of!(first) as usize);
        let second_prefix = module_constant_symbol_prefix_for_instance(
            &second,
            std::ptr::addr_of!(second) as usize,
        );
        let first_symbol =
            module_constant_object_symbol(first_prefix.as_str(), ModuleConstantId(0));
        let second_symbol =
            module_constant_object_symbol(second_prefix.as_str(), ModuleConstantId(0));

        assert_ne!(first_symbol, second_symbol);
        assert_eq!(
            lookup_registered_jit_data_symbol(first_symbol.as_str()),
            Some(first_ptr.cast::<u8>() as *const u8)
        );
        assert_eq!(
            lookup_registered_jit_data_symbol(second_symbol.as_str()),
            Some(second_ptr.cast::<u8>() as *const u8)
        );
    }

    #[test]
    fn precompiled_symbol_scopes_use_source_hash_and_logical_function_id() {
        let cached_id =
            persistent_function_id_for_module_function("pkg.mod", 0x1234, LocalFunctionId::new(7));
        let remapped_id =
            persistent_function_id_for_module_function("pkg.mod", 0x1234, LocalFunctionId::new(7));
        let cached_scope = precompiled_direct_function_symbol_scope_for_persistent(&cached_id);
        let remapped_scope = precompiled_direct_function_symbol_scope_for_persistent(&remapped_id);
        assert_eq!(
            cached_scope, remapped_scope,
            "precompiled symbols must survive module id remapping after cache load"
        );
        assert_ne!(
            cached_scope,
            precompiled_direct_function_symbol_scope_for_persistent(
                &persistent_function_id_for_module_function(
                    "pkg.mod",
                    0x1234,
                    LocalFunctionId::new(8),
                )
            ),
            "distinct logical function ids need distinct direct entry symbols"
        );
        assert_ne!(
            cached_scope,
            precompiled_direct_function_symbol_scope_for_persistent(
                &persistent_function_id_for_module_function(
                    "pkg.mod",
                    0x4321,
                    LocalFunctionId::new(7),
                )
            ),
            "distinct source hashes need distinct direct entry symbols"
        );
        assert_ne!(
            cached_scope,
            precompiled_direct_function_symbol_scope_for_persistent(
                &persistent_function_id_for_module_function(
                    "other.mod",
                    0x1234,
                    LocalFunctionId::new(7),
                )
            ),
            "distinct module names need distinct direct entry symbols"
        );
    }

    #[test]
    fn precompile_codegen_module_emits_relocatable_object() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def add(a, b):
    return a + b
"#,
        )
        .expect("lowering precompile smoke source should succeed")
        .codegen_module;

        let object = precompile_codegen_module_to_object_bytes(
            "precompile_smoke",
            0x1234,
            &lowered,
            None,
            None,
            None,
        )
        .expect("precompile should emit object bytes");
        assert!(
            object.function_count >= lowered.callable_defs.len(),
            "object should contain generated functions plus runtime support"
        );
        assert!(
            object.data_object_count > 0,
            "object should contain module constant slot data objects"
        );
        assert_eq!(
            object.object.get(0..4),
            Some(b"\x7fELF".as_slice()),
            "precompiled bytes should start with an ELF header"
        );
        assert!(
            object
                .function_symbols
                .iter()
                .any(|symbol| symbol.contains("py:d:add")),
            "precompiled object should define the direct add function"
        );
        assert!(
            object
                .data_symbols
                .iter()
                .any(|symbol| symbol.starts_with("__soac_module_constant_shared_")),
            "precompiled object should define module constant slot symbols"
        );
        for section in [
            ".eh_frame",
            ".rela.eh_frame",
            ".debug_info",
            ".rela.debug_info",
            ".debug_abbrev",
            ".debug_line",
            ".rela.debug_line",
            ".debug_str",
        ] {
            assert!(
                object
                    .object
                    .windows(section.len())
                    .any(|window| window == section.as_bytes()),
                "precompiled object should contain {section}"
            );
        }
    }

    #[test]
    fn precompile_codegen_module_emits_static_pylong_in_rodata() {
        crate::initialize_test_python();
        let module_name = "precompile_static_int";
        let source_hash = 0x5678;
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def get_value():
    return 12345
"#,
        )
        .expect("lowering precompile static int source should succeed")
        .codegen_module;
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&lowered);
        let constant_id = module_constants.require_int_constant_id(12345);
        let symbol_prefix =
            module_constant_symbol_prefix_for_module_identity(module_name, source_hash);
        let constant_symbol = module_constant_object_symbol(symbol_prefix.as_str(), constant_id);

        let object = precompile_codegen_module_to_object_bytes(
            module_name,
            source_hash,
            &lowered,
            None,
            None,
            None,
        )
        .expect("precompile should emit object bytes");

        assert!(
            object
                .data_symbol_writable
                .iter()
                .any(|(symbol, writable)| symbol == &constant_symbol && !*writable),
            "static compact PyLong constants should be emitted as read-only object data"
        );
        assert!(
            object
                .object
                .windows(b".rela.rodata".len())
                .any(|window| window == b".rela.rodata"),
            "static PyLong object data should carry a relocation for PyLong_Type"
        );
    }

    #[test]
    fn precompile_codegen_module_emits_static_big_pylong_in_rodata() {
        crate::initialize_test_python();
        let module_name = "precompile_static_big_int";
        let source_hash = 0x6677;
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def get_value():
    return 123456789012345678901234567890
"#,
        )
        .expect("lowering precompile static big int source should succeed")
        .codegen_module;
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&lowered);
        let constant_id =
            module_constants.require_big_int_constant_id("123456789012345678901234567890");
        let symbol_prefix =
            module_constant_symbol_prefix_for_module_identity(module_name, source_hash);
        let constant_symbol = module_constant_object_symbol(symbol_prefix.as_str(), constant_id);

        let object = precompile_codegen_module_to_object_bytes(
            module_name,
            source_hash,
            &lowered,
            None,
            None,
            None,
        )
        .expect("precompile should emit object bytes");

        assert!(
            object
                .data_symbol_writable
                .iter()
                .any(|(symbol, writable)| symbol == &constant_symbol && !*writable),
            "static big PyLong constants should be emitted as read-only object data"
        );
        assert!(
            object
                .object
                .windows(b".rela.rodata".len())
                .any(|window| window == b".rela.rodata"),
            "static big PyLong object data should carry a relocation for PyLong_Type"
        );
    }

    #[test]
    fn precompile_codegen_module_emits_static_compact_ascii_unicode_in_data() {
        let module_name = "precompile_static_ascii";
        let source_hash = 0x6789;
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def get_value():
    return "ascii-value"
"#,
        )
        .expect("lowering precompile static ASCII source should succeed")
        .codegen_module;
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&lowered);
        let constant_id = module_constants.require_unicode_constant_id("ascii-value");
        let symbol_prefix =
            module_constant_symbol_prefix_for_module_identity(module_name, source_hash);
        let constant_symbol = module_constant_object_symbol(symbol_prefix.as_str(), constant_id);

        let object = precompile_codegen_module_to_object_bytes(
            module_name,
            source_hash,
            &lowered,
            None,
            None,
            None,
        )
        .expect("precompile should emit object bytes");

        assert!(
            object
                .data_symbol_writable
                .iter()
                .any(|(symbol, writable)| symbol == &constant_symbol && *writable),
            "static compact ASCII Unicode constants should be emitted as writable object data"
        );
        assert!(
            object
                .object
                .windows(b".rela.data".len())
                .any(|window| window == b".rela.data"),
            "static Unicode object data should carry a writable-data relocation for PyUnicode_Type"
        );
    }

    #[test]
    fn precompile_codegen_module_emits_static_compact_non_ascii_unicode_in_data() {
        crate::initialize_test_python();
        let module_name = "precompile_static_non_ascii";
        let source_hash = 0x7788;
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def get_value():
    return "caf\u00e9 \U0001f40d"
"#,
        )
        .expect("lowering precompile static non-ASCII source should succeed")
        .codegen_module;
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&lowered);
        let constant_id = module_constants.require_unicode_constant_id("caf\u{e9} \u{1f40d}");
        let symbol_prefix =
            module_constant_symbol_prefix_for_module_identity(module_name, source_hash);
        let constant_symbol = module_constant_object_symbol(symbol_prefix.as_str(), constant_id);

        let object = precompile_codegen_module_to_object_bytes(
            module_name,
            source_hash,
            &lowered,
            None,
            None,
            None,
        )
        .expect("precompile should emit object bytes");

        assert!(
            object
                .data_symbol_writable
                .iter()
                .any(|(symbol, writable)| symbol == &constant_symbol && *writable),
            "static compact non-ASCII Unicode constants should be emitted as writable object data"
        );
        assert!(
            object
                .object
                .windows(b".rela.data".len())
                .any(|window| window == b".rela.data"),
            "static Unicode object data should carry a writable-data relocation for PyUnicode_Type"
        );
    }

    #[test]
    fn blockpy_entry_interpreter_binds_positional_args() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def add(left, right):
    return left + right
"#,
            )
            .expect("lowering entry interpreter smoke source should succeed")
            .codegen_module;
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "entry_test", "")
                    .expect("shared state should build for entry interpreter smoke test");
            let function = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "add")
                .expect("lowered module should contain add function");
            let entry_plan = RuntimeFunctionEntryPlan::from_function(function)
                .expect("entry interpreter plan should build");
            let left = ffi::PyLong_FromLong(123_456_789);
            assert!(!left.is_null(), "test left allocation should succeed");
            let right = ffi::PyLong_FromLong(987_654_321);
            assert!(!right.is_null(), "test right allocation should succeed");
            let before_left = ffi::Py_REFCNT(left);
            let before_right = ffi::Py_REFCNT(right);
            let args = [left.cast::<c_void>(), right.cast::<c_void>()];
            let context = BlockPyEntryRuntimeContext::new(
                std::sync::Arc::new(crate::session::CompileSession::new()),
                std::sync::Arc::clone(&shared_state),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &entry_plan,
            );
            let result = run_blockpy_function_from_entry(function, context, &args)
                .expect("entry interpreter should run simple positional function");

            assert_eq!(
                ffi::PyLong_AsLongLong(result.cast::<ffi::PyObject>()),
                1_111_111_110,
                "entry interpreter should execute the function body"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful entry interpreter run should not leave a Python exception"
            );
            assert_eq!(
                ffi::Py_REFCNT(left),
                before_left,
                "entry interpreter should release its owned frame reference to left"
            );
            assert_eq!(
                ffi::Py_REFCNT(right),
                before_right,
                "entry interpreter should release its owned frame reference to right"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(right);
            ffi::Py_DECREF(left);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_reports_argument_binding_gaps() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def needs_arg(value):
    return value
"#,
            )
            .expect("lowering entry interpreter missing-arg source should succeed")
            .codegen_module;
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "entry_test", "")
                    .expect("shared state should build for entry interpreter missing-arg test");
            let function = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "needs_arg")
                .expect("lowered module should contain needs_arg function");
            let entry_plan = RuntimeFunctionEntryPlan::from_function(function)
                .expect("entry interpreter plan should build");
            let context = BlockPyEntryRuntimeContext::new(
                std::sync::Arc::new(crate::session::CompileSession::new()),
                std::sync::Arc::clone(&shared_state),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &entry_plan,
            );
            let result = unsafe { run_blockpy_function_from_entry(function, context, &[]) }.expect(
                "entry interpreter should handle missing positional args as a Python call error",
            );

            assert!(
                result.is_null(),
                "entry interpreter should return null for Python argument binding errors"
            );
            assert!(
                unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError) } != 0,
                "missing positional arg should raise TypeError"
            );
            unsafe { ffi::PyErr_Clear() };
        });
    }

    #[test]
    fn blockpy_entry_interpreter_binds_positional_defaults_from_function_data() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def add_default(left, right=9):
    return left + right
"#,
            )
            .expect("lowering entry interpreter default source should succeed")
            .codegen_module;
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "entry_test", "")
                    .expect("shared state should build for entry interpreter default test");
            let function = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "add_default")
                .expect("lowered module should contain add_default function");
            let runtime_layout = FunctionRuntimeDataLayout::from_function(function);
            let mut function_data =
                vec![std::ptr::null_mut::<ffi::PyObject>(); runtime_layout.total_len()];
            let default_slot = runtime_layout
                .positional_default_slot_for_param_index(1)
                .expect("right param should have a runtime default slot");
            let default = ffi::PyLong_FromLong(9);
            assert!(!default.is_null(), "test default allocation should succeed");
            function_data[default_slot] = default;
            let left = ffi::PyLong_FromLong(33);
            assert!(!left.is_null(), "test left allocation should succeed");
            let args = [left.cast::<c_void>()];
            let entry_plan = RuntimeFunctionEntryPlan::from_function(function)
                .expect("entry interpreter plan should build");
            let context = BlockPyEntryRuntimeContext::new(
                std::sync::Arc::new(crate::session::CompileSession::new()),
                std::sync::Arc::clone(&shared_state),
                std::ptr::null_mut(),
                function_data.as_mut_ptr().cast::<c_void>(),
                &entry_plan,
            );

            let result = run_blockpy_function_from_entry(function, context, &args)
                .expect("entry interpreter should bind missing positional arg from function data");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                42,
                "entry interpreter should use the runtime default for right"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful entry interpreter run should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(left);
            ffi::Py_DECREF(default);
        });
    }

    unsafe fn run_named_blockpy_entry_for_test(
        py: Python<'_>,
        source: &str,
        function_name: &str,
        globals_obj: ObjPtr,
        positional_args: &[ObjPtr],
    ) -> Result<ObjPtr, String> {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .map_err(|err| format!("lowering entry interpreter source failed: {err}"))?
            .codegen_module;
        let module_code = compile_original_module_code_for_test(py, source)
            .map_err(|err| format!("compiling original entry interpreter source failed: {err}"))?;
        let original_code_by_function_id =
            match_original_code_to_functions_for_test(py, module_code.bind(py), &lowered).map_err(
                |err| format!("mapping original code to lowered functions failed: {err}"),
            )?;
        let shared_state = crate::module_type::build_shared_state_for_testing_with_original_code(
            py,
            lowered,
            "entry_test",
            "",
            original_code_by_function_id,
        )
        .map_err(|err| format!("building entry interpreter shared state failed: {err}"))?;
        let function = shared_state
            .lowered_module
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == function_name)
            .ok_or_else(|| format!("lowered module should contain function {function_name:?}"))?;
        let entry_plan = RuntimeFunctionEntryPlan::from_function(function)
            .expect("entry interpreter plan should build");
        let context = BlockPyEntryRuntimeContext::new(
            std::sync::Arc::new(crate::session::CompileSession::new()),
            std::sync::Arc::clone(&shared_state),
            globals_obj,
            std::ptr::null_mut(),
            &entry_plan,
        );
        unsafe { run_blockpy_function_from_entry(function, context, positional_args) }
    }

    unsafe fn entry_test_globals(py: Python<'_>) -> *mut ffi::PyObject {
        let globals = unsafe { ffi::PyDict_New() };
        assert!(!globals.is_null(), "test globals dict should allocate");
        let builtins = py.import("builtins").expect("builtins should import");
        assert_eq!(
            unsafe {
                ffi::PyDict_SetItemString(globals, c"__builtins__".as_ptr(), builtins.as_ptr())
            },
            0,
            "test globals should store builtins"
        );
        let module_name = unsafe { ffi::PyUnicode_FromStringAndSize(c"entry_test".as_ptr(), 10) };
        assert!(!module_name.is_null(), "test module name should allocate");
        assert_eq!(
            unsafe { ffi::PyDict_SetItemString(globals, c"__name__".as_ptr(), module_name) },
            0,
            "test globals should store __name__"
        );
        unsafe { ffi::Py_DECREF(module_name) };
        globals
    }

    unsafe fn entry_test_kwnames(names: &[&str]) -> *mut ffi::PyObject {
        let tuple = ffi::PyTuple_New(names.len() as ffi::Py_ssize_t);
        assert!(!tuple.is_null(), "kwnames tuple should allocate");
        for (index, name) in names.iter().enumerate() {
            let key = ffi::PyUnicode_FromStringAndSize(
                name.as_ptr().cast(),
                name.len() as ffi::Py_ssize_t,
            );
            assert!(!key.is_null(), "keyword name should allocate");
            assert_eq!(
                ffi::PyTuple_SetItem(tuple, index as ffi::Py_ssize_t, key),
                0,
                "keyword name should insert into kwnames tuple"
            );
        }
        tuple
    }

    unsafe fn entry_test_tuple<'py>(
        py: Python<'py>,
        values: &[*mut ffi::PyObject],
    ) -> Bound<'py, PyTuple> {
        let tuple = ffi::PyTuple_New(values.len() as ffi::Py_ssize_t);
        assert!(!tuple.is_null(), "test tuple should allocate");
        for (index, value) in values.iter().copied().enumerate() {
            assert!(!value.is_null(), "test tuple value should be non-null");
            ffi::Py_INCREF(value);
            assert_eq!(
                ffi::PyTuple_SetItem(tuple, index as ffi::Py_ssize_t, value),
                0,
                "test tuple item should insert"
            );
        }
        Bound::from_owned_ptr(py, tuple)
            .cast_into::<PyTuple>()
            .expect("test tuple should cast")
    }

    unsafe fn entry_test_int_tuple<'py>(py: Python<'py>, values: &[i64]) -> Bound<'py, PyTuple> {
        let tuple = ffi::PyTuple_New(values.len() as ffi::Py_ssize_t);
        assert!(!tuple.is_null(), "test int tuple should allocate");
        for (index, value) in values.iter().copied().enumerate() {
            let item = ffi::PyLong_FromLongLong(value);
            assert!(!item.is_null(), "test int tuple value should allocate");
            if ffi::PyTuple_SetItem(tuple, index as ffi::Py_ssize_t, item) != 0 {
                ffi::Py_DECREF(item);
                ffi::Py_DECREF(tuple);
                panic!("test int tuple item should insert");
            }
        }
        Bound::from_owned_ptr(py, tuple)
            .cast_into::<PyTuple>()
            .expect("test int tuple should cast")
    }

    type OriginalCodeByQualname = HashMap<String, VecDeque<Py<PyAny>>>;
    type OriginalCodeMap = HashMap<RuntimeFunctionId, Py<PyAny>>;

    fn compile_original_module_code_for_test(py: Python<'_>, source: &str) -> PyResult<Py<PyAny>> {
        let code = PyModule::import(py, "builtins")?
            .getattr("compile")?
            .call1((source, "<entry_test>", "exec"))?;
        Ok(code.unbind())
    }

    fn collect_original_code_objects_for_test(
        code: &Bound<'_, PyAny>,
        code_type: &Bound<'_, PyAny>,
        by_qualname: &mut OriginalCodeByQualname,
    ) -> PyResult<()> {
        let qualname = code.getattr("co_qualname")?.extract::<String>()?;
        by_qualname
            .entry(qualname)
            .or_default()
            .push_back(code.clone().unbind());

        let consts = code.getattr("co_consts")?;
        let const_count = unsafe { ffi::PyTuple_Size(consts.as_ptr()) };
        if const_count < 0 {
            return Err(PyErr::fetch(code.py()));
        }
        for index in 0..const_count {
            let item = unsafe { ffi::PyTuple_GetItem(consts.as_ptr(), index) };
            if item.is_null() {
                return Err(PyErr::fetch(code.py()));
            }
            let item = unsafe { Bound::from_borrowed_ptr(code.py(), item) };
            if item.is_instance(code_type)? {
                collect_original_code_objects_for_test(&item, code_type, by_qualname)?;
            }
        }
        Ok(())
    }

    fn is_synthetic_class_helper_for_original_code(
        function: &BlockPyFunction<CodegenModuleShape>,
    ) -> bool {
        function.names.bind_name.starts_with("_dp_class_ns_")
            || function.names.bind_name.starts_with("_dp_define_class_")
    }

    fn original_code_lookup_key_for_test(
        function: &BlockPyFunction<CodegenModuleShape>,
    ) -> Option<&str> {
        if function.execution_mode() == FunctionExecutionMode::Interpreted {
            return None;
        }
        let qualname = function.names.qualname.as_str();
        if qualname == "_dp_module_init"
            || function.names.fn_name == "_dp_resume"
            || is_synthetic_class_helper_for_original_code(function)
        {
            return None;
        }
        Some(qualname)
    }

    fn match_original_code_to_functions_for_test(
        py: Python<'_>,
        module_code: &Bound<'_, PyAny>,
        lowered_module: &BlockPyModule<CodegenModuleShape>,
    ) -> PyResult<OriginalCodeMap> {
        let code_type = PyModule::import(py, "types")?.getattr("CodeType")?;
        let mut code_by_qualname = HashMap::new();
        collect_original_code_objects_for_test(module_code, &code_type, &mut code_by_qualname)?;

        let mut code_by_function_id = HashMap::new();
        for function in &lowered_module.callable_defs {
            let Some(qualname) = original_code_lookup_key_for_test(function) else {
                continue;
            };
            let Some(codes) = code_by_qualname.get_mut(qualname) else {
                continue;
            };
            let Some(code) = codes.pop_front() else {
                continue;
            };
            code_by_function_id.insert(function.function_id, code);
        }
        Ok(code_by_function_id)
    }

    unsafe fn run_registered_module_init_entry_for_test<'py>(
        py: Python<'py>,
        source: &str,
    ) -> Bound<'py, PyDict> {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("lowering module init source should succeed")
            .codegen_module;
        let module_code = compile_original_module_code_for_test(py, source)
            .expect("original module source should compile for entry test");
        let original_code_by_function_id =
            match_original_code_to_functions_for_test(py, module_code.bind(py), &lowered)
                .expect("original code should map to lowered entry-test functions");
        let shared_state = crate::module_type::build_shared_state_for_testing_with_original_code(
            py,
            lowered,
            "entry_test",
            "",
            original_code_by_function_id,
        )
        .expect("shared state should build for module init test");
        let function_id = shared_state
            .lowered_module
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "_dp_module_init")
            .expect("lowered module should contain module init")
            .function_id;
        let globals = PyDict::new(py);
        let builtins = py.import("builtins").expect("builtins should import");
        globals
            .set_item("__builtins__", &builtins)
            .expect("module globals should accept builtins");
        globals
            .set_item("__name__", "entry_test")
            .expect("module globals should accept __name__");

        let captures = entry_test_tuple(py, &[]);
        let param_defaults = entry_test_tuple(py, &[]);
        let annotate_fn = py.None();
        let module_init = crate::function_instantiation::make_function_in_shared_state(
            py,
            std::sync::Arc::new(crate::session::CompileSession::new()),
            std::sync::Arc::clone(&shared_state),
            function_id,
            FunctionKind::Function,
            captures.as_any(),
            param_defaults.as_any(),
            annotate_fn.bind(py),
            globals.as_any(),
        )
        .expect("registered module init should instantiate");
        let module_init = module_init.bind(py);
        assert!(
            !crate::PyFunction_GetSoacMetadata(module_init.as_ptr()).is_null(),
            "module init should have SOAC metadata"
        );

        let result = module_init
            .call0()
            .expect("entry-interpreter vectorcall should execute module init");
        assert!(
            result.is_none(),
            "module init should return None through Python call dispatch"
        );
        assert!(
            ffi::PyErr_Occurred().is_null(),
            "successful module init run should not leave a Python exception"
        );
        globals
    }

    #[test]
    fn blockpy_entry_interpreter_uses_registered_function_env() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def add_default(left, right=9):
    return left + right
"#,
            )
            .expect("lowering registered entry source should succeed")
            .codegen_module;
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "entry_test", "")
                    .expect("shared state should build for registered entry test");
            let function_id = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "add_default")
                .expect("lowered module should contain add_default")
                .function_id;
            let module = PyModule::new(py, "entry_test").expect("module should allocate");
            let globals = module.dict();
            let builtins = py.import("builtins").expect("builtins should import");
            globals
                .set_item("__builtins__", &builtins)
                .expect("module globals should accept builtins");
            globals
                .set_item("__name__", "entry_test")
                .expect("module globals should accept __name__");

            let captures = entry_test_tuple(py, &[]);
            let default = ffi::PyLong_FromLong(9);
            assert!(!default.is_null(), "default value should allocate");
            let param_defaults = entry_test_tuple(py, &[default]);
            ffi::Py_DECREF(default);
            let annotate_fn = py.None();
            let function_obj = crate::function_instantiation::make_function_in_shared_state(
                py,
                std::sync::Arc::new(crate::session::CompileSession::new()),
                std::sync::Arc::clone(&shared_state),
                function_id,
                FunctionKind::Function,
                captures.as_any(),
                param_defaults.as_any(),
                annotate_fn.bind(py),
                globals.as_any(),
            )
            .expect("registered function should instantiate");
            let function_obj = function_obj.bind(py);
            assert!(
                !crate::PyFunction_GetSoacMetadata(function_obj.as_ptr()).is_null(),
                "instantiated function should have SOAC metadata"
            );

            let left = ffi::PyLong_FromLong(33);
            assert!(!left.is_null(), "left value should allocate");
            let args = [left];
            let result = crate::run_registered_clif_function_from_vectorcall_entry(
                function_obj.as_ptr(),
                args.as_ptr(),
                args.len(),
                std::ptr::null_mut(),
            )
            .expect("registered entry interpreter should execute");

            assert_eq!(
                ffi::PyLong_AsLong(result),
                42,
                "entry interpreter should read the default from FunctionEnv runtime objects"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful registered entry run should not leave a Python exception"
            );
            ffi::Py_DECREF(result);
            ffi::Py_DECREF(left);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_vectorcall_executes_class_creation() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = run_registered_module_init_entry_for_test(
                py,
                r#"
class C:
    marker = 40

    def method(self):
        return self.marker + 2

RESULT = C().method()
"#,
            );
            let stored_result = globals
                .get_item("RESULT")
                .expect("RESULT lookup should succeed")
                .expect("module init should store RESULT");
            assert_eq!(
                stored_result
                    .extract::<i64>()
                    .expect("RESULT should be int"),
                42,
                "class creation and method call should execute through entry interpreter dispatch"
            );
            let stored_class = globals
                .get_item("C")
                .expect("C lookup should succeed")
                .expect("module init should store class C");
            assert!(
                ffi::PyType_Check(stored_class.as_ptr()) != 0,
                "class creation should store a Python type"
            );
        });
    }

    #[test]
    fn blockpy_entry_interpreter_vectorcall_executes_class_super_closure() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = run_registered_module_init_entry_for_test(
                py,
                r#"
class Base:
    def value(self):
        return 40

class C(Base):
    def value(self):
        return super().value() + 2

RESULT = C().value()
"#,
            );
            let stored_result = globals
                .get_item("RESULT")
                .expect("RESULT lookup should succeed")
                .expect("module init should store RESULT");
            assert_eq!(
                stored_result
                    .extract::<i64>()
                    .expect("RESULT should be int"),
                42,
                "entry interpreter dispatch should preserve __class__ closure for super()"
            );
        });
    }

    #[test]
    fn blockpy_entry_interpreter_vectorcall_executes_decorator_and_metaclass() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = run_registered_module_init_entry_for_test(
                py,
                r#"
def decorate(cls):
    cls.decorated = cls.flag + 1
    return cls

class Meta(type):
    def __new__(mcls, name, bases, ns, **kw):
        cls = type.__new__(mcls, name, bases, ns)
        cls.flag = kw["flag"]
        return cls

@decorate
class C(metaclass=Meta, flag=41):
    pass

RESULT = C.decorated
"#,
            );
            let stored_result = globals
                .get_item("RESULT")
                .expect("RESULT lookup should succeed")
                .expect("module init should store RESULT");
            assert_eq!(
                stored_result
                    .extract::<i64>()
                    .expect("RESULT should be int"),
                42,
                "entry interpreter dispatch should handle decorators and metaclass kwargs"
            );
        });
    }

    #[test]
    fn blockpy_entry_interpreter_vectorcall_preserves_generator_call_semantics() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = run_registered_module_init_entry_for_test(
                py,
                r#"
def gen():
    yield 40
    yield 2

RESULT = list(gen())
"#,
            );
            let stored_result = globals
                .get_item("RESULT")
                .expect("RESULT lookup should succeed")
                .expect("module init should store RESULT");
            assert_eq!(
                stored_result
                    .extract::<Vec<i64>>()
                    .expect("RESULT should be list[int]"),
                vec![40, 2],
                "forced entry dispatch should leave generator calls as generator-object creation"
            );
        });
    }

    #[test]
    fn blockpy_entry_interpreter_vectorcall_preserves_coroutine_call_semantics() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = run_registered_module_init_entry_for_test(
                py,
                r#"
async def coro():
    return 42

OBJ = coro()
RESULT = hasattr(OBJ, "__await__")
OBJ.close()
"#,
            );
            let stored_result = globals
                .get_item("RESULT")
                .expect("RESULT lookup should succeed")
                .expect("module init should store RESULT");
            assert!(
                stored_result
                    .extract::<bool>()
                    .expect("RESULT should be bool"),
                "forced entry dispatch should leave coroutine calls as coroutine-object creation"
            );
        });
    }

    #[test]
    fn blockpy_entry_interpreter_vectorcall_preserves_async_generator_call_semantics() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = run_registered_module_init_entry_for_test(
                py,
                r#"
async def agen():
    yield 42

OBJ = agen()
RESULT = hasattr(OBJ, "__anext__")
"#,
            );
            let stored_result = globals
                .get_item("RESULT")
                .expect("RESULT lookup should succeed")
                .expect("module init should store RESULT");
            assert!(
                stored_result
                    .extract::<bool>()
                    .expect("RESULT should be bool"),
                "forced entry dispatch should leave async-generator calls as async-generator object creation"
            );
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_module_init_globals() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let globals = ffi::PyDict_New();
            assert!(!globals.is_null(), "test globals dict should allocate");
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
VALUE = 41

def add_one(value):
    return value + 1

RESULT = add_one(VALUE)
"#,
                "_dp_module_init",
                globals.cast(),
                &[],
            )
            .expect("entry interpreter should execute module init");

            assert_eq!(
                result,
                ffi::Py_None().cast(),
                "module init should return owned None"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful module init should not leave a Python exception"
            );
            let stored_result = ffi::PyDict_GetItemString(globals, c"RESULT".as_ptr());
            assert!(
                !stored_result.is_null(),
                "module init should store RESULT in globals"
            );
            assert_eq!(
                ffi::PyLong_AsLong(stored_result),
                42,
                "module init should call the nested function through globals"
            );
            let stored_function = ffi::PyDict_GetItemString(globals, c"add_one".as_ptr());
            assert!(
                !stored_function.is_null(),
                "module init should store the nested function in globals"
            );
            assert!(
                ffi::PyCallable_Check(stored_function) != 0,
                "stored nested function should be callable"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_attr_and_item_mutation() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let obj = ffi::PyModule_New(c"entry_attr_item_target".as_ptr());
            assert!(!obj.is_null(), "test module object should allocate");
            let data = ffi::PyDict_New();
            assert!(!data.is_null(), "test data dict should allocate");
            let start = ffi::PyLong_FromLong(10);
            assert!(!start.is_null(), "test start value should allocate");
            assert_eq!(
                ffi::PyDict_SetItemString(data, c"start".as_ptr(), start),
                0,
                "test dict setup should store start"
            );

            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def mutate(obj, data):
    obj.value = data["start"]
    data["next"] = obj.value + 1
    del data["start"]
    return obj.value, data["next"], "start" in data
"#,
                "mutate",
                std::ptr::null_mut(),
                &[obj.cast(), data.cast()],
            )
            .expect("entry interpreter should execute attr/item mutation");

            if result.is_null() {
                ffi::PyErr_Print();
                panic!("entry interpreter attr/item mutation should produce a result");
            }
            assert!(
                ffi::PyTuple_Check(result.cast::<ffi::PyObject>()) != 0,
                "mutation result should be a tuple"
            );
            assert_eq!(
                ffi::PyTuple_Size(result.cast::<ffi::PyObject>()),
                3,
                "mutation result should have three values"
            );
            let first = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 0);
            let second = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 1);
            let third = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 2);
            assert_eq!(
                ffi::PyLong_AsLong(first),
                10,
                "entry interpreter should return the assigned attribute"
            );
            assert_eq!(
                ffi::PyLong_AsLong(second),
                11,
                "entry interpreter should return the updated item"
            );
            assert_eq!(
                third,
                ffi::Py_False(),
                "entry interpreter should observe the deleted item as absent"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful attr/item mutation should not leave a Python exception"
            );
            let stored_attr = ffi::PyObject_GetAttrString(obj, c"value".as_ptr());
            assert!(!stored_attr.is_null(), "mutate should store obj.value");
            assert_eq!(
                ffi::PyLong_AsLong(stored_attr),
                10,
                "stored obj.value should match data['start']"
            );
            assert!(
                ffi::PyDict_GetItemString(data, c"start".as_ptr()).is_null(),
                "mutate should delete data['start']"
            );
            assert!(
                !ffi::PyDict_GetItemString(data, c"next".as_ptr()).is_null(),
                "mutate should store data['next']"
            );
            ffi::Py_DECREF(stored_attr);
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(start);
            ffi::Py_DECREF(data);
            ffi::Py_DECREF(obj);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_local_store_and_tuple_return() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let input = ffi::PyLong_FromLong(41);
            assert!(!input.is_null(), "test input allocation should succeed");
            let args = [input.cast::<c_void>()];
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def build(value):
    next_value = value + 1
    return (next_value, value)
"#,
                "build",
                std::ptr::null_mut(),
                &args,
            )
            .expect("entry interpreter should execute local store and tuple return");

            assert_eq!(
                ffi::PyTuple_Size(result.cast::<ffi::PyObject>()),
                2,
                "entry interpreter should return a 2-tuple"
            );
            let first = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 0);
            let second = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 1);
            assert_eq!(
                ffi::PyLong_AsLong(first),
                42,
                "entry interpreter should store and return the computed local"
            );
            assert_eq!(
                ffi::PyLong_AsLong(second),
                41,
                "entry interpreter should preserve the original arg local"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful entry interpreter run should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(input);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_branch_and_global_load() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let globals = ffi::PyDict_New();
            assert!(!globals.is_null(), "test globals allocation should succeed");
            let value = ffi::PyLong_FromLong(40);
            assert!(
                !value.is_null(),
                "test global value allocation should succeed"
            );
            assert_eq!(
                ffi::PyDict_SetItemString(globals, c"VALUE".as_ptr(), value),
                0,
                "test global value should insert"
            );
            let flag = ffi::PyBool_FromLong(1);
            assert!(!flag.is_null(), "test flag allocation should succeed");
            let args = [flag.cast::<c_void>()];
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def choose(flag):
    if flag:
        return VALUE + 2
    return 5
"#,
                "choose",
                globals.cast(),
                &args,
            )
            .expect("entry interpreter should execute branch and global load");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                42,
                "entry interpreter should take the true branch and load VALUE from globals"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful entry interpreter run should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(flag);
            ffi::Py_DECREF(value);
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_global_keyword_call() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let helper_module = PyModule::from_code(
                py,
                c"
def helper(value, scale=1):
    return value * scale + 7
",
                c"entry_helper.py",
                c"entry_helper",
            )
            .expect("helper module should execute");
            let helper = helper_module
                .getattr("helper")
                .expect("helper function should exist");
            let globals = ffi::PyDict_New();
            assert!(!globals.is_null(), "test globals allocation should succeed");
            assert_eq!(
                ffi::PyDict_SetItemString(globals, c"helper".as_ptr(), helper.as_ptr()),
                0,
                "helper should insert into globals"
            );
            let input = ffi::PyLong_FromLong(11);
            assert!(!input.is_null(), "test input allocation should succeed");
            let args = [input.cast::<c_void>()];
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def call_helper(value):
    return helper(value, scale=3)
"#,
                "call_helper",
                globals.cast(),
                &args,
            )
            .expect("entry interpreter should execute global keyword call");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                40,
                "entry interpreter should call the global helper with a keyword"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful entry interpreter run should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(input);
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_binds_vectorcall_varargs_kwonly_and_kwargs() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def shaped(a, /, b, *args, c, **kwargs):
    return (a, b, args, c, kwargs["extra"])
"#,
            )
            .expect("lowering vectorcall entry source should succeed")
            .codegen_module;
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "entry_test", "")
                    .expect("shared state should build for vectorcall entry test");
            let function = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "shaped")
                .expect("lowered module should contain shaped function");
            let entry_plan = RuntimeFunctionEntryPlan::from_function(function)
                .expect("entry interpreter plan should build");
            let context = BlockPyEntryRuntimeContext::new(
                std::sync::Arc::new(crate::session::CompileSession::new()),
                std::sync::Arc::clone(&shared_state),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &entry_plan,
            );
            let a = ffi::PyLong_FromLong(1);
            let b = ffi::PyLong_FromLong(2);
            let extra_positional = ffi::PyLong_FromLong(3);
            let c = ffi::PyLong_FromLong(4);
            let extra = ffi::PyLong_FromLong(5);
            assert!(!a.is_null() && !b.is_null() && !extra_positional.is_null());
            assert!(!c.is_null() && !extra.is_null());
            let args = [
                a.cast::<c_void>(),
                b.cast::<c_void>(),
                extra_positional.cast::<c_void>(),
                c.cast::<c_void>(),
                extra.cast::<c_void>(),
            ];
            let kwnames = entry_test_kwnames(&["c", "extra"]);

            let result = run_blockpy_function_from_vectorcall_entry(
                function,
                context,
                args.as_ptr(),
                3,
                kwnames.cast(),
            )
            .expect("entry interpreter should bind vectorcall-shaped arguments");

            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful vectorcall entry run should not leave a Python exception"
            );
            assert_eq!(
                ffi::PyTuple_Size(result.cast::<ffi::PyObject>()),
                5,
                "entry interpreter should return the shaped argument tuple"
            );
            let item0 = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 0);
            let item1 = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 1);
            let varargs = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 2);
            let item3 = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 3);
            let item4 = ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 4);
            assert_eq!(ffi::PyLong_AsLong(item0), 1);
            assert_eq!(ffi::PyLong_AsLong(item1), 2);
            assert_eq!(ffi::PyTuple_Size(varargs), 1);
            assert_eq!(ffi::PyLong_AsLong(ffi::PyTuple_GetItem(varargs, 0)), 3);
            assert_eq!(ffi::PyLong_AsLong(item3), 4);
            assert_eq!(ffi::PyLong_AsLong(item4), 5);
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(kwnames);
            ffi::Py_DECREF(extra);
            ffi::Py_DECREF(c);
            ffi::Py_DECREF(extra_positional);
            ffi::Py_DECREF(b);
            ffi::Py_DECREF(a);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_binds_kwonly_defaults_from_function_data() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def add_kw_default(value, *, scale=9):
    return value + scale
"#,
            )
            .expect("lowering keyword-default entry source should succeed")
            .codegen_module;
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "entry_test", "")
                    .expect("shared state should build for keyword-default entry test");
            let function = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "add_kw_default")
                .expect("lowered module should contain add_kw_default function");
            let runtime_layout = FunctionRuntimeDataLayout::from_function(function);
            let mut function_data =
                vec![std::ptr::null_mut::<ffi::PyObject>(); runtime_layout.total_len()];
            let default_slot = runtime_layout
                .kwonly_default_slot("scale")
                .expect("scale should have a runtime kwonly default slot");
            let default = ffi::PyLong_FromLong(9);
            assert!(!default.is_null(), "test kwonly default should allocate");
            function_data[default_slot] = default;
            let input = ffi::PyLong_FromLong(33);
            assert!(!input.is_null(), "test input should allocate");
            let args = [input.cast::<c_void>()];
            let entry_plan = RuntimeFunctionEntryPlan::from_function(function)
                .expect("entry interpreter plan should build");
            let context = BlockPyEntryRuntimeContext::new(
                std::sync::Arc::new(crate::session::CompileSession::new()),
                std::sync::Arc::clone(&shared_state),
                std::ptr::null_mut(),
                function_data.as_mut_ptr().cast::<c_void>(),
                &entry_plan,
            );

            let result = run_blockpy_function_from_vectorcall_entry(
                function,
                context,
                args.as_ptr(),
                1,
                std::ptr::null_mut(),
            )
            .expect("entry interpreter should bind keyword-only default from function data");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                42,
                "entry interpreter should use kwonly runtime default"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful kwonly default run should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(input);
            ffi::Py_DECREF(default);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_reports_duplicate_vectorcall_argument() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def takes_one(value):
    return value
"#,
            )
            .expect("lowering duplicate vectorcall entry source should succeed")
            .codegen_module;
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "entry_test", "")
                    .expect("shared state should build for duplicate vectorcall entry test");
            let function = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "takes_one")
                .expect("lowered module should contain takes_one function");
            let entry_plan = RuntimeFunctionEntryPlan::from_function(function)
                .expect("entry interpreter plan should build");
            let context = BlockPyEntryRuntimeContext::new(
                std::sync::Arc::new(crate::session::CompileSession::new()),
                std::sync::Arc::clone(&shared_state),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &entry_plan,
            );
            let positional = ffi::PyLong_FromLong(1);
            let keyword = ffi::PyLong_FromLong(2);
            assert!(!positional.is_null() && !keyword.is_null());
            let args = [positional.cast::<c_void>(), keyword.cast::<c_void>()];
            let kwnames = entry_test_kwnames(&["value"]);

            let result = run_blockpy_function_from_vectorcall_entry(
                function,
                context,
                args.as_ptr(),
                1,
                kwnames.cast(),
            )
            .expect("duplicate vectorcall argument should be reported as a Python call error");

            assert!(
                result.is_null(),
                "duplicate vectorcall argument should return null"
            );
            assert!(
                ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError) != 0,
                "duplicate vectorcall argument should raise TypeError"
            );
            ffi::PyErr_Clear();
            ffi::Py_DECREF(kwnames);
            ffi::Py_DECREF(keyword);
            ffi::Py_DECREF(positional);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_nested_function_with_closure() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let globals = ffi::PyDict_New();
            assert!(!globals.is_null(), "test globals allocation should succeed");
            let input = ffi::PyLong_FromLong(37);
            assert!(!input.is_null(), "test input allocation should succeed");
            let args = [input.cast::<c_void>()];
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def outer(x):
    def inner(y):
        return x + y
    return inner(5)
"#,
                "outer",
                globals.cast(),
                &args,
            )
            .expect("entry interpreter should instantiate and call a nested closure");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                42,
                "entry interpreter should preserve closure capture through nested function call"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful entry interpreter run should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(input);
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_catches_raised_exception() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def catch_value_error():
    try:
        raise ValueError("boom")
    except ValueError:
        return 42
"#,
                "catch_value_error",
                std::ptr::null_mut(),
                &[],
            )
            .expect("entry interpreter should execute try/except");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                42,
                "entry interpreter should dispatch raised ValueError to the except handler"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "caught exception should not remain active after entry execution"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
        });
    }

    #[test]
    fn blockpy_entry_interpreter_reraises_current_exception() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def reraise_value_error():
    try:
        raise ValueError("boom")
    except ValueError:
        raise
"#,
                "reraise_value_error",
                std::ptr::null_mut(),
                &[],
            )
            .expect("entry interpreter should execute bare reraise");

            assert!(
                result.is_null(),
                "bare reraise should propagate the active exception"
            );
            assert!(
                ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) != 0,
                "bare reraise should restore the caught ValueError"
            );
            ffi::PyErr_Clear();
        });
    }

    #[test]
    fn blockpy_entry_interpreter_runs_finally_before_return() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def return_through_finally():
    value = 40
    try:
        return value
    finally:
        value = 99
"#,
                "return_through_finally",
                std::ptr::null_mut(),
                &[],
            )
            .expect("entry interpreter should execute try/finally return dispatch");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                40,
                "finally should run but preserve the original return value"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful try/finally return should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
        });
    }

    #[test]
    fn blockpy_entry_interpreter_finally_return_overrides_exception() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def finally_overrides_exception():
    try:
        raise ValueError("boom")
    finally:
        return 42
"#,
                "finally_overrides_exception",
                std::ptr::null_mut(),
                &[],
            )
            .expect("entry interpreter should let finally return suppress an exception");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                42,
                "return from finally should suppress the pending ValueError"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "suppressed exception should not remain active after finally return"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
        });
    }

    #[test]
    fn blockpy_entry_interpreter_preserves_exception_through_finally() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def exception_through_finally():
    marker = 0
    try:
        try:
            raise ValueError("boom")
        finally:
            marker = 40
    except ValueError:
        return marker + 2
"#,
                "exception_through_finally",
                std::ptr::null_mut(),
                &[],
            )
            .expect("entry interpreter should propagate an exception through finally");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                42,
                "finally should run while preserving the exception for the outer handler"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "handled exception-through-finally should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
        });
    }

    #[test]
    fn blockpy_entry_interpreter_runs_finally_before_loop_break() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def break_through_finally():
    total = 0
    for value in (1, 2, 3):
        try:
            break
        finally:
            total = total + 40
    return total + value
"#,
                "break_through_finally",
                std::ptr::null_mut(),
                &[],
            )
            .expect("entry interpreter should run finally before loop break");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                41,
                "finally should run once before the break leaves the loop"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "break-through-finally should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
        });
    }

    #[test]
    fn blockpy_entry_interpreter_runs_finally_before_loop_continue() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def continue_through_finally():
    total = 0
    for value in (1, 2, 3):
        try:
            if value == 2:
                continue
            total = total + value
        finally:
            total = total + 10
    return total
"#,
                "continue_through_finally",
                std::ptr::null_mut(),
                &[],
            )
            .expect("entry interpreter should run finally before loop continue");

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                34,
                "finally should run for normal and continue loop iterations"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "continue-through-finally should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_with_statement_value_flow() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = entry_test_globals(py);
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def use_manager():
    class Manager:
        def __enter__(self):
            return 40

        def __exit__(self, exc_type, exc, tb):
            return False

    with Manager() as value:
        result = value + 2
    return result
"#,
                "use_manager",
                globals.cast(),
                &[],
            )
            .expect("entry interpreter should execute normal with-statement flow");
            if result.is_null() {
                ffi::PyErr_Print();
                panic!("normal with-statement execution returned null");
            }

            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                42,
                "entry interpreter should bind the __enter__ value and leave through __exit__"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "normal with-statement execution should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_with_statement_exception_suppression() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = entry_test_globals(py);
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def suppress_with_exception():
    class Manager:
        def __enter__(self):
            return self

        def __exit__(self, exc_type, exc, tb):
            self.saw_value_error = exc_type is ValueError
            return True

    manager = Manager()
    with manager:
        raise ValueError("boom")
    return manager.saw_value_error
"#,
                "suppress_with_exception",
                globals.cast(),
                &[],
            )
            .expect("entry interpreter should execute with-statement exception suppression");
            if result.is_null() {
                ffi::PyErr_Print();
                panic!("with-statement exception suppression returned null");
            }

            assert_eq!(
                result.cast::<ffi::PyObject>(),
                ffi::Py_True(),
                "entry interpreter should pass the active exception to __exit__ and honor suppression"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "suppressed with-statement exception should not remain active"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_comprehensions_with_captures() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = entry_test_globals(py);
            let input = entry_test_int_tuple(py, &[1, 2, 3]);
            let args = [input.as_ptr().cast()];
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def build(values):
    scale = 2
    odd_list = [value + scale for value in values if value % 2]
    odd_dict = {value: value + scale for value in values if value % 2}
    odd_set = {value + scale for value in values if value % 2}
    return odd_list == [3, 5] and odd_dict == {1: 3, 3: 5} and odd_set == {3, 5}
"#,
                "build",
                globals.cast(),
                &args,
            )
            .expect("entry interpreter should execute comprehension helpers");
            if result.is_null() {
                ffi::PyErr_Print();
                panic!("comprehension execution returned null");
            }

            assert_eq!(
                result.cast::<ffi::PyObject>(),
                ffi::Py_True(),
                "entry interpreter should execute list/dict/set comprehensions with captured locals"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "comprehension execution should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_generator_expression_with_capture() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = entry_test_globals(py);
            let input = entry_test_int_tuple(py, &[1, 2, 3]);
            let args = [input.as_ptr().cast()];
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def build(values):
    scale = 2
    return tuple(value + scale for value in values if value % 2) == (3, 5)
"#,
                "build",
                globals.cast(),
                &args,
            )
            .expect("entry interpreter should execute generator expression");
            if result.is_null() {
                ffi::PyErr_Print();
                panic!("generator expression execution returned null");
            }

            assert_eq!(
                result.cast::<ffi::PyObject>(),
                ffi::Py_True(),
                "entry interpreter should create generator expressions with captured locals"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "generator expression execution should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_import_statements() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = entry_test_globals(py);
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def build():
    import collections as c
    from collections import deque
    values = deque()
    values.append(41)
    return c.deque is deque and values.pop() == 41
"#,
                "build",
                globals.cast(),
                &[],
            )
            .expect("entry interpreter should execute import statements");
            if result.is_null() {
                ffi::PyErr_Print();
                panic!("import statement execution returned null");
            }

            assert_eq!(
                result.cast::<ffi::PyObject>(),
                ffi::Py_True(),
                "entry interpreter should bind import aliases and from-import names"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "import statement execution should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn blockpy_entry_interpreter_executes_for_loop_control_flow() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let _entry_vectorcall = ForceEntryInterpreterVectorcallGuard::new();
        Python::attach(|py| unsafe {
            let globals = entry_test_globals(py);
            let input = entry_test_int_tuple(py, &[1, 2, 3]);
            let args = [input.as_ptr().cast()];
            let result = run_named_blockpy_entry_for_test(
                py,
                r#"
def build(values):
    exhausted = []
    for value in values:
        if value == 2:
            continue
        exhausted.append(value)
    else:
        exhausted.append(99)
    exhausted_last = value

    stopped = []
    for value in values:
        if value == 2:
            continue
        if value == 3:
            break
        stopped.append(value)
    else:
        stopped.append(99)
    stopped_last = value

    return exhausted == [1, 3, 99] and exhausted_last == 3 and stopped == [1] and stopped_last == 3
"#,
                "build",
                globals.cast(),
                &args,
            )
            .expect("entry interpreter should execute for-loop control flow");
            if result.is_null() {
                ffi::PyErr_Print();
                panic!("for-loop execution returned null");
            }

            assert_eq!(
                result.cast::<ffi::PyObject>(),
                ffi::Py_True(),
                "entry interpreter should handle for-loop exhaustion, else, continue, and break"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "for-loop execution should not leave a Python exception"
            );
            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn stored_local_binding_facts_only_require_checks_for_unbound_values() {
        assert_eq!(
            local_binding_facts_for_stored_value(LocalRefKind::Owned),
            ParamBindingFacts::DefinitelyBound
        );
        assert_eq!(
            local_binding_facts_for_stored_value(LocalRefKind::Borrowed),
            ParamBindingFacts::DefinitelyBound
        );
        assert_eq!(
            local_binding_facts_for_stored_value(LocalRefKind::Immortal),
            ParamBindingFacts::DefinitelyBound
        );
        assert_eq!(
            local_binding_facts_for_stored_value(LocalRefKind::Unknown),
            ParamBindingFacts::DefinitelyBound
        );
        assert_eq!(
            local_binding_facts_for_stored_value(LocalRefKind::Unbound),
            ParamBindingFacts::MaybeUnbound
        );
    }

    unsafe extern "C" fn test_capsule_destructor(_capsule: *mut ffi::PyObject) {
        CAPSULE_DESTROYED.store(true, Ordering::SeqCst);
    }

    fn test_name(name: &str) -> ResolvedName {
        test_local_name(name, 0)
    }

    fn test_local_name(name: &str, slot: u32) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::local(slot),
        }
    }

    fn test_global_name(name: &str) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::global(0),
        }
    }

    fn test_runtime_name(name: &str) -> ResolvedName {
        let runtime_name = RuntimeName::from_name(name)
            .unwrap_or_else(|| panic!("unknown test runtime name {name:?}"));
        ResolvedName {
            id: runtime_name.name().into(),
            location: NameLocation::RuntimeName(runtime_name),
        }
    }

    fn test_closure_cell_name(name: &str, slot: u32) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::closure_cell(slot),
        }
    }

    fn test_owned_cell_name(name: &str, slot: u32) -> ResolvedName {
        ResolvedName {
            id: name.into(),
            location: NameLocation::owned_cell(slot),
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
                    .expect("test integer literal should parse")
                    .into(),
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

    fn tuple_expr(values: Vec<InstrCodegen>) -> InstrCodegen {
        Tuple::new(values).into()
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

    struct ExplicitTestInstrIdCollector {
        block_label: BlockLabel,
        used: std::collections::HashSet<u32>,
    }

    impl Visit<InstrCodegen> for ExplicitTestInstrIdCollector {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if let Some(instr_id) = expr.try_semantic_instr_id()
                && instr_id.block_label() == self.block_label
            {
                self.used.insert(instr_id.instr_index_in_block());
            }
            expr.visit_children(self);
        }
    }

    struct MissingTestInstrIdAssigner {
        block_label: BlockLabel,
        next_instr_index_in_block: u32,
        used: std::collections::HashSet<u32>,
    }

    impl VisitMut<InstrCodegen> for MissingTestInstrIdAssigner {
        fn visit_instr_mut(&mut self, expr: &mut InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if expr.try_semantic_instr_id().is_none() {
                while self.used.contains(&self.next_instr_index_in_block) {
                    self.next_instr_index_in_block = self
                        .next_instr_index_in_block
                        .checked_add(1)
                        .expect("test block instruction count should fit in u32");
                }
                let instr_id = InstrId::new(self.block_label, self.next_instr_index_in_block);
                self.used.insert(self.next_instr_index_in_block);
                self.next_instr_index_in_block = self
                    .next_instr_index_in_block
                    .checked_add(1)
                    .expect("test block instruction count should fit in u32");
                let mut meta = expr.meta();
                meta.instr_id = Some(instr_id);
                *expr = expr.clone().with_meta(meta);
            }
            expr.visit_children_mut(self);
        }
    }

    fn assign_missing_test_instr_ids(function: &mut BlockPyFunction<CodegenModuleShape>) {
        for block in &mut function.blocks {
            let mut collector = ExplicitTestInstrIdCollector {
                block_label: block.label,
                used: std::collections::HashSet::new(),
            };
            collector.visit_block(block);

            let mut assigner = MissingTestInstrIdAssigner {
                block_label: block.label,
                next_instr_index_in_block: 0,
                used: collector.used,
            };
            assigner.visit_block_mut(block);
        }
    }

    fn assign_missing_test_module_instr_ids(module: &mut BlockPyModule<CodegenModuleShape>) {
        for function in &mut module.callable_defs {
            assign_missing_test_instr_ids(function);
        }
        validate_codegen_instr_ids(module)
            .expect("JIT test modules should carry semantic instruction ids");
    }

    fn fresh_test_work_dir(prefix: &str) -> PathBuf {
        let work_dir = crate::test_repo_root()
            .join("target")
            .join("debug")
            .join(format!(
                "{prefix}-{}-{}",
                std::process::id(),
                NEXT_TEST_WORK_DIR_ID.fetch_add(1, Ordering::Relaxed)
            ));
        if work_dir.exists() {
            std::fs::remove_dir_all(&work_dir).expect("stale test work dir should be removable");
        }
        std::fs::create_dir_all(&work_dir).expect("test work dir should exist");
        work_dir
    }

    fn write_test_counter_dump(path: &Path, record: &CounterDumpRecord) {
        write_counter_dump_records(path, std::iter::once(record))
            .expect("test counter dump should be writable");
    }

    fn count_typed_instrs(
        function: &BlockPyFunction<TypedCodegenModuleShape>,
        mut predicate: impl FnMut(&InstrTyped) -> bool,
    ) -> usize {
        struct Counter<'a, P> {
            predicate: &'a mut P,
            count: usize,
        }

        impl<P> Visit<InstrTyped> for Counter<'_, P>
        where
            P: FnMut(&InstrTyped) -> bool,
        {
            fn visit_instr(&mut self, expr: &InstrTyped)
            where
                InstrTyped: ChildVisitable<InstrTyped>,
            {
                if (self.predicate)(expr) {
                    self.count += 1;
                }
                expr.visit_children(self);
            }
        }

        let mut counter = Counter {
            predicate: &mut predicate,
            count: 0,
        };
        counter.visit_fn(function);
        counter.count
    }

    fn write_test_optimization_artifacts_v3(path: &Path, artifacts: &ExactIntBranchV3Artifacts) {
        write_optimization_artifacts_v3(path, artifacts)
            .expect("test optimization plan v3 should be writable");
    }

    fn test_codegen_cache_metadata_for_shared_state(
        shared_state: &crate::module_type::SharedModuleState,
        cache_source: PythonModuleCacheSource,
    ) -> CachedCodegenModuleMetadata {
        CachedCodegenModuleMetadata {
            source: cache_source,
            module_name: shared_state.module_name.clone(),
            source_hash: shared_state.source_hash,
            cache_identity: pre_optimization_module_cache_identity(
                env!("SOAC_BUILD_IDENTITY"),
                shared_state.module_name == "soac.runtime",
            ),
        }
    }

    fn write_test_optimized_codegen_module_v3(
        cache_root: &Path,
        cache_source: PythonModuleCacheSource,
        metadata: &CachedCodegenModuleMetadata,
        module: &BlockPyModule<CodegenModuleShape>,
    ) -> Result<(), String> {
        let path = module_optimized_codegen_v3_path(
            cache_root,
            cache_source,
            metadata.module_name.as_str(),
        )
        .map_err(|err| err.to_string())?;
        store_codegen_module_cache(path.as_path(), metadata, module).map_err(|err| err.to_string())
    }

    fn write_test_optimization_artifacts_v3_for_shared_state(
        cache_root: &Path,
        cache_source: PythonModuleCacheSource,
        shared_state: &crate::module_type::SharedModuleState,
        artifacts: &ExactIntBranchV3Artifacts,
    ) {
        let v3_path = module_optimization_plan_v3_path(
            cache_root,
            cache_source,
            shared_state.module_name.as_str(),
        )
        .expect("v3 test optimization plan path should build");
        write_test_optimization_artifacts_v3(v3_path.as_path(), artifacts);
        let metadata = test_codegen_cache_metadata_for_shared_state(shared_state, cache_source);
        write_test_optimized_codegen_module_v3(
            cache_root,
            cache_source,
            &metadata,
            &shared_state.lowered_module,
        )
        .expect("test optimized v3 BlockPy module cache should be writable");
    }

    fn ensure_test_optimization_artifacts_v3_for_shared_state(
        shared_state: &crate::module_type::SharedModuleState,
    ) -> Result<(), String> {
        let env_config = SoacEnvConfig::from_env()?;
        if !matches!(
            env_config.specialization_mode(),
            Some(SpecializationMode::Verify | SpecializationMode::Apply)
        ) {
            return Ok(());
        }
        let Some(cache_root) = env_config.module_cache_root() else {
            return Ok(());
        };
        let cache_source = shared_state
            .module_cache_source
            .unwrap_or(PythonModuleCacheSource::Project);
        let output_path = module_optimization_plan_v3_path(
            cache_root.as_path(),
            cache_source,
            shared_state.module_name.as_str(),
        )
        .map_err(|err| err.to_string())?;
        let optimized_output_path = module_optimized_codegen_v3_path(
            cache_root.as_path(),
            cache_source,
            shared_state.module_name.as_str(),
        )
        .map_err(|err| err.to_string())?;
        if output_path.exists() && optimized_output_path.exists() {
            return Ok(());
        }
        let metadata = test_codegen_cache_metadata_for_shared_state(shared_state, cache_source);
        if !optimized_output_path.exists() {
            write_test_optimized_codegen_module_v3(
                cache_root.as_path(),
                cache_source,
                &metadata,
                &shared_state.lowered_module,
            )?;
        }
        if output_path.exists() {
            return Ok(());
        }
        let evidence_store = match env_config
            .counter_dump_input_path()
            .filter(|path| path.exists())
        {
            Some(path) => ProfileEvidenceStore::from_counter_dump(path.as_path())
                .map_err(|err| err.to_string())?,
            None => ProfileEvidenceStore::default(),
        };
        let catalog = AlternativeCatalog::default_v3();
        let artifacts = plan_and_emit_module_v3_from_raw_evidence(
            &catalog,
            ModulePlanIdentity {
                module_name: metadata.module_name.clone(),
                source_hash: metadata.source_hash,
                cache_identity: metadata.cache_identity.clone(),
            },
            &shared_state.lowered_module,
            &evidence_store,
        )
        .map_err(|err| format!("generate test optimization plan v3: {err}"))?;
        write_optimization_artifacts_v3(output_path.as_path(), &artifacts)
            .map_err(|err| err.to_string())
    }

    fn test_empty_v3_artifacts_for_function(
        module_name: &str,
        source_hash: u64,
        cache_identity: &str,
        serialized_module_id: u32,
        function: &BlockPyFunction<CodegenModuleShape>,
    ) -> ExactIntBranchV3Artifacts {
        let serialized_function =
            test_serialized_function_id(serialized_module_id, function.function_id);
        ExactIntBranchV3Artifacts {
            plan: ModuleOptimizationPlanV3 {
                module: ModulePlanIdentity {
                    module_name: module_name.to_string(),
                    source_hash,
                    cache_identity: cache_identity.to_string(),
                },
                identity_tables: test_plan_identities(
                    module_name,
                    source_hash,
                    cache_identity,
                    serialized_function,
                    function.names.qualname.as_str(),
                    &[],
                ),
                helper_catalog_version: 1,
                cost_model_version: 1,
                functions: vec![soac_opt::plan_v3::FunctionOptimizationPlanV3 {
                    function: FunctionPlanIdentity {
                        function: serialized_function,
                        debug_name: Some(function.names.qualname.clone()),
                    },
                    regions: Vec::new(),
                    scalar_threads: Vec::new(),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    deopt_points: Vec::new(),
                    ownership: soac_opt::plan_v3::FunctionOwnershipPlan::default(),
                    diagnostics: Vec::new(),
                }],
            },
            emission: MechanicalModuleEmission {
                module_name: module_name.to_string(),
                functions: vec![soac_opt::emit_v3::MechanicalFunctionEmission {
                    function: serialized_function,
                    debug_name: Some(function.names.qualname.clone()),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    scalar_threads: Vec::new(),
                    regions: Vec::new(),
                }],
            },
        }
    }

    fn test_serialized_function_id(
        serialized_module_id: u32,
        function_id: RuntimeFunctionId,
    ) -> SerializedFunctionId {
        SerializedFunctionId::new(
            SerializedModuleId::new(serialized_module_id),
            function_id.local_function_id(),
        )
    }

    fn test_plan_identities(
        module_name: &str,
        source_hash: u64,
        cache_identity: &str,
        debug_function: SerializedFunctionId,
        debug_qualname: &str,
        extra_modules: &[(&str, u64)],
    ) -> SerializedIdentityTables {
        let mut modules = vec![SerializedModuleIdentity {
            module_name: module_name.to_string(),
            source_hash,
            cache_identity: Some(cache_identity.to_string()),
        }];
        for (module_name, source_hash) in extra_modules {
            modules.push(SerializedModuleIdentity {
                module_name: (*module_name).to_string(),
                source_hash: *source_hash,
                cache_identity: None,
            });
        }
        SerializedIdentityTables {
            modules,
            debug_names: vec![SerializedFunctionDebugName {
                function: debug_function,
                qualname: debug_qualname.to_string(),
            }],
        }
    }

    fn cached_split_key_layout(
        py: Python<'_>,
        owner_type: *mut ffi::PyTypeObject,
    ) -> Vec<(String, u32)> {
        const DICT_KEYS_SPLIT: u8 = 2;

        let heap_type = owner_type.cast::<ffi::PyHeapTypeObject>();
        if heap_type.is_null() {
            return Vec::new();
        }
        let keys = unsafe { (*heap_type).ht_cached_keys.cast::<RawPyDictKeysObject>() };
        if keys.is_null() || unsafe { (*keys).dk_kind } != DICT_KEYS_SPLIT {
            return Vec::new();
        }
        let entries = unsafe {
            keys.cast::<u8>()
                .add(size_of::<RawPyDictKeysObject>() + (1usize << (*keys).dk_log2_index_bytes))
                .cast::<RawPyDictUnicodeEntry>()
        };
        let mut out = Vec::new();
        let nentries = unsafe { (*keys).dk_nentries };
        for index in 0..nentries {
            let key = unsafe { (*entries.add(index as usize)).me_key };
            if key.is_null() {
                continue;
            }
            out.push((
                unsafe { pyo3::Bound::from_borrowed_ptr(py, key) }
                    .extract()
                    .expect("split-key entry should be a Python string"),
                u32::try_from(index).expect("split-key index should fit in u32"),
            ));
        }
        out
    }

    fn assign_stmt(target: ResolvedName, value: InstrCodegen) -> InstrCodegen {
        expr_stmt(op_expr(Store::new(target, value)))
    }

    fn ret_term(value: InstrCodegen) -> BlockTerm<InstrCodegen> {
        BlockTerm::Return(value)
    }

    fn raise_term() -> BlockTerm<InstrCodegen> {
        BlockTerm::Raise(soac_core::block_py::TermRaise { exc: None })
    }

    fn test_source_block(
        function: &BlockPyFunction<CodegenModuleShape>,
        ops: Vec<InstrCodegen>,
        term: BlockTerm<InstrCodegen>,
    ) -> CodegenBlock {
        CodegenBlock {
            label: function.name_gen.next_block_name(),
            body: ops,
            term,
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        }
    }

    fn test_function() -> BlockPyFunction<CodegenModuleShape> {
        let module_name_gen = ModuleNameGen::new(0);
        test_function_in_module(&module_name_gen, "test")
    }

    fn test_function_in_module(
        module_name_gen: &ModuleNameGen,
        name: &str,
    ) -> BlockPyFunction<CodegenModuleShape> {
        let name_gen = module_name_gen.next_function_name_gen();
        BlockPyFunction {
            function_id: name_gen.function_id(),
            name_gen,
            names: FunctionName::new(name, name, name, name),
            kind: soac_core::block_py::FunctionKind::Function,
            execution_mode: Default::default(),
            params: ParamSpec::default(),
            blocks: vec![],
            doc: None,
            storage_layout: None,
            scope: Default::default(),
        }
    }

    fn test_module(
        module_name_gen: ModuleNameGen,
        callable_defs: Vec<BlockPyFunction<CodegenModuleShape>>,
    ) -> BlockPyModule<CodegenModuleShape> {
        let mut module = BlockPyModule {
            module_name_gen,
            global_names: Vec::new(),
            callable_defs,
            module_constants: Vec::new(),
            counter_defs: Vec::new(),
        };
        assign_missing_test_module_instr_ids(&mut module);
        module
    }

    fn with_test_blocks(
        mut function: BlockPyFunction<CodegenModuleShape>,
        blocks: Vec<CodegenBlock>,
    ) -> BlockPyFunction<CodegenModuleShape> {
        function.blocks = blocks;
        assign_missing_test_instr_ids(&mut function);
        function
    }

    fn set_stack_slots(function: &mut BlockPyFunction<CodegenModuleShape>, names: &[&str]) {
        function
            .storage_layout
            .get_or_insert_with(StorageLayout::default)
            .set_stack_slots(names.iter().map(|name| (*name).to_string()).collect());
    }

    fn with_single_test_block(
        function: BlockPyFunction<CodegenModuleShape>,
        ops: Vec<InstrCodegen>,
        term: BlockTerm<InstrCodegen>,
    ) -> BlockPyFunction<CodegenModuleShape> {
        let block = test_source_block(&function, ops, term);
        with_test_blocks(function, vec![block])
    }

    fn build_test_cranelift_run_bb_specialized_function(
        jit_module: &mut JITModule,
        blocks: &[ObjPtr],
        module: &BlockPyModule<CodegenModuleShape>,
        function: &BlockPyFunction<CodegenModuleShape>,
        module_constants: &crate::module_constants::ModuleCodegenConstants,
        counter_defs: &[CounterDef],
        module_constant_object_data_ids: &[DataId],
        counter_slots_by_id: &[CounterRuntimeSlot],
        scalar_counter_data_id: Option<DataId>,
        top_value_counter_data_id: Option<DataId>,
        compile_session: &crate::session::CompileSession,
        direct_call_resolver: Option<&crate::module_type::SharedModuleState>,
        symbol_scope: Option<&str>,
        predeclared_direct_functions: Option<&HashMap<RuntimeFunctionId, DeclaredJitFunction>>,
        options: BuildSpecializedFunctionOptions,
    ) -> Result<BuiltSpecializedFunction, String> {
        let jit_module_plan = build_jit_module_plan(module)?;
        let planned_module = jit_module_plan.module.as_ref();
        let planned_function = planned_module
            .callable_defs
            .iter()
            .find(|candidate| candidate.function_id == function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing planned function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let jit_local_plan = jit_module_plan
            .locals
            .function(planned_function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT local plan for function {} ({})",
                    planned_function.function_id, planned_function.names.qualname
                )
            })?;
        let jit_deopt_resume_plan = jit_module_plan
            .deopt_resume
            .function(planned_function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT deopt resume plan for function {} ({})",
                    planned_function.function_id, planned_function.names.qualname
                )
            })?;
        if let Some(shared_state) = direct_call_resolver {
            ensure_test_optimization_artifacts_v3_for_shared_state(shared_state)?;
        }
        let specialization_profile = SpecializationProfile::from_runtime_state_with_session(
            direct_call_resolver,
            Some(compile_session),
        )?;
        predeclare_specialization_type_imports(jit_module, &specialization_profile)?;
        let specialization_inputs =
            FunctionSpecializationInputs::from_profile(&specialization_profile, planned_function)?;
        let mut direct_call_typed_function = planned_function.clone();
        apply_profile_call_emissions_to_typed_function(
            &mut direct_call_typed_function,
            Some(&specialization_profile),
        )?;
        lower_typed_function_call_access_plan_instrs(&mut direct_call_typed_function);
        predeclare_typed_direct_call_imports(jit_module, &direct_call_typed_function)?;
        let mut options = options;
        if options.specialization_inputs.is_none() {
            options.specialization_inputs = Some(specialization_inputs);
        }
        build_cranelift_run_bb_specialized_function(
            jit_module,
            blocks,
            planned_module,
            planned_function,
            Some(function),
            &jit_module_plan.value_facts,
            jit_local_plan,
            jit_deopt_resume_plan,
            module_constants,
            counter_defs,
            module_constant_object_data_ids,
            counter_slots_by_id,
            scalar_counter_data_id,
            top_value_counter_data_id,
            compile_session,
            direct_call_resolver,
            Some(&specialization_profile),
            symbol_scope,
            predeclared_direct_functions,
            options,
        )
    }

    fn build_test_specialized_function(
        blocks: &[ObjPtr],
        module: &BlockPyModule<CodegenModuleShape>,
        function: &BlockPyFunction<CodegenModuleShape>,
        module_constants: &crate::module_constants::ModuleCodegenConstants,
    ) -> BuiltSpecializedFunction {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let module_constant_ptrs = placeholder_module_constant_ptrs(module_constants.len());
        let module_constant_object_data_ids =
            declare_module_constant_object_data(&mut jit_module, module, &module_constant_ptrs)
                .expect("module constant object data should declare");
        let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
            define_test_counter_storage(&mut jit_module, module, &module.counter_defs);
        build_test_cranelift_run_bb_specialized_function(
            &mut jit_module,
            blocks,
            module,
            function,
            module_constants,
            &module.counter_defs,
            module_constant_object_data_ids.as_slice(),
            counter_slots_by_id.as_ref(),
            scalar_counter_data_id,
            top_value_counter_data_id,
            &compile_session,
            None,
            None,
            None,
            BuildSpecializedFunctionOptions::default(),
        )
        .expect("test specialized JIT function should build")
    }

    #[test]
    fn specialized_jit_branch_terms_compile_via_typed_truthiness() {
        let mut constants = TestConstantPool::default();
        let function = test_function();
        let entry_label = function.name_gen.next_block_name();
        let then_label = function.name_gen.next_block_name();
        let else_label = function.name_gen.next_block_name();
        let entry = CodegenBlock {
            label: entry_label,
            body: vec![],
            term: BlockTerm::IfTerm(soac_core::block_py::TermIf {
                test: name_expr(test_runtime_name("TRUE")),
                then_label,
                else_label,
            }),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let then_block = CodegenBlock {
            label: then_label,
            body: vec![],
            term: ret_term(constants.int_expr(1)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let else_block = CodegenBlock {
            label: else_label,
            body: vec![],
            term: ret_term(constants.int_expr(0)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let function = with_test_blocks(function, vec![entry, then_block, else_block]);
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        build_test_specialized_function(
            &[1usize as ObjPtr, 2usize as ObjPtr, 3usize as ObjPtr],
            &module,
            &function,
            &module_constants,
        );
    }

    #[test]
    fn specialized_jit_raise_terms_compile_via_typed_exception_expr() {
        let mut constants = TestConstantPool::default();
        let function = test_function();
        let function = with_single_test_block(
            function,
            vec![],
            BlockTerm::Raise(soac_core::block_py::TermRaise {
                exc: Some(constants.int_expr(1)),
            }),
        );
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        build_test_specialized_function(&[1usize as ObjPtr], &module, &function, &module_constants);
    }
    #[test]
    fn specialized_jit_branch_table_terms_compile_via_typed_index_expr() {
        let mut constants = TestConstantPool::default();
        let function = test_function();
        let entry_label = function.name_gen.next_block_name();
        let case_label = function.name_gen.next_block_name();
        let default_label = function.name_gen.next_block_name();
        let entry = CodegenBlock {
            label: entry_label,
            body: vec![],
            term: BlockTerm::BranchTable(soac_core::block_py::TermBranchTable {
                index: constants.int_expr(0),
                targets: vec![case_label],
                default_label,
            }),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let case_block = CodegenBlock {
            label: case_label,
            body: vec![],
            term: ret_term(constants.int_expr(1)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let default_block = CodegenBlock {
            label: default_label,
            body: vec![],
            term: ret_term(constants.int_expr(2)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let function = with_test_blocks(function, vec![entry, case_block, default_block]);
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        build_test_specialized_function(
            &[1usize as ObjPtr, 2usize as ObjPtr, 3usize as ObjPtr],
            &module,
            &function,
            &module_constants,
        );
    }
    #[test]
    fn specialized_jit_body_statements_compile_via_typed_ops() {
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![expr_stmt(constants.int_expr(1))],
            ret_term(constants.int_expr(2)),
        );
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        build_test_specialized_function(&[1usize as ObjPtr], &module, &function, &module_constants);
    }

    #[test]
    fn specialized_jit_body_binops_compile_via_typed_ops() {
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![expr_stmt(op_expr(BinOp::new(
                BinOpKind::Add,
                constants.int_expr(1),
                constants.int_expr(2),
            )))],
            ret_term(constants.int_expr(3)),
        );
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        build_test_specialized_function(&[1usize as ObjPtr], &module, &function, &module_constants);
    }

    #[test]
    fn specialized_jit_local_store_rhs_compiles_via_typed_demand() {
        let mut constants = TestConstantPool::default();
        let mut function = with_single_test_block(
            test_function(),
            vec![expr_stmt(op_expr(Store::new(
                test_name("x"),
                op_expr(BinOp::new(
                    BinOpKind::Add,
                    constants.int_expr(1),
                    constants.int_expr(2),
                )),
            )))],
            ret_term(name_expr(test_name("x"))),
        );
        set_stack_slots(&mut function, &["x"]);
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        build_test_specialized_function(&[1usize as ObjPtr], &module, &function, &module_constants);
    }

    #[test]
    fn specialized_jit_body_tuple_compile_via_effect_only_typed_ops() {
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![expr_stmt(tuple_expr(vec![constants.int_expr(1)]))],
            ret_term(constants.int_expr(2)),
        );
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        build_test_specialized_function(&[1usize as ObjPtr], &module, &function, &module_constants);
    }

    fn annotate_test_result_demands(
        mut function: BlockPyFunction<TypedCodegenModuleShape>,
    ) -> BlockPyFunction<TypedCodegenModuleShape> {
        annotate_typed_function_result_demands(&mut function);
        function
    }

    fn annotate_test_result_demands_and_plans(
        mut function: BlockPyFunction<TypedCodegenModuleShape>,
    ) -> BlockPyFunction<TypedCodegenModuleShape> {
        annotate_typed_function_result_demands(&mut function);
        annotate_typed_function_planned_results(&mut function);
        function
    }

    fn typed_demand_for_instr_id(
        function: &BlockPyFunction<TypedCodegenModuleShape>,
        instr_id: InstrId,
    ) -> Option<ResultDemand> {
        struct Finder {
            instr_id: InstrId,
            demand: Option<ResultDemand>,
        }

        impl Visit<InstrTyped> for Finder {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if self.demand.is_none() && expr.try_semantic_instr_id() == Some(self.instr_id) {
                    self.demand = expr.result_demand();
                }
                expr.visit_children(self);
            }
        }

        let mut finder = Finder {
            instr_id,
            demand: None,
        };
        finder.visit_fn(function);
        finder.demand
    }

    fn typed_planned_result_for_instr_id(
        function: &BlockPyFunction<TypedCodegenModuleShape>,
        instr_id: InstrId,
    ) -> Option<PlannedResult> {
        struct Finder {
            instr_id: InstrId,
            planned_result: Option<PlannedResult>,
        }

        impl Visit<InstrTyped> for Finder {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if self.planned_result.is_none()
                    && expr.try_semantic_instr_id() == Some(self.instr_id)
                {
                    self.planned_result = expr.planned_result();
                }
                expr.visit_children(self);
            }
        }

        let mut finder = Finder {
            instr_id,
            planned_result: None,
        };
        finder.visit_fn(function);
        finder.planned_result
    }

    #[test]
    fn typed_result_demand_extra_marks_statement_roots_effect_only() {
        let mut constants = TestConstantPool::default();
        let instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let function = with_single_test_block(
            test_function(),
            vec![expr_stmt(with_instr_id(constants.int_expr(1), instr_id))],
            ret_term(constants.int_expr(2)),
        );
        let typed_function =
            lower_typed_function_if_tests_to_truthy(lower_codegen_function_to_typed(function));
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, instr_id),
            Some(ResultDemand::EffectOnly)
        );
    }

    #[test]
    fn typed_planned_result_extra_marks_statement_roots_effect_only() {
        let mut constants = TestConstantPool::default();
        let instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let function = with_single_test_block(
            test_function(),
            vec![expr_stmt(with_instr_id(constants.int_expr(1), instr_id))],
            ret_term(constants.int_expr(2)),
        );
        let typed_function =
            lower_typed_function_if_tests_to_truthy(lower_codegen_function_to_typed(function));
        let typed_function = annotate_test_result_demands_and_plans(typed_function);

        assert_eq!(
            typed_planned_result_for_instr_id(&typed_function, instr_id),
            Some(PlannedResult::EffectOnly)
        );
    }

    #[test]
    fn typed_planned_result_extra_marks_borrowed_local_for_borrowed_input_demand() {
        let mut constants = TestConstantPool::default();
        let arg_instr_id = InstrId::new(BlockLabel::from_index(0), 2);
        let call = op_expr(Call::new(
            name_expr(test_runtime_name("callable")),
            vec![CallArgPositional::Positional(with_instr_id(
                name_expr(test_name("x")),
                arg_instr_id,
            ))],
            Vec::<CallArgKeyword<InstrCodegen>>::new(),
        ));
        let function =
            with_single_test_block(test_function(), vec![call], ret_term(constants.int_expr(1)));
        let typed_function = lower_codegen_function_to_typed(function);
        let typed_function = annotate_test_result_demands_and_plans(typed_function);

        assert_eq!(
            typed_planned_result_for_instr_id(&typed_function, arg_instr_id),
            Some(PlannedResult::PYOBJECT_BORROWED_LOCAL)
        );
    }

    #[test]
    fn typed_planned_result_extra_marks_immortal_pyobject_from_value_facts() {
        let return_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(with_instr_id(none_expr(), return_instr_id)),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let facts = infer_module_value_facts(&module);
        let mut typed_function = lower_codegen_function_to_typed(module.callable_defs[0].clone());
        annotate_typed_function_value_facts(&mut typed_function, &facts);
        refresh_typed_function_value_facts(&mut typed_function);
        let typed_function = annotate_test_result_demands_and_plans(typed_function);

        assert_eq!(
            typed_planned_result_for_instr_id(&typed_function, return_instr_id),
            Some(PlannedResult::PYOBJECT_IMMORTAL)
        );
    }

    #[test]
    fn typed_planned_result_extra_marks_module_constant_inputs_immortal() {
        let attr_instr_id = InstrId::new(BlockLabel::from_index(0), 1);
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(GetAttr::new(
                name_expr(test_name("obj")),
                with_instr_id(constants.string_expr("field"), attr_instr_id),
            ))),
        );
        let mut module = test_module(ModuleNameGen::new(0), vec![function]);
        module.module_constants = constants.module_constants;
        let facts = infer_module_value_facts(&module);
        let mut typed_function = lower_codegen_function_to_typed(module.callable_defs[0].clone());
        annotate_typed_function_value_facts(&mut typed_function, &facts);
        refresh_typed_function_value_facts(&mut typed_function);
        let typed_function = annotate_test_result_demands_and_plans(typed_function);

        assert_eq!(
            typed_planned_result_for_instr_id(&typed_function, attr_instr_id),
            Some(PlannedResult::PYOBJECT_IMMORTAL)
        );
    }

    #[test]
    fn typed_result_demand_extra_marks_local_store_rhs_pyobject_owned() {
        let mut constants = TestConstantPool::default();
        let store_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let rhs_instr_id = InstrId::new(BlockLabel::from_index(0), 1);
        let store = with_instr_id(
            op_expr(Store::new(
                test_name("x"),
                with_instr_id(constants.int_expr(1), rhs_instr_id),
            )),
            store_instr_id,
        );
        let function = with_single_test_block(
            test_function(),
            vec![store],
            ret_term(constants.int_expr(2)),
        );
        let typed_function =
            lower_typed_function_if_tests_to_truthy(lower_codegen_function_to_typed(function));
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, store_instr_id),
            Some(ResultDemand::EffectOnly)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, rhs_instr_id),
            Some(ResultDemand::PYOBJECT_OWNED)
        );
    }

    #[test]
    fn typed_result_demand_extra_marks_call_inputs_pyobject_borrowed_ok() {
        let mut constants = TestConstantPool::default();
        let call_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let func_instr_id = InstrId::new(BlockLabel::from_index(0), 1);
        let positional_instr_id = InstrId::new(BlockLabel::from_index(0), 2);
        let keyword_instr_id = InstrId::new(BlockLabel::from_index(0), 3);
        let call = with_instr_id(
            op_expr(Call::new(
                with_instr_id(name_expr(test_runtime_name("callable")), func_instr_id),
                vec![CallArgPositional::Positional(with_instr_id(
                    constants.int_expr(1),
                    positional_instr_id,
                ))],
                vec![CallArgKeyword::Named {
                    arg: "value".into(),
                    value: with_instr_id(constants.int_expr(2), keyword_instr_id),
                }],
            )),
            call_instr_id,
        );
        let function =
            with_single_test_block(test_function(), vec![call], ret_term(constants.int_expr(3)));
        let typed_function = lower_codegen_function_to_typed(function);
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, call_instr_id),
            Some(ResultDemand::EffectOnly)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, func_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, positional_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, keyword_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
    }

    #[test]
    fn typed_result_demand_extra_marks_lowered_guarded_call_inputs_borrowed_ok() {
        let mut constants = TestConstantPool::default();
        let call_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let func_instr_id = InstrId::new(BlockLabel::from_index(0), 1);
        let positional_instr_id = InstrId::new(BlockLabel::from_index(0), 2);
        let call = with_instr_id(
            op_expr(Call::new(
                with_instr_id(name_expr(test_runtime_name("callable")), func_instr_id),
                vec![CallArgPositional::Positional(with_instr_id(
                    constants.int_expr(1),
                    positional_instr_id,
                ))],
                Vec::<CallArgKeyword<InstrCodegen>>::new(),
            )),
            call_instr_id,
        );
        let function =
            with_single_test_block(test_function(), vec![call], ret_term(constants.int_expr(2)));
        let mut typed_function = lower_codegen_function_to_typed(function);
        let first_instr = typed_function.blocks[0]
            .body
            .first_mut()
            .expect("test block should contain call");
        let soac_opt::passes::InstrTyped::CallTyped(call) = first_instr else {
            panic!("test call should lower to typed call");
        };
        call.access = soac_opt::passes::TypedCallAccessPlan::GuardedCallable {
            function_guards: vec![soac_opt::passes::TypedDirectFunctionCallGuard {
                function_id: RuntimeFunctionId::from_raw_parts(0, 1),
                arg_plan: soac_opt::passes::TypedDirectCallArgPlan {
                    sources: vec![soac_opt::passes::TypedDirectCallArgSource::Provided(0)],
                },
            }],
            constructor_guards: Vec::new(),
        };

        assert_eq!(
            lower_typed_function_call_access_plan_instrs(&mut typed_function),
            1
        );
        assert!(
            matches!(
                typed_function.blocks[0].body.first(),
                Some(soac_opt::passes::InstrTyped::GuardedCallableCallTyped(_))
            ),
            "guarded call plan should lower before demand planning"
        );
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, call_instr_id),
            Some(ResultDemand::EffectOnly)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, func_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, positional_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
    }

    #[test]
    fn opt_v3_typed_call_preparation_skips_inline_body_plans() {
        let function_id = RuntimeFunctionId::from_raw_parts(0, 1);
        let source = InstrId::new(BlockLabel::from_index(0), 7);
        let target = RuntimeFunctionId::from_raw_parts(0, 9);
        let direct_call = ResolvedV3DirectCallPlan {
            source,
            target,
            arg_plan: TypedDirectCallArgPlan {
                sources: vec![TypedDirectCallArgSource::Provided(0)],
            },
            body: test_v3_inline_call_body(),
            reason: "profiled direct call".to_string(),
        };
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            optimized_module: None,
            direct_call_emission_scope: DirectCallEmissionScope::DirectCallBodiesOnly,
            opt_v3_emitted_direct_calls: HashMap::from([(
                function_id,
                HashMap::from([(source, vec![direct_call])]),
            )]),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        assert!(
            profile.codegen_opt_v3_direct_calls(function_id).is_empty(),
            "Inline direct-call body plans are owned by the early BlockPy rewrite path"
        );
    }

    #[test]
    fn typed_result_demand_extra_marks_direct_call_inputs_pyobject_borrowed_ok() {
        let mut constants = TestConstantPool::default();
        let call_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let callable_instr_id = InstrId::new(BlockLabel::from_index(0), 1);
        let positional_instr_id = InstrId::new(BlockLabel::from_index(0), 2);
        let call = with_instr_id(
            InstrCodegen::CallDirect(CallDirect::new(
                with_instr_id(name_expr(test_global_name("callee")), callable_instr_id),
                RuntimeFunctionId::from_raw_parts(0, 1),
                vec![CallArgPositional::Positional(with_instr_id(
                    constants.int_expr(1),
                    positional_instr_id,
                ))],
                Vec::<CallArgKeyword<InstrCodegen>>::new(),
            )),
            call_instr_id,
        );
        let function =
            with_single_test_block(test_function(), vec![call], ret_term(constants.int_expr(2)));
        let typed_function = lower_codegen_function_to_typed(function);
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, call_instr_id),
            Some(ResultDemand::EffectOnly)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, callable_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, positional_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
    }

    #[test]
    fn typed_planned_result_extra_marks_direct_call_local_inputs_borrowed() {
        let mut constants = TestConstantPool::default();
        let positional_instr_id = InstrId::new(BlockLabel::from_index(0), 2);
        let call = InstrCodegen::CallDirect(CallDirect::new(
            name_expr(test_global_name("callee")),
            RuntimeFunctionId::from_raw_parts(0, 1),
            vec![CallArgPositional::Positional(with_instr_id(
                name_expr(test_name("x")),
                positional_instr_id,
            ))],
            Vec::<CallArgKeyword<InstrCodegen>>::new(),
        ));
        let function =
            with_single_test_block(test_function(), vec![call], ret_term(constants.int_expr(2)));
        let typed_function = lower_codegen_function_to_typed(function);
        let typed_function = annotate_test_result_demands_and_plans(typed_function);

        assert_eq!(
            typed_planned_result_for_instr_id(&typed_function, positional_instr_id),
            Some(PlannedResult::PYOBJECT_BORROWED_LOCAL)
        );
    }

    #[test]
    fn runtime_builtin_primitive_recognition_requires_static_runtime_name() {
        let mut module = test_module(ModuleNameGen::new(0), vec![test_function()]);
        module
            .module_constants
            .push(InstrResolved::Load(Load::new(test_runtime_name("ord"))));
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let arg = name_expr(test_name("x"));

        let runtime_ord_call = Call::new(
            name_expr(test_runtime_name("ord")),
            vec![CallArgPositional::Positional(arg.clone())],
            vec![],
        );
        assert_eq!(
            static_runtime_primitive_for_call(&runtime_ord_call, &module_constants),
            Some(RuntimePrimitiveId::BuiltinOrdI64)
        );

        let constant_ord_call = Call::new(
            name_expr(test_constant_name(0)),
            vec![CallArgPositional::Positional(arg.clone())],
            vec![],
        );
        assert_eq!(
            static_runtime_primitive_for_call(&constant_ord_call, &module_constants),
            Some(RuntimePrimitiveId::BuiltinOrdI64)
        );

        let global_ord_call = Call::new(
            name_expr(test_global_name("ord")),
            vec![CallArgPositional::Positional(arg)],
            vec![],
        );
        assert_eq!(
            static_runtime_primitive_for_call(&global_ord_call, &module_constants),
            None
        );

        let wrong_arity_ord_call = Call::new(
            name_expr(test_runtime_name("ord")),
            vec![
                CallArgPositional::Positional(name_expr(test_name("x"))),
                CallArgPositional::Positional(name_expr(test_name("y"))),
            ],
            vec![],
        );
        assert_eq!(
            static_runtime_primitive_for_call(&wrong_arity_ord_call, &module_constants),
            None
        );
    }

    #[test]
    fn runtime_builtin_i64_demand_accepts_ord_and_i64_constants() {
        let mut module = test_module(ModuleNameGen::new(0), vec![test_function()]);
        module
            .module_constants
            .push(InstrResolved::Load(Load::new(test_runtime_name("ord"))));
        module.module_constants.push(int_literal(65));
        module.module_constants.push(int_literal(1));
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let arg = name_expr(test_name("x"));
        let ord_call = InstrCodegen::Call(Call::new(
            name_expr(test_constant_name(0)),
            vec![CallArgPositional::Positional(arg)],
            vec![],
        ));
        let int_constant = name_expr(test_constant_name(1));
        let one_constant = name_expr(test_constant_name(2));
        let ord_plus_one = op_expr(BinOp::new(
            BinOpKind::Add,
            Box::new(ord_call.clone()),
            Box::new(one_constant),
        ));

        assert!(codegen_expr_static_can_satisfy_i64_demand(
            &ord_call,
            &module_constants
        ));
        assert!(codegen_expr_static_can_satisfy_i64_demand(
            &int_constant,
            &module_constants
        ));
        assert!(codegen_expr_static_can_satisfy_i64_demand(
            &ord_plus_one,
            &module_constants
        ));
        assert_eq!(
            codegen_expr_static_i64_demand_facts(&ord_plus_one, &module_constants)
                .and_then(|facts| facts.range),
            Some(IntRange {
                min: 1,
                max: 0x110000
            })
        );
    }

    #[test]
    fn i64_demand_facts_accept_checked_machine_int_overflow_paths() {
        let mut module = test_module(ModuleNameGen::new(0), vec![test_function()]);
        module.module_constants.push(int_literal(i64::MAX));
        module.module_constants.push(int_literal(1));
        module.module_constants.push(int_literal(3037000500i64));
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);

        let overflowing_add = op_expr(BinOp::new(
            BinOpKind::Add,
            name_expr(test_constant_name(0)),
            name_expr(test_constant_name(1)),
        ));
        let checked_mul = op_expr(BinOp::new(
            BinOpKind::Mul,
            name_expr(test_constant_name(2)),
            name_expr(test_constant_name(2)),
        ));

        assert_eq!(
            codegen_expr_static_i64_demand_facts(&overflowing_add, &module_constants),
            Some(IntFacts::i64_unknown()),
        );
        assert_eq!(
            codegen_expr_static_i64_demand_facts(&checked_mul, &module_constants),
            Some(IntFacts::i64_unknown()),
        );
    }

    #[test]
    fn i64_demand_codegen_emits_checked_machine_int_overflow_paths() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "i64_demand_codegen_emits_checked_machine_int_overflow_paths",
        ) {
            return;
        }

        for (kind, lhs, rhs, opcode) in [
            (BinOpKind::Add, 1, 2, ir::Opcode::SaddOverflow),
            (BinOpKind::Sub, 1, 2, ir::Opcode::SsubOverflow),
            (BinOpKind::Mul, 3, 4, ir::Opcode::SmulOverflow),
        ] {
            let blocks = [1usize as ObjPtr];
            let mut constants = TestConstantPool::default();
            constants
                .module_constants
                .push(InstrResolved::Load(Load::new(test_runtime_name("chr"))));
            let chr = name_expr(test_constant_name(0));
            let lhs = constants.int_expr(lhs);
            let rhs = constants.int_expr(rhs);
            let checked_arg = op_expr(BinOp::new(kind, lhs, rhs));
            let chr_call = Call::new(
                chr,
                vec![CallArgPositional::Positional(checked_arg)],
                vec![],
            );
            let mut function = test_function();
            let block = CodegenBlock {
                label: function.name_gen.next_block_name(),
                body: vec![],
                term: ret_term(op_expr(chr_call)),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            };
            function.blocks = vec![block];
            let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
            module.module_constants = constants.module_constants.clone();
            let module_constants =
                crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
            let built = build_test_jit_function_with_constants(
                &module,
                &module.callable_defs[0],
                &blocks,
                &module_constants,
            );

            assert_eq!(
                count_opcode(&built.ctx.func, opcode),
                1,
                "{kind:?} should lower to a signed overflow-checking Cranelift opcode"
            );
            let helper_names =
                import_user_names_for_symbols(&built, &["dp_jit_raise_i64_overflow"]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &helper_names),
                1,
                "{kind:?} overflow path should call the SOAC overflow raiser"
            );
        }
    }

    #[test]
    fn runtime_builtin_param_matching_uses_descriptor_abi() {
        let mut module = test_module(ModuleNameGen::new(0), vec![test_function()]);
        module
            .module_constants
            .push(InstrResolved::Load(Load::new(test_runtime_name("chr"))));
        module.module_constants.push(int_literal(65));
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);

        let literal_chr_call = Call::new(
            name_expr(test_constant_name(0)),
            vec![CallArgPositional::Positional(name_expr(
                test_constant_name(1),
            ))],
            vec![],
        );
        let unknown_chr_call = Call::new(
            name_expr(test_constant_name(0)),
            vec![CallArgPositional::Positional(name_expr(test_name("x")))],
            vec![],
        );
        let desc = static_runtime_primitive_desc_for_call(&literal_chr_call, &module_constants)
            .expect("chr runtime primitive should be recognized");

        assert!(runtime_primitive_call_static_params_can_satisfy_abi(
            &literal_chr_call,
            desc,
            &module_constants
        ));
        assert!(!runtime_primitive_call_static_params_can_satisfy_abi(
            &unknown_chr_call,
            desc,
            &module_constants
        ));
    }

    #[test]
    fn typed_result_demand_is_node_local_without_instr_id() {
        let mut constants = TestConstantPool::default();
        let call = op_expr(Call::new(
            name_expr(test_runtime_name("callable")),
            vec![CallArgPositional::Positional(constants.int_expr(1))],
            Vec::<CallArgKeyword<InstrCodegen>>::new(),
        ));
        let function =
            with_single_test_block(test_function(), vec![call], ret_term(constants.int_expr(2)));
        let typed_function =
            annotate_test_result_demands(lower_codegen_function_to_typed(function));
        let Some(InstrTyped::CallTyped(call)) = typed_function.blocks[0].body.first() else {
            panic!("test call should lower to typed call");
        };

        assert_eq!(call.extra.demand(), Some(ResultDemand::EffectOnly));
        assert_eq!(
            call.func.result_demand(),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            call.args[0].expr().result_demand(),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
    }

    #[test]
    fn typed_result_demand_extra_marks_intrinsic_inputs_pyobject_borrowed_ok() {
        let mut constants = TestConstantPool::default();
        let binop_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let left_instr_id = InstrId::new(BlockLabel::from_index(0), 1);
        let right_instr_id = InstrId::new(BlockLabel::from_index(0), 2);
        let binop = with_instr_id(
            op_expr(BinOp::new(
                BinOpKind::Add,
                Box::new(with_instr_id(constants.int_expr(1), left_instr_id)),
                Box::new(with_instr_id(constants.int_expr(2), right_instr_id)),
            )),
            binop_instr_id,
        );
        let function = with_single_test_block(
            test_function(),
            vec![binop],
            ret_term(constants.int_expr(3)),
        );
        let typed_function = lower_codegen_function_to_typed(function);
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, binop_instr_id),
            Some(ResultDemand::EffectOnly)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, left_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            typed_demand_for_instr_id(&typed_function, right_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
    }

    #[test]
    fn typed_result_demand_extra_marks_branch_tests_i32_bool01() {
        let mut constants = TestConstantPool::default();
        let function = test_function();
        let entry_label = function.name_gen.next_block_name();
        let then_label = function.name_gen.next_block_name();
        let else_label = function.name_gen.next_block_name();
        let test_instr_id = InstrId::new(entry_label, 0);
        let entry = CodegenBlock {
            label: entry_label,
            body: vec![],
            term: BlockTerm::IfTerm(soac_core::block_py::TermIf {
                test: with_instr_id(constants.int_expr(0), test_instr_id),
                then_label,
                else_label,
            }),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let then_block = CodegenBlock {
            label: then_label,
            body: vec![],
            term: ret_term(constants.int_expr(1)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let else_block = CodegenBlock {
            label: else_label,
            body: vec![],
            term: ret_term(constants.int_expr(2)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let function = with_test_blocks(function, vec![entry, then_block, else_block]);
        let typed_function =
            lower_typed_function_if_tests_to_truthy(lower_codegen_function_to_typed(function));
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, test_instr_id),
            Some(ResultDemand::I32_BOOL01)
        );
    }

    #[test]
    fn typed_result_demand_extra_marks_branch_table_indices_i64_index() {
        let mut constants = TestConstantPool::default();
        let function = test_function();
        let entry_label = function.name_gen.next_block_name();
        let case_label = function.name_gen.next_block_name();
        let default_label = function.name_gen.next_block_name();
        let index_instr_id = InstrId::new(entry_label, 0);
        let entry = CodegenBlock {
            label: entry_label,
            body: vec![],
            term: BlockTerm::BranchTable(soac_core::block_py::TermBranchTable {
                index: with_instr_id(constants.int_expr(0), index_instr_id),
                targets: vec![case_label],
                default_label,
            }),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let case_block = CodegenBlock {
            label: case_label,
            body: vec![],
            term: ret_term(constants.int_expr(1)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let default_block = CodegenBlock {
            label: default_label,
            body: vec![],
            term: ret_term(constants.int_expr(2)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let function = with_test_blocks(function, vec![entry, case_block, default_block]);
        let typed_function = lower_codegen_function_to_typed(function);
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, index_instr_id),
            Some(ResultDemand::I64_INDEX)
        );
    }

    #[test]
    fn typed_result_demand_extra_marks_return_values_pyobject_owned() {
        let mut constants = TestConstantPool::default();
        let return_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(with_instr_id(constants.int_expr(2), return_instr_id)),
        );
        let typed_function = lower_codegen_function_to_typed(function);
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, return_instr_id),
            Some(ResultDemand::PYOBJECT_OWNED)
        );
    }

    #[test]
    fn typed_result_demand_extra_marks_raise_values_pyobject_owned() {
        let mut constants = TestConstantPool::default();
        let raise_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let function = with_single_test_block(
            test_function(),
            vec![],
            BlockTerm::Raise(soac_core::block_py::TermRaise {
                exc: Some(with_instr_id(constants.int_expr(2), raise_instr_id)),
            }),
        );
        let typed_function = lower_codegen_function_to_typed(function);
        let typed_function = annotate_test_result_demands(typed_function);

        assert_eq!(
            typed_demand_for_instr_id(&typed_function, raise_instr_id),
            Some(ResultDemand::PYOBJECT_OWNED)
        );
    }

    fn direct_call_expr(function_id: RuntimeFunctionId) -> InstrCodegen {
        InstrCodegen::CallDirect(CallDirect::new(
            none_expr(),
            function_id,
            Vec::<CallArgPositional<InstrCodegen>>::new(),
            Vec::<CallArgKeyword<InstrCodegen>>::new(),
        ))
    }

    fn test_param(name: &str, kind: ParamKind, has_default: bool) -> Param {
        Param {
            name: name.into(),
            kind,
            has_default,
        }
    }

    #[test]
    fn direct_call_arg_plan_binds_defaults_to_parameter_slots() {
        let mut target = test_function();
        target.params.params = vec![
            test_param("x", ParamKind::Any, false),
            test_param("y", ParamKind::Any, true),
            test_param("z", ParamKind::KwOnly, true),
        ];

        let plan = plan_direct_call_args_for_target(&target, 1, 0, false, false)
            .expect("defaulted parameters should use sentinel direct args");
        assert_eq!(
            plan.sources,
            vec![
                DirectCallArgSource::Provided(0),
                DirectCallArgSource::DefaultSentinel,
                DirectCallArgSource::DefaultSentinel,
            ]
        );
    }

    #[test]
    fn direct_call_arg_plan_accounts_for_bound_receiver() {
        let mut target = test_function();
        target.params.params = vec![
            test_param("self", ParamKind::Any, false),
            test_param("value", ParamKind::Any, false),
        ];

        let plan = plan_direct_call_args_for_target(&target, 1, 1, false, false)
            .expect("bound receiver should count as a provided direct arg");
        assert_eq!(
            plan.sources,
            vec![
                DirectCallArgSource::Provided(0),
                DirectCallArgSource::Provided(1),
            ]
        );
    }

    #[test]
    fn direct_call_arg_plan_rejects_unsupported_call_shapes() {
        let mut target = test_function();
        target.params.params = vec![test_param("x", ParamKind::Any, false)];

        assert_eq!(
            plan_direct_call_args_for_target(&target, 1, 0, true, false),
            Err(DirectCallIncompatibility::StarredArguments)
        );
        assert_eq!(
            plan_direct_call_args_for_target(&target, 1, 0, false, true),
            Err(DirectCallIncompatibility::Keywords)
        );

        let mut vararg_target = test_function();
        vararg_target.params.params = vec![test_param("args", ParamKind::VarArg, false)];
        assert_eq!(
            plan_direct_call_args_for_target(&vararg_target, 0, 0, false, false),
            Err(DirectCallIncompatibility::UnsupportedParameterKind {
                kind: ParamKind::VarArg,
            })
        );
    }

    #[test]
    fn direct_call_arg_plan_rejects_incompatible_arity() {
        let mut target = test_function();
        target.params.params = vec![test_param("x", ParamKind::Any, false)];

        assert_eq!(
            plan_direct_call_args_for_target(&target, 0, 0, false, false),
            Err(DirectCallIncompatibility::MissingRequiredArgument)
        );
        assert_eq!(
            plan_direct_call_args_for_target(&target, 2, 0, false, false),
            Err(DirectCallIncompatibility::TooManyPositionalArguments {
                provided: 2,
                accepted: 1,
            })
        );
    }

    #[test]
    fn direct_call_compatibility_accepts_target_without_predeclared_symbol() {
        let target = test_function();

        assert_eq!(
            validate_direct_call_compatibility(
                &target,
                &std::collections::HashMap::new(),
                0,
                0,
                false,
                false,
            ),
            Ok(DirectCallArgPlan {
                sources: Vec::new(),
            })
        );
    }

    #[test]
    fn specialized_jit_call_direct_uses_loaded_function_env_without_predeclared_symbol() {
        let blocks = [1usize as ObjPtr];
        let module_name_gen = ModuleNameGen::new(95);
        let mut constants = TestConstantPool::default();
        let callee = with_single_test_block(
            test_function_in_module(&module_name_gen, "callee"),
            vec![],
            ret_term(constants.int_expr(7)),
        );
        let caller = with_single_test_block(
            test_function_in_module(&module_name_gen, "caller"),
            vec![],
            ret_term(InstrCodegen::CallDirect(CallDirect::new(
                name_expr(test_global_name("callee")),
                callee.function_id,
                Vec::<CallArgPositional<InstrCodegen>>::new(),
                Vec::<CallArgKeyword<InstrCodegen>>::new(),
            ))),
        );
        let mut module = test_module(module_name_gen, vec![callee, caller.clone()]);
        module.global_names = vec!["callee".into()];
        module.module_constants = constants.module_constants;
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let built =
            build_test_jit_function_with_constants(&module, &caller, &blocks, &module_constants);
        assert!(
            count_indirect_calls(&built.ctx.func) >= 1,
            "direct-call lowering without a predeclared target should indirect through FunctionEnv.direct_code_ptr",
        );
        let generic_call_helpers = import_user_names_for_symbols(
            &built,
            &[
                DP_JIT_PY_CALL_OBJECT_IMPORT.symbol,
                DP_JIT_PY_VECTORCALL_IMPORT.symbol,
                DP_JIT_PY_CALL_WITH_KW_IMPORT.symbol,
                DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT.symbol,
            ],
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &generic_call_helpers),
            0,
            "direct-call lowering should not fall back to the generic Python call helpers",
        );
    }

    #[test]
    fn local_env_cleanup_values_exclude_stack_mirrors_and_immortals() {
        let owned_local = ir::Value::from_u32(1);
        let owned_mirror = ir::Value::from_u32(2);
        let immortal_local = ir::Value::from_u32(3);
        let env = LocalEnv {
            entries: vec![
                LocalEnvEntry {
                    location: Some(LocalLocation(0)),
                    name: "local".to_string(),
                    aliases: Vec::new(),
                    value: owned_local,
                    ref_kind: LocalRefKind::Owned,
                    storage: LocalEnvStorage::LocalOnly,
                    binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                    py_facts: None,
                },
                LocalEnvEntry {
                    location: Some(LocalLocation(1)),
                    name: "mirror".to_string(),
                    aliases: Vec::new(),
                    value: owned_mirror,
                    ref_kind: LocalRefKind::Owned,
                    storage: LocalEnvStorage::StackMirror,
                    binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                    py_facts: None,
                },
                LocalEnvEntry {
                    location: Some(LocalLocation(2)),
                    name: "immortal".to_string(),
                    aliases: Vec::new(),
                    value: immortal_local,
                    ref_kind: LocalRefKind::Immortal,
                    storage: LocalEnvStorage::LocalOnly,
                    binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Immortal),
                    py_facts: None,
                },
            ],
        };

        assert_eq!(env.local_only_cleanup_values(), vec![owned_local]);
    }

    #[test]
    fn local_env_semantic_cleanup_names_excluding_only_reports_unforwarded_locations() {
        let env = LocalEnv {
            entries: vec![
                LocalEnvEntry {
                    location: Some(LocalLocation(0)),
                    name: "x".to_string(),
                    aliases: Vec::new(),
                    value: ir::Value::from_u32(1),
                    ref_kind: LocalRefKind::Owned,
                    storage: LocalEnvStorage::LocalOnly,
                    binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                    py_facts: None,
                },
                LocalEnvEntry {
                    location: Some(LocalLocation(1)),
                    name: "y".to_string(),
                    aliases: Vec::new(),
                    value: ir::Value::from_u32(2),
                    ref_kind: LocalRefKind::Owned,
                    storage: LocalEnvStorage::LocalOnly,
                    binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                    py_facts: None,
                },
                LocalEnvEntry {
                    location: Some(LocalLocation(2)),
                    name: "tmp".to_string(),
                    aliases: Vec::new(),
                    value: ir::Value::from_u32(3),
                    ref_kind: LocalRefKind::Borrowed,
                    storage: LocalEnvStorage::LocalOnly,
                    binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Borrowed),
                    py_facts: None,
                },
                LocalEnvEntry {
                    location: Some(LocalLocation(3)),
                    name: "immortal".to_string(),
                    aliases: Vec::new(),
                    value: ir::Value::from_u32(4),
                    ref_kind: LocalRefKind::Immortal,
                    storage: LocalEnvStorage::LocalOnly,
                    binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Immortal),
                    py_facts: None,
                },
            ],
        };
        let forwarded = HashSet::from([LocalLocation(1)]);

        assert_eq!(
            env.transient_semantic_cleanup_names_excluding(&forwarded, &[]),
            vec!["x".to_string()]
        );
    }

    #[cfg(debug_assertions)]
    #[test]
    fn emit_decref_unforwarded_local_env_panics_on_residual_semantic_cleanup() {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id = declare_local_fn(
            &mut jit_module,
            "local_env_residual_semantic_cleanup_test",
            &wrapper_signature,
        )
        .expect("wrapper function should declare");
        let decref_id = declare_local_fn(
            &mut jit_module,
            "local_env_residual_semantic_cleanup_test_decref",
            &refcount_signature,
        )
        .expect("decref helper should declare");

        let mut ctx = jit_module.make_context();
        ctx.func.name = ir::UserFuncName::user(0, wrapper_id.as_u32());
        ctx.func.signature = wrapper_signature;

        let mut builder_ctx = FunctionBuilderContext::new();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
            let entry = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.switch_to_block(entry);
            fb.seal_block(entry);

            let null_tstate = fb.ins().iconst(ptr_ty, 0);
            let decref_ref = jit_module.declare_func_in_func(decref_id, &mut fb.func);
            let env = LocalEnv {
                entries: vec![LocalEnvEntry {
                    location: Some(LocalLocation(0)),
                    name: "x".to_string(),
                    aliases: Vec::new(),
                    value: fb.block_params(entry)[0],
                    ref_kind: LocalRefKind::Owned,
                    storage: LocalEnvStorage::LocalOnly,
                    binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                    py_facts: None,
                }],
            };
            let forwarded = HashSet::new();
            emit_decref_unforwarded_local_env(
                &mut fb,
                &env,
                &forwarded,
                &[],
                null_tstate,
                decref_ref,
            );
        }));

        assert!(
            panic.is_err(),
            "expected semantic LocalEnv cleanup without a planned release to trip the debug assertion"
        );
    }

    #[test]
    fn emit_decref_unforwarded_local_env_allows_forwarded_semantic_local() {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id = declare_local_fn(
            &mut jit_module,
            "local_env_forwarded_semantic_cleanup_test",
            &wrapper_signature,
        )
        .expect("wrapper function should declare");
        let decref_id = declare_local_fn(
            &mut jit_module,
            "local_env_forwarded_semantic_cleanup_test_decref",
            &refcount_signature,
        )
        .expect("decref helper should declare");

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

            let null_tstate = fb.ins().iconst(ptr_ty, 0);
            let decref_ref = jit_module.declare_func_in_func(decref_id, &mut fb.func);
            let env = LocalEnv {
                entries: vec![LocalEnvEntry {
                    location: Some(LocalLocation(0)),
                    name: "x".to_string(),
                    aliases: Vec::new(),
                    value: fb.block_params(entry)[0],
                    ref_kind: LocalRefKind::Owned,
                    storage: LocalEnvStorage::LocalOnly,
                    binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                    py_facts: None,
                }],
            };
            let forwarded = HashSet::from([LocalLocation(0)]);
            emit_decref_unforwarded_local_env(
                &mut fb,
                &env,
                &forwarded,
                &[],
                null_tstate,
                decref_ref,
            );
            fb.ins().return_(&[]);
            fb.seal_all_blocks();
            fb.finalize();
        }
    }

    #[test]
    fn local_env_borrowability_uses_location_entries() {
        let env = LocalEnv {
            entries: vec![LocalEnvEntry {
                location: Some(LocalLocation(0)),
                name: "x".to_string(),
                aliases: Vec::new(),
                value: ir::Value::from_u32(1),
                ref_kind: LocalRefKind::Owned,
                storage: LocalEnvStorage::LocalOnly,
                binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                py_facts: None,
            }],
        };
        let stack_slots = StackSlots {
            names: Vec::new(),
            slots: Vec::new(),
        };

        assert!(codegen_expr_is_borrowable_from_local_env(
            &name_expr(test_name("x")),
            &env,
            &stack_slots,
            None,
        ));
    }

    #[test]
    fn local_env_borrowability_uses_storage_layout_name_entries() {
        let env = LocalEnv {
            entries: vec![LocalEnvEntry {
                location: Some(LocalLocation(9)),
                name: "x".to_string(),
                aliases: Vec::new(),
                value: ir::Value::from_u32(1),
                ref_kind: LocalRefKind::Owned,
                storage: LocalEnvStorage::LocalOnly,
                binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                py_facts: None,
            }],
        };
        let stack_slots = StackSlots {
            names: Vec::new(),
            slots: Vec::new(),
        };
        let storage_layout = StorageLayout {
            freevars: Vec::new(),
            cellvars: Vec::new(),
            runtime_cells: Vec::new(),
            stack_slots: vec!["x".to_string()],
        };

        assert!(codegen_expr_is_borrowable_from_local_env(
            &name_expr(test_name("x")),
            &env,
            &stack_slots,
            Some(&storage_layout),
        ));
    }

    #[test]
    fn typed_planned_borrowed_local_input_still_requires_local_env_borrowability() {
        let env = LocalEnv {
            entries: vec![LocalEnvEntry {
                location: Some(LocalLocation(0)),
                name: "x".to_string(),
                aliases: Vec::new(),
                value: ir::Value::from_u32(1),
                ref_kind: LocalRefKind::Owned,
                storage: LocalEnvStorage::LocalOnly,
                binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                py_facts: None,
            }],
        };
        let stack_slots = StackSlots {
            names: Vec::new(),
            slots: Vec::new(),
        };
        let mut extra = TypedInstrExtra::default();
        extra.set_planned_result(PlannedResult::PYOBJECT_BORROWED_LOCAL);
        let expr = InstrTyped::Load(Load::<InstrTyped>::new(test_name("x")).with_extra(extra));

        assert_eq!(
            typed_expr_planned_pyobject_input_is_borrowed_from_local_env(
                &expr,
                &env,
                &stack_slots,
                None,
            ),
            Some(true)
        );
        assert_eq!(
            typed_expr_planned_pyobject_input_is_borrowed_from_local_env(
                &expr,
                &LocalEnv::default(),
                &stack_slots,
                None,
            ),
            Some(false)
        );

        let mut immortal_extra = TypedInstrExtra::default();
        immortal_extra.set_planned_result(PlannedResult::PYOBJECT_IMMORTAL);
        let immortal_expr = InstrTyped::Load(
            Load::<InstrTyped>::new(test_constant_name(0)).with_extra(immortal_extra),
        );
        assert_eq!(
            typed_expr_planned_pyobject_input_is_borrowed_from_local_env(
                &immortal_expr,
                &LocalEnv::default(),
                &stack_slots,
                None,
            ),
            Some(true)
        );
    }

    #[test]
    fn typed_planned_pyobject_ownership_preserves_immortal_result_facts() {
        let mut extra = TypedInstrExtra::default();
        extra.refine_result_facts(ValueFacts::PyObj(PyObjFacts::none_singleton()));
        extra.set_planned_result(PlannedResult::PYOBJECT_IMMORTAL);
        let expr =
            InstrTyped::Load(Load::<InstrTyped>::new(test_runtime_name("NONE")).with_extra(extra));

        let (ownership, facts) =
            planned_owned_pyobject_result_for_typed_expr(&expr, &LocalEnv::default());

        assert_eq!(ownership, ValueOwnership::Immortal);
        assert!(facts.is_none());

        let mut op_extra = TypedInstrExtra::default();
        op_extra.refine_result_facts(ValueFacts::PyObj(PyObjFacts::bool_object()));
        op_extra.set_planned_result(PlannedResult::PYOBJECT_IMMORTAL);
        let operand = InstrTyped::Load(Load::<InstrTyped>::new(test_runtime_name("NONE")));
        let expr = InstrTyped::LegacyUnaryOp(
            UnaryOp::new(UnaryOpKind::Not, Box::new(operand)).with_extra(op_extra),
        );

        let (ownership, facts) =
            planned_owned_pyobject_result_for_typed_expr(&expr, &LocalEnv::default());

        assert_eq!(ownership, ValueOwnership::Immortal);
        assert!(facts.is_exact_type(PyExactType::Bool));
    }

    #[test]
    fn typed_local_load_result_plan_uses_borrowed_and_immortal_plans() {
        let env = LocalEnv {
            entries: vec![LocalEnvEntry {
                location: Some(LocalLocation(0)),
                name: "x".to_string(),
                aliases: Vec::new(),
                value: ir::Value::from_u32(1),
                ref_kind: LocalRefKind::Owned,
                storage: LocalEnvStorage::LocalOnly,
                binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                py_facts: None,
            }],
        };
        let stack_slots = StackSlots {
            names: Vec::new(),
            slots: Vec::new(),
        };

        let mut borrowed_extra = TypedInstrExtra::default();
        borrowed_extra.set_planned_result(PlannedResult::PYOBJECT_BORROWED_LOCAL);
        let borrowed_expr =
            InstrTyped::Load(Load::<InstrTyped>::new(test_name("x")).with_extra(borrowed_extra));
        assert_eq!(
            typed_local_load_direct_result_plan(
                &borrowed_expr,
                &env,
                &stack_slots,
                None,
                ResultDemand::PYOBJECT_BORROWED_OK,
            ),
            Some((ValueOwnership::Borrowed, PyObjFacts::unknown()))
        );
        assert_eq!(
            typed_local_load_direct_result_plan(
                &borrowed_expr,
                &env,
                &stack_slots,
                None,
                ResultDemand::PYOBJECT_OWNED,
            ),
            None
        );
        assert_eq!(
            typed_local_load_direct_result_plan(
                &borrowed_expr,
                &env,
                &stack_slots,
                None,
                ResultDemand::EffectOnly,
            ),
            Some((ValueOwnership::Borrowed, PyObjFacts::unknown()))
        );

        let mut immortal_extra = TypedInstrExtra::default();
        immortal_extra.refine_result_facts(ValueFacts::PyObj(PyObjFacts::none_singleton()));
        immortal_extra.set_planned_result(PlannedResult::PYOBJECT_IMMORTAL);
        let immortal_expr =
            InstrTyped::Load(Load::<InstrTyped>::new(test_name("x")).with_extra(immortal_extra));
        assert_eq!(
            typed_local_load_direct_result_plan(
                &immortal_expr,
                &env,
                &stack_slots,
                None,
                ResultDemand::PYOBJECT_OWNED,
            ),
            Some((ValueOwnership::Immortal, PyObjFacts::none_singleton()))
        );
        assert_eq!(
            typed_local_load_direct_result_plan(
                &immortal_expr,
                &LocalEnv::default(),
                &stack_slots,
                None,
                ResultDemand::PYOBJECT_OWNED,
            ),
            None
        );
    }

    fn local_env_store_test_state(
        stack_slot_names: &[&str],
        initial_storage: LocalEnvStorage,
    ) -> (LocalEnv, String) {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));
        let mut decref_signature = jit_module.make_signature();
        decref_signature.params.push(ir::AbiParam::new(ptr_ty));
        decref_signature.params.push(ir::AbiParam::new(ptr_ty));

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));
        wrapper_signature.returns.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id =
            declare_local_fn(&mut jit_module, "local_env_store_test", &wrapper_signature)
                .expect("wrapper function should declare");
        let incref_id = declare_local_fn(
            &mut jit_module,
            "local_env_store_test_incref",
            &refcount_signature,
        )
        .expect("incref helper should declare");
        let decref_id = declare_local_fn(
            &mut jit_module,
            "local_env_store_test_decref",
            &refcount_signature,
        )
        .expect("decref helper should declare");

        let mut ctx = jit_module.make_context();
        ctx.func.name = ir::UserFuncName::user(0, wrapper_id.as_u32());
        ctx.func.signature = wrapper_signature;

        let mut env = LocalEnv::default();
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
            let entry = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.switch_to_block(entry);
            fb.seal_block(entry);

            let old_value = fb.block_params(entry)[0];
            let new_value = fb.block_params(entry)[1];
            let null_tstate = fb.ins().iconst(ptr_ty, 0);
            let incref_ref = jit_module.declare_func_in_func(incref_id, &mut fb.func);
            let decref_ref = jit_module.declare_func_in_func(decref_id, &mut fb.func);
            env.entries.push(LocalEnvEntry {
                location: Some(LocalLocation(0)),
                name: "x".to_string(),
                aliases: Vec::new(),
                value: old_value,
                ref_kind: LocalRefKind::Owned,
                storage: initial_storage,
                binding_facts: local_binding_facts_for_stored_value(LocalRefKind::Owned),
                py_facts: None,
            });
            let stack_slots = StackSlots::new(
                &mut fb,
                &stack_slot_names
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<_>>(),
            );

            env.store_location(
                &mut fb,
                LocalLocation(0),
                "x",
                new_value,
                LocalRefKind::Owned,
                None,
                true,
                &stack_slots,
                ptr_ty,
                null_tstate,
                incref_ref,
                decref_ref,
            );
            fb.ins().return_(&[new_value]);
            fb.seal_all_blocks();
            fb.finalize();
        }

        let rendered = ctx.func.display().to_string();
        (env, rendered)
    }

    fn local_env_first_store_test_state(
        stack_slot_names: &[&str],
        allow_local_only_slot_backed_store: bool,
    ) -> (LocalEnv, String) {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));
        wrapper_signature.returns.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id = declare_local_fn(
            &mut jit_module,
            "local_env_first_store_test",
            &wrapper_signature,
        )
        .expect("wrapper function should declare");
        let incref_id = declare_local_fn(
            &mut jit_module,
            "local_env_first_store_test_incref",
            &refcount_signature,
        )
        .expect("incref helper should declare");
        let decref_id = declare_local_fn(
            &mut jit_module,
            "local_env_first_store_test_decref",
            &refcount_signature,
        )
        .expect("decref helper should declare");

        let mut ctx = jit_module.make_context();
        ctx.func.name = ir::UserFuncName::user(0, wrapper_id.as_u32());
        ctx.func.signature = wrapper_signature;

        let mut env = LocalEnv::default();
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
            let entry = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.switch_to_block(entry);
            fb.seal_block(entry);

            let new_value = fb.block_params(entry)[0];
            let null_tstate = fb.ins().iconst(ptr_ty, 0);
            let incref_ref = jit_module.declare_func_in_func(incref_id, &mut fb.func);
            let decref_ref = jit_module.declare_func_in_func(decref_id, &mut fb.func);
            let stack_slots = StackSlots::new(
                &mut fb,
                &stack_slot_names
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<_>>(),
            );

            env.store_location(
                &mut fb,
                LocalLocation(0),
                "x",
                new_value,
                LocalRefKind::Owned,
                None,
                allow_local_only_slot_backed_store,
                &stack_slots,
                ptr_ty,
                null_tstate,
                incref_ref,
                decref_ref,
            );
            fb.ins().return_(&[new_value]);
            fb.seal_all_blocks();
            fb.finalize();
        }

        let rendered = ctx.func.display().to_string();
        (env, rendered)
    }

    #[test]
    fn local_env_store_keeps_new_local_binding_after_rebind() {
        let (env, rendered) = local_env_store_test_state(&[], LocalEnvStorage::LocalOnly);

        assert_eq!(env.entries.len(), 1, "{rendered}");
        assert_eq!(env.entries[0].location, Some(LocalLocation(0)));
        assert_eq!(env.entries[0].name, "x");
        assert_eq!(env.entries[0].ref_kind, LocalRefKind::Owned);
        assert_eq!(env.entries[0].storage, LocalEnvStorage::LocalOnly);
        assert!(
            rendered.contains("call"),
            "owned previous local should still be released after rebinding:\n{rendered}"
        );
    }

    #[test]
    fn local_env_store_preserves_local_only_storage_for_slot_backed_name() {
        let (env, rendered) = local_env_store_test_state(&["x"], LocalEnvStorage::LocalOnly);

        assert_eq!(env.entries.len(), 1, "{rendered}");
        assert_eq!(env.entries[0].storage, LocalEnvStorage::LocalOnly);
        assert!(
            !rendered.contains("stack_store"),
            "local-only slot-backed rebind should avoid stack-slot mirroring:\n{rendered}"
        );
    }

    #[test]
    fn local_env_first_store_uses_local_only_for_slot_backed_name_when_allowed() {
        let (env, rendered) = local_env_first_store_test_state(&["x"], true);

        assert_eq!(env.entries.len(), 1, "{rendered}");
        assert_eq!(env.entries[0].storage, LocalEnvStorage::LocalOnly);
        assert!(
            !rendered.contains("stack_store"),
            "first store should avoid stack-slot mirroring when local-only is allowed:\n{rendered}"
        );
    }

    #[test]
    fn local_env_first_store_uses_stack_mirror_for_slot_backed_name_when_required() {
        let (env, rendered) = local_env_first_store_test_state(&["x"], false);

        assert_eq!(env.entries.len(), 1, "{rendered}");
        assert_eq!(env.entries[0].storage, LocalEnvStorage::StackMirror);
        assert!(
            rendered.contains("stack_store"),
            "first store should keep stack-slot mirroring when local-only is disallowed:\n{rendered}"
        );
    }

    #[test]
    fn local_env_delete_preserves_local_only_storage_for_slot_backed_name() {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id = declare_local_fn(
            &mut jit_module,
            "local_env_delete_local_only_test",
            &wrapper_signature,
        )
        .expect("wrapper function should declare");
        let decref_id = declare_local_fn(
            &mut jit_module,
            "local_env_delete_local_only_test_decref",
            &refcount_signature,
        )
        .expect("decref helper should declare");

        let mut ctx = jit_module.make_context();
        ctx.func.name = ir::UserFuncName::user(0, wrapper_id.as_u32());
        ctx.func.signature = wrapper_signature;

        let mut env = LocalEnv::default();
        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
            let entry = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.switch_to_block(entry);
            fb.seal_block(entry);

            let value = fb.block_params(entry)[0];
            let null_tstate = fb.ins().iconst(ptr_ty, 0);
            let decref_ref = jit_module.declare_func_in_func(decref_id, &mut fb.func);
            let stack_slots = StackSlots::new(&mut fb, &["x".to_string()]);
            env.bind_entry_location_with_aliases(
                LocalLocation(0),
                "x",
                Vec::new(),
                value,
                LocalRefKind::Owned,
                LocalEnvStorage::LocalOnly,
                ParamBindingFacts::DefinitelyBound,
                None,
            );

            env.delete_location(
                &mut fb,
                LocalLocation(0),
                "x",
                &stack_slots,
                ptr_ty,
                null_tstate,
                decref_ref,
            )
            .expect("local-only delete should succeed");
            fb.ins().return_(&[]);
            fb.seal_all_blocks();
            fb.finalize();
        }

        let rendered = ctx.func.display().to_string();
        assert_eq!(env.entries.len(), 1, "{rendered}");
        assert_eq!(env.entries[0].storage, LocalEnvStorage::LocalOnly);
        assert_eq!(env.entries[0].ref_kind, LocalRefKind::Unbound);
        assert!(
            !rendered.contains("stack_store") && !rendered.contains("stack_load"),
            "deleting a local-only slot-backed binding should not touch the stack slot:\n{rendered}"
        );
    }

    #[test]
    fn local_ref_forwarding_increfs_borrowed_and_duplicate_owned_values() {
        assert!(!local_ref_kind_needs_incref_for_forward(
            LocalRefKind::Owned,
            0
        ));
        assert!(local_ref_kind_needs_incref_for_forward(
            LocalRefKind::Owned,
            1
        ));
        assert!(local_ref_kind_needs_incref_for_forward(
            LocalRefKind::Borrowed,
            0
        ));
        assert!(!local_ref_kind_needs_incref_for_forward(
            LocalRefKind::Immortal,
            0
        ));
    }

    #[test]
    fn stack_mirror_local_ref_kind_borrows_non_immortal_values() {
        assert_eq!(
            local_ref_kind_for_stack_mirror(LocalRefKind::Owned),
            LocalRefKind::Borrowed
        );
        assert_eq!(
            local_ref_kind_for_stack_mirror(LocalRefKind::Unknown),
            LocalRefKind::Borrowed
        );
        assert_eq!(
            local_ref_kind_for_stack_mirror(LocalRefKind::Immortal),
            LocalRefKind::Immortal
        );
    }

    fn render_test_jit_function(
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
    ) -> String {
        render_test_jit_function_with_module_constants(function, blocks, Vec::new())
    }

    #[test]
    fn process_jit_registry_does_not_reuse_colliding_function_ids_with_different_shapes() {
        let compile_session = crate::session::CompileSession::new();
        let module =
            ProcessJitModule::new(&compile_session).expect("process JIT module should initialize");
        let mut jit_module = module
            .lock_for_serial_phase()
            .expect("process JIT module should lock");
        let mut state = ProcessJitState::new();
        let first = test_function();
        let mut second = test_function();
        second.params.params.push(Param {
            name: "x".into(),
            kind: ParamKind::Any,
            has_default: false,
        });

        let first_decl = state
            .declare_direct_function(&mut jit_module, &first, None)
            .expect("first function should declare");
        let first_decl_again = state
            .declare_direct_function(&mut jit_module, &first, None)
            .expect("same shape should reuse declaration");
        assert_eq!(first_decl.symbol, first_decl_again.symbol);

        let session = std::sync::Arc::new(crate::session::CompileSession::new());
        let first_handle = state
            .mark_direct_function_ready(
                &session,
                first.function_id,
                1usize as *const u8,
                1usize as *const u8,
                first.params.len(),
                std::sync::Arc::new(RuntimeJitDeoptTable {
                    function_id: first.function_id,
                    function: Box::new(first.clone()),
                    module_constant_ptrs: Vec::new(),
                    points: Vec::new(),
                }),
                JitCodegenStats::default(),
            )
            .expect("first function should mark ready");
        let ready_handle = state
            .ready_direct_function(&first)
            .expect("first function should be ready");
        assert!(std::sync::Arc::ptr_eq(&first_handle, &ready_handle));
        assert!(state.ready_direct_function(&second).is_none());

        let second_decl = state
            .declare_direct_function(&mut jit_module, &second, None)
            .expect("colliding function id with different shape should redeclare");
        assert_ne!(first_decl.symbol, second_decl.symbol);
    }

    #[test]
    fn process_jit_batch_collection_skips_cross_module_targets_for_lazy_env_lookup() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let result = Python::attach(|py| {
            let session = std::sync::Arc::new(crate::session::CompileSession::new());
            let caller_module_name_gen = ModuleNameGen::new(91);
            let callee_module_name_gen = ModuleNameGen::new(92);
            let callee = test_function_in_module(&callee_module_name_gen, "callee");
            let caller = test_function_in_module(&caller_module_name_gen, "caller");
            let direct_call = InstrCodegen::CallDirect(CallDirect::new(
                none_expr(),
                callee.function_id,
                Vec::<CallArgPositional<InstrCodegen>>::new(),
                Vec::<CallArgKeyword<InstrCodegen>>::new(),
            ));
            let caller = with_single_test_block(caller, vec![direct_call], ret_term(none_expr()));
            let caller_state = crate::module_type::build_shared_state_for_testing(
                py,
                test_module(caller_module_name_gen, vec![caller.clone()]),
                "caller_test",
                "",
            )
            .expect("caller shared state should build");
            let callee_state = crate::module_type::build_shared_state_for_testing(
                py,
                test_module(callee_module_name_gen, vec![callee.clone()]),
                "callee_test",
                "",
            )
            .expect("callee shared state should build");
            session
                .retain_shared_module_state(std::sync::Arc::clone(&caller_state))
                .expect("caller state should be retained");
            session
                .retain_shared_module_state(callee_state)
                .expect("callee state should be retained");

            let batch =
                collect_process_jit_batch_functions(&session, &caller, Some(caller_state.as_ref()))
                    .expect("cross-module process JIT batch should collect");
            let function_ids = batch
                .iter()
                .map(|batch_function| batch_function.function.function_id)
                .collect::<Vec<_>>();
            assert_eq!(function_ids, vec![caller.function_id]);
            assert_eq!(
                caller_state
                    .lookup_direct_call_target_function(session.as_ref(), callee.function_id)
                    .expect("cross-module target lookup should succeed")
                    .expect("cross-module target metadata should resolve")
                    .function_id,
                callee.function_id
            );
        });
        result
    }

    #[test]
    fn process_jit_batch_collection_handles_recursive_direct_call() {
        let module_name_gen = ModuleNameGen::new(93);
        let function = test_function_in_module(&module_name_gen, "recursive");
        let function = with_single_test_block(
            function.clone(),
            vec![direct_call_expr(function.function_id)],
            ret_term(none_expr()),
        );

        let session = std::sync::Arc::new(crate::session::CompileSession::new());
        let batch = collect_process_jit_batch_functions(&session, &function, None)
            .expect("recursive process JIT batch should collect");
        let function_ids = batch
            .iter()
            .map(|batch_function| batch_function.function.function_id)
            .collect::<Vec<_>>();
        assert_eq!(function_ids, vec![function.function_id]);
    }

    #[test]
    fn process_jit_compile_direct_function_handles_mutual_recursion() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let result = Python::attach(|py| {
            let session = std::sync::Arc::new(crate::session::CompileSession::new());
            let module_name_gen = ModuleNameGen::new(94);
            let first = test_function_in_module(&module_name_gen, "first");
            let second = test_function_in_module(&module_name_gen, "second");
            let first = with_single_test_block(
                first.clone(),
                vec![direct_call_expr(second.function_id)],
                ret_term(none_expr()),
            );
            let second = with_single_test_block(
                second.clone(),
                vec![direct_call_expr(first.function_id)],
                ret_term(none_expr()),
            );
            let shared_state = crate::module_type::build_shared_state_for_testing(
                py,
                test_module(module_name_gen, vec![first.clone(), second.clone()]),
                "mutual_recursion_test",
                "",
            )
            .expect("shared state should build");
            session
                .retain_shared_module_state(std::sync::Arc::clone(&shared_state))
                .expect("shared state should be retained");

            let engine =
                ProcessJitEngine::new(session.as_ref()).expect("process JIT should construct");
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let blocks = vec![std::ptr::null_mut::<c_void>(); first.blocks.len()];
            let compiled = unsafe {
                engine.compile_direct_function(
                    &session,
                    blocks.as_slice(),
                    &shared_state.lowered_module,
                    &first,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(shared_state.as_ref()),
                )
            }
            .expect("mutually-recursive process JIT batch should compile");
            assert!(compiled.compiled);
            let state = engine
                .state
                .lock()
                .expect("process JIT state lock should not be poisoned");
            let first_handle = state
                .ready_direct_function(&first)
                .expect("root function should be marked ready");
            let second_handle = state
                .ready_direct_function(&second)
                .expect("mutually-recursive callee should be marked ready");
            let first_deopt_table = first_handle
                .direct_deopt_table()
                .expect("root compiled handle should carry deopt metadata");
            assert_eq!(first_deopt_table.function_id(), first.function_id);
            assert!(
                first_deopt_table.len() >= first.blocks.len() * 2,
                "block entry and before-term points should be available for each block"
            );
            let first_entry_record = first_deopt_table
                .record_for_point(LocalEnvResumePoint::BlockEntry {
                    function_id: first.function_id,
                    block: first.blocks[0].label,
                })
                .expect("block-entry deopt point should be addressable by resume point");
            assert_eq!(first_entry_record.id().function_id, first.function_id);
            assert_eq!(
                first_entry_record.ordinal(),
                0,
                "runtime deopt records should preserve planned ordinal ids"
            );
            assert_eq!(
                first_entry_record.precision(),
                LocalEnvResumeStatePrecision::BlockEntry
            );
            assert_eq!(
                first_deopt_table
                    .record_for_ordinal(first_entry_record.ordinal() as i64)
                    .expect("runtime deopt records should be addressable by ordinal")
                    .resume_point(),
                first_entry_record.resume_point()
            );
            assert_eq!(
                first_deopt_table
                    .record_for_ordinal(first_entry_record.ordinal() as i64)
                    .expect("runtime deopt records should be addressable by ordinal")
                    .locals(),
                first_entry_record.locals()
            );
            let first_entry_description = first_deopt_table
                .describe_record_ordinal(first_entry_record.ordinal() as i64)
                .expect("runtime deopt record should be describable by ordinal");
            assert!(
                first_entry_description.contains(&format!("function {}", first.function_id))
                    && first_entry_description.contains("record 0"),
                "runtime deopt record descriptions should include stable lookup context: {first_entry_description}"
            );
            assert!(
                first_deopt_table.describe_record_ordinal(-1).is_err(),
                "runtime deopt record lookup should reject negative ordinals"
            );
            assert!(
                first_deopt_table.record_for_ordinal(-1).is_err(),
                "structured runtime deopt record lookup should reject negative ordinals"
            );
            assert_eq!(
                compiled_direct_deopt_table_ptr(first_handle.raw_handle())
                    .expect("root deopt table pointer should be available"),
                std::sync::Arc::as_ptr(&first_deopt_table) as ObjPtr,
                "compiled direct handle should expose the runtime deopt table pointer"
            );
            assert_eq!(
                first_entry_record.continuation(),
                &RuntimeJitDeoptContinuation::ResumeBlockTail {
                    cursor: RuntimeJitDeoptCursor::at_block_entry(first.blocks[0].label),
                },
                "block-entry deopt records should now be executable from body index 0"
            );
            let second_deopt_table = second_handle
                .direct_deopt_table()
                .expect("callee compiled handle should carry deopt metadata");
            assert_eq!(second_deopt_table.function_id(), second.function_id);
        });
        result
    }

    #[test]
    fn process_jit_compile_direct_function_leaves_cross_module_callee_lazy() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        let result = Python::attach(|py| {
            let session = std::sync::Arc::new(crate::session::CompileSession::new());
            let caller_module_name_gen = ModuleNameGen::new(96);
            let callee_module_name_gen = ModuleNameGen::new(97);
            let callee = test_function_in_module(&callee_module_name_gen, "callee");
            let caller = test_function_in_module(&caller_module_name_gen, "caller");
            let caller = with_single_test_block(
                caller.clone(),
                vec![direct_call_expr(callee.function_id)],
                ret_term(none_expr()),
            );
            let caller_state = crate::module_type::build_shared_state_for_testing(
                py,
                test_module(caller_module_name_gen, vec![caller.clone()]),
                "caller_test",
                "",
            )
            .expect("caller shared state should build");
            let callee_state = crate::module_type::build_shared_state_for_testing(
                py,
                test_module(callee_module_name_gen, vec![callee.clone()]),
                "callee_test",
                "",
            )
            .expect("callee shared state should build");
            session
                .retain_shared_module_state(std::sync::Arc::clone(&caller_state))
                .expect("caller state should be retained");
            session
                .retain_shared_module_state(callee_state)
                .expect("callee state should be retained");

            let engine =
                ProcessJitEngine::new(session.as_ref()).expect("process JIT should construct");
            let module_constant_ptrs = caller_state.module_constant_ptrs();
            let blocks = vec![std::ptr::null_mut::<c_void>(); caller.blocks.len()];
            let compiled = unsafe {
                engine.compile_direct_function(
                    &session,
                    blocks.as_slice(),
                    &caller_state.lowered_module,
                    &caller,
                    &caller_state.codegen_constants,
                    &caller_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(caller_state.as_ref()),
                )
            }
            .expect("cross-module caller should compile without precompiling the callee");
            assert!(compiled.compiled);
            let state = engine
                .state
                .lock()
                .expect("process JIT state lock should not be poisoned");
            assert!(
                state.ready_direct_function(&caller).is_some(),
                "root function should be marked ready",
            );
            assert!(
                state.ready_direct_function(&callee).is_none(),
                "cross-module callee should stay lazy until its FunctionEnv is materialized",
            );
        });
        result
    }

    fn render_test_jit_function_with_module_constants(
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
        module_constants: Vec<InstrResolved>,
    ) -> String {
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = module_constants;
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        render_test_jit_function_with_constants(&module, &function, blocks, &module_constants)
    }

    fn render_test_jit_function_with_block_entry_counts(
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
        module_constants: Vec<InstrResolved>,
        block_entry_counts: &[(BlockLabel, u64)],
        enable_profiled_cold_blocks: bool,
    ) -> String {
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = module_constants;
        let function = module.callable_defs[0].clone();
        let module_name = "counter_test";
        let soac_work_dir = fresh_test_work_dir("test-work");
        write_test_counter_dump(
            soac_work_dir.join("profile.bin").as_path(),
            &CounterDumpRecord {
                source_hash: 0,
                module_name: module_name.to_string(),
                package_name: None,
                rows: block_entry_counts
                    .iter()
                    .enumerate()
                    .map(|(index, (block_label, count))| CounterDumpRow {
                        counter_id: u32::try_from(index)
                            .expect("test block-entry counter count should fit in u32"),
                        scope: "this".to_string(),
                        kind: "block_entry".to_string(),
                        site_kind: "block_entry".to_string(),
                        function_id: Some(function.function_id),
                        current_function_id: Some(function.function_id),
                        instr_id: None,
                        function_qualname: Some(function.names.qualname.clone()),
                        block_label: Some(block_label.to_string()),
                        value: *count,
                        branch_values: Vec::new(),
                        observed_value: None,
                        max_overcount: None,
                    })
                    .collect(),
                module_keys: Vec::new(),
                type_keys: Vec::new(),
                type_table: Vec::new(),
            },
        );

        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let old_soac_work_dir = std::env::var_os("SOAC_WORK_DIR");
        let old_soac_opt_mode = std::env::var_os("SOAC_OPT_MODE");
        let old_profiled_cold_blocks = std::env::var_os("SOAC_ENABLE_PROFILED_COLD_BLOCKS");
        unsafe {
            std::env::set_var("SOAC_WORK_DIR", &soac_work_dir);
            std::env::set_var("SOAC_OPT_MODE", "apply");
            if enable_profiled_cold_blocks {
                std::env::set_var("SOAC_ENABLE_PROFILED_COLD_BLOCKS", "1");
            } else {
                std::env::remove_var("SOAC_ENABLE_PROFILED_COLD_BLOCKS");
            }
        }

        crate::initialize_test_python();

        let rendered = Python::attach(|py| {
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, module, module_name, "")
                    .expect("shared state should build");
            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let module_constant_object_data_ids = declare_module_constant_object_data(
                &mut jit_module,
                &shared_state.lowered_module,
                &module_constant_ptrs,
            )
            .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, _top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    &shared_state.lowered_module.counter_defs,
                );
            let top_value_counter_data_id = declare_shared_state_top_value_counter_storage(
                &mut jit_module,
                shared_state.as_ref(),
            );
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks,
                &shared_state.lowered_module,
                &function,
                &shared_state.codegen_constants,
                &shared_state.lowered_module.counter_defs,
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                &compile_session,
                Some(shared_state.as_ref()),
                None,
                None,
                BuildSpecializedFunctionOptions::default(),
            )
            .expect("specialized JIT build should succeed");
            let (clif, _cfg_dot, _vcode_disasm) = render_compiled_clif_and_vcode_disasm(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.ctx,
                &built.import_id_to_symbol,
                &built.block_annotations,
            )
            .expect("specialized JIT CLIF render should succeed");
            clif
        });

        unsafe {
            match old_soac_work_dir {
                Some(value) => std::env::set_var("SOAC_WORK_DIR", value),
                None => std::env::remove_var("SOAC_WORK_DIR"),
            }
            match old_soac_opt_mode {
                Some(value) => std::env::set_var("SOAC_OPT_MODE", value),
                None => std::env::remove_var("SOAC_OPT_MODE"),
            }
            match old_profiled_cold_blocks {
                Some(value) => std::env::set_var("SOAC_ENABLE_PROFILED_COLD_BLOCKS", value),
                None => std::env::remove_var("SOAC_ENABLE_PROFILED_COLD_BLOCKS"),
            }
        }

        rendered
    }

    #[test]
    fn reloc_type_ref_uses_cpython_symbols_for_builtin_types() {
        let long_ref = reloc_type_ref_for_type(std::ptr::addr_of_mut!(PyLong_Type))
            .expect("builtin type relocation should not error");
        assert_eq!(
            long_ref,
            Some(RelocTypeRef::CpythonTypeSymbol(CpythonTypeSymbol::Long))
        );
        assert_eq!(
            resolve_reloc_type_ref_to_type(long_ref.as_ref().expect("reloc ref should exist"))
                .expect("builtin symbol should resolve"),
            Some(std::ptr::addr_of_mut!(PyLong_Type))
        );
    }

    #[test]
    fn field_index_layouts_prime_owner_type_key_layouts() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();

        Python::attach(|py| {
            let module = PyModule::from_code(
                py,
                c"
class Point:
    pass
",
                c"field_type_test.py",
                c"field_type_test",
            )
            .expect("test module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_type_test", module.as_any())
                .expect("test module should be registered");
            let owner_type = module
                .getattr("Point")
                .expect("Point should exist")
                .as_ptr() as *mut ffi::PyTypeObject;

            assert!(
                unsafe { owner_type_supports_field_layout_priming(owner_type) },
                "expected Point to support field-layout priming"
            );
            assert_eq!(
                cached_split_key_layout(py, owner_type),
                Vec::<(String, u32)>::new()
            );

            prime_field_index_layout(
                owner_type,
                &[
                    CollectedTypeKeyLayout {
                        owner_type_id: 7,
                        key: "x".to_string(),
                        index: 0,
                    },
                    CollectedTypeKeyLayout {
                        owner_type_id: 7,
                        key: "y".to_string(),
                        index: 1,
                    },
                ],
            )
            .expect("field-layout priming should succeed");

            assert!(
                cached_split_key_layout(py, owner_type)
                    .starts_with(&[("x".to_string(), 0), ("y".to_string(), 1)]),
                "expected priming to recreate Point split-key layout"
            );

            modules
                .del_item("field_type_test")
                .expect("test module should be removed");
        });
    }

    #[test]
    fn field_index_specialized_setattr_hits_apply_mode_first_insert() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_index_specialized_setattr_hits_apply_mode_first_insert",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let old_soac_work_dir = std::env::var_os("SOAC_WORK_DIR");
        let old_soac_opt_mode = std::env::var_os("SOAC_OPT_MODE");
        let soac_work_dir = fresh_test_work_dir("test-work");

        unsafe {
            std::env::set_var("SOAC_WORK_DIR", &soac_work_dir);
            std::env::set_var("SOAC_OPT_MODE", "apply");
        }
        crate::initialize_test_python();

        Python::attach(|py| {
            let owner_module = PyModule::from_code(
                py,
                c"
class Point:
    pass
",
                c"field_type_test.py",
                c"field_type_test",
            )
            .expect("owner module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_type_test", owner_module.as_any())
                .expect("owner module should be registered");

            write_test_counter_dump(
                soac_work_dir.join("profile.bin").as_path(),
                &CounterDumpRecord {
                    source_hash: 0,
                    module_name: "counter_test".to_string(),
                    package_name: None,
                    rows: Vec::new(),
                    module_keys: Vec::new(),
                    type_keys: vec![CounterDumpTypeKeyLayout {
                        owner_type_id: 7,
                        key: "x".to_string(),
                        index: 0,
                    }],
                    type_table: vec![CounterDumpTypeTableEntry {
                        type_id: 7,
                        key: CounterDumpTypeKey {
                            module_name: "field_type_test".to_string(),
                            qualname: "Point".to_string(),
                        },
                    }],
                },
            );
            let mut lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def write_point(point, value):
    point.x = value
    return point.x
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            instrument_module_with_legacy_call_target_counters(&mut lowered);
            let function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "write_point")
                .expect("missing lowered function write_point")
                .clone();
            let setattr_instr_id = function
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .find_map(|expr| match expr {
                    InstrCodegen::SetAttr(_) => Some(expr.semantic_instr_id()),
                    _ => None,
                })
                .expect("write_point should contain a SetAttr");
            let (hit_counter_id, hit_branch_id) = runtime_branch_counter_for(
                &lowered.counter_defs,
                function.function_id,
                setattr_instr_id,
                "field_access",
                "indexed_hit",
            );
            let (fallback_counter_id, fallback_branch_id) = runtime_branch_counter_for(
                &lowered.counter_defs,
                function.function_id,
                setattr_instr_id,
                "field_access",
                "indexed_fallback",
            );

            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "counter_test", "")
                    .expect("shared state should build");
            let runtime = unsafe { build_test_module_runtime(py, shared_state.clone()) };
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let compile_session = crate::session::CompileSession::process();
            let compiled_handle = unsafe {
                compile_cranelift_run_bb_specialized_cached(
                    &compile_session,
                    &blocks,
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(shared_state.as_ref()),
                )
            }
            .expect("specialized write_point should compile");
            let (code_ptr, _default_code_ptr, param_count) = compiled_handle
                .handle
                .direct_runner_info()
                .expect("compiled direct runner should expose entrypoint");
            assert_eq!(param_count, 2, "write_point should take two direct args");
            let entry: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
            ) -> *mut c_void = unsafe { std::mem::transmute(code_ptr) };

            let point_type = owner_module
                .getattr("Point")
                .expect("Point should exist on owner module");
            let point = unsafe { ffi::PyObject_CallNoArgs(point_type.as_ptr()) };
            assert!(!point.is_null(), "Point() should create a test instance");
            unsafe { ffi::Py_INCREF(point) };
            let value = unsafe { ffi::PyLong_FromLong(1_234_567) };
            assert!(!value.is_null(), "test value should allocate");

            let mut function_context = test_function_jit_context(&runtime, std::ptr::null_mut());
            let thread_state = unsafe { ffi::PyThreadState_Get() }.cast::<c_void>();
            let result = unsafe {
                entry(
                    std::ptr::addr_of_mut!(function_context).cast(),
                    thread_state,
                    point.cast(),
                    value.cast(),
                )
            };
            assert!(
                !result.is_null(),
                "write_point should return the stored value"
            );

            assert_eq!(
                shared_state.counter_branch_value(hit_counter_id, hit_branch_id),
                1,
                "apply-mode SetAttr should take the indexed-store fast path"
            );
            assert_eq!(
                shared_state.counter_branch_value(fallback_counter_id, fallback_branch_id),
                0,
                "apply-mode SetAttr should avoid the generic setattr fallback"
            );

            let point_obj = unsafe { pyo3::Bound::from_borrowed_ptr(py, point) };
            let stored = point_obj
                .getattr("x")
                .expect("Point instance should now expose x");
            assert_eq!(
                stored.extract::<i64>().expect("stored x should be an int"),
                1_234_567
            );
            let result_obj = unsafe { pyo3::Bound::from_owned_ptr(py, result.cast()) };
            assert_eq!(
                result_obj
                    .extract::<i64>()
                    .expect("write_point result should be an int"),
                1_234_567
            );

            unsafe { ffi::Py_DECREF(point) };
            modules
                .del_item("field_type_test")
                .expect("owner module should be removed");
        });

        unsafe {
            match old_soac_work_dir {
                Some(value) => std::env::set_var("SOAC_WORK_DIR", value),
                None => std::env::remove_var("SOAC_WORK_DIR"),
            }
            match old_soac_opt_mode {
                Some(value) => std::env::set_var("SOAC_OPT_MODE", value),
                None => std::env::remove_var("SOAC_OPT_MODE"),
            }
        }
    }

    #[test]
    fn field_index_specialized_constructor_stores_hit_verify_mode_first_inserts() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_index_specialized_constructor_stores_hit_verify_mode_first_inserts",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let old_soac_work_dir = std::env::var_os("SOAC_WORK_DIR");
        let old_soac_opt_mode = std::env::var_os("SOAC_OPT_MODE");
        let soac_work_dir = fresh_test_work_dir("test-work");

        unsafe {
            std::env::set_var("SOAC_WORK_DIR", &soac_work_dir);
            std::env::set_var("SOAC_OPT_MODE", "verify");
        }
        crate::initialize_test_python();

        Python::attach(|py| {
            let owner_module = PyModule::from_code(
                py,
                c"
class Record:
    pass
",
                c"field_record_test.py",
                c"field_record_test",
            )
            .expect("test module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_record_test", owner_module.as_any())
                .expect("owner module should be registered");
            let record_type = owner_module
                .getattr("Record")
                .expect("Record should exist on owner module");
            let owner_type = record_type.as_ptr() as *mut ffi::PyTypeObject;
            assert_eq!(
                cached_split_key_layout(py, owner_type),
                Vec::<(String, u32)>::new(),
                "empty __static_attributes__ should leave SOAC's profile input as the split-key source"
            );

            write_test_counter_dump(
                soac_work_dir.join("profile.bin").as_path(),
                &CounterDumpRecord {
                    source_hash: 0,
                    module_name: "counter_test".to_string(),
                    package_name: None,
                    rows: Vec::new(),
                    module_keys: Vec::new(),
                    type_keys: vec![
                        CounterDumpTypeKeyLayout {
                            owner_type_id: 7,
                            key: "PtrComp".to_string(),
                            index: 0,
                        },
                        CounterDumpTypeKeyLayout {
                            owner_type_id: 7,
                            key: "Discr".to_string(),
                            index: 1,
                        },
                        CounterDumpTypeKeyLayout {
                            owner_type_id: 7,
                            key: "EnumComp".to_string(),
                            index: 2,
                        },
                        CounterDumpTypeKeyLayout {
                            owner_type_id: 7,
                            key: "IntComp".to_string(),
                            index: 3,
                        },
                        CounterDumpTypeKeyLayout {
                            owner_type_id: 7,
                            key: "StringComp".to_string(),
                            index: 4,
                        },
                    ],
                    type_table: vec![CounterDumpTypeTableEntry {
                        type_id: 7,
                        key: CounterDumpTypeKey {
                            module_name: "field_record_test".to_string(),
                            qualname: "Record".to_string(),
                        },
                    }],
                },
            );
            let mut lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
class Record:
    def __init__(self, PtrComp=None, Discr=0, EnumComp=0, IntComp=0, StringComp=0):
        self.PtrComp = PtrComp
        self.Discr = Discr
        self.EnumComp = EnumComp
        self.IntComp = IntComp
        self.StringComp = StringComp
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            instrument_module_with_legacy_call_target_counters(&mut lowered);
            let function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.qualname == "Record.__init__")
                .expect("missing lowered function Record.__init__")
                .clone();
            let setattr_instr_ids = function
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .filter_map(|expr| match expr {
                    InstrCodegen::SetAttr(_) => Some(expr.semantic_instr_id()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                setattr_instr_ids.len(),
                5,
                "Record.__init__ should contain five SetAttr operations"
            );
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "counter_test", "")
                    .expect("shared state should build");
            let hit_counter_ids = setattr_instr_ids
                .iter()
                .map(|setattr_instr_id| {
                    runtime_branch_counter_for(
                        &shared_state.lowered_module.counter_defs,
                        function.function_id,
                        *setattr_instr_id,
                        "field_access",
                        "indexed_hit",
                    )
                })
                .collect::<Vec<_>>();
            let fallback_counter_ids = setattr_instr_ids
                .iter()
                .map(|setattr_instr_id| {
                    runtime_branch_counter_for(
                        &shared_state.lowered_module.counter_defs,
                        function.function_id,
                        *setattr_instr_id,
                        "field_access",
                        "indexed_fallback",
                    )
                })
                .collect::<Vec<_>>();

            let runtime = unsafe { build_test_module_runtime(py, shared_state.clone()) };
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let compile_session = crate::session::CompileSession::process();
            let compiled_handle = unsafe {
                compile_cranelift_run_bb_specialized_cached(
                    &compile_session,
                    &blocks,
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(shared_state.as_ref()),
                )
            }
            .expect("specialized Record.__init__ should compile");
            let (code_ptr, _default_code_ptr, param_count) = compiled_handle
                .handle
                .direct_runner_info()
                .expect("compiled direct runner should expose entrypoint");
            assert_eq!(
                param_count, 6,
                "Record.__init__ should take six direct args"
            );
            let entry: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
            ) -> *mut c_void = unsafe { std::mem::transmute(code_ptr) };

            let record = unsafe { ffi::PyType_GenericAlloc(owner_type, 0) };
            assert!(
                !record.is_null(),
                "PyType_GenericAlloc should create a fresh uninitialized Record instance"
            );
            let none = unsafe { ffi::Py_None() };
            unsafe { ffi::Py_INCREF(none) };
            let discr = unsafe { ffi::PyLong_FromLong(1) };
            let enum_comp = unsafe { ffi::PyLong_FromLong(2) };
            let int_comp = unsafe { ffi::PyLong_FromLong(3) };
            let string_comp = unsafe { ffi::PyUnicode_FromString(c"value".as_ptr()) };
            assert!(
                !discr.is_null()
                    && !enum_comp.is_null()
                    && !int_comp.is_null()
                    && !string_comp.is_null(),
                "test values should allocate"
            );

            let mut function_context = test_function_jit_context(&runtime, std::ptr::null_mut());
            let thread_state = unsafe { ffi::PyThreadState_Get() }.cast::<c_void>();
            let result = unsafe {
                entry(
                    std::ptr::addr_of_mut!(function_context).cast(),
                    thread_state,
                    record.cast(),
                    none.cast(),
                    discr.cast(),
                    enum_comp.cast(),
                    int_comp.cast(),
                    string_comp.cast(),
                )
            };
            assert!(!result.is_null(), "Record.__init__ should return None");
            unsafe { ffi::Py_DECREF(result.cast::<ffi::PyObject>()) };

            for (counter_id, branch_id) in hit_counter_ids {
                assert_eq!(
                    shared_state.counter_branch_value(counter_id, branch_id),
                    1,
                    "constructor SetAttr should take the indexed-store fast path"
                );
            }
            for (counter_id, branch_id) in fallback_counter_ids {
                assert_eq!(
                    shared_state.counter_branch_value(counter_id, branch_id),
                    0,
                    "verify-mode constructor SetAttr should avoid the generic setattr fallback"
                );
            }

            let record_obj = unsafe { pyo3::Bound::from_borrowed_ptr(py, record) };
            assert_eq!(
                record_obj
                    .getattr("IntComp")
                    .expect("Record should expose IntComp")
                    .extract::<i64>()
                    .expect("IntComp should be an int"),
                3
            );

            unsafe { ffi::Py_DECREF(record) };
            modules
                .del_item("field_record_test")
                .expect("owner module should be removed");
        });

        unsafe {
            match old_soac_work_dir {
                Some(value) => std::env::set_var("SOAC_WORK_DIR", value),
                None => std::env::remove_var("SOAC_WORK_DIR"),
            }
            match old_soac_opt_mode {
                Some(value) => std::env::set_var("SOAC_OPT_MODE", value),
                None => std::env::remove_var("SOAC_OPT_MODE"),
            }
        }
    }

    fn first_getattr_instr_id(function: &BlockPyFunction<CodegenModuleShape>) -> InstrId {
        struct GetAttrFinder {
            instr_id: Option<InstrId>,
        }

        impl Visit<InstrCodegen> for GetAttrFinder {
            fn visit_instr(&mut self, expr: &InstrCodegen)
            where
                InstrCodegen: ChildVisitable<InstrCodegen>,
            {
                if self.instr_id.is_none()
                    && let InstrCodegen::GetAttr(_) = expr
                {
                    self.instr_id = Some(expr.semantic_instr_id());
                }
                expr.visit_children(self);
            }
        }

        let mut finder = GetAttrFinder { instr_id: None };
        for block in &function.blocks {
            for expr in &block.body {
                finder.visit_instr(expr);
            }
            finder.visit_term(&block.term);
        }
        finder.instr_id.expect("function should contain a GetAttr")
    }

    #[test]
    fn field_index_specialized_getattr_hits_apply_mode_fast_path() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_index_specialized_getattr_hits_apply_mode_fast_path",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let old_soac_work_dir = std::env::var_os("SOAC_WORK_DIR");
        let old_soac_opt_mode = std::env::var_os("SOAC_OPT_MODE");
        let soac_work_dir = fresh_test_work_dir("test-work");

        unsafe {
            std::env::set_var("SOAC_WORK_DIR", &soac_work_dir);
            std::env::set_var("SOAC_OPT_MODE", "apply");
        }
        crate::initialize_test_python();

        Python::attach(|py| {
            let owner_module = PyModule::from_code(
                py,
                c"
class Point:
    pass
",
                c"field_type_test.py",
                c"field_type_test",
            )
            .expect("owner module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_type_test", owner_module.as_any())
                .expect("owner module should be registered");

            write_test_counter_dump(
                soac_work_dir.join("profile.bin").as_path(),
                &CounterDumpRecord {
                    source_hash: 0,
                    module_name: "counter_test".to_string(),
                    package_name: None,
                    rows: Vec::new(),
                    module_keys: Vec::new(),
                    type_keys: vec![CounterDumpTypeKeyLayout {
                        owner_type_id: 7,
                        key: "x".to_string(),
                        index: 0,
                    }],
                    type_table: vec![CounterDumpTypeTableEntry {
                        type_id: 7,
                        key: CounterDumpTypeKey {
                            module_name: "field_type_test".to_string(),
                            qualname: "Point".to_string(),
                        },
                    }],
                },
            );

            let mut lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def read_point(point):
    return point.x
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            instrument_module_with_legacy_call_target_counters(&mut lowered);
            let function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "read_point")
                .expect("missing lowered function read_point")
                .clone();
            let getattr_instr_id = first_getattr_instr_id(&function);
            let (hit_counter_id, hit_branch_id) = runtime_branch_counter_for(
                &lowered.counter_defs,
                function.function_id,
                getattr_instr_id,
                "field_access",
                "indexed_hit",
            );
            let (fallback_counter_id, fallback_branch_id) = runtime_branch_counter_for(
                &lowered.counter_defs,
                function.function_id,
                getattr_instr_id,
                "field_access",
                "indexed_fallback",
            );

            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "counter_test", "")
                    .expect("shared state should build");
            let runtime = unsafe { build_test_module_runtime(py, shared_state.clone()) };
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let compile_session = crate::session::CompileSession::process();
            let compiled_handle = unsafe {
                compile_cranelift_run_bb_specialized_cached(
                    &compile_session,
                    &blocks,
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(shared_state.as_ref()),
                )
            }
            .expect("specialized read_point should compile");
            let (code_ptr, _default_code_ptr, param_count) = compiled_handle
                .handle
                .direct_runner_info()
                .expect("compiled direct runner should expose entrypoint");
            assert_eq!(param_count, 1, "read_point should take one direct arg");
            let entry: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
                unsafe { std::mem::transmute(code_ptr) };

            let point_type = owner_module
                .getattr("Point")
                .expect("Point should exist on owner module");
            let point = unsafe { ffi::PyObject_CallNoArgs(point_type.as_ptr()) };
            assert!(!point.is_null(), "Point() should create a test instance");
            let point_obj = unsafe { pyo3::Bound::from_borrowed_ptr(py, point) };
            point_obj
                .setattr("x", 98_765_i64)
                .expect("Point instance should accept x");

            let mut function_context = test_function_jit_context(&runtime, std::ptr::null_mut());
            let thread_state = unsafe { ffi::PyThreadState_Get() }.cast::<c_void>();
            let result = unsafe {
                entry(
                    std::ptr::addr_of_mut!(function_context).cast(),
                    thread_state,
                    point.cast(),
                )
            };
            assert!(
                !result.is_null(),
                "read_point should return the stored value"
            );

            assert_eq!(
                shared_state.counter_branch_value(hit_counter_id, hit_branch_id),
                1,
                "apply-mode GetAttr should take the indexed-load fast path"
            );
            assert_eq!(
                shared_state.counter_branch_value(fallback_counter_id, fallback_branch_id),
                0,
                "apply-mode GetAttr should avoid the generic getattr fallback"
            );

            let result_obj = unsafe { pyo3::Bound::from_owned_ptr(py, result.cast()) };
            assert_eq!(
                result_obj
                    .extract::<i64>()
                    .expect("read_point result should be an int"),
                98_765
            );

            unsafe { ffi::Py_DECREF(point) };
            modules
                .del_item("field_type_test")
                .expect("owner module should be removed");
        });

        unsafe {
            match old_soac_work_dir {
                Some(value) => std::env::set_var("SOAC_WORK_DIR", value),
                None => std::env::remove_var("SOAC_WORK_DIR"),
            }
            match old_soac_opt_mode {
                Some(value) => std::env::set_var("SOAC_OPT_MODE", value),
                None => std::env::remove_var("SOAC_OPT_MODE"),
            }
        }
    }

    #[test]
    fn v3_field_indexed_getattr_store_rhs_hits_apply_mode_fast_path() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "v3_field_indexed_getattr_store_rhs_hits_apply_mode_fast_path",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let soac_work_dir = fresh_test_work_dir("v3-field-getattr-store-rhs");
            let module_cache_root = soac_work_dir.join("modules");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let _opt_mode = set_opt_mode("apply");
            let owner_module = PyModule::from_code(
                py,
                c"
class Point:
    pass
",
                c"field_type_test.py",
                c"field_type_test",
            )
            .expect("owner module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_type_test", owner_module.as_any())
                .expect("owner module should be registered");

            let module_name = "counter_test";
            let mut lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def read_point(point):
    value = point.x
    return value
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            instrument_module_with_legacy_call_target_counters(&mut lowered);
            let function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "read_point")
                .expect("missing lowered function read_point")
                .clone();
            let getattr_instr_id = first_getattr_instr_id(&function);
            let (hit_counter_id, hit_branch_id) = runtime_branch_counter_for(
                &lowered.counter_defs,
                function.function_id,
                getattr_instr_id,
                "field_access",
                "indexed_hit",
            );
            let (fallback_counter_id, fallback_branch_id) = runtime_branch_counter_for(
                &lowered.counter_defs,
                function.function_id,
                getattr_instr_id,
                "field_access",
                "indexed_fallback",
            );

            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, module_name, "")
                    .expect("shared state should build");
            let current_function = shared_state
                .lookup_function(function.function_id)
                .expect("read_point should be present in shared state");
            let cache_identity = pre_optimization_module_cache_identity(
                env!("SOAC_BUILD_IDENTITY"),
                shared_state.module_name == "soac.runtime",
            );
            let mut artifacts = test_empty_v3_artifacts_for_function(
                module_name,
                shared_state.source_hash,
                cache_identity.as_str(),
                0,
                current_function,
            );
            let owner_type = IndexedFieldOwnerType {
                module_name: "field_type_test".to_string(),
                qualname: "Point".to_string(),
            };
            let field_guard = IndexedFieldGuardPlan {
                kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
            };
            let field_fallback = IndexedFieldFallbackPlan {
                kind: IndexedFieldFallbackKind::OriginalAttrAccess,
            };
            artifacts.plan.functions[0]
                .indexed_fields
                .push(IndexedFieldSpecializationPlan {
                    source: getattr_instr_id,
                    access: IndexedFieldAccessKind::Load,
                    owner_type: owner_type.clone(),
                    attr_name: "x".to_string(),
                    expected_index: 0,
                    guard: field_guard.clone(),
                    fallback: field_fallback.clone(),
                    reason: "profiled type_keys selected this indexed-field layout".to_string(),
                });
            artifacts.emission.functions[0].indexed_fields.push(
                soac_opt::emit_v3::MechanicalIndexedFieldEmission {
                    source: getattr_instr_id,
                    access: IndexedFieldAccessKind::Load,
                    guard: MechanicalIndexedFieldGuard {
                        kind: field_guard.kind,
                        owner_type,
                        attr_name: "x".to_string(),
                        expected_index: 0,
                    },
                    fallback: field_fallback,
                    reason: "profiled type_keys selected this indexed-field layout".to_string(),
                },
            );
            write_test_optimization_artifacts_v3_for_shared_state(
                module_cache_root.as_path(),
                PythonModuleCacheSource::Project,
                shared_state.as_ref(),
                &artifacts,
            );

            let runtime = unsafe { build_test_module_runtime(py, shared_state.clone()) };
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let compile_session = crate::session::CompileSession::process();
            let compiled_handle = unsafe {
                compile_cranelift_run_bb_specialized_cached(
                    &compile_session,
                    &blocks,
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(shared_state.as_ref()),
                )
            }
            .expect("specialized read_point should compile");
            let (code_ptr, _default_code_ptr, param_count) = compiled_handle
                .handle
                .direct_runner_info()
                .expect("compiled direct runner should expose entrypoint");
            assert_eq!(param_count, 1, "read_point should take one direct arg");
            let entry: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
                unsafe { std::mem::transmute(code_ptr) };

            let point_type = owner_module
                .getattr("Point")
                .expect("Point should exist on owner module");
            let point = unsafe { ffi::PyObject_CallNoArgs(point_type.as_ptr()) };
            assert!(!point.is_null(), "Point() should create a test instance");
            let point_obj = unsafe { pyo3::Bound::from_borrowed_ptr(py, point) };
            point_obj
                .setattr("x", 123_456_i64)
                .expect("Point instance should accept x");

            let mut function_context = test_function_jit_context(&runtime, std::ptr::null_mut());
            let thread_state = unsafe { ffi::PyThreadState_Get() }.cast::<c_void>();
            let result = unsafe {
                entry(
                    std::ptr::addr_of_mut!(function_context).cast(),
                    thread_state,
                    point.cast(),
                )
            };
            assert!(
                !result.is_null(),
                "read_point should return the stored value"
            );

            assert_eq!(
                shared_state.counter_branch_value(hit_counter_id, hit_branch_id),
                1,
                "v3 indexed GetAttr used as a store RHS should take the fast path"
            );
            assert_eq!(
                shared_state.counter_branch_value(fallback_counter_id, fallback_branch_id),
                0,
                "v3 indexed GetAttr used as a store RHS should avoid generic getattr"
            );

            let result_obj = unsafe { pyo3::Bound::from_owned_ptr(py, result.cast()) };
            assert_eq!(
                result_obj
                    .extract::<i64>()
                    .expect("read_point result should be an int"),
                123_456
            );

            unsafe { ffi::Py_DECREF(point) };
            modules
                .del_item("field_type_test")
                .expect("owner module should be removed");
        });
    }

    fn build_field_indexed_specialization_for_source(
        py: Python<'_>,
        mode: &str,
        source: &str,
        function_bind_name: &str,
    ) -> BuiltSpecializedFunction {
        let _opt_mode = set_opt_mode(mode);
        let soac_work_dir = fresh_test_work_dir("field-getattr-deopt");
        let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
        let owner_module = PyModule::from_code(
            py,
            c"
class Point:
    pass
",
            c"field_type_test.py",
            c"field_type_test",
        )
        .expect("owner module should execute");
        let sys = PyModule::import(py, "sys").expect("sys should import");
        let modules = sys
            .getattr("modules")
            .expect("sys.modules should exist")
            .cast_into::<pyo3::types::PyDict>()
            .expect("sys.modules should be a dict");
        modules
            .set_item("field_type_test", owner_module.as_any())
            .expect("owner module should be registered");

        write_test_counter_dump(
            soac_work_dir.join("profile.bin").as_path(),
            &CounterDumpRecord {
                source_hash: 0,
                module_name: "counter_test".to_string(),
                package_name: None,
                rows: Vec::new(),
                module_keys: Vec::new(),
                type_keys: vec![CounterDumpTypeKeyLayout {
                    owner_type_id: 7,
                    key: "x".to_string(),
                    index: 0,
                }],
                type_table: vec![CounterDumpTypeTableEntry {
                    type_id: 7,
                    key: CounterDumpTypeKey {
                        module_name: "field_type_test".to_string(),
                        qualname: "Point".to_string(),
                    },
                }],
            },
        );

        let mut lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("lowering should succeed")
            .codegen_module;
        instrument_module_with_legacy_call_target_counters(&mut lowered);
        let shared_state =
            crate::module_type::build_shared_state_for_testing(py, lowered, "counter_test", "")
                .expect("shared state should build");
        let function = shared_state
            .lowered_module
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == function_bind_name)
            .unwrap_or_else(|| panic!("missing shared-state function {function_bind_name}"))
            .clone();
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let module_constant_ptrs = shared_state.module_constant_ptrs();
        let module_constant_object_data_ids = declare_module_constant_object_data(
            &mut jit_module,
            &shared_state.lowered_module,
            &module_constant_ptrs,
        )
        .expect("module constant object data should declare");
        let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
            define_test_counter_storage(
                &mut jit_module,
                &shared_state.lowered_module,
                shared_state.lowered_module.counter_defs.as_slice(),
            );
        let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
        let built = build_test_cranelift_run_bb_specialized_function(
            &mut jit_module,
            blocks.as_slice(),
            &shared_state.lowered_module,
            &function,
            &shared_state.codegen_constants,
            shared_state.lowered_module.counter_defs.as_slice(),
            module_constant_object_data_ids.as_slice(),
            counter_slots_by_id.as_ref(),
            scalar_counter_data_id,
            top_value_counter_data_id,
            &compile_session,
            Some(shared_state.as_ref()),
            None,
            None,
            BuildSpecializedFunctionOptions::default(),
        )
        .expect("specialized JIT build should succeed");
        modules
            .del_item("field_type_test")
            .expect("owner module should be removed");
        built
    }

    #[test]
    fn field_indexed_getattr_guard_miss_deopts_when_operands_are_replay_safe() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_indexed_getattr_guard_miss_deopts_when_operands_are_replay_safe",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_field_indexed_specialization_for_source(
                py,
                "verify",
                r#"
def read_point(point):
    return point.x
"#,
                "read_point",
            );
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let getattr_helpers =
                import_user_names_for_symbols(&built, &["dp_jit_pyobject_getattr"]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "replay-safe indexed GetAttr guard miss should call the deopt resume helper"
            );
            assert_eq!(
                count_cold_block_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "replay-safe indexed GetAttr deopt helper call should be cold"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &getattr_helpers),
                0,
                "replay-safe indexed GetAttr should not emit a local getattr fallback"
            );
        });
    }

    #[test]
    fn field_indexed_getattr_guard_miss_keeps_fallback_when_receiver_replay_is_unsafe() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_indexed_getattr_guard_miss_keeps_fallback_when_receiver_replay_is_unsafe",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_field_indexed_specialization_for_source(
                py,
                "verify",
                r#"
def read_point(factory):
    return factory().x
"#,
                "read_point",
            );
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let getattr_helpers =
                import_user_names_for_symbols(&built, &["dp_jit_pyobject_getattr"]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                0,
                "receiver calls are not replay-safe, so the guard miss should not deopt"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &getattr_helpers),
                1,
                "unsafe-to-replay indexed GetAttr should keep the local getattr fallback"
            );
        });
    }

    #[test]
    fn field_indexed_setattr_guard_miss_deopts_when_operands_are_replay_safe() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_indexed_setattr_guard_miss_deopts_when_operands_are_replay_safe",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_field_indexed_specialization_for_source(
                py,
                "verify",
                r#"
def write_point(point, value):
    point.x = value
    return value
"#,
                "write_point",
            );
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let setattr_helpers =
                import_user_names_for_symbols(&built, &[DP_JIT_PYOBJECT_SETATTR_IMPORT.symbol]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "replay-safe indexed SetAttr guard miss should call the deopt resume helper"
            );
            assert_eq!(
                count_cold_block_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "replay-safe indexed SetAttr deopt helper call should be cold"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &setattr_helpers),
                0,
                "replay-safe indexed SetAttr should not emit a local setattr fallback"
            );
        });
    }

    #[test]
    fn field_indexed_setattr_guard_miss_deopts_when_receiver_call_is_presequenced() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_indexed_setattr_guard_miss_deopts_when_receiver_call_is_presequenced",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_field_indexed_specialization_for_source(
                py,
                "verify",
                r#"
def write_point(factory, value):
    factory().x = value
    return value
"#,
                "write_point",
            );
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let setattr_helpers =
                import_user_names_for_symbols(&built, &[DP_JIT_PYOBJECT_SETATTR_IMPORT.symbol]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "lowering should sequence factory() before SetAttr, so the guard miss can deopt without replaying it"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &setattr_helpers),
                0,
                "presequenced indexed SetAttr should not keep the local setattr fallback"
            );
        });
    }

    #[test]
    fn field_indexed_getattr_guard_miss_deopt_resumes_generic_getattr_runtime() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_indexed_getattr_guard_miss_deopt_resumes_generic_getattr_runtime",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            #[repr(C)]
            struct TestFunctionEnv {
                direct_code_ptr: *const u8,
                default_direct_code_ptr: *const u8,
                deopt_table_ptr: ObjPtr,
                globals_obj: ObjPtr,
            }

            let _opt_mode = set_opt_mode("verify");
            let soac_work_dir = fresh_test_work_dir("field-getattr-deopt-runtime");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let owner_module = PyModule::from_code(
                py,
                c"
class Point:
    pass

class Other:
    pass
",
                c"field_type_test.py",
                c"field_type_test",
            )
            .expect("owner module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_type_test", owner_module.as_any())
                .expect("owner module should be registered");

            write_test_counter_dump(
                soac_work_dir.join("profile.bin").as_path(),
                &CounterDumpRecord {
                    source_hash: 0,
                    module_name: "counter_test".to_string(),
                    package_name: None,
                    rows: Vec::new(),
                    module_keys: Vec::new(),
                    type_keys: vec![CounterDumpTypeKeyLayout {
                        owner_type_id: 7,
                        key: "x".to_string(),
                        index: 0,
                    }],
                    type_table: vec![CounterDumpTypeTableEntry {
                        type_id: 7,
                        key: CounterDumpTypeKey {
                            module_name: "field_type_test".to_string(),
                            qualname: "Point".to_string(),
                        },
                    }],
                },
            );

            let mut lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def read_point(point):
    return point.x
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            instrument_module_with_legacy_call_target_counters(&mut lowered);
            let function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "read_point")
                .expect("missing lowered function read_point")
                .clone();
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "counter_test", "")
                    .expect("shared state should build");
            let runtime = build_test_module_runtime(py, shared_state.clone());
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let compile_session = runtime.compile_session.as_ref();
            let mut jit_module =
                new_jit_module(compile_session).expect("test jit module should construct");
            let module_constant_object_data_ids = declare_module_constant_object_data(
                &mut jit_module,
                &shared_state.lowered_module,
                &module_constant_ptrs,
            )
            .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    shared_state.lowered_module.counter_defs.as_slice(),
                );
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks.as_slice(),
                &shared_state.lowered_module,
                &function,
                &shared_state.codegen_constants,
                shared_state.lowered_module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                compile_session,
                Some(shared_state.as_ref()),
                None,
                None,
                BuildSpecializedFunctionOptions::default(),
            )
            .expect("specialized JIT build should succeed");
            let facts = infer_jit_value_facts(&shared_state.lowered_module);
            let module_plan =
                plan_jit_module_from_codegen(&shared_state.lowered_module, facts.clone())
                    .map(|prepared| prepared.deopt_resume)
                    .expect("JIT deopt resume planning should succeed");
            let function_plan = module_plan
                .function(function.function_id)
                .expect("read_point should have a JIT deopt plan");
            let deopt_table =
                RuntimeJitDeoptTable::from_plan(&function, function_plan, &module_constant_ptrs)
                    .expect("runtime deopt table should build from plan");

            let mut ctx = built.ctx;
            define_prepared_function(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.main_id,
                &mut ctx,
                "test-field-getattr-deopt-runtime-read-point",
                "field getattr deopt runtime test should define",
            )
            .expect("test function should define");
            jit_module.clear_context(&mut ctx);
            jit_module
                .finalize_definitions()
                .expect("test jit module should finalize");
            let code_ptr = jit_module.get_finalized_function(built.main_id);

            let other_type = owner_module
                .getattr("Other")
                .expect("Other should exist on owner module");
            let other = ffi::PyObject_CallNoArgs(other_type.as_ptr());
            assert!(!other.is_null(), "Other() should create a test instance");
            let other_obj = pyo3::Bound::from_borrowed_ptr(py, other);
            other_obj
                .setattr("x", 112_233_i64)
                .expect("Other instance should accept x");

            let function_env = TestFunctionEnv {
                direct_code_ptr: code_ptr,
                default_direct_code_ptr: std::ptr::null(),
                deopt_table_ptr: std::ptr::addr_of!(deopt_table).cast_mut().cast(),
                globals_obj: runtime.mod_ctx.globals_obj,
            };
            let entry: unsafe extern "C" fn(ObjPtr, ObjPtr, ObjPtr) -> ObjPtr =
                std::mem::transmute(code_ptr);
            let result = entry(
                std::ptr::addr_of!(function_env).cast_mut().cast(),
                ffi::PyThreadState_Get().cast(),
                other.cast(),
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful field guard-miss deopt should not leave a Python exception"
            );
            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                112_233,
                "field guard-miss deopt should resume before generic point.x and return its result"
            );

            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(other);
            modules
                .del_item("field_type_test")
                .expect("owner module should be removed");
        });
    }

    #[test]
    fn field_indexed_setattr_guard_miss_deopt_resumes_generic_setattr_runtime() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_indexed_setattr_guard_miss_deopt_resumes_generic_setattr_runtime",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            #[repr(C)]
            struct TestFunctionEnv {
                direct_code_ptr: *const u8,
                default_direct_code_ptr: *const u8,
                deopt_table_ptr: ObjPtr,
                globals_obj: ObjPtr,
            }

            let _opt_mode = set_opt_mode("verify");
            let soac_work_dir = fresh_test_work_dir("field-setattr-deopt-runtime");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let owner_module = PyModule::from_code(
                py,
                c"
class Point:
    pass

class Other:
    pass
",
                c"field_type_test.py",
                c"field_type_test",
            )
            .expect("owner module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_type_test", owner_module.as_any())
                .expect("owner module should be registered");

            write_test_counter_dump(
                soac_work_dir.join("profile.bin").as_path(),
                &CounterDumpRecord {
                    source_hash: 0,
                    module_name: "counter_test".to_string(),
                    package_name: None,
                    rows: Vec::new(),
                    module_keys: Vec::new(),
                    type_keys: vec![CounterDumpTypeKeyLayout {
                        owner_type_id: 7,
                        key: "x".to_string(),
                        index: 0,
                    }],
                    type_table: vec![CounterDumpTypeTableEntry {
                        type_id: 7,
                        key: CounterDumpTypeKey {
                            module_name: "field_type_test".to_string(),
                            qualname: "Point".to_string(),
                        },
                    }],
                },
            );

            let mut lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def write_point(point, value):
    point.x = value
    return value
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            instrument_module_with_legacy_call_target_counters(&mut lowered);
            let function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "write_point")
                .expect("missing lowered function write_point")
                .clone();
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, "counter_test", "")
                    .expect("shared state should build");
            let runtime = build_test_module_runtime(py, shared_state.clone());
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let compile_session = runtime.compile_session.as_ref();
            let mut jit_module =
                new_jit_module(compile_session).expect("test jit module should construct");
            let module_constant_object_data_ids = declare_module_constant_object_data(
                &mut jit_module,
                &shared_state.lowered_module,
                &module_constant_ptrs,
            )
            .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    shared_state.lowered_module.counter_defs.as_slice(),
                );
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks.as_slice(),
                &shared_state.lowered_module,
                &function,
                &shared_state.codegen_constants,
                shared_state.lowered_module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                compile_session,
                Some(shared_state.as_ref()),
                None,
                None,
                BuildSpecializedFunctionOptions::default(),
            )
            .expect("specialized JIT build should succeed");
            let facts = infer_jit_value_facts(&shared_state.lowered_module);
            let module_plan =
                plan_jit_module_from_codegen(&shared_state.lowered_module, facts.clone())
                    .map(|prepared| prepared.deopt_resume)
                    .expect("JIT deopt resume planning should succeed");
            let function_plan = module_plan
                .function(function.function_id)
                .expect("write_point should have a JIT deopt plan");
            let deopt_table =
                RuntimeJitDeoptTable::from_plan(&function, function_plan, &module_constant_ptrs)
                    .expect("runtime deopt table should build from plan");

            let mut ctx = built.ctx;
            define_prepared_function(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.main_id,
                &mut ctx,
                "test-field-setattr-deopt-runtime-write-point",
                "field setattr deopt runtime test should define",
            )
            .expect("test function should define");
            jit_module.clear_context(&mut ctx);
            jit_module
                .finalize_definitions()
                .expect("test jit module should finalize");
            let code_ptr = jit_module.get_finalized_function(built.main_id);

            let other_type = owner_module
                .getattr("Other")
                .expect("Other should exist on owner module");
            let other = ffi::PyObject_CallNoArgs(other_type.as_ptr());
            assert!(!other.is_null(), "Other() should create a test instance");
            let replacement = ffi::PyLong_FromLong(445_566);
            assert!(!replacement.is_null(), "replacement value should allocate");

            let function_env = TestFunctionEnv {
                direct_code_ptr: code_ptr,
                default_direct_code_ptr: std::ptr::null(),
                deopt_table_ptr: std::ptr::addr_of!(deopt_table).cast_mut().cast(),
                globals_obj: runtime.mod_ctx.globals_obj,
            };
            let entry: unsafe extern "C" fn(ObjPtr, ObjPtr, ObjPtr, ObjPtr) -> ObjPtr =
                std::mem::transmute(code_ptr);
            let result = entry(
                std::ptr::addr_of!(function_env).cast_mut().cast(),
                ffi::PyThreadState_Get().cast(),
                other.cast(),
                replacement.cast(),
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful field SetAttr guard-miss deopt should not leave a Python exception"
            );
            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                445_566,
                "field SetAttr guard-miss deopt should resume and return the local value"
            );
            let other_obj = pyo3::Bound::from_borrowed_ptr(py, other);
            assert_eq!(
                other_obj
                    .getattr("x")
                    .expect("generic SetAttr deopt should write x")
                    .extract::<i64>()
                    .expect("x should be an int"),
                445_566
            );

            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(replacement);
            ffi::Py_DECREF(other);
            modules
                .del_item("field_type_test")
                .expect("owner module should be removed");
        });
    }

    fn render_test_jit_function_with_constants(
        module: &BlockPyModule<CodegenModuleShape>,
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
        module_constants: &crate::module_constants::ModuleCodegenConstants,
    ) -> String {
        unsafe {
            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = placeholder_module_constant_ptrs(module_constants.len());
            let module_constant_object_data_ids =
                declare_module_constant_object_data(&mut jit_module, module, &module_constant_ptrs)
                    .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    module,
                    module.counter_defs.as_slice(),
                );
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks,
                module,
                function,
                module_constants,
                module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                &compile_session,
                None,
                None,
                None,
                BuildSpecializedFunctionOptions::default(),
            )
            .expect("specialized JIT build should succeed");
            let (clif, _cfg_dot, _vcode_disasm) = render_compiled_clif_and_vcode_disasm(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.ctx,
                &built.import_id_to_symbol,
                &built.block_annotations,
            )
            .expect("specialized JIT CLIF render should succeed");
            clif
        }
    }

    fn define_test_counter_storage(
        jit_module: &mut JITModule,
        module: &BlockPyModule<CodegenModuleShape>,
        counter_defs: &[CounterDef],
    ) -> (Box<[CounterRuntimeSlot]>, Option<DataId>, Option<DataId>) {
        let (counter_slots_by_id, scalar_counter_count, top_value_counter_count) =
            build_counter_storage_layout(counter_defs)
                .expect("counter storage layout should build");
        let scalar_counter_data_id = if scalar_counter_count == 0 {
            None
        } else {
            Some(
                define_scalar_counter_storage_data(jit_module, module, scalar_counter_count)
                    .expect("scalar counter storage data should define"),
            )
        };
        let top_value_counter_data_id = if top_value_counter_count == 0 {
            None
        } else {
            Some(
                define_top_value_counter_storage_data(jit_module, module, top_value_counter_count)
                    .expect("top-value counter storage data should define"),
            )
        };
        (
            counter_slots_by_id,
            scalar_counter_data_id,
            top_value_counter_data_id,
        )
    }

    fn declare_shared_state_top_value_counter_storage(
        jit_module: &mut JITModule,
        shared_state: &crate::module_type::SharedModuleState,
    ) -> Option<DataId> {
        let top_value_counter_base_ptr = shared_state.top_value_counter_values_ptr();
        if top_value_counter_base_ptr.is_null() {
            None
        } else {
            Some(
                declare_top_value_counter_storage_import(
                    jit_module,
                    top_value_counter_storage_symbol_for_instance(
                        &shared_state.lowered_module,
                        shared_state.storage_instance_key(),
                    )
                    .as_str(),
                )
                .expect("top-value counter storage import should declare"),
            )
        }
    }

    fn build_test_jit_function_with_constants(
        module: &BlockPyModule<CodegenModuleShape>,
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
        module_constants: &crate::module_constants::ModuleCodegenConstants,
    ) -> BuiltSpecializedFunction {
        build_test_jit_function_with_constants_and_options(
            module,
            function,
            blocks,
            module_constants,
            BuildSpecializedFunctionOptions::default(),
        )
    }

    fn build_test_jit_function_with_constants_and_options(
        module: &BlockPyModule<CodegenModuleShape>,
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
        module_constants: &crate::module_constants::ModuleCodegenConstants,
        options: BuildSpecializedFunctionOptions,
    ) -> BuiltSpecializedFunction {
        unsafe {
            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = placeholder_module_constant_ptrs(module_constants.len());
            let module_constant_object_data_ids =
                declare_module_constant_object_data(&mut jit_module, module, &module_constant_ptrs)
                    .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    module,
                    module.counter_defs.as_slice(),
                );
            build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks,
                module,
                function,
                module_constants,
                module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                &compile_session,
                None,
                None,
                None,
                options,
            )
            .expect("specialized JIT build should succeed")
        }
    }

    fn render_test_jit_function_with_constants_and_runtime_inline(
        module: &BlockPyModule<CodegenModuleShape>,
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
        module_constants: &crate::module_constants::ModuleCodegenConstants,
    ) -> String {
        unsafe {
            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = placeholder_module_constant_ptrs(module_constants.len());
            let module_constant_object_data_ids =
                declare_module_constant_object_data(&mut jit_module, module, &module_constant_ptrs)
                    .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    module,
                    module.counter_defs.as_slice(),
                );
            let mut built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks,
                module,
                function,
                module_constants,
                module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                &compile_session,
                None,
                None,
                None,
                BuildSpecializedFunctionOptions::default(),
            )
            .expect("specialized JIT build should succeed");
            inline_runtime_support_calls(
                &mut jit_module,
                &SoacEnvConfig::default(),
                &mut built.ctx,
                "test",
            )
            .expect("runtime support helpers should inline");
            let (clif, _cfg_dot, _vcode_disasm) = render_compiled_clif_and_vcode_disasm(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.ctx,
                &built.import_id_to_symbol,
                &built.block_annotations,
            )
            .expect("specialized JIT CLIF render should succeed");
            clif
        }
    }

    fn render_test_jit_function_with_runtime_inline(
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
        module_constants: Vec<InstrResolved>,
    ) -> String {
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = module_constants;
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        render_test_jit_function_with_constants_and_runtime_inline(
            &module,
            &function,
            blocks,
            &module_constants,
        )
    }

    fn assert_inlined_indexed_global_guard(rendered: &str, message: &str) {
        let globals_base = i64_load_bases_for_offset(rendered, 40);
        let indexed_names_base = i64_load_bases_for_offset(rendered, 32);
        let loads_indexed_globals_and_names = globals_base
            .iter()
            .any(|base| indexed_names_base.iter().any(|other| other == base));
        assert!(
            loads_indexed_globals_and_names
                && rendered.contains("load.i8 notrap")
                && rendered.contains("iconst.i64 -1"),
            "{message}:\n{rendered}"
        );
    }

    fn i64_load_bases_for_offset(rendered: &str, offset: usize) -> Vec<&str> {
        let offset_suffix = format!("+{offset}");
        rendered
            .lines()
            .filter_map(|line| {
                let load_arg = line.split_once("load.i64 notrap ")?.1;
                let load_arg = load_arg.strip_prefix("aligned ").unwrap_or(load_arg);
                let (base, _) = load_arg.split_once(offset_suffix.as_str())?;
                Some(base.trim())
            })
            .collect()
    }

    unsafe fn build_test_module_runtime(
        _py: Python<'_>,
        shared_state: std::sync::Arc<crate::module_type::SharedModuleState>,
    ) -> crate::jit::ModuleRuntimeContext {
        ensure_test_optimization_artifacts_v3_for_shared_state(shared_state.as_ref())
            .expect("test optimization plan v3 should generate for specialized mode");
        let globals_obj = ffi::PyDict_New().cast::<c_void>();
        assert!(
            !globals_obj.is_null(),
            "PyDict_New should produce globals for test runtime"
        );
        crate::jit::ModuleRuntimeContext {
            mod_ctx: crate::jit::ModuleJitContext {
                shared_module_state: std::sync::Arc::as_ptr(&shared_state),
                globals_obj,
            },
            compile_session: crate::session::CompileSession::process(),
            shared_module_state_owner: shared_state,
        }
    }

    fn test_function_jit_context(
        runtime: &crate::jit::ModuleRuntimeContext,
        runtime_objects: *mut c_void,
    ) -> [*mut c_void; 2] {
        [
            std::ptr::addr_of!(runtime.mod_ctx).cast_mut().cast(),
            runtime_objects,
        ]
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

    fn direct_call_args_to_runtime_helpers(
        function: &ir::Function,
        helpers: &[ir::UserExternalName],
    ) -> Vec<Vec<ir::Value>> {
        let mut args = Vec::new();
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
                    args.push(function.dfg.inst_args(inst).to_vec());
                }
            }
        }
        args
    }

    fn value_is_iconst_imm(function: &ir::Function, value: ir::Value, expected_imm: i64) -> bool {
        let ir::ValueDef::Result(inst, _) = function.dfg.value_def(value) else {
            return false;
        };
        matches!(
            function.dfg.insts[inst],
            ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm }
                if imm.bits() == expected_imm
        )
    }

    fn count_cold_block_direct_calls_to_runtime_helpers(
        function: &ir::Function,
        helpers: &[ir::UserExternalName],
    ) -> usize {
        let mut count = 0usize;
        for block in function.layout.blocks() {
            if !function.layout.is_cold(block) {
                continue;
            }
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

    fn count_deopt_helper_success_returns(
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
                if !helpers.contains(user_name) {
                    continue;
                }
                let [deopt_result] = function.dfg.inst_results(inst) else {
                    panic!("deopt helper call should have one result");
                };
                let Some(term) = function.layout.last_inst(block) else {
                    continue;
                };
                for destination in function.dfg.insts[term]
                    .branch_destination(&function.dfg.jump_tables, &function.dfg.exception_tables)
                {
                    let args = destination
                        .args(&function.dfg.value_lists)
                        .collect::<Vec<_>>();
                    if !args.iter().any(
                        |arg| matches!(arg, ir::BlockArg::Value(value) if value == deopt_result),
                    ) {
                        continue;
                    }
                    let target = destination.block(&function.dfg.value_lists);
                    let Some(target_term) = function.layout.last_inst(target) else {
                        continue;
                    };
                    if function.dfg.insts[target_term].opcode() != ir::Opcode::Return {
                        continue;
                    }
                    let return_args = function.dfg.inst_args(target_term);
                    let target_params = function.dfg.block_params(target);
                    if return_args.len() == 1
                        && target_params.len() == 1
                        && return_args[0] == target_params[0]
                    {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    fn assert_guard_miss_deopts_without_local_fallback(
        built: &BuiltSpecializedFunction,
        fallback_symbols: &[&'static str],
        case_name: &str,
    ) {
        let deopt_helpers = import_user_names_for_symbols(built, &["dp_jit_deopt_resume"]);
        let fallback_helpers = import_user_names_for_symbols(built, fallback_symbols);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
            1,
            "{case_name}: replay-safe guard miss should call the deopt resume helper"
        );
        assert_eq!(
            count_cold_block_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
            1,
            "{case_name}: deopt helper call should be cold"
        );
        assert_eq!(
            count_deopt_helper_success_returns(&built.ctx.func, &deopt_helpers),
            1,
            "{case_name}: deopt should return a successful continuation result"
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &fallback_helpers),
            0,
            "{case_name}: replay-safe guard miss should not emit the local fallback helper"
        );
    }

    fn direct_call_colocated_flags_to_runtime_helpers(
        function: &ir::Function,
        helpers: &[ir::UserExternalName],
    ) -> Vec<bool> {
        let mut colocated = Vec::new();
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
                    colocated.push(ext_func.colocated);
                }
            }
        }
        colocated
    }

    fn parsed_runtime_clif_function(symbol: &str) -> ParsedRuntimeClifFunction {
        parse_runtime_clif_functions()
            .expect("runtime CLIF should parse")
            .into_iter()
            .find(|function| function.symbol == symbol)
            .unwrap_or_else(|| panic!("missing parsed runtime CLIF function for {symbol}"))
    }

    fn single_direct_call_callee_name(function: &ir::Function) -> ir::UserExternalName {
        let mut found = None;
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
                let user_name = function.params.user_named_funcs()[*name_ref].clone();
                match &found {
                    None => found = Some(user_name),
                    Some(previous) if *previous == user_name => {}
                    Some(previous) => {
                        panic!("expected one direct call callee, found {previous} and {user_name}")
                    }
                }
            }
        }
        found.expect("expected the example function to contain one direct call")
    }

    fn specialize_runtime_i64_call_to_constant(
        function: &ir::Function,
        callee_name: &ir::UserExternalName,
        known_value: i64,
    ) -> ir::Function {
        let mut specialized = function.clone();
        let mut cursor = cranelift_codegen::cursor::FuncCursor::new(&mut specialized);
        let mut replaced_calls = 0usize;

        while let Some(_block) = cursor.next_block() {
            while let Some(inst) = cursor.next_inst() {
                let func_ref = match cursor.func.dfg.insts[inst] {
                    ir::InstructionData::Call {
                        opcode: ir::Opcode::Call,
                        func_ref,
                        ..
                    } => func_ref,
                    _ => continue,
                };
                let ext_func = &cursor.func.dfg.ext_funcs[func_ref];
                let ir::ExternalName::User(name_ref) = ext_func.name else {
                    continue;
                };
                if &cursor.func.params.user_named_funcs()[name_ref] != callee_name {
                    continue;
                }
                let [result] = cursor.func.dfg.inst_results(inst) else {
                    panic!("example specialization expects a single-result call");
                };
                let result_ty = cursor.func.dfg.value_type(*result);
                assert_eq!(
                    result_ty,
                    ir::types::I64,
                    "example specialization expects an i64 call result"
                );
                cursor.func.dfg.replace(inst).iconst(result_ty, known_value);
                replaced_calls += 1;
            }
        }

        assert_eq!(
            replaced_calls, 1,
            "example specialization should replace exactly one helper call"
        );
        specialized
    }

    fn optimize_test_ir_function(function: ir::Function) -> ir::Function {
        let mut flag_builder = cranelift_codegen::settings::builder();
        flag_builder
            .set("opt_level", "speed")
            .expect("test ISA should accept opt_level");
        let isa_builder = cranelift_native::builder().expect("test ISA builder should construct");
        let isa = isa_builder
            .finish(cranelift_codegen::settings::Flags::new(flag_builder))
            .expect("test ISA should finish");
        let mut ctx = cranelift_codegen::Context::for_function(function);
        let mut ctrl_plane = cranelift_control::ControlPlane::default();
        ctx.optimize(isa.as_ref(), &mut ctrl_plane)
            .expect("test IR function should optimize");
        ctx.func
    }

    fn function_contains_iconst_imm(function: &ir::Function, expected_imm: i64) -> bool {
        function.layout.blocks().any(|block| {
            function.layout.block_insts(block).any(|inst| {
                matches!(
                    function.dfg.insts[inst],
                    ir::InstructionData::UnaryImm { opcode: ir::Opcode::Iconst, imm }
                        if imm.bits() == expected_imm
                )
            })
        })
    }

    fn count_opcode(function: &ir::Function, opcode: ir::Opcode) -> usize {
        function
            .layout
            .blocks()
            .map(|block| {
                function
                    .layout
                    .block_insts(block)
                    .filter(|inst| function.dfg.insts[*inst].opcode() == opcode)
                    .count()
            })
            .sum()
    }

    fn count_symbolic_global_values(function: &ir::Function) -> usize {
        function
            .global_values
            .values()
            .filter(|global_value| matches!(global_value, ir::GlobalValueData::Symbol { .. }))
            .count()
    }

    fn count_indirect_calls(function: &ir::Function) -> usize {
        function
            .layout
            .blocks()
            .map(|block| {
                function
                    .layout
                    .block_insts(block)
                    .filter(|inst| {
                        matches!(
                            function.dfg.insts[*inst],
                            ir::InstructionData::CallIndirect { .. }
                                | ir::InstructionData::TryCallIndirect { .. }
                        )
                    })
                    .count()
            })
            .sum()
    }

    fn import_user_names_for_symbols(
        built: &BuiltSpecializedFunction,
        symbols: &[&'static str],
    ) -> Vec<ir::UserExternalName> {
        built
            .import_id_to_symbol
            .iter()
            .filter_map(|(import_id, symbol)| {
                symbols
                    .iter()
                    .any(|wanted| wanted == symbol)
                    .then(|| ir::UserExternalName::new(0, *import_id))
            })
            .collect()
    }

    fn declared_user_names_for_symbols(
        built: &BuiltSpecializedFunction,
        symbols: &[&'static str],
    ) -> Vec<ir::UserExternalName> {
        built
            .func_id_to_symbol
            .iter()
            .filter_map(|(func_id, symbol)| {
                symbols
                    .iter()
                    .any(|wanted| wanted == symbol)
                    .then(|| ir::UserExternalName::new(0, *func_id))
            })
            .collect()
    }

    #[test]
    fn runtime_deopt_invocation_materializes_live_local_snapshot() {
        let function = test_function();
        let function_id = function.function_id;
        let block = BlockLabel::from_index(0);
        let location = LocalLocation(0);
        let binding = LocalEnvResumeBinding {
            name: "x".to_string(),
            location,
            binding: LocalEnvResumeBindingState::Bound,
            source: LocalEnvResumeValueSource::BlockParam(location),
            ownership: LocalRefKind::Owned,
            value: None,
        };
        let table = RuntimeJitDeoptTable {
            function_id,
            function: Box::new(function),
            module_constant_ptrs: Vec::new(),
            points: vec![RuntimeJitDeoptRecord {
                id: PlannedJitDeoptPointId {
                    function_id,
                    ordinal: 0,
                },
                resume_point: LocalEnvResumePoint::BlockEntry { function_id, block },
                precision: LocalEnvResumeStatePrecision::BlockEntry,
                locals: vec![binding],
                continuation: RuntimeJitDeoptContinuation::unsupported(
                    RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
                ),
            }],
        };
        let expected_value = 0x1234usize as ObjPtr;
        let mut live_values = vec![expected_value];
        let invocation = unsafe {
            RuntimeJitDeoptInvocation::from_raw(
                std::ptr::addr_of!(table).cast_mut().cast(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                live_values.as_mut_ptr().cast(),
                live_values.len() as i64,
            )
            .expect("well-formed live value buffer should validate")
        };
        let locals = invocation
            .materialize_locals()
            .expect("validated live bindings should materialize into runtime locals");

        assert_eq!(locals.len(), 1);
        let local = locals
            .get_by_name("x")
            .expect("runtime locals should be addressable by source name");
        assert_eq!(local.binding().location, location);
        assert_eq!(local.value(), expected_value);
        assert_eq!(
            locals
                .get_by_location(location)
                .expect("runtime locals should be addressable by location")
                .value(),
            expected_value
        );
        assert!(
            locals
                .describe()
                .contains(format!("x@{}=", location.slot()).as_str()),
            "runtime locals diagnostics should include local names and slots"
        );
    }

    #[test]
    fn runtime_deopt_table_marks_return_local_before_term_continuation() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    return x
"#,
        )
        .expect("lowering should succeed")
        .codegen_module;
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("lowered function should exist");
        let facts = infer_module_value_facts(&lowered);
        let module_plan = plan_jit_module_from_codegen(&lowered, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: function.entry_block().label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term return point should have a runtime record");
        let _x_binding = record
            .locals()
            .iter()
            .find(|binding| binding.name == "x")
            .expect("return-local deopt state should carry x");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(
                    function.entry_block().label,
                    function.entry_block().body.len(),
                ),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_return_global_before_term_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(name_expr(test_global_name("x"))),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: function.entry_block().label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term return point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(
                    function.entry_block().label,
                    function.entry_block().body.len(),
                ),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_return_binop_before_term_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(BinOp::new(
                BinOpKind::Add,
                name_expr(test_constant_name(0)),
                name_expr(test_constant_name(1)),
            ))),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: function.entry_block().label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term return-binop point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(
                    function.entry_block().label,
                    function.entry_block().body.len(),
                ),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_return_call_direct_before_term_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(InstrCodegen::CallDirect(CallDirect::new(
                name_expr(test_constant_name(0)),
                RuntimeFunctionId::from_raw_parts(0, 999),
                vec![CallArgPositional::Positional(name_expr(
                    test_constant_name(1),
                ))],
                Vec::<CallArgKeyword<InstrCodegen>>::new(),
            ))),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: function.entry_block().label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term return-call-direct point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(
                    function.entry_block().label,
                    function.entry_block().body.len(),
                ),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_return_starred_positional_call_before_term_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Call::new(
                name_expr(test_constant_name(0)),
                vec![CallArgPositional::Starred(name_expr(test_constant_name(1)))],
                Vec::<CallArgKeyword<InstrCodegen>>::new(),
            ))),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: function.entry_block().label,
        };
        let record = table.record_for_point(point).expect(
            "before-term return-starred-positional-call point should have a runtime record",
        );
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(
                    function.entry_block().label,
                    function.entry_block().body.len(),
                ),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_return_starred_keyword_call_before_term_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Call::new(
                name_expr(test_constant_name(0)),
                Vec::<CallArgPositional<InstrCodegen>>::new(),
                vec![CallArgKeyword::Starred(name_expr(test_constant_name(1)))],
            ))),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: function.entry_block().label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term return-starred-keyword-call point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(
                    function.entry_block().label,
                    function.entry_block().body.len(),
                ),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_raise_before_term_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            BlockTerm::Raise(soac_core::block_py::TermRaise {
                exc: Some(name_expr(test_constant_name(0))),
            }),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: function.entry_block().label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term raise point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(
                    function.entry_block().label,
                    function.entry_block().body.len(),
                ),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_body_instr_block_tail_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![expr_stmt(name_expr(test_global_name("x")))],
            ret_term(none_expr()),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let body_instr_id = block.body[0]
            .try_semantic_instr_id()
            .expect("test body instruction should have an id");
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeInstr {
            key: InstrKey::new(function.function_id, body_instr_id),
        };
        let record = table
            .record_for_point(point)
            .expect("before-instr body point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_block_entry_continuation() {
        let function = with_single_test_block(test_function(), vec![], ret_term(none_expr()));
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BlockEntry {
            function_id: function.function_id,
            block: block.label,
        };
        let record = table
            .record_for_point(point)
            .expect("block-entry point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_rejects_unsupported_block_entry_tail() {
        let test_function = test_function();
        let function = with_single_test_block(
            test_function,
            vec![expr_stmt(op_expr(CellRef::new(CellLocation::Owned(0))))],
            ret_term(none_expr()),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let body_instr_id = block.body[0]
            .try_semantic_instr_id()
            .expect("test body instruction should have an id");
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");

        let block_entry_record = table
            .record_for_point(LocalEnvResumePoint::BlockEntry {
                function_id: function.function_id,
                block: block.label,
            })
            .expect("block-entry point should have a runtime record");
        assert_eq!(
            block_entry_record.continuation(),
            &RuntimeJitDeoptContinuation::unsupported(
                RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
            ),
            "block-entry continuation should not claim support for unsupported body tails"
        );

        let before_instr_record = table
            .record_for_point(LocalEnvResumePoint::BeforeInstr {
                key: InstrKey::new(function.function_id, body_instr_id),
            })
            .expect("before-instr point should have a runtime record");
        assert_eq!(
            before_instr_record.continuation(),
            &RuntimeJitDeoptContinuation::unsupported(
                RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
            ),
            "before-instr continuation should not claim support for unsupported body tails"
        );
    }

    #[test]
    fn runtime_deopt_table_accepts_make_function_with_closure_block_tail() {
        let test_function = test_function();
        let function_id = test_function.function_id;
        let empty_tuple_expr = || tuple_expr(Vec::new());
        let function = with_single_test_block(
            test_function,
            vec![expr_stmt(op_expr(
                soac_core::block_py::MakeFunctionWithClosure::new(
                    function_id,
                    soac_core::block_py::FunctionKind::Function,
                    empty_tuple_expr(),
                    empty_tuple_expr(),
                    none_expr(),
                ),
            ))],
            ret_term(none_expr()),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let block_entry_record = table
            .record_for_point(LocalEnvResumePoint::BlockEntry {
                function_id: function.function_id,
                block: block.label,
            })
            .expect("block-entry point should have a runtime record");
        assert_eq!(
            block_entry_record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
            },
            "MakeFunctionWithClosure should be replayable by the deopt interpreter"
        );
    }

    #[test]
    fn runtime_deopt_guard_miss_rejects_replay_unsafe_operands() {
        let function_id = test_function().function_id;
        assert!(
            runtime_jit_deopt_guard_operand_replay_safe(&name_expr(test_name("x"))),
            "plain local loads should be replay-safe guard operands"
        );
        assert!(
            !runtime_jit_deopt_guard_operand_replay_safe(&direct_call_expr(function_id)),
            "guard miss deopt should reject operands that could repeat side effects"
        );
    }

    #[test]
    fn runtime_deopt_table_marks_store_body_tail_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![assign_stmt(test_name("x"), none_expr())],
            ret_term(name_expr(test_name("x"))),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let body_instr_id = block.body[0]
            .try_semantic_instr_id()
            .expect("test body instruction should have an id");
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");

        let block_entry_record = table
            .record_for_point(LocalEnvResumePoint::BlockEntry {
                function_id: function.function_id,
                block: block.label,
            })
            .expect("block-entry point should have a runtime record");
        assert_eq!(
            block_entry_record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
            }
        );

        let before_instr_record = table
            .record_for_point(LocalEnvResumePoint::BeforeInstr {
                key: InstrKey::new(function.function_id, body_instr_id),
            })
            .expect("before-instr point should have a runtime record");
        assert_eq!(
            before_instr_record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_increment_counter_body_tail_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![op_expr(IncrementCounter::new(CounterId(0)))],
            ret_term(none_expr()),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let body_instr_id = block.body[0]
            .try_semantic_instr_id()
            .expect("test body instruction should have an id");
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");

        let block_entry_record = table
            .record_for_point(LocalEnvResumePoint::BlockEntry {
                function_id: function.function_id,
                block: block.label,
            })
            .expect("block-entry point should have a runtime record");
        assert_eq!(
            block_entry_record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
            }
        );

        let before_instr_record = table
            .record_for_point(LocalEnvResumePoint::BeforeInstr {
                key: InstrKey::new(function.function_id, body_instr_id),
            })
            .expect("before-instr point should have a runtime record");
        assert_eq!(
            before_instr_record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_make_cell_return_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(MakeCell::with_initial_value(name_expr(
                test_constant_name(0),
            )))),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: block.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term make-cell point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(block.label, block.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_owned_cell_ref_return_continuation() {
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(soac_core::block_py::CellRef::new(
                CellLocation::Owned(0),
            ))),
        );
        function.storage_layout = Some(StorageLayout {
            freevars: vec![],
            cellvars: vec![ClosureSlot {
                logical_name: "cell".to_string(),
                storage_name: "cell".to_string(),
                init: ClosureInit::Deferred,
            }],
            runtime_cells: vec![],
            stack_slots: vec!["cell".to_string()],
        });
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: block.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term owned-cell-ref point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(block.label, block.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_closure_cell_ref_return_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(soac_core::block_py::CellRef::new(
                CellLocation::Closure(0),
            ))),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: block.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term closure-cell-ref point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(block.label, block.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_captured_source_cell_ref_return_continuation() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(soac_core::block_py::CellRef::new(
                CellLocation::CapturedSource(0),
            ))),
        );
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: block.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term captured-source-cell-ref point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(block.label, block.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_owned_cell_load_return_continuation() {
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(name_expr(test_owned_cell_name("cell", 0))),
        );
        function.storage_layout = Some(StorageLayout {
            freevars: vec![],
            cellvars: vec![ClosureSlot {
                logical_name: "cell".to_string(),
                storage_name: "cell".to_string(),
                init: ClosureInit::Deferred,
            }],
            runtime_cells: vec![],
            stack_slots: vec!["cell".to_string()],
        });
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: block.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term owned-cell-load point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(block.label, block.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_owned_cell_store_delete_body_tail_continuation() {
        let mut function = with_single_test_block(
            test_function(),
            vec![
                op_expr(Store::new(
                    test_owned_cell_name("cell", 0),
                    name_expr(test_constant_name(0)),
                )),
                op_expr(Del::new(test_owned_cell_name("cell", 0), false)),
            ],
            ret_term(none_expr()),
        );
        function.storage_layout = Some(StorageLayout {
            freevars: vec![],
            cellvars: vec![ClosureSlot {
                logical_name: "cell".to_string(),
                storage_name: "cell".to_string(),
                init: ClosureInit::Deferred,
            }],
            runtime_cells: vec![],
            stack_slots: vec!["cell".to_string()],
        });
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let body_instr_id = block.body[0]
            .try_semantic_instr_id()
            .expect("test body instruction should have an id");
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");

        let block_entry_record = table
            .record_for_point(LocalEnvResumePoint::BlockEntry {
                function_id: function.function_id,
                block: block.label,
            })
            .expect("block-entry point should have a runtime record");
        assert_eq!(
            block_entry_record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
            }
        );

        let before_instr_record = table
            .record_for_point(LocalEnvResumePoint::BeforeInstr {
                key: InstrKey::new(function.function_id, body_instr_id),
            })
            .expect("before-instr owned-cell-store point should have a runtime record");
        assert_eq!(
            before_instr_record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(block.label),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_exception_edge_body_tail_continuation() {
        let mut function = test_function();
        set_stack_slots(&mut function, &["exc"]);
        let mut handler = test_source_block(
            &function,
            vec![],
            ret_term(name_expr(test_local_name("exc", 0))),
        );
        handler.params = vec![BlockParam {
            name: "exc".to_string(),
            role: BlockParamRole::Exception,
        }];
        let mut entry = test_source_block(
            &function,
            vec![op_expr(Call::new(
                name_expr(test_constant_name(0)),
                vec![CallArgPositional::Positional(name_expr(
                    test_constant_name(1),
                ))],
                Vec::<CallArgKeyword<InstrCodegen>>::new(),
            ))],
            ret_term(none_expr()),
        );
        entry.exc_edge = Some(BlockEdge::with_args(
            handler.label,
            vec![BlockArg::CurrentException],
        ));
        let module = test_module(
            ModuleNameGen::new(0),
            vec![with_test_blocks(function, vec![entry.clone(), handler])],
        );
        let function = &module.callable_defs[0];
        let body_instr_id = function.blocks[0].body[0]
            .try_semantic_instr_id()
            .expect("test body instruction should have an id");
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");

        let block_entry_record = table
            .record_for_point(LocalEnvResumePoint::BlockEntry {
                function_id: function.function_id,
                block: entry.label,
            })
            .expect("block-entry point should have a runtime record");
        assert_eq!(
            block_entry_record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(entry.label),
            }
        );

        let before_instr_record = table
            .record_for_point(LocalEnvResumePoint::BeforeInstr {
                key: InstrKey::new(function.function_id, body_instr_id),
            })
            .expect("before-instr point should have a runtime record");
        assert_eq!(
            before_instr_record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::at_block_entry(entry.label),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_no_arg_jump_before_term_continuation() {
        let function = test_function();
        let target = test_source_block(&function, vec![], ret_term(none_expr()));
        let entry = test_source_block(
            &function,
            vec![],
            BlockTerm::Jump(BlockEdge::new(target.label)),
        );
        let module = test_module(
            ModuleNameGen::new(0),
            vec![with_test_blocks(function, vec![entry.clone(), target])],
        );
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: entry.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term jump point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_simple_jump_args_before_term_continuation() {
        let mut function = test_function();
        set_stack_slots(&mut function, &["x", "y"]);
        let mut target = test_source_block(
            &function,
            vec![],
            ret_term(name_expr(test_local_name("y", 1))),
        );
        target.params = vec![BlockParam {
            name: "y".to_string(),
            role: BlockParamRole::AbruptPayload,
        }];
        let entry = test_source_block(
            &function,
            vec![],
            BlockTerm::Jump(BlockEdge::with_args(
                target.label,
                vec![BlockArg::Name("x".to_string())],
            )),
        );
        let module = test_module(
            ModuleNameGen::new(0),
            vec![with_test_blocks(function, vec![entry.clone(), target])],
        );
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: entry.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term jump-arg point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_abrupt_kind_jump_args_before_term_continuation() {
        let mut function = test_function();
        set_stack_slots(&mut function, &["kind"]);
        let mut target = test_source_block(
            &function,
            vec![],
            ret_term(name_expr(test_local_name("kind", 0))),
        );
        target.params = vec![BlockParam {
            name: "kind".to_string(),
            role: BlockParamRole::AbruptKind,
        }];
        let entry = test_source_block(
            &function,
            vec![],
            BlockTerm::Jump(BlockEdge::with_args(
                target.label,
                vec![BlockArg::AbruptKind(AbruptKind::Exception)],
            )),
        );
        let module = test_module(
            ModuleNameGen::new(0),
            vec![with_test_blocks(function, vec![entry.clone(), target])],
        );
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: entry.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term abrupt-kind jump-arg point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_current_exception_jump_args_before_term_continuation() {
        let mut function = test_function();
        set_stack_slots(&mut function, &["exc"]);
        let mut target = test_source_block(
            &function,
            vec![],
            ret_term(name_expr(test_local_name("exc", 0))),
        );
        target.params = vec![BlockParam {
            name: "exc".to_string(),
            role: BlockParamRole::Exception,
        }];
        let entry = test_source_block(
            &function,
            vec![],
            BlockTerm::Jump(BlockEdge::with_args(
                target.label,
                vec![BlockArg::CurrentException],
            )),
        );
        let module = test_module(
            ModuleNameGen::new(0),
            vec![with_test_blocks(function, vec![entry.clone(), target])],
        );
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: entry.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term current-exception jump-arg point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_bare_raise_before_term_continuation() {
        let function = with_single_test_block(test_function(), vec![], raise_term());
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let block = function.entry_block();
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: block.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term bare raise point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(block.label, block.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_if_before_term_continuation() {
        let function = test_function();
        let then_block = test_source_block(&function, vec![], ret_term(none_expr()));
        let else_block = test_source_block(&function, vec![], ret_term(none_expr()));
        let entry = test_source_block(
            &function,
            vec![],
            BlockTerm::IfTerm(soac_core::block_py::TermIf {
                test: name_expr(test_constant_name(0)),
                then_label: then_block.label,
                else_label: else_block.label,
            }),
        );
        let module = test_module(
            ModuleNameGen::new(0),
            vec![with_test_blocks(
                function,
                vec![entry.clone(), then_block, else_block],
            )],
        );
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: entry.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term if point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_branch_table_before_term_continuation() {
        let function = test_function();
        let first_block = test_source_block(&function, vec![], ret_term(none_expr()));
        let second_block = test_source_block(&function, vec![], ret_term(none_expr()));
        let default_block = test_source_block(&function, vec![], ret_term(none_expr()));
        let entry = test_source_block(
            &function,
            vec![],
            BlockTerm::BranchTable(soac_core::block_py::TermBranchTable {
                index: name_expr(test_constant_name(0)),
                targets: vec![first_block.label, second_block.label],
                default_label: default_block.label,
            }),
        );
        let module = test_module(
            ModuleNameGen::new(0),
            vec![with_test_blocks(
                function,
                vec![entry.clone(), first_block, second_block, default_block],
            )],
        );
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: entry.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term branch-table point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
            }
        );
    }

    #[test]
    fn runtime_deopt_table_marks_callee_id_branch_table_before_term_continuation() {
        let function = test_function();
        let first_block = test_source_block(&function, vec![], ret_term(none_expr()));
        let second_block = test_source_block(&function, vec![], ret_term(none_expr()));
        let default_block = test_source_block(&function, vec![], ret_term(none_expr()));
        let entry = test_source_block(
            &function,
            vec![],
            BlockTerm::BranchTable(soac_core::block_py::TermBranchTable {
                index: InstrCodegen::CalleeFunctionId(CalleeFunctionId::new(name_expr(
                    test_constant_name(0),
                ))),
                targets: vec![first_block.label, second_block.label],
                default_label: default_block.label,
            }),
        );
        let module = test_module(
            ModuleNameGen::new(0),
            vec![with_test_blocks(
                function,
                vec![entry.clone(), first_block, second_block, default_block],
            )],
        );
        let function = &module.callable_defs[0];
        let facts = infer_module_value_facts(&module);
        let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
            .map(|prepared| prepared.deopt_resume)
            .expect("JIT deopt resume planning should succeed");
        let function_plan = module_plan
            .function(function.function_id)
            .expect("function should have a JIT deopt plan");
        let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
            .expect("runtime deopt table should build from plan");
        let point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: entry.label,
        };
        let record = table
            .record_for_point(point)
            .expect("before-term callee-id branch-table point should have a runtime record");
        assert_eq!(
            record.continuation(),
            &RuntimeJitDeoptContinuation::ResumeBlockTail {
                cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
            }
        );
    }

    #[test]
    fn deopt_unimplemented_continuation_reports_record_description() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let function = with_single_test_block(test_function(), vec![], ret_term(none_expr()));
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::unsupported(
                        RuntimeJitDeoptUnsupportedReason::UnsupportedBlockTail,
                    ),
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                result.is_null(),
                "unimplemented deopt continuation should return a null error sentinel"
            );
            let deopt_error = pyo3::PyErr::fetch(py);
            let deopt_error_text = deopt_error.to_string();
            assert!(
                deopt_error_text.contains("JIT deopt helper is not implemented")
                    && deopt_error_text.contains(&format!("function {function_id}"))
                    && deopt_error_text.contains("record 0"),
                "unimplemented deopt continuation should report the planned runtime record: {deopt_error_text}"
            );
        });
    }

    #[test]
    fn deopt_return_local_continuation_returns_owned_live_value() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(name_expr(test_name("x"))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let location = LocalLocation(0);
            let binding = LocalEnvResumeBinding {
                name: "x".to_string(),
                location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let value = unsafe { ffi::PyLong_FromLong(123_456_789) };
            assert!(!value.is_null(), "test PyLong allocation should succeed");
            let before = unsafe { ffi::Py_REFCNT(value) };
            unsafe {
                ffi::Py_INCREF(value);
            }
            let mut live_values = vec![value.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(result, value.cast::<c_void>());
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value) },
                before + 1,
                "returned deopt value should be owned by the JIT caller"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value);
            }
        });
    }

    #[test]
    fn deopt_frame_releases_unreturned_owned_live_local() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(test_function(), vec![], ret_term(none_expr()));
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let location = LocalLocation(0);
            let binding = LocalEnvResumeBinding {
                name: "x".to_string(),
                location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let value = unsafe { ffi::PyLong_FromLong(246_813_579) };
            assert!(!value.is_null(), "test PyLong allocation should succeed");
            let before = unsafe { ffi::Py_REFCNT(value) };
            unsafe {
                ffi::Py_INCREF(value);
            }
            let mut live_values = vec![value.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "block-tail deopt should continue to return None"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value) },
                before,
                "deopt frame should release the live local reference it took ownership of"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value);
            }
        });
    }

    #[test]
    fn deopt_return_global_continuation_loads_owned_value_from_globals() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(name_expr(test_global_name("x"))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let globals = unsafe { ffi::PyDict_New() };
            assert!(
                !globals.is_null(),
                "test globals dict allocation should succeed"
            );
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test key allocation should succeed");
            let value = unsafe { ffi::PyLong_FromLong(987_654_321) };
            assert!(!value.is_null(), "test PyLong allocation should succeed");
            assert_eq!(
                unsafe { ffi::PyDict_SetItem(globals, key, value) },
                0,
                "test globals dict insertion should succeed"
            );
            let before = unsafe { ffi::Py_REFCNT(value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    globals.cast(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(result, value.cast::<c_void>());
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-global deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value) },
                before + 1,
                "global deopt load should return an owned reference"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value);
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(globals);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_body_load_and_return_none() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![expr_stmt(name_expr(test_global_name("x")))],
                ret_term(none_expr()),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeInstr {
                        key: InstrKey::new(function_id, InstrId::new(block, 0)),
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let globals = unsafe { ffi::PyDict_New() };
            assert!(
                !globals.is_null(),
                "test globals dict allocation should succeed"
            );
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test key allocation should succeed");
            let value = unsafe { ffi::PyLong_FromLong(111_222_333) };
            assert!(!value.is_null(), "test PyLong allocation should succeed");
            assert_eq!(
                unsafe { ffi::PyDict_SetItem(globals, key, value) },
                0,
                "test globals dict insertion should succeed"
            );
            let before = unsafe { ffi::Py_REFCNT(value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    globals.cast(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "block-tail deopt should continue to return None"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful block-tail deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value) },
                before,
                "expression-statement global load should be decref'd before returning"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value);
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(globals);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_dispatches_body_exception_edge() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let exc_location = LocalLocation(0);
            let function = test_function();
            let mut handler = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_local_name("exc", exc_location.slot()))),
            );
            handler.params = vec![BlockParam {
                name: "exc".to_string(),
                role: BlockParamRole::Exception,
            }];
            let mut entry = test_source_block(
                &function,
                vec![op_expr(Call::new(
                    name_expr(test_constant_name(0)),
                    vec![CallArgPositional::Positional(name_expr(
                        test_constant_name(1),
                    ))],
                    Vec::<CallArgKeyword<InstrCodegen>>::new(),
                ))],
                ret_term(none_expr()),
            );
            entry.exc_edge = Some(BlockEdge::with_args(
                handler.label,
                vec![BlockArg::CurrentException],
            ));
            let function = with_test_blocks(function, vec![entry.clone(), handler]);
            let function_id = function.function_id;
            let exc_binding = LocalEnvResumeBinding {
                name: "exc".to_string(),
                location: exc_location,
                binding: LocalEnvResumeBindingState::Unbound,
                source: LocalEnvResumeValueSource::BlockParam(exc_location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let int_callable = std::ptr::addr_of_mut!(ffi::PyLong_Type).cast::<ffi::PyObject>();
            unsafe {
                ffi::Py_INCREF(int_callable);
            }
            let input = unsafe { ffi::PyUnicode_FromString(c"not-an-int".as_ptr()) };
            assert!(
                !input.is_null(),
                "test failing int input allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![int_callable.cast(), input.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeInstr {
                        key: InstrKey::new(function_id, InstrId::new(entry.label, 0)),
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![exc_binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(entry.label),
                    },
                }],
            };
            let mut live_values = vec![std::ptr::null_mut::<c_void>()];
            let before_input = unsafe { ffi::Py_REFCNT(input) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert!(
                !result.is_null(),
                "exception-edge deopt should catch the body failure and return the handler value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "caught exception-edge deopt should clear the active Python error"
            );
            assert_ne!(
                unsafe { ffi::PyExceptionInstance_Check(result.cast::<ffi::PyObject>()) },
                0,
                "exception-edge deopt should pass the raised exception instance"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(input) },
                before_input,
                "failing call argument module constant should not leak through exception dispatch"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(input);
                ffi::Py_DECREF(int_callable);
            }
        });
    }

    #[test]
    fn deopt_block_entry_continuation_executes_from_body_start() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let mut constants = TestConstantPool::default();
            let function = with_single_test_block(
                test_function(),
                vec![expr_stmt(constants.int_expr(111))],
                ret_term(constants.int_expr(222)),
            );
            let mut module = test_module(ModuleNameGen::new(0), vec![function]);
            module.module_constants = constants.module_constants;
            let function = &module.callable_defs[0];
            let block = function.entry_block();
            let body_value = unsafe { ffi::PyLong_FromLong(111_111_111) };
            assert!(
                !body_value.is_null(),
                "test body constant allocation should succeed"
            );
            let return_value = unsafe { ffi::PyLong_FromLong(222_222_222) };
            assert!(
                !return_value.is_null(),
                "test return constant allocation should succeed"
            );
            let module_constant_ptrs = vec![body_value, return_value];
            let facts = infer_module_value_facts(&module);
            let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
                .map(|prepared| prepared.deopt_resume)
                .expect("JIT deopt resume planning should succeed");
            let function_plan = module_plan
                .function(function.function_id)
                .expect("function should have a JIT deopt plan");
            let table =
                RuntimeJitDeoptTable::from_plan(function, function_plan, &module_constant_ptrs)
                    .expect("runtime deopt table should build from plan");
            let point = LocalEnvResumePoint::BlockEntry {
                function_id: function.function_id,
                block: block.label,
            };
            let ordinal = table
                .record_for_point(point)
                .expect("block-entry point should have a runtime record")
                .id()
                .ordinal as i64;

            let before_body = unsafe { ffi::Py_REFCNT(body_value) };
            let before_return = unsafe { ffi::Py_REFCNT(return_value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    ordinal,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                return_value.cast(),
                "block-entry deopt should execute body index 0 and then return"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful block-entry deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(body_value) },
                before_body,
                "block-entry deopt should consume and release the body expression value"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(return_value) },
                before_return + 1,
                "block-entry deopt should return an owned value"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(body_value);
                ffi::Py_DECREF(return_value);
            }
        });
    }

    #[test]
    fn deopt_block_entry_continuation_skips_increment_counter() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![op_expr(IncrementCounter::new(CounterId(0)))],
                ret_term(none_expr()),
            );
            let module = test_module(ModuleNameGen::new(0), vec![function]);
            let function = &module.callable_defs[0];
            let block = function.entry_block();
            let facts = infer_module_value_facts(&module);
            let module_plan = plan_jit_module_from_codegen(&module, facts.clone())
                .map(|prepared| prepared.deopt_resume)
                .expect("JIT deopt resume planning should succeed");
            let function_plan = module_plan
                .function(function.function_id)
                .expect("function should have a JIT deopt plan");
            let table = RuntimeJitDeoptTable::from_plan(function, function_plan, &[])
                .expect("runtime deopt table should build from plan");
            let point = LocalEnvResumePoint::BlockEntry {
                function_id: function.function_id,
                block: block.label,
            };
            let ordinal = table
                .record_for_point(point)
                .expect("block-entry point should have a runtime record")
                .id()
                .ordinal as i64;

            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    ordinal,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "synthetic counter replay should be a no-op and continue to the return"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful counter deopt continuation should not leave a Python exception"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_follows_no_arg_jump() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = test_function();
            let target = test_source_block(&function, vec![], ret_term(none_expr()));
            let entry = test_source_block(
                &function,
                vec![],
                BlockTerm::Jump(BlockEdge::new(target.label)),
            );
            let function = with_test_blocks(function, vec![entry.clone(), target]);
            let function_id = function.function_id;
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm {
                        function_id,
                        block: entry.label,
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
                    },
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "block-tail deopt should follow no-arg jumps and return from the target block"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful jump deopt continuation should not leave a Python exception"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_applies_simple_jump_args() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let x_location = LocalLocation(0);
            let y_location = LocalLocation(1);
            let function = test_function();
            let mut target = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_local_name("y", y_location.slot()))),
            );
            target.params = vec![BlockParam {
                name: "y".to_string(),
                role: BlockParamRole::AbruptPayload,
            }];
            let entry = test_source_block(
                &function,
                vec![],
                BlockTerm::Jump(BlockEdge::with_args(
                    target.label,
                    vec![BlockArg::Name("x".to_string())],
                )),
            );
            let function = with_test_blocks(function, vec![entry.clone(), target]);
            let function_id = function.function_id;
            let x_binding = LocalEnvResumeBinding {
                name: "x".to_string(),
                location: x_location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(x_location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let y_binding = LocalEnvResumeBinding {
                name: "y".to_string(),
                location: y_location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(y_location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm {
                        function_id,
                        block: entry.label,
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![x_binding, y_binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
                    },
                }],
            };
            let x_value = unsafe { ffi::PyLong_FromLong(123_123_123) };
            assert!(!x_value.is_null(), "test x allocation should succeed");
            let old_y_value = unsafe { ffi::PyLong_FromLong(456_456_456) };
            assert!(
                !old_y_value.is_null(),
                "test old y allocation should succeed"
            );
            let before_x = unsafe { ffi::Py_REFCNT(x_value) };
            let before_old_y = unsafe { ffi::Py_REFCNT(old_y_value) };
            unsafe {
                ffi::Py_INCREF(x_value);
                ffi::Py_INCREF(old_y_value);
            }
            let mut live_values = vec![x_value.cast::<c_void>(), old_y_value.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                x_value.cast(),
                "jump-arg deopt should bind target param y to source local x"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful jump-arg deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(x_value) },
                before_x + 1,
                "returned jump-arg value should be owned by the JIT caller after frame cleanup"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(old_y_value) },
                before_old_y,
                "jump-arg target rebinding should release the replaced frame-owned local"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(x_value);
                ffi::Py_DECREF(old_y_value);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_applies_none_jump_arg() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let y_location = LocalLocation(0);
            let function = test_function();
            let mut target = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_local_name("y", y_location.slot()))),
            );
            target.params = vec![BlockParam {
                name: "y".to_string(),
                role: BlockParamRole::AbruptPayload,
            }];
            let entry = test_source_block(
                &function,
                vec![],
                BlockTerm::Jump(BlockEdge::with_args(target.label, vec![BlockArg::None])),
            );
            let function = with_test_blocks(function, vec![entry.clone(), target]);
            let function_id = function.function_id;
            let y_binding = LocalEnvResumeBinding {
                name: "y".to_string(),
                location: y_location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(y_location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm {
                        function_id,
                        block: entry.label,
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![y_binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
                    },
                }],
            };
            let old_y_value = unsafe { ffi::PyLong_FromLong(789_789_789) };
            assert!(
                !old_y_value.is_null(),
                "test old y allocation should succeed"
            );
            let before_old_y = unsafe { ffi::Py_REFCNT(old_y_value) };
            unsafe {
                ffi::Py_INCREF(old_y_value);
            }
            let mut live_values = vec![old_y_value.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "jump None arg should bind the target param to None"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful None jump-arg deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(old_y_value) },
                before_old_y,
                "jump None arg should release the replaced frame-owned local"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(old_y_value);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_applies_abrupt_kind_jump_arg() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let kind_location = LocalLocation(0);
            let function = test_function();
            let mut target = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_local_name("kind", kind_location.slot()))),
            );
            target.params = vec![BlockParam {
                name: "kind".to_string(),
                role: BlockParamRole::AbruptKind,
            }];
            let entry = test_source_block(
                &function,
                vec![],
                BlockTerm::Jump(BlockEdge::with_args(
                    target.label,
                    vec![BlockArg::AbruptKind(AbruptKind::Return)],
                )),
            );
            let function = with_test_blocks(function, vec![entry.clone(), target]);
            let function_id = function.function_id;
            let kind_binding = LocalEnvResumeBinding {
                name: "kind".to_string(),
                location: kind_location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(kind_location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm {
                        function_id,
                        block: entry.label,
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![kind_binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
                    },
                }],
            };
            let old_kind_value = unsafe { ffi::PyLong_FromLong(987_654_321) };
            assert!(
                !old_kind_value.is_null(),
                "test old abrupt-kind allocation should succeed"
            );
            let before_old_kind = unsafe { ffi::Py_REFCNT(old_kind_value) };
            unsafe {
                ffi::Py_INCREF(old_kind_value);
            }
            let mut live_values = vec![old_kind_value.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert!(
                !result.is_null(),
                "jump abrupt-kind arg should produce a Python int"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful abrupt-kind jump-arg deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(result.cast::<ffi::PyObject>()) },
                abrupt_kind_tag(AbruptKind::Return),
                "jump abrupt-kind arg should bind the target param to its integer tag"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(old_kind_value) },
                before_old_kind,
                "jump abrupt-kind arg should release the replaced frame-owned local"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(old_kind_value);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_applies_current_exception_jump_arg() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let exc_location = LocalLocation(0);
            let function = test_function();
            let mut target = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_local_name("exc", exc_location.slot()))),
            );
            target.params = vec![BlockParam {
                name: "exc".to_string(),
                role: BlockParamRole::Exception,
            }];
            let entry = test_source_block(
                &function,
                vec![],
                BlockTerm::Jump(BlockEdge::with_args(
                    target.label,
                    vec![BlockArg::CurrentException],
                )),
            );
            let function = with_test_blocks(function, vec![entry.clone(), target]);
            let function_id = function.function_id;
            let exc_binding = LocalEnvResumeBinding {
                name: "exc".to_string(),
                location: exc_location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(exc_location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm {
                        function_id,
                        block: entry.label,
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![exc_binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
                    },
                }],
            };
            let old_exc_value = unsafe { ffi::PyLong_FromLong(123_456_789) };
            assert!(
                !old_exc_value.is_null(),
                "test old exception-param allocation should succeed"
            );
            let before_old_exc = unsafe { ffi::Py_REFCNT(old_exc_value) };
            let exc = unsafe { ffi::PyObject_CallNoArgs(ffi::PyExc_ValueError) };
            assert!(
                !exc.is_null(),
                "test current exception allocation should succeed"
            );
            let before_exc = unsafe { ffi::Py_REFCNT(exc) };
            unsafe {
                ffi::Py_INCREF(old_exc_value);
                ffi::Py_INCREF(exc);
                ffi::PyErr_SetRaisedException(exc);
            }
            let mut live_values = vec![old_exc_value.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                exc.cast(),
                "jump current-exception arg should bind the target param to the active exception"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "current-exception jump arg should consume the active error state"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(old_exc_value) },
                before_old_exc,
                "current-exception jump arg should release the replaced frame-owned local"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(exc) },
                before_exc + 1,
                "returned current exception should be owned by the JIT caller after frame cleanup"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(exc);
                ffi::Py_DECREF(old_exc_value);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_follows_if_term() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = test_function();
            let then_block = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_constant_name(1))),
            );
            let else_block = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_constant_name(2))),
            );
            let entry = test_source_block(
                &function,
                vec![],
                BlockTerm::IfTerm(soac_core::block_py::TermIf {
                    test: name_expr(test_constant_name(0)),
                    then_label: then_block.label,
                    else_label: else_block.label,
                }),
            );
            let function = with_test_blocks(function, vec![entry.clone(), then_block, else_block]);
            let function_id = function.function_id;
            let condition = unsafe { ffi::PyList_New(0) };
            assert!(
                !condition.is_null(),
                "test condition allocation should succeed"
            );
            let then_value = unsafe { ffi::PyLong_FromLong(444_444_444) };
            assert!(
                !then_value.is_null(),
                "test then-value allocation should succeed"
            );
            let else_value = unsafe { ffi::PyLong_FromLong(555_555_555) };
            assert!(
                !else_value.is_null(),
                "test else-value allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![condition.cast(), then_value.cast(), else_value.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm {
                        function_id,
                        block: entry.label,
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
                    },
                }],
            };
            let before_condition = unsafe { ffi::Py_REFCNT(condition) };
            let before_else = unsafe { ffi::Py_REFCNT(else_value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                else_value.cast(),
                "false if-term deopt should continue through the else target"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful if deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(condition) },
                before_condition,
                "if-term truthiness should release its owned condition load"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(else_value) },
                before_else + 1,
                "if-term target return should be owned by the JIT caller"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(else_value);
                ffi::Py_DECREF(then_value);
                ffi::Py_DECREF(condition);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_follows_branch_table_term() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = test_function();
            let first_block = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_constant_name(1))),
            );
            let second_block = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_constant_name(2))),
            );
            let default_block = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_constant_name(3))),
            );
            let entry = test_source_block(
                &function,
                vec![],
                BlockTerm::BranchTable(soac_core::block_py::TermBranchTable {
                    index: name_expr(test_constant_name(0)),
                    targets: vec![first_block.label, second_block.label],
                    default_label: default_block.label,
                }),
            );
            let function = with_test_blocks(
                function,
                vec![entry.clone(), first_block, second_block, default_block],
            );
            let function_id = function.function_id;
            let index = unsafe { ffi::PyLong_FromLong(1) };
            assert!(!index.is_null(), "test index allocation should succeed");
            let first_value = unsafe { ffi::PyLong_FromLong(666_666_666) };
            assert!(
                !first_value.is_null(),
                "test first-value allocation should succeed"
            );
            let second_value = unsafe { ffi::PyLong_FromLong(777_777_777) };
            assert!(
                !second_value.is_null(),
                "test second-value allocation should succeed"
            );
            let default_value = unsafe { ffi::PyLong_FromLong(888_888_888) };
            assert!(
                !default_value.is_null(),
                "test default-value allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![
                    index.cast(),
                    first_value.cast(),
                    second_value.cast(),
                    default_value.cast(),
                ],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm {
                        function_id,
                        block: entry.label,
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
                    },
                }],
            };
            let before_second = unsafe { ffi::Py_REFCNT(second_value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                second_value.cast(),
                "branch-table deopt should continue through the indexed target"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful branch-table deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(second_value) },
                before_second + 1,
                "branch-table target return should be owned by the JIT caller"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(default_value);
                ffi::Py_DECREF(second_value);
                ffi::Py_DECREF(first_value);
                ffi::Py_DECREF(index);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_follows_callee_id_branch_table_term() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let function = test_function();
            let first_block = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_constant_name(1))),
            );
            let second_block = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_constant_name(2))),
            );
            let default_block = test_source_block(
                &function,
                vec![],
                ret_term(name_expr(test_constant_name(3))),
            );
            let entry = test_source_block(
                &function,
                vec![],
                BlockTerm::BranchTable(soac_core::block_py::TermBranchTable {
                    index: InstrCodegen::CalleeFunctionId(CalleeFunctionId::new(name_expr(
                        test_constant_name(0),
                    ))),
                    targets: vec![first_block.label, second_block.label],
                    default_label: default_block.label,
                }),
            );
            let function = with_test_blocks(
                function,
                vec![entry.clone(), first_block, second_block, default_block],
            );
            let function_id = function.function_id;
            let module = PyModule::from_code(
                py,
                c"
def g():
    return None
",
                c"deopt_callee_id.py",
                c"deopt_callee_id",
            )
            .expect("test module should execute");
            let callable = module
                .getattr("g")
                .expect("test function should exist")
                .as_ptr();
            unsafe {
                ffi::Py_INCREF(callable);
                assert_eq!(
                    crate::PyFunction_SetSoacMetadata(
                        callable,
                        RuntimeFunctionId::from_packed_runtime_u64(1).to_packed_runtime_u64(),
                        std::ptr::null_mut(),
                        None,
                    ),
                    0,
                    "test function should accept SOAC function id metadata"
                );
            }
            let first_value = unsafe { ffi::PyLong_FromLong(111_111_111) };
            assert!(
                !first_value.is_null(),
                "test first-value allocation should succeed"
            );
            let second_value = unsafe { ffi::PyLong_FromLong(222_222_222) };
            assert!(
                !second_value.is_null(),
                "test second-value allocation should succeed"
            );
            let default_value = unsafe { ffi::PyLong_FromLong(333_333_333) };
            assert!(
                !default_value.is_null(),
                "test default-value allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![
                    callable.cast(),
                    first_value.cast(),
                    second_value.cast(),
                    default_value.cast(),
                ],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm {
                        function_id,
                        block: entry.label,
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::new(entry.label, entry.body.len()),
                    },
                }],
            };
            let before_callable = unsafe { ffi::Py_REFCNT(callable) };
            let before_second = unsafe { ffi::Py_REFCNT(second_value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                second_value.cast(),
                "callee-id branch-table deopt should select the branch matching the function id"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful callee-id branch-table deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(callable) },
                before_callable,
                "callee-id evaluation should release its owned callable load"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(second_value) },
                before_second + 1,
                "callee-id branch target return should be owned by the JIT caller"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(default_value);
                ffi::Py_DECREF(second_value);
                ffi::Py_DECREF(first_value);
                ffi::Py_DECREF(callable);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_returns_owned_module_constant() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![expr_stmt(name_expr(test_global_name("x")))],
                ret_term(name_expr(test_constant_name(0))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let constant = unsafe { ffi::PyLong_FromLong(444_555_666) };
            assert!(
                !constant.is_null(),
                "test module constant allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![constant.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeInstr {
                        key: InstrKey::new(function_id, InstrId::new(block, 0)),
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let globals = unsafe { ffi::PyDict_New() };
            assert!(
                !globals.is_null(),
                "test globals dict allocation should succeed"
            );
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test key allocation should succeed");
            let value = unsafe { ffi::PyLong_FromLong(222_333_444) };
            assert!(!value.is_null(), "test PyLong allocation should succeed");
            assert_eq!(
                unsafe { ffi::PyDict_SetItem(globals, key, value) },
                0,
                "test globals dict insertion should succeed"
            );
            let before_constant = unsafe { ffi::Py_REFCNT(constant) };
            let before_value = unsafe { ffi::Py_REFCNT(value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    globals.cast(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                constant.cast(),
                "block-tail deopt should return the module constant"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful block-tail constant deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(constant) },
                before_constant + 1,
                "returned module constant should be owned by the JIT caller"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value) },
                before_value,
                "expression-statement global load should still be decref'd before returning"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value);
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(globals);
                ffi::Py_DECREF(constant);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_binop() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(BinOp::new(
                    BinOpKind::Add,
                    name_expr(test_constant_name(0)),
                    name_expr(test_constant_name(1)),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let left = unsafe { ffi::PyLong_FromLong(222_333_444) };
            assert!(
                !left.is_null(),
                "test left PyLong allocation should succeed"
            );
            let right = unsafe { ffi::PyLong_FromLong(111_222_333) };
            assert!(
                !right.is_null(),
                "test right PyLong allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![left.cast(), right.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_left = unsafe { ffi::Py_REFCNT(left) };
            let before_right = unsafe { ffi::Py_REFCNT(right) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-binop deopt should produce a value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-binop deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(result.cast::<ffi::PyObject>()) },
                333_555_777,
                "return-binop deopt should execute PyNumber_Add"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(left) },
                before_left,
                "left module constant should not leak through binop execution"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(right) },
                before_right,
                "right module constant should not leak through binop execution"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(right);
                ffi::Py_DECREF(left);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_unary_ops() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            for (kind, input, expected, check_refcount) in [
                (UnaryOpKind::Pos, 222_333_444, 222_333_444, true),
                (UnaryOpKind::Neg, 222_333_444, -222_333_444, true),
                (UnaryOpKind::Invert, 222_333_444, -222_333_445, true),
                (UnaryOpKind::Not, 0, 1, false),
                (UnaryOpKind::Truth, 0, 0, false),
            ] {
                let function = with_single_test_block(
                    test_function(),
                    vec![],
                    ret_term(op_expr(UnaryOp::new(
                        kind,
                        name_expr(test_constant_name(0)),
                    ))),
                );
                let function_id = function.function_id;
                let block = function.entry_block().label;
                let operand = unsafe { ffi::PyLong_FromLong(input) };
                assert!(
                    !operand.is_null(),
                    "test unary operand PyLong allocation should succeed"
                );
                let table = RuntimeJitDeoptTable {
                    function_id,
                    function: Box::new(function),
                    module_constant_ptrs: vec![operand.cast()],
                    points: vec![RuntimeJitDeoptRecord {
                        id: PlannedJitDeoptPointId {
                            function_id,
                            ordinal: 0,
                        },
                        resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                        precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                        locals: vec![],
                        continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                            cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                        },
                    }],
                };
                let before_operand = unsafe { ffi::Py_REFCNT(operand) };
                let result = unsafe {
                    test_dp_jit_deopt_resume(
                        std::ptr::addr_of!(table).cast_mut().cast(),
                        std::ptr::null_mut(),
                        0,
                        std::ptr::null_mut(),
                        0,
                    )
                };
                assert!(
                    !result.is_null(),
                    "return-unary deopt should produce a value for {kind:?}"
                );
                assert!(
                    unsafe { ffi::PyErr_Occurred() }.is_null(),
                    "successful return-unary deopt should not leave a Python exception for {kind:?}"
                );
                assert_eq!(
                    unsafe { ffi::PyLong_AsLongLong(result.cast::<ffi::PyObject>()) },
                    expected,
                    "return-unary deopt should execute {kind:?}"
                );
                unsafe {
                    ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                }
                if check_refcount {
                    assert_eq!(
                        unsafe { ffi::Py_REFCNT(operand) },
                        before_operand,
                        "operand module constant should not leak after releasing unary result for {kind:?}"
                    );
                }
                unsafe {
                    ffi::Py_DECREF(operand);
                }
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_getattr() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(GetAttr::new(
                    name_expr(test_constant_name(0)),
                    name_expr(test_constant_name(1)),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let value = unsafe { ffi::PyLong_FromLong(222_333_444) };
            assert!(
                !value.is_null(),
                "test getattr value PyLong allocation should succeed"
            );
            let attr = unsafe { ffi::PyUnicode_FromString(c"denominator".as_ptr()) };
            assert!(
                !attr.is_null(),
                "test getattr attr string allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![value.cast(), attr.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-getattr deopt should produce a value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-getattr deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(result.cast::<ffi::PyObject>()) },
                1,
                "return-getattr deopt should execute PyObject_GetAttr"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(attr);
                ffi::Py_DECREF(value);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_getitem() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(GetItem::new(
                    name_expr(test_constant_name(0)),
                    name_expr(test_constant_name(1)),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let list = unsafe { ffi::PyList_New(1) };
            assert!(
                !list.is_null(),
                "test getitem list allocation should succeed"
            );
            let item = unsafe { ffi::PyLong_FromLong(777_888_999) };
            assert!(
                !item.is_null(),
                "test getitem item allocation should succeed"
            );
            unsafe {
                ffi::Py_INCREF(item);
            }
            assert_eq!(
                unsafe { ffi::PyList_SetItem(list, 0, item) },
                0,
                "test getitem list setup should succeed"
            );
            let index = unsafe { ffi::PyLong_FromLong(0) };
            assert!(
                !index.is_null(),
                "test getitem index allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![list.cast(), index.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-getitem deopt should produce a value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-getitem deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(result.cast::<ffi::PyObject>()) },
                777_888_999,
                "return-getitem deopt should execute PyObject_GetItem"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(index);
                ffi::Py_DECREF(list);
                ffi::Py_DECREF(item);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_setattr() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(SetAttr::new(
                    name_expr(test_constant_name(0)),
                    name_expr(test_constant_name(1)),
                    name_expr(test_constant_name(2)),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let module = unsafe { ffi::PyModule_New(c"deopt_setattr_test".as_ptr()) };
            assert!(
                !module.is_null(),
                "test setattr module allocation should succeed"
            );
            let attr = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(
                !attr.is_null(),
                "test setattr attr allocation should succeed"
            );
            let replacement = unsafe { ffi::PyLong_FromLong(444_555_666) };
            assert!(
                !replacement.is_null(),
                "test setattr replacement allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![module.cast(), attr.cast(), replacement.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "return-setattr deopt should return owned None on success"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-setattr deopt should not leave a Python exception"
            );
            let stored = unsafe { ffi::PyObject_GetAttr(module, attr) };
            assert!(
                !stored.is_null(),
                "return-setattr deopt should write the requested attribute"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(stored) },
                444_555_666,
                "return-setattr deopt should write the replacement value"
            );
            unsafe {
                ffi::Py_DECREF(stored);
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(module);
                ffi::Py_DECREF(replacement);
                ffi::Py_DECREF(attr);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_setitem() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(SetItem::new(
                    name_expr(test_constant_name(0)),
                    name_expr(test_constant_name(1)),
                    name_expr(test_constant_name(2)),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let dict = unsafe { ffi::PyDict_New() };
            assert!(
                !dict.is_null(),
                "test setitem dict allocation should succeed"
            );
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test setitem key allocation should succeed");
            let replacement = unsafe { ffi::PyLong_FromLong(555_666_777) };
            assert!(
                !replacement.is_null(),
                "test setitem replacement allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![dict.cast(), key.cast(), replacement.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "return-setitem deopt should return owned None on success"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-setitem deopt should not leave a Python exception"
            );
            let stored = unsafe { ffi::PyDict_GetItemWithError(dict, key) };
            assert!(
                !stored.is_null(),
                "return-setitem deopt should write the requested key"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(stored) },
                555_666_777,
                "return-setitem deopt should write the replacement value"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(dict);
                ffi::Py_DECREF(replacement);
                ffi::Py_DECREF(key);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_delitem() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(DelItem::new(
                    name_expr(test_constant_name(0)),
                    name_expr(test_constant_name(1)),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let dict = unsafe { ffi::PyDict_New() };
            assert!(
                !dict.is_null(),
                "test delitem dict allocation should succeed"
            );
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test delitem key allocation should succeed");
            let value = unsafe { ffi::PyLong_FromLong(666_777_888) };
            assert!(
                !value.is_null(),
                "test delitem value allocation should succeed"
            );
            assert_eq!(
                unsafe { ffi::PyDict_SetItem(dict, key, value) },
                0,
                "test delitem dict setup should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![dict.cast(), key.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "return-delitem deopt should return owned None on success"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-delitem deopt should not leave a Python exception"
            );
            assert!(
                unsafe { ffi::PyDict_GetItemWithError(dict, key) }.is_null(),
                "return-delitem deopt should delete the requested key"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "deleted key lookup should not leave a Python exception"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value);
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(dict);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_positional_call() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(Call::new(
                    name_expr(test_constant_name(0)),
                    vec![CallArgPositional::Positional(name_expr(
                        test_constant_name(1),
                    ))],
                    Vec::<CallArgKeyword<InstrCodegen>>::new(),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let int_callable = std::ptr::addr_of_mut!(ffi::PyLong_Type).cast::<ffi::PyObject>();
            unsafe {
                ffi::Py_INCREF(int_callable);
            }
            let input = unsafe { ffi::PyUnicode_FromString(c"222333444".as_ptr()) };
            assert!(
                !input.is_null(),
                "test call input string allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![int_callable.cast(), input.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_input = unsafe { ffi::Py_REFCNT(input) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-call deopt should produce a value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-call deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(result.cast::<ffi::PyObject>()) },
                222_333_444,
                "return-call deopt should execute PyObject_CallObject"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(input) },
                before_input,
                "argument module constant should not leak through call execution"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(input);
                ffi::Py_DECREF(int_callable);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_starred_positional_call() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(Call::new(
                    name_expr(test_constant_name(0)),
                    vec![CallArgPositional::Starred(name_expr(test_constant_name(1)))],
                    Vec::<CallArgKeyword<InstrCodegen>>::new(),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let int_callable = std::ptr::addr_of_mut!(ffi::PyLong_Type).cast::<ffi::PyObject>();
            unsafe {
                ffi::Py_INCREF(int_callable);
            }
            let input = unsafe { ffi::PyUnicode_FromString(c"444555666".as_ptr()) };
            assert!(
                !input.is_null(),
                "test starred call input string allocation should succeed"
            );
            let starred_args = unsafe { ffi::PyTuple_New(1) };
            assert!(
                !starred_args.is_null(),
                "test starred args tuple allocation should succeed"
            );
            assert_eq!(
                unsafe { ffi::PyTuple_SetItem(starred_args, 0, input) },
                0,
                "test starred args tuple should accept the input item"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![int_callable.cast(), starred_args.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_starred_args = unsafe { ffi::Py_REFCNT(starred_args) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-starred-positional-call deopt should produce a value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-starred-positional-call deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(result.cast::<ffi::PyObject>()) },
                444_555_666,
                "return-starred-positional-call deopt should expand the starred tuple"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(starred_args) },
                before_starred_args,
                "starred args module constant should not leak through call execution"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(starred_args);
                ffi::Py_DECREF(int_callable);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_keyword_call() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(Call::new(
                    name_expr(test_constant_name(0)),
                    Vec::<CallArgPositional<InstrCodegen>>::new(),
                    vec![CallArgKeyword::Named {
                        arg: "x".into(),
                        value: name_expr(test_constant_name(1)),
                    }],
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let dict_callable = std::ptr::addr_of_mut!(ffi::PyDict_Type).cast::<ffi::PyObject>();
            unsafe {
                ffi::Py_INCREF(dict_callable);
            }
            let value = unsafe { ffi::PyLong_FromLong(777_888_999) };
            assert!(
                !value.is_null(),
                "test keyword value PyLong allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![dict_callable.cast(), value.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_value = unsafe { ffi::Py_REFCNT(value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-keyword-call deopt should produce a value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-keyword-call deopt should not leave a Python exception"
            );
            assert_ne!(
                unsafe { ffi::PyDict_Check(result.cast::<ffi::PyObject>()) },
                0,
                "return-keyword-call deopt should call dict with kwargs"
            );
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test keyword key allocation should succeed");
            let stored =
                unsafe { ffi::PyDict_GetItemWithError(result.cast::<ffi::PyObject>(), key) };
            assert!(
                !stored.is_null(),
                "return-keyword-call deopt should store the named keyword in the result dict"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(stored) },
                777_888_999,
                "return-keyword-call deopt should preserve the keyword value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "result dict lookup should not leave a Python exception"
            );
            unsafe {
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            }
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value) },
                before_value,
                "keyword value module constant should not leak after releasing the call result"
            );
            unsafe {
                ffi::Py_DECREF(value);
                ffi::Py_DECREF(dict_callable);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_starred_keyword_call() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(Call::new(
                    name_expr(test_constant_name(0)),
                    Vec::<CallArgPositional<InstrCodegen>>::new(),
                    vec![CallArgKeyword::Starred(name_expr(test_constant_name(1)))],
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let dict_callable = std::ptr::addr_of_mut!(ffi::PyDict_Type).cast::<ffi::PyObject>();
            unsafe {
                ffi::Py_INCREF(dict_callable);
            }
            let kwargs = unsafe { ffi::PyDict_New() };
            assert!(!kwargs.is_null(), "test kwargs allocation should succeed");
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test keyword key allocation should succeed");
            let value = unsafe { ffi::PyLong_FromLong(123_456_789) };
            assert!(
                !value.is_null(),
                "test keyword value allocation should succeed"
            );
            assert_eq!(
                unsafe { ffi::PyDict_SetItem(kwargs, key, value) },
                0,
                "test kwargs dict should accept the keyword"
            );
            unsafe {
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(value);
            }
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![dict_callable.cast(), kwargs.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_kwargs = unsafe { ffi::Py_REFCNT(kwargs) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-starred-keyword-call deopt should produce a value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-starred-keyword-call deopt should not leave a Python exception"
            );
            assert_ne!(
                unsafe { ffi::PyDict_Check(result.cast::<ffi::PyObject>()) },
                0,
                "return-starred-keyword-call deopt should call dict with kwargs"
            );
            let lookup_key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(
                !lookup_key.is_null(),
                "test keyword lookup key allocation should succeed"
            );
            let stored =
                unsafe { ffi::PyDict_GetItemWithError(result.cast::<ffi::PyObject>(), lookup_key) };
            assert!(
                !stored.is_null(),
                "return-starred-keyword-call deopt should store the unpacked keyword"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(stored) },
                123_456_789,
                "return-starred-keyword-call deopt should preserve the unpacked value"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(kwargs) },
                before_kwargs,
                "starred kwargs module constant should not leak through call execution"
            );
            unsafe {
                ffi::Py_DECREF(lookup_key);
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(kwargs);
                ffi::Py_DECREF(dict_callable);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_rejects_duplicate_starred_keyword_call() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(Call::new(
                    name_expr(test_constant_name(0)),
                    Vec::<CallArgPositional<InstrCodegen>>::new(),
                    vec![
                        CallArgKeyword::Starred(name_expr(test_constant_name(1))),
                        CallArgKeyword::Named {
                            arg: "x".into(),
                            value: name_expr(test_constant_name(2)),
                        },
                    ],
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let dict_callable = std::ptr::addr_of_mut!(ffi::PyDict_Type).cast::<ffi::PyObject>();
            unsafe {
                ffi::Py_INCREF(dict_callable);
            }
            let kwargs = unsafe { ffi::PyDict_New() };
            assert!(!kwargs.is_null(), "test kwargs allocation should succeed");
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test keyword key allocation should succeed");
            let first = unsafe { ffi::PyLong_FromLong(1) };
            assert!(
                !first.is_null(),
                "test first keyword value allocation should succeed"
            );
            assert_eq!(
                unsafe { ffi::PyDict_SetItem(kwargs, key, first) },
                0,
                "test kwargs dict should accept the first keyword"
            );
            unsafe {
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(first);
            }
            let duplicate = unsafe { ffi::PyLong_FromLong(2) };
            assert!(
                !duplicate.is_null(),
                "test duplicate keyword value allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![dict_callable.cast(), kwargs.cast(), duplicate.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                result.is_null(),
                "duplicate-starred-keyword-call deopt should signal a Python error"
            );
            assert_ne!(
                unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_TypeError) },
                0,
                "duplicate-starred-keyword-call deopt should raise TypeError"
            );
            unsafe {
                ffi::PyErr_Clear();
                ffi::Py_DECREF(duplicate);
                ffi::Py_DECREF(kwargs);
                ffi::Py_DECREF(dict_callable);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_call_direct() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(InstrCodegen::CallDirect(CallDirect::new(
                    name_expr(test_constant_name(0)),
                    RuntimeFunctionId::from_raw_parts(0, 999),
                    vec![CallArgPositional::Positional(name_expr(
                        test_constant_name(1),
                    ))],
                    Vec::<CallArgKeyword<InstrCodegen>>::new(),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let int_callable = std::ptr::addr_of_mut!(ffi::PyLong_Type).cast::<ffi::PyObject>();
            unsafe {
                ffi::Py_INCREF(int_callable);
            }
            let input = unsafe { ffi::PyUnicode_FromString(c"333444555".as_ptr()) };
            assert!(
                !input.is_null(),
                "test direct-call input string allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![int_callable.cast(), input.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_input = unsafe { ffi::Py_REFCNT(input) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-call-direct deopt should produce a value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-call-direct deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::PyLong_AsLongLong(result.cast::<ffi::PyObject>()) },
                333_444_555,
                "return-call-direct deopt should execute a generic Python call"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(input) },
                before_input,
                "direct-call argument module constant should not leak through call execution"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(input);
                ffi::Py_DECREF(int_callable);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_make_cell() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(MakeCell::with_initial_value(name_expr(
                    test_constant_name(0),
                )))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let cell_value = unsafe { ffi::PyLong_FromLong(123_321_123) };
            assert!(
                !cell_value.is_null(),
                "test cell value allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![cell_value.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_value = unsafe { ffi::Py_REFCNT(cell_value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-make-cell deopt should produce a cell object"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-make-cell deopt should not leave a Python exception"
            );
            let contents = unsafe {
                ffi::PyObject_GetAttrString(
                    result.cast::<ffi::PyObject>(),
                    c"cell_contents".as_ptr(),
                )
            };
            assert!(
                !contents.is_null(),
                "return-make-cell deopt should populate the returned cell"
            );
            assert_eq!(
                contents, cell_value,
                "return-make-cell deopt should populate the cell with the initial value"
            );
            unsafe {
                ffi::Py_DECREF(contents);
            }
            assert_eq!(
                unsafe { ffi::Py_REFCNT(cell_value) },
                before_value + 1,
                "returned cell should own one reference to the initial value"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            }
            assert_eq!(
                unsafe { ffi::Py_REFCNT(cell_value) },
                before_value,
                "dropping the returned cell should release the initial value"
            );
            unsafe {
                ffi::Py_DECREF(cell_value);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_empty_make_cell() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(MakeCell::empty())),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                !result.is_null(),
                "return-empty-make-cell deopt should produce a cell object"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful return-empty-make-cell deopt should not leave a Python exception"
            );
            let contents = unsafe {
                ffi::PyObject_GetAttrString(
                    result.cast::<ffi::PyObject>(),
                    c"cell_contents".as_ptr(),
                )
            };
            assert!(
                contents.is_null(),
                "return-empty-make-cell deopt should leave the returned cell empty"
            );
            assert_ne!(
                unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) },
                0,
                "empty cell_contents access should raise ValueError"
            );
            unsafe {
                ffi::PyErr_Clear();
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_owned_cell_ref() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let cell_location = LocalLocation(0);
            let mut function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(soac_core::block_py::CellRef::new(
                    CellLocation::Owned(0),
                ))),
            );
            function.storage_layout = Some(StorageLayout {
                freevars: vec![],
                cellvars: vec![ClosureSlot {
                    logical_name: "cell".to_string(),
                    storage_name: "cell".to_string(),
                    init: ClosureInit::Deferred,
                }],
                runtime_cells: vec![],
                stack_slots: vec!["cell".to_string()],
            });
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let cell_contents = unsafe { ffi::PyLong_FromLong(321_123_321) };
            assert!(
                !cell_contents.is_null(),
                "test cell contents allocation should succeed"
            );
            let cell = unsafe { PyCell_New(cell_contents) };
            assert!(!cell.is_null(), "test cell allocation should succeed");
            let binding = LocalEnvResumeBinding {
                name: "cell".to_string(),
                location: cell_location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(cell_location),
                ownership: LocalRefKind::Borrowed,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_cell = unsafe { ffi::Py_REFCNT(cell) };
            let mut live_values = vec![cell.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                cell.cast(),
                "owned-cell-ref deopt should return the materialized cell object"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful owned-cell-ref deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(cell) },
                before_cell + 1,
                "owned-cell-ref deopt should return an owned reference"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            }
            assert_eq!(
                unsafe { ffi::Py_REFCNT(cell) },
                before_cell,
                "dropping the returned cell_ref should release the returned reference"
            );
            unsafe {
                ffi::Py_DECREF(cell);
                ffi::Py_DECREF(cell_contents);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_closure_cell_ref() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(soac_core::block_py::CellRef::new(
                    CellLocation::Closure(0),
                ))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let layout = FunctionRuntimeDataLayout::from_function(&function);
            let cell_contents = unsafe { ffi::PyLong_FromLong(987_654_321) };
            assert!(
                !cell_contents.is_null(),
                "test cell contents allocation should succeed"
            );
            let cell = unsafe { PyCell_New(cell_contents) };
            assert!(!cell.is_null(), "test cell allocation should succeed");
            let mut function_data: Vec<ObjPtr> = vec![std::ptr::null_mut(); layout.total_len()];
            function_data[layout.closure_cell_slot(0)] = cell.cast();
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_cell = unsafe { ffi::Py_REFCNT(cell) };
            let result = unsafe {
                test_dp_jit_deopt_resume_with_function_data(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    function_data.as_mut_ptr().cast(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                cell.cast(),
                "closure-cell-ref deopt should return the function-data cell object"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful closure-cell-ref deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(cell) },
                before_cell + 1,
                "closure-cell-ref deopt should return an owned reference"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(cell);
                ffi::Py_DECREF(cell_contents);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_owned_cell_load() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let cell_location = LocalLocation(0);
            let mut function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(name_expr(test_owned_cell_name("cell", 0))),
            );
            function.storage_layout = Some(StorageLayout {
                freevars: vec![],
                cellvars: vec![ClosureSlot {
                    logical_name: "cell".to_string(),
                    storage_name: "cell".to_string(),
                    init: ClosureInit::Deferred,
                }],
                runtime_cells: vec![],
                stack_slots: vec!["cell".to_string()],
            });
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let cell_contents = unsafe { ffi::PyLong_FromLong(654_456_654) };
            assert!(
                !cell_contents.is_null(),
                "test cell contents allocation should succeed"
            );
            let cell = unsafe { PyCell_New(cell_contents) };
            assert!(!cell.is_null(), "test cell allocation should succeed");
            let binding = LocalEnvResumeBinding {
                name: "cell".to_string(),
                location: cell_location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(cell_location),
                ownership: LocalRefKind::Borrowed,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_contents = unsafe { ffi::Py_REFCNT(cell_contents) };
            let before_cell = unsafe { ffi::Py_REFCNT(cell) };
            let mut live_values = vec![cell.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                cell_contents.cast(),
                "owned-cell-load deopt should return the cell contents"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful owned-cell-load deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(cell_contents) },
                before_contents + 1,
                "owned-cell-load deopt should return an owned reference to the contents"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(cell) },
                before_cell,
                "owned-cell-load deopt should release its temporary cell reference"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            }
            assert_eq!(
                unsafe { ffi::Py_REFCNT(cell_contents) },
                before_contents,
                "dropping the returned cell contents should release the returned reference"
            );
            unsafe {
                ffi::Py_DECREF(cell);
                ffi::Py_DECREF(cell_contents);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_owned_cell_store() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let cell_location = LocalLocation(0);
            let mut function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(Store::new(
                    test_owned_cell_name("cell", 0),
                    name_expr(test_constant_name(0)),
                ))),
            );
            function.storage_layout = Some(StorageLayout {
                freevars: vec![],
                cellvars: vec![ClosureSlot {
                    logical_name: "cell".to_string(),
                    storage_name: "cell".to_string(),
                    init: ClosureInit::Deferred,
                }],
                runtime_cells: vec![],
                stack_slots: vec!["cell".to_string()],
            });
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let old_contents = unsafe { ffi::PyLong_FromLong(100_200_300) };
            assert!(
                !old_contents.is_null(),
                "test old cell contents allocation should succeed"
            );
            let cell = unsafe { PyCell_New(old_contents) };
            assert!(!cell.is_null(), "test cell allocation should succeed");
            let replacement = unsafe { ffi::PyLong_FromLong(400_500_600) };
            assert!(
                !replacement.is_null(),
                "test replacement allocation should succeed"
            );
            let binding = LocalEnvResumeBinding {
                name: "cell".to_string(),
                location: cell_location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(cell_location),
                ownership: LocalRefKind::Borrowed,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![replacement.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_old_contents = unsafe { ffi::Py_REFCNT(old_contents) };
            let before_replacement = unsafe { ffi::Py_REFCNT(replacement) };
            let mut live_values = vec![cell.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                replacement.cast(),
                "owned-cell-store deopt should evaluate to the replacement value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful owned-cell-store deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(old_contents) },
                before_old_contents - 1,
                "owned-cell-store deopt should release the previous cell contents"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(replacement) },
                before_replacement + 2,
                "owned-cell-store deopt should return and store the replacement"
            );
            let contents = unsafe {
                ffi::PyObject_GetAttrString(cell.cast::<ffi::PyObject>(), c"cell_contents".as_ptr())
            };
            assert_eq!(
                contents, replacement,
                "owned-cell-store deopt should update the cell contents"
            );
            unsafe {
                ffi::Py_DECREF(contents);
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            }
            assert_eq!(
                unsafe { ffi::Py_REFCNT(replacement) },
                before_replacement + 1,
                "dropping the store result should leave only the cell-held replacement ref"
            );
            unsafe {
                ffi::Py_DECREF(cell);
                ffi::Py_DECREF(replacement);
                ffi::Py_DECREF(old_contents);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_return_owned_cell_delete() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let cell_location = LocalLocation(0);
            let mut function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(Del::new(test_owned_cell_name("cell", 0), false))),
            );
            function.storage_layout = Some(StorageLayout {
                freevars: vec![],
                cellvars: vec![ClosureSlot {
                    logical_name: "cell".to_string(),
                    storage_name: "cell".to_string(),
                    init: ClosureInit::Deferred,
                }],
                runtime_cells: vec![],
                stack_slots: vec!["cell".to_string()],
            });
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let cell_contents = unsafe { ffi::PyLong_FromLong(777_888_999) };
            assert!(
                !cell_contents.is_null(),
                "test cell contents allocation should succeed"
            );
            let cell = unsafe { PyCell_New(cell_contents) };
            assert!(!cell.is_null(), "test cell allocation should succeed");
            let binding = LocalEnvResumeBinding {
                name: "cell".to_string(),
                location: cell_location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(cell_location),
                ownership: LocalRefKind::Borrowed,
                value: None,
            };
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_contents = unsafe { ffi::Py_REFCNT(cell_contents) };
            let mut live_values = vec![cell.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "owned-cell-delete deopt should evaluate to None"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful owned-cell-delete deopt should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(cell_contents) },
                before_contents - 1,
                "owned-cell-delete deopt should release the deleted cell contents"
            );
            let contents = unsafe {
                ffi::PyObject_GetAttrString(cell.cast::<ffi::PyObject>(), c"cell_contents".as_ptr())
            };
            assert!(
                contents.is_null(),
                "owned-cell-delete deopt should leave the cell empty"
            );
            assert_ne!(
                unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) },
                0,
                "empty cell_contents access should raise ValueError"
            );
            unsafe {
                ffi::PyErr_Clear();
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(cell);
                ffi::Py_DECREF(cell_contents);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_raise_instance() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                BlockTerm::Raise(soac_core::block_py::TermRaise {
                    exc: Some(name_expr(test_constant_name(0))),
                }),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let exc = unsafe { ffi::PyObject_CallNoArgs(ffi::PyExc_ValueError) };
            assert!(
                !exc.is_null(),
                "test exception instance allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![exc.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_exc = unsafe { ffi::Py_REFCNT(exc) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                result.is_null(),
                "raise-instance deopt should return null to signal Python error"
            );
            assert_ne!(
                unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) },
                0,
                "raise-instance deopt should set the requested exception"
            );
            let raised = unsafe { ffi::PyErr_GetRaisedException() };
            assert!(
                !raised.is_null(),
                "raise-instance deopt should leave a raised exception object"
            );
            assert_eq!(
                raised, exc,
                "raise-instance deopt should raise the original exception instance"
            );
            unsafe {
                ffi::Py_DECREF(raised);
            }
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "fetching the raised exception should clear it"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(exc) },
                before_exc,
                "raise-instance deopt should not leak the module constant exception"
            );
            unsafe {
                ffi::Py_DECREF(exc);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_raise_class() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![],
                BlockTerm::Raise(soac_core::block_py::TermRaise {
                    exc: Some(name_expr(test_constant_name(0))),
                }),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let exc_class = unsafe { ffi::PyExc_ValueError };
            unsafe {
                ffi::Py_INCREF(exc_class);
            }
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![exc_class.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                result.is_null(),
                "raise-class deopt should return null to signal Python error"
            );
            assert_ne!(
                unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) },
                0,
                "raise-class deopt should set the requested exception"
            );
            let raised = unsafe { ffi::PyErr_GetRaisedException() };
            assert!(
                !raised.is_null(),
                "raise-class deopt should leave a normalized exception object"
            );
            assert_ne!(
                unsafe { ffi::PyExceptionInstance_Check(raised) },
                0,
                "raise-class deopt should normalize the exception class"
            );
            unsafe {
                ffi::Py_DECREF(raised);
                ffi::Py_DECREF(exc_class);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_bare_raise() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(test_function(), vec![], raise_term());
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let exc = unsafe { ffi::PyObject_CallNoArgs(ffi::PyExc_ValueError) };
            assert!(
                !exc.is_null(),
                "test current exception allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeTerm { function_id, block },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_exc = unsafe { ffi::Py_REFCNT(exc) };
            unsafe {
                ffi::Py_INCREF(exc);
                ffi::PyErr_SetRaisedException(exc);
            }
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert!(
                result.is_null(),
                "bare-raise deopt should return null to signal Python error"
            );
            assert_ne!(
                unsafe { ffi::PyErr_ExceptionMatches(ffi::PyExc_ValueError) },
                0,
                "bare-raise deopt should re-raise the active exception"
            );
            let raised = unsafe { ffi::PyErr_GetRaisedException() };
            assert!(
                !raised.is_null(),
                "bare-raise deopt should leave a raised exception object"
            );
            assert_eq!(
                raised, exc,
                "bare-raise deopt should preserve the active exception object"
            );
            unsafe {
                ffi::Py_DECREF(raised);
            }
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "fetching the raised exception should clear it"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(exc) },
                before_exc,
                "bare-raise deopt should not leak the active exception"
            );
            unsafe {
                ffi::Py_DECREF(exc);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_global_store() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![assign_stmt(
                    test_global_name("x"),
                    name_expr(test_constant_name(0)),
                )],
                ret_term(name_expr(test_global_name("x"))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let constant = unsafe { ffi::PyLong_FromLong(777_888_999) };
            assert!(
                !constant.is_null(),
                "test module constant allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![constant.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeInstr {
                        key: InstrKey::new(function_id, InstrId::new(block, 0)),
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let globals = unsafe { ffi::PyDict_New() };
            assert!(
                !globals.is_null(),
                "test globals dict allocation should succeed"
            );
            let before = unsafe { ffi::Py_REFCNT(constant) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    globals.cast(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                constant.cast(),
                "block-tail deopt should return the stored global value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful block-tail store deopt should not leave a Python exception"
            );
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test key allocation should succeed");
            let stored = unsafe { ffi::PyDict_GetItemWithError(globals, key) };
            assert_eq!(
                stored, constant,
                "block-tail deopt should store the module constant in globals"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "test globals lookup should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(constant) },
                before + 2,
                "global store and returned global load should each own one reference"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(globals);
                ffi::Py_DECREF(constant);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_global_delete() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let function = with_single_test_block(
                test_function(),
                vec![op_expr(Del::new(test_global_name("x"), false))],
                ret_term(none_expr()),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeInstr {
                        key: InstrKey::new(function_id, InstrId::new(block, 0)),
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let globals = unsafe { ffi::PyDict_New() };
            assert!(
                !globals.is_null(),
                "test globals dict allocation should succeed"
            );
            let key = unsafe { ffi::PyUnicode_FromString(c"x".as_ptr()) };
            assert!(!key.is_null(), "test key allocation should succeed");
            let value = unsafe { ffi::PyLong_FromLong(333_444_555) };
            assert!(!value.is_null(), "test value allocation should succeed");
            assert_eq!(
                unsafe { ffi::PyDict_SetItem(globals, key, value) },
                0,
                "test globals insertion should succeed"
            );
            let before = unsafe { ffi::Py_REFCNT(value) };
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    globals.cast(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "block-tail global delete deopt should continue to return None"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful block-tail delete deopt should not leave a Python exception"
            );
            assert!(
                unsafe { ffi::PyDict_GetItemWithError(globals, key) }.is_null(),
                "global delete replay should remove the key"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "deleted key lookup should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value) },
                before - 1,
                "global delete replay should drop the globals dict reference"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value);
                ffi::Py_DECREF(key);
                ffi::Py_DECREF(globals);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_local_store() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let location = LocalLocation(0);
            let function = with_single_test_block(
                test_function(),
                vec![assign_stmt(
                    test_name("x"),
                    name_expr(test_constant_name(0)),
                )],
                ret_term(name_expr(test_name("x"))),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let binding = LocalEnvResumeBinding {
                name: "x".to_string(),
                location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let old_value = unsafe { ffi::PyLong_FromLong(111_111_111) };
            assert!(
                !old_value.is_null(),
                "test old local allocation should succeed"
            );
            let new_value = unsafe { ffi::PyLong_FromLong(222_222_222) };
            assert!(
                !new_value.is_null(),
                "test module constant allocation should succeed"
            );
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: vec![new_value.cast()],
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeInstr {
                        key: InstrKey::new(function_id, InstrId::new(block, 0)),
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before_old = unsafe { ffi::Py_REFCNT(old_value) };
            let before_new = unsafe { ffi::Py_REFCNT(new_value) };
            unsafe {
                ffi::Py_INCREF(old_value);
            }
            let mut live_values = vec![old_value.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                new_value.cast(),
                "block-tail local-store deopt should return the rebound local value"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful local-store deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(old_value) },
                before_old,
                "local store replay should release the old frame-owned local"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(new_value) },
                before_new + 1,
                "returned rebound local should be owned by the JIT caller after frame cleanup"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(new_value);
                ffi::Py_DECREF(old_value);
            }
        });
    }

    #[test]
    fn deopt_block_tail_continuation_executes_local_delete() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let location = LocalLocation(0);
            let function = with_single_test_block(
                test_function(),
                vec![op_expr(Del::new(test_name("x"), false))],
                ret_term(none_expr()),
            );
            let function_id = function.function_id;
            let block = function.entry_block().label;
            let binding = LocalEnvResumeBinding {
                name: "x".to_string(),
                location,
                binding: LocalEnvResumeBindingState::Bound,
                source: LocalEnvResumeValueSource::BlockParam(location),
                ownership: LocalRefKind::Owned,
                value: None,
            };
            let value = unsafe { ffi::PyLong_FromLong(333_333_333) };
            assert!(!value.is_null(), "test local allocation should succeed");
            let table = RuntimeJitDeoptTable {
                function_id,
                function: Box::new(function),
                module_constant_ptrs: Vec::new(),
                points: vec![RuntimeJitDeoptRecord {
                    id: PlannedJitDeoptPointId {
                        function_id,
                        ordinal: 0,
                    },
                    resume_point: LocalEnvResumePoint::BeforeInstr {
                        key: InstrKey::new(function_id, InstrId::new(block, 0)),
                    },
                    precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                    locals: vec![binding],
                    continuation: RuntimeJitDeoptContinuation::ResumeBlockTail {
                        cursor: RuntimeJitDeoptCursor::at_block_entry(block),
                    },
                }],
            };
            let before = unsafe { ffi::Py_REFCNT(value) };
            unsafe {
                ffi::Py_INCREF(value);
            }
            let mut live_values = vec![value.cast::<c_void>()];
            let result = unsafe {
                test_dp_jit_deopt_resume(
                    std::ptr::addr_of!(table).cast_mut().cast(),
                    std::ptr::null_mut(),
                    0,
                    live_values.as_mut_ptr().cast(),
                    live_values.len() as i64,
                )
            };
            assert_eq!(
                result,
                unsafe { ffi::Py_None() }.cast(),
                "block-tail local-delete deopt should continue to return None"
            );
            assert!(
                unsafe { ffi::PyErr_Occurred() }.is_null(),
                "successful local-delete deopt continuation should not leave a Python exception"
            );
            assert_eq!(
                unsafe { ffi::Py_REFCNT(value) },
                before,
                "local delete replay should release the frame-owned local"
            );
            unsafe {
                ffi::Py_DECREF(result.cast::<ffi::PyObject>());
                ffi::Py_DECREF(value);
            }
        });
    }

    #[test]
    fn deopt_resume_call_uses_function_env_deopt_table_and_ordinal() {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();
        let mut signature = jit_module.make_signature();
        signature.params.push(ir::AbiParam::new(ptr_ty));
        signature.returns.push(ir::AbiParam::new(ptr_ty));
        let wrapper_id = declare_local_fn(&mut jit_module, "test_deopt_exit_call", &signature)
            .expect("test wrapper should declare");
        let mut module_imports = ModuleFuncImports::new();
        let mut ctx = jit_module.make_context();
        ctx.func.name = ir::UserFuncName::user(0, wrapper_id.as_u32());
        ctx.func.signature = signature;

        let mut builder_ctx = FunctionBuilderContext::new();
        {
            let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
            let entry = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.switch_to_block(entry);
            fb.seal_block(entry);
            let mut func_imports = FuncBuildImports::new(&mut module_imports);
            let deopt_ref = func_imports.get_or_panic(
                &mut jit_module,
                &mut fb.func,
                &DP_JIT_DEOPT_RESUME_IMPORT,
            );
            let function_env_value = fb.block_params(entry)[0];
            let globals_obj = fb.ins().iconst(ptr_ty, 0);
            let live_values = fb.ins().iconst(ptr_ty, 0);
            let result = emit_deopt_resume_call(
                &mut fb,
                JitDeoptExitRef {
                    function_env_value,
                    record_ordinal: 42,
                },
                deopt_ref,
                globals_obj,
                live_values,
                0,
                ptr_ty,
                ir::types::I64,
            );
            fb.ins().return_(&[result]);
            fb.finalize();
        }

        let deopt_import = module_imports
            .debug_symbols()
            .iter()
            .find_map(|(import_id, symbol)| {
                (*symbol == "dp_jit_deopt_resume").then(|| ir::UserExternalName::new(0, *import_id))
            })
            .expect("deopt helper import should be declared");
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&ctx.func, &[deopt_import]),
            1,
            "deopt exit call should target the runtime deopt helper"
        );
        assert!(
            function_contains_iconst_imm(&ctx.func, 42),
            "deopt exit call should materialize the planned record ordinal"
        );
        assert!(
            ctx.func.layout.blocks().any(|block| {
                ctx.func.layout.block_insts(block).any(|inst| {
                    matches!(
                        ctx.func.dfg.insts[inst].load_store_offset(),
                        Some(offset) if offset == FUNCTION_ENV_DEOPT_TABLE_PTR_OFFSET
                    )
                })
            }),
            "deopt exit call should load the deopt table pointer from the function env"
        );
    }

    struct EnvVarGuard {
        name: &'static str,
        old_value: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let old_value = std::env::var_os(name);
            unsafe { std::env::set_var(name, value) };
            Self { name, old_value }
        }

        fn set_os(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let old_value = std::env::var_os(name);
            unsafe { std::env::set_var(name, value) };
            Self { name, old_value }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.old_value.as_ref() {
                Some(value) => unsafe { std::env::set_var(self.name, value) },
                None => unsafe { std::env::remove_var(self.name) },
            }
        }
    }

    fn set_opt_mode(mode: &str) -> EnvVarGuard {
        EnvVarGuard::set("SOAC_OPT_MODE", mode)
    }

    unsafe fn build_runtime_refcount_smoke_context() -> (
        crate::session::CompileSession,
        JITModule,
        cranelift_codegen::Context,
        FuncId,
        [ir::UserExternalName; 2],
    ) {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));
        let mut decref_signature = jit_module.make_signature();
        decref_signature.params.push(ir::AbiParam::new(ptr_ty));
        decref_signature.params.push(ir::AbiParam::new(ptr_ty));

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
            &decref_signature,
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
            let null_tstate = fb.ins().iconst(ptr_ty, 0);
            fb.ins().call(incref_ref, &[arg]);
            fb.ins().call(decref_ref, &[null_tstate, arg]);
            fb.ins().return_(&[arg]);
            fb.finalize();
        }

        (
            compile_session,
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
        let (_compile_session, mut jit_module, mut ctx, wrapper_id, _) =
            build_runtime_refcount_smoke_context();

        define_prepared_function(
            &mut jit_module,
            &SoacEnvConfig::default(),
            wrapper_id,
            &mut ctx,
            "test-runtime-refcount-smoke-wrapper",
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

    unsafe fn build_runtime_decref_wrapper()
    -> unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));
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
            let tstate = fb.block_params(entry)[0];
            let arg = fb.block_params(entry)[1];
            fb.ins().call(decref_ref, &[tstate, arg]);
            fb.ins().return_(&[]);
            fb.finalize();
        }

        define_prepared_function(
            &mut jit_module,
            &SoacEnvConfig::default(),
            wrapper_id,
            &mut ctx,
            "test-runtime-refcount-decref-wrapper",
            "test wrapper function should define",
        )
        .expect("wrapper function should compile");
        jit_module.clear_context(&mut ctx);
        jit_module
            .finalize_definitions()
            .expect("jit module should finalize");

        let code_ptr = jit_module.get_finalized_function(wrapper_id);
        let compiled: unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) =
            std::mem::transmute(code_ptr);
        Box::leak(Box::new(jit_module));
        compiled
    }

    unsafe fn build_counted_runtime_incref_wrapper() -> (
        unsafe extern "C" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void,
        *const u64,
    ) {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let scalar_counter_data_id = define_scalar_counter_storage_data_for_symbol(
            &mut jit_module,
            "test_counted_runtime_incref_counter",
            1,
        )
        .expect("scalar counter storage should define");
        let counted_incref_id = build_counted_runtime_refcount_helper(
            &mut jit_module,
            &SoacEnvConfig::default(),
            "test_counted_runtime_incref",
            "test-counted-runtime-incref",
            &DP_JIT_INCREF_IMPORT,
            &SOAC_RUNTIME_INCREF_APPLIED_IMPORT,
            scalar_counter_data_id,
            0,
        )
        .expect("counted incref helper should build");

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));
        wrapper_signature.returns.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id = declare_local_fn(
            &mut jit_module,
            "jit_counted_runtime_incref_wrapper",
            &wrapper_signature,
        )
        .expect("wrapper function should declare");

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

            let counted_incref_ref =
                jit_module.declare_func_in_func(counted_incref_id, &mut fb.func);
            let arg = fb.block_params(entry)[0];
            fb.ins().call(counted_incref_ref, &[arg]);
            fb.ins().return_(&[arg]);
            fb.finalize();
        }

        define_prepared_function(
            &mut jit_module,
            &SoacEnvConfig::default(),
            wrapper_id,
            &mut ctx,
            "test-counted-runtime-incref-wrapper",
            "counted incref wrapper should define",
        )
        .expect("wrapper function should compile");
        jit_module.clear_context(&mut ctx);
        jit_module
            .finalize_definitions()
            .expect("jit module should finalize");

        let code_ptr = jit_module.get_finalized_function(wrapper_id);
        let compiled: unsafe extern "C" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void =
            std::mem::transmute(code_ptr);
        let (counter_ptr, counter_size) = jit_module.get_finalized_data(scalar_counter_data_id);
        assert_eq!(
            counter_size,
            std::mem::size_of::<u64>(),
            "counted incref test should expose exactly one scalar counter"
        );
        let counter_ptr = counter_ptr.cast::<u64>();

        Box::leak(Box::new(jit_module));
        (compiled, counter_ptr)
    }

    unsafe fn build_counted_runtime_decref_wrapper() -> (
        unsafe extern "C" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void,
        *const u64,
    ) {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let scalar_counter_data_id = define_scalar_counter_storage_data_for_symbol(
            &mut jit_module,
            "test_counted_runtime_decref_counter",
            1,
        )
        .expect("scalar counter storage should define");
        let counted_decref_id = build_counted_runtime_refcount_helper(
            &mut jit_module,
            &SoacEnvConfig::default(),
            "test_counted_runtime_decref",
            "test-counted-runtime-decref",
            &DP_JIT_DECREF_IMPORT,
            &SOAC_RUNTIME_DECREF_APPLIED_IMPORT,
            scalar_counter_data_id,
            0,
        )
        .expect("counted decref helper should build");

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));
        wrapper_signature.returns.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id = declare_local_fn(
            &mut jit_module,
            "jit_counted_runtime_decref_wrapper",
            &wrapper_signature,
        )
        .expect("wrapper function should declare");

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

            let counted_decref_ref =
                jit_module.declare_func_in_func(counted_decref_id, &mut fb.func);
            let arg = fb.block_params(entry)[0];
            let null_tstate = fb.ins().iconst(ptr_ty, 0);
            fb.ins().call(counted_decref_ref, &[null_tstate, arg]);
            fb.ins().return_(&[arg]);
            fb.finalize();
        }

        define_prepared_function(
            &mut jit_module,
            &SoacEnvConfig::default(),
            wrapper_id,
            &mut ctx,
            "test-counted-runtime-decref-wrapper",
            "counted decref wrapper should define",
        )
        .expect("wrapper function should compile");
        jit_module.clear_context(&mut ctx);
        jit_module
            .finalize_definitions()
            .expect("jit module should finalize");

        let code_ptr = jit_module.get_finalized_function(wrapper_id);
        let compiled: unsafe extern "C" fn(*mut std::ffi::c_void) -> *mut std::ffi::c_void =
            std::mem::transmute(code_ptr);
        let (counter_ptr, counter_size) = jit_module.get_finalized_data(scalar_counter_data_id);
        assert_eq!(
            counter_size,
            std::mem::size_of::<u64>(),
            "counted decref test should expose exactly one scalar counter"
        );
        let counter_ptr = counter_ptr.cast::<u64>();

        Box::leak(Box::new(jit_module));
        (compiled, counter_ptr)
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
        let (_compile_session, mut jit_module, mut ctx, _wrapper_id, helper_names) =
            unsafe { build_runtime_refcount_smoke_context() };
        let before = count_direct_calls_to_runtime_helpers(&ctx.func, &helper_names);
        assert_eq!(
            before, 2,
            "test caller should start with direct incref/decref calls"
        );

        let inlined = inline_runtime_support_calls(
            &mut jit_module,
            &SoacEnvConfig::default(),
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
    fn runtime_support_inliner_uses_noop_refcount_helpers_when_disabled() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "runtime_support_inliner_uses_noop_refcount_helpers_when_disabled",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let disabled_insts = {
            let (_compile_session, mut jit_module, mut ctx, _wrapper_id, helper_names) =
                unsafe { build_runtime_refcount_smoke_context() };
            let before_calls = count_direct_calls_to_runtime_helpers(&ctx.func, &helper_names);
            assert_eq!(
                before_calls, 2,
                "test caller should start with direct incref/decref calls"
            );

            let inlined = inline_runtime_support_calls(
                &mut jit_module,
                &SoacEnvConfig::default().with_jit_refcount_emission_enabled(false),
                &mut ctx,
                "test runtime support inliner should run with refcounts disabled",
            )
            .expect("runtime support inliner should succeed");
            let after_calls = count_direct_calls_to_runtime_helpers(&ctx.func, &helper_names);
            assert!(
                inlined,
                "runtime support inliner should report at least one inlined call"
            );
            assert_eq!(
                after_calls, 0,
                "disabled refcount emission should remove direct helper calls"
            );
            ctx.func.dfg.num_insts()
        };

        let enabled_insts = {
            let (_compile_session, mut jit_module, mut ctx, _wrapper_id, _helper_names) =
                unsafe { build_runtime_refcount_smoke_context() };
            inline_runtime_support_calls(
                &mut jit_module,
                &SoacEnvConfig::default().with_jit_refcount_emission_enabled(true),
                &mut ctx,
                "test runtime support inliner should run with refcounts enabled",
            )
            .expect("runtime support inliner should succeed");
            ctx.func.dfg.num_insts()
        };
        assert!(
            disabled_insts < enabled_insts,
            "disabled refcount emission should inline smaller no-op helpers; disabled={disabled_insts} enabled={enabled_insts}"
        );
    }

    #[test]
    fn runtime_clif_example_can_specialize_known_value_call_to_constant() {
        let source = parsed_runtime_clif_function("soac_runtime_example_known_value_source");
        assert!(
            source.function.dfg.num_insts() > 0,
            "example source helper should load from soac_jit_runtime CLIF as an ir::Function"
        );
        let parsed = parsed_runtime_clif_function("soac_runtime_example_offset_known_value");
        let callee_name = single_direct_call_callee_name(&parsed.function);

        assert_eq!(
            count_direct_calls_to_runtime_helpers(
                &parsed.function,
                std::slice::from_ref(&callee_name)
            ),
            1,
            "loaded runtime CLIF example should start with one helper call"
        );
        assert!(
            function_contains_iconst_imm(&parsed.function, 5),
            "loaded runtime CLIF example should preserve the fixed offset constant"
        );

        let specialized_true =
            specialize_runtime_i64_call_to_constant(&parsed.function, &callee_name, 7);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(
                &specialized_true,
                std::slice::from_ref(&callee_name)
            ),
            0,
            "known-value specialization should remove the helper call from the cloned IR"
        );
        assert!(
            function_contains_iconst_imm(&specialized_true, 7),
            "specialized clone should materialize the known helper result as an iconst"
        );
        let specialized_true = optimize_test_ir_function(specialized_true);
        assert!(
            function_contains_iconst_imm(&specialized_true, 12),
            "optimizing the specialized seven clone should fold the helper result plus offset:\n{}",
            specialized_true.display()
        );

        let specialized_false =
            specialize_runtime_i64_call_to_constant(&parsed.function, &callee_name, 9);
        let specialized_false = optimize_test_ir_function(specialized_false);
        assert!(
            function_contains_iconst_imm(&specialized_false, 14),
            "optimizing the specialized nine clone should fold the helper result plus offset:\n{}",
            specialized_false.display()
        );
    }

    #[test]
    fn runtime_clif_builtin_primitive_symbols_are_available() {
        let ord = parsed_runtime_clif_function(direct_abi::SOAC_RUNTIME_BUILTIN_ORD_I64_SYMBOL);
        assert_eq!(ord.function.signature.params.len(), 2);
        assert_eq!(ord.function.signature.returns.len(), 1);
        assert_eq!(ord.function.signature.returns[0].value_type, ir::types::I64);

        let chr = parsed_runtime_clif_function(direct_abi::SOAC_RUNTIME_BUILTIN_CHR_I64_SYMBOL);
        assert_eq!(chr.function.signature.params.len(), 2);
        assert_eq!(chr.function.signature.returns.len(), 1);
        assert_eq!(chr.function.signature.returns[0].value_type, ir::types::I64);

        let len = parsed_runtime_clif_function(direct_abi::SOAC_RUNTIME_BUILTIN_LEN_I64_SYMBOL);
        assert_eq!(len.function.signature.params.len(), 2);
        assert_eq!(len.function.signature.returns.len(), 1);
        assert_eq!(len.function.signature.returns[0].value_type, ir::types::I64);

        let pylong_as_i64 = parsed_runtime_clif_function(SOAC_RUNTIME_PYLONG_AS_I64_SYMBOL);
        assert_eq!(pylong_as_i64.function.signature.params.len(), 2);
        assert_eq!(pylong_as_i64.function.signature.returns.len(), 1);
        assert_eq!(
            pylong_as_i64.function.signature.returns[0].value_type,
            ir::types::I64
        );

        let pylong_as_i64_saturating =
            parsed_runtime_clif_function(SOAC_RUNTIME_PYLONG_AS_I64_SATURATING_SYMBOL);
        assert_eq!(pylong_as_i64_saturating.function.signature.params.len(), 2);
        assert_eq!(pylong_as_i64_saturating.function.signature.returns.len(), 1);
        assert_eq!(
            pylong_as_i64_saturating.function.signature.returns[0].value_type,
            ir::types::I64
        );
    }

    #[test]
    fn jit_runtime_clif_refcount_roundtrip_preserves_py_long_refcount() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            let wrapper = build_runtime_refcount_smoke_wrapper();
            crate::initialize_test_python();
            Python::attach(|_| {
                let obj = ffi::PyCapsule_New(
                    std::ptr::dangling_mut::<c_void>(),
                    c"soac.runtime.counted_incref".as_ptr(),
                    None,
                );
                assert!(!obj.is_null(), "PyCapsule_New should produce a test object");
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
            crate::initialize_test_python();
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

                let tstate = PyThreadState_GetUnchecked();
                wrapper(tstate.cast(), capsule.cast());
                let after = ffi::Py_REFCNT(capsule);

                assert!(
                    CAPSULE_DESTROYED.load(Ordering::SeqCst),
                    "runtime CLIF decref should drive PyCapsule destruction through _Py_Dealloc; refcnt after wrapper = {after}"
                );
            });
        }
    }

    #[test]
    fn jit_counted_runtime_incref_counter_tracks_only_applied_refcount_ops() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            let (wrapper, counter_ptr) = build_counted_runtime_incref_wrapper();
            crate::initialize_test_python();
            Python::attach(|_| {
                let obj = ffi::PyCapsule_New(
                    std::ptr::dangling_mut::<c_void>(),
                    c"soac.runtime.counted_decref".as_ptr(),
                    None,
                );
                assert!(!obj.is_null(), "PyCapsule_New should produce a test object");

                assert_eq!(
                    *counter_ptr, 0,
                    "counted incref helper should start with a zeroed scalar counter"
                );
                let result = wrapper(obj.cast());
                assert_eq!(
                    result,
                    obj.cast(),
                    "wrapper should preserve the input pointer"
                );
                assert_eq!(
                    *counter_ptr, 1,
                    "heap object incref should increment the counter"
                );

                let none = ffi::Py_None();
                let none_result = wrapper(none.cast());
                assert_eq!(none_result, none.cast(), "wrapper should preserve Py_None");
                assert_eq!(
                    *counter_ptr, 1,
                    "immortal skipped incref should not increment the applied counter"
                );

                ffi::Py_DECREF(obj);
                ffi::Py_DECREF(obj);
            });
        }
    }

    #[test]
    fn jit_counted_runtime_decref_counter_tracks_only_applied_refcount_ops() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            let (wrapper, counter_ptr) = build_counted_runtime_decref_wrapper();
            crate::initialize_test_python();
            Python::attach(|_| {
                let obj = ffi::PyCapsule_New(
                    std::ptr::dangling_mut::<c_void>(),
                    c"soac.runtime.counted_decref".as_ptr(),
                    None,
                );
                assert!(!obj.is_null(), "PyCapsule_New should produce a test object");
                ffi::Py_INCREF(obj);

                assert_eq!(
                    *counter_ptr, 0,
                    "counted decref helper should start with a zeroed scalar counter"
                );
                let result = wrapper(obj.cast());
                assert_eq!(
                    result,
                    obj.cast(),
                    "wrapper should preserve the input pointer"
                );
                assert_eq!(
                    *counter_ptr, 1,
                    "heap object decref should increment the counter"
                );

                let none = ffi::Py_None();
                let none_result = wrapper(none.cast());
                assert_eq!(none_result, none.cast(), "wrapper should preserve Py_None");
                assert_eq!(
                    *counter_ptr, 1,
                    "immortal skipped decref should not increment the applied counter"
                );

                ffi::Py_DECREF(obj);
            });
        }
    }

    #[test]
    fn jit_vectorcall_trampoline_can_link_runtime_decref_clif() {
        let compile_session = crate::session::CompileSession::new();
        let engine =
            ProcessJitEngine::new(&compile_session).expect("process jit engine should construct");
        engine
            .vectorcall_trampoline(&compile_session, 0)
            .expect("vectorcall trampoline should link runtime CLIF refcount helpers");
    }

    #[test]
    fn jit_block_entry_counter_updates_shared_state() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            crate::initialize_test_python();
            Python::attach(|py| {
                let mut lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                    r#"
def f():
    return None
"#,
                )
                .expect("lowering should succeed")
                .codegen_module;
                instrument_module_with_legacy_block_entry_counters(&mut lowered);

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
                let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
                let compile_session = crate::session::CompileSession::process();
                let compiled_handle = compile_cranelift_run_bb_specialized_cached(
                    &compile_session,
                    &blocks,
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(shared_state.as_ref()),
                )
                .expect("direct counter test function should compile");
                let (code_ptr, _default_code_ptr, param_count) = compiled_handle
                    .handle
                    .direct_runner_info()
                    .expect("compiled direct runner should expose entrypoint");
                assert_eq!(param_count, 0, "test function should not take direct args");
                let entry: unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
                    std::mem::transmute(code_ptr);
                let mut function_context =
                    test_function_jit_context(&runtime, std::ptr::null_mut());
                let thread_state = ffi::PyThreadState_Get().cast::<c_void>();

                let result1 = entry(
                    std::ptr::addr_of_mut!(function_context).cast(),
                    thread_state,
                );
                let result2 = entry(
                    std::ptr::addr_of_mut!(function_context).cast(),
                    thread_state,
                );

                assert_eq!(
                    shared_state.counter_value(entry_counter_id),
                    2,
                    "entry counter should reflect the number of completed direct JIT calls"
                );

                ffi::Py_DECREF(result1.cast());
                ffi::Py_DECREF(result2.cast());
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
        let rendered = render_test_jit_function_with_runtime_inline(&function, &blocks, Vec::new());
        assert!(
            rendered.contains("function"),
            "specialized JIT CLIF render should produce function text:\n{}",
            rendered
        );
    }

    #[test]
    fn specialized_jit_direct_entry_has_fn_env_and_tstate_params() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = test_function();
        set_stack_slots(&mut function, &["current", "acc"]);
        let mut source = test_source_block(&function, vec![], ret_term(constants.int_expr(7)));
        source.ensure_param("current", BlockParamRole::AbruptKind);
        source.ensure_param("acc", BlockParamRole::AbruptPayload);
        let function = with_test_blocks(function, vec![source]);
        let module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function.clone()],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        unsafe {
            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = placeholder_module_constant_ptrs(module_constants.len());
            let module_constant_object_data_ids = declare_module_constant_object_data(
                &mut jit_module,
                &module,
                &module_constant_ptrs,
            )
            .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    &module,
                    module.counter_defs.as_slice(),
                );
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                &blocks,
                &module,
                &function,
                &module_constants,
                module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                &compile_session,
                None,
                None,
                None,
                BuildSpecializedFunctionOptions::default(),
            )
            .expect("specialized JIT build should succeed");
            assert_eq!(
                built
                    .ctx
                    .func
                    .signature
                    .params
                    .iter()
                    .map(|param| param.value_type)
                    .collect::<Vec<_>>(),
                vec![ir::types::I64, ir::types::I64],
                "direct JIT entry should take only fn_env and tstate as hidden params"
            );
            let entry_block = built
                .ctx
                .func
                .layout
                .entry_block()
                .expect("direct JIT function should have an entry block");
            assert_eq!(
                built.ctx.func.dfg.block_params(entry_block).len(),
                2,
                "entry block params should match the hidden direct-entry ABI"
            );
        }
    }

    #[test]
    fn render_specialized_jit_exception_dispatch_takes_raised_exception_directly() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def f():
    try:
        raise ValueError("boom")
    except ValueError:
        return 1
    return 0
"#,
        )
        .expect("lowering try/except test source should succeed")
        .codegen_module;

        let codegen_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&lowered);
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "f")
            .expect("missing lowered function f")
            .clone();
        let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
        let rendered = render_test_jit_function_with_constants(
            &lowered,
            &function,
            &blocks,
            &codegen_constants,
        );

        assert!(
            !rendered.contains("dp_jit_get_raised_exception"),
            "exception dispatch should no longer import/call the raised-exception helper:\n{rendered}"
        );
    }

    #[test]
    fn specialized_jit_try_finally_return_payload_builds_with_refcount_cleanup() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
events = []

def f(mode):
    try:
        if mode == "ret":
            return 10
        if mode == "raise":
            raise ValueError("boom")
        events.append("body")
    except ValueError:
        events.append("except")
    else:
        events.append("else")
    finally:
        events.append("finally")
    return 20
"#,
        )
        .expect("lowering try/except/else/finally source should succeed")
        .codegen_module;

        let codegen_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&lowered);
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "f")
            .expect("missing lowered function f")
            .clone();
        let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
        build_test_specialized_function(&blocks, &lowered, &function, &codegen_constants);
    }

    #[test]
    fn specialized_jit_with_return_payload_failure_cleanup_forwards_stack_slot() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
from pathlib import Path

class Wrapper:
    def __init__(self, path: Path) -> None:
        self.path = path

    def open(self, mode: str = "r", *, encoding: str = "utf8"):
        path = self.path
        return open(path, mode, encoding=encoding)

def write_and_read(path: Path) -> str:
    wrapper = Wrapper(path)
    with wrapper.open("w", encoding="utf8") as handle:
        handle.write("payload")
    with wrapper.open("r", encoding="utf8") as handle:
        return handle.read()
"#,
        )
        .expect("lowering method_named_open source should succeed")
        .codegen_module;

        let codegen_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&lowered);
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "write_and_read")
            .expect("missing lowered function write_and_read")
            .clone();
        let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
        build_test_specialized_function(&blocks, &lowered, &function, &codegen_constants);
    }

    #[test]
    fn specialized_jit_except_star_failure_cleanup_forwards_unbound_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def run():
    global caught
    ok = False
    try:
        raise ExceptionGroup("eg", [ValueError("boom")])
    except* ValueError as caught:
        value = caught
        ok = isinstance(value, ExceptionGroup)
    return ok
"#,
        )
        .expect("lowering except star source should succeed")
        .codegen_module;

        let codegen_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&lowered);
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "run")
            .expect("missing lowered function run")
            .clone();
        let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
        build_test_specialized_function(&blocks, &lowered, &function, &codegen_constants);
    }

    fn assert_exception_dispatch_forwards_live_local(
        source: &str,
        source_block_matches: impl Fn(&CodegenBlock) -> bool,
    ) {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("lowering try/except local-forwarding test source should succeed")
            .codegen_module;

        let codegen_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&lowered);
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.bind_name == "f")
            .expect("missing lowered function f")
            .clone();
        let source_label = function
            .blocks
            .iter()
            .find(|block| source_block_matches(block))
            .map(|block| block.label)
            .expect("expected matching exception edge source block");
        let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
        {
            let built =
                build_test_specialized_function(&blocks, &lowered, &function, &codegen_constants);

            let expected_name = format!("exc_dispatch::{source_label}");
            let (dispatch_block_name, dispatch_annotation) = built
                .block_annotations
                .iter()
                .find(|(_, annotation)| annotation.semantic_name == expected_name)
                .expect("missing exception dispatch block annotation");
            assert_eq!(
                dispatch_annotation.param_names,
                vec!["x".to_string()],
                "exception dispatch annotation should carry forwarded local names"
            );

            let dispatch_block = built
                .ctx
                .func
                .layout
                .blocks()
                .find(|block| block.to_string() == *dispatch_block_name)
                .expect("annotated exception dispatch block should exist in CLIF");
            assert_eq!(
                built.ctx.func.dfg.block_params(dispatch_block).len(),
                1,
                "exception dispatch block should take the forwarded local as a block param"
            );
        }
    }

    #[test]
    fn render_specialized_jit_exception_dispatch_forwards_live_locals_from_call_failure() {
        assert_exception_dispatch_forwards_live_local(
            r#"
def f(x):
    try:
        int("bad")
    except ValueError:
        return x
    return 0
"#,
            |block| {
                block.exc_edge.is_some()
                    && block
                        .body
                        .iter()
                        .any(|instr| matches!(instr, InstrCodegen::Call(_)))
            },
        );
    }

    #[test]
    fn render_specialized_jit_exception_dispatch_forwards_live_locals_from_explicit_raise() {
        assert_exception_dispatch_forwards_live_local(
            r#"
def f(x):
    try:
        raise ValueError("boom")
    except ValueError:
        return x
    return 0
"#,
            |block| block.exc_edge.is_some() && matches!(block.term, BlockTerm::Raise(_)),
        );
    }

    #[test]
    fn render_specialized_jit_operator_calls_use_python_capi() {
        let blocks = [1usize as ObjPtr];
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(BinOp::new(
                BinOpKind::Add,
                name_expr(test_name("a")),
                name_expr(test_local_name("b", 1)),
            ))),
        );
        set_stack_slots(&mut function, &["a", "b"]);
        let rendered = render_test_jit_function_with_module_constants(&function, &blocks, vec![]);
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
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(BinOp::new(
                BinOpKind::Lt,
                name_expr(test_name("a")),
                name_expr(test_local_name("b", 1)),
            ))),
        );
        set_stack_slots(&mut function, &["a", "b"]);
        let rendered = render_test_jit_function_with_module_constants(&function, &blocks, vec![]);
        assert!(
            rendered.contains("call PyObject_RichCompare"),
            "comparison lowering should use PyObject_RichCompare in rendered CLIF:\n{rendered}"
        );
    }

    #[test]
    fn specialized_jit_opt_v3_exact_int_branch_artifact_emits_machine_path() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialized_jit_opt_v3_exact_int_branch_artifact_emits_machine_path",
        ) {
            return;
        }
        let blocks = [1usize as ObjPtr, 2usize as ObjPtr, 3usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = test_function();
        function.params = ParamSpec {
            params: vec![
                Param {
                    name: "a".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
                Param {
                    name: "b".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
            ],
        };
        let entry_label = function.name_gen.next_block_name();
        let then_label = function.name_gen.next_block_name();
        let else_label = function.name_gen.next_block_name();
        let add_instr_id = InstrId::new(entry_label, 2);
        let compare_instr_id = InstrId::new(entry_label, 4);
        let entry = CodegenBlock {
            label: entry_label,
            body: vec![],
            term: BlockTerm::IfTerm(soac_core::block_py::TermIf {
                test: with_instr_id(
                    op_expr(BinOp::new(
                        BinOpKind::Gt,
                        with_instr_id(
                            op_expr(BinOp::new(
                                BinOpKind::Add,
                                with_instr_id(
                                    name_expr(test_name("a")),
                                    InstrId::new(entry_label, 0),
                                ),
                                with_instr_id(
                                    name_expr(test_local_name("b", 1)),
                                    InstrId::new(entry_label, 1),
                                ),
                            )),
                            add_instr_id,
                        ),
                        with_instr_id(constants.int_expr(0), InstrId::new(entry_label, 3)),
                    )),
                    compare_instr_id,
                ),
                then_label,
                else_label,
            }),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let then_block = CodegenBlock {
            label: then_label,
            body: vec![],
            term: ret_term(constants.int_expr(1)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let else_block = CodegenBlock {
            label: else_label,
            body: vec![],
            term: ret_term(constants.int_expr(0)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        function.blocks = vec![entry, then_block, else_block];
        set_stack_slots(&mut function, &["a", "b"]);

        let mut module = test_module(ModuleNameGen::new(0), vec![function]);
        module.module_constants = constants.module_constants;
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let exact_int_shape = soac_opt::operator_specialization::pack_binary_shape(
            soac_opt::operator_specialization::ExactTypeTag::Int,
            soac_opt::operator_specialization::ExactTypeTag::Int,
        );
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .operator_specializations
            .insert(add_instr_id, vec![exact_int_shape]);
        let artifacts = plan_and_emit_function_exact_int_branches_v3_with_module_constants(
            &AlternativeCatalog::default_v3(),
            ModulePlanIdentity {
                module_name: "test".to_string(),
                source_hash: 0,
                cache_identity: "test-cache".to_string(),
            },
            FunctionPlanIdentity {
                function: SerializedFunctionId::new(
                    SerializedModuleId::new(0),
                    function.function_id.local_function_id(),
                ),
                debug_name: Some(function.names.qualname.clone()),
            },
            &function,
            &evidence,
            module.module_constants.as_slice(),
        )
        .unwrap();
        assert_eq!(artifacts.emission.functions[0].regions.len(), 2);

        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                specialization_inputs: Some(FunctionSpecializationInputs {
                    legacy_overlays: Some(LegacyFunctionSpecializationOverlays {
                        exact_int_branch_artifacts: Some(std::sync::Arc::new(artifacts)),
                        ..LegacyFunctionSpecializationOverlays::default()
                    }),
                }),
                ..BuildSpecializedFunctionOptions::default()
            },
        );

        assert_eq!(
            count_opcode(&built.ctx.func, ir::Opcode::SaddOverflow),
            1,
            "v3 exact-int branch artifact should emit the selected checked machine add"
        );
        let generic_helpers = import_user_names_for_symbols(
            &built,
            &[
                "PyNumber_Add",
                "PyObject_RichCompare",
                "dp_jit_is_true",
                "PyLong_FromLongLong",
            ],
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &generic_helpers),
            4,
            "v3 exact-int branch should keep a local generic fallback region"
        );
    }

    #[test]
    fn specialized_jit_opt_v3_add_store_then_compare_constant_emits_machine_paths() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialized_jit_opt_v3_add_store_then_compare_constant_emits_machine_paths",
        ) {
            return;
        }
        let blocks = [
            1usize as ObjPtr,
            2usize as ObjPtr,
            3usize as ObjPtr,
            4usize as ObjPtr,
        ];
        let mut constants = TestConstantPool::default();
        let mut function = test_function();
        function.params = ParamSpec {
            params: vec![
                Param {
                    name: "a".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
                Param {
                    name: "b".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
            ],
        };
        let entry_label = function.name_gen.next_block_name();
        let test_label = function.name_gen.next_block_name();
        let then_label = function.name_gen.next_block_name();
        let else_label = function.name_gen.next_block_name();
        let store_instr_id = InstrId::new(entry_label, 0);
        let add_instr_id = InstrId::new(entry_label, 1);
        let compare_instr_id = InstrId::new(test_label, 0);
        let c_name = test_local_name("c", 2);
        let entry = CodegenBlock {
            label: entry_label,
            body: vec![with_instr_id(
                op_expr(Store::new(
                    c_name.clone(),
                    with_instr_id(
                        op_expr(BinOp::new(
                            BinOpKind::Add,
                            name_expr(test_name("a")),
                            name_expr(test_local_name("b", 1)),
                        )),
                        add_instr_id,
                    ),
                )),
                store_instr_id,
            )],
            term: BlockTerm::Jump(BlockEdge::new(test_label)),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let test_block = CodegenBlock {
            label: test_label,
            body: vec![],
            term: BlockTerm::IfTerm(soac_core::block_py::TermIf {
                test: with_instr_id(
                    op_expr(BinOp::new(
                        BinOpKind::Gt,
                        name_expr(c_name),
                        constants.int_expr(0),
                    )),
                    compare_instr_id,
                ),
                then_label,
                else_label,
            }),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let then_block = CodegenBlock {
            label: then_label,
            body: vec![],
            term: ret_term(name_expr(test_runtime_name("TRUE"))),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        let else_block = CodegenBlock {
            label: else_label,
            body: vec![],
            term: ret_term(none_expr()),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        };
        function.blocks = vec![entry, test_block, then_block, else_block];
        set_stack_slots(&mut function, &["a", "b", "c"]);

        let mut module = test_module(ModuleNameGen::new(0), vec![function]);
        module.module_constants = constants.module_constants;
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let exact_int_shape = soac_opt::operator_specialization::pack_binary_shape(
            soac_opt::operator_specialization::ExactTypeTag::Int,
            soac_opt::operator_specialization::ExactTypeTag::Int,
        );
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .operator_specializations
            .insert(add_instr_id, vec![exact_int_shape]);
        evidence
            .operator_specializations
            .insert(compare_instr_id, vec![exact_int_shape]);
        let artifacts = plan_and_emit_function_exact_int_branches_v3_with_module_constants(
            &AlternativeCatalog::default_v3(),
            ModulePlanIdentity {
                module_name: "test".to_string(),
                source_hash: 0,
                cache_identity: "test-cache".to_string(),
            },
            FunctionPlanIdentity {
                function: SerializedFunctionId::new(
                    SerializedModuleId::new(0),
                    function.function_id.local_function_id(),
                ),
                debug_name: Some(function.names.qualname.clone()),
            },
            &function,
            &evidence,
            module.module_constants.as_slice(),
        )
        .unwrap();
        assert_eq!(artifacts.emission.functions[0].regions.len(), 4);
        assert_eq!(
            artifacts.plan.functions[0].scalar_threads.len(),
            1,
            "v3 add-store/compare should plan one scalar thread for c"
        );

        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                specialization_inputs: Some(FunctionSpecializationInputs {
                    legacy_overlays: Some(LegacyFunctionSpecializationOverlays {
                        exact_int_branch_artifacts: Some(std::sync::Arc::new(artifacts)),
                        ..LegacyFunctionSpecializationOverlays::default()
                    }),
                }),
                ..BuildSpecializedFunctionOptions::default()
            },
        );

        assert_eq!(
            count_opcode(&built.ctx.func, ir::Opcode::SaddOverflow),
            1,
            "v3 add-store region should emit the selected checked machine add"
        );
        let generic_helpers = import_user_names_for_symbols(
            &built,
            &["PyNumber_Add", "PyObject_RichCompare", "dp_jit_is_true"],
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &generic_helpers),
            3,
            "v3 add-store/compare should keep one generic local fallback for add and one for compare truthiness"
        );
        let materialize_helpers = import_user_names_for_symbols(&built, &["PyLong_FromLongLong"]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &materialize_helpers),
            1,
            "v3 scalar-thread hot path should not materialize c when both branch targets return without reading it"
        );
    }

    #[test]
    fn specialized_jit_opt_v3_exact_int_arithmetic_return_artifacts_emit_local_fallback() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialized_jit_opt_v3_exact_int_arithmetic_return_artifacts_emit_local_fallback",
        ) {
            return;
        }
        for (kind, opcode, generic_helper) in [
            (BinOpKind::Add, ir::Opcode::SaddOverflow, "PyNumber_Add"),
            (
                BinOpKind::Sub,
                ir::Opcode::SsubOverflow,
                "PyNumber_Subtract",
            ),
            (
                BinOpKind::Mul,
                ir::Opcode::SmulOverflow,
                "PyNumber_Multiply",
            ),
        ] {
            let blocks = [1usize as ObjPtr];
            let mut function = test_function();
            function.params = ParamSpec {
                params: vec![
                    Param {
                        name: "a".into(),
                        kind: ParamKind::Any,
                        has_default: false,
                    },
                    Param {
                        name: "b".into(),
                        kind: ParamKind::Any,
                        has_default: false,
                    },
                ],
            };
            let block_label = function.name_gen.next_block_name();
            let op_instr_id = InstrId::new(block_label, 2);
            function.blocks = vec![CodegenBlock {
                label: block_label,
                body: vec![],
                term: ret_term(with_instr_id(
                    op_expr(BinOp::new(
                        kind,
                        with_instr_id(name_expr(test_name("a")), InstrId::new(block_label, 0)),
                        with_instr_id(
                            name_expr(test_local_name("b", 1)),
                            InstrId::new(block_label, 1),
                        ),
                    )),
                    op_instr_id,
                )),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            }];
            set_stack_slots(&mut function, &["a", "b"]);

            let module = test_module(ModuleNameGen::new(0), vec![function]);
            let function = module.callable_defs[0].clone();
            let module_constants =
                crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
            let exact_int_shape = soac_opt::operator_specialization::pack_binary_shape(
                soac_opt::operator_specialization::ExactTypeTag::Int,
                soac_opt::operator_specialization::ExactTypeTag::Int,
            );
            let mut evidence = FunctionProfileEvidence::default();
            evidence
                .operator_specializations
                .insert(op_instr_id, vec![exact_int_shape]);
            let artifacts = plan_and_emit_function_exact_int_branches_v3_with_module_constants(
                &AlternativeCatalog::default_v3(),
                ModulePlanIdentity {
                    module_name: "test".to_string(),
                    source_hash: 0,
                    cache_identity: "test-cache".to_string(),
                },
                FunctionPlanIdentity {
                    function: SerializedFunctionId::new(
                        SerializedModuleId::new(0),
                        function.function_id.local_function_id(),
                    ),
                    debug_name: Some(function.names.qualname.clone()),
                },
                &function,
                &evidence,
                module.module_constants.as_slice(),
            )
            .unwrap();
            assert_eq!(artifacts.emission.functions[0].regions.len(), 2, "{kind:?}");

            let built = build_test_jit_function_with_constants_and_options(
                &module,
                &function,
                &blocks,
                &module_constants,
                BuildSpecializedFunctionOptions {
                    specialization_inputs: Some(FunctionSpecializationInputs {
                        legacy_overlays: Some(LegacyFunctionSpecializationOverlays {
                            exact_int_branch_artifacts: Some(std::sync::Arc::new(artifacts)),
                            ..LegacyFunctionSpecializationOverlays::default()
                        }),
                    }),
                    ..BuildSpecializedFunctionOptions::default()
                },
            );

            assert_eq!(
                count_opcode(&built.ctx.func, opcode),
                1,
                "v3 exact-int {kind:?} return should emit the selected checked machine operation"
            );
            let helper_names =
                import_user_names_for_symbols(&built, &[generic_helper, "PyLong_FromLongLong"]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &helper_names),
                2,
                "v3 exact-int {kind:?} return should keep generic fallback and PyLong materialization"
            );
        }
    }

    #[test]
    fn specialized_jit_opt_v3_exact_int_bitwise_return_artifacts_emit_local_fallback() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialized_jit_opt_v3_exact_int_bitwise_return_artifacts_emit_local_fallback",
        ) {
            return;
        }
        for (kind, opcode, generic_helper) in [
            (BinOpKind::And, ir::Opcode::Band, "PyNumber_And"),
            (BinOpKind::Or, ir::Opcode::Bor, "PyNumber_Or"),
            (BinOpKind::Xor, ir::Opcode::Bxor, "PyNumber_Xor"),
        ] {
            let blocks = [1usize as ObjPtr];
            let mut function = test_function();
            function.params = ParamSpec {
                params: vec![
                    Param {
                        name: "a".into(),
                        kind: ParamKind::Any,
                        has_default: false,
                    },
                    Param {
                        name: "b".into(),
                        kind: ParamKind::Any,
                        has_default: false,
                    },
                ],
            };
            let block_label = function.name_gen.next_block_name();
            let op_instr_id = InstrId::new(block_label, 2);
            function.blocks = vec![CodegenBlock {
                label: block_label,
                body: vec![],
                term: ret_term(with_instr_id(
                    op_expr(BinOp::new(
                        kind,
                        with_instr_id(name_expr(test_name("a")), InstrId::new(block_label, 0)),
                        with_instr_id(
                            name_expr(test_local_name("b", 1)),
                            InstrId::new(block_label, 1),
                        ),
                    )),
                    op_instr_id,
                )),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            }];
            set_stack_slots(&mut function, &["a", "b"]);

            let module = test_module(ModuleNameGen::new(0), vec![function]);
            let function = module.callable_defs[0].clone();
            let module_constants =
                crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
            let exact_int_shape = soac_opt::operator_specialization::pack_binary_shape(
                soac_opt::operator_specialization::ExactTypeTag::Int,
                soac_opt::operator_specialization::ExactTypeTag::Int,
            );
            let mut evidence = FunctionProfileEvidence::default();
            evidence
                .operator_specializations
                .insert(op_instr_id, vec![exact_int_shape]);
            let artifacts = plan_and_emit_function_exact_int_branches_v3_with_module_constants(
                &AlternativeCatalog::default_v3(),
                ModulePlanIdentity {
                    module_name: "test".to_string(),
                    source_hash: 0,
                    cache_identity: "test-cache".to_string(),
                },
                FunctionPlanIdentity {
                    function: SerializedFunctionId::new(
                        SerializedModuleId::new(0),
                        function.function_id.local_function_id(),
                    ),
                    debug_name: Some(function.names.qualname.clone()),
                },
                &function,
                &evidence,
                module.module_constants.as_slice(),
            )
            .unwrap();
            assert_eq!(artifacts.emission.functions[0].regions.len(), 2, "{kind:?}");

            let built = build_test_jit_function_with_constants_and_options(
                &module,
                &function,
                &blocks,
                &module_constants,
                BuildSpecializedFunctionOptions {
                    specialization_inputs: Some(FunctionSpecializationInputs {
                        legacy_overlays: Some(LegacyFunctionSpecializationOverlays {
                            exact_int_branch_artifacts: Some(std::sync::Arc::new(artifacts)),
                            ..LegacyFunctionSpecializationOverlays::default()
                        }),
                    }),
                    ..BuildSpecializedFunctionOptions::default()
                },
            );

            assert!(
                count_opcode(&built.ctx.func, opcode) >= 1,
                "v3 exact-int {kind:?} return should emit the selected machine bitwise op"
            );
            let helper_names =
                import_user_names_for_symbols(&built, &[generic_helper, "PyLong_FromLongLong"]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &helper_names),
                2,
                "v3 exact-int {kind:?} return should keep generic fallback and PyLong materialization"
            );
        }
    }

    #[test]
    fn specialized_jit_opt_v3_exact_int_compare_return_artifact_emits_local_fallback() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialized_jit_opt_v3_exact_int_compare_return_artifact_emits_local_fallback",
        ) {
            return;
        }
        let blocks = [1usize as ObjPtr];
        let mut function = test_function();
        function.params = ParamSpec {
            params: vec![
                Param {
                    name: "a".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
                Param {
                    name: "b".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
            ],
        };
        let block_label = function.name_gen.next_block_name();
        let compare_instr_id = InstrId::new(block_label, 2);
        function.blocks = vec![CodegenBlock {
            label: block_label,
            body: vec![],
            term: ret_term(with_instr_id(
                op_expr(BinOp::new(
                    BinOpKind::Lt,
                    with_instr_id(name_expr(test_name("a")), InstrId::new(block_label, 0)),
                    with_instr_id(
                        name_expr(test_local_name("b", 1)),
                        InstrId::new(block_label, 1),
                    ),
                )),
                compare_instr_id,
            )),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        }];
        set_stack_slots(&mut function, &["a", "b"]);

        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let exact_int_shape = soac_opt::operator_specialization::pack_binary_shape(
            soac_opt::operator_specialization::ExactTypeTag::Int,
            soac_opt::operator_specialization::ExactTypeTag::Int,
        );
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .operator_specializations
            .insert(compare_instr_id, vec![exact_int_shape]);
        let artifacts = plan_and_emit_function_exact_int_branches_v3_with_module_constants(
            &AlternativeCatalog::default_v3(),
            ModulePlanIdentity {
                module_name: "test".to_string(),
                source_hash: 0,
                cache_identity: "test-cache".to_string(),
            },
            FunctionPlanIdentity {
                function: SerializedFunctionId::new(
                    SerializedModuleId::new(0),
                    function.function_id.local_function_id(),
                ),
                debug_name: Some(function.names.qualname.clone()),
            },
            &function,
            &evidence,
            module.module_constants.as_slice(),
        )
        .unwrap();
        assert_eq!(artifacts.emission.functions[0].regions.len(), 2);

        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                specialization_inputs: Some(FunctionSpecializationInputs {
                    legacy_overlays: Some(LegacyFunctionSpecializationOverlays {
                        exact_int_branch_artifacts: Some(std::sync::Arc::new(artifacts)),
                        ..LegacyFunctionSpecializationOverlays::default()
                    }),
                }),
                ..BuildSpecializedFunctionOptions::default()
            },
        );

        let generic_compare = import_user_names_for_symbols(&built, &["PyObject_RichCompare"]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &generic_compare),
            1,
            "v3 exact-int comparison return should keep one generic rich-compare fallback"
        );
    }

    #[test]
    fn planned_precompile_inputs_accept_serialized_v3_module_artifact() {
        let function = test_function();
        let module = test_module(ModuleNameGen::new(0), vec![function]);
        let function = &module.callable_defs[0];
        let serialized_function = SerializedFunctionId::new(
            SerializedModuleId::new(0),
            function.function_id.local_function_id(),
        );
        let artifacts = ExactIntBranchV3Artifacts {
            plan: ModuleOptimizationPlanV3 {
                module: ModulePlanIdentity {
                    module_name: "test".to_string(),
                    source_hash: 0,
                    cache_identity: "test-cache".to_string(),
                },
                identity_tables: test_plan_identities(
                    "test",
                    0,
                    "test-cache",
                    serialized_function,
                    function.names.qualname.as_str(),
                    &[],
                ),
                helper_catalog_version: 1,
                cost_model_version: 1,
                functions: vec![soac_opt::plan_v3::FunctionOptimizationPlanV3 {
                    function: FunctionPlanIdentity {
                        function: serialized_function,
                        debug_name: Some(function.names.qualname.clone()),
                    },
                    regions: Vec::new(),
                    scalar_threads: Vec::new(),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    deopt_points: Vec::new(),
                    ownership: soac_opt::plan_v3::FunctionOwnershipPlan::default(),
                    diagnostics: Vec::new(),
                }],
            },
            emission: MechanicalModuleEmission {
                module_name: "test".to_string(),
                functions: vec![soac_opt::emit_v3::MechanicalFunctionEmission {
                    function: serialized_function,
                    debug_name: Some(function.names.qualname.clone()),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    scalar_threads: Vec::new(),
                    regions: Vec::new(),
                }],
            },
        };

        let inputs = planned_optimization_inputs_from_v3_artifacts_for_codegen_module(
            &artifacts,
            &module,
            artifacts.plan.module.module_name.as_str(),
            artifacts.plan.module.source_hash,
            None,
        )
        .expect("v3 module artifact should map onto the current codegen module");
        let function_artifacts = inputs
            .opt_v3_exact_int_branch_artifacts
            .get(&function.function_id)
            .expect("v3 inputs should include the current function");
        assert_eq!(function_artifacts.plan.functions.len(), 1);
        assert_eq!(function_artifacts.emission.functions.len(), 1);
        assert_eq!(
            function_artifacts.plan.functions[0]
                .function
                .function
                .local_function_id(),
            function.function_id.local_function_id()
        );
    }

    #[test]
    fn planned_precompile_inputs_consume_v3_emitted_direct_calls() {
        let module_name_gen = ModuleNameGen::new(7);
        let caller = test_function_in_module(&module_name_gen, "caller");
        let callee = test_function_in_module(&module_name_gen, "callee");
        let caller_id = caller.function_id;
        let callee_id = callee.function_id;
        let module = test_module(module_name_gen, vec![caller, callee]);
        let source = InstrId::new(BlockLabel::from_index(0), 11);
        let serialized_caller =
            SerializedFunctionId::new(SerializedModuleId::new(0), caller_id.local_function_id());
        let serialized_callee =
            SerializedFunctionId::new(SerializedModuleId::new(0), callee_id.local_function_id());
        let artifacts = ExactIntBranchV3Artifacts {
            plan: ModuleOptimizationPlanV3 {
                module: ModulePlanIdentity {
                    module_name: "test".to_string(),
                    source_hash: 0,
                    cache_identity: "test-cache".to_string(),
                },
                identity_tables: test_plan_identities(
                    "test",
                    0,
                    "test-cache",
                    serialized_caller,
                    "caller",
                    &[],
                ),
                helper_catalog_version: 1,
                cost_model_version: 1,
                functions: vec![soac_opt::plan_v3::FunctionOptimizationPlanV3 {
                    function: FunctionPlanIdentity {
                        function: serialized_caller,
                        debug_name: Some("caller".to_string()),
                    },
                    regions: Vec::new(),
                    scalar_threads: Vec::new(),
                    direct_calls: vec![DirectCallSpecializationPlan {
                        source,
                        target: serialized_callee,
                        arg_plan: PlanV3DirectCallArgPlan {
                            sources: vec![PlanV3DirectCallArgSource::Provided(0)],
                        },
                        body: test_v3_inline_call_body(),
                        reason: "profiled direct call".to_string(),
                    }],
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    deopt_points: Vec::new(),
                    ownership: soac_opt::plan_v3::FunctionOwnershipPlan::default(),
                    diagnostics: Vec::new(),
                }],
            },
            emission: MechanicalModuleEmission {
                module_name: "test".to_string(),
                functions: vec![soac_opt::emit_v3::MechanicalFunctionEmission {
                    function: serialized_caller,
                    debug_name: Some("caller".to_string()),
                    direct_calls: vec![soac_opt::emit_v3::MechanicalDirectCallEmission {
                        source,
                        target: serialized_callee,
                        arg_plan: PlanV3DirectCallArgPlan {
                            sources: vec![PlanV3DirectCallArgSource::Provided(0)],
                        },
                        body: test_v3_inline_call_body(),
                        reason: "profiled direct call".to_string(),
                    }],
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    scalar_threads: Vec::new(),
                    regions: Vec::new(),
                }],
            },
        };

        let planned_inputs = planned_optimization_inputs_from_v3_artifacts_for_codegen_module(
            &artifacts,
            &module,
            artifacts.plan.module.module_name.as_str(),
            artifacts.plan.module.source_hash,
            None,
        )
        .unwrap();

        assert_eq!(
            planned_inputs
                .opt_v3_emitted_direct_calls
                .get(&caller_id)
                .unwrap()
                .get(&source)
                .unwrap()
                .first()
                .unwrap(),
            &ResolvedV3DirectCallPlan {
                source,
                target: callee_id,
                arg_plan: soac_opt::passes::TypedDirectCallArgPlan {
                    sources: vec![soac_opt::passes::TypedDirectCallArgSource::Provided(0)],
                },
                body: test_v3_inline_call_body(),
                reason: "profiled direct call".to_string(),
            }
        );
    }

    #[test]
    fn planned_precompile_inputs_consume_cross_module_v3_emitted_direct_calls() {
        let caller_module_name = "test_v3_cross_module_direct_call_caller";
        let callee_module_name = "test_v3_cross_module_direct_call_callee";
        let caller_source_hash = 0xabcdef21;
        let callee_source_hash = 0xabcdef22;
        let caller_module_name_gen = ModuleNameGen::new(37);
        let callee_module_name_gen = ModuleNameGen::new(38);
        let caller = test_function_in_module(&caller_module_name_gen, "caller");
        let callee = test_function_in_module(&callee_module_name_gen, "callee");
        let caller_id = caller.function_id;
        let callee_id = callee.function_id;
        let caller_module = test_module(caller_module_name_gen, vec![caller]);
        let callee_module = test_module(callee_module_name_gen, vec![callee]);
        let module_index = PrecompileModuleIndex::from_entries([
            PrecompileModuleIndexEntry {
                module_name: caller_module_name,
                source_hash: caller_source_hash,
                module: &caller_module,
            },
            PrecompileModuleIndexEntry {
                module_name: callee_module_name,
                source_hash: callee_source_hash,
                module: &callee_module,
            },
        ])
        .expect("precompile module index should build");
        let source = InstrId::new(BlockLabel::from_index(0), 11);
        let serialized_caller =
            SerializedFunctionId::new(SerializedModuleId::new(0), caller_id.local_function_id());
        let serialized_callee =
            SerializedFunctionId::new(SerializedModuleId::new(1), callee_id.local_function_id());
        let artifacts = ExactIntBranchV3Artifacts {
            plan: ModuleOptimizationPlanV3 {
                module: ModulePlanIdentity {
                    module_name: caller_module_name.to_string(),
                    source_hash: caller_source_hash,
                    cache_identity: "test-cache".to_string(),
                },
                identity_tables: test_plan_identities(
                    caller_module_name,
                    caller_source_hash,
                    "test-cache",
                    serialized_caller,
                    "caller",
                    &[(callee_module_name, callee_source_hash)],
                ),
                helper_catalog_version: 1,
                cost_model_version: 1,
                functions: vec![soac_opt::plan_v3::FunctionOptimizationPlanV3 {
                    function: FunctionPlanIdentity {
                        function: serialized_caller,
                        debug_name: Some("caller".to_string()),
                    },
                    regions: Vec::new(),
                    scalar_threads: Vec::new(),
                    direct_calls: vec![DirectCallSpecializationPlan {
                        source,
                        target: serialized_callee,
                        arg_plan: PlanV3DirectCallArgPlan {
                            sources: vec![PlanV3DirectCallArgSource::Provided(0)],
                        },
                        body: test_v3_inline_call_body(),
                        reason: "profiled cross-module direct call".to_string(),
                    }],
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    deopt_points: Vec::new(),
                    ownership: soac_opt::plan_v3::FunctionOwnershipPlan::default(),
                    diagnostics: Vec::new(),
                }],
            },
            emission: MechanicalModuleEmission {
                module_name: caller_module_name.to_string(),
                functions: vec![soac_opt::emit_v3::MechanicalFunctionEmission {
                    function: serialized_caller,
                    debug_name: Some("caller".to_string()),
                    direct_calls: vec![soac_opt::emit_v3::MechanicalDirectCallEmission {
                        source,
                        target: serialized_callee,
                        arg_plan: PlanV3DirectCallArgPlan {
                            sources: vec![PlanV3DirectCallArgSource::Provided(0)],
                        },
                        body: test_v3_inline_call_body(),
                        reason: "profiled cross-module direct call".to_string(),
                    }],
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    scalar_threads: Vec::new(),
                    regions: Vec::new(),
                }],
            },
        };

        let planned_inputs = planned_optimization_inputs_from_v3_artifacts_for_codegen_module(
            &artifacts,
            &caller_module,
            caller_module_name,
            caller_source_hash,
            Some(&module_index),
        )
        .unwrap();

        assert_eq!(
            planned_inputs
                .opt_v3_emitted_direct_calls
                .get(&caller_id)
                .unwrap()
                .get(&source)
                .unwrap()
                .first()
                .unwrap(),
            &ResolvedV3DirectCallPlan {
                source,
                target: callee_id,
                arg_plan: soac_opt::passes::TypedDirectCallArgPlan {
                    sources: vec![soac_opt::passes::TypedDirectCallArgSource::Provided(0)],
                },
                body: test_v3_inline_call_body(),
                reason: "profiled cross-module direct call".to_string(),
            },
            "JIT precompile loading should resolve v3 cross-module direct-call targets through the identity table and module index"
        );
    }

    #[test]
    fn planned_precompile_inputs_consume_v3_emitted_indexed_fields() {
        let module_name_gen = ModuleNameGen::new(7);
        let mut constants = TestConstantPool::default();
        let mut caller = test_function_in_module(&module_name_gen, "caller");
        caller.params = ParamSpec {
            params: vec![
                Param {
                    name: "obj".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
                Param {
                    name: "replacement".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
            ],
        };
        let block_label = caller.name_gen.next_block_name();
        let load_source = InstrId::new(block_label, 11);
        let store_source = InstrId::new(block_label, 13);
        caller.blocks = vec![CodegenBlock {
            label: block_label,
            body: vec![with_instr_id(
                op_expr(SetAttr::new(
                    name_expr(test_name("obj")),
                    constants.string_expr("value"),
                    name_expr(test_name("replacement")),
                )),
                store_source,
            )],
            term: ret_term(with_instr_id(
                op_expr(GetAttr::new(
                    name_expr(test_name("obj")),
                    constants.string_expr("value"),
                )),
                load_source,
            )),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        }];
        set_stack_slots(&mut caller, &["obj", "replacement"]);
        let caller_id = caller.function_id;
        let mut module = test_module(module_name_gen, vec![caller]);
        module.module_constants = constants.module_constants;
        let serialized_caller =
            SerializedFunctionId::new(SerializedModuleId::new(0), caller_id.local_function_id());
        let owner_type = IndexedFieldOwnerType {
            module_name: "pkg.model".to_string(),
            qualname: "Record".to_string(),
        };
        let artifacts = ExactIntBranchV3Artifacts {
            plan: ModuleOptimizationPlanV3 {
                module: ModulePlanIdentity {
                    module_name: "test".to_string(),
                    source_hash: 0,
                    cache_identity: "test-cache".to_string(),
                },
                identity_tables: test_plan_identities(
                    "test",
                    0,
                    "test-cache",
                    serialized_caller,
                    "caller",
                    &[],
                ),
                helper_catalog_version: 1,
                cost_model_version: 1,
                functions: vec![soac_opt::plan_v3::FunctionOptimizationPlanV3 {
                    function: FunctionPlanIdentity {
                        function: serialized_caller,
                        debug_name: Some("caller".to_string()),
                    },
                    regions: Vec::new(),
                    scalar_threads: Vec::new(),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: vec![
                        IndexedFieldSpecializationPlan {
                            source: load_source,
                            access: IndexedFieldAccessKind::Load,
                            owner_type: owner_type.clone(),
                            attr_name: "value".to_string(),
                            expected_index: 2,
                            guard: IndexedFieldGuardPlan {
                                kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                            },
                            fallback: IndexedFieldFallbackPlan {
                                kind: IndexedFieldFallbackKind::OriginalAttrAccess,
                            },
                            reason: "profiled type_keys selected this indexed-field layout"
                                .to_string(),
                        },
                        IndexedFieldSpecializationPlan {
                            source: store_source,
                            access: IndexedFieldAccessKind::Store,
                            owner_type: owner_type.clone(),
                            attr_name: "value".to_string(),
                            expected_index: 2,
                            guard: IndexedFieldGuardPlan {
                                kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                            },
                            fallback: IndexedFieldFallbackPlan {
                                kind: IndexedFieldFallbackKind::OriginalAttrAccess,
                            },
                            reason: "profiled type_keys selected this indexed-field layout"
                                .to_string(),
                        },
                    ],
                    indexed_globals: Vec::new(),
                    deopt_points: Vec::new(),
                    ownership: soac_opt::plan_v3::FunctionOwnershipPlan::default(),
                    diagnostics: Vec::new(),
                }],
            },
            emission: MechanicalModuleEmission {
                module_name: "test".to_string(),
                functions: vec![soac_opt::emit_v3::MechanicalFunctionEmission {
                    function: serialized_caller,
                    debug_name: Some("caller".to_string()),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: vec![
                        soac_opt::emit_v3::MechanicalIndexedFieldEmission {
                            source: load_source,
                            access: IndexedFieldAccessKind::Load,
                            guard: MechanicalIndexedFieldGuard {
                                kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                                owner_type: owner_type.clone(),
                                attr_name: "value".to_string(),
                                expected_index: 2,
                            },
                            fallback: IndexedFieldFallbackPlan {
                                kind: IndexedFieldFallbackKind::OriginalAttrAccess,
                            },
                            reason: "profiled type_keys selected this indexed-field layout"
                                .to_string(),
                        },
                        soac_opt::emit_v3::MechanicalIndexedFieldEmission {
                            source: store_source,
                            access: IndexedFieldAccessKind::Store,
                            guard: MechanicalIndexedFieldGuard {
                                kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                                owner_type: owner_type.clone(),
                                attr_name: "value".to_string(),
                                expected_index: 2,
                            },
                            fallback: IndexedFieldFallbackPlan {
                                kind: IndexedFieldFallbackKind::OriginalAttrAccess,
                            },
                            reason: "profiled type_keys selected this indexed-field layout"
                                .to_string(),
                        },
                    ],
                    indexed_globals: Vec::new(),
                    scalar_threads: Vec::new(),
                    regions: Vec::new(),
                }],
            },
        };

        let planned_inputs = planned_optimization_inputs_from_v3_artifacts_for_codegen_module(
            &artifacts,
            &module,
            artifacts.plan.module.module_name.as_str(),
            artifacts.plan.module.source_hash,
            None,
        )
        .unwrap();

        assert_eq!(
            planned_inputs
                .opt_v3_emitted_indexed_fields
                .get(&caller_id)
                .unwrap()
                .get(&load_source)
                .unwrap(),
            &vec![OptV3IndexedFieldAccessPlan {
                access: IndexedFieldAccessKind::Load,
                guard: MechanicalIndexedFieldGuard {
                    kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                    owner_type: owner_type.clone(),
                    attr_name: "value".to_string(),
                    expected_index: 2,
                },
                fallback: IndexedFieldFallbackKind::OriginalAttrAccess,
            }]
        );
        assert_eq!(
            planned_inputs
                .opt_v3_emitted_indexed_fields
                .get(&caller_id)
                .unwrap()
                .get(&store_source)
                .unwrap(),
            &vec![OptV3IndexedFieldAccessPlan {
                access: IndexedFieldAccessKind::Store,
                guard: MechanicalIndexedFieldGuard {
                    kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                    owner_type: owner_type.clone(),
                    attr_name: "value".to_string(),
                    expected_index: 2,
                },
                fallback: IndexedFieldFallbackKind::OriginalAttrAccess,
            }]
        );
    }

    #[test]
    fn planned_precompile_inputs_consume_v3_emitted_indexed_globals() {
        let module_name_gen = ModuleNameGen::new(7);
        let load_source = InstrId::new(BlockLabel::from_index(0), 11);
        let store_source = InstrId::new(BlockLabel::from_index(0), 13);
        let caller = with_single_test_block(
            test_function_in_module(&module_name_gen, "caller"),
            vec![with_instr_id(
                assign_stmt(test_global_name("counter"), name_expr(test_name("value"))),
                store_source,
            )],
            ret_term(with_instr_id(
                name_expr(test_global_name("counter")),
                load_source,
            )),
        );
        let caller_id = caller.function_id;
        let mut module = test_module(module_name_gen, vec![caller]);
        module.global_names = vec!["counter".to_string()];
        let serialized_caller =
            SerializedFunctionId::new(SerializedModuleId::new(0), caller_id.local_function_id());
        let guard = IndexedGlobalGuardPlan {
            kind: IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
        };
        let fallback = IndexedGlobalFallbackPlan {
            kind: IndexedGlobalFallbackKind::OriginalGlobalAccess,
        };
        let artifacts = ExactIntBranchV3Artifacts {
            plan: ModuleOptimizationPlanV3 {
                module: ModulePlanIdentity {
                    module_name: "test".to_string(),
                    source_hash: 0,
                    cache_identity: "test-cache".to_string(),
                },
                identity_tables: test_plan_identities(
                    "test",
                    0,
                    "test-cache",
                    serialized_caller,
                    "caller",
                    &[],
                ),
                helper_catalog_version: 1,
                cost_model_version: 1,
                functions: vec![soac_opt::plan_v3::FunctionOptimizationPlanV3 {
                    function: FunctionPlanIdentity {
                        function: serialized_caller,
                        debug_name: Some("caller".to_string()),
                    },
                    regions: Vec::new(),
                    scalar_threads: Vec::new(),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: vec![
                        IndexedGlobalSpecializationPlan {
                            source: load_source,
                            access: IndexedGlobalAccessKind::Load,
                            module_name: "test".to_string(),
                            name: "counter".to_string(),
                            expected_index: 0,
                            guard: guard.clone(),
                            fallback: fallback.clone(),
                            reason: "profiled module_keys selected this indexed-global slot"
                                .to_string(),
                        },
                        IndexedGlobalSpecializationPlan {
                            source: store_source,
                            access: IndexedGlobalAccessKind::Store,
                            module_name: "test".to_string(),
                            name: "counter".to_string(),
                            expected_index: 0,
                            guard: guard.clone(),
                            fallback: fallback.clone(),
                            reason: "profiled module_keys selected this indexed-global slot"
                                .to_string(),
                        },
                    ],
                    deopt_points: Vec::new(),
                    ownership: soac_opt::plan_v3::FunctionOwnershipPlan::default(),
                    diagnostics: Vec::new(),
                }],
            },
            emission: MechanicalModuleEmission {
                module_name: "test".to_string(),
                functions: vec![soac_opt::emit_v3::MechanicalFunctionEmission {
                    function: serialized_caller,
                    debug_name: Some("caller".to_string()),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: vec![
                        soac_opt::emit_v3::MechanicalIndexedGlobalEmission {
                            source: load_source,
                            access: IndexedGlobalAccessKind::Load,
                            module_name: "test".to_string(),
                            name: "counter".to_string(),
                            expected_index: 0,
                            guard: guard.clone(),
                            fallback: fallback.clone(),
                            reason: "profiled module_keys selected this indexed-global slot"
                                .to_string(),
                        },
                        soac_opt::emit_v3::MechanicalIndexedGlobalEmission {
                            source: store_source,
                            access: IndexedGlobalAccessKind::Store,
                            module_name: "test".to_string(),
                            name: "counter".to_string(),
                            expected_index: 0,
                            guard,
                            fallback,
                            reason: "profiled module_keys selected this indexed-global slot"
                                .to_string(),
                        },
                    ],
                    scalar_threads: Vec::new(),
                    regions: Vec::new(),
                }],
            },
        };

        let planned_inputs = planned_optimization_inputs_from_v3_artifacts_for_codegen_module(
            &artifacts,
            &module,
            artifacts.plan.module.module_name.as_str(),
            artifacts.plan.module.source_hash,
            None,
        )
        .unwrap();

        assert_eq!(
            planned_inputs
                .opt_v3_emitted_indexed_globals
                .get(&caller_id)
                .unwrap()
                .get(&load_source)
                .unwrap(),
            &OptV3IndexedGlobalAccessPlan {
                source: load_source,
                access: IndexedGlobalAccessKind::Load,
                module_name: "test".to_string(),
                name: "counter".to_string(),
                expected_index: 0,
                guard: IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
                fallback: IndexedGlobalFallbackKind::OriginalGlobalAccess,
            }
        );
        assert_eq!(
            planned_inputs
                .opt_v3_emitted_indexed_globals
                .get(&caller_id)
                .unwrap()
                .get(&store_source)
                .unwrap(),
            &OptV3IndexedGlobalAccessPlan {
                source: store_source,
                access: IndexedGlobalAccessKind::Store,
                module_name: "test".to_string(),
                name: "counter".to_string(),
                expected_index: 0,
                guard: IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
                fallback: IndexedGlobalFallbackKind::OriginalGlobalAccess,
            }
        );
    }

    fn indexed_global_test_function(
        module_name_gen: &ModuleNameGen,
        load_source: InstrId,
        store_source: InstrId,
    ) -> BlockPyFunction<CodegenModuleShape> {
        with_single_test_block(
            test_function_in_module(module_name_gen, "global_user"),
            vec![
                with_instr_id(
                    assign_stmt(test_global_name("counter"), none_expr()),
                    store_source,
                ),
                with_instr_id(
                    expr_stmt(name_expr(test_global_name("counter"))),
                    load_source,
                ),
            ],
            ret_term(none_expr()),
        )
    }

    fn opt_v3_indexed_global_plan_for_name(
        source: InstrId,
        access: IndexedGlobalAccessKind,
        name: &str,
    ) -> OptV3IndexedGlobalAccessPlan {
        OptV3IndexedGlobalAccessPlan {
            source,
            access,
            module_name: "test".to_string(),
            name: name.to_string(),
            expected_index: 0,
            guard: IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
            fallback: IndexedGlobalFallbackKind::OriginalGlobalAccess,
        }
    }

    fn opt_v3_indexed_global_plan(
        source: InstrId,
        access: IndexedGlobalAccessKind,
    ) -> OptV3IndexedGlobalAccessPlan {
        opt_v3_indexed_global_plan_for_name(source, access, "counter")
    }

    fn first_indexed_global_access_source(
        function: &BlockPyFunction<CodegenModuleShape>,
        access: IndexedGlobalAccessKind,
        name: &str,
    ) -> InstrId {
        struct Finder<'a> {
            access: IndexedGlobalAccessKind,
            name: &'a str,
            source: Option<InstrId>,
        }

        impl Visit<InstrCodegen> for Finder<'_> {
            fn visit_instr(&mut self, expr: &InstrCodegen)
            where
                InstrCodegen: ChildVisitable<InstrCodegen>,
            {
                if self.source.is_none() {
                    match expr {
                        InstrCodegen::Load(op)
                            if self.access == IndexedGlobalAccessKind::Load
                                && op.name.id_str() == self.name
                                && matches!(op.name.location, NameLocation::Global(_)) =>
                        {
                            self.source = Some(op.semantic_instr_id());
                        }
                        InstrCodegen::Store(op)
                            if self.access == IndexedGlobalAccessKind::Store
                                && op.name.id_str() == self.name
                                && matches!(op.name.location, NameLocation::Global(_)) =>
                        {
                            self.source = Some(op.semantic_instr_id());
                        }
                        _ => {}
                    }
                }
                expr.visit_children(self);
            }
        }

        let mut finder = Finder {
            access,
            name,
            source: None,
        };
        finder.visit_fn(function);
        finder.source.unwrap_or_else(|| {
            panic!(
                "function {} should contain indexed-global {access:?} for {name}",
                function.names.qualname
            )
        })
    }

    fn indexed_global_specialization_inputs_for_function(
        function: &BlockPyFunction<CodegenModuleShape>,
        access: IndexedGlobalAccessKind,
        name: &str,
    ) -> FunctionSpecializationInputs {
        let source = first_indexed_global_access_source(function, access, name);
        FunctionSpecializationInputs {
            legacy_overlays: Some(LegacyFunctionSpecializationOverlays {
                indexed_globals_by_instr: HashMap::from([(
                    source,
                    opt_v3_indexed_global_plan_for_name(source, access, name),
                )]),
                ..LegacyFunctionSpecializationOverlays::default()
            }),
        }
    }

    #[test]
    fn codegen_consumes_v3_indexed_global_inputs_without_legacy_counters() {
        let module_name_gen = ModuleNameGen::new(7);
        let load_source = InstrId::new(BlockLabel::from_index(0), 11);
        let store_source = InstrId::new(BlockLabel::from_index(0), 13);
        let function = indexed_global_test_function(&module_name_gen, load_source, store_source);
        let mut module = test_module(module_name_gen, vec![function.clone()]);
        module.global_names = vec!["counter".to_string()];
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let blocks = [1usize as ObjPtr];
        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                specialization_inputs: Some(FunctionSpecializationInputs {
                    legacy_overlays: Some(LegacyFunctionSpecializationOverlays {
                        indexed_globals_by_instr: HashMap::from([
                            (
                                load_source,
                                opt_v3_indexed_global_plan(
                                    load_source,
                                    IndexedGlobalAccessKind::Load,
                                ),
                            ),
                            (
                                store_source,
                                opt_v3_indexed_global_plan(
                                    store_source,
                                    IndexedGlobalAccessKind::Store,
                                ),
                            ),
                        ]),
                        ..LegacyFunctionSpecializationOverlays::default()
                    }),
                }),
                ..BuildSpecializedFunctionOptions::default()
            },
        );

        let probe_helper = declared_user_names_for_symbols(
            &built,
            &[super::SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL],
        );
        let store_helper = declared_user_names_for_symbols(
            &built,
            &[super::SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL],
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &probe_helper),
            1
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &store_helper),
            1
        );
    }

    #[test]
    fn codegen_does_not_rediscover_indexed_globals_from_profile_counters() {
        let module_name_gen = ModuleNameGen::new(7);
        let load_source = InstrId::new(BlockLabel::from_index(0), 11);
        let store_source = InstrId::new(BlockLabel::from_index(0), 13);
        let function = indexed_global_test_function(&module_name_gen, load_source, store_source);
        let mut module = test_module(module_name_gen, vec![function.clone()]);
        module.global_names = vec!["counter".to_string()];
        module.counter_defs.extend([
            CounterDef {
                id: CounterId(0),
                scope: CounterScope::This,
                kind: "global_indexed".to_string(),
                site: CounterSite::Runtime {
                    function_id: Some(function.function_id),
                    instr_id: Some(load_source),
                },
                branches: vec![
                    soac_core::block_py::CounterBranch::new("hit"),
                    soac_core::block_py::CounterBranch::new("fallback"),
                ],
            },
            CounterDef {
                id: CounterId(1),
                scope: CounterScope::This,
                kind: "global_indexed".to_string(),
                site: CounterSite::Runtime {
                    function_id: Some(function.function_id),
                    instr_id: Some(store_source),
                },
                branches: vec![
                    soac_core::block_py::CounterBranch::new("hit"),
                    soac_core::block_py::CounterBranch::new("fallback"),
                ],
            },
        ]);
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let blocks = [1usize as ObjPtr];
        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                specialization_inputs: Some(FunctionSpecializationInputs {
                    legacy_overlays: Some(LegacyFunctionSpecializationOverlays::default()),
                }),
                ..BuildSpecializedFunctionOptions::default()
            },
        );

        let indexed_helpers = declared_user_names_for_symbols(
            &built,
            &[
                super::SOAC_RUNTIME_PROBE_GLOBAL_INDEXED_SYMBOL,
                super::SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL,
            ],
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &indexed_helpers),
            0
        );
    }

    #[test]
    fn specialization_profile_consumes_v3_indexed_fields_as_codegen_inputs() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialization_profile_consumes_v3_indexed_fields_as_codegen_inputs",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let soac_work_dir = fresh_test_work_dir("strict-v3-indexed-field-input");
            let module_cache_root = soac_work_dir.join("modules");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let _opt_mode = set_opt_mode("apply");
            let owner_module = PyModule::from_code(
                py,
                c"
class Point:
    pass
",
                c"field_type_test.py",
                c"field_type_test",
            )
            .expect("owner module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_type_test", owner_module.as_any())
                .expect("owner module should be registered");

            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def read_point(point):
    return point.x
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            let module_name = "counter_test";
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, module_name, "")
                    .expect("shared state should build");
            let function = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "read_point")
                .expect("missing read_point")
                .clone();
            struct GetAttrFinder {
                instr_id: Option<InstrId>,
            }
            impl Visit<InstrCodegen> for GetAttrFinder {
                fn visit_instr(&mut self, expr: &InstrCodegen)
                where
                    InstrCodegen: ChildVisitable<InstrCodegen>,
                {
                    if self.instr_id.is_none()
                        && let InstrCodegen::GetAttr(_) = expr
                    {
                        self.instr_id = Some(expr.semantic_instr_id());
                    }
                    expr.visit_children(self);
                }
            }
            let mut finder = GetAttrFinder { instr_id: None };
            for block in &function.blocks {
                for expr in &block.body {
                    finder.visit_instr(expr);
                }
                finder.visit_term(&block.term);
            }
            let getattr_instr_id = finder
                .instr_id
                .expect("read_point should contain a GetAttr");
            let current_function = shared_state
                .lookup_function(function.function_id)
                .expect("read_point should be present in shared state");
            let cache_identity = pre_optimization_module_cache_identity(
                env!("SOAC_BUILD_IDENTITY"),
                shared_state.module_name == "soac.runtime",
            );
            let mut artifacts = test_empty_v3_artifacts_for_function(
                module_name,
                shared_state.source_hash,
                cache_identity.as_str(),
                0,
                current_function,
            );
            let owner_type = IndexedFieldOwnerType {
                module_name: "field_type_test".to_string(),
                qualname: "Point".to_string(),
            };
            let field_guard = IndexedFieldGuardPlan {
                kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
            };
            let field_fallback = IndexedFieldFallbackPlan {
                kind: IndexedFieldFallbackKind::OriginalAttrAccess,
            };
            artifacts.plan.functions[0]
                .indexed_fields
                .push(IndexedFieldSpecializationPlan {
                    source: getattr_instr_id,
                    access: IndexedFieldAccessKind::Load,
                    owner_type: owner_type.clone(),
                    attr_name: "x".to_string(),
                    expected_index: 0,
                    guard: field_guard.clone(),
                    fallback: field_fallback.clone(),
                    reason: "profiled type_keys selected this indexed-field layout".to_string(),
                });
            artifacts.emission.functions[0].indexed_fields.push(
                soac_opt::emit_v3::MechanicalIndexedFieldEmission {
                    source: getattr_instr_id,
                    access: IndexedFieldAccessKind::Load,
                    guard: MechanicalIndexedFieldGuard {
                        kind: field_guard.kind,
                        owner_type,
                        attr_name: "x".to_string(),
                        expected_index: 0,
                    },
                    fallback: field_fallback,
                    reason: "profiled type_keys selected this indexed-field layout".to_string(),
                },
            );
            write_test_optimization_artifacts_v3_for_shared_state(
                module_cache_root.as_path(),
                PythonModuleCacheSource::Project,
                shared_state.as_ref(),
                &artifacts,
            );

            let profile = SpecializationProfile::from_runtime_state_with_session(
                Some(shared_state.as_ref()),
                None,
            )
            .expect("strict v3 indexed-field artifact should load");
            let inputs = FunctionSpecializationInputs::from_profile(&profile, &function)
                .expect("v3 indexed-field input should resolve to codegen guards");
            assert!(
                inputs
                    .legacy_overlays
                    .as_ref()
                    .expect("legacy artifact path should carry sidecar overlays")
                    .indexed_fields_by_instr
                    .contains_key(&getattr_instr_id),
                "v3 emitted indexed-field decision should become explicit v3 codegen input"
            );

            modules
                .del_item("field_type_test")
                .expect("owner module should be removed");
        });
    }

    #[test]
    fn v3_indexed_field_input_skips_unresolvable_owner_type() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|_| {
            let module_name_gen = ModuleNameGen::new(7);
            let mut constants = TestConstantPool::default();
            let mut function = test_function_in_module(&module_name_gen, "read_missing_owner");
            function.params = ParamSpec {
                params: vec![Param {
                    name: "obj".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                }],
            };
            let block_label = function.name_gen.next_block_name();
            let getattr_instr_id = InstrId::new(block_label, 1);
            function.blocks = vec![CodegenBlock {
                label: block_label,
                body: vec![],
                term: ret_term(with_instr_id(
                    op_expr(GetAttr::new(
                        name_expr(test_name("obj")),
                        constants.string_expr("x"),
                    )),
                    getattr_instr_id,
                )),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            }];
            set_stack_slots(&mut function, &["obj"]);

            let owner_type = IndexedFieldOwnerType {
                module_name: "missing_field_owner_module".to_string(),
                qualname: "Point".to_string(),
            };
            let profile = SpecializationProfile {
                module_name: None,
                counter_dump_path: None,
                optimized_module: None,
                direct_call_emission_scope: DirectCallEmissionScope::DirectCallBodiesOnly,
                opt_v3_emitted_direct_calls: HashMap::new(),
                opt_v3_emitted_exact_list_items: HashMap::new(),
                opt_v3_emitted_indexed_fields: HashMap::from([(
                    function.function_id,
                    HashMap::from([(
                        getattr_instr_id,
                        vec![OptV3IndexedFieldAccessPlan {
                            access: IndexedFieldAccessKind::Load,
                            guard: MechanicalIndexedFieldGuard {
                                kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                                owner_type,
                                attr_name: "x".to_string(),
                                expected_index: 0,
                            },
                            fallback: IndexedFieldFallbackKind::OriginalAttrAccess,
                        }],
                    )]),
                )]),
                opt_v3_emitted_indexed_globals: HashMap::new(),
                opt_v3_exact_int_branch_artifacts: HashMap::new(),
                behavior_change_indexed_stores: false,
                profiled_cold_blocks: false,
                guard_miss_deopt: false,
            };

            let inputs = FunctionSpecializationInputs::from_profile(&profile, &function)
                .expect("unresolvable v3 indexed-field owner should keep local fallback");
            assert!(
                !inputs
                    .legacy_overlays
                    .as_ref()
                    .expect("legacy artifact path should carry sidecar overlays")
                    .indexed_fields_by_instr
                    .contains_key(&getattr_instr_id),
                "unresolvable v3 indexed-field owner should not become codegen input"
            );
        });
    }

    #[test]
    fn strict_v3_indexed_field_setattr_hits_apply_mode_first_insert() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "strict_v3_indexed_field_setattr_hits_apply_mode_first_insert",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();

        Python::attach(|py| {
            let soac_work_dir = fresh_test_work_dir("strict-v3-indexed-field-setattr");
            let module_cache_root = soac_work_dir.join("modules");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let _opt_mode = set_opt_mode("apply");
            let owner_module = PyModule::from_code(
                py,
                c"
class Point:
    pass
",
                c"field_type_test.py",
                c"field_type_test",
            )
            .expect("owner module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_type_test", owner_module.as_any())
                .expect("owner module should be registered");

            let mut lowered = soac_lowering::lower_python_to_blockpy_for_testing(
                r#"
def write_point(point, value):
    point.x = value
    return point.x
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            instrument_module_with_legacy_call_target_counters(&mut lowered);
            let module_name = "counter_test";
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, lowered, module_name, "")
                    .expect("shared state should build");
            let function = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "write_point")
                .expect("missing write_point")
                .clone();
            let setattr_instr_id = function
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .find_map(|expr| match expr {
                    InstrCodegen::SetAttr(_) => Some(expr.semantic_instr_id()),
                    _ => None,
                })
                .expect("write_point should contain a SetAttr");

            let current_function = shared_state
                .lookup_function(function.function_id)
                .expect("write_point should be present in shared state");
            let cache_identity = pre_optimization_module_cache_identity(
                env!("SOAC_BUILD_IDENTITY"),
                shared_state.module_name == "soac.runtime",
            );
            let mut artifacts = test_empty_v3_artifacts_for_function(
                module_name,
                shared_state.source_hash,
                cache_identity.as_str(),
                0,
                current_function,
            );
            let owner_type = IndexedFieldOwnerType {
                module_name: "field_type_test".to_string(),
                qualname: "Point".to_string(),
            };
            let field_guard = IndexedFieldGuardPlan {
                kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
            };
            let field_fallback = IndexedFieldFallbackPlan {
                kind: IndexedFieldFallbackKind::OriginalAttrAccess,
            };
            let store_reason =
                "profiled type_keys selected this indexed-field layout for SetAttr".to_string();
            artifacts.plan.functions[0]
                .indexed_fields
                .push(IndexedFieldSpecializationPlan {
                    source: setattr_instr_id,
                    access: IndexedFieldAccessKind::Store,
                    owner_type: owner_type.clone(),
                    attr_name: "x".to_string(),
                    expected_index: 0,
                    guard: field_guard.clone(),
                    fallback: field_fallback.clone(),
                    reason: store_reason.clone(),
                });
            artifacts.emission.functions[0].indexed_fields.push(
                soac_opt::emit_v3::MechanicalIndexedFieldEmission {
                    source: setattr_instr_id,
                    access: IndexedFieldAccessKind::Store,
                    guard: MechanicalIndexedFieldGuard {
                        kind: field_guard.kind,
                        owner_type,
                        attr_name: "x".to_string(),
                        expected_index: 0,
                    },
                    fallback: field_fallback,
                    reason: store_reason,
                },
            );
            write_test_optimization_artifacts_v3_for_shared_state(
                module_cache_root.as_path(),
                PythonModuleCacheSource::Project,
                shared_state.as_ref(),
                &artifacts,
            );

            let profile = SpecializationProfile::from_runtime_state_with_session(
                Some(shared_state.as_ref()),
                None,
            )
            .expect("strict v3 indexed-field SetAttr artifact should load");
            let inputs = FunctionSpecializationInputs::from_profile(&profile, &function)
                .expect("v3 indexed-field SetAttr input should resolve to codegen guards");
            assert!(
                inputs
                    .legacy_overlays
                    .as_ref()
                    .expect("legacy artifact path should carry sidecar overlays")
                    .indexed_fields_by_instr
                    .contains_key(&setattr_instr_id),
                "v3 emitted indexed-field Store decision should become explicit v3 codegen input"
            );

            let (hit_counter_id, hit_branch_id) = runtime_branch_counter_for(
                &shared_state.lowered_module.counter_defs,
                function.function_id,
                setattr_instr_id,
                "field_access",
                "indexed_hit",
            );
            let (fallback_counter_id, fallback_branch_id) = runtime_branch_counter_for(
                &shared_state.lowered_module.counter_defs,
                function.function_id,
                setattr_instr_id,
                "field_access",
                "indexed_fallback",
            );
            let (generic_counter_id, generic_branch_id) = runtime_branch_counter_for(
                &shared_state.lowered_module.counter_defs,
                function.function_id,
                setattr_instr_id,
                "field_access",
                "generic_setattr",
            );

            let runtime = unsafe { build_test_module_runtime(py, shared_state.clone()) };
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let compile_session = crate::session::CompileSession::process();
            let compiled_handle = unsafe {
                compile_cranelift_run_bb_specialized_cached(
                    &compile_session,
                    &blocks,
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    Some(shared_state.as_ref()),
                )
            }
            .expect("strict v3 specialized write_point should compile");
            let (code_ptr, _default_code_ptr, param_count) = compiled_handle
                .handle
                .direct_runner_info()
                .expect("compiled direct runner should expose entrypoint");
            assert_eq!(param_count, 2, "write_point should take two direct args");
            let entry: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *mut c_void,
            ) -> *mut c_void = unsafe { std::mem::transmute(code_ptr) };

            let point_type = owner_module
                .getattr("Point")
                .expect("Point should exist on owner module");
            let point = unsafe { ffi::PyObject_CallNoArgs(point_type.as_ptr()) };
            assert!(!point.is_null(), "Point() should create a test instance");
            unsafe { ffi::Py_INCREF(point) };
            let value = unsafe { ffi::PyLong_FromLong(7_654_321) };
            assert!(!value.is_null(), "test value should allocate");

            let mut function_context = test_function_jit_context(&runtime, std::ptr::null_mut());
            let thread_state = unsafe { ffi::PyThreadState_Get() }.cast::<c_void>();
            let result = unsafe {
                entry(
                    std::ptr::addr_of_mut!(function_context).cast(),
                    thread_state,
                    point.cast(),
                    value.cast(),
                )
            };
            assert!(
                !result.is_null(),
                "write_point should return the stored value"
            );

            assert_eq!(
                shared_state.counter_branch_value(hit_counter_id, hit_branch_id),
                1,
                "strict v3 SetAttr should take the indexed-store fast path"
            );
            assert_eq!(
                shared_state.counter_branch_value(fallback_counter_id, fallback_branch_id),
                0,
                "strict v3 SetAttr should avoid selected indexed fallback"
            );
            assert_eq!(
                shared_state.counter_branch_value(generic_counter_id, generic_branch_id),
                0,
                "strict v3 SetAttr should avoid the generic setattr path"
            );

            let point_obj = unsafe { pyo3::Bound::from_borrowed_ptr(py, point) };
            let stored = point_obj
                .getattr("x")
                .expect("Point instance should now expose x");
            assert_eq!(
                stored.extract::<i64>().expect("stored x should be an int"),
                7_654_321
            );
            let result_obj = unsafe { pyo3::Bound::from_owned_ptr(py, result.cast()) };
            assert_eq!(
                result_obj
                    .extract::<i64>()
                    .expect("write_point result should be an int"),
                7_654_321
            );

            unsafe { ffi::Py_DECREF(point) };
            modules
                .del_item("field_type_test")
                .expect("owner module should be removed");
        });
    }

    #[test]
    fn v3_indexed_field_annotation_preserves_mechanical_plan_source() {
        let module_name_gen = ModuleNameGen::new(0);
        let mut constants = TestConstantPool::default();
        let mut function = test_function_in_module(&module_name_gen, "write");
        function.params = ParamSpec {
            params: vec![
                Param {
                    name: "obj".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
                Param {
                    name: "value".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
            ],
        };
        let block_label = function.name_gen.next_block_name();
        let setattr_instr_id = InstrId::new(block_label, 1);
        function.blocks = vec![CodegenBlock {
            label: block_label,
            body: vec![with_instr_id(
                op_expr(SetAttr::new(
                    name_expr(test_name("obj")),
                    constants.string_expr("x"),
                    name_expr(test_name("value")),
                )),
                setattr_instr_id,
            )],
            term: ret_term(none_expr()),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        }];
        set_stack_slots(&mut function, &["obj", "value"]);

        let mut typed_function =
            lower_typed_function_if_tests_to_truthy(lower_codegen_function_to_typed(function));
        let opt_v3_indexed_fields_by_instr = HashMap::from([(
            setattr_instr_id,
            vec![OptV3ResolvedIndexedFieldAccess {
                access: IndexedFieldAccessKind::Store,
                attr_name: "x".to_string(),
                guard: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                fallback: IndexedFieldFallbackKind::OriginalAttrAccess,
                specialization: FieldIndexSpecialization {
                    expected_index: 0,
                    owner_type_ref: RelocTypeRef::TypeKey(CounterDumpTypeKey {
                        module_name: "field_type_test".to_string(),
                        qualname: "Point".to_string(),
                    }),
                    type_version: 1,
                },
            }],
        )]);

        let annotated = annotate_typed_attr_accesses(
            &mut typed_function,
            &HashMap::new(),
            &HashMap::new(),
            &opt_v3_indexed_fields_by_instr,
            true,
        )
        .expect("v3 indexed-field SetAttr annotation should succeed");
        assert_eq!(annotated, 1);

        let InstrTyped::SetAttrTyped(op) = &typed_function.blocks[0].body[0] else {
            panic!("test function body should contain typed SetAttr");
        };
        let TypedAttrAccessPlan::IndexedField { guards, .. } = &op.access else {
            panic!("v3 SetAttr should be annotated as an indexed-field access");
        };
        assert_eq!(guards.len(), 1);
        assert_eq!(guards[0].expected_index, 0);
    }

    #[test]
    fn v3_indexed_field_annotation_trusts_prevalidated_plan() {
        let module_name_gen = ModuleNameGen::new(0);
        let mut constants = TestConstantPool::default();
        let mut function = test_function_in_module(&module_name_gen, "read");
        function.params = ParamSpec {
            params: vec![Param {
                name: "obj".into(),
                kind: ParamKind::Any,
                has_default: false,
            }],
        };
        let block_label = function.name_gen.next_block_name();
        let getattr_instr_id = InstrId::new(block_label, 1);
        function.blocks = vec![CodegenBlock {
            label: block_label,
            body: vec![],
            term: ret_term(with_instr_id(
                op_expr(GetAttr::new(
                    name_expr(test_name("obj")),
                    constants.string_expr("actual"),
                )),
                getattr_instr_id,
            )),
            params: vec![],
            exc_edge: None,
            extra: Default::default(),
        }];
        set_stack_slots(&mut function, &["obj"]);

        let mut typed_function =
            lower_typed_function_if_tests_to_truthy(lower_codegen_function_to_typed(function));
        let opt_v3_indexed_fields_by_instr = HashMap::from([(
            getattr_instr_id,
            vec![OptV3ResolvedIndexedFieldAccess {
                access: IndexedFieldAccessKind::Load,
                attr_name: "planned".to_string(),
                guard: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                fallback: IndexedFieldFallbackKind::OriginalAttrAccess,
                specialization: FieldIndexSpecialization {
                    expected_index: 0,
                    owner_type_ref: RelocTypeRef::TypeKey(CounterDumpTypeKey {
                        module_name: "field_type_test".to_string(),
                        qualname: "Point".to_string(),
                    }),
                    type_version: 1,
                },
            }],
        )]);

        let annotated = annotate_typed_attr_accesses(
            &mut typed_function,
            &HashMap::new(),
            &HashMap::new(),
            &opt_v3_indexed_fields_by_instr,
            true,
        )
        .expect("JIT annotation should trust the prevalidated v3 indexed-field source");
        assert_eq!(annotated, 1);
    }

    #[test]
    fn planned_precompile_inputs_reject_mismatched_v3_direct_call_emission() {
        let module_name_gen = ModuleNameGen::new(7);
        let caller = test_function_in_module(&module_name_gen, "caller");
        let callee = test_function_in_module(&module_name_gen, "callee");
        let caller_id = caller.function_id;
        let callee_id = callee.function_id;
        let module = test_module(module_name_gen, vec![caller, callee]);
        let source = InstrId::new(BlockLabel::from_index(0), 11);
        let serialized_caller =
            SerializedFunctionId::new(SerializedModuleId::new(0), caller_id.local_function_id());
        let serialized_callee =
            SerializedFunctionId::new(SerializedModuleId::new(0), callee_id.local_function_id());
        let artifacts = ExactIntBranchV3Artifacts {
            plan: ModuleOptimizationPlanV3 {
                module: ModulePlanIdentity {
                    module_name: "test".to_string(),
                    source_hash: 0,
                    cache_identity: "test-cache".to_string(),
                },
                identity_tables: test_plan_identities(
                    "test",
                    0,
                    "test-cache",
                    serialized_caller,
                    "caller",
                    &[],
                ),
                helper_catalog_version: 1,
                cost_model_version: 1,
                functions: vec![soac_opt::plan_v3::FunctionOptimizationPlanV3 {
                    function: FunctionPlanIdentity {
                        function: serialized_caller,
                        debug_name: Some("caller".to_string()),
                    },
                    regions: Vec::new(),
                    scalar_threads: Vec::new(),
                    direct_calls: vec![DirectCallSpecializationPlan {
                        source,
                        target: serialized_callee,
                        arg_plan: PlanV3DirectCallArgPlan {
                            sources: vec![PlanV3DirectCallArgSource::Provided(0)],
                        },
                        body: test_v3_inline_call_body(),
                        reason: "profiled direct call".to_string(),
                    }],
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    deopt_points: Vec::new(),
                    ownership: soac_opt::plan_v3::FunctionOwnershipPlan::default(),
                    diagnostics: Vec::new(),
                }],
            },
            emission: MechanicalModuleEmission {
                module_name: "test".to_string(),
                functions: vec![soac_opt::emit_v3::MechanicalFunctionEmission {
                    function: serialized_caller,
                    debug_name: Some("caller".to_string()),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    scalar_threads: Vec::new(),
                    regions: Vec::new(),
                }],
            },
        };

        let err = match planned_optimization_inputs_from_v3_artifacts_for_codegen_module(
            &artifacts,
            &module,
            artifacts.plan.module.module_name.as_str(),
            artifacts.plan.module.source_hash,
            None,
        ) {
            Ok(_) => panic!("mismatched v3 direct-call emission should be rejected"),
            Err(err) => err,
        };

        assert!(
            err.contains("validate optimization plan v3 artifacts")
                && err.contains("optimization plan v3 emission mismatch"),
            "{err}"
        );
    }

    #[test]
    fn planned_precompile_inputs_reject_mismatched_v3_indexed_field_emission() {
        let module_name_gen = ModuleNameGen::new(7);
        let caller = test_function_in_module(&module_name_gen, "caller");
        let caller_id = caller.function_id;
        let module = test_module(module_name_gen, vec![caller]);
        let source = InstrId::new(BlockLabel::from_index(0), 11);
        let serialized_caller =
            SerializedFunctionId::new(SerializedModuleId::new(0), caller_id.local_function_id());
        let owner_type = IndexedFieldOwnerType {
            module_name: "pkg.model".to_string(),
            qualname: "Record".to_string(),
        };
        let artifacts = ExactIntBranchV3Artifacts {
            plan: ModuleOptimizationPlanV3 {
                module: ModulePlanIdentity {
                    module_name: "test".to_string(),
                    source_hash: 0,
                    cache_identity: "test-cache".to_string(),
                },
                identity_tables: test_plan_identities(
                    "test",
                    0,
                    "test-cache",
                    serialized_caller,
                    "caller",
                    &[],
                ),
                helper_catalog_version: 1,
                cost_model_version: 1,
                functions: vec![soac_opt::plan_v3::FunctionOptimizationPlanV3 {
                    function: FunctionPlanIdentity {
                        function: serialized_caller,
                        debug_name: Some("caller".to_string()),
                    },
                    regions: Vec::new(),
                    scalar_threads: Vec::new(),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: vec![IndexedFieldSpecializationPlan {
                        source,
                        access: IndexedFieldAccessKind::Load,
                        owner_type,
                        attr_name: "value".to_string(),
                        expected_index: 2,
                        guard: IndexedFieldGuardPlan {
                            kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                        },
                        fallback: IndexedFieldFallbackPlan {
                            kind: IndexedFieldFallbackKind::OriginalAttrAccess,
                        },
                        reason: "profiled type_keys selected this indexed-field layout".to_string(),
                    }],
                    indexed_globals: Vec::new(),
                    deopt_points: Vec::new(),
                    ownership: soac_opt::plan_v3::FunctionOwnershipPlan::default(),
                    diagnostics: Vec::new(),
                }],
            },
            emission: MechanicalModuleEmission {
                module_name: "test".to_string(),
                functions: vec![soac_opt::emit_v3::MechanicalFunctionEmission {
                    function: serialized_caller,
                    debug_name: Some("caller".to_string()),
                    direct_calls: Vec::new(),
                    exact_list_items: Vec::new(),
                    indexed_fields: Vec::new(),
                    indexed_globals: Vec::new(),
                    scalar_threads: Vec::new(),
                    regions: Vec::new(),
                }],
            },
        };

        let err = match planned_optimization_inputs_from_v3_artifacts_for_codegen_module(
            &artifacts,
            &module,
            artifacts.plan.module.module_name.as_str(),
            artifacts.plan.module.source_hash,
            None,
        ) {
            Ok(_) => panic!("mismatched v3 indexed-field emission should be rejected"),
            Err(err) => err,
        };

        assert!(
            err.contains("validate optimization plan v3 artifacts")
                && err.contains("optimization plan v3 emission mismatch"),
            "{err}"
        );
    }

    #[test]
    fn specialized_jit_string_literals_load_from_module_constant_object_symbol() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(constants.string_expr("hello")),
        );
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = constants.module_constants;
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let built =
            build_test_jit_function_with_constants(&module, &function, &blocks, &module_constants);
        assert!(
            !function_contains_iconst_imm(&built.ctx.func, 0x1000),
            "string literal lowering should not bake the placeholder module constant pointer into the function body"
        );
        assert!(
            count_symbolic_global_values(&built.ctx.func) >= module_constants.len(),
            "string literal lowering should reference one symbolic object per module constant"
        );
        assert_eq!(
            count_opcode(&built.ctx.func, ir::Opcode::GlobalValue),
            1,
            "string literal lowering should materialize only the used module constant object"
        );
    }

    #[test]
    fn specialized_jit_constant_locations_load_from_module_constant_object_symbol() {
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Load::new(test_constant_name(0)))),
        );
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = vec![int_literal(7)];
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let built =
            build_test_jit_function_with_constants(&module, &function, &blocks, &module_constants);
        assert!(
            !function_contains_iconst_imm(&built.ctx.func, 0x1000),
            "constant object lowering should not bake the placeholder module constant pointer into the function body"
        );
        assert!(
            count_symbolic_global_values(&built.ctx.func) >= module_constants.len(),
            "constant object lowering should reference one symbolic object per module constant"
        );
        assert_eq!(
            count_opcode(&built.ctx.func, ir::Opcode::GlobalValue),
            1,
            "constant object lowering should materialize only the used module constant object"
        );
    }

    #[test]
    fn specialized_jit_scalar_counters_load_from_counter_table_symbol() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            crate::initialize_test_python();
            Python::attach(|py| {
                let source = r#"
def f():
    return None
"#;
                let mut baseline = soac_lowering::lower_python_to_blockpy_for_testing(source)
                    .expect("lowering should succeed")
                    .codegen_module;
                let baseline_function = baseline
                    .callable_defs
                    .iter()
                    .find(|function| function.names.bind_name == "f")
                    .expect("missing lowered function f")
                    .clone();
                let baseline_blocks =
                    vec![std::ptr::null_mut::<c_void>(); baseline_function.blocks.len()];
                let baseline_module_constants =
                    crate::module_constants::ModuleCodegenConstants::collect_from_module(&baseline);
                let baseline_built = build_test_jit_function_with_constants(
                    &baseline,
                    &baseline_function,
                    baseline_blocks.as_slice(),
                    &baseline_module_constants,
                );
                let baseline_symbolic_globals =
                    count_symbolic_global_values(&baseline_built.ctx.func);

                instrument_module_with_legacy_block_entry_counters(&mut baseline);

                let shared_state = crate::module_type::build_shared_state_for_testing(
                    py,
                    baseline,
                    "counter_test",
                    "",
                )
                .expect("shared state should build");
                let function = shared_state
                    .lowered_module
                    .callable_defs
                    .iter()
                    .find(|function| function.names.bind_name == "f")
                    .expect("missing instrumented function f")
                    .clone();
                let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
                let compile_session = crate::session::CompileSession::new();
                let mut jit_module =
                    new_jit_module(&compile_session).expect("test jit module should construct");
                let module_constant_ptrs = shared_state.module_constant_ptrs();
                let module_constant_object_data_ids = declare_module_constant_object_data(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    &module_constant_ptrs,
                )
                .expect("module constant object data should declare");
                let scalar_counter_ptr = shared_state.scalar_counter_values_ptr() as i64;
                assert_ne!(
                    scalar_counter_ptr, 0,
                    "instrumented module should allocate scalar counter storage"
                );
                let scalar_counter_symbol = scalar_counter_storage_symbol_for_instance(
                    &shared_state.lowered_module,
                    shared_state.storage_instance_key(),
                );
                let scalar_counter_data_id = Some(
                    declare_scalar_counter_storage_import(
                        &mut jit_module,
                        scalar_counter_symbol.as_str(),
                    )
                    .expect("scalar counter storage import should declare"),
                );
                let top_value_counter_data_id = declare_shared_state_top_value_counter_storage(
                    &mut jit_module,
                    shared_state.as_ref(),
                );
                let built = build_test_cranelift_run_bb_specialized_function(
                    &mut jit_module,
                    blocks.as_slice(),
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    module_constant_object_data_ids.as_slice(),
                    shared_state.counter_slots_by_id(),
                    scalar_counter_data_id,
                    top_value_counter_data_id,
                    &compile_session,
                    Some(shared_state.as_ref()),
                    None,
                    None,
                    BuildSpecializedFunctionOptions::default(),
                )
                .expect("specialized JIT build should succeed");

                assert!(
                    count_symbolic_global_values(&built.ctx.func) >= baseline_symbolic_globals + 1,
                    "scalar counter lowering should add a symbolic counter table reference"
                );
                assert!(
                    !function_contains_iconst_imm(&built.ctx.func, scalar_counter_ptr),
                    "scalar counter lowering should not bake the shared-state counter base pointer into the function body"
                );
            });
        }
    }

    #[test]
    fn specialized_jit_top_value_counters_load_from_counter_table_symbol() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            crate::initialize_test_python();
            Python::attach(|py| {
                let source = r#"
def f(x, y):
    return x + y
"#;
                let baseline = soac_lowering::lower_python_to_blockpy_for_testing(source)
                    .expect("lowering should succeed")
                    .codegen_module;
                let baseline_function = baseline
                    .callable_defs
                    .iter()
                    .find(|function| function.names.bind_name == "f")
                    .expect("missing lowered function f")
                    .clone();
                let baseline_blocks =
                    vec![std::ptr::null_mut::<c_void>(); baseline_function.blocks.len()];
                let baseline_module_constants =
                    crate::module_constants::ModuleCodegenConstants::collect_from_module(&baseline);
                let baseline_built = build_test_jit_function_with_constants(
                    &baseline,
                    &baseline_function,
                    baseline_blocks.as_slice(),
                    &baseline_module_constants,
                );
                let baseline_symbolic_globals =
                    count_symbolic_global_values(&baseline_built.ctx.func);

                let mut instrumented = soac_lowering::lower_python_to_blockpy_for_testing(source)
                    .expect("lowering should succeed")
                    .codegen_module;
                instrument_module_with_legacy_call_target_counters(&mut instrumented);
                let shared_state = crate::module_type::build_shared_state_for_testing(
                    py,
                    instrumented,
                    "counter_test",
                    "",
                )
                .expect("shared state should build");
                let function = shared_state
                    .lowered_module
                    .callable_defs
                    .iter()
                    .find(|function| function.names.bind_name == "f")
                    .expect("missing instrumented function f")
                    .clone();
                let operator_counter = shared_state
                    .lowered_module
                    .counter_defs
                    .iter()
                    .find(|counter| {
                        counter.kind == "operator_hot_shapes"
                            && matches!(
                                counter.site,
                                CounterSite::Runtime {
                                    function_id: Some(id),
                                    ..
                                } if id == function.function_id
                            )
                    })
                    .expect("instrumented module should have operator hot-shape counter");
                let CounterRuntimeSlot::TopValues(counter_slot) =
                    shared_state.counter_slots_by_id()[operator_counter.id.0]
                else {
                    panic!("operator hot-shape counter should use top-value storage");
                };

                let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
                let compile_session = crate::session::CompileSession::new();
                let mut jit_module =
                    new_jit_module(&compile_session).expect("test jit module should construct");
                let module_constant_ptrs = shared_state.module_constant_ptrs();
                let module_constant_object_data_ids = declare_module_constant_object_data(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    &module_constant_ptrs,
                )
                .expect("module constant object data should declare");
                let top_value_counter_base_ptr = shared_state.top_value_counter_values_ptr();
                assert!(
                    !top_value_counter_base_ptr.is_null(),
                    "instrumented module should allocate top-value counter storage"
                );
                let top_value_counter_ptr = top_value_counter_base_ptr.cast::<u8>().wrapping_add(
                    counter_slot
                        .checked_mul(size_of::<crate::counter::TopValueCounter>())
                        .expect("top-value counter byte offset should fit"),
                ) as i64;
                let top_value_counter_data_id = declare_shared_state_top_value_counter_storage(
                    &mut jit_module,
                    shared_state.as_ref(),
                );
                let built = build_test_cranelift_run_bb_specialized_function(
                    &mut jit_module,
                    blocks.as_slice(),
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    module_constant_object_data_ids.as_slice(),
                    shared_state.counter_slots_by_id(),
                    None,
                    top_value_counter_data_id,
                    &compile_session,
                    Some(shared_state.as_ref()),
                    None,
                    None,
                    BuildSpecializedFunctionOptions::default(),
                )
                .expect("specialized JIT build should succeed");

                assert!(
                    count_symbolic_global_values(&built.ctx.func) >= baseline_symbolic_globals + 1,
                    "top-value counter lowering should add a symbolic counter table reference"
                );
                assert!(
                    !function_contains_iconst_imm(&built.ctx.func, top_value_counter_ptr),
                    "top-value counter lowering should not bake the shared-state counter slot pointer into the function body"
                );
            });
        }
    }

    #[test]
    fn render_specialized_jit_pow_calls_use_pynumber_power() {
        let blocks = [1usize as ObjPtr];
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(BinOp::new(
                BinOpKind::Pow,
                name_expr(test_name("a")),
                name_expr(test_local_name("b", 1)),
            ))),
        );
        set_stack_slots(&mut function, &["a", "b"]);
        let rendered = render_test_jit_function_with_module_constants(&function, &blocks, vec![]);
        assert!(
            rendered.contains("call PyNumber_Power"),
            "power lowering should use PyNumber_Power in rendered CLIF:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_inplace_pow_calls_use_pynumber_inplace_power() {
        let blocks = [1usize as ObjPtr];
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(BinOp::new(
                BinOpKind::InplacePow,
                name_expr(test_name("a")),
                name_expr(test_local_name("b", 1)),
            ))),
        );
        set_stack_slots(&mut function, &["a", "b"]);
        let rendered = render_test_jit_function_with_module_constants(&function, &blocks, vec![]);
        assert!(
            rendered.contains("call PyNumber_InPlacePower"),
            "inplace power lowering should use PyNumber_InPlacePower in rendered CLIF:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_skips_unused_function_state_slots() {
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
            !rendered.contains("explicit_slot 8"),
            "unused storage-layout locals should not allocate stack slots:\n{rendered}"
        );
    }

    #[test]
    fn specialization_profile_loads_serialized_v3_artifact_in_verify_mode() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialization_profile_loads_serialized_v3_artifact_in_verify_mode",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let soac_work_dir = fresh_test_work_dir("strict-v3-runtime-v3-plan");
            let module_cache_root = soac_work_dir.join("modules");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let _opt_mode = set_opt_mode("verify");
            let module_name = "strict_v3_runtime_v3_plan_test";
            let module_name_gen = ModuleNameGen::new(0);
            let function = with_single_test_block(
                test_function_in_module(&module_name_gen, "target"),
                vec![],
                ret_term(none_expr()),
            );
            let function_id = function.function_id;
            let module = test_module(module_name_gen, vec![function]);
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, module, module_name, "")
                    .expect("shared state should build");
            let current_function = shared_state
                .lookup_function(function_id)
                .expect("test function should be present in shared state");
            let cache_identity = pre_optimization_module_cache_identity(
                env!("SOAC_BUILD_IDENTITY"),
                shared_state.module_name == "soac.runtime",
            );
            let artifacts = test_empty_v3_artifacts_for_function(
                module_name,
                shared_state.source_hash,
                cache_identity.as_str(),
                0,
                current_function,
            );
            write_test_optimization_artifacts_v3_for_shared_state(
                module_cache_root.as_path(),
                PythonModuleCacheSource::Project,
                shared_state.as_ref(),
                &artifacts,
            );

            let profile = SpecializationProfile::from_runtime_state_with_session(
                Some(shared_state.as_ref()),
                None,
            )
            .expect("verify mode should load serialized runtime artifacts");
            assert!(
                profile.optimized_module.is_some(),
                "serialized v3 artifacts should load the paired optimized BlockPy module"
            );
            assert!(
                existing_counter_dump_path(profile.counter_dump_path.as_deref()).is_none(),
                "test should prove the profile is not relying on a profile.bin fallback"
            );
            let function_artifacts = profile
                .opt_v3_exact_int_branch_artifacts
                .get(&function_id)
                .expect("v3 profile should include the current function");
            assert_eq!(function_artifacts.plan.functions.len(), 1);
            assert_eq!(function_artifacts.emission.functions.len(), 1);
            assert_eq!(
                function_artifacts.plan.functions[0]
                    .function
                    .function
                    .local_function_id(),
                function_id.local_function_id()
            );
        });
    }

    #[test]
    fn runtime_typed_v3_pipeline_bypasses_serialized_v3_artifacts_in_verify_mode() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "runtime_typed_v3_pipeline_bypasses_serialized_v3_artifacts_in_verify_mode",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let soac_work_dir = fresh_test_work_dir("runtime-typed-v3-identity");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let _opt_mode = set_opt_mode("verify");
            let _pipeline = EnvVarGuard::set("SOAC_OPT_RUNTIME_PIPELINE", "typed-v3");
            let module_name = "runtime_typed_v3_identity_test";
            let module_name_gen = ModuleNameGen::new(0);
            let function = with_single_test_block(
                test_function_in_module(&module_name_gen, "target"),
                vec![],
                ret_term(none_expr()),
            );
            let module = test_module(module_name_gen, vec![function]);
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, module, module_name, "")
                    .expect("shared state should build");

            let profile = SpecializationProfile::from_runtime_state_with_session(
                Some(shared_state.as_ref()),
                None,
            )
            .expect("typed-v3 runtime pipeline should not require serialized v3 artifacts");
            assert!(
                profile.optimized_module.is_none(),
                "typed-v3 runtime should use the pre-optimization BlockPy module"
            );
            assert!(
                profile.counter_dump_path.is_none(),
                "typed-v3 runtime should not enable legacy profile evidence fallback"
            );
            assert!(profile.opt_v3_emitted_direct_calls.is_empty());
            assert!(profile.opt_v3_emitted_exact_list_items.is_empty());
            assert!(profile.opt_v3_emitted_indexed_fields.is_empty());
            assert!(profile.opt_v3_emitted_indexed_globals.is_empty());
            assert!(profile.opt_v3_exact_int_branch_artifacts.is_empty());
            assert!(!profile.behavior_change_indexed_stores);
            assert!(!profile.guard_miss_deopt);

            let module_plan = build_typed_v3_jit_module_plan(
                &shared_state.lowered_module,
                None,
                &typed_v3_env_config(),
            )
            .expect("typed-v3 runtime should lower CodegenModuleShape to typed JIT");
            assert_eq!(module_plan.module.callable_defs.len(), 1);
            assert_eq!(
                module_plan.module.module_name_gen.module_id(),
                shared_state.lowered_module.module_name_gen.module_id()
            );
        });
    }

    #[test]
    fn runtime_typed_v3_pipeline_emits_direct_calls_from_raw_profile_evidence() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "runtime_typed_v3_pipeline_emits_direct_calls_from_raw_profile_evidence",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let soac_work_dir = fresh_test_work_dir("runtime-typed-v3-direct-call");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let _opt_mode = set_opt_mode("verify");
            let _pipeline = EnvVarGuard::set("SOAC_OPT_RUNTIME_PIPELINE", "typed-v3");

            let module_name = "runtime_typed_v3_direct_call_test";
            let module_name_gen = ModuleNameGen::new(0);
            let mut callee_function = test_function_in_module(&module_name_gen, "callee");
            callee_function.params.params.push(Param {
                name: "x".into(),
                kind: ParamKind::Any,
                has_default: false,
            });
            callee_function = with_single_test_block(
                callee_function,
                vec![],
                ret_term(name_expr(test_local_name("x", 0))),
            );
            set_stack_slots(&mut callee_function, &["x"]);

            let mut caller_function = test_function_in_module(&module_name_gen, "caller");
            caller_function.params.params.extend([
                Param {
                    name: "fn".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
                Param {
                    name: "x".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                },
            ]);
            let caller_block_label = caller_function.name_gen.next_block_name();
            let call_instr_id = InstrId::new(caller_block_label, 1);
            caller_function = with_test_blocks(
                caller_function,
                vec![CodegenBlock {
                    label: caller_block_label,
                    body: vec![assign_stmt(
                        test_local_name("y", 2),
                        with_instr_id(
                            op_expr(Call::new(
                                name_expr(test_local_name("fn", 0)),
                                vec![CallArgPositional::Positional(name_expr(test_local_name(
                                    "x", 1,
                                )))],
                                Vec::<CallArgKeyword<InstrCodegen>>::new(),
                            )),
                            call_instr_id,
                        ),
                    )],
                    term: ret_term(name_expr(test_local_name("y", 2))),
                    params: vec![],
                    exc_edge: None,
                    extra: Default::default(),
                }],
            );
            set_stack_slots(&mut caller_function, &["fn", "x", "y"]);

            let caller_id = caller_function.function_id;
            let callee_id = callee_function.function_id;
            write_test_counter_dump(
                soac_work_dir.join("profile.bin").as_path(),
                &CounterDumpRecord {
                    source_hash: 0,
                    module_name: module_name.to_string(),
                    package_name: None,
                    rows: vec![
                        CounterDumpRow {
                            counter_id: 0,
                            scope: "this".to_string(),
                            kind: "call_hot_targets".to_string(),
                            site_kind: "runtime".to_string(),
                            function_id: Some(caller_id),
                            current_function_id: Some(caller_id),
                            instr_id: Some(call_instr_id),
                            function_qualname: Some(caller_function.names.qualname.clone()),
                            block_label: None,
                            value: 1,
                            branch_values: Vec::new(),
                            observed_value: Some(callee_id.to_packed_runtime_u64()),
                            max_overcount: Some(0),
                        },
                        CounterDumpRow {
                            counter_id: 1,
                            scope: "this".to_string(),
                            kind: "function_seen".to_string(),
                            site_kind: "runtime".to_string(),
                            function_id: Some(callee_id),
                            current_function_id: Some(callee_id),
                            instr_id: None,
                            function_qualname: Some(callee_function.names.qualname.clone()),
                            block_label: None,
                            value: 1,
                            branch_values: Vec::new(),
                            observed_value: None,
                            max_overcount: Some(0),
                        },
                    ],
                    module_keys: Vec::new(),
                    type_keys: Vec::new(),
                    type_table: Vec::new(),
                },
            );

            let mut module = test_module(module_name_gen, vec![callee_function, caller_function]);
            instrument_module_with_legacy_call_target_counters(&mut module);
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, module, module_name, "")
                    .expect("shared state should build");

            let profile = SpecializationProfile::from_runtime_state_with_session(
                Some(shared_state.as_ref()),
                None,
            )
            .expect("typed-v3 runtime should plan direct calls from raw profile evidence");
            assert!(
                profile.optimized_module.is_none(),
                "typed-v3 runtime should not require an optimized BlockPy artifact"
            );
            assert!(
                profile.counter_dump_path.is_none(),
                "typed-v3 direct-call planning should not enable legacy profile evidence fallback"
            );
            let direct_calls = profile
                .opt_v3_emitted_direct_calls
                .get(&caller_id)
                .and_then(|calls| calls.get(&call_instr_id))
                .expect("typed-v3 runtime should retain the profiled direct-call candidate");
            assert_eq!(direct_calls[0].target, callee_id);
            assert_eq!(
                direct_calls[0].body.kind,
                CallBodyKind::Inline,
                "the raw v3 planner can still record that inline won the body cost model"
            );

            let module_plan = build_typed_v3_jit_module_plan(
                &shared_state.lowered_module,
                Some(&profile),
                &typed_v3_env_config(),
            )
            .expect("typed-v3 runtime should lower the cached pre-opt module to typed JIT");
            let planned_caller = module_plan
                .module
                .callable_defs
                .iter()
                .find(|function| function.function_id == caller_id)
                .expect("planned module should include caller");
            assert_eq!(
                profile
                    .typed_inline_direct_calls(caller_id)
                    .get(&call_instr_id)
                    .map(Vec::len),
                Some(1),
                "typed-v3 should preserve inline-winning v3 body decisions for typed inlining"
            );

            let guarded_callable_calls = count_typed_instrs(planned_caller, |expr| {
                matches!(expr, InstrTyped::GuardedCallableCallTyped(_))
            });
            let generic_typed_calls = count_typed_instrs(planned_caller, |expr| {
                matches!(expr, InstrTyped::CallTyped(_))
            });
            let direct_call_guard_tests = planned_caller
                .blocks
                .iter()
                .filter(|block| {
                    matches!(
                        &block.term,
                        BlockTerm::IfTerm(term)
                            if matches!(term.test, InstrTyped::DirectCallGuardTest(_))
                    )
                })
                .count();
            assert_eq!(
                guarded_callable_calls, 0,
                "inline-winning typed-v3 direct calls should be expanded into typed CFG instead of a local guarded call expression"
            );
            assert_eq!(
                direct_call_guard_tests, 1,
                "typed-v3 inlining should expose the direct-call guard as typed CFG"
            );
            assert_eq!(
                generic_typed_calls, 1,
                "typed-v3 inlining should keep a generic fallback call"
            );
        });
    }

    #[test]
    fn runtime_typed_v3_module_plan_carries_non_inline_direct_call_shape() {
        let module_name = "runtime_typed_v3_non_inline_direct_call_test";
        let module_name_gen = ModuleNameGen::new(0);
        let mut callee_function = test_function_in_module(&module_name_gen, "callee");
        callee_function.params.params.push(Param {
            name: "x".into(),
            kind: ParamKind::Any,
            has_default: false,
        });
        callee_function = with_single_test_block(
            callee_function,
            vec![],
            ret_term(name_expr(test_local_name("x", 0))),
        );
        set_stack_slots(&mut callee_function, &["x"]);

        let mut caller_function = test_function_in_module(&module_name_gen, "caller");
        caller_function.params.params.extend([
            Param {
                name: "fn".into(),
                kind: ParamKind::Any,
                has_default: false,
            },
            Param {
                name: "x".into(),
                kind: ParamKind::Any,
                has_default: false,
            },
        ]);
        let caller_block_label = caller_function.name_gen.next_block_name();
        let call_instr_id = InstrId::new(caller_block_label, 1);
        caller_function = with_test_blocks(
            caller_function,
            vec![CodegenBlock {
                label: caller_block_label,
                body: vec![assign_stmt(
                    test_local_name("y", 2),
                    with_instr_id(
                        op_expr(Call::new(
                            name_expr(test_local_name("fn", 0)),
                            vec![CallArgPositional::Positional(name_expr(test_local_name(
                                "x", 1,
                            )))],
                            Vec::<CallArgKeyword<InstrCodegen>>::new(),
                        )),
                        call_instr_id,
                    ),
                )],
                term: ret_term(name_expr(test_local_name("y", 2))),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            }],
        );
        set_stack_slots(&mut caller_function, &["fn", "x", "y"]);

        let caller_id = caller_function.function_id;
        let callee_id = callee_function.function_id;
        let module = test_module(module_name_gen, vec![callee_function, caller_function]);
        let profile = SpecializationProfile {
            module_name: Some(module_name),
            counter_dump_path: None,
            optimized_module: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::from([(
                caller_id,
                HashMap::from([(
                    call_instr_id,
                    vec![ResolvedV3DirectCallPlan {
                        source: call_instr_id,
                        target: callee_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![TypedDirectCallArgSource::Provided(0)],
                        },
                        body: CallBodyPlan {
                            kind: CallBodyKind::DirectCall,
                            cost: Cost::default(),
                            inline_target: None,
                            reason: "test keeps body as direct call".to_string(),
                        },
                        reason: "test direct-call candidate".to_string(),
                    }],
                )]),
            )]),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };

        let module_plan =
            build_typed_v3_jit_module_plan(&module, Some(&profile), &typed_v3_env_config())
                .expect("typed-v3 module plan should lower non-inline direct calls");
        let planned_caller = module_plan
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == caller_id)
            .expect("planned module should include caller");
        assert_eq!(
            count_typed_instrs(planned_caller, |expr| {
                matches!(expr, InstrTyped::GuardedCallableCallTyped(_))
            }),
            1,
            "non-inline typed-v3 direct calls should be represented in InstrTyped before codegen"
        );
        assert_eq!(
            count_typed_instrs(planned_caller, |expr| {
                matches!(expr, InstrTyped::CallTyped(_))
            }),
            0,
            "the selected call site should no longer remain a generic typed call"
        );
        assert_eq!(
            count_typed_instrs(planned_caller, |expr| {
                matches!(expr, InstrTyped::DirectCallGuardTest(_))
            }),
            0,
            "non-inline direct calls should not be expanded into inline guard CFG"
        );
        let planned_direct_targets =
            collect_planned_typed_call_direct_targets(&module_plan, caller_id)
                .expect("planned module should expose direct-call dependency targets");
        assert!(
            planned_direct_targets.contains(&callee_id),
            "typed-v3 worker dependencies should come from the planned InstrTyped module"
        );
    }

    #[test]
    fn runtime_typed_v3_pipeline_keeps_access_plans_from_raw_profile_evidence() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "runtime_typed_v3_pipeline_keeps_access_plans_from_raw_profile_evidence",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let soac_work_dir = fresh_test_work_dir("runtime-typed-v3-access-plans");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let _opt_mode = set_opt_mode("verify");
            let _pipeline = EnvVarGuard::set("SOAC_OPT_RUNTIME_PIPELINE", "typed-v3");

            let module_name = "runtime_typed_v3_access_plan_test";
            let module_name_gen = ModuleNameGen::new(0);
            let mut function = test_function_in_module(&module_name_gen, "load_global");
            let block_label = function.name_gen.next_block_name();
            let load_instr_id = InstrId::new(block_label, 0);
            function = with_test_blocks(
                function,
                vec![CodegenBlock {
                    label: block_label,
                    body: Vec::new(),
                    term: ret_term(with_instr_id(
                        op_expr(Load::new(test_global_name("x"))),
                        load_instr_id,
                    )),
                    params: Vec::new(),
                    exc_edge: None,
                    extra: Default::default(),
                }],
            );
            let function_id = function.function_id;

            write_test_counter_dump(
                soac_work_dir.join("profile.bin").as_path(),
                &CounterDumpRecord {
                    source_hash: 0,
                    module_name: module_name.to_string(),
                    package_name: None,
                    rows: Vec::new(),
                    module_keys: vec![CounterDumpKeyLayout {
                        owner: module_name.to_string(),
                        key: "x".to_string(),
                        index: 0,
                    }],
                    type_keys: Vec::new(),
                    type_table: Vec::new(),
                },
            );

            let mut module = test_module(module_name_gen, vec![function]);
            module.global_names = vec!["x".to_string()];
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, module, module_name, "")
                    .expect("shared state should build");

            let profile = SpecializationProfile::from_runtime_state_with_session(
                Some(shared_state.as_ref()),
                None,
            )
            .expect("typed-v3 runtime should plan access emissions from raw profile evidence");
            assert!(
                profile.optimized_module.is_none(),
                "typed-v3 runtime should not require an optimized BlockPy artifact"
            );
            assert!(
                profile.counter_dump_path.is_none(),
                "typed-v3 access planning should not enable legacy profile evidence fallback"
            );
            let indexed_global = profile
                .opt_v3_emitted_indexed_globals
                .get(&function_id)
                .and_then(|globals| globals.get(&load_instr_id))
                .expect("typed-v3 runtime should retain indexed-global plans from raw evidence");
            assert_eq!(indexed_global.access, IndexedGlobalAccessKind::Load);
            assert_eq!(indexed_global.module_name, module_name);
            assert_eq!(indexed_global.name, "x");
            assert_eq!(indexed_global.expected_index, 0);

            let planned_function = shared_state
                .lowered_module
                .callable_defs
                .iter()
                .find(|function| function.function_id == function_id)
                .expect("lowered module should include the test function");
            let specialization_inputs =
                FunctionSpecializationInputs::from_profile(&profile, planned_function)
                    .expect("typed-v3 access plans should become specialization inputs");
            assert!(
                specialization_inputs.legacy_overlays.is_none(),
                "typed-v3 runtime should keep indexed-global access plans embedded in InstrTyped"
            );
        });
    }

    #[test]
    fn runtime_typed_v3_module_plan_carries_indexed_field_access_shape() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "runtime_typed_v3_module_plan_carries_indexed_field_access_shape",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let owner_module = PyModule::from_code(
                py,
                c"
class Point:
    pass
",
                c"field_type_test.py",
                c"field_type_test",
            )
            .expect("owner module should execute");
            let sys = PyModule::import(py, "sys").expect("sys should import");
            let modules = sys
                .getattr("modules")
                .expect("sys.modules should exist")
                .cast_into::<pyo3::types::PyDict>()
                .expect("sys.modules should be a dict");
            modules
                .set_item("field_type_test", owner_module.as_any())
                .expect("owner module should be registered");

            let module_name = "runtime_typed_v3_indexed_field_plan_test";
            let module_name_gen = ModuleNameGen::new(0);
            let mut constants = TestConstantPool::default();
            let mut function = test_function_in_module(&module_name_gen, "read_point");
            function.params = ParamSpec {
                params: vec![Param {
                    name: "point".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                }],
            };
            let block_label = function.name_gen.next_block_name();
            let setattr_instr_id = InstrId::new(block_label, 0);
            let getattr_instr_id = InstrId::new(block_label, 1);
            function.blocks = vec![CodegenBlock {
                label: block_label,
                body: vec![with_instr_id(
                    op_expr(SetAttr::new(
                        name_expr(test_name("point")),
                        constants.string_expr("x"),
                        constants.int_expr(7),
                    )),
                    setattr_instr_id,
                )],
                term: ret_term(with_instr_id(
                    op_expr(GetAttr::new(
                        name_expr(test_name("point")),
                        constants.string_expr("x"),
                    )),
                    getattr_instr_id,
                )),
                params: Vec::new(),
                exc_edge: None,
                extra: Default::default(),
            }];
            set_stack_slots(&mut function, &["point"]);
            let function_id = function.function_id;
            let mut module = test_module(module_name_gen, vec![function]);
            module.module_constants = constants.module_constants;

            let profile = SpecializationProfile {
                module_name: Some(module_name),
                counter_dump_path: None,
                optimized_module: None,
                direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
                opt_v3_emitted_direct_calls: HashMap::new(),
                opt_v3_emitted_exact_list_items: HashMap::new(),
                opt_v3_emitted_indexed_fields: HashMap::from([(
                    function_id,
                    HashMap::from([
                        (
                            getattr_instr_id,
                            vec![OptV3IndexedFieldAccessPlan {
                                access: IndexedFieldAccessKind::Load,
                                guard: MechanicalIndexedFieldGuard {
                                    kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                                    owner_type: IndexedFieldOwnerType {
                                        module_name: "field_type_test".to_string(),
                                        qualname: "Point".to_string(),
                                    },
                                    attr_name: "x".to_string(),
                                    expected_index: 0,
                                },
                                fallback: IndexedFieldFallbackKind::OriginalAttrAccess,
                            }],
                        ),
                        (
                            setattr_instr_id,
                            vec![OptV3IndexedFieldAccessPlan {
                                access: IndexedFieldAccessKind::Store,
                                guard: MechanicalIndexedFieldGuard {
                                    kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                                    owner_type: IndexedFieldOwnerType {
                                        module_name: "field_type_test".to_string(),
                                        qualname: "Point".to_string(),
                                    },
                                    attr_name: "x".to_string(),
                                    expected_index: 0,
                                },
                                fallback: IndexedFieldFallbackKind::OriginalAttrAccess,
                            }],
                        ),
                    ]),
                )]),
                opt_v3_emitted_indexed_globals: HashMap::new(),
                opt_v3_exact_int_branch_artifacts: HashMap::new(),
                behavior_change_indexed_stores: false,
                profiled_cold_blocks: false,
                guard_miss_deopt: false,
            };

            let module_plan =
                build_typed_v3_jit_module_plan(&module, Some(&profile), &typed_v3_env_config())
                    .expect("typed-v3 module plan should attach indexed-field access plans");
            let planned_function = module_plan
                .module
                .callable_defs
                .iter()
                .find(|function| function.function_id == function_id)
                .expect("planned module should include read_point");
            let [InstrTyped::SetAttrTyped(setattr)] = planned_function.blocks[0].body.as_slice()
            else {
                panic!("read_point should keep an indexed typed SetAttr in the body");
            };
            let TypedAttrAccessPlan::IndexedField { source, guards } = &setattr.access else {
                panic!("typed-v3 module plan should carry indexed-field store shape");
            };
            assert_eq!(*source, TypedIndexedFieldPlanSource::OptimizationPlanV3);
            assert_eq!(guards.len(), 1);
            assert_eq!(guards[0].expected_index, 0);

            let BlockTerm::Return(InstrTyped::GetAttrTyped(op)) = &planned_function.blocks[0].term
            else {
                panic!("read_point should still return a typed GetAttr");
            };
            let TypedAttrAccessPlan::IndexedField { source, guards } = &op.access else {
                panic!("typed-v3 module plan should carry indexed-field access shape");
            };
            assert_eq!(*source, TypedIndexedFieldPlanSource::OptimizationPlanV3);
            assert_eq!(guards.len(), 1);
            assert_eq!(guards[0].expected_index, 0);

            modules
                .del_item("field_type_test")
                .expect("owner module should be removed");
        });
    }

    #[test]
    fn runtime_typed_v3_module_plan_carries_indexed_global_access_shape() {
        let module_name = "runtime_typed_v3_indexed_global_plan_test";
        let module_name_gen = ModuleNameGen::new(0);
        let mut function = test_function_in_module(&module_name_gen, "update_counter");
        let block_label = function.name_gen.next_block_name();
        let store_instr_id = InstrId::new(block_label, 0);
        let load_instr_id = InstrId::new(block_label, 1);
        function.blocks = vec![CodegenBlock {
            label: block_label,
            body: vec![with_instr_id(
                assign_stmt(test_global_name("counter"), none_expr()),
                store_instr_id,
            )],
            term: ret_term(with_instr_id(
                name_expr(test_global_name("counter")),
                load_instr_id,
            )),
            params: Vec::new(),
            exc_edge: None,
            extra: Default::default(),
        }];
        let function_id = function.function_id;
        let mut module = test_module(module_name_gen, vec![function]);
        module.global_names = vec!["counter".to_string()];

        let indexed_global_plan = |source, access| OptV3IndexedGlobalAccessPlan {
            source,
            access,
            module_name: module_name.to_string(),
            name: "counter".to_string(),
            expected_index: 0,
            guard: IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
            fallback: IndexedGlobalFallbackKind::OriginalGlobalAccess,
        };
        let profile = SpecializationProfile {
            module_name: Some(module_name),
            counter_dump_path: None,
            optimized_module: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::from([(
                function_id,
                HashMap::from([
                    (
                        store_instr_id,
                        indexed_global_plan(store_instr_id, IndexedGlobalAccessKind::Store),
                    ),
                    (
                        load_instr_id,
                        indexed_global_plan(load_instr_id, IndexedGlobalAccessKind::Load),
                    ),
                ]),
            )]),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };

        let module_plan =
            build_typed_v3_jit_module_plan(&module, Some(&profile), &typed_v3_env_config())
                .expect("typed-v3 module plan should attach indexed-global access plans");
        let planned_function = module_plan
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == function_id)
            .expect("planned module should include update_counter");
        let [InstrTyped::LegacyStore(store)] = planned_function.blocks[0].body.as_slice() else {
            panic!("update_counter should keep the global store as a typed legacy store");
        };
        let store_plan = store
            .extra()
            .indexed_global_access_plan()
            .expect("typed-v3 module plan should carry indexed-global store shape");
        assert_eq!(
            store_plan.source,
            TypedIndexedGlobalPlanSource::OptimizationPlanV3
        );
        assert_eq!(store_plan.instr_id, store_instr_id);
        assert_eq!(store_plan.access, IndexedGlobalAccessKind::Store);
        assert_eq!(store_plan.name, "counter");
        assert_eq!(store_plan.expected_index, 0);

        let BlockTerm::Return(InstrTyped::Load(load)) = &planned_function.blocks[0].term else {
            panic!("update_counter should still return a typed Load");
        };
        let load_plan = load
            .extra()
            .indexed_global_access_plan()
            .expect("typed-v3 module plan should carry indexed-global load shape");
        assert_eq!(
            load_plan.source,
            TypedIndexedGlobalPlanSource::OptimizationPlanV3
        );
        assert_eq!(load_plan.instr_id, load_instr_id);
        assert_eq!(load_plan.access, IndexedGlobalAccessKind::Load);
        assert_eq!(load_plan.name, "counter");
        assert_eq!(load_plan.expected_index, 0);

        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &module.callable_defs[0],
            &[1usize as ObjPtr],
            &module_constants,
            BuildSpecializedFunctionOptions {
                specialization_inputs: Some(FunctionSpecializationInputs {
                    legacy_overlays: None,
                }),
                legacy_call_emission_typed_function: Some(planned_function.clone()),
                ..BuildSpecializedFunctionOptions::default()
            },
        );
        let store_helper = declared_user_names_for_symbols(
            &built,
            &[super::SOAC_RUNTIME_STORE_GLOBAL_INDEXED_SYMBOL],
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &store_helper),
            1,
            "typed-v3 indexed-global store codegen should be driven by the InstrTyped plan, not the legacy sidecar flag"
        );
    }

    #[test]
    fn runtime_typed_v3_module_plan_carries_exact_list_item_access_shape() {
        let module_name = "runtime_typed_v3_exact_list_item_plan_test";
        let module_name_gen = ModuleNameGen::new(0);
        let mut function = test_function_in_module(&module_name_gen, "replace_first");
        function.params = ParamSpec {
            params: vec![
                test_param("items", ParamKind::Any, false),
                test_param("index", ParamKind::Any, false),
            ],
        };
        let block_label = function.name_gen.next_block_name();
        let setitem_instr_id = InstrId::new(block_label, 0);
        let getitem_instr_id = InstrId::new(block_label, 1);
        function.blocks = vec![CodegenBlock {
            label: block_label,
            body: vec![with_instr_id(
                op_expr(SetItem::new(
                    name_expr(test_name("items")),
                    name_expr(test_name("index")),
                    none_expr(),
                )),
                setitem_instr_id,
            )],
            term: ret_term(with_instr_id(
                op_expr(GetItem::new(
                    name_expr(test_name("items")),
                    name_expr(test_name("index")),
                )),
                getitem_instr_id,
            )),
            params: Vec::new(),
            exc_edge: None,
            extra: Default::default(),
        }];
        set_stack_slots(&mut function, &["items", "index"]);
        let function_id = function.function_id;
        let module = test_module(module_name_gen, vec![function]);

        let exact_list_item_plan = |source, access| OptV3ExactListItemAccessPlan {
            source,
            access,
            shape: PlanV3ExactListItemShape::ExactListExactInt,
            guard: PlanV3ExactListItemGuardKind::ExactListExactCompactIntInBounds,
            fallback: PlanV3ExactListItemFallbackKind::OriginalItemAccess,
        };
        let profile = SpecializationProfile {
            module_name: Some(module_name),
            counter_dump_path: None,
            optimized_module: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::from([(
                function_id,
                HashMap::from([
                    (
                        setitem_instr_id,
                        exact_list_item_plan(setitem_instr_id, PlanV3ExactListItemAccessKind::Set),
                    ),
                    (
                        getitem_instr_id,
                        exact_list_item_plan(getitem_instr_id, PlanV3ExactListItemAccessKind::Get),
                    ),
                ]),
            )]),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };

        let module_plan =
            build_typed_v3_jit_module_plan(&module, Some(&profile), &typed_v3_env_config())
                .expect("typed-v3 module plan should attach exact-list item access plans");
        let planned_function = module_plan
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == function_id)
            .expect("planned module should include replace_first");
        let [InstrTyped::LegacySetItem(setitem)] = planned_function.blocks[0].body.as_slice()
        else {
            panic!("replace_first should keep setitem as a typed legacy SetItem");
        };
        let setitem_plan = setitem
            .extra()
            .exact_list_item_access_plan()
            .expect("typed-v3 module plan should carry exact-list setitem shape");
        assert_eq!(
            setitem_plan.source,
            TypedExactListItemPlanSource::OptimizationPlanV3
        );
        assert_eq!(setitem_plan.instr_id, setitem_instr_id);
        assert_eq!(setitem_plan.access, PlanV3ExactListItemAccessKind::Set);

        let BlockTerm::Return(InstrTyped::LegacyGetItem(getitem)) =
            &planned_function.blocks[0].term
        else {
            panic!("replace_first should still return a typed legacy GetItem");
        };
        let getitem_plan = getitem
            .extra()
            .exact_list_item_access_plan()
            .expect("typed-v3 module plan should carry exact-list getitem shape");
        assert_eq!(
            getitem_plan.source,
            TypedExactListItemPlanSource::OptimizationPlanV3
        );
        assert_eq!(getitem_plan.instr_id, getitem_instr_id);
        assert_eq!(getitem_plan.access, PlanV3ExactListItemAccessKind::Get);
    }

    #[test]
    fn runtime_typed_v3_module_plan_carries_exact_int_selection_shapes() {
        let module_name = "runtime_typed_v3_exact_int_plan_test";
        let module_name_gen = ModuleNameGen::new(0);
        let mut function = test_function_in_module(&module_name_gen, "branch_and_add");
        function.params = ParamSpec {
            params: vec![
                test_param("a", ParamKind::Any, false),
                test_param("b", ParamKind::Any, false),
            ],
        };
        let entry_label = function.name_gen.next_block_name();
        let then_label = function.name_gen.next_block_name();
        let else_label = function.name_gen.next_block_name();
        let branch_add_instr_id = InstrId::new(entry_label, 2);
        let compare_instr_id = InstrId::new(entry_label, 4);
        let return_add_instr_id = InstrId::new(then_label, 2);
        let mut constants = TestConstantPool::default();
        function.blocks = vec![
            CodegenBlock {
                label: entry_label,
                body: Vec::new(),
                term: BlockTerm::IfTerm(soac_core::block_py::TermIf {
                    test: with_instr_id(
                        op_expr(BinOp::new(
                            BinOpKind::Gt,
                            with_instr_id(
                                op_expr(BinOp::new(
                                    BinOpKind::Add,
                                    with_instr_id(
                                        name_expr(test_name("a")),
                                        InstrId::new(entry_label, 0),
                                    ),
                                    with_instr_id(
                                        name_expr(test_local_name("b", 1)),
                                        InstrId::new(entry_label, 1),
                                    ),
                                )),
                                branch_add_instr_id,
                            ),
                            with_instr_id(constants.int_expr(0), InstrId::new(entry_label, 3)),
                        )),
                        compare_instr_id,
                    ),
                    then_label,
                    else_label,
                }),
                params: Vec::new(),
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: then_label,
                body: Vec::new(),
                term: ret_term(with_instr_id(
                    op_expr(BinOp::new(
                        BinOpKind::Add,
                        with_instr_id(name_expr(test_name("a")), InstrId::new(then_label, 0)),
                        with_instr_id(
                            name_expr(test_local_name("b", 1)),
                            InstrId::new(then_label, 1),
                        ),
                    )),
                    return_add_instr_id,
                )),
                params: Vec::new(),
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: else_label,
                body: Vec::new(),
                term: ret_term(constants.int_expr(0)),
                params: Vec::new(),
                exc_edge: None,
                extra: Default::default(),
            },
        ];
        set_stack_slots(&mut function, &["a", "b"]);
        let function_id = function.function_id;
        let mut module = test_module(module_name_gen, vec![function]);
        module.module_constants = constants.module_constants;
        let function = module.callable_defs[0].clone();
        let exact_int_shape = soac_opt::operator_specialization::pack_binary_shape(
            soac_opt::operator_specialization::ExactTypeTag::Int,
            soac_opt::operator_specialization::ExactTypeTag::Int,
        );
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .operator_specializations
            .insert(branch_add_instr_id, vec![exact_int_shape]);
        evidence
            .operator_specializations
            .insert(return_add_instr_id, vec![exact_int_shape]);
        let artifacts = plan_and_emit_function_exact_int_branches_v3_with_module_constants(
            &AlternativeCatalog::default_v3(),
            ModulePlanIdentity {
                module_name: module_name.to_string(),
                source_hash: 0,
                cache_identity: "test-cache".to_string(),
            },
            FunctionPlanIdentity {
                function: SerializedFunctionId::new(
                    SerializedModuleId::new(0),
                    function.function_id.local_function_id(),
                ),
                debug_name: Some(function.names.qualname.clone()),
            },
            &function,
            &evidence,
            module.module_constants.as_slice(),
        )
        .expect("exact-int v3 artifacts should plan for branch and return sources");
        let profile = SpecializationProfile {
            module_name: Some(module_name),
            counter_dump_path: None,
            optimized_module: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::from([(
                function_id,
                std::sync::Arc::new(artifacts),
            )]),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };

        let module_plan =
            build_typed_v3_jit_module_plan(&module, Some(&profile), &typed_v3_env_config())
                .expect("typed-v3 module plan should attach exact-int selections");
        let planned_function = module_plan
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == function_id)
            .expect("planned module should include branch_and_add");
        let BlockTerm::IfTerm(if_term) = &planned_function.blocks[0].term else {
            panic!("entry block should remain a typed if term");
        };
        let branch_plan = if_term
            .test
            .typed_extra()
            .and_then(|extra| extra.exact_int_branch_plan())
            .expect("typed-v3 module plan should carry exact-int branch selection");
        assert_eq!(
            branch_plan.source,
            TypedExactIntPlanSource::OptimizationPlanV3
        );
        assert_eq!(branch_plan.instr_id, compare_instr_id);
        assert_eq!(branch_plan.hot_plan.id, branch_plan.hot_region.region);
        assert_eq!(
            branch_plan
                .hot_region
                .exits
                .first()
                .and_then(|exit| exit.source),
            Some(compare_instr_id)
        );

        let BlockTerm::Return(return_value) = &planned_function.blocks[1].term else {
            panic!("then block should remain a typed return");
        };
        let return_plan = return_value
            .typed_extra()
            .and_then(|extra| extra.exact_int_return_plan())
            .expect("typed-v3 module plan should carry exact-int return selection");
        assert_eq!(
            return_plan.source,
            TypedExactIntPlanSource::OptimizationPlanV3
        );
        assert_eq!(return_plan.instr_id, return_add_instr_id);
        assert_eq!(return_plan.hot_plan.id, return_plan.hot_region.region);
        assert_eq!(
            return_plan
                .hot_region
                .exits
                .first()
                .and_then(|exit| exit.source),
            Some(return_add_instr_id)
        );
    }

    #[test]
    fn runtime_typed_v3_module_plan_carries_exact_int_scalar_thread_shape() {
        let module_name = "runtime_typed_v3_exact_int_scalar_thread_test";
        let module_name_gen = ModuleNameGen::new(0);
        let mut function = test_function_in_module(&module_name_gen, "store_then_compare");
        function.params = ParamSpec {
            params: vec![
                test_param("a", ParamKind::Any, false),
                test_param("b", ParamKind::Any, false),
            ],
        };
        let entry_label = function.name_gen.next_block_name();
        let test_label = function.name_gen.next_block_name();
        let then_label = function.name_gen.next_block_name();
        let else_label = function.name_gen.next_block_name();
        let store_instr_id = InstrId::new(entry_label, 0);
        let add_instr_id = InstrId::new(entry_label, 1);
        let compare_instr_id = InstrId::new(test_label, 0);
        let c_name = test_local_name("c", 2);
        let mut constants = TestConstantPool::default();
        function.blocks = vec![
            CodegenBlock {
                label: entry_label,
                body: vec![with_instr_id(
                    op_expr(Store::new(
                        c_name.clone(),
                        with_instr_id(
                            op_expr(BinOp::new(
                                BinOpKind::Add,
                                name_expr(test_name("a")),
                                name_expr(test_local_name("b", 1)),
                            )),
                            add_instr_id,
                        ),
                    )),
                    store_instr_id,
                )],
                term: BlockTerm::Jump(BlockEdge::new(test_label)),
                params: Vec::new(),
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: test_label,
                body: Vec::new(),
                term: BlockTerm::IfTerm(soac_core::block_py::TermIf {
                    test: with_instr_id(
                        op_expr(BinOp::new(
                            BinOpKind::Gt,
                            name_expr(c_name),
                            constants.int_expr(0),
                        )),
                        compare_instr_id,
                    ),
                    then_label,
                    else_label,
                }),
                params: Vec::new(),
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: then_label,
                body: Vec::new(),
                term: ret_term(name_expr(test_runtime_name("TRUE"))),
                params: Vec::new(),
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: else_label,
                body: Vec::new(),
                term: ret_term(none_expr()),
                params: Vec::new(),
                exc_edge: None,
                extra: Default::default(),
            },
        ];
        set_stack_slots(&mut function, &["a", "b", "c"]);
        let function_id = function.function_id;
        let mut module = test_module(module_name_gen, vec![function]);
        module.module_constants = constants.module_constants;
        let function = module.callable_defs[0].clone();
        let exact_int_shape = soac_opt::operator_specialization::pack_binary_shape(
            soac_opt::operator_specialization::ExactTypeTag::Int,
            soac_opt::operator_specialization::ExactTypeTag::Int,
        );
        let mut evidence = FunctionProfileEvidence::default();
        evidence
            .operator_specializations
            .insert(add_instr_id, vec![exact_int_shape]);
        evidence
            .operator_specializations
            .insert(compare_instr_id, vec![exact_int_shape]);
        let artifacts = plan_and_emit_function_exact_int_branches_v3_with_module_constants(
            &AlternativeCatalog::default_v3(),
            ModulePlanIdentity {
                module_name: module_name.to_string(),
                source_hash: 0,
                cache_identity: "test-cache".to_string(),
            },
            FunctionPlanIdentity {
                function: SerializedFunctionId::new(
                    SerializedModuleId::new(0),
                    function.function_id.local_function_id(),
                ),
                debug_name: Some(function.names.qualname.clone()),
            },
            &function,
            &evidence,
            module.module_constants.as_slice(),
        )
        .expect("exact-int v3 artifacts should plan scalar thread");
        assert_eq!(artifacts.plan.functions[0].scalar_threads.len(), 1);
        let profile = SpecializationProfile {
            module_name: Some(module_name),
            counter_dump_path: None,
            optimized_module: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::from([(
                function_id,
                std::sync::Arc::new(artifacts),
            )]),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };

        let module_plan =
            build_typed_v3_jit_module_plan(&module, Some(&profile), &typed_v3_env_config())
                .expect("typed-v3 module plan should attach scalar-thread selection");
        let planned_function = module_plan
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == function_id)
            .expect("planned module should include store_then_compare");
        let InstrTyped::LegacyStore(store) = &planned_function.blocks[0].body[0] else {
            panic!("entry block should keep a typed producer store");
        };
        let scalar_thread_plan = store
            .extra()
            .exact_int_scalar_thread_plan()
            .expect("typed-v3 module plan should carry exact-int scalar-thread selection");
        assert_eq!(
            scalar_thread_plan.source,
            TypedExactIntPlanSource::OptimizationPlanV3
        );
        assert_eq!(scalar_thread_plan.store_instr_id, store_instr_id);
        assert_eq!(scalar_thread_plan.producer_instr_id, add_instr_id);
        assert_eq!(scalar_thread_plan.consumer_instr_id, compare_instr_id);
        assert_eq!(
            scalar_thread_plan.producer_hot_plan.id,
            scalar_thread_plan.producer_hot_region.region
        );
        assert_eq!(
            scalar_thread_plan.consumer_hot_plan.id,
            scalar_thread_plan.consumer_hot_region.region
        );
        let specialization_inputs =
            FunctionSpecializationInputs::from_profile(&profile, planned_function)
                .expect("typed-v3 scalar thread should not require sidecar inputs");
        assert!(
            specialization_inputs.legacy_overlays.is_none(),
            "typed-v3 scalar-int selection should be represented in InstrTyped, not in FunctionSpecializationInputs"
        );
    }

    #[test]
    fn specialized_jit_assignment_to_direct_entry_param_avoids_stack_mirror() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = with_single_test_block(
            test_function(),
            vec![assign_stmt(test_name("x"), constants.int_expr(7))],
            ret_term(name_expr(test_name("x"))),
        );
        function.params = ParamSpec {
            params: vec![test_param("x", ParamKind::Any, false)],
        };
        set_stack_slots(&mut function, &["x"]);
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = constants.module_constants;
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let built =
            build_test_jit_function_with_constants(&module, &function, &blocks, &module_constants);
        assert_eq!(
            count_opcode(&built.ctx.func, ir::Opcode::StackStore),
            0,
            "direct-entry param assignments should stay in LocalEnv without stack-slot mirroring:\n{}",
            built.ctx.func.display()
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
        let rendered = render_test_jit_function_with_runtime_inline(&function, &blocks, Vec::new());
        assert!(
            !rendered.contains("call dp_jit_function_globals")
                && !rendered.contains("call dp_jit_load_module_constant"),
            "global located names should use mod_ctx-backed globals and module constant names:\n{rendered}"
        );
        assert_inlined_indexed_global_guard(
            &rendered,
            "global located names should inline the indexed-module-dict guard",
        );
    }

    fn assert_indexed_global_guard_miss_targets_cold_deopt_stub(
        function: BlockPyFunction<CodegenModuleShape>,
        case_name: &str,
    ) {
        let blocks = [1usize as ObjPtr];
        let mut module = test_module(ModuleNameGen::new(0), vec![function]);
        instrument_module_with_legacy_call_target_counters(&mut module);
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let specialization_inputs = indexed_global_specialization_inputs_for_function(
            &function,
            IndexedGlobalAccessKind::Load,
            "x",
        );
        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                guard_miss_deopt_stub: true,
                specialization_inputs: Some(specialization_inputs),
                ..BuildSpecializedFunctionOptions::default()
            },
        );
        let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
        let slow_global_helpers =
            import_user_names_for_symbols(&built, &["soac_runtime_load_global_slow"]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
            1,
            "{case_name}: test deopt guard mode should call the deopt resume helper"
        );
        assert_eq!(
            count_cold_block_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
            1,
            "{case_name}: test deopt guard mode should isolate the deopt helper call in a cold block"
        );
        assert_eq!(
            count_deopt_helper_success_returns(&built.ctx.func, &deopt_helpers),
            1,
            "{case_name}: test deopt guard mode should return a successful deopt continuation result"
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &slow_global_helpers),
            0,
            "{case_name}: test deopt guard mode should not emit the local slow global-load fallback"
        );
    }

    fn build_indexed_global_guard_miss_with_runtime_profile(
        py: Python<'_>,
        mode: &str,
        env_config: Option<SoacEnvConfig>,
    ) -> BuiltSpecializedFunction {
        let _opt_mode = set_opt_mode(mode);
        let soac_work_dir = fresh_test_work_dir("indexed-global-deopt-profile");
        let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![op_expr(Load::new(test_global_name("x")))],
            ret_term(none_expr()),
        );
        let mut module = test_module(ModuleNameGen::new(0), vec![function]);
        instrument_module_with_legacy_call_target_counters(&mut module);
        let shared_state = crate::module_type::build_shared_state_for_testing(
            py,
            module,
            "indexed_global_deopt_profile_test",
            "",
        )
        .expect("shared state should build");
        let function = shared_state.lowered_module.callable_defs[0].clone();
        let specialization_inputs = (mode != "profile").then(|| {
            indexed_global_specialization_inputs_for_function(
                &function,
                IndexedGlobalAccessKind::Load,
                "x",
            )
        });
        let compile_session = match env_config {
            Some(env_config) => crate::session::CompileSession::new_with_env_config(env_config),
            None => crate::session::CompileSession::new(),
        };
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let module_constant_ptrs = shared_state.module_constant_ptrs();
        let module_constant_object_data_ids = declare_module_constant_object_data(
            &mut jit_module,
            &shared_state.lowered_module,
            &module_constant_ptrs,
        )
        .expect("module constant object data should declare");
        let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
            define_test_counter_storage(
                &mut jit_module,
                &shared_state.lowered_module,
                shared_state.lowered_module.counter_defs.as_slice(),
            );
        build_test_cranelift_run_bb_specialized_function(
            &mut jit_module,
            &blocks,
            &shared_state.lowered_module,
            &function,
            &shared_state.codegen_constants,
            shared_state.lowered_module.counter_defs.as_slice(),
            module_constant_object_data_ids.as_slice(),
            counter_slots_by_id.as_ref(),
            scalar_counter_data_id,
            top_value_counter_data_id,
            &compile_session,
            Some(shared_state.as_ref()),
            None,
            None,
            BuildSpecializedFunctionOptions {
                specialization_inputs,
                ..BuildSpecializedFunctionOptions::default()
            },
        )
        .expect("specialized JIT build should succeed")
    }

    fn build_indexed_global_store_guard_miss_with_runtime_profile(
        py: Python<'_>,
        mode: &str,
    ) -> BuiltSpecializedFunction {
        let _opt_mode = set_opt_mode(mode);
        let soac_work_dir = fresh_test_work_dir("indexed-global-store-deopt-profile");
        let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![op_expr(Store::new(
                test_global_name("x"),
                constants.int_expr(3),
            ))],
            ret_term(none_expr()),
        );
        let mut module = test_module(ModuleNameGen::new(0), vec![function]);
        module.module_constants = constants.module_constants;
        instrument_module_with_legacy_call_target_counters(&mut module);
        let shared_state = crate::module_type::build_shared_state_for_testing(
            py,
            module,
            "indexed_global_store_deopt_profile_test",
            "",
        )
        .expect("shared state should build");
        let function = shared_state.lowered_module.callable_defs[0].clone();
        let specialization_inputs = (mode != "profile").then(|| {
            indexed_global_specialization_inputs_for_function(
                &function,
                IndexedGlobalAccessKind::Store,
                "x",
            )
        });
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let module_constant_ptrs = shared_state.module_constant_ptrs();
        let module_constant_object_data_ids = declare_module_constant_object_data(
            &mut jit_module,
            &shared_state.lowered_module,
            &module_constant_ptrs,
        )
        .expect("module constant object data should declare");
        let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
            define_test_counter_storage(
                &mut jit_module,
                &shared_state.lowered_module,
                shared_state.lowered_module.counter_defs.as_slice(),
            );
        build_test_cranelift_run_bb_specialized_function(
            &mut jit_module,
            &blocks,
            &shared_state.lowered_module,
            &function,
            &shared_state.codegen_constants,
            shared_state.lowered_module.counter_defs.as_slice(),
            module_constant_object_data_ids.as_slice(),
            counter_slots_by_id.as_ref(),
            scalar_counter_data_id,
            top_value_counter_data_id,
            &compile_session,
            Some(shared_state.as_ref()),
            None,
            None,
            BuildSpecializedFunctionOptions {
                specialization_inputs,
                ..BuildSpecializedFunctionOptions::default()
            },
        )
        .expect("specialized JIT build should succeed")
    }

    #[test]
    fn indexed_global_guard_miss_deopt_enabled_by_verify_mode_runtime_profile() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "indexed_global_guard_miss_deopt_enabled_by_verify_mode_runtime_profile",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_indexed_global_guard_miss_with_runtime_profile(py, "verify", None);
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let slow_global_helpers =
                import_user_names_for_symbols(&built, &["soac_runtime_load_global_slow"]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "verify mode should enable indexed global guard-miss deopt through SpecializationProfile"
            );
            assert_eq!(
                count_cold_block_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "verify-mode guard-miss deopt should keep the helper call cold"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &slow_global_helpers),
                0,
                "verify mode should not emit the local slow global-load fallback for a planned deopt point"
            );
        });
    }

    #[test]
    fn indexed_global_guard_miss_deopt_disabled_when_refcount_emission_is_disabled() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "indexed_global_guard_miss_deopt_disabled_when_refcount_emission_is_disabled",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_indexed_global_guard_miss_with_runtime_profile(
                py,
                "verify",
                Some(SoacEnvConfig::default().with_jit_refcount_emission_enabled(false)),
            );
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let slow_global_helpers =
                import_user_names_for_symbols(&built, &["soac_runtime_load_global_slow"]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                0,
                "disabled refcount emission should keep guard misses out of ownership-sensitive deopt replay"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &slow_global_helpers),
                1,
                "disabled refcount emission should preserve the local slow global-load fallback"
            );
        });
    }

    #[test]
    fn indexed_global_guard_miss_deopt_disabled_by_profile_mode_runtime_profile() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "indexed_global_guard_miss_deopt_disabled_by_profile_mode_runtime_profile",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_indexed_global_guard_miss_with_runtime_profile(py, "profile", None);
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let slow_global_helpers =
                import_user_names_for_symbols(&built, &["soac_runtime_load_global_slow"]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                0,
                "profile mode should not replace the guard-miss path with deopt"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &slow_global_helpers),
                0,
                "profile mode without a v3 indexed-global plan should not emit the planned slow fallback"
            );
        });
    }

    #[test]
    fn indexed_global_store_guard_miss_deopt_enabled_by_verify_mode_runtime_profile() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "indexed_global_store_guard_miss_deopt_enabled_by_verify_mode_runtime_profile",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_indexed_global_store_guard_miss_with_runtime_profile(py, "verify");
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let slow_store_helpers =
                import_user_names_for_symbols(&built, &[SOAC_RUNTIME_STORE_GLOBAL_IMPORT.symbol]);
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "verify mode should enable indexed global store guard-miss deopt"
            );
            assert_eq!(
                count_cold_block_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "verify-mode indexed global store deopt helper call should be cold"
            );
            assert_eq!(
                count_deopt_helper_success_returns(&built.ctx.func, &deopt_helpers),
                1,
                "verify-mode indexed global store deopt should return a successful continuation result"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &slow_store_helpers),
                0,
                "verify mode should not emit the local slow global-store fallback for a planned deopt point"
            );
        });
    }

    #[test]
    fn indexed_global_store_guard_miss_deopt_disabled_by_profile_mode_runtime_profile() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "indexed_global_store_guard_miss_deopt_disabled_by_profile_mode_runtime_profile",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_indexed_global_store_guard_miss_with_runtime_profile(py, "profile");
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let indexed_store_helpers = import_user_names_for_symbols(
                &built,
                &[SOAC_RUNTIME_STORE_GLOBAL_INDEXED_IMPORT.symbol],
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                0,
                "profile mode should not replace the global-store guard-miss path with deopt"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &indexed_store_helpers),
                0,
                "profile mode should not enable the indexed global-store helper"
            );
        });
    }

    fn build_direct_call_guard_miss_with_runtime_profile(
        py: Python<'_>,
        callable_replay_safe: bool,
    ) -> BuiltSpecializedFunction {
        let _opt_mode = set_opt_mode("verify");
        let soac_work_dir = fresh_test_work_dir("direct-call-deopt-profile");
        let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
        let blocks = [1usize as ObjPtr];
        let module_name_gen = ModuleNameGen::new(0);
        let callee_function = with_single_test_block(
            test_function_in_module(&module_name_gen, "callee"),
            vec![],
            ret_term(none_expr()),
        );

        let caller_name_gen = module_name_gen.next_function_name_gen();
        let caller_function_id = caller_name_gen.function_id();
        let call_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let callable_expr = if callable_replay_safe {
            name_expr(test_name("fn"))
        } else {
            op_expr(Call::new(
                name_expr(test_runtime_name("list")),
                Vec::<CallArgPositional<InstrCodegen>>::new(),
                Vec::<CallArgKeyword<InstrCodegen>>::new(),
            ))
        };
        let caller_params = if callable_replay_safe {
            ParamSpec {
                params: vec![Param {
                    name: "fn".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                }],
            }
        } else {
            ParamSpec::default()
        };
        let mut caller_function = BlockPyFunction {
            function_id: caller_function_id,
            name_gen: caller_name_gen,
            names: FunctionName::new("caller", "caller", "caller", "caller"),
            kind: soac_core::block_py::FunctionKind::Function,
            execution_mode: Default::default(),
            params: caller_params,
            blocks: vec![CodegenBlock {
                label: BlockLabel::from_index(0),
                body: vec![],
                term: ret_term(with_instr_id(
                    op_expr(Call::new(
                        callable_expr,
                        Vec::<CallArgPositional<InstrCodegen>>::new(),
                        Vec::<CallArgKeyword<InstrCodegen>>::new(),
                    )),
                    call_instr_id,
                )),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            }],
            doc: None,
            storage_layout: None,
            scope: Default::default(),
        };
        if callable_replay_safe {
            set_stack_slots(&mut caller_function, &["fn"]);
        }

        let mut module = test_module(
            ModuleNameGen::new(0),
            vec![callee_function.clone(), caller_function.clone()],
        );
        write_test_counter_dump(
            soac_work_dir.join("profile.bin").as_path(),
            &CounterDumpRecord {
                source_hash: 0,
                module_name: "direct_call_deopt_profile_test".to_string(),
                package_name: None,
                rows: vec![CounterDumpRow {
                    counter_id: 0,
                    scope: "this".to_string(),
                    kind: "call_hot_targets".to_string(),
                    site_kind: "runtime".to_string(),
                    function_id: Some(caller_function.function_id),
                    current_function_id: Some(caller_function.function_id),
                    instr_id: Some(call_instr_id),
                    function_qualname: Some(caller_function.names.qualname.clone()),
                    block_label: None,
                    value: 1,
                    branch_values: Vec::new(),
                    observed_value: Some(callee_function.function_id.to_packed_runtime_u64()),
                    max_overcount: Some(0),
                }],
                module_keys: Vec::new(),
                type_keys: Vec::new(),
                type_table: Vec::new(),
            },
        );
        instrument_module_with_legacy_call_target_counters(&mut module);

        let shared_state = crate::module_type::build_shared_state_for_testing(
            py,
            module,
            "direct_call_deopt_profile_test",
            "",
        )
        .expect("shared state should build");
        let caller_function = shared_state.lowered_module.callable_defs[1].clone();
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let module_constant_ptrs = shared_state.module_constant_ptrs();
        let module_constant_object_data_ids = declare_module_constant_object_data(
            &mut jit_module,
            &shared_state.lowered_module,
            &module_constant_ptrs,
        )
        .expect("module constant object data should declare");
        let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
            define_test_counter_storage(
                &mut jit_module,
                &shared_state.lowered_module,
                shared_state.lowered_module.counter_defs.as_slice(),
            );
        build_test_cranelift_run_bb_specialized_function(
            &mut jit_module,
            &blocks,
            &shared_state.lowered_module,
            &caller_function,
            &shared_state.codegen_constants,
            shared_state.lowered_module.counter_defs.as_slice(),
            module_constant_object_data_ids.as_slice(),
            counter_slots_by_id.as_ref(),
            scalar_counter_data_id,
            top_value_counter_data_id,
            &compile_session,
            Some(shared_state.as_ref()),
            None,
            None,
            BuildSpecializedFunctionOptions::default(),
        )
        .expect("specialized JIT build should succeed")
    }

    #[test]
    fn direct_call_guard_miss_deopt_enabled_for_replay_safe_callable() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "direct_call_guard_miss_deopt_enabled_for_replay_safe_callable",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_direct_call_guard_miss_with_runtime_profile(py, true);
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let generic_call_helpers = import_user_names_for_symbols(
                &built,
                &[
                    DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT.symbol,
                    DP_JIT_PY_VECTORCALL_IMPORT.symbol,
                ],
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "verify mode should enable direct-call guard-miss deopt for replay-safe callables"
            );
            assert_eq!(
                count_cold_block_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                1,
                "direct-call guard-miss deopt should keep the helper call cold"
            );
            assert_eq!(
                count_deopt_helper_success_returns(&built.ctx.func, &deopt_helpers),
                1,
                "direct-call guard-miss deopt should return a successful deopt continuation result"
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &generic_call_helpers),
                0,
                "direct-call guard miss should not emit the local generic call fallback when deopt is planned"
            );
        });
    }

    #[test]
    fn direct_call_guard_miss_keeps_fallback_for_replay_unsafe_callable() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "direct_call_guard_miss_keeps_fallback_for_replay_unsafe_callable",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| {
            let built = build_direct_call_guard_miss_with_runtime_profile(py, false);
            let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
            let generic_call_helpers = import_user_names_for_symbols(
                &built,
                &[
                    DP_JIT_PY_CALL_POSITIONAL_THREE_IMPORT.symbol,
                    DP_JIT_PY_VECTORCALL_IMPORT.symbol,
                ],
            );
            assert_eq!(
                count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
                0,
                "replay-unsafe callable expressions should keep the local fallback instead of deopt"
            );
            assert_eq!(
                count_cold_block_direct_calls_to_runtime_helpers(
                    &built.ctx.func,
                    &generic_call_helpers
                ),
                1,
                "replay-unsafe callable expressions should still emit the generic fallback call"
            );
        });
    }

    #[test]
    fn direct_call_guard_miss_deopt_resumes_generic_call_runtime() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "direct_call_guard_miss_deopt_resumes_generic_call_runtime",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            #[repr(C)]
            struct TestFunctionEnv {
                direct_code_ptr: *const u8,
                default_direct_code_ptr: *const u8,
                deopt_table_ptr: ObjPtr,
                globals_obj: ObjPtr,
            }

            let _opt_mode = set_opt_mode("verify");
            let soac_work_dir = fresh_test_work_dir("direct-call-deopt-runtime");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let module_name_gen = ModuleNameGen::new(0);
            let callee_function = with_single_test_block(
                test_function_in_module(&module_name_gen, "profiled_callee"),
                vec![],
                ret_term(none_expr()),
            );

            let caller_name_gen = module_name_gen.next_function_name_gen();
            let caller_function_id = caller_name_gen.function_id();
            let call_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
            let mut caller_function = BlockPyFunction {
                function_id: caller_function_id,
                name_gen: caller_name_gen,
                names: FunctionName::new("caller", "caller", "caller", "caller"),
                kind: soac_core::block_py::FunctionKind::Function,
                execution_mode: Default::default(),
                params: ParamSpec {
                    params: vec![Param {
                        name: "fn".into(),
                        kind: ParamKind::Any,
                        has_default: false,
                    }],
                },
                blocks: vec![CodegenBlock {
                    label: BlockLabel::from_index(0),
                    body: vec![],
                    term: ret_term(with_instr_id(
                        op_expr(Call::new(
                            name_expr(test_name("fn")),
                            Vec::<CallArgPositional<InstrCodegen>>::new(),
                            Vec::<CallArgKeyword<InstrCodegen>>::new(),
                        )),
                        call_instr_id,
                    )),
                    params: vec![],
                    exc_edge: None,
                    extra: Default::default(),
                }],
                doc: None,
                storage_layout: None,
                scope: Default::default(),
            };
            set_stack_slots(&mut caller_function, &["fn"]);

            let mut module = test_module(
                ModuleNameGen::new(0),
                vec![callee_function.clone(), caller_function.clone()],
            );
            write_test_counter_dump(
                soac_work_dir.join("profile.bin").as_path(),
                &CounterDumpRecord {
                    source_hash: 0,
                    module_name: "direct_call_deopt_runtime_test".to_string(),
                    package_name: None,
                    rows: vec![CounterDumpRow {
                        counter_id: 0,
                        scope: "this".to_string(),
                        kind: "call_hot_targets".to_string(),
                        site_kind: "runtime".to_string(),
                        function_id: Some(caller_function.function_id),
                        current_function_id: Some(caller_function.function_id),
                        instr_id: Some(call_instr_id),
                        function_qualname: Some(caller_function.names.qualname.clone()),
                        block_label: None,
                        value: 1,
                        branch_values: Vec::new(),
                        observed_value: Some(callee_function.function_id.to_packed_runtime_u64()),
                        max_overcount: Some(0),
                    }],
                    module_keys: Vec::new(),
                    type_keys: Vec::new(),
                    type_table: Vec::new(),
                },
            );
            instrument_module_with_legacy_call_target_counters(&mut module);

            let shared_state = crate::module_type::build_shared_state_for_testing(
                py,
                module,
                "direct_call_deopt_runtime_test",
                "",
            )
            .expect("shared state should build");
            let runtime = build_test_module_runtime(py, shared_state.clone());
            let callee_function = shared_state.lowered_module.callable_defs[0].clone();
            let caller_function = shared_state.lowered_module.callable_defs[1].clone();
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let compile_session = runtime.compile_session.as_ref();
            let mut jit_module =
                new_jit_module(compile_session).expect("test jit module should construct");
            let module_constant_object_data_ids = declare_module_constant_object_data(
                &mut jit_module,
                &shared_state.lowered_module,
                &module_constant_ptrs,
            )
            .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    shared_state.lowered_module.counter_defs.as_slice(),
                );
            let (_callee_sig, declared_callee) =
                declare_direct_function(&mut jit_module, &callee_function, None)
                    .expect("profiled callee should declare");
            let predeclared =
                HashMap::from([(callee_function.function_id, declared_callee.clone())]);
            let blocks = vec![std::ptr::null_mut::<c_void>(); caller_function.blocks.len()];
            let built_callee = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks.as_slice(),
                &shared_state.lowered_module,
                &callee_function,
                &shared_state.codegen_constants,
                shared_state.lowered_module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                compile_session,
                Some(shared_state.as_ref()),
                None,
                Some(&predeclared),
                BuildSpecializedFunctionOptions::default(),
            )
            .expect("profiled callee JIT build should succeed");
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks.as_slice(),
                &shared_state.lowered_module,
                &caller_function,
                &shared_state.codegen_constants,
                shared_state.lowered_module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                compile_session,
                Some(shared_state.as_ref()),
                None,
                Some(&predeclared),
                BuildSpecializedFunctionOptions::default(),
            )
            .expect("caller JIT build should succeed");
            let facts = infer_jit_value_facts(&shared_state.lowered_module);
            let module_plan =
                plan_jit_module_from_codegen(&shared_state.lowered_module, facts.clone())
                    .map(|prepared| prepared.deopt_resume)
                    .expect("JIT deopt resume planning should succeed");
            let function_plan = module_plan
                .function(caller_function.function_id)
                .expect("caller should have a JIT deopt plan");
            let deopt_table = RuntimeJitDeoptTable::from_plan(
                &caller_function,
                function_plan,
                &module_constant_ptrs,
            )
            .expect("runtime deopt table should build from plan");

            let mut callee_ctx = built_callee.ctx;
            define_prepared_function(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built_callee.main_id,
                &mut callee_ctx,
                "test-direct-call-deopt-profiled-callee",
                "profiled callee should define",
            )
            .expect("profiled callee should define");
            jit_module.clear_context(&mut callee_ctx);
            let mut caller_ctx = built.ctx;
            define_prepared_function(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.main_id,
                &mut caller_ctx,
                "test-direct-call-deopt-runtime-caller",
                "caller should define",
            )
            .expect("caller should define");
            jit_module.clear_context(&mut caller_ctx);
            jit_module
                .finalize_definitions()
                .expect("test jit module should finalize");
            let code_ptr = jit_module.get_finalized_function(built.main_id);

            let globals = ffi::PyDict_New();
            assert!(!globals.is_null(), "test globals dict should allocate");
            assert_eq!(
                ffi::PyDict_SetItemString(
                    globals,
                    c"__builtins__".as_ptr(),
                    ffi::PyEval_GetBuiltins()
                ),
                0,
                "test globals should accept builtins"
            );
            let source = c"def miss_callable():\n    return 4242\n";
            let run_result = ffi::PyRun_StringFlags(
                source.as_ptr(),
                ffi::Py_file_input,
                globals,
                globals,
                std::ptr::null_mut(),
            );
            assert!(
                !run_result.is_null(),
                "test callable definition should execute"
            );
            ffi::Py_DECREF(run_result);
            let callable = ffi::PyDict_GetItemString(globals, c"miss_callable".as_ptr());
            assert!(!callable.is_null(), "test callable should exist");
            ffi::Py_INCREF(callable);

            let function_env = TestFunctionEnv {
                direct_code_ptr: code_ptr,
                default_direct_code_ptr: std::ptr::null(),
                deopt_table_ptr: std::ptr::addr_of!(deopt_table).cast_mut().cast(),
                globals_obj: runtime.mod_ctx.globals_obj,
            };
            let entry: unsafe extern "C" fn(ObjPtr, ObjPtr, ObjPtr) -> ObjPtr =
                std::mem::transmute(code_ptr);
            let result = entry(
                std::ptr::addr_of!(function_env).cast_mut().cast(),
                ffi::PyThreadState_Get().cast(),
                callable.cast(),
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful direct-call guard-miss deopt should not leave a Python exception"
            );
            assert_eq!(
                ffi::PyLong_AsLong(result.cast::<ffi::PyObject>()),
                4242,
                "guard-miss deopt should resume before the generic call and return its result"
            );

            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(callable);
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn profiled_v3_module_plan_uses_loaded_optimized_module_without_rewriting_direct_calls() {
        let module_name = "v3_source_keyed_shape_test";
        let module_name_gen = ModuleNameGen::new(0);
        let mut callee_function = test_function_in_module(&module_name_gen, "callee");
        callee_function.params.params.push(Param {
            name: "x".into(),
            kind: ParamKind::Any,
            has_default: false,
        });
        callee_function = with_single_test_block(
            callee_function,
            vec![],
            ret_term(name_expr(test_local_name("x", 0))),
        );
        set_stack_slots(&mut callee_function, &["x"]);

        let mut caller_function = test_function_in_module(&module_name_gen, "caller");
        caller_function.params.params.extend([
            Param {
                name: "fn".into(),
                kind: ParamKind::Any,
                has_default: false,
            },
            Param {
                name: "x".into(),
                kind: ParamKind::Any,
                has_default: false,
            },
        ]);
        let caller_block_label = caller_function.name_gen.next_block_name();
        let call_instr_id = InstrId::new(caller_block_label, 1);
        caller_function = with_test_blocks(
            caller_function,
            vec![CodegenBlock {
                label: caller_block_label,
                body: vec![assign_stmt(
                    test_local_name("y", 2),
                    with_instr_id(
                        op_expr(Call::new(
                            name_expr(test_local_name("fn", 0)),
                            vec![CallArgPositional::Positional(name_expr(test_local_name(
                                "x", 1,
                            )))],
                            Vec::<CallArgKeyword<InstrCodegen>>::new(),
                        )),
                        call_instr_id,
                    ),
                )],
                term: ret_term(name_expr(test_local_name("y", 2))),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            }],
        );
        set_stack_slots(&mut caller_function, &["fn", "x", "y"]);
        let module = test_module(
            module_name_gen,
            vec![callee_function.clone(), caller_function.clone()],
        );
        let v3_plan = ResolvedV3DirectCallPlan {
            source: call_instr_id,
            target: callee_function.function_id,
            arg_plan: TypedDirectCallArgPlan {
                sources: vec![TypedDirectCallArgSource::Provided(0)],
            },
            body: test_v3_inline_call_body(),
            reason: "profiled direct call".to_string(),
        };
        let non_call_v3_source = InstrId::new(caller_block_label, 99);
        let exact_list_item = OptV3ExactListItemAccessPlan {
            source: non_call_v3_source,
            access: PlanV3ExactListItemAccessKind::Get,
            shape: PlanV3ExactListItemShape::ExactListExactInt,
            guard: PlanV3ExactListItemGuardKind::ExactListExactCompactIntInBounds,
            fallback: PlanV3ExactListItemFallbackKind::OriginalItemAccess,
        };
        let profile = SpecializationProfile {
            module_name: Some(module_name),
            counter_dump_path: None,
            optimized_module: Some(Arc::new(module.clone())),
            direct_call_emission_scope: DirectCallEmissionScope::DirectCallBodiesOnly,
            opt_v3_emitted_direct_calls: HashMap::from([(
                caller_function.function_id,
                HashMap::from([(call_instr_id, vec![v3_plan])]),
            )]),
            opt_v3_emitted_exact_list_items: HashMap::from([(
                caller_function.function_id,
                HashMap::from([(non_call_v3_source, exact_list_item)]),
            )]),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        let plan = build_profiled_jit_module_plan(&module, &profile)
            .expect("v3-profiled JIT module plan should build");
        let planned_caller = plan
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == caller_function.function_id)
            .expect("planned module should keep caller");

        assert!(
            !planned_caller.blocks.iter().any(|block| {
                matches!(
                    &block.term,
                    BlockTerm::IfTerm(term)
                        if matches!(term.test, InstrTyped::DirectCallGuardTest(_))
                )
            }),
            "JIT should not perform plan-level direct-call CFG expansion; it should trust the loaded optimized module"
        );
        assert!(
            planned_caller
                .blocks
                .iter()
                .flat_map(|block| &block.body)
                .any(|instr| matches!(instr, InstrTyped::LegacyStore(store) if matches!(store.value.as_ref(), InstrTyped::CallTyped(_)))),
            "loaded optimized module fixture intentionally still has the original generic call"
        );
    }

    #[test]
    fn indexed_global_body_guard_miss_can_target_cold_deopt_stub() {
        let function = with_single_test_block(
            test_function(),
            vec![op_expr(Load::new(test_global_name("x")))],
            ret_term(none_expr()),
        );
        assert_indexed_global_guard_miss_targets_cold_deopt_stub(function, "body load");
    }

    #[test]
    fn indexed_global_nested_body_guard_miss_deopts_from_enclosing_body_instr() {
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![tuple_expr(vec![op_expr(Load::new(test_global_name("x")))])],
            ret_term(none_expr()),
        );
        let mut module = test_module(ModuleNameGen::new(0), vec![function]);
        instrument_module_with_legacy_call_target_counters(&mut module);
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let specialization_inputs = indexed_global_specialization_inputs_for_function(
            &function,
            IndexedGlobalAccessKind::Load,
            "x",
        );
        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                guard_miss_deopt_stub: true,
                specialization_inputs: Some(specialization_inputs),
                ..BuildSpecializedFunctionOptions::default()
            },
        );
        assert_guard_miss_deopts_without_local_fallback(
            &built,
            &["soac_runtime_load_global_slow"],
            "nested global load before any replay-unsafe effect",
        );
    }

    #[test]
    fn indexed_global_nested_body_guard_miss_keeps_fallback_after_replay_unsafe_effect() {
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![tuple_expr(vec![
                op_expr(Call::new(
                    none_expr(),
                    Vec::<CallArgPositional<InstrCodegen>>::new(),
                    Vec::<CallArgKeyword<InstrCodegen>>::new(),
                )),
                op_expr(Load::new(test_global_name("x"))),
            ])],
            ret_term(none_expr()),
        );
        let mut module = test_module(ModuleNameGen::new(0), vec![function]);
        instrument_module_with_legacy_call_target_counters(&mut module);
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let specialization_inputs = indexed_global_specialization_inputs_for_function(
            &function,
            IndexedGlobalAccessKind::Load,
            "x",
        );
        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                guard_miss_deopt_stub: true,
                specialization_inputs: Some(specialization_inputs),
                ..BuildSpecializedFunctionOptions::default()
            },
        );
        let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
        let slow_global_helpers =
            import_user_names_for_symbols(&built, &["soac_runtime_load_global_slow"]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
            0,
            "nested guard miss after a possibly side-effecting call must not deopt to before the statement"
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &slow_global_helpers),
            1,
            "nested guard miss after a replay-unsafe effect should preserve the local slow fallback"
        );
    }

    #[test]
    fn indexed_global_nested_body_guard_miss_deopt_resumes_enclosing_instr_runtime() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            #[repr(C)]
            struct TestFunctionEnv {
                direct_code_ptr: *const u8,
                default_direct_code_ptr: *const u8,
                deopt_table_ptr: ObjPtr,
                globals_obj: ObjPtr,
            }

            let tuple_value_expr = tuple_expr(vec![op_expr(Load::new(test_global_name("x")))]);
            let mut function = with_single_test_block(
                test_function(),
                vec![assign_stmt(test_name("out"), tuple_value_expr)],
                ret_term(name_expr(test_name("out"))),
            );
            set_stack_slots(&mut function, &["out"]);
            let module = test_module(ModuleNameGen::new(0), vec![function]);
            let shared_state = crate::module_type::build_shared_state_for_testing(
                py,
                module,
                "nested_deopt_test",
                "",
            )
            .expect("shared state should build");
            let function = shared_state.lowered_module.callable_defs[0].clone();
            let runtime = build_test_module_runtime(py, shared_state.clone());
            let key = ffi::PyUnicode_FromString(c"x".as_ptr());
            assert!(!key.is_null(), "test key allocation should succeed");
            let value = ffi::PyLong_FromLong(24_681_357);
            assert!(!value.is_null(), "test value allocation should succeed");
            assert_eq!(
                ffi::PyDict_SetItem(runtime.mod_ctx.globals_obj.cast(), key, value),
                0,
                "test globals insertion should succeed"
            );

            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let module_constant_object_data_ids = declare_module_constant_object_data(
                &mut jit_module,
                &shared_state.lowered_module,
                &module_constant_ptrs,
            )
            .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    shared_state.lowered_module.counter_defs.as_slice(),
                );
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let specialization_inputs = indexed_global_specialization_inputs_for_function(
                &function,
                IndexedGlobalAccessKind::Load,
                "x",
            );
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks.as_slice(),
                &shared_state.lowered_module,
                &function,
                &shared_state.codegen_constants,
                shared_state.lowered_module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                &compile_session,
                Some(shared_state.as_ref()),
                None,
                None,
                BuildSpecializedFunctionOptions {
                    guard_miss_deopt_stub: true,
                    specialization_inputs: Some(specialization_inputs),
                    ..BuildSpecializedFunctionOptions::default()
                },
            )
            .expect("specialized JIT build should succeed");
            let facts = infer_jit_value_facts(&shared_state.lowered_module);
            let module_plan =
                plan_jit_module_from_codegen(&shared_state.lowered_module, facts.clone())
                    .map(|prepared| prepared.deopt_resume)
                    .expect("JIT deopt resume planning should succeed");
            let function_plan = module_plan
                .function(function.function_id)
                .expect("function should have a JIT deopt plan");
            let deopt_table =
                RuntimeJitDeoptTable::from_plan(&function, function_plan, &module_constant_ptrs)
                    .expect("runtime deopt table should build from plan");

            let mut ctx = built.ctx;
            define_prepared_function(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.main_id,
                &mut ctx,
                "test-nested-body-global-deopt-resume",
                "nested body-position global deopt test should define",
            )
            .expect("test function should define");
            jit_module.clear_context(&mut ctx);
            jit_module
                .finalize_definitions()
                .expect("test jit module should finalize");
            let code_ptr = jit_module.get_finalized_function(built.main_id);
            let function_env = TestFunctionEnv {
                direct_code_ptr: code_ptr,
                default_direct_code_ptr: std::ptr::null(),
                deopt_table_ptr: std::ptr::addr_of!(deopt_table).cast_mut().cast(),
                globals_obj: runtime.mod_ctx.globals_obj,
            };
            let entry: unsafe extern "C" fn(ObjPtr, ObjPtr) -> ObjPtr =
                std::mem::transmute(code_ptr);
            let result = entry(
                std::ptr::addr_of!(function_env).cast_mut().cast(),
                ffi::PyThreadState_Get().cast(),
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful nested body-position deopt should not leave a Python exception"
            );
            assert_eq!(
                ffi::PyTuple_Size(result.cast::<ffi::PyObject>()),
                1,
                "deopt continuation should return the tuple produced by the enclosing statement"
            );
            assert_eq!(
                ffi::PyTuple_GetItem(result.cast::<ffi::PyObject>(), 0),
                value.cast::<ffi::PyObject>(),
                "deopt continuation should replay the nested global load into Tuple"
            );

            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value);
            ffi::Py_DECREF(key);
        });
    }

    #[test]
    fn indexed_global_body_guard_miss_deopt_resumes_block_tail_runtime() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            #[repr(C)]
            struct TestFunctionEnv {
                direct_code_ptr: *const u8,
                default_direct_code_ptr: *const u8,
                deopt_table_ptr: ObjPtr,
                globals_obj: ObjPtr,
            }

            let function = with_single_test_block(
                test_function(),
                vec![op_expr(Load::new(test_global_name("x")))],
                ret_term(none_expr()),
            );
            let module = test_module(ModuleNameGen::new(0), vec![function]);
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, module, "deopt_test", "")
                    .expect("shared state should build");
            let function = shared_state.lowered_module.callable_defs[0].clone();
            let runtime = build_test_module_runtime(py, shared_state.clone());
            let key = ffi::PyUnicode_FromString(c"x".as_ptr());
            assert!(!key.is_null(), "test key allocation should succeed");
            let value = ffi::PyLong_FromLong(135_791_113);
            assert!(!value.is_null(), "test value allocation should succeed");
            assert_eq!(
                ffi::PyDict_SetItem(runtime.mod_ctx.globals_obj.cast(), key, value),
                0,
                "test globals insertion should succeed"
            );

            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let module_constant_object_data_ids = declare_module_constant_object_data(
                &mut jit_module,
                &shared_state.lowered_module,
                &module_constant_ptrs,
            )
            .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    shared_state.lowered_module.counter_defs.as_slice(),
                );
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let specialization_inputs = indexed_global_specialization_inputs_for_function(
                &function,
                IndexedGlobalAccessKind::Load,
                "x",
            );
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks.as_slice(),
                &shared_state.lowered_module,
                &function,
                &shared_state.codegen_constants,
                shared_state.lowered_module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                &compile_session,
                Some(shared_state.as_ref()),
                None,
                None,
                BuildSpecializedFunctionOptions {
                    guard_miss_deopt_stub: true,
                    specialization_inputs: Some(specialization_inputs),
                    ..BuildSpecializedFunctionOptions::default()
                },
            )
            .expect("specialized JIT build should succeed");
            let facts = infer_jit_value_facts(&shared_state.lowered_module);
            let module_plan =
                plan_jit_module_from_codegen(&shared_state.lowered_module, facts.clone())
                    .map(|prepared| prepared.deopt_resume)
                    .expect("JIT deopt resume planning should succeed");
            let function_plan = module_plan
                .function(function.function_id)
                .expect("function should have a JIT deopt plan");
            let deopt_table =
                RuntimeJitDeoptTable::from_plan(&function, function_plan, &module_constant_ptrs)
                    .expect("runtime deopt table should build from plan");

            let mut ctx = built.ctx;
            define_prepared_function(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.main_id,
                &mut ctx,
                "test-body-global-deopt-resume",
                "body-position global deopt test should define",
            )
            .expect("test function should define");
            jit_module.clear_context(&mut ctx);
            jit_module
                .finalize_definitions()
                .expect("test jit module should finalize");
            let code_ptr = jit_module.get_finalized_function(built.main_id);
            let function_env = TestFunctionEnv {
                direct_code_ptr: code_ptr,
                default_direct_code_ptr: std::ptr::null(),
                deopt_table_ptr: std::ptr::addr_of!(deopt_table).cast_mut().cast(),
                globals_obj: runtime.mod_ctx.globals_obj,
            };
            let entry: unsafe extern "C" fn(ObjPtr, ObjPtr) -> ObjPtr =
                std::mem::transmute(code_ptr);
            let before = ffi::Py_REFCNT(value);
            let result = entry(
                std::ptr::addr_of!(function_env).cast_mut().cast(),
                ffi::PyThreadState_Get().cast(),
            );
            assert_eq!(
                result,
                ffi::Py_None().cast(),
                "guard-miss deopt should execute the body load and continue to return None"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful body-position deopt should not leave a Python exception"
            );
            assert_eq!(
                ffi::Py_REFCNT(value),
                before,
                "discarded body load result should be decref'd before continuation return"
            );

            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value);
            ffi::Py_DECREF(key);
        });
    }

    #[test]
    fn indexed_global_guard_miss_deopt_forwards_live_local_count() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = with_single_test_block(
            test_function(),
            vec![
                assign_stmt(test_name("x"), constants.int_expr(7)),
                op_expr(Load::new(test_global_name("y"))),
            ],
            ret_term(none_expr()),
        );
        set_stack_slots(&mut function, &["x"]);
        let mut module = BlockPyModule {
            module_name_gen: ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![function],
            module_constants: constants.module_constants,
            counter_defs: Vec::new(),
        };
        instrument_module_with_legacy_call_target_counters(&mut module);
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let specialization_inputs = indexed_global_specialization_inputs_for_function(
            &function,
            IndexedGlobalAccessKind::Load,
            "y",
        );
        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                guard_miss_deopt_stub: true,
                specialization_inputs: Some(specialization_inputs),
                ..BuildSpecializedFunctionOptions::default()
            },
        );
        let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
        let deopt_call_args = direct_call_args_to_runtime_helpers(&built.ctx.func, &deopt_helpers);
        let [deopt_args] = deopt_call_args.as_slice() else {
            panic!(
                "test should emit exactly one deopt helper call, got {}",
                deopt_call_args.len()
            );
        };
        assert_eq!(
            deopt_args.len(),
            6,
            "deopt helper call should pass table, globals, function data, record ordinal, live buffer, and live count"
        );
        assert!(
            value_is_iconst_imm(&built.ctx.func, deopt_args[5], 1),
            "guard-miss deopt should pass one live local value for x"
        );
    }

    #[test]
    fn indexed_global_term_guard_miss_can_target_cold_deopt_stub() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Load::new(test_global_name("x")))),
        );
        assert_indexed_global_guard_miss_targets_cold_deopt_stub(function, "term load");
    }

    #[test]
    fn indexed_global_term_guard_miss_keeps_fallback_after_replay_unsafe_effect() {
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(tuple_expr(vec![
                op_expr(Call::new(
                    none_expr(),
                    Vec::<CallArgPositional<InstrCodegen>>::new(),
                    Vec::<CallArgKeyword<InstrCodegen>>::new(),
                )),
                op_expr(Load::new(test_global_name("x"))),
            ])),
        );
        let mut module = test_module(ModuleNameGen::new(0), vec![function]);
        instrument_module_with_legacy_call_target_counters(&mut module);
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let specialization_inputs = indexed_global_specialization_inputs_for_function(
            &function,
            IndexedGlobalAccessKind::Load,
            "x",
        );
        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                guard_miss_deopt_stub: true,
                specialization_inputs: Some(specialization_inputs),
                ..BuildSpecializedFunctionOptions::default()
            },
        );
        let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_resume"]);
        let slow_global_helpers =
            import_user_names_for_symbols(&built, &["soac_runtime_load_global_slow"]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
            0,
            "nested term guard miss after a possibly side-effecting call must not deopt to before the term"
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &slow_global_helpers),
            1,
            "nested term guard miss after a replay-unsafe effect should preserve the local slow fallback"
        );
    }

    #[test]
    fn indexed_global_term_guard_miss_deopt_resumes_return_global_runtime() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            #[repr(C)]
            struct TestFunctionEnv {
                direct_code_ptr: *const u8,
                default_direct_code_ptr: *const u8,
                deopt_table_ptr: ObjPtr,
                globals_obj: ObjPtr,
            }

            let function = with_single_test_block(
                test_function(),
                vec![],
                ret_term(op_expr(Load::new(test_global_name("x")))),
            );
            let module = test_module(ModuleNameGen::new(0), vec![function]);
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, module, "deopt_test", "")
                    .expect("shared state should build");
            let function = shared_state.lowered_module.callable_defs[0].clone();
            let runtime = build_test_module_runtime(py, shared_state.clone());
            let key = ffi::PyUnicode_FromString(c"x".as_ptr());
            assert!(!key.is_null(), "test key allocation should succeed");
            let value = ffi::PyLong_FromLong(246_813_579);
            assert!(!value.is_null(), "test value allocation should succeed");
            assert_eq!(
                ffi::PyDict_SetItem(runtime.mod_ctx.globals_obj.cast(), key, value),
                0,
                "test globals insertion should succeed"
            );

            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let module_constant_object_data_ids = declare_module_constant_object_data(
                &mut jit_module,
                &shared_state.lowered_module,
                &module_constant_ptrs,
            )
            .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    shared_state.lowered_module.counter_defs.as_slice(),
                );
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let specialization_inputs = indexed_global_specialization_inputs_for_function(
                &function,
                IndexedGlobalAccessKind::Load,
                "x",
            );
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks.as_slice(),
                &shared_state.lowered_module,
                &function,
                &shared_state.codegen_constants,
                shared_state.lowered_module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                &compile_session,
                Some(shared_state.as_ref()),
                None,
                None,
                BuildSpecializedFunctionOptions {
                    guard_miss_deopt_stub: true,
                    specialization_inputs: Some(specialization_inputs),
                    ..BuildSpecializedFunctionOptions::default()
                },
            )
            .expect("specialized JIT build should succeed");
            let facts = infer_jit_value_facts(&shared_state.lowered_module);
            let module_plan =
                plan_jit_module_from_codegen(&shared_state.lowered_module, facts.clone())
                    .map(|prepared| prepared.deopt_resume)
                    .expect("JIT deopt resume planning should succeed");
            let function_plan = module_plan
                .function(function.function_id)
                .expect("function should have a JIT deopt plan");
            let deopt_table =
                RuntimeJitDeoptTable::from_plan(&function, function_plan, &module_constant_ptrs)
                    .expect("runtime deopt table should build from plan");

            let mut ctx = built.ctx;
            define_prepared_function(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.main_id,
                &mut ctx,
                "test-return-global-deopt-resume",
                "return-global deopt test should define",
            )
            .expect("test function should define");
            jit_module.clear_context(&mut ctx);
            jit_module
                .finalize_definitions()
                .expect("test jit module should finalize");
            let code_ptr = jit_module.get_finalized_function(built.main_id);
            let function_env = TestFunctionEnv {
                direct_code_ptr: code_ptr,
                default_direct_code_ptr: std::ptr::null(),
                deopt_table_ptr: std::ptr::addr_of!(deopt_table).cast_mut().cast(),
                globals_obj: runtime.mod_ctx.globals_obj,
            };
            let entry: unsafe extern "C" fn(ObjPtr, ObjPtr) -> ObjPtr =
                std::mem::transmute(code_ptr);
            let before = ffi::Py_REFCNT(value);
            let result = entry(
                std::ptr::addr_of!(function_env).cast_mut().cast(),
                ffi::PyThreadState_Get().cast(),
            );
            assert_eq!(
                result,
                value.cast::<c_void>(),
                "guard-miss deopt should resume to the global return value"
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful return-global deopt should not leave a Python exception"
            );
            assert_eq!(
                ffi::Py_REFCNT(value),
                before + 1,
                "resumed return value should be owned by the caller"
            );

            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value);
            ffi::Py_DECREF(key);
        });
    }

    #[test]
    fn indexed_global_store_guard_miss_deopt_resumes_generic_store_runtime() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "indexed_global_store_guard_miss_deopt_resumes_generic_store_runtime",
        ) {
            return;
        }
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        crate::initialize_test_python();
        Python::attach(|py| unsafe {
            #[repr(C)]
            struct TestFunctionEnv {
                direct_code_ptr: *const u8,
                default_direct_code_ptr: *const u8,
                deopt_table_ptr: ObjPtr,
                globals_obj: ObjPtr,
            }

            let _opt_mode = set_opt_mode("verify");
            let soac_work_dir = fresh_test_work_dir("indexed-global-store-deopt-runtime");
            let _work_dir = EnvVarGuard::set_os("SOAC_WORK_DIR", soac_work_dir.as_os_str());
            let mut function = test_function();
            function.params = ParamSpec {
                params: vec![Param {
                    name: "value".into(),
                    kind: ParamKind::Any,
                    has_default: false,
                }],
            };
            function = with_single_test_block(
                function,
                vec![op_expr(Store::new(
                    test_global_name("x"),
                    name_expr(test_name("value")),
                ))],
                ret_term(name_expr(test_name("value"))),
            );
            set_stack_slots(&mut function, &["value"]);
            let mut module = test_module(ModuleNameGen::new(0), vec![function]);
            instrument_module_with_legacy_call_target_counters(&mut module);
            let shared_state = crate::module_type::build_shared_state_for_testing(
                py,
                module,
                "global_store_deopt_test",
                "",
            )
            .expect("shared state should build");
            let _runtime = build_test_module_runtime(py, shared_state.clone());
            let function = shared_state.lowered_module.callable_defs[0].clone();

            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let module_constant_object_data_ids = declare_module_constant_object_data(
                &mut jit_module,
                &shared_state.lowered_module,
                &module_constant_ptrs,
            )
            .expect("module constant object data should declare");
            let (counter_slots_by_id, scalar_counter_data_id, top_value_counter_data_id) =
                define_test_counter_storage(
                    &mut jit_module,
                    &shared_state.lowered_module,
                    shared_state.lowered_module.counter_defs.as_slice(),
                );
            let blocks = vec![std::ptr::null_mut::<c_void>(); function.blocks.len()];
            let specialization_inputs = indexed_global_specialization_inputs_for_function(
                &function,
                IndexedGlobalAccessKind::Store,
                "x",
            );
            let built = build_test_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks.as_slice(),
                &shared_state.lowered_module,
                &function,
                &shared_state.codegen_constants,
                shared_state.lowered_module.counter_defs.as_slice(),
                module_constant_object_data_ids.as_slice(),
                counter_slots_by_id.as_ref(),
                scalar_counter_data_id,
                top_value_counter_data_id,
                &compile_session,
                Some(shared_state.as_ref()),
                None,
                None,
                BuildSpecializedFunctionOptions {
                    specialization_inputs: Some(specialization_inputs),
                    ..BuildSpecializedFunctionOptions::default()
                },
            )
            .expect("specialized JIT build should succeed");
            let facts = infer_jit_value_facts(&shared_state.lowered_module);
            let module_plan =
                plan_jit_module_from_codegen(&shared_state.lowered_module, facts.clone())
                    .map(|prepared| prepared.deopt_resume)
                    .expect("JIT deopt resume planning should succeed");
            let function_plan = module_plan
                .function(function.function_id)
                .expect("function should have a JIT deopt plan");
            let deopt_table =
                RuntimeJitDeoptTable::from_plan(&function, function_plan, &module_constant_ptrs)
                    .expect("runtime deopt table should build from plan");

            let mut ctx = built.ctx;
            define_prepared_function(
                &mut jit_module,
                &SoacEnvConfig::default(),
                built.main_id,
                &mut ctx,
                "test-global-store-deopt-resume",
                "global-store deopt test should define",
            )
            .expect("test function should define");
            jit_module.clear_context(&mut ctx);
            jit_module
                .finalize_definitions()
                .expect("test jit module should finalize");
            let code_ptr = jit_module.get_finalized_function(built.main_id);

            let globals = ffi::PyDict_New();
            assert!(!globals.is_null(), "test globals dict should allocate");
            assert_eq!(
                ffi::PyDict_SetItemString(
                    globals,
                    c"__builtins__".as_ptr(),
                    ffi::PyEval_GetBuiltins()
                ),
                0,
                "test globals should accept builtins"
            );
            let key = ffi::PyUnicode_FromString(c"x".as_ptr());
            assert!(!key.is_null(), "test key allocation should succeed");
            let value = ffi::PyLong_FromLong(445_566);
            assert!(!value.is_null(), "test value allocation should succeed");

            let function_env = TestFunctionEnv {
                direct_code_ptr: code_ptr,
                default_direct_code_ptr: std::ptr::null(),
                deopt_table_ptr: std::ptr::addr_of!(deopt_table).cast_mut().cast(),
                globals_obj: globals.cast(),
            };
            let entry: unsafe extern "C" fn(ObjPtr, ObjPtr, ObjPtr) -> ObjPtr =
                std::mem::transmute(code_ptr);
            let result = entry(
                std::ptr::addr_of!(function_env).cast_mut().cast(),
                ffi::PyThreadState_Get().cast(),
                value.cast(),
            );
            assert!(
                ffi::PyErr_Occurred().is_null(),
                "successful global-store guard-miss deopt should not leave a Python exception"
            );
            assert_eq!(
                result,
                value.cast::<c_void>(),
                "global-store guard-miss deopt should resume and return the stored value"
            );
            assert_eq!(
                ffi::PyDict_GetItem(globals, key),
                value,
                "global-store guard-miss deopt should execute the generic global store"
            );

            ffi::Py_DECREF(result.cast::<ffi::PyObject>());
            ffi::Py_DECREF(value);
            ffi::Py_DECREF(key);
            ffi::Py_DECREF(globals);
        });
    }

    #[test]
    fn render_specialized_jit_load_global_intrinsic_uses_direct_helper() {
        let blocks = [1usize as ObjPtr];
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Load::new(test_global_name("x")))),
        );
        let rendered = render_test_jit_function_with_runtime_inline(&function, &blocks, Vec::new());
        assert_inlined_indexed_global_guard(
            &rendered,
            "load_global intrinsic should inline the indexed-module-dict guard",
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
        let rendered = render_test_jit_function_with_runtime_inline(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert_inlined_indexed_global_guard(
            &rendered,
            "store_global intrinsic should inline the indexed-module-dict guard",
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
            !rendered.contains("call dp_jit_function_closure_cell")
                && rendered.contains("load.i64")
                && rendered.contains("call dp_jit_load_cell"),
            "closure located names should load through the function-data object block:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_cell_ref_intrinsic_uses_function_closure_cells() {
        let blocks = [1usize as ObjPtr];
        let mut function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(soac_core::block_py::CellRef::new(
                CellLocation::Closure(2),
            ))),
        );
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            !rendered.contains("call dp_jit_function_closure_cell")
                && rendered.contains("load.i64"),
            "cell_ref intrinsic should use the function-data object block:\n{rendered}"
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
            ret_term(op_expr(soac_core::block_py::CellRef::new(
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
            !rendered.contains("call dp_jit_function_closure_cell")
                && rendered.contains("load.i64"),
            "captured cell sources should resolve through the function-data object block:\n{rendered}"
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
            !rendered.contains("call dp_jit_function_positional_default_obj")
                && rendered.contains("load.i64"),
            "direct entry lowering should source omitted positional defaults from the function-data object block:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_direct_entry_param_avoids_stack_roundtrip() {
        let blocks = [1usize as ObjPtr];
        let mut function =
            with_single_test_block(test_function(), vec![], ret_term(name_expr(test_name("x"))));
        function.params = ParamSpec {
            params: vec![Param {
                name: "x".into(),
                kind: ParamKind::Any,
                has_default: false,
            }],
        };
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            !rendered.contains("explicit_slot 8")
                && !rendered.contains("stack_load")
                && !rendered.contains("stack_store"),
            "direct-entry params should travel through block params without stack roundtrips:\n{rendered}"
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
            !rendered.contains("call dp_jit_function_kwonly_default_obj")
                && rendered.contains("load.i64"),
            "direct entry lowering should source omitted kwonly defaults from the function-data object block:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_deleted_name_checks_use_null_unbound_state() {
        let blocks = [1usize as ObjPtr];
        let mut function =
            with_single_test_block(test_function(), vec![], ret_term(name_expr(test_name("x"))));
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            rendered.contains("call dp_jit_raise_deleted_name_error"),
            "maybe-unbound local loads should lower through the null/unbound state:\n{rendered}"
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
    fn render_specialized_jit_leaves_rare_profiled_blocks_hot_by_default() {
        let blocks = [1usize as ObjPtr, 2usize as ObjPtr, 3usize as ObjPtr];
        let mut function = test_function();
        let entry_label = function.name_gen.next_block_name();
        let hot_label = function.name_gen.next_block_name();
        let cold_label = function.name_gen.next_block_name();
        function.blocks = vec![
            CodegenBlock {
                label: entry_label,
                body: vec![],
                term: BlockTerm::IfTerm(soac_core::block_py::TermIf {
                    test: none_expr(),
                    then_label: hot_label,
                    else_label: cold_label,
                }),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: hot_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: cold_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            },
        ];
        assign_missing_test_instr_ids(&mut function);

        let rendered = render_test_jit_function_with_block_entry_counts(
            &function,
            &blocks,
            Vec::new(),
            &[(entry_label, 10_000), (hot_label, 9_500), (cold_label, 75)],
            false,
        );

        assert!(
            !rendered.contains(&format!(" cold: ; block {cold_label}(")),
            "profiled cold-block replay should stay disabled by default:\n{rendered}"
        );
        assert!(
            !rendered.contains(&format!(" cold: ; block {hot_label}(")),
            "frequently visited block should stay hot in rendered CLIF:\n{rendered}"
        );
    }

    #[test]
    fn render_specialized_jit_marks_rare_profiled_blocks_cold_when_enabled() {
        let blocks = [1usize as ObjPtr, 2usize as ObjPtr, 3usize as ObjPtr];
        let mut function = test_function();
        let entry_label = function.name_gen.next_block_name();
        let hot_label = function.name_gen.next_block_name();
        let cold_label = function.name_gen.next_block_name();
        function.blocks = vec![
            CodegenBlock {
                label: entry_label,
                body: vec![],
                term: BlockTerm::IfTerm(soac_core::block_py::TermIf {
                    test: none_expr(),
                    then_label: hot_label,
                    else_label: cold_label,
                }),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: hot_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: cold_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            },
        ];
        assign_missing_test_instr_ids(&mut function);

        let rendered = render_test_jit_function_with_block_entry_counts(
            &function,
            &blocks,
            Vec::new(),
            &[(entry_label, 10_000), (hot_label, 9_500), (cold_label, 75)],
            true,
        );

        assert!(
            rendered.contains(&format!(" cold: ; block {cold_label}(")),
            "rarely visited block should be marked cold in rendered CLIF when enabled:\n{rendered}"
        );
        assert!(
            !rendered.contains(&format!(" cold: ; block {hot_label}(")),
            "frequently visited block should stay hot in rendered CLIF:\n{rendered}"
        );
    }

    #[test]
    fn profiled_cold_blocks_are_attached_to_typed_block_extra() {
        let mut function = test_function();
        let entry_label = function.name_gen.next_block_name();
        let hot_label = function.name_gen.next_block_name();
        let cold_label = function.name_gen.next_block_name();
        function.blocks = vec![
            CodegenBlock {
                label: entry_label,
                body: vec![],
                term: BlockTerm::IfTerm(soac_core::block_py::TermIf {
                    test: none_expr(),
                    then_label: hot_label,
                    else_label: cold_label,
                }),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: hot_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            },
            CodegenBlock {
                label: cold_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
                extra: Default::default(),
            },
        ];
        assign_missing_test_instr_ids(&mut function);
        let module_name = "typed_cold_block_extra_test";
        let soac_work_dir = fresh_test_work_dir("typed-cold-block-extra");
        let profile_path = soac_work_dir.join("profile.bin");
        write_test_counter_dump(
            profile_path.as_path(),
            &CounterDumpRecord {
                source_hash: 0,
                module_name: module_name.to_string(),
                package_name: None,
                rows: [(entry_label, 10_000), (hot_label, 9_500), (cold_label, 75)]
                    .into_iter()
                    .enumerate()
                    .map(|(index, (block_label, count))| CounterDumpRow {
                        counter_id: u32::try_from(index)
                            .expect("test block-entry counter count should fit in u32"),
                        scope: "this".to_string(),
                        kind: "block_entry".to_string(),
                        site_kind: "block_entry".to_string(),
                        function_id: Some(function.function_id),
                        current_function_id: Some(function.function_id),
                        instr_id: None,
                        function_qualname: Some(function.names.qualname.clone()),
                        block_label: Some(block_label.to_string()),
                        value: count,
                        branch_values: Vec::new(),
                        observed_value: None,
                        max_overcount: None,
                    })
                    .collect(),
                module_keys: Vec::new(),
                type_keys: Vec::new(),
                type_table: Vec::new(),
            },
        );
        let module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        let facts = infer_jit_value_facts(&module);
        let typed_function = lower_codegen_function_to_typed(function);
        let profile = SpecializationProfile {
            module_name: Some(module_name),
            counter_dump_path: Some(std::borrow::Cow::Owned(profile_path)),
            optimized_module: None,
            direct_call_emission_scope: DirectCallEmissionScope::DirectCallBodiesOnly,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: true,
            guard_miss_deopt: false,
        };

        let prepared =
            prepare_specialized_typed_function(&typed_function, None, Some(&profile), &facts, None)
                .expect("typed function should prepare with profiled cold blocks")
                .typed_function;
        let block_layout = prepared
            .blocks
            .iter()
            .map(|block| (block.label, block.extra.layout))
            .collect::<HashMap<_, _>>();

        assert_eq!(
            block_layout.get(&cold_label).copied(),
            Some(TypedBlockLayoutHint::Cold),
            "rarely visited block should carry cold layout in typed block extra"
        );
        assert_eq!(
            block_layout.get(&hot_label).copied(),
            Some(TypedBlockLayoutHint::Normal),
            "frequently visited block should stay normal in typed block extra"
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
    fn specialized_jit_tuple_instruction_uses_runtime_tuple_helpers() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(tuple_expr(vec![
                constants.int_expr(1),
                constants.int_expr(2),
            ])),
        );
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = constants.module_constants;
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let built =
            build_test_jit_function_with_constants(&module, &function, &blocks, &module_constants);
        let tuple_new_helpers =
            declared_user_names_for_symbols(&built, &[SOAC_RUNTIME_TUPLE_NEW_IMPORT.symbol]);
        let tuple_set_item_helpers = declared_user_names_for_symbols(
            &built,
            &[SOAC_RUNTIME_TUPLE_SET_ITEM_STOLEN_IMPORT.symbol],
        );
        let public_tuple_set_item_helpers =
            import_user_names_for_symbols(&built, &["PyTuple_SetItem"]);
        let vectorcall_helpers =
            import_user_names_for_symbols(&built, &[DP_JIT_PY_VECTORCALL_IMPORT.symbol]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &tuple_new_helpers),
            1,
            "Tuple should allocate via the tuple runtime helper"
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &tuple_set_item_helpers),
            1,
            "Tuple should fill via the fresh-tuple runtime helper"
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &public_tuple_set_item_helpers),
            0,
            "Tuple should not call PyTuple_SetItem for fresh tuple stores"
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &vectorcall_helpers),
            0,
            "Tuple should not call a Python helper through vectorcall"
        );
    }

    #[test]
    fn specialized_jit_import_helpers_use_direct_external_refs() {
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
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = constants.module_constants;
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let built =
            build_test_jit_function_with_constants(&module, &function, &blocks, &module_constants);

        let vectorcall_helpers =
            import_user_names_for_symbols(&built, &[DP_JIT_PY_VECTORCALL_IMPORT.symbol]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &vectorcall_helpers),
            1,
            "generic positional calls should still call the vectorcall helper"
        );
        assert_eq!(
            direct_call_colocated_flags_to_runtime_helpers(&built.ctx.func, &vectorcall_helpers),
            vec![false],
            "imported helpers should be direct external refs, not colocated local trampolines"
        );
    }
}
