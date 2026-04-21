use super::{Block, BlockPyFormat, Instr, ModuleShape, ResolvedName};
use crate::passes::{
    CodegenModuleShape, CoreModuleShape, CoreModuleShapeWithAwaitAndYield,
    CoreModuleShapeWithYield, ResolvedStorageModuleShape,
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
    fn block_metadata_lines(block: &Block<Self::Instr>) -> Vec<String> {
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

impl BlockPyFormat for CodegenModuleShape {
    fn block_metadata_lines(block: &Block<Self::Instr>) -> Vec<String> {
        render_resolved_storage_block_metadata::<Self>(block)
    }
}

fn render_resolved_storage_block_metadata<P>(block: &Block<P::Instr>) -> Vec<String>
where
    P: ModuleShape,
    P::Instr: Instr<Name = ResolvedName>,
{
    let mut lines = Vec::new();
    if !block.params.is_empty() {
        lines.push(format!(
            "params: [{}]",
            block
                .param_names()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(exc_edge) = &block.exc_edge {
        lines.push(format!("exc_target: {}", exc_edge.target));
    }
    if let Some(exc_name) = block.exception_param() {
        lines.push(format!("exc_name: {exc_name}"));
    }
    lines
}
