// The explicit operation families expand through DelegateMatchDefault's
// recursive enum matcher. Keep this bounded allowance above the full IR enum.
#![recursion_limit = "512"]
#![deny(unreachable_pub)]

mod block_py;
mod canonical_annotations;
mod canonical_class_bindings;
mod driver;
#[cfg(test)]
mod fixture;
mod namegen;
#[cfg(test)]
mod pass_tracker;
pub mod passes;
mod ruff_ast;
mod template;
#[cfg(test)]
mod test_util;
mod transformer;

pub use crate::driver::{
    lower_python_to_blockpy_for_testing, lower_python_to_blockpy_with_tracker_and_options,
    lower_source_to_blockpy_module_with_tracker, LoweringError, LoweringOptions, LoweringResult,
    Result,
};
pub use canonical_annotations::CanonicalAnnotationStrings;
pub use canonical_class_bindings::CanonicalClassBindings;
pub use soac_core::block_py::{
    ClassBindingAccess, ClassBindingAccessContext, ClassBindingAccessSelection,
    ClassBindingCapture, ClassBindingCodeNode, ClassBindingExport, ClassBindingExportKind,
    ClassBindingInitialValue, ClassBindingInitializer, ClassBindingPhase, ClassBindingProjection,
    ClassBindingRecipe, ClassBindingScope, ClassBindingSlotBinding, ClassBindingSlotId,
    ClassBindingSlotProjection, ClassBindingStorage, NativeCodeId, NativeCompileScopeKind,
    NativeLocalsPlusKind, NativeLocalsPlusSlot, NativeSymbolScopeKind,
    CLASS_BINDINGS_SCHEMA_VERSION,
};

#[cfg(test)]
mod test;
