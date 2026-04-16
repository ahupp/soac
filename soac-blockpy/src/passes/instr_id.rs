use crate::block_py::{
    walk_expr_mut, BlockLabel, BlockPyFunction, BlockPyModule, ChildVisitable, HasMeta,
    HasSemanticInstrId, InstrCodegen, InstrId, InstrKey, Visit, VisitMut, WithMeta,
};
use crate::passes::{CodegenModuleShape, CodegenUnidentifiedModuleShape};
use std::collections::HashSet;

struct BlockInstrIdAssigner {
    block_label: BlockLabel,
    next_instr_index_in_block: u32,
}

impl BlockInstrIdAssigner {
    fn assign<I>(&mut self, expr: &mut I)
    where
        I: crate::block_py::Instr + ChildVisitable<I> + HasMeta + WithMeta + Clone,
    {
        let mut meta = expr.meta();
        meta.instr_id = Some(InstrId::new(
            self.block_label,
            self.next_instr_index_in_block,
        ));
        self.next_instr_index_in_block = self
            .next_instr_index_in_block
            .checked_add(1)
            .expect("per-block instruction count should fit in u32");
        *expr = expr.clone().with_meta(meta);
    }
}

impl VisitMut<InstrCodegen> for BlockInstrIdAssigner {
    fn visit_instr_mut(&mut self, expr: &mut InstrCodegen)
    where
        InstrCodegen: ChildVisitable<InstrCodegen>,
    {
        self.assign(expr);
        walk_expr_mut(self, expr);
    }
}

pub fn assign_function_instr_ids(function: &mut BlockPyFunction<CodegenUnidentifiedModuleShape>) {
    for block in &mut function.blocks {
        let mut assigner = BlockInstrIdAssigner {
            block_label: block.label,
            next_instr_index_in_block: 0,
        };
        assigner.visit_block_mut(block);
    }
}

pub fn reassign_codegen_function_instr_ids(function: &mut BlockPyFunction<CodegenModuleShape>) {
    for block in &mut function.blocks {
        let mut assigner = BlockInstrIdAssigner {
            block_label: block.label,
            next_instr_index_in_block: 0,
        };
        assigner.visit_block_mut(block);
    }
}

pub fn reassign_codegen_module_instr_ids(module: &mut BlockPyModule<CodegenModuleShape>) {
    for function in &mut module.callable_defs {
        reassign_codegen_function_instr_ids(function);
    }
}

fn into_identified_function(
    mut function: BlockPyFunction<CodegenUnidentifiedModuleShape>,
) -> BlockPyFunction<CodegenModuleShape> {
    assign_function_instr_ids(&mut function);
    BlockPyFunction {
        function_id: function.function_id,
        name_gen: function.name_gen,
        names: function.names,
        kind: function.kind,
        execution_mode: function.execution_mode,
        params: function.params,
        blocks: function.blocks,
        doc: function.doc,
        storage_layout: function.storage_layout,
        scope: function.scope,
    }
}

pub fn assign_module_instr_ids(
    module: BlockPyModule<CodegenUnidentifiedModuleShape>,
) -> BlockPyModule<CodegenModuleShape> {
    BlockPyModule {
        module_name_gen: module.module_name_gen,
        global_names: module.global_names,
        callable_defs: module
            .callable_defs
            .into_iter()
            .map(into_identified_function)
            .collect(),
        module_constants: module.module_constants,
        counter_defs: module.counter_defs,
    }
}

struct CodegenInstrIdValidator<'a> {
    function: &'a BlockPyFunction<CodegenModuleShape>,
    seen: HashSet<InstrKey>,
    errors: Vec<String>,
}

impl<'a> CodegenInstrIdValidator<'a> {
    fn validate_function(
        function: &'a BlockPyFunction<CodegenModuleShape>,
    ) -> Result<(), Vec<String>> {
        let mut validator = Self {
            function,
            seen: HashSet::new(),
            errors: Vec::new(),
        };
        validator.visit_fn(function);
        if validator.errors.is_empty() {
            Ok(())
        } else {
            Err(validator.errors)
        }
    }
}

impl Visit<InstrCodegen> for CodegenInstrIdValidator<'_> {
    fn visit_instr(&mut self, expr: &InstrCodegen)
    where
        InstrCodegen: ChildVisitable<InstrCodegen>,
    {
        let Some(instr_id) = expr.try_semantic_instr_id() else {
            if matches!(expr, InstrCodegen::IncrementCounter(_)) {
                crate::block_py::walk_expr(self, expr);
                return;
            }
            self.errors.push(format!(
                "missing codegen instruction id in function {} ({})",
                self.function.names.qualname, self.function.function_id
            ));
            crate::block_py::walk_expr(self, expr);
            return;
        };

        let key = InstrKey::new(self.function.function_id, instr_id);
        if !self.seen.insert(key) {
            self.errors.push(format!(
                "duplicate codegen instruction id {} in function {}",
                key, self.function.names.qualname
            ));
        }

        crate::block_py::walk_expr(self, expr);
    }
}

pub fn validate_codegen_instr_ids(
    module: &BlockPyModule<CodegenModuleShape>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    for function in &module.callable_defs {
        if let Err(function_errors) = CodegenInstrIdValidator::validate_function(function) {
            errors.extend(function_errors);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

#[cfg(test)]
mod test {
    use super::validate_codegen_instr_ids;
    use crate::block_py::{
        walk_block, ChildVisitable, HasMeta, HasSemanticInstrId, InstrCodegen, InstrId, Visit,
        VisitMut, WithMeta,
    };
    use crate::lower_python_to_blockpy_for_testing;
    use crate::passes::{instrument_bb_module_with_block_entry_counters, CodegenModuleShape};
    use std::collections::HashMap;

    struct InstrIdCollector {
        ids_by_block: HashMap<crate::block_py::BlockLabel, Vec<InstrId>>,
    }

    impl Visit<InstrCodegen> for InstrIdCollector {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            let instr_id = expr.semantic_instr_id();
            self.ids_by_block
                .entry(instr_id.block_label())
                .or_default()
                .push(instr_id);
            crate::block_py::walk_expr(self, expr);
        }
    }

    struct NthInstrIdReader {
        target: usize,
        seen: usize,
        result: Option<InstrId>,
    }

    impl Visit<InstrCodegen> for NthInstrIdReader {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if self.seen == self.target {
                self.result = expr.try_semantic_instr_id();
            }
            self.seen += 1;
            crate::block_py::walk_expr(self, expr);
        }
    }

    struct NthInstrIdSetter {
        target: usize,
        seen: usize,
        instr_id: Option<InstrId>,
        changed: bool,
    }

    impl crate::block_py::VisitMut<InstrCodegen> for NthInstrIdSetter {
        fn visit_instr_mut(&mut self, expr: &mut InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if self.seen == self.target {
                let mut meta = expr.meta();
                meta.instr_id = self.instr_id;
                *expr = expr.clone().with_meta(meta);
                self.changed = true;
            }
            self.seen += 1;
            crate::block_py::walk_expr_mut(self, expr);
        }
    }

    fn nth_instr_id(
        function: &crate::block_py::BlockPyFunction<CodegenModuleShape>,
        target: usize,
    ) -> InstrId {
        let mut reader = NthInstrIdReader {
            target,
            seen: 0,
            result: None,
        };
        reader.visit_fn(function);
        reader.result.expect("expected nth instruction to have id")
    }

    fn set_nth_instr_id(
        function: &mut crate::block_py::BlockPyFunction<CodegenModuleShape>,
        target: usize,
        instr_id: Option<InstrId>,
    ) {
        let mut setter = NthInstrIdSetter {
            target,
            seen: 0,
            instr_id,
            changed: false,
        };
        setter.visit_fn_mut(function);
        assert!(setter.changed, "expected nth instruction to exist");
    }

    #[test]
    fn assigns_sequential_instr_ids_per_block() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    if x:
        return h(g(x + 1))
    y = g(x + 1)
    return h(y)

def g(v):
    return v
"#,
        )
        .expect("transform should succeed")
        .codegen_module;

        let f = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f");
        let mut collector = InstrIdCollector {
            ids_by_block: HashMap::new(),
        };
        for block in &f.blocks {
            walk_block(&mut collector, block);
        }

        for (block_label, ids) in &collector.ids_by_block {
            let expected = (0..u32::try_from(ids.len()).unwrap())
                .map(|instr_index_in_block| InstrId::new(*block_label, instr_index_in_block))
                .collect::<Vec<_>>();
            assert_eq!(*ids, expected);
        }
    }

    #[test]
    fn validates_codegen_instr_ids_after_assignment() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    return x + 1
"#,
        )
        .expect("transform should succeed")
        .codegen_module;

        validate_codegen_instr_ids(&lowered).expect("assigned ids should validate");
    }

    #[test]
    fn rejects_missing_codegen_instr_id() {
        let mut lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    return x + 1
"#,
        )
        .expect("transform should succeed")
        .codegen_module;

        let f = lowered
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f");
        set_nth_instr_id(f, 0, None);

        let err = validate_codegen_instr_ids(&lowered)
            .expect_err("missing semantic codegen ids should fail validation");
        assert!(err.contains("missing codegen instruction id"));
    }

    #[test]
    fn allows_unidentified_synthetic_counter_instrs() {
        let mut lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    return x + 1
"#,
        )
        .expect("transform should succeed")
        .codegen_module;

        instrument_bb_module_with_block_entry_counters(&mut lowered);

        validate_codegen_instr_ids(&lowered)
            .expect("synthetic counter instrumentation should not require semantic ids");
    }

    #[test]
    fn rejects_duplicate_codegen_instr_id_in_same_function() {
        let mut lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    y = x + 1
    return y
"#,
        )
        .expect("transform should succeed")
        .codegen_module;

        let f = lowered
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f");
        let duplicate_id = nth_instr_id(f, 0);
        set_nth_instr_id(f, 1, Some(duplicate_id));

        let err = validate_codegen_instr_ids(&lowered)
            .expect_err("duplicate semantic codegen ids should fail validation");
        assert!(err.contains("duplicate codegen instruction id"));
    }
}
