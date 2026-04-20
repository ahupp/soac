#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V3MigrationStatus {
    RepresentedForComparison,
    LiveValidationOnly,
    LiveCodegenInputOnly,
    LegacyOnly,
    NotAnOptimizationPlanTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct V3OptimizationFamilyStatus {
    pub family: &'static str,
    pub legacy_input: &'static str,
    pub status: V3MigrationStatus,
    pub next_step: &'static str,
}

pub const V3_OPTIMIZATION_FAMILY_STATUS: &[V3OptimizationFamilyStatus] = &[
    V3OptimizationFamilyStatus {
        family: "exact-int direct-compare/add-compare branches and add returns",
        legacy_input: "operator_hot_shapes",
        status: V3MigrationStatus::LiveCodegenInputOnly,
        next_step: "migrate the remaining value-producing exact-int binary and unary operators to v3 plans",
    },
    V3OptimizationFamilyStatus {
        family: "remaining exact-int value-producing binary and unary operators",
        legacy_input: "operator_hot_shapes",
        status: V3MigrationStatus::LegacyOnly,
        next_step: "model non-add operations, bool materialization for comparisons, and fallback ownership",
    },
    V3OptimizationFamilyStatus {
        family: "profiled direct calls",
        legacy_input: "call_hot_targets",
        status: V3MigrationStatus::LegacyOnly,
        next_step: "model call alternatives, argument ownership, and callable guard failure policy",
    },
    V3OptimizationFamilyStatus {
        family: "exact-list getitem and setitem",
        legacy_input: "getitem_hot_shapes and setitem_hot_shapes",
        status: V3MigrationStatus::LegacyOnly,
        next_step: "model item-operation alternatives with local fallback and mutation effects",
    },
    V3OptimizationFamilyStatus {
        family: "indexed globals and fields",
        legacy_input: "module_keys, type_keys, and indexed hit/fallback counters",
        status: V3MigrationStatus::LegacyOnly,
        next_step: "model indexed load/store alternatives with explicit deopt replay reasons",
    },
    V3OptimizationFamilyStatus {
        family: "branch locality and cold block layout",
        legacy_input: "branch_outcomes and block_entry",
        status: V3MigrationStatus::NotAnOptimizationPlanTarget,
        next_step: "keep as layout metadata unless a future v3 CFG-placement plan needs it",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn v3_family_status_has_unique_family_names() {
        let mut names = HashSet::new();
        for entry in V3_OPTIMIZATION_FAMILY_STATUS {
            assert!(
                names.insert(entry.family),
                "duplicate family {}",
                entry.family
            );
            assert!(!entry.next_step.is_empty());
        }
    }
}
