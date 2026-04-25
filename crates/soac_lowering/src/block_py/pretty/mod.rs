use super::{Block, BlockPyFormat};
use crate::passes::{
    CoreModuleShape, CoreModuleShapeWithAwaitAndYield, CoreModuleShapeWithYield,
    ResolvedStorageModuleShape,
};

macro_rules! impl_default_blockpy_format {
    ($($pass:ty),* $(,)?) => {
        $(
            impl BlockPyFormat for $pass {}
        )*
    };
}

impl_default_blockpy_format!(
    CoreModuleShapeWithAwaitAndYield,
    CoreModuleShapeWithYield,
    CoreModuleShape,
);

impl BlockPyFormat for ResolvedStorageModuleShape {
    fn block_metadata_lines(block: &Block<Self::Instr, Self::BlockExtra>) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(exc_edge) = &block.exc_edge {
            lines.push(format!("exc_target: {}", exc_edge.target));
        }
        if let Some(exc_name) = block.exception_param() {
            lines.push(format!("exc_name: {exc_name}"));
        }
        lines
    }
}
