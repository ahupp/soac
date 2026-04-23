use crate::artifacts_v3::ExactIntBranchV3Artifacts;
use crate::emit_v3::{MechanicalExitKind, MechanicalRegionEmission, MechanicalStepOp};
use crate::plan_v3::{
    FailureMode, FallbackTarget, GuardFailure, RegionId, RegionPlan, ScalarLocalThreadPlan,
    ScalarThreadFallback, ScalarThreadLocalCleanup, ScalarThreadLocalLocation,
    ScalarThreadLocalState,
};
use soac_core::block_py::{InstrId, LocalLocation, NameLike, ResolvedName};

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
