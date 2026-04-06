use crate::block_py::{
    walk_expr_mut, BlockLabel, BlockPyFunction, BlockPyModule, ChildVisitable, CodegenBlockPyExpr,
    HasMeta, InstrId, VisitMut, WithMeta,
};
use crate::passes::CodegenBlockPyPass;

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

impl VisitMut<CodegenBlockPyExpr> for BlockInstrIdAssigner {
    fn visit_instr_mut(&mut self, expr: &mut CodegenBlockPyExpr)
    where
        CodegenBlockPyExpr: ChildVisitable<CodegenBlockPyExpr>,
    {
        self.assign(expr);
        walk_expr_mut(self, expr);
    }
}

pub fn assign_function_instr_ids(function: &mut BlockPyFunction<CodegenBlockPyPass>) {
    for block in &mut function.blocks {
        let mut assigner = BlockInstrIdAssigner {
            block_label: block.label,
            next_instr_index_in_block: 0,
        };
        assigner.visit_block_mut(block);
    }
}

pub fn assign_module_instr_ids(module: &mut BlockPyModule<CodegenBlockPyPass>) {
    for function in &mut module.callable_defs {
        assign_function_instr_ids(function);
    }
}

#[cfg(test)]
mod test {
    use super::assign_module_instr_ids;
    use crate::block_py::{
        walk_block, ChildVisitable, CodegenBlockPyExpr, HasMeta, InstrId, Visit,
    };
    use crate::lower_python_to_blockpy_for_testing;
    use std::collections::HashMap;

    struct InstrIdCollector {
        ids_by_block: HashMap<crate::block_py::BlockLabel, Vec<InstrId>>,
    }

    impl Visit<CodegenBlockPyExpr> for InstrIdCollector {
        fn visit_instr(&mut self, expr: &CodegenBlockPyExpr)
        where
            CodegenBlockPyExpr: ChildVisitable<CodegenBlockPyExpr>,
        {
            let instr_id = expr.meta().instr_id.expect("instr ids should be assigned");
            self.ids_by_block
                .entry(instr_id.block_label())
                .or_default()
                .push(instr_id);
            crate::block_py::walk_expr(self, expr);
        }
    }

    #[test]
    fn assigns_sequential_instr_ids_per_block() {
        let mut lowered = lower_python_to_blockpy_for_testing(
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

        assign_module_instr_ids(&mut lowered);

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
}
