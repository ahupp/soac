use ruff_python_ast::{self as ast, ModModule};
use ruff_text_size::TextRange;
use soac_core::block_py::{BlockPyModule, ModuleShape};
pub use soac_core::pass_tracker::{NoopPassTracker, PassTiming, PassTracker, RecordingPassTracker};

fn blockpy_module_block_count<S>(module: &BlockPyModule<S>) -> usize
where
    S: ModuleShape,
{
    module
        .callable_defs
        .iter()
        .map(|function| function.blocks.len())
        .sum()
}

pub trait LoweringPassTrackerExt {
    fn pass_ast_to_ast(&self) -> Option<ModModule>;
    fn pass_block_count(&self, name: &str) -> Option<usize>;
}

impl LoweringPassTrackerExt for RecordingPassTracker {
    fn pass_ast_to_ast(&self) -> Option<ModModule> {
        self.get::<crate::driver::AstToAstPassResult>("ast-to-ast")
            .map(|pass| ModModule {
                node_index: ast::AtomicNodeIndex::default(),
                range: TextRange::default(),
                body: pass.module.clone(),
            })
    }

    fn pass_block_count(&self, name: &str) -> Option<usize> {
        match name {
            "core_blockpy" => self.pass_core_blockpy().map(blockpy_module_block_count),
            "core_blockpy_with_await_and_yield" => self
                .pass_core_blockpy_with_await_and_yield()
                .map(blockpy_module_block_count),
            "name_binding" => self.pass_name_binding().map(blockpy_module_block_count),
            _ => None,
        }
    }
}

pub(crate) trait LoweringPassTrackerInternalExt {
    fn pass_core_blockpy(&self) -> Option<&BlockPyModule<crate::passes::CoreModuleShape>>;

    fn pass_core_blockpy_with_await_and_yield(
        &self,
    ) -> Option<&BlockPyModule<crate::passes::CoreModuleShapeWithAwaitAndYield>>;

    fn pass_name_binding(
        &self,
    ) -> Option<&BlockPyModule<crate::passes::ResolvedStorageModuleShape>>;
}

impl LoweringPassTrackerInternalExt for RecordingPassTracker {
    fn pass_core_blockpy(&self) -> Option<&BlockPyModule<crate::passes::CoreModuleShape>> {
        self.get::<BlockPyModule<crate::passes::CoreModuleShape>>("core_blockpy")
    }

    fn pass_core_blockpy_with_await_and_yield(
        &self,
    ) -> Option<&BlockPyModule<crate::passes::CoreModuleShapeWithAwaitAndYield>> {
        self.get::<BlockPyModule<crate::passes::CoreModuleShapeWithAwaitAndYield>>(
            "core_blockpy_with_await_and_yield",
        )
    }

    fn pass_name_binding(
        &self,
    ) -> Option<&BlockPyModule<crate::passes::ResolvedStorageModuleShape>> {
        self.get::<BlockPyModule<crate::passes::ResolvedStorageModuleShape>>("name_binding")
    }
}
