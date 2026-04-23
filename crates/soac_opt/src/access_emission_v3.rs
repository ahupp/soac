use crate::artifacts_v3::ExactIntBranchV3Artifacts;
use crate::emit_v3::MechanicalIndexedFieldGuard;
use crate::plan_v3::{
    ExactListItemAccessKind, ExactListItemFallbackKind, ExactListItemGuardKind, ExactListItemShape,
    IndexedFieldAccessKind, IndexedFieldFallbackKind, IndexedFieldGuardKind,
    IndexedGlobalAccessKind, IndexedGlobalFallbackKind, IndexedGlobalGuardKind,
};
use soac_core::block_py::InstrId;
use soac_core::profile::{CollectedTypeKeyLayout, CounterDumpTypeKey};
use std::collections::{HashMap, HashSet};

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
pub struct IndexedFieldLayoutGroup {
    pub type_key: CounterDumpTypeKey,
    pub layouts: Vec<CollectedTypeKeyLayout>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFieldRuntimeAccessRequest {
    pub access: IndexedFieldAccessKind,
    pub attr_name: String,
    pub guard: IndexedFieldGuardKind,
    pub fallback: IndexedFieldFallbackKind,
    pub type_key: CounterDumpTypeKey,
    pub expected_index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedIndexedFieldAccess<T> {
    pub access: IndexedFieldAccessKind,
    pub attr_name: String,
    pub guard: IndexedFieldGuardKind,
    pub fallback: IndexedFieldFallbackKind,
    pub specialization: T,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedIndexedFieldAccessPlan<T> {
    pub access: IndexedFieldAccessKind,
    pub specializations: Vec<T>,
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

pub fn indexed_field_type_key(plan: &IndexedFieldAccessPlan) -> CounterDumpTypeKey {
    CounterDumpTypeKey {
        module_name: plan.guard.owner_type.module_name.clone(),
        qualname: plan.guard.owner_type.qualname.clone(),
    }
}

pub fn indexed_field_runtime_access_request(
    plan: &IndexedFieldAccessPlan,
) -> IndexedFieldRuntimeAccessRequest {
    IndexedFieldRuntimeAccessRequest {
        access: plan.access,
        attr_name: plan.guard.attr_name.clone(),
        guard: plan.guard.kind,
        fallback: plan.fallback,
        type_key: indexed_field_type_key(plan),
        expected_index: plan.guard.expected_index,
    }
}

pub fn indexed_field_layout_groups<'a>(
    plans: impl IntoIterator<Item = &'a IndexedFieldAccessPlan>,
) -> Vec<IndexedFieldLayoutGroup> {
    let mut layouts_by_type = HashMap::<CounterDumpTypeKey, Vec<CollectedTypeKeyLayout>>::new();
    let mut seen_layouts = HashSet::<(CounterDumpTypeKey, String, u32)>::new();
    for plan in plans {
        let request = indexed_field_runtime_access_request(plan);
        if !seen_layouts.insert((
            request.type_key.clone(),
            request.attr_name.clone(),
            request.expected_index,
        )) {
            continue;
        }
        layouts_by_type
            .entry(request.type_key)
            .or_default()
            .push(CollectedTypeKeyLayout {
                owner_type_id: 0,
                key: request.attr_name,
                index: request.expected_index,
            });
    }

    let mut groups = layouts_by_type
        .into_iter()
        .map(|(type_key, mut layouts)| {
            layouts.sort_by(|lhs, rhs| {
                lhs.index
                    .cmp(&rhs.index)
                    .then_with(|| lhs.key.cmp(&rhs.key))
            });
            IndexedFieldLayoutGroup { type_key, layouts }
        })
        .collect::<Vec<_>>();
    groups.sort_by(|lhs, rhs| {
        lhs.type_key
            .module_name
            .cmp(&rhs.type_key.module_name)
            .then_with(|| lhs.type_key.qualname.cmp(&rhs.type_key.qualname))
    });
    groups
}

pub fn prepare_indexed_field_accesses_for_codegen<T: PartialEq>(
    planned_by_instr: Option<&HashMap<InstrId, Vec<IndexedFieldAccessPlan>>>,
    mut resolve: impl FnMut(&IndexedFieldRuntimeAccessRequest) -> Result<Option<T>, String>,
) -> Result<HashMap<InstrId, Vec<ResolvedIndexedFieldAccess<T>>>, String> {
    let Some(planned_by_instr) = planned_by_instr else {
        return Ok(HashMap::new());
    };
    let mut by_instr = HashMap::new();
    for (instr_id, planned_accesses) in planned_by_instr {
        let mut resolved_accesses = Vec::new();
        let mut seen_requests = Vec::<IndexedFieldRuntimeAccessRequest>::new();
        for planned in planned_accesses {
            let request = indexed_field_runtime_access_request(planned);
            if seen_requests.contains(&request) {
                continue;
            }
            seen_requests.push(request.clone());
            let Some(specialization) = resolve(&request).map_err(|err| {
                format!(
                    "optimizer v3 indexed-field plan {:?} for {} attr {:?} could not bind a runtime field guard: {err}",
                    request.access, instr_id, request.attr_name
                )
            })?
            else {
                continue;
            };
            let resolved = ResolvedIndexedFieldAccess {
                access: request.access,
                attr_name: request.attr_name,
                guard: request.guard,
                fallback: request.fallback,
                specialization,
            };
            if !resolved_accesses.contains(&resolved) {
                resolved_accesses.push(resolved);
            }
        }
        if !resolved_accesses.is_empty() {
            by_instr.insert(*instr_id, resolved_accesses);
        }
    }
    Ok(by_instr)
}

pub fn prepared_indexed_field_access_plan<T: Clone + PartialEq>(
    instr_id: InstrId,
    expected_access: IndexedFieldAccessKind,
    resolved_by_instr: &HashMap<InstrId, Vec<ResolvedIndexedFieldAccess<T>>>,
) -> Result<PreparedIndexedFieldAccessPlan<T>, String> {
    let accesses = resolved_by_instr.get(&instr_id).ok_or_else(|| {
        format!(
            "optimizer v3 indexed-field {:?} for {instr_id} lost its prevalidated codegen guard payload",
            expected_access
        )
    })?;
    let mut specializations = Vec::with_capacity(accesses.len());
    for access in accesses {
        if access.access != expected_access {
            return Err(format!(
                "optimizer v3 indexed-field for {instr_id} was prevalidated as {:?}, but typed lowering requested {:?}",
                access.access, expected_access
            ));
        }
        if !specializations.contains(&access.specialization) {
            specializations.push(access.specialization.clone());
        }
    }
    if specializations.is_empty() {
        return Err(format!(
            "optimizer v3 indexed-field {:?} for {instr_id} lost all prevalidated codegen guards",
            expected_access
        ));
    }
    Ok(PreparedIndexedFieldAccessPlan {
        access: expected_access,
        specializations,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_v3::{
        IndexedFieldGuardKind, IndexedFieldOwnerType, IndexedFieldSpecializationPlan,
    };
    use soac_core::block_py::BlockLabel;

    fn field_plan(
        module_name: &str,
        qualname: &str,
        attr_name: &str,
        expected_index: u32,
        access: IndexedFieldAccessKind,
    ) -> IndexedFieldAccessPlan {
        let source = InstrId::new(BlockLabel::from_index(0), expected_index);
        let emitted = IndexedFieldSpecializationPlan {
            source,
            access,
            guard: crate::plan_v3::IndexedFieldGuardPlan {
                kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
            },
            fallback: crate::plan_v3::IndexedFieldFallbackPlan {
                kind: IndexedFieldFallbackKind::OriginalAttrAccess,
            },
            owner_type: IndexedFieldOwnerType {
                module_name: module_name.to_string(),
                qualname: qualname.to_string(),
            },
            attr_name: attr_name.to_string(),
            expected_index,
            reason: "test".to_string(),
        };
        IndexedFieldAccessPlan {
            access: emitted.access,
            guard: MechanicalIndexedFieldGuard {
                kind: emitted.guard.kind,
                owner_type: emitted.owner_type,
                attr_name: emitted.attr_name,
                expected_index: emitted.expected_index,
            },
            fallback: emitted.fallback.kind,
        }
    }

    #[test]
    fn indexed_field_layout_groups_are_deduped_and_sorted() {
        let duplicate = field_plan("zmod", "B", "z", 2, IndexedFieldAccessKind::Load);
        let plans = [
            duplicate.clone(),
            field_plan("amod", "A", "b", 3, IndexedFieldAccessKind::Store),
            field_plan("amod", "A", "a", 1, IndexedFieldAccessKind::Load),
            duplicate,
        ];

        let groups = indexed_field_layout_groups(plans.iter());

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].type_key.module_name, "amod");
        assert_eq!(
            groups[0]
                .layouts
                .iter()
                .map(|layout| (layout.key.as_str(), layout.index))
                .collect::<Vec<_>>(),
            vec![("a", 1), ("b", 3)]
        );
        assert_eq!(groups[1].type_key.module_name, "zmod");
        assert_eq!(
            groups[1]
                .layouts
                .iter()
                .map(|layout| (layout.key.as_str(), layout.index))
                .collect::<Vec<_>>(),
            vec![("z", 2)]
        );
    }

    #[test]
    fn prepare_indexed_field_accesses_dedupes_before_runtime_resolution() {
        let instr_id = InstrId::new(BlockLabel::from_index(1), 4);
        let plan = field_plan("module", "Owner", "field", 7, IndexedFieldAccessKind::Load);
        let mut by_instr = HashMap::new();
        by_instr.insert(instr_id, vec![plan.clone(), plan]);
        let mut resolved_requests = Vec::new();

        let prepared = prepare_indexed_field_accesses_for_codegen(Some(&by_instr), |request| {
            resolved_requests.push((
                request.type_key.module_name.clone(),
                request.attr_name.clone(),
                request.expected_index,
            ));
            Ok(Some(request.expected_index))
        })
        .expect("indexed-field requests should prepare");

        assert_eq!(
            resolved_requests,
            vec![("module".to_string(), "field".to_string(), 7)]
        );
        assert_eq!(prepared[&instr_id].len(), 1);
        assert_eq!(prepared[&instr_id][0].specialization, 7);
    }

    #[test]
    fn prepared_indexed_field_access_plan_validates_requested_access() {
        let instr_id = InstrId::new(BlockLabel::from_index(1), 4);
        let resolved = ResolvedIndexedFieldAccess {
            access: IndexedFieldAccessKind::Load,
            attr_name: "field".to_string(),
            guard: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
            fallback: IndexedFieldFallbackKind::OriginalAttrAccess,
            specialization: 11,
        };
        let mut by_instr = HashMap::new();
        by_instr.insert(instr_id, vec![resolved]);

        let prepared =
            prepared_indexed_field_access_plan(instr_id, IndexedFieldAccessKind::Load, &by_instr)
                .expect("matching indexed-field access should prepare");

        assert_eq!(prepared.access, IndexedFieldAccessKind::Load);
        assert_eq!(prepared.specializations, vec![11]);

        let err =
            prepared_indexed_field_access_plan(instr_id, IndexedFieldAccessKind::Store, &by_instr)
                .expect_err("mismatched indexed-field access should be rejected");
        assert!(
            err.contains("typed lowering requested Store"),
            "unexpected error: {err}"
        );
    }
}
