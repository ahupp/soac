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
        family: "exact-int direct-compare/add-compare branches, add/sub/mul/bitwise returns, and comparison returns",
        legacy_input: "operator_hot_shapes",
        status: V3MigrationStatus::LiveCodegenInputOnly,
        next_step: "migrate the remaining division/modulo/shift and unary value-producing exact-int operators to v3 plans",
    },
    V3OptimizationFamilyStatus {
        family: "remaining division/modulo/shift and unary value-producing exact-int operators",
        legacy_input: "operator_hot_shapes",
        status: V3MigrationStatus::LegacyOnly,
        next_step: "model remaining operator semantics, fallback ownership, and unsupported-overflow boundaries",
    },
    V3OptimizationFamilyStatus {
        family: "profiled direct calls and guarded receiver-method calls",
        legacy_input: "call_hot_targets",
        status: V3MigrationStatus::LiveCodegenInputOnly,
        next_step: "add constructor variants and lift call lowering further into mechanical v3 nodes",
    },
    V3OptimizationFamilyStatus {
        family: "exact-list getitem and setitem",
        legacy_input: "getitem_hot_shapes and setitem_hot_shapes",
        status: V3MigrationStatus::LiveCodegenInputOnly,
        next_step: "lift exact-list getitem/setitem lowering itself into mechanical v3 nodes with explicit list effects",
    },
    V3OptimizationFamilyStatus {
        family: "indexed fields",
        legacy_input: "type_keys and field_indexed hit/fallback counters",
        status: V3MigrationStatus::LiveCodegenInputOnly,
        next_step: "lift typed attribute load/store lowering itself into mechanical v3 nodes",
    },
    V3OptimizationFamilyStatus {
        family: "indexed globals",
        legacy_input: "module_keys and global_indexed hit/fallback counters",
        status: V3MigrationStatus::LiveCodegenInputOnly,
        next_step: "lift indexed global load/store lowering itself into mechanical v3 nodes",
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
