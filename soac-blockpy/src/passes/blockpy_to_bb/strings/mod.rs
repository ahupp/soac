use crate::block_py::{
    BlockPyFunction, BlockPyModule, HasMeta, InstrCodegen, InstrLow, InstrResolved, LiteralValue,
    Load, MapFunction, MapInstr, Mappable, NameLocation, ResolvedName, WithMeta,
};
use crate::passes::{CodegenBlockPyPass, ResolvedStorageBlockPyPass};
use soac_macros::match_default;

pub fn normalize_bb_module_strings(
    module: &BlockPyModule<ResolvedStorageBlockPyPass>,
) -> BlockPyModule<CodegenBlockPyPass> {
    let mut normalizer = CodegenExprNormalizer::default();
    let module = module.clone();
    let mut module_constants = module.module_constants;
    let callable_defs = module
        .callable_defs
        .into_iter()
        .map(|function| normalizer.map_fn(function))
        .collect::<Vec<BlockPyFunction<CodegenBlockPyPass>>>();
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
struct CodegenExprNormalizer {
    module_constants: Vec<InstrResolved>,
}

impl CodegenExprNormalizer {
    fn push_module_constant(&mut self, literal: LiteralValue) -> u32 {
        let index = u32::try_from(self.module_constants.len())
            .expect("module constant count should fit in u32");
        self.module_constants.push(InstrResolved::Literal(literal));
        index
    }
}

impl MapInstr<InstrResolved, InstrCodegen> for CodegenExprNormalizer {
    fn map_instr(&mut self, expr: InstrResolved) -> InstrCodegen {
        match_default!(expr: crate::passes::InstrLow<ResolvedName> {
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
            InstrResolved::CellRefForName(node) => {
                panic!(
                    "cell_ref should lower to a resolved cell ref before codegen, got {:?}",
                    node.logical_name
                );
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
