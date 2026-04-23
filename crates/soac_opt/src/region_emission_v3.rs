use crate::artifacts_v3::ExactIntBranchV3Artifacts;
use crate::emit_v3::{MechanicalExitKind, MechanicalRegionEmission, MechanicalStepOp};
use crate::passes::InstrTyped;
use crate::plan_v3::{
    FailureMode, FallbackTarget, GuardFailure, RegionId, RegionPlan, ScalarLocalThreadPlan,
    ScalarThreadFallback, ScalarThreadLocalCleanup, ScalarThreadLocalLocation,
    ScalarThreadLocalState, ScalarThreadMaterialization,
};
use soac_core::block_py::{
    BlockArg, BlockLabel, BlockPyFunction, BlockTerm, ChildVisitable, InstrId, LocalLocation,
    NameLike, ResolvedName, Visit,
};
use soac_lowering::passes::{CodegenModuleShape, InstrCodegen};
use std::collections::HashMap;

#[derive(Clone, Copy)]
pub struct ExactIntBranchSelection<'a> {
    pub hot_plan: &'a RegionPlan,
    pub hot_region: &'a MechanicalRegionEmission,
    pub fallback_plan: &'a RegionPlan,
    pub fallback_region: &'a MechanicalRegionEmission,
}

#[derive(Clone, Copy)]
pub struct ExactIntReturnSelection<'a> {
    pub hot_plan: &'a RegionPlan,
    pub hot_region: &'a MechanicalRegionEmission,
    pub fallback_plan: &'a RegionPlan,
    pub fallback_region: &'a MechanicalRegionEmission,
}

#[derive(Clone, Copy)]
pub struct ScalarThreadSelection<'a> {
    pub thread: &'a ScalarLocalThreadPlan,
    pub producer: ExactIntReturnSelection<'a>,
    pub consumer: ExactIntBranchSelection<'a>,
}

#[derive(Clone, Copy)]
pub struct ScalarThreadInlineReturnTargets<'a> {
    pub then_label: BlockLabel,
    pub then_term: &'a BlockTerm<InstrCodegen>,
    pub else_label: BlockLabel,
    pub else_term: &'a BlockTerm<InstrCodegen>,
}

pub fn exact_int_branch_selection_for_source(
    artifacts: &ExactIntBranchV3Artifacts,
    source: InstrId,
) -> Result<Option<ExactIntBranchSelection<'_>>, String> {
    let Some(planned_function) = artifacts.plan.functions.first() else {
        return Ok(None);
    };
    let Some(emitted_function) = artifacts.emission.functions.first() else {
        return Ok(None);
    };
    let Some(hot_region) = emitted_function
        .regions
        .iter()
        .find(|region| region_has_branch_source(region, source))
    else {
        return Ok(None);
    };
    let hot_plan = planned_function
        .regions
        .iter()
        .find(|region| region.id == hot_region.region)
        .ok_or_else(|| {
            format!(
                "optimizer v3 emission region {:?} for source {source} has no matching plan region",
                hot_region.region
            )
        })?;
    let fallback_region_id = local_fallback_region(hot_region).ok_or_else(|| {
        format!(
            "prevalidated optimizer v3 branch region {:?} for source {source} has no local fallback region",
            hot_region.region
        )
    })?;
    let fallback_region = emitted_function
        .regions
        .iter()
        .find(|region| region.region == fallback_region_id)
        .ok_or_else(|| {
            format!(
                "optimizer v3 branch region {:?} for source {source} references missing fallback region {:?}",
                hot_region.region, fallback_region_id
            )
        })?;
    let fallback_plan = planned_function
        .regions
        .iter()
        .find(|region| region.id == fallback_region_id)
        .ok_or_else(|| {
            format!(
                "optimizer v3 fallback emission region {:?} for source {source} has no matching plan region",
                fallback_region_id
            )
        })?;
    Ok(Some(ExactIntBranchSelection {
        hot_plan,
        hot_region,
        fallback_plan,
        fallback_region,
    }))
}

pub fn exact_int_return_selection_for_source(
    artifacts: &ExactIntBranchV3Artifacts,
    source: InstrId,
) -> Result<Option<ExactIntReturnSelection<'_>>, String> {
    let Some(planned_function) = artifacts.plan.functions.first() else {
        return Ok(None);
    };
    let Some(emitted_function) = artifacts.emission.functions.first() else {
        return Ok(None);
    };
    let Some(hot_region) = emitted_function
        .regions
        .iter()
        .find(|region| region_has_return_source(region, source))
    else {
        return Ok(None);
    };
    let hot_plan = planned_function
        .regions
        .iter()
        .find(|region| region.id == hot_region.region)
        .ok_or_else(|| {
            format!(
                "optimizer v3 emission region {:?} for source {source} has no matching plan region",
                hot_region.region
            )
        })?;
    let fallback_region_id = local_fallback_region(hot_region).ok_or_else(|| {
        format!(
            "prevalidated optimizer v3 return region {:?} for source {source} has no local fallback region",
            hot_region.region
        )
    })?;
    let fallback_region = emitted_function
        .regions
        .iter()
        .find(|region| region.region == fallback_region_id)
        .ok_or_else(|| {
            format!(
                "optimizer v3 return region {:?} for source {source} references missing fallback region {:?}",
                hot_region.region, fallback_region_id
            )
        })?;
    let fallback_plan = planned_function
        .regions
        .iter()
        .find(|region| region.id == fallback_region_id)
        .ok_or_else(|| {
            format!(
                "optimizer v3 fallback emission region {:?} for source {source} has no matching plan region",
                fallback_region_id
            )
        })?;
    Ok(Some(ExactIntReturnSelection {
        hot_plan,
        hot_region,
        fallback_plan,
        fallback_region,
    }))
}

pub fn scalar_thread_selection_for_store_branch<'a>(
    artifacts: &'a ExactIntBranchV3Artifacts,
    producer_source: InstrId,
    consumer_source: InstrId,
    local_name: &ResolvedName,
) -> Result<Option<ScalarThreadSelection<'a>>, String> {
    let Some(emitted_function) = artifacts.emission.functions.first() else {
        return Ok(None);
    };
    let Some(producer) = exact_int_return_selection_for_source(artifacts, producer_source)? else {
        return Ok(None);
    };
    let Some(consumer) = exact_int_branch_selection_for_source(artifacts, consumer_source)? else {
        return Ok(None);
    };
    let Some(thread) = emitted_function.scalar_threads.iter().find(|thread| {
        thread.producer.region == producer.hot_plan.id
            && thread.consumer.region == consumer.hot_plan.id
            && scalar_thread_matches_local(thread, local_name)
    }) else {
        return Ok(None);
    };
    let ScalarThreadFallback::LocalFallbackRegion { region, .. } = &thread.fallback;
    debug_assert_eq!(*region, producer.fallback_plan.id);
    if !matches!(
        thread.materialization,
        ScalarThreadMaterialization::DeferredUntilPythonObjectUse { .. }
    ) {
        return Err(format!(
            "optimizer v3 scalar thread for local {} has materialization unsupported by current mechanical lowering: {:?}",
            thread.local.name, thread.materialization
        ));
    }
    Ok(Some(ScalarThreadSelection {
        thread,
        producer,
        consumer,
    }))
}

pub fn scalar_thread_unmaterialized_local_location(
    thread: &ScalarLocalThreadPlan,
) -> Result<Option<LocalLocation>, String> {
    match &thread.local_state {
        ScalarThreadLocalState::ScalarOnlyHotPath {
            cleanup: ScalarThreadLocalCleanup::NoPyObjectSlotOwnership,
            ..
        } => match thread.local.location {
            ScalarThreadLocalLocation::Local { slot } => Ok(Some(LocalLocation(slot))),
        },
    }
}

pub fn scalar_thread_inline_return_targets<'a>(
    function: &'a BlockPyFunction<CodegenModuleShape>,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    if_term: &soac_core::block_py::TermIf<InstrTyped>,
    local_name: &ResolvedName,
) -> Result<Option<ScalarThreadInlineReturnTargets<'a>>, String> {
    if if_term.then_label == if_term.else_label {
        return Ok(None);
    }
    let Some(then_term) = scalar_thread_inline_return_target(
        function,
        block_indices_by_label,
        if_term.then_label,
        local_name,
    )?
    else {
        return Ok(None);
    };
    let Some(else_term) = scalar_thread_inline_return_target(
        function,
        block_indices_by_label,
        if_term.else_label,
        local_name,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(ScalarThreadInlineReturnTargets {
        then_label: if_term.then_label,
        then_term,
        else_label: if_term.else_label,
        else_term,
    }))
}

fn scalar_thread_matches_local(thread: &ScalarLocalThreadPlan, local_name: &ResolvedName) -> bool {
    let Some(location) = local_name.local_location() else {
        return false;
    };
    match thread.local.location {
        ScalarThreadLocalLocation::Local { slot } => {
            slot == location.slot() && thread.local.name == local_name.id_str()
        }
    }
}

fn scalar_thread_inline_return_target<'a>(
    function: &'a BlockPyFunction<CodegenModuleShape>,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    target: BlockLabel,
    local_name: &ResolvedName,
) -> Result<Option<&'a BlockTerm<InstrCodegen>>, String> {
    if block_predecessor_count(function, target) != 1 {
        return Ok(None);
    }
    let target_index = *block_indices_by_label
        .get(&target)
        .ok_or_else(|| format!("missing optimizer v3 scalar-thread target block {target}"))?;
    let target_block = &function.blocks[target_index];
    if !target_block.body.is_empty() || target_block.exception_param().is_some() {
        return Ok(None);
    }
    let BlockTerm::Return(_) = &target_block.term else {
        return Ok(None);
    };
    if term_references_local(&target_block.term, local_name) {
        return Ok(None);
    }
    Ok(Some(&target_block.term))
}

fn term_references_local(term: &BlockTerm<InstrCodegen>, local_name: &ResolvedName) -> bool {
    struct LocalRefFinder<'a> {
        local_name: &'a ResolvedName,
        found: bool,
    }

    impl Visit<InstrCodegen> for LocalRefFinder<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if let InstrCodegen::Load(load) = expr
                && same_resolved_local_name(&load.name, self.local_name)
            {
                self.found = true;
                return;
            }
            expr.visit_children(self);
        }

        fn visit_block_arg(&mut self, arg: &BlockArg) {
            if let BlockArg::Name(name) = arg
                && name == self.local_name.id_str()
            {
                self.found = true;
            }
        }
    }

    let mut finder = LocalRefFinder {
        local_name,
        found: false,
    };
    finder.visit_term(term);
    finder.found
}

fn same_resolved_local_name(left: &ResolvedName, right: &ResolvedName) -> bool {
    left.local_location() == right.local_location() && left.id_str() == right.id_str()
}

fn block_predecessor_count(
    function: &BlockPyFunction<CodegenModuleShape>,
    target: BlockLabel,
) -> usize {
    function
        .blocks
        .iter()
        .filter(|block| match &block.term {
            BlockTerm::Jump(edge) => edge.target == target,
            BlockTerm::IfTerm(if_term) => {
                if_term.then_label == target || if_term.else_label == target
            }
            BlockTerm::BranchTable(branch) => {
                branch.default_label == target || branch.targets.contains(&target)
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => false,
        })
        .count()
}

fn region_has_branch_source(region: &MechanicalRegionEmission, source: InstrId) -> bool {
    region.exits.iter().any(|exit| {
        exit.source == Some(source) && matches!(exit.kind, MechanicalExitKind::Branch { .. })
    })
}

fn region_has_return_source(region: &MechanicalRegionEmission, source: InstrId) -> bool {
    region.exits.iter().any(|exit| {
        exit.source == Some(source) && matches!(exit.kind, MechanicalExitKind::Return { .. })
    })
}

fn local_fallback_region(region: &MechanicalRegionEmission) -> Option<RegionId> {
    for step in &region.steps {
        match &step.op {
            MechanicalStepOp::Guard { failure, .. } => match failure {
                GuardFailure::FallbackToPlan {
                    target: FallbackTarget::Region(region),
                    ..
                } => return Some(*region),
                GuardFailure::FallbackToPlan { .. } | GuardFailure::DeoptTo { .. } => {}
            },
            MechanicalStepOp::Convert { failure, .. }
            | MechanicalStepOp::Operation { failure, .. } => {
                if let Some(region) = failure_fallback_region(failure) {
                    return Some(region);
                }
            }
            _ => {}
        }
    }
    None
}

fn failure_fallback_region(failure: &FailureMode) -> Option<RegionId> {
    match failure {
        FailureMode::FallbackToPlan {
            target: FallbackTarget::Region(region),
            ..
        } => Some(*region),
        FailureMode::CannotFail
        | FailureMode::Raise(_)
        | FailureMode::FallbackToPlan { .. }
        | FailureMode::DeoptTo { .. } => None,
    }
}
