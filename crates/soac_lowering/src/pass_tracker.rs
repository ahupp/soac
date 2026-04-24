use ruff_python_ast::{self as ast, ModModule};
use ruff_text_size::TextRange;
use soac_core::block_py::BlockPyModule;
use soac_core::pass_tracker::RecordingPassTracker;

pub(crate) trait LoweringPassTrackerExt {
    fn pass_ast_to_ast(&self) -> Option<ModModule>;
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
