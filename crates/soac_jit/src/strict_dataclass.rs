//! Invocation-scoped standard-dataclass adapter.
//!
//! Signed transforms select proposals, native-build recipes attest helper
//! bodies, and actual function/environment/entry witnesses select individual
//! roles. None makes a shared helper immutable or grants source/JIT ownership.
//! Fresh generated functions additionally require native creation records.

mod adoption;
mod catalog;
mod code;
mod edges;
mod fields;
mod generation;
mod invocation;
mod method_values;
mod native;
mod nominal;
mod operands;
mod produced;
mod producer_protocol;
mod protocol;
mod slots;
mod transcript;

pub(crate) use adoption::{
    DataclassAdoptedMembers, DataclassClassState, DataclassConstruction, DataclassNamespace,
    DataclassSlotsConstruction,
};
pub(crate) use adoption::{complete_application, complete_native_application};
pub(crate) use invocation::{
    apply, begin_apply, decline, discard, fail_native_call, finish_apply, finish_factory,
    native_completion_matches, native_invocation_for, native_source_matches, prepare,
    prepare_construction, prepare_native,
};
pub(crate) use native::RawFrameView as RawDataclassFrameView;
pub(crate) use slots::failed_replacement;

use pyo3::prelude::*;

use code::{CodeRecipe, RecipeKind};

/// Rust-only implementation evidence. Decoding does not execute a module and
/// retaining these snapshots retains no Python object.
struct StdlibRecipes {
    dataclasses: CodeRecipe,
    reprlib: CodeRecipe,
}

impl StdlibRecipes {
    fn load(py: Python<'_>) -> PyResult<Self> {
        Ok(Self {
            dataclasses: CodeRecipe::load(py, RecipeKind::Dataclasses)?,
            reprlib: CodeRecipe::load(py, RecipeKind::Reprlib)?,
        })
    }
}
