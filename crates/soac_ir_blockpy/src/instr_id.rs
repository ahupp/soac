use crate::{CodegenModuleShape, InstrCodegen};
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, ChildVisitable, HasMeta, HasSemanticInstrId, Instr, InstrId,
    InstrKey, Visit, VisitMut, WithMeta, walk_expr, walk_expr_mut,
};
use std::collections::HashSet;

struct BlockInstrIdAssigner {
    next_instr_index: u32,
}

impl BlockInstrIdAssigner {
    fn assign<I>(&mut self, expr: &mut I)
    where
        I: Instr + ChildVisitable<I> + HasMeta + WithMeta + Clone,
    {
        let mut meta = expr.meta();
        meta.instr_id = Some(InstrId::new(self.next_instr_index));
        self.next_instr_index = self
            .next_instr_index
            .checked_add(1)
            .expect("per-function instruction count should fit in u32");
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

pub fn reassign_codegen_function_instr_ids(function: &mut BlockPyFunction<CodegenModuleShape>) {
    let mut assigner = BlockInstrIdAssigner {
        next_instr_index: 0,
    };
    assigner.visit_fn_mut(function);
}

struct MissingBlockInstrIdAssigner<'a> {
    next_instr_index: u32,
    used: &'a mut HashSet<InstrId>,
}

impl MissingBlockInstrIdAssigner<'_> {
    fn assign<I>(&mut self, expr: &mut I)
    where
        I: Instr + ChildVisitable<I> + HasMeta + WithMeta + Clone,
    {
        if expr.try_semantic_instr_id().is_some() {
            return;
        }
        while self.used.contains(&InstrId::new(self.next_instr_index)) {
            self.next_instr_index = self
                .next_instr_index
                .checked_add(1)
                .expect("per-function instruction count should fit in u32");
        }
        let mut meta = expr.meta();
        let instr_id = InstrId::new(self.next_instr_index);
        meta.instr_id = Some(instr_id);
        self.used.insert(instr_id);
        self.next_instr_index = self
            .next_instr_index
            .checked_add(1)
            .expect("per-function instruction count should fit in u32");
        *expr = expr.clone().with_meta(meta);
    }
}

impl VisitMut<InstrCodegen> for MissingBlockInstrIdAssigner<'_> {
    fn visit_instr_mut(&mut self, expr: &mut InstrCodegen)
    where
        InstrCodegen: ChildVisitable<InstrCodegen>,
    {
        self.assign(expr);
        walk_expr_mut(self, expr);
    }
}

pub fn assign_missing_codegen_function_instr_ids(
    function: &mut BlockPyFunction<CodegenModuleShape>,
) {
    let mut next_instr_index = 0;
    let mut used = HashSet::new();
    {
        struct MaxIdCollector<'a> {
            next_instr_index: &'a mut u32,
            used: &'a mut HashSet<InstrId>,
        }

        impl Visit<InstrCodegen> for MaxIdCollector<'_> {
            fn visit_instr(&mut self, expr: &InstrCodegen)
            where
                InstrCodegen: ChildVisitable<InstrCodegen>,
            {
                if let Some(instr_id) = expr.try_semantic_instr_id() {
                    self.used.insert(instr_id);
                    *self.next_instr_index = (*self.next_instr_index).max(
                        instr_id
                            .index()
                            .checked_add(1)
                            .expect("per-function instruction count should fit in u32"),
                    );
                }
                walk_expr(self, expr);
            }
        }

        let mut collector = MaxIdCollector {
            next_instr_index: &mut next_instr_index,
            used: &mut used,
        };
        collector.visit_fn(function);
    }

    let mut assigner = MissingBlockInstrIdAssigner {
        next_instr_index,
        used: &mut used,
    };
    for block in &mut function.blocks {
        assigner.visit_block_mut(block);
    }
}

pub fn reassign_codegen_module_instr_ids(module: &mut BlockPyModule<CodegenModuleShape>) {
    for function in &mut module.callable_defs {
        reassign_codegen_function_instr_ids(function);
    }
}

pub fn assign_codegen_module_instr_ids(
    mut module: BlockPyModule<CodegenModuleShape>,
) -> BlockPyModule<CodegenModuleShape> {
    for function in &mut module.callable_defs {
        reassign_codegen_function_instr_ids(function);
    }
    module
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
                walk_expr(self, expr);
                return;
            }
            self.errors.push(format!(
                "missing codegen instruction id in function {} ({})",
                self.function.names.qualname, self.function.function_id
            ));
            walk_expr(self, expr);
            return;
        };

        let key = InstrKey::new(self.function.function_id, instr_id);
        if !self.seen.insert(key) {
            self.errors.push(format!(
                "duplicate codegen instruction id {} in function {}",
                key, self.function.names.qualname
            ));
        }

        walk_expr(self, expr);
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
