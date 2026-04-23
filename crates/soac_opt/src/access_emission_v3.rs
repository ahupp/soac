use crate::artifacts_v3::ExactIntBranchV3Artifacts;
use crate::emit_v3::MechanicalIndexedFieldGuard;
use crate::plan_v3::{
    ExactListItemAccessKind, ExactListItemFallbackKind, ExactListItemGuardKind, ExactListItemShape,
    IndexedFieldAccessKind, IndexedFieldFallbackKind, IndexedGlobalAccessKind,
    IndexedGlobalFallbackKind, IndexedGlobalGuardKind,
};
use soac_core::block_py::InstrId;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactListItemAccessPlan {
    pub source: InstrId,
    pub access: ExactListItemAccessKind,
    pub shape: ExactListItemShape,
    pub guard: ExactListItemGuardKind,
    pub fallback: ExactListItemFallbackKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFieldAccessPlan {
    pub access: IndexedFieldAccessKind,
    pub guard: MechanicalIndexedFieldGuard,
    pub fallback: IndexedFieldFallbackKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedGlobalAccessPlan {
    pub source: InstrId,
    pub access: IndexedGlobalAccessKind,
    pub module_name: String,
    pub name: String,
    pub expected_index: u32,
    pub guard: IndexedGlobalGuardKind,
    pub fallback: IndexedGlobalFallbackKind,
}

pub fn indexed_fields_for_function_from_artifacts(
    artifacts: &ExactIntBranchV3Artifacts,
) -> Result<Option<HashMap<InstrId, Vec<IndexedFieldAccessPlan>>>, String> {
    let emitted_function = &artifacts.emission.functions[0];
    if emitted_function.indexed_fields.is_empty() {
        return Ok(None);
    }

    let mut by_source = HashMap::<InstrId, Vec<IndexedFieldAccessPlan>>::new();
    for indexed_field in &emitted_function.indexed_fields {
        let access = IndexedFieldAccessPlan {
            access: indexed_field.access,
            guard: indexed_field.guard.clone(),
            fallback: indexed_field.fallback.kind,
        };
        let entry = by_source.entry(indexed_field.source).or_default();
        if !entry.contains(&access) {
            entry.push(access);
        }
    }
    Ok(Some(by_source))
}

pub fn exact_list_items_for_function_from_artifacts(
    artifacts: &ExactIntBranchV3Artifacts,
) -> Result<Option<HashMap<InstrId, ExactListItemAccessPlan>>, String> {
    let emitted_function = &artifacts.emission.functions[0];
    if emitted_function.exact_list_items.is_empty() {
        return Ok(None);
    }

    let mut by_source = HashMap::<InstrId, ExactListItemAccessPlan>::new();
    for item in &emitted_function.exact_list_items {
        by_source.insert(
            item.source,
            ExactListItemAccessPlan {
                source: item.source,
                access: item.access,
                shape: item.shape,
                guard: item.guard.kind,
                fallback: item.fallback.kind,
            },
        );
    }
    Ok(Some(by_source))
}

pub fn indexed_globals_for_function_from_artifacts(
    artifacts: &ExactIntBranchV3Artifacts,
) -> Result<Option<HashMap<InstrId, IndexedGlobalAccessPlan>>, String> {
    let emitted_function = &artifacts.emission.functions[0];
    if emitted_function.indexed_globals.is_empty() {
        return Ok(None);
    }

    let mut by_source = HashMap::<InstrId, IndexedGlobalAccessPlan>::new();
    for indexed_global in &emitted_function.indexed_globals {
        by_source.insert(
            indexed_global.source,
            IndexedGlobalAccessPlan {
                source: indexed_global.source,
                access: indexed_global.access,
                module_name: indexed_global.module_name.clone(),
                name: indexed_global.name.clone(),
                expected_index: indexed_global.expected_index,
                guard: indexed_global.guard.kind,
                fallback: indexed_global.fallback.kind,
            },
        );
    }
    Ok(Some(by_source))
}
