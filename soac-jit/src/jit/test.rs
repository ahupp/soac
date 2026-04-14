use super::*;
use soac_blockpy::block_py::{
    BinOp, BinOpKind, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm, Call,
    CallArgKeyword, CallArgPositional, CallDirect, CellLocation, ChildVisitable, ClosureInit,
    ClosureSlot, CodegenBlock, CounterDef, CounterSite, Del, DelItem, FunctionId, FunctionName,
    HasMeta, HasSemanticInstrId, InstrCodegen, InstrResolved, Literal, LiteralValue, Load,
    LocalLocation, Meta, ModuleNameGen, NameLocation, NumberLiteral, NumberLiteralValue, Param,
    ParamKind, ParamSpec, ResolvedName, StorageLayout, Store, StringLiteral, Visit, VisitMut,
    WithMeta,
};
use soac_blockpy::passes::{
    CodegenModuleShape, instrument_bb_module_with_block_entry_counters,
    instrument_bb_module_with_call_target_counters, validate_codegen_instr_ids,
};
mod tests {
    use super::*;
    use crate::counter_dump::{
        CounterDumpRecord, CounterDumpRow, CounterDumpTypeKey, CounterDumpTypeKeyLayout,
        CounterDumpTypeTableEntry, write_counter_dump_records,
    };
    use crate::jit::direct_abi::RuntimePrimitiveId;
    use cranelift_codegen::cursor::Cursor;
    use pyo3::types::{PyAnyMethods, PyDictMethods, PyModule};
    use pyo3::{Python, ffi};
    use ruff_python_ast as ast;
    use std::ffi::c_void;
    use std::mem::size_of;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static CAPSULE_DESTROYED: AtomicBool = AtomicBool::new(false);
    static NEXT_TEST_WORK_DIR_ID: AtomicUsize = AtomicUsize::new(0);

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
            let lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
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
                    FunctionId::new(0, 1)
                ),
                direct_function_symbol_scope_for_shared_state(
                    second.as_ref(),
                    FunctionId::new(0, 1)
                )
            );
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
        std::fs::create_dir_all(&work_dir).expect("test work dir should exist");
        work_dir
    }

    fn write_test_counter_dump(path: &Path, record: &CounterDumpRecord) {
        write_counter_dump_records(path, std::iter::once(record))
            .expect("test counter dump should be writable");
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
        BlockTerm::Raise(soac_blockpy::block_py::TermRaise { exc: None })
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
            kind: soac_blockpy::block_py::FunctionKind::Function,
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
        predeclared_direct_functions: Option<&HashMap<FunctionId, DeclaredJitFunction>>,
        options: BuildSpecializedFunctionOptions,
    ) -> Result<BuiltSpecializedFunction, String> {
        let value_facts = infer_jit_value_facts(module);
        let jit_module_local_plan = plan_jit_module_locals(module, &value_facts)?;
        let jit_module_deopt_resume_plan = plan_jit_deopt_resume_module(module, &value_facts)?;
        let jit_local_plan = jit_module_local_plan
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT local plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let jit_deopt_resume_plan = jit_module_deopt_resume_plan
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing JIT deopt resume plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        build_cranelift_run_bb_specialized_function(
            jit_module,
            blocks,
            module,
            function,
            &value_facts,
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
            term: BlockTerm::IfTerm(soac_blockpy::block_py::TermIf {
                test: name_expr(test_runtime_name("TRUE")),
                then_label,
                else_label,
            }),
            params: vec![],
            exc_edge: None,
        };
        let then_block = CodegenBlock {
            label: then_label,
            body: vec![],
            term: ret_term(constants.int_expr(1)),
            params: vec![],
            exc_edge: None,
        };
        let else_block = CodegenBlock {
            label: else_label,
            body: vec![],
            term: ret_term(constants.int_expr(0)),
            params: vec![],
            exc_edge: None,
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
            BlockTerm::Raise(soac_blockpy::block_py::TermRaise {
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
            term: BlockTerm::BranchTable(soac_blockpy::block_py::TermBranchTable {
                index: constants.int_expr(0),
                targets: vec![case_label],
                default_label,
            }),
            params: vec![],
            exc_edge: None,
        };
        let case_block = CodegenBlock {
            label: case_label,
            body: vec![],
            term: ret_term(constants.int_expr(1)),
            params: vec![],
            exc_edge: None,
        };
        let default_block = CodegenBlock {
            label: default_label,
            body: vec![],
            term: ret_term(constants.int_expr(2)),
            params: vec![],
            exc_edge: None,
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
    fn specialized_jit_body_calls_compile_via_effect_only_typed_ops() {
        let mut constants = TestConstantPool::default();
        let call = Call::new(
            name_expr(test_runtime_name("tuple_values")),
            vec![CallArgPositional::Positional(constants.int_expr(1))],
            Vec::<CallArgKeyword<InstrCodegen>>::new(),
        );
        let function = with_single_test_block(
            test_function(),
            vec![expr_stmt(op_expr(call))],
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
    fn typed_result_demand_plan_marks_statement_roots_effect_only() {
        let mut constants = TestConstantPool::default();
        let instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let function = with_single_test_block(
            test_function(),
            vec![expr_stmt(with_instr_id(constants.int_expr(1), instr_id))],
            ret_term(constants.int_expr(2)),
        );
        let typed_function =
            lower_typed_function_if_tests_to_truthy(lower_codegen_function_to_typed(function));
        let plan = plan_typed_result_demands(&typed_function);

        assert_eq!(
            plan.demand_for_instr_id(instr_id),
            Some(ResultDemand::EffectOnly)
        );
    }

    #[test]
    fn typed_result_demand_plan_marks_local_store_rhs_pyobject_owned() {
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
        let plan = plan_typed_result_demands(&typed_function);

        assert_eq!(
            plan.demand_for_instr_id(store_instr_id),
            Some(ResultDemand::EffectOnly)
        );
        assert_eq!(
            plan.demand_for_instr_id(rhs_instr_id),
            Some(ResultDemand::PYOBJECT_OWNED)
        );
    }

    #[test]
    fn typed_result_demand_plan_marks_call_inputs_pyobject_borrowed_ok() {
        let mut constants = TestConstantPool::default();
        let call_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let func_instr_id = InstrId::new(BlockLabel::from_index(0), 1);
        let positional_instr_id = InstrId::new(BlockLabel::from_index(0), 2);
        let keyword_instr_id = InstrId::new(BlockLabel::from_index(0), 3);
        let call = with_instr_id(
            op_expr(Call::new(
                with_instr_id(name_expr(test_runtime_name("tuple_values")), func_instr_id),
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
        let plan = plan_typed_result_demands(&typed_function);

        assert_eq!(
            plan.demand_for_instr_id(call_instr_id),
            Some(ResultDemand::EffectOnly)
        );
        assert_eq!(
            plan.demand_for_instr_id(func_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            plan.demand_for_instr_id(positional_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            plan.demand_for_instr_id(keyword_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
    }

    #[test]
    fn typed_result_demand_plan_marks_direct_call_inputs_pyobject_borrowed_ok() {
        let mut constants = TestConstantPool::default();
        let call_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let callable_instr_id = InstrId::new(BlockLabel::from_index(0), 1);
        let positional_instr_id = InstrId::new(BlockLabel::from_index(0), 2);
        let call = with_instr_id(
            InstrCodegen::CallDirect(CallDirect::new(
                with_instr_id(name_expr(test_global_name("callee")), callable_instr_id),
                FunctionId::new(0, 1),
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
        let plan = plan_typed_result_demands(&typed_function);

        assert_eq!(
            plan.demand_for_instr_id(call_instr_id),
            Some(ResultDemand::EffectOnly)
        );
        assert_eq!(
            plan.demand_for_instr_id(callable_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            plan.demand_for_instr_id(positional_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
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
    fn planned_pyobject_input_borrowed_ok_for_codegen_expr_uses_result_demand_plan() {
        let mut constants = TestConstantPool::default();
        let borrowed_id = InstrId::new(BlockLabel::from_index(0), 1);
        let owned_id = InstrId::new(BlockLabel::from_index(0), 2);
        let bool_id = InstrId::new(BlockLabel::from_index(0), 3);
        let missing_id = InstrId::new(BlockLabel::from_index(0), 4);
        let borrowed_expr = with_instr_id(constants.int_expr(1), borrowed_id);
        let owned_expr = with_instr_id(constants.int_expr(2), owned_id);
        let bool_expr = with_instr_id(constants.int_expr(3), bool_id);
        let missing_expr = with_instr_id(constants.int_expr(4), missing_id);
        let no_id_expr = constants.int_expr(5);
        let mut plan = ResultDemandPlan::default();
        plan.demands_by_instr_id
            .insert(borrowed_id, ResultDemand::PYOBJECT_BORROWED_OK);
        plan.demands_by_instr_id
            .insert(owned_id, ResultDemand::PYOBJECT_OWNED);
        plan.demands_by_instr_id
            .insert(bool_id, ResultDemand::I32_BOOL01);

        assert_eq!(
            planned_pyobject_input_borrowed_ok_for_codegen_expr(&plan, &borrowed_expr),
            Some(true)
        );
        assert_eq!(
            planned_pyobject_input_borrowed_ok_for_codegen_expr(&plan, &owned_expr),
            Some(false)
        );
        assert_eq!(
            planned_pyobject_input_borrowed_ok_for_codegen_expr(&plan, &bool_expr),
            Some(false)
        );
        assert_eq!(
            planned_pyobject_input_borrowed_ok_for_codegen_expr(&plan, &missing_expr),
            None
        );
        assert_eq!(
            planned_pyobject_input_borrowed_ok_for_codegen_expr(&plan, &no_id_expr),
            None
        );
    }

    #[test]
    fn typed_result_demand_plan_marks_intrinsic_inputs_pyobject_borrowed_ok() {
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
        let plan = plan_typed_result_demands(&typed_function);

        assert_eq!(
            plan.demand_for_instr_id(binop_instr_id),
            Some(ResultDemand::EffectOnly)
        );
        assert_eq!(
            plan.demand_for_instr_id(left_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
        assert_eq!(
            plan.demand_for_instr_id(right_instr_id),
            Some(ResultDemand::PYOBJECT_BORROWED_OK)
        );
    }

    #[test]
    fn typed_result_demand_plan_marks_branch_tests_i32_bool01() {
        let mut constants = TestConstantPool::default();
        let function = test_function();
        let entry_label = function.name_gen.next_block_name();
        let then_label = function.name_gen.next_block_name();
        let else_label = function.name_gen.next_block_name();
        let test_instr_id = InstrId::new(entry_label, 0);
        let entry = CodegenBlock {
            label: entry_label,
            body: vec![],
            term: BlockTerm::IfTerm(soac_blockpy::block_py::TermIf {
                test: with_instr_id(constants.int_expr(0), test_instr_id),
                then_label,
                else_label,
            }),
            params: vec![],
            exc_edge: None,
        };
        let then_block = CodegenBlock {
            label: then_label,
            body: vec![],
            term: ret_term(constants.int_expr(1)),
            params: vec![],
            exc_edge: None,
        };
        let else_block = CodegenBlock {
            label: else_label,
            body: vec![],
            term: ret_term(constants.int_expr(2)),
            params: vec![],
            exc_edge: None,
        };
        let function = with_test_blocks(function, vec![entry, then_block, else_block]);
        let typed_function =
            lower_typed_function_if_tests_to_truthy(lower_codegen_function_to_typed(function));
        let plan = plan_typed_result_demands(&typed_function);

        assert_eq!(
            plan.demand_for_instr_id(test_instr_id),
            Some(ResultDemand::I32_BOOL01)
        );
    }

    #[test]
    fn typed_result_demand_plan_marks_branch_table_indices_i64_index() {
        let mut constants = TestConstantPool::default();
        let function = test_function();
        let entry_label = function.name_gen.next_block_name();
        let case_label = function.name_gen.next_block_name();
        let default_label = function.name_gen.next_block_name();
        let index_instr_id = InstrId::new(entry_label, 0);
        let entry = CodegenBlock {
            label: entry_label,
            body: vec![],
            term: BlockTerm::BranchTable(soac_blockpy::block_py::TermBranchTable {
                index: with_instr_id(constants.int_expr(0), index_instr_id),
                targets: vec![case_label],
                default_label,
            }),
            params: vec![],
            exc_edge: None,
        };
        let case_block = CodegenBlock {
            label: case_label,
            body: vec![],
            term: ret_term(constants.int_expr(1)),
            params: vec![],
            exc_edge: None,
        };
        let default_block = CodegenBlock {
            label: default_label,
            body: vec![],
            term: ret_term(constants.int_expr(2)),
            params: vec![],
            exc_edge: None,
        };
        let function = with_test_blocks(function, vec![entry, case_block, default_block]);
        let typed_function = lower_codegen_function_to_typed(function);
        let plan = plan_typed_result_demands(&typed_function);

        assert_eq!(
            plan.demand_for_instr_id(index_instr_id),
            Some(ResultDemand::I64_INDEX)
        );
    }

    #[test]
    fn typed_result_demand_plan_marks_return_values_pyobject_owned() {
        let mut constants = TestConstantPool::default();
        let return_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(with_instr_id(constants.int_expr(2), return_instr_id)),
        );
        let typed_function = lower_codegen_function_to_typed(function);
        let plan = plan_typed_result_demands(&typed_function);

        assert_eq!(
            plan.demand_for_instr_id(return_instr_id),
            Some(ResultDemand::PYOBJECT_OWNED)
        );
    }

    #[test]
    fn typed_result_demand_plan_marks_raise_values_pyobject_owned() {
        let mut constants = TestConstantPool::default();
        let raise_instr_id = InstrId::new(BlockLabel::from_index(0), 0);
        let function = with_single_test_block(
            test_function(),
            vec![],
            BlockTerm::Raise(soac_blockpy::block_py::TermRaise {
                exc: Some(with_instr_id(constants.int_expr(2), raise_instr_id)),
            }),
        );
        let typed_function = lower_codegen_function_to_typed(function);
        let plan = plan_typed_result_demands(&typed_function);

        assert_eq!(
            plan.demand_for_instr_id(raise_instr_id),
            Some(ResultDemand::PYOBJECT_OWNED)
        );
    }

    fn direct_call_expr(function_id: FunctionId) -> InstrCodegen {
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
        let mut state =
            ProcessJitState::new(&compile_session).expect("process JIT state should initialize");
        let first = test_function();
        let mut second = test_function();
        second.params.params.push(Param {
            name: "x".into(),
            kind: ParamKind::Any,
            has_default: false,
        });

        let first_decl = state
            .declare_direct_function(&first, None)
            .expect("first function should declare");
        let first_decl_again = state
            .declare_direct_function(&first, None)
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
                    points: Vec::new(),
                }),
            )
            .expect("first function should mark ready");
        let ready_handle = state
            .ready_direct_function(&first)
            .expect("first function should be ready");
        assert!(std::sync::Arc::ptr_eq(&first_handle, &ready_handle));
        assert!(state.ready_direct_function(&second).is_none());

        let second_decl = state
            .declare_direct_function(&second, None)
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

            let batch = collect_process_jit_batch_functions(
                &session,
                &caller,
                &caller_state.codegen_constants,
                Some(caller_state.as_ref()),
            )
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
        let module = test_module(module_name_gen, vec![function.clone()]);
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let batch =
            collect_process_jit_batch_functions(&session, &function, &module_constants, None)
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
            assert_eq!(
                first_entry_record.id.ordinal, 0,
                "runtime deopt records should preserve planned ordinal ids"
            );
            let first_entry_description = first_deopt_table
                .describe_record_ordinal(first_entry_record.id.ordinal as i64)
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
            assert_eq!(
                compiled_direct_deopt_table_ptr(first_handle.raw_handle())
                    .expect("root deopt table pointer should be available"),
                std::sync::Arc::as_ptr(&first_deopt_table) as ObjPtr,
                "compiled direct handle should expose the runtime deopt table pointer"
            );
            let deopt_result = unsafe {
                crate::jit::specialized_helpers::dp_jit_deopt_unimplemented(
                    std::sync::Arc::as_ptr(&first_deopt_table) as ObjPtr,
                    first_entry_record.id.ordinal as i64,
                )
            };
            assert!(
                deopt_result.is_null(),
                "placeholder deopt helper should return a null error sentinel"
            );
            let deopt_error = pyo3::PyErr::fetch(py);
            let deopt_error_text = deopt_error.to_string();
            assert!(
                deopt_error_text.contains("JIT deopt helper is not implemented")
                    && deopt_error_text.contains(&format!("function {}", first.function_id))
                    && deopt_error_text.contains("record 0"),
                "placeholder deopt helper should report the planned runtime record: {deopt_error_text}"
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

    fn build_test_jit_function_with_operator_specializations(
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
        module_constants: Vec<InstrResolved>,
        operator_specializations: &[(InstrId, u64)],
    ) -> (JITModule, BuiltSpecializedFunction) {
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = module_constants;
        let function = module.callable_defs[0].clone();
        let module_name = "counter_test";
        let soac_work_dir = fresh_test_work_dir("test-work");
        write_test_counter_dump(
            soac_work_dir.join("profile.bin").as_path(),
            &CounterDumpRecord {
                module_name: module_name.to_string(),
                package_name: None,
                rows: operator_specializations
                    .iter()
                    .enumerate()
                    .map(|(index, (instr_id, shape))| CounterDumpRow {
                        counter_id: u32::try_from(index)
                            .expect("test specialization count should fit in u32"),
                        scope: "this".to_string(),
                        kind: "operator_hot_shapes".to_string(),
                        site_kind: "runtime".to_string(),
                        function_id: Some(function.function_id),
                        current_function_id: Some(function.function_id),
                        instr_id: Some(*instr_id),
                        function_qualname: Some(function.names.qualname.clone()),
                        block_label: None,
                        value: 1,
                        observed_value: Some(*shape),
                        max_overcount: Some(0),
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
        unsafe {
            std::env::set_var("SOAC_WORK_DIR", &soac_work_dir);
            std::env::set_var("SOAC_OPT_MODE", "apply");
        }
        crate::initialize_test_python();

        let (jit_module, built) = Python::attach(|py| {
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
            (jit_module, built)
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

        (jit_module, built)
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
    fn field_index_specializations_resolve_type_key_without_module_globals() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "field_index_specializations_resolve_type_key_without_module_globals",
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

            write_test_counter_dump(
                soac_work_dir.join("profile.bin").as_path(),
                &CounterDumpRecord {
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

            let specializations = load_field_index_specializations()
                .expect("field specializations should load from type table");
            let x_specializations = specializations
                .get("x")
                .expect("x specialization should be present");
            assert_eq!(x_specializations.len(), 1);
            assert_eq!(
                x_specializations[0].owner_type_ref,
                RelocTypeRef::TypeKey(CounterDumpTypeKey {
                    module_name: "field_type_test".to_string(),
                    qualname: "Point".to_string(),
                })
            );
            assert_eq!(
                resolve_reloc_type_ref_to_type(&x_specializations[0].owner_type_ref)
                    .expect("type key should resolve back to a live type"),
                Some(owner_type)
            );
            assert_eq!(x_specializations[0].expected_index, 0);
            assert_ne!(x_specializations[0].type_version, 0);

            modules
                .del_item("field_type_test")
                .expect("test module should be removed");
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
    fn field_index_specializations_prime_owner_type_key_layouts() {
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
            let mut lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
                r#"
def write_point(point, value):
    point.x = value
    return point.x
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            instrument_bb_module_with_call_target_counters(&mut lowered);
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
            let field_counter_sites = lowered
                .counter_defs
                .iter()
                .filter_map(|counter| match &counter.site {
                    CounterSite::Runtime {
                        function_id: Some(counter_function_id),
                        instr_id: Some(counter_instr_id),
                    } if *counter_function_id == function.function_id
                        && counter.kind.starts_with("field_indexed") =>
                    {
                        Some(format!("{}@{:?}", counter.kind, counter_instr_id))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let hit_counter_id = lowered
                .counter_defs
                .iter()
                .find_map(|counter| match &counter.site {
                    CounterSite::Runtime {
                        function_id: Some(counter_function_id),
                        instr_id: Some(counter_instr_id),
                    } if counter.kind == "field_indexed_hit"
                        && *counter_function_id == function.function_id
                        && *counter_instr_id == setattr_instr_id =>
                    {
                        Some(counter.id)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!(
                        "missing field_indexed_hit counter for SetAttr {:?} in {:?}",
                        setattr_instr_id, field_counter_sites
                    )
                });
            let fallback_counter_id = lowered
                .counter_defs
                .iter()
                .find_map(|counter| match &counter.site {
                    CounterSite::Runtime {
                        function_id: Some(counter_function_id),
                        instr_id: Some(counter_instr_id),
                    } if counter.kind == "field_indexed_fallback"
                        && *counter_function_id == function.function_id
                        && *counter_instr_id == setattr_instr_id =>
                    {
                        Some(counter.id)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!(
                        "missing field_indexed_fallback counter for SetAttr {:?} in {:?}",
                        setattr_instr_id, field_counter_sites
                    )
                });

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
                shared_state.counter_value(hit_counter_id),
                1,
                "apply-mode SetAttr should take the indexed-store fast path"
            );
            assert_eq!(
                shared_state.counter_value(fallback_counter_id),
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
            let loaded_specializations =
                load_field_index_specializations().expect("field specializations should load");
            assert_eq!(
                loaded_specializations
                    .values()
                    .map(std::vec::Vec::len)
                    .sum::<usize>(),
                5,
                "each profiled Record field should produce one specialization"
            );
            assert!(
                cached_split_key_layout(py, owner_type).starts_with(&[
                    ("PtrComp".to_string(), 0),
                    ("Discr".to_string(), 1),
                    ("EnumComp".to_string(), 2),
                    ("IntComp".to_string(), 3),
                    ("StringComp".to_string(), 4),
                ]),
                "SOAC priming should establish the profile-order split-key layout"
            );

            let mut lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
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
            instrument_bb_module_with_call_target_counters(&mut lowered);
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
                    shared_state
                        .lowered_module
                        .counter_defs
                        .iter()
                        .find_map(|counter| match &counter.site {
                            CounterSite::Runtime {
                                function_id: Some(counter_function_id),
                                instr_id: Some(counter_instr_id),
                            } if counter.kind == "field_indexed_hit"
                                && *counter_function_id == function.function_id
                                && counter_instr_id == setattr_instr_id =>
                            {
                                Some(counter.id)
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| {
                            panic!("missing field_indexed_hit counter for {setattr_instr_id:?}")
                        })
                })
                .collect::<Vec<_>>();
            let fallback_counter_ids = setattr_instr_ids
                .iter()
                .map(|setattr_instr_id| {
                    shared_state
                        .lowered_module
                        .counter_defs
                        .iter()
                        .find_map(|counter| match &counter.site {
                            CounterSite::Runtime {
                                function_id: Some(counter_function_id),
                                instr_id: Some(counter_instr_id),
                            } if counter.kind == "field_indexed_fallback"
                                && *counter_function_id == function.function_id
                                && counter_instr_id == setattr_instr_id =>
                            {
                                Some(counter.id)
                            }
                            _ => None,
                        })
                        .unwrap_or_else(|| {
                            panic!(
                                "missing field_indexed_fallback counter for {setattr_instr_id:?}"
                            )
                        })
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

            for counter_id in hit_counter_ids {
                assert_eq!(
                    shared_state.counter_value(counter_id),
                    1,
                    "constructor SetAttr should take the indexed-store fast path"
                );
            }
            for counter_id in fallback_counter_ids {
                assert_eq!(
                    shared_state.counter_value(counter_id),
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

            let mut lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
                r#"
def read_point(point):
    return point.x
"#,
            )
            .expect("lowering should succeed")
            .codegen_module;
            instrument_bb_module_with_call_target_counters(&mut lowered);
            let function = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.bind_name == "read_point")
                .expect("missing lowered function read_point")
                .clone();
            let getattr_instr_id = function
                .blocks
                .iter()
                .find_map(|block| {
                    block
                        .body
                        .iter()
                        .find_map(|expr| match expr {
                            InstrCodegen::GetAttr(_) => Some(expr.semantic_instr_id()),
                            _ => None,
                        })
                        .or_else(|| match &block.term {
                            BlockTerm::Return(InstrCodegen::GetAttr(expr)) => {
                                Some(expr.semantic_instr_id())
                            }
                            _ => None,
                        })
                })
                .expect("read_point should contain a GetAttr");
            let field_counter_sites = lowered
                .counter_defs
                .iter()
                .filter_map(|counter| match &counter.site {
                    CounterSite::Runtime {
                        function_id: Some(counter_function_id),
                        instr_id: Some(counter_instr_id),
                    } if *counter_function_id == function.function_id
                        && counter.kind.starts_with("field_indexed") =>
                    {
                        Some(format!("{}@{:?}", counter.kind, counter_instr_id))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let hit_counter_id = lowered
                .counter_defs
                .iter()
                .find_map(|counter| match &counter.site {
                    CounterSite::Runtime {
                        function_id: Some(counter_function_id),
                        instr_id: Some(counter_instr_id),
                    } if counter.kind == "field_indexed_hit"
                        && *counter_function_id == function.function_id
                        && *counter_instr_id == getattr_instr_id =>
                    {
                        Some(counter.id)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!(
                        "missing field_indexed_hit counter for GetAttr {:?} in {:?}",
                        getattr_instr_id, field_counter_sites
                    )
                });
            let fallback_counter_id = lowered
                .counter_defs
                .iter()
                .find_map(|counter| match &counter.site {
                    CounterSite::Runtime {
                        function_id: Some(counter_function_id),
                        instr_id: Some(counter_instr_id),
                    } if counter.kind == "field_indexed_fallback"
                        && *counter_function_id == function.function_id
                        && *counter_instr_id == getattr_instr_id =>
                    {
                        Some(counter.id)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!(
                        "missing field_indexed_fallback counter for GetAttr {:?} in {:?}",
                        getattr_instr_id, field_counter_sites
                    )
                });

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
                shared_state.counter_value(hit_counter_id),
                1,
                "apply-mode GetAttr should take the indexed-load fast path"
            );
            assert_eq!(
                shared_state.counter_value(fallback_counter_id),
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
            inline_runtime_support_calls(&mut jit_module, &mut built.ctx, "test")
                .expect("runtime support helpers should inline");
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

    fn imported_symbol_names(built: &BuiltSpecializedFunction) -> Vec<&'static str> {
        let mut symbols: Vec<&'static str> = built.import_id_to_symbol.values().copied().collect();
        symbols.sort_unstable();
        symbols.dedup();
        symbols
    }

    #[test]
    fn deopt_unimplemented_exit_call_uses_function_env_deopt_table_and_ordinal() {
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
                &DP_JIT_DEOPT_UNIMPLEMENTED_IMPORT,
            );
            let function_env_value = fb.block_params(entry)[0];
            let result = emit_deopt_unimplemented_exit_call(
                &mut fb,
                JitDeoptExitRef {
                    function_env_value,
                    record_ordinal: 42,
                },
                deopt_ref,
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
                (*symbol == "dp_jit_deopt_unimplemented")
                    .then(|| ir::UserExternalName::new(0, *import_id))
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

        fn remove(name: &'static str) -> Self {
            let old_value = std::env::var_os(name);
            unsafe { std::env::remove_var(name) };
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
    fn jit_refcount_emission_env_defaults_to_enabled() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        {
            let _env = EnvVarGuard::remove(SOAC_JIT_EMIT_REFCOUNTS_ENV);
            assert!(
                jit_refcount_emission_enabled(),
                "refcount emission should be enabled by default"
            );
        }
        for value in ["0", "false", "False", "no", "off"] {
            let _env = EnvVarGuard::set(SOAC_JIT_EMIT_REFCOUNTS_ENV, value);
            assert!(
                !jit_refcount_emission_enabled(),
                "{SOAC_JIT_EMIT_REFCOUNTS_ENV}={value:?} should disable refcount emission"
            );
        }
        for value in ["", "1", "true", "yes", "on"] {
            let _env = EnvVarGuard::set(SOAC_JIT_EMIT_REFCOUNTS_ENV, value);
            assert!(
                jit_refcount_emission_enabled(),
                "{SOAC_JIT_EMIT_REFCOUNTS_ENV}={value:?} should keep refcount emission enabled"
            );
        }
    }

    #[test]
    fn runtime_support_inliner_uses_noop_refcount_helpers_when_disabled() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        let disabled_insts = {
            let _env = EnvVarGuard::set(SOAC_JIT_EMIT_REFCOUNTS_ENV, "0");
            let (_compile_session, mut jit_module, mut ctx, _wrapper_id, helper_names) =
                unsafe { build_runtime_refcount_smoke_context() };
            let before_calls = count_direct_calls_to_runtime_helpers(&ctx.func, &helper_names);
            assert_eq!(
                before_calls, 2,
                "test caller should start with direct incref/decref calls"
            );

            let inlined = inline_runtime_support_calls(
                &mut jit_module,
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
            let _env = EnvVarGuard::set(SOAC_JIT_EMIT_REFCOUNTS_ENV, "1");
            let (_compile_session, mut jit_module, mut ctx, _wrapper_id, _helper_names) =
                unsafe { build_runtime_refcount_smoke_context() };
            inline_runtime_support_calls(
                &mut jit_module,
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
            "example source helper should load from soac-runtime CLIF as an ir::Function"
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
        let lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
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
        let lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
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
        let lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
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
        let lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
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
        let lowered = soac_blockpy::lower_python_to_blockpy_for_testing(source)
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
    fn specialized_jit_exact_int_binop_uses_operator_fast_path() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialized_jit_exact_int_binop_uses_operator_fast_path",
        ) {
            return;
        }
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
        let mut baseline_module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        baseline_module.module_constants = constants.module_constants.clone();
        let baseline_function = baseline_module.callable_defs[0].clone();
        let baseline_module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&baseline_module);
        let baseline_built = build_test_jit_function_with_constants(
            &baseline_module,
            &baseline_function,
            &blocks,
            &baseline_module_constants,
        );
        let baseline_symbolic_globals = count_symbolic_global_values(&baseline_built.ctx.func);
        let (_jit_module, built) = build_test_jit_function_with_operator_specializations(
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
        let helper_names = import_user_names_for_symbols(&built, &["dp_jit_exact_long_add_slot"]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &helper_names),
            1,
            "exact-int binop specialization should call the profiled PyLong number slot",
        );
        assert!(
            function_contains_iconst_imm(
                &built.ctx.func,
                crate::operator_specialization::pack_binary_shape(
                    crate::operator_specialization::ExactTypeTag::Int,
                    crate::operator_specialization::ExactTypeTag::Int,
                ) as i64,
            ),
            "exact-int binop specialization should guard on the profiled exact-int shape",
        );
        assert!(
            !function_contains_iconst_imm(
                &built.ctx.func,
                std::ptr::addr_of_mut!(PyLong_Type) as i64
            ),
            "exact-int binop specialization should not bake the PyLong type pointer into the function body",
        );
        assert!(
            count_symbolic_global_values(&built.ctx.func) > baseline_symbolic_globals,
            "exact-int binop specialization should add a symbolic global for the profiled type guard",
        );
    }

    #[test]
    fn specialized_jit_exact_int_compare_uses_operator_fast_path() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialized_jit_exact_int_compare_uses_operator_fast_path",
        ) {
            return;
        }
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
        let mut baseline_module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        baseline_module.module_constants = constants.module_constants.clone();
        let baseline_function = baseline_module.callable_defs[0].clone();
        let baseline_module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&baseline_module);
        let baseline_built = build_test_jit_function_with_constants(
            &baseline_module,
            &baseline_function,
            &blocks,
            &baseline_module_constants,
        );
        let baseline_symbolic_globals = count_symbolic_global_values(&baseline_built.ctx.func);
        let (_jit_module, built) = build_test_jit_function_with_operator_specializations(
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
        let helper_names =
            import_user_names_for_symbols(&built, &["dp_jit_exact_long_richcompare_slot"]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &helper_names),
            1,
            "exact-int compare specialization should call the profiled PyLong richcompare slot",
        );
        assert!(
            function_contains_iconst_imm(
                &built.ctx.func,
                crate::operator_specialization::pack_binary_shape(
                    crate::operator_specialization::ExactTypeTag::Int,
                    crate::operator_specialization::ExactTypeTag::Int,
                ) as i64,
            ),
            "exact-int compare specialization should guard on the profiled exact-int shape",
        );
        assert!(
            !function_contains_iconst_imm(
                &built.ctx.func,
                std::ptr::addr_of_mut!(PyLong_Type) as i64
            ),
            "exact-int compare specialization should not bake the PyLong type pointer into the function body",
        );
        assert!(
            count_symbolic_global_values(&built.ctx.func) > baseline_symbolic_globals,
            "exact-int compare specialization should add a symbolic global for the profiled type guard",
        );
    }

    #[test]
    fn specialized_jit_exact_int_unary_uses_operator_fast_path() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialized_jit_exact_int_unary_uses_operator_fast_path",
        ) {
            return;
        }
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
        let mut baseline_module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        baseline_module.module_constants = constants.module_constants.clone();
        let baseline_function = baseline_module.callable_defs[0].clone();
        let baseline_module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&baseline_module);
        let baseline_built = build_test_jit_function_with_constants(
            &baseline_module,
            &baseline_function,
            &blocks,
            &baseline_module_constants,
        );
        let baseline_symbolic_globals = count_symbolic_global_values(&baseline_built.ctx.func);
        let (_jit_module, built) = build_test_jit_function_with_operator_specializations(
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
        let helper_names = import_user_names_for_symbols(&built, &["dp_jit_exact_long_unary_op"]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &helper_names),
            1,
            "exact-int unary specialization should call the direct helper",
        );
        assert!(
            function_contains_iconst_imm(
                &built.ctx.func,
                crate::operator_specialization::pack_unary_shape(
                    crate::operator_specialization::ExactTypeTag::Int,
                ) as i64,
            ),
            "exact-int unary specialization should guard on the profiled exact-int shape",
        );
        assert!(
            !function_contains_iconst_imm(
                &built.ctx.func,
                std::ptr::addr_of_mut!(PyLong_Type) as i64
            ),
            "exact-int unary specialization should not bake the PyLong type pointer into the function body",
        );
        assert!(
            count_symbolic_global_values(&built.ctx.func) > baseline_symbolic_globals,
            "exact-int unary specialization should add a symbolic global for the profiled type guard",
        );
    }

    #[test]
    fn apply_mode_operator_specialization_omits_top_value_counter_helper_imports() {
        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let mut function = test_function();
        let block_label = function.name_gen.next_block_name();
        let instr_id = InstrId::new(block_label, 0);
        function.blocks = vec![CodegenBlock {
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
        }];

        let (_jit_module, built) = build_test_jit_function_with_operator_specializations(
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
        let imported_symbols = imported_symbol_names(&built);
        assert!(
            !imported_symbols.contains(&DP_JIT_RECORD_TOP_VALUE_SAMPLE_IMPORT.symbol),
            "apply-mode specialization should not import top-value counter helpers: {:?}",
            imported_symbols
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
                let mut baseline = soac_blockpy::lower_python_to_blockpy_for_testing(source)
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

                instrument_bb_module_with_block_entry_counters(&mut baseline);

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
                let baseline = soac_blockpy::lower_python_to_blockpy_for_testing(source)
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

                let mut instrumented = soac_blockpy::lower_python_to_blockpy_for_testing(source)
                    .expect("lowering should succeed")
                    .codegen_module;
                instrument_bb_module_with_call_target_counters(&mut instrumented);
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
    fn render_specialized_jit_ignores_field_index_specializations_without_runtime_state() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "render_specialized_jit_ignores_field_index_specializations_without_runtime_state",
        ) {
            return;
        }
        let old_soac_work_dir = std::env::var_os("SOAC_WORK_DIR");
        let old_soac_opt_mode = std::env::var_os("SOAC_OPT_MODE");
        let soac_work_dir = fresh_test_work_dir("test-work");
        unsafe {
            std::env::set_var("SOAC_WORK_DIR", &soac_work_dir);
            std::env::set_var("SOAC_OPT_MODE", "verify");
        }

        write_test_counter_dump(
            soac_work_dir.join("profile.bin").as_path(),
            &CounterDumpRecord {
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

        let blocks = [1usize as ObjPtr];
        let mut constants = TestConstantPool::default();
        let function =
            with_single_test_block(test_function(), vec![], ret_term(constants.int_expr(7)));
        let rendered = render_test_jit_function_with_module_constants(
            &function,
            &blocks,
            constants.module_constants,
        );
        assert!(
            rendered.contains("function"),
            "rendering should succeed without runtime state even when field specializations are configured:\n{rendered}"
        );

        match old_soac_work_dir {
            Some(value) => unsafe { std::env::set_var("SOAC_WORK_DIR", value) },
            None => unsafe { std::env::remove_var("SOAC_WORK_DIR") },
        }
        match old_soac_opt_mode {
            Some(value) => unsafe { std::env::set_var("SOAC_OPT_MODE", value) },
            None => unsafe { std::env::remove_var("SOAC_OPT_MODE") },
        }
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
        instrument_bb_module_with_call_target_counters(&mut module);
        let function = module.callable_defs[0].clone();
        let module_constants =
            crate::module_constants::ModuleCodegenConstants::collect_from_module(&module);
        let built = build_test_jit_function_with_constants_and_options(
            &module,
            &function,
            &blocks,
            &module_constants,
            BuildSpecializedFunctionOptions {
                indexed_global_guard_miss_deopt_stub: true,
            },
        );
        let deopt_helpers = import_user_names_for_symbols(&built, &["dp_jit_deopt_unimplemented"]);
        let slow_global_helpers =
            import_user_names_for_symbols(&built, &["soac_runtime_load_global_slow"]);
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
            1,
            "{case_name}: test deopt guard mode should call the placeholder deopt helper"
        );
        assert_eq!(
            count_cold_block_direct_calls_to_runtime_helpers(&built.ctx.func, &deopt_helpers),
            1,
            "{case_name}: test deopt guard mode should isolate the deopt helper call in a cold block"
        );
        assert_eq!(
            count_direct_calls_to_runtime_helpers(&built.ctx.func, &slow_global_helpers),
            0,
            "{case_name}: test deopt guard mode should not emit the local slow global-load fallback"
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
    fn indexed_global_term_guard_miss_can_target_cold_deopt_stub() {
        let function = with_single_test_block(
            test_function(),
            vec![],
            ret_term(op_expr(Load::new(test_global_name("x")))),
        );
        assert_indexed_global_guard_miss_targets_cold_deopt_stub(function, "term load");
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
            ret_term(op_expr(soac_blockpy::block_py::CellRef::new(
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
    fn render_specialized_jit_local_store_of_deleted_sentinel_behaves_like_delete() {
        let blocks = [1usize as ObjPtr];
        let mut function = with_single_test_block(
            test_function(),
            vec![assign_stmt(
                test_name("x"),
                name_expr(test_runtime_name("DELETED")),
            )],
            ret_term(name_expr(test_name("x"))),
        );
        set_stack_slots(&mut function, &["x"]);
        let rendered = render_test_jit_function(&function, &blocks);
        assert!(
            rendered.contains("call dp_jit_raise_deleted_name_error"),
            "store of the deleted sentinel should clear the local binding, not keep a local-only DELETED object:\n{rendered}"
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
                term: BlockTerm::IfTerm(soac_blockpy::block_py::TermIf {
                    test: none_expr(),
                    then_label: hot_label,
                    else_label: cold_label,
                }),
                params: vec![],
                exc_edge: None,
            },
            CodegenBlock {
                label: hot_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
            },
            CodegenBlock {
                label: cold_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
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
                term: BlockTerm::IfTerm(soac_blockpy::block_py::TermIf {
                    test: none_expr(),
                    then_label: hot_label,
                    else_label: cold_label,
                }),
                params: vec![],
                exc_edge: None,
            },
            CodegenBlock {
                label: hot_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
            },
            CodegenBlock {
                label: cold_label,
                body: vec![],
                term: ret_term(none_expr()),
                params: vec![],
                exc_edge: None,
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

    #[test]
    fn specialized_jit_type_constructors_use_constructor_fast_path() {
        if crate::run_test_in_isolated_process_if_needed(
            module_path!(),
            "specialized_jit_type_constructors_use_constructor_fast_path",
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

        unsafe {
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
                let mut caller_function = BlockPyFunction {
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
                assign_missing_test_instr_ids(&mut init_function);
                assign_missing_test_instr_ids(&mut caller_function);

                let mut module = test_module(
                    ModuleNameGen::new(0),
                    vec![init_function.clone(), caller_function.clone()],
                );
                module.global_names = vec!["Record".into()];
                module.module_constants = constants.module_constants;
                write_test_counter_dump(
                    soac_work_dir.join("profile.bin").as_path(),
                    &CounterDumpRecord {
                        module_name: "counter_test".to_string(),
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
                            observed_value: Some(init_function.function_id.packed()),
                            max_overcount: Some(0),
                        }],
                        module_keys: Vec::new(),
                        type_keys: Vec::new(),
                        type_table: Vec::new(),
                    },
                );

                let shared_state = crate::module_type::build_shared_state_for_testing(
                    py,
                    module,
                    "counter_test",
                    "",
                )
                .expect("shared state should build");
                let runtime = build_test_module_runtime(py, shared_state.clone());
                let globals = pyo3::Bound::<pyo3::PyAny>::from_borrowed_ptr(
                    py,
                    runtime.mod_ctx.globals_obj.cast(),
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
                let cls = globals
                    .get_item("Record")
                    .expect("class lookup should not error")
                    .expect("class should exist");
                let owner_type = cls.as_ptr() as *mut ffi::PyTypeObject;
                let init_function_obj =
                    ffi::PyDict_GetItemString((*owner_type).tp_dict, c"__init__".as_ptr());
                assert!(
                    !init_function_obj.is_null(),
                    "class dict should contain __init__"
                );
                let init_function_ptr = init_function_obj as i64;
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
                let module_obj = ffi::PyModule_New(c"counter_test".as_ptr());
                assert!(!module_obj.is_null(), "test module should allocate");
                let module_dict = ffi::PyModule_GetDict(module_obj);
                assert!(
                    ffi::PyDict_SetItemString(module_dict, c"Record".as_ptr(), cls.as_ptr()) == 0,
                    "test module should accept Record binding"
                );
                crate::register_function_owner_types_for_module(module_obj)
                    .expect("owner types should register from explicit test module");
                ffi::Py_DECREF(module_obj);

                let mut jit_module = new_jit_module(runtime.compile_session.as_ref())
                    .expect("test jit module should construct");
                let (_init_sig, declared_init) =
                    declare_direct_function(&mut jit_module, &init_function, None)
                        .expect("test __init__ direct function should declare");
                let predeclared = HashMap::from([(init_function.function_id, declared_init)]);
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
                    &[1usize as ObjPtr],
                    &shared_state.lowered_module,
                    &caller_function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    module_constant_object_data_ids.as_slice(),
                    counter_slots_by_id.as_ref(),
                    scalar_counter_data_id,
                    top_value_counter_data_id,
                    runtime.compile_session.as_ref(),
                    Some(shared_state.as_ref()),
                    None,
                    Some(&predeclared),
                    BuildSpecializedFunctionOptions::default(),
                )
                .expect("specialized JIT build should succeed");
                let alloc_helpers = import_user_names_for_symbols(
                    &built,
                    &[DP_JIT_PYTYPE_GENERIC_ALLOC_IMPORT.symbol],
                );
                assert_eq!(
                    count_direct_calls_to_runtime_helpers(&built.ctx.func, &alloc_helpers),
                    1,
                    "constructor specialization should allocate via the constructor fast path",
                );
                let finish_helpers = import_user_names_for_symbols(
                    &built,
                    &[DP_JIT_FINISH_CONSTRUCTOR_INIT_IMPORT.symbol],
                );
                assert_eq!(
                    count_direct_calls_to_runtime_helpers(&built.ctx.func, &finish_helpers),
                    1,
                    "constructor specialization should validate __init__ results in the fast path",
                );
                assert!(
                    !function_contains_iconst_imm(&built.ctx.func, owner_type as i64),
                    "constructor specialization should not bake the owner type pointer into the function body",
                );
                assert!(
                    !function_contains_iconst_imm(&built.ctx.func, init_function_ptr),
                    "constructor specialization should not bake the __init__ function pointer into the function body",
                );
            })
        };

        match old_soac_work_dir {
            Some(value) => unsafe { std::env::set_var("SOAC_WORK_DIR", value) },
            None => unsafe { std::env::remove_var("SOAC_WORK_DIR") },
        }
        match old_soac_opt_mode {
            Some(value) => unsafe { std::env::set_var("SOAC_OPT_MODE", value) },
            None => unsafe { std::env::remove_var("SOAC_OPT_MODE") },
        }
    }
}
