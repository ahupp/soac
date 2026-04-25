use crate::{InstrTyped, TypedCodegenModuleShape};
use soac_core::block_py::{
    BlockPyFunction, ChildVisitable, HasMeta, HasSemanticInstrId, InstrId, Visit, VisitMut,
    WithMeta,
};
use std::collections::HashSet;

struct MissingTypedBlockInstrIdAssigner<'a> {
    next_instr_index: u32,
    used: &'a mut HashSet<InstrId>,
}

impl MissingTypedBlockInstrIdAssigner<'_> {
    fn assign(&mut self, expr: &mut InstrTyped) {
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

impl VisitMut<InstrTyped> for MissingTypedBlockInstrIdAssigner<'_> {
    fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
        self.assign(expr);
        expr.visit_children_mut(self);
    }
}

pub fn assign_missing_typed_function_instr_ids(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
) {
    let mut next_instr_index = 0;
    let mut used = HashSet::new();
    {
        struct MaxIdCollector<'a> {
            next_instr_index: &'a mut u32,
            used: &'a mut HashSet<InstrId>,
        }

        impl Visit<InstrTyped> for MaxIdCollector<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let Some(instr_id) = expr.try_semantic_instr_id() {
                    self.used.insert(instr_id);
                    *self.next_instr_index = (*self.next_instr_index).max(
                        instr_id
                            .index()
                            .checked_add(1)
                            .expect("per-function instruction count should fit in u32"),
                    );
                }
                expr.visit_children(self);
            }
        }

        let mut collector = MaxIdCollector {
            next_instr_index: &mut next_instr_index,
            used: &mut used,
        };
        collector.visit_fn(function);
    }

    let mut assigner = MissingTypedBlockInstrIdAssigner {
        next_instr_index,
        used: &mut used,
    };
    for block in &mut function.blocks {
        assigner.visit_block_mut(block);
    }
}
