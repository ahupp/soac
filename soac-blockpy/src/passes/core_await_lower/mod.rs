use crate::block_py::{
    core_runtime_positional_call_expr_with_meta, BlockPyModule, HasMeta, InstrWithAwaitAndYield,
    InstrWithYield, MapInstr, MapModule, Mappable, UnresolvedName, WithMeta, YieldFrom,
};
use crate::passes::{CoreBlockPyPassWithAwaitAndYield, CoreBlockPyPassWithYield};
use soac_macros::match_default;

struct CoreAwaitLoweringMap;

impl MapInstr<InstrWithAwaitAndYield, InstrWithYield> for CoreAwaitLoweringMap {
    fn map_instr(&mut self, expr: InstrWithAwaitAndYield) -> InstrWithYield {
        match_default!(expr: crate::passes::InstrWithAwaitAndYield {
            InstrWithAwaitAndYield::Await(node) => {
                let meta = node.meta();
                InstrWithYield::YieldFrom(
                    YieldFrom::new(core_runtime_positional_call_expr_with_meta(
                        "await_iter",
                        meta.node_index.clone(),
                        meta.range,
                        vec![self.map_instr(*node.value)],
                    ))
                    .with_meta(meta),
                )
            },
            rest => rest.map_children(self).into(),
        })
    }

    fn map_name(&mut self, name: UnresolvedName) -> UnresolvedName {
        name
    }
}

pub(crate) fn lower_awaits_in_core_blockpy_module(
    module: BlockPyModule<CoreBlockPyPassWithAwaitAndYield>,
) -> BlockPyModule<CoreBlockPyPassWithYield> {
    let mut mapper = CoreAwaitLoweringMap;
    mapper.map_module(module)
}

#[cfg(test)]
mod test;
