use crate::block_py::{
    BlockPyFunction, BlockPyModule, ConstantExpr, HasMeta, InstrBlockPy, InstrResolved,
    LiteralValue, Load, MapFunction, MapInstr, Mappable, NameLocation, ResolvedName, WithMeta,
};
use crate::passes::ResolvedStorageModuleShape;
use soac_ir_blockpy::BlockPyModuleShape;
use soac_macros::match_default;

pub(crate) fn hoist_module_constants(
    module: &BlockPyModule<ResolvedStorageModuleShape>,
) -> BlockPyModule<BlockPyModuleShape> {
    let mut normalizer = BlockPyExprNormalizer::default();
    let module = module.clone();
    let mut module_constants = module
        .module_constants
        .into_iter()
        .map(resolved_module_constant_to_constant_expr)
        .collect::<Vec<_>>();
    let callable_defs = module
        .callable_defs
        .into_iter()
        .map(|function| normalizer.map_fn(function))
        .collect::<Vec<BlockPyFunction<BlockPyModuleShape>>>();
    module_constants.extend(normalizer.module_constants);
    BlockPyModule {
        module_name_gen: module.module_name_gen,
        global_names: module.global_names,
        callable_defs,
        module_constants,
        counter_defs: module.counter_defs,
    }
}

#[derive(Default)]
struct BlockPyExprNormalizer {
    module_constants: Vec<ConstantExpr>,
}

impl BlockPyExprNormalizer {
    fn push_module_constant(&mut self, literal: LiteralValue) -> u32 {
        let index = u32::try_from(self.module_constants.len())
            .expect("module constant count should fit in u32");
        self.module_constants.push(ConstantExpr::Literal(literal));
        index
    }
}

fn resolved_module_constant_to_constant_expr(expr: InstrResolved) -> ConstantExpr {
    match expr {
        InstrResolved::Literal(literal) => ConstantExpr::Literal(literal),
        InstrResolved::Load(load) if load.name.is_runtime_name() => ConstantExpr::RuntimeName(
            load.name
                .runtime_name_id()
                .expect("runtime-name load should carry a RuntimeName id"),
        ),
        other => panic!("unsupported resolved module constant after name binding: {other:?}"),
    }
}

impl MapInstr<InstrResolved, InstrBlockPy> for BlockPyExprNormalizer {
    fn map_instr(&mut self, expr: InstrResolved) -> InstrBlockPy {
        match_default!(expr: crate::passes::InstrResolved {
            InstrResolved::Literal(literal) => {
                let meta = literal.meta();
                let constant_index = self.push_module_constant(literal);
                Load::new(ResolvedName {
                    id: format!("__dp_constant_{constant_index}").into(),
                    location: NameLocation::Constant(constant_index),
                })
                .with_meta(meta)
                .into()
            },
            InstrResolved::CellRef(node) => node.into(),
            rest => rest.map_children(self).into(),
        })
    }

    fn map_name(&mut self, name: ResolvedName) -> ResolvedName {
        name
    }
}

#[cfg(test)]
mod test;
