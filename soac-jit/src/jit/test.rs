use super::*;
use soac_blockpy::block_py::{
    BinOp, BinOpKind, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm, Call,
    CallArgKeyword, CallArgPositional, CallDirect, CellLocation, ChildVisitable, ClosureInit,
    ClosureSlot, CodegenBlock, CounterSite, Del, DelItem, FunctionId, FunctionName, HasMeta,
    HasSemanticInstrId, InstrCodegen, InstrResolved, Literal, LiteralValue, Load, LocalLocation,
    Meta, ModuleNameGen, NameLocation, NumberLiteral, NumberLiteralValue, Param, ParamKind,
    ParamSpec, ResolvedName, StorageLayout, Store, StringLiteral, Visit, VisitMut, WithMeta,
};
use soac_blockpy::passes::{
    CodegenModuleShape, instrument_bb_module_with_block_entry_counters,
    instrument_bb_module_with_refcount_counters, validate_codegen_instr_ids,
};
mod tests {
    use super::*;
    use crate::counter_dump::{CounterDumpRecord, CounterDumpRow, write_counter_dump_records};
    use pyo3::types::PyAnyMethods;
    use pyo3::{Python, ffi};
    use ruff_python_ast as ast;
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    static CAPSULE_DESTROYED: AtomicBool = AtomicBool::new(false);
    static NEXT_TEST_WORK_DIR_ID: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn cranelift_compile_cache_name_is_stable_from_logical_cache_name() {
        assert_eq!(
            stable_compile_cache_hash(b""),
            0xcbf29ce484222325,
            "empty FNV-1a hash should stay stable"
        );
        assert_eq!(
            stable_cranelift_compile_cache_name("direct:pkg.mod.fn:2"),
            stable_cranelift_compile_cache_name("direct:pkg.mod.fn:2")
        );
        assert_ne!(
            stable_cranelift_compile_cache_name("direct:pkg.mod.fn:2"),
            stable_cranelift_compile_cache_name("direct:pkg.mod.fn:3")
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
                "{prefix}-{}",
                NEXT_TEST_WORK_DIR_ID.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&work_dir).expect("test work dir should exist");
        work_dir
    }

    fn write_test_counter_dump(path: &Path, record: &CounterDumpRecord) {
        write_counter_dump_records(path, std::iter::once(record))
            .expect("test counter dump should be writable");
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
    fn direct_call_compatibility_requires_predeclared_target() {
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
            Err(DirectCallIncompatibility::MissingPredeclared)
        );
    }

    #[test]
    fn local_env_legacy_adapter_tracks_owned_transient_entries() {
        let mut env = LocalEnv::default();
        let first = ir::Value::from_u32(1);
        let second = ir::Value::from_u32(2);

        env.with_legacy_parts_mut(|names, values| {
            names.push("x".to_string());
            values.push(first);
        });
        assert_eq!(env.entries.len(), 1);
        assert_eq!(env.entries[0].name, "x");
        assert_eq!(env.entries[0].value, first);
        assert_eq!(env.entries[0].ref_kind, LocalRefKind::Owned);

        env.with_legacy_parts_mut(|names, values| {
            values[0] = second;
            names.push("y".to_string());
            values.push(first);
        });
        assert_eq!(
            env.entries
                .iter()
                .map(|entry| (entry.name.as_str(), entry.value, entry.ref_kind))
                .collect::<Vec<_>>(),
            vec![
                ("x", second, LocalRefKind::Owned),
                ("y", first, LocalRefKind::Owned),
            ]
        );
        assert_eq!(
            env.legacy_ref_kinds(),
            vec![LocalRefKind::Owned, LocalRefKind::Owned]
        );
    }

    #[test]
    fn local_env_legacy_adapter_preserves_location_keys_by_name() {
        let first = ir::Value::from_u32(1);
        let second = ir::Value::from_u32(2);
        let mut env = LocalEnv {
            entries: vec![LocalEnvEntry {
                key: LocalEnvKey::Location(LocalLocation(7)),
                name: "x".to_string(),
                value: first,
                ref_kind: LocalRefKind::Immortal,
                storage: LocalEnvStorage::StackMirror,
            }],
        };

        env.with_legacy_parts_mut(|_names, values| {
            values[0] = second;
        });

        assert_eq!(env.entries.len(), 1);
        assert_eq!(env.entries[0].key, LocalEnvKey::Location(LocalLocation(7)));
        assert_eq!(env.entries[0].name, "x");
        assert_eq!(env.entries[0].value, second);
        assert_eq!(env.entries[0].ref_kind, LocalRefKind::Owned);
        assert_eq!(env.entries[0].storage, LocalEnvStorage::LocalOnly);
    }

    #[test]
    fn local_env_cleanup_values_exclude_stack_mirrors_and_immortals() {
        let owned_local = ir::Value::from_u32(1);
        let owned_mirror = ir::Value::from_u32(2);
        let immortal_local = ir::Value::from_u32(3);
        let env = LocalEnv {
            entries: vec![
                LocalEnvEntry {
                    key: LocalEnvKey::legacy_name("local"),
                    name: "local".to_string(),
                    value: owned_local,
                    ref_kind: LocalRefKind::Owned,
                    storage: LocalEnvStorage::LocalOnly,
                },
                LocalEnvEntry {
                    key: LocalEnvKey::Location(LocalLocation(0)),
                    name: "mirror".to_string(),
                    value: owned_mirror,
                    ref_kind: LocalRefKind::Owned,
                    storage: LocalEnvStorage::StackMirror,
                },
                LocalEnvEntry {
                    key: LocalEnvKey::legacy_name("immortal"),
                    name: "immortal".to_string(),
                    value: immortal_local,
                    ref_kind: LocalRefKind::Immortal,
                    storage: LocalEnvStorage::LocalOnly,
                },
            ],
        };

        assert_eq!(env.local_only_cleanup_values(), vec![owned_local]);
    }

    fn local_env_store_test_state() -> (LocalEnv, String) {
        let compile_session = crate::session::CompileSession::new();
        let mut jit_module =
            new_jit_module(&compile_session).expect("test jit module should construct");
        let ptr_ty = jit_module.target_config().pointer_type();

        let mut refcount_signature = jit_module.make_signature();
        refcount_signature.params.push(ir::AbiParam::new(ptr_ty));

        let mut wrapper_signature = jit_module.make_signature();
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));
        wrapper_signature.params.push(ir::AbiParam::new(ptr_ty));
        wrapper_signature.returns.push(ir::AbiParam::new(ptr_ty));

        let wrapper_id = declare_local_fn(
            &mut jit_module,
            "local_env_store_test",
            &wrapper_signature,
        )
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
            let incref_ref = jit_module.declare_func_in_func(incref_id, &mut fb.func);
            let decref_ref = jit_module.declare_func_in_func(decref_id, &mut fb.func);
            env.entries.push(LocalEnvEntry {
                key: LocalEnvKey::Location(LocalLocation(0)),
                name: "x".to_string(),
                value: old_value,
                ref_kind: LocalRefKind::Owned,
                storage: LocalEnvStorage::LocalOnly,
            });
            let stack_slots = StackSlots {
                names: Vec::new(),
                slots: Vec::new(),
            };

            env.store_location(
                &mut fb,
                LocalLocation(0),
                "x",
                new_value,
                &stack_slots,
                ptr_ty,
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
        let (env, rendered) = local_env_store_test_state();

        assert_eq!(env.entries.len(), 1, "{rendered}");
        assert_eq!(env.entries[0].key, LocalEnvKey::Location(LocalLocation(0)));
        assert_eq!(env.entries[0].name, "x");
        assert_eq!(env.entries[0].ref_kind, LocalRefKind::Owned);
        assert_eq!(env.entries[0].storage, LocalEnvStorage::LocalOnly);
        assert!(
            rendered.contains("call"),
            "owned previous local should still be released after rebinding:\n{rendered}"
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

    #[test]
    fn local_env_recomputes_legacy_ref_kinds_from_current_values() {
        let old_value = ir::Value::from_u32(1);
        let new_value = ir::Value::from_u32(2);
        let env = LocalEnv {
            entries: vec![LocalEnvEntry {
                key: LocalEnvKey::Location(LocalLocation(0)),
                name: "x".to_string(),
                value: old_value,
                ref_kind: LocalRefKind::Borrowed,
                storage: LocalEnvStorage::StackMirror,
            }],
        };

        assert_eq!(
            env.ref_kinds_for_legacy_parts(&["x".to_string()], &[old_value]),
            vec![LocalRefKind::Borrowed]
        );
        assert_eq!(
            env.ref_kinds_for_legacy_parts(&["x".to_string()], &[new_value]),
            vec![LocalRefKind::Owned]
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
            .declare_direct_function(&first)
            .expect("first function should declare");
        let first_decl_again = state
            .declare_direct_function(&first)
            .expect("same shape should reuse declaration");
        assert_eq!(first_decl.symbol, first_decl_again.symbol);

        let session = std::sync::Arc::new(crate::session::CompileSession::new());
        let first_handle = state
            .mark_direct_function_ready(
                &session,
                first.function_id,
                1usize as *const u8,
                first.params.len(),
            )
            .expect("first function should mark ready");
        let ready_handle = state
            .ready_direct_function(&first)
            .expect("first function should be ready");
        assert!(std::sync::Arc::ptr_eq(&first_handle, &ready_handle));
        assert!(state.ready_direct_function(&second).is_none());

        let second_decl = state
            .declare_direct_function(&second)
            .expect("colliding function id with different shape should redeclare");
        assert_ne!(first_decl.symbol, second_decl.symbol);
    }

    #[test]
    fn process_jit_batch_collection_resolves_cross_module_targets_from_compile_session() {
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
            assert_eq!(function_ids, vec![caller.function_id, callee.function_id]);
            assert_eq!(
                batch[1]
                    .source
                    .shared_state()
                    .expect("cross-module callee should carry its owning shared state")
                    .module_name,
                "callee_test"
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
            let counter_ptrs = shared_state.counter_ptrs();
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
                    &counter_ptrs,
                    Some(shared_state.as_ref()),
                    None,
                )
            }
            .expect("mutually-recursive process JIT batch should compile");
            assert!(compiled.compiled);
            let state = engine
                .state
                .lock()
                .expect("process JIT state lock should not be poisoned");
            assert!(
                state.ready_direct_function(&first).is_some(),
                "root function should be marked ready"
            );
            assert!(
                state.ready_direct_function(&second).is_some(),
                "mutually-recursive callee should be marked ready"
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

    fn render_test_jit_function_with_operator_specializations(
        function: &BlockPyFunction<CodegenModuleShape>,
        blocks: &[ObjPtr],
        module_constants: Vec<InstrResolved>,
        operator_specializations: &[(InstrId, u64)],
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

        let rendered = Python::attach(|py| {
            let shared_state =
                crate::module_type::build_shared_state_for_testing(py, module, module_name, "")
                    .expect("shared state should build");
            let compile_session = crate::session::CompileSession::new();
            let mut jit_module =
                new_jit_module(&compile_session).expect("test jit module should construct");
            let module_constant_ptrs = shared_state.module_constant_ptrs();
            let counter_ptrs = shared_state.counter_ptrs();
            let built = build_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks,
                &shared_state.lowered_module,
                &function,
                &shared_state.codegen_constants,
                &shared_state.lowered_module.counter_defs,
                &module_constant_ptrs,
                &counter_ptrs,
                &compile_session,
                Some(shared_state.as_ref()),
                None,
                None,
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

        rendered
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
                &compile_session,
                None,
                None,
                None,
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
            let counter_ptrs = placeholder_counter_ptrs(0);
            let mut built = build_cranelift_run_bb_specialized_function(
                &mut jit_module,
                blocks,
                module,
                function,
                module_constants,
                &[],
                &module_constant_ptrs,
                &counter_ptrs,
                &compile_session,
                None,
                None,
                None,
                None,
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
        let (compile_session, mut jit_module, mut ctx, wrapper_id, _) =
            build_runtime_refcount_smoke_context();

        define_function_with_incremental_cache(
            &compile_session,
            &mut jit_module,
            wrapper_id,
            &mut ctx,
            "test-runtime-refcount-smoke-wrapper",
            CraneliftCompileCachePolicy::Enabled,
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
            &compile_session,
            &mut jit_module,
            wrapper_id,
            &mut ctx,
            "test-runtime-refcount-decref-wrapper",
            CraneliftCompileCachePolicy::Enabled,
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
    fn jit_runtime_clif_refcount_roundtrip_preserves_py_long_refcount() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            let wrapper = build_runtime_refcount_smoke_wrapper();
            crate::initialize_test_python();
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
                let counter_ptrs = shared_state.counter_ptrs();
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
                    &counter_ptrs,
                    Some(shared_state.as_ref()),
                    Some(runtime.mod_ctx.globals_obj),
                )
                .expect("direct counter test function should compile");
                let (code_ptr, param_count) = compiled_handle
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
    fn jit_function_scope_refcount_counters_track_runtime_helpers() {
        let _guard = crate::python_runtime_test_lock().lock().unwrap();
        unsafe {
            crate::initialize_test_python();
            Python::attach(|py| {
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
                let compile_session = crate::session::CompileSession::process();
                let compiled_handle = compile_cranelift_run_bb_specialized_cached(
                    &compile_session,
                    &blocks,
                    &shared_state.lowered_module,
                    &function,
                    &shared_state.codegen_constants,
                    &shared_state.lowered_module.counter_defs,
                    &module_constant_ptrs,
                    &counter_ptrs,
                    Some(shared_state.as_ref()),
                    Some(runtime.mod_ctx.globals_obj),
                )
                .expect("direct refcount counter test function should compile");
                let (code_ptr, param_count) = compiled_handle
                    .handle
                    .direct_runner_info()
                    .expect("compiled direct runner should expose entrypoint");
                assert_eq!(param_count, 1, "test function should take one direct arg");
                let entry: unsafe extern "C" fn(
                    *mut c_void,
                    *mut c_void,
                    *mut c_void,
                ) -> *mut c_void = std::mem::transmute(code_ptr);
                let mut function_context =
                    test_function_jit_context(&runtime, std::ptr::null_mut());
                let thread_state = ffi::PyThreadState_Get().cast::<c_void>();

                let result1 = entry(
                    std::ptr::addr_of_mut!(function_context).cast(),
                    thread_state,
                    ffi::PyLong_FromLong(7).cast(),
                );
                let incref_after_first = shared_state.counter_value(incref_counter_id);
                let decref_after_first = shared_state.counter_value(decref_counter_id);
                let result2 = entry(
                    std::ptr::addr_of_mut!(function_context).cast(),
                    thread_state,
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
            let counter_ptrs = placeholder_counter_ptrs(0);
            let built = build_cranelift_run_bb_specialized_function(
                &mut jit_module,
                &blocks,
                &module,
                &function,
                &module_constants,
                &[],
                &module_constant_ptrs,
                &counter_ptrs,
                &compile_session,
                None,
                None,
                None,
                None,
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
            rendered.contains("call dp_jit_exact_long_add_slot"),
            "exact-int binop specialization should call the profiled PyLong number slot:\n{rendered}"
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
            rendered.contains("call dp_jit_exact_long_richcompare_slot"),
            "exact-int compare specialization should call the profiled PyLong richcompare slot:\n{rendered}"
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
        let mut module = test_module(ModuleNameGen::new(0), vec![function.clone()]);
        module.module_constants = vec![int_literal(7)];
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
    fn specialized_jit_type_constructors_use_constructor_fast_path() {
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
                    runtime.compile_session.as_ref(),
                    Some(shared_state.as_ref()),
                    Some(runtime.mod_ctx.globals_obj),
                    None,
                    Some(&predeclared),
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
