use soac_core::block_py::{
    BinOpKind, BlockLabel, CallArgKeyword, CallArgPositional, CounterBranchId, CounterDef,
    CounterId, CounterScope, CounterSite, InstrId, RuntimeFunctionId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CounterHandle {
    id: CounterId,
}

impl CounterHandle {
    pub const fn id(self) -> CounterId {
        self.id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub(crate) struct CounterBranchHandle {
    id: CounterId,
    branch_id: CounterBranchId,
}

#[allow(dead_code)]
impl CounterBranchHandle {
    pub(crate) const fn id(self) -> CounterId {
        self.id
    }

    pub(crate) const fn branch_id(self) -> CounterBranchId {
        self.branch_id
    }
}

pub trait CounterSpec {
    fn scope(&self) -> CounterScope;
    fn kind(&self) -> &str;
    fn site(&self) -> CounterSite;
}

pub struct CounterBuilder<'a> {
    defs: &'a mut Vec<CounterDef>,
    next_id: usize,
}

impl<'a> CounterBuilder<'a> {
    pub fn new(defs: &'a mut Vec<CounterDef>) -> Self {
        let next_id = defs
            .iter()
            .map(|def| def.id.0)
            .max()
            .map(|id| id + 1)
            .unwrap_or(0);
        Self { defs, next_id }
    }

    pub fn define(
        &mut self,
        scope: CounterScope,
        kind: impl Into<String>,
        site: CounterSite,
    ) -> CounterHandle {
        let handle = CounterHandle {
            id: CounterId(self.next_id),
        };
        self.next_id += 1;
        self.defs
            .push(CounterDef::scalar(handle.id, scope, kind, site));
        handle
    }

    pub fn define_branch_counter(
        &mut self,
        scope: CounterScope,
        kind: impl Into<String>,
        site: CounterSite,
        branches: impl IntoIterator<Item = impl Into<String>>,
    ) -> CounterHandle {
        let handle = CounterHandle {
            id: CounterId(self.next_id),
        };
        self.next_id += 1;
        self.defs.push(CounterDef::branch_counter(
            handle.id, scope, kind, site, branches,
        ));
        handle
    }

    pub fn define_spec(&mut self, spec: &impl CounterSpec) -> CounterHandle {
        self.define(spec.scope(), spec.kind(), spec.site())
    }

    pub fn define_if_missing(
        &mut self,
        scope: CounterScope,
        kind: impl Into<String>,
        site: CounterSite,
    ) -> CounterHandle {
        let kind = kind.into();
        if let Some(existing) = self
            .defs
            .iter()
            .find(|counter| counter.scope == scope && counter.kind == kind && counter.site == site)
        {
            return CounterHandle { id: existing.id };
        }
        self.define(scope, kind, site)
    }

    pub fn define_branch_counter_if_missing(
        &mut self,
        scope: CounterScope,
        kind: impl Into<String>,
        site: CounterSite,
        branches: impl IntoIterator<Item = impl Into<String>>,
    ) -> CounterHandle {
        let kind = kind.into();
        let branches = branches
            .into_iter()
            .map(Into::into)
            .collect::<Vec<String>>();
        if let Some(existing) = self
            .defs
            .iter()
            .find(|counter| counter.scope == scope && counter.kind == kind && counter.site == site)
        {
            let existing_branches = existing
                .branches
                .iter()
                .map(|branch| branch.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                existing_branches,
                branches.iter().map(String::as_str).collect::<Vec<_>>(),
                "counter {kind:?} at {site:?} was already defined with different branches",
            );
            return CounterHandle { id: existing.id };
        }
        self.define_branch_counter(scope, kind, site, branches)
    }

    #[allow(dead_code)]
    pub(crate) fn branch_handle(&self, handle: CounterHandle, branch: &str) -> CounterBranchHandle {
        let counter = self
            .defs
            .iter()
            .find(|counter| counter.id == handle.id)
            .unwrap_or_else(|| panic!("missing counter definition for id {}", handle.id.0));
        let branch_id = counter.branch_id(branch).unwrap_or_else(|| {
            panic!(
                "missing branch {branch:?} for counter {} ({})",
                handle.id.0, counter.kind
            )
        });
        CounterBranchHandle {
            id: handle.id,
            branch_id,
        }
    }

    pub fn define_if_missing_spec(&mut self, spec: &impl CounterSpec) -> CounterHandle {
        self.define_if_missing(spec.scope(), spec.kind(), spec.site())
    }
}

pub(crate) fn define_block_entry_counter(
    counters: &mut CounterBuilder<'_>,
    function_id: RuntimeFunctionId,
    block_label: BlockLabel,
) -> CounterHandle {
    counters.define_if_missing(
        CounterScope::This,
        "block_entry",
        CounterSite::BlockEntry {
            function_id,
            block_label,
        },
    )
}

pub(crate) fn define_refcount_counters(
    counters: &mut CounterBuilder<'_>,
    scope: CounterScope,
    function_ids: impl IntoIterator<Item = RuntimeFunctionId>,
) -> Result<(), String> {
    match scope {
        CounterScope::This => Err(
            "refcount counters do not yet support CounterScope::This; use Function or Global"
                .to_string(),
        ),
        CounterScope::Function => {
            for function_id in function_ids {
                for kind in ["runtime_incref", "runtime_decref"] {
                    counters.define_if_missing(
                        scope,
                        kind,
                        CounterSite::Runtime {
                            function_id: Some(function_id),
                            instr_id: None,
                        },
                    );
                }
            }
            Ok(())
        }
        CounterScope::Global => {
            for kind in ["runtime_incref", "runtime_decref"] {
                counters.define_if_missing(
                    scope,
                    kind,
                    CounterSite::Runtime {
                        function_id: None,
                        instr_id: None,
                    },
                );
            }
            Ok(())
        }
    }
}

pub(crate) fn define_branch_outcome_counter(
    counters: &mut CounterBuilder<'_>,
    function_id: RuntimeFunctionId,
    instr_id: InstrId,
) -> CounterHandle {
    counters.define_if_missing(
        CounterScope::This,
        "branch_outcomes",
        CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: Some(instr_id),
        },
    )
}

pub(crate) fn define_indexed_counter(
    counters: &mut CounterBuilder<'_>,
    function_id: RuntimeFunctionId,
    instr_id: InstrId,
    kind: &'static str,
) -> CounterHandle {
    counters.define_branch_counter_if_missing(
        CounterScope::This,
        kind,
        CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: Some(instr_id),
        },
        ["hit", "fallback"],
    )
}

pub(crate) fn define_field_access_counter(
    counters: &mut CounterBuilder<'_>,
    function_id: RuntimeFunctionId,
    instr_id: InstrId,
) -> CounterHandle {
    counters.define_branch_counter_if_missing(
        CounterScope::This,
        "field_access",
        CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: Some(instr_id),
        },
        [
            "indexed_hit",
            "indexed_fallback",
            "generic_getattr",
            "generic_setattr",
        ],
    )
}

pub(crate) fn define_instr_shape_counters(
    counters: &mut CounterBuilder<'_>,
    function_id: RuntimeFunctionId,
    instr_id: InstrId,
    shape_kind: &'static str,
    branch_kind: &'static str,
) {
    counters.define_if_missing(
        CounterScope::This,
        shape_kind,
        CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: Some(instr_id),
        },
    );
    counters.define_branch_counter_if_missing(
        CounterScope::This,
        branch_kind,
        CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: Some(instr_id),
        },
        ["hit", "fallback"],
    );
}

pub(crate) fn define_operator_hot_shapes_counter(
    counters: &mut CounterBuilder<'_>,
    function_id: RuntimeFunctionId,
    instr_id: InstrId,
) -> CounterHandle {
    counters.define_if_missing(
        CounterScope::This,
        "operator_hot_shapes",
        CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: Some(instr_id),
        },
    )
}

pub(crate) fn define_call_counters(
    counters: &mut CounterBuilder<'_>,
    function_id: RuntimeFunctionId,
    instr_id: InstrId,
) {
    counters.define_if_missing(
        CounterScope::This,
        "call_hot_targets",
        CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: Some(instr_id),
        },
    );
    counters.define_branch_counter_if_missing(
        CounterScope::This,
        "call_direct",
        CounterSite::Runtime {
            function_id: Some(function_id),
            instr_id: Some(instr_id),
        },
        ["hit", "fallback"],
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecializationCounterCandidate {
    GlobalIndexed { instr_id: InstrId },
    FieldAccess { instr_id: InstrId },
    OperatorHotShapes { instr_id: InstrId },
    GetItem { instr_id: InstrId },
    SetItem { instr_id: InstrId },
    Call { instr_id: InstrId },
}

pub(crate) fn is_operator_specialization_binop_kind(kind: BinOpKind) -> bool {
    matches!(
        kind,
        BinOpKind::Add
            | BinOpKind::Sub
            | BinOpKind::Mul
            | BinOpKind::And
            | BinOpKind::Or
            | BinOpKind::Xor
            | BinOpKind::Eq
            | BinOpKind::Ne
            | BinOpKind::Lt
            | BinOpKind::Le
            | BinOpKind::Gt
            | BinOpKind::Ge
    )
}

pub(crate) fn is_profile_call_candidate<I>(
    args: &[CallArgPositional<I>],
    keywords: &[CallArgKeyword<I>],
) -> bool {
    keywords.is_empty()
        && args
            .iter()
            .all(|arg| matches!(arg, CallArgPositional::Positional(_)))
}

pub(crate) fn define_specialization_counter_candidate(
    counters: &mut CounterBuilder<'_>,
    function_id: RuntimeFunctionId,
    candidate: SpecializationCounterCandidate,
) {
    match candidate {
        SpecializationCounterCandidate::GlobalIndexed { instr_id } => {
            define_indexed_counter(counters, function_id, instr_id, "global_indexed");
        }
        SpecializationCounterCandidate::FieldAccess { instr_id } => {
            define_field_access_counter(counters, function_id, instr_id);
        }
        SpecializationCounterCandidate::OperatorHotShapes { instr_id } => {
            define_operator_hot_shapes_counter(counters, function_id, instr_id);
        }
        SpecializationCounterCandidate::GetItem { instr_id } => {
            define_instr_shape_counters(
                counters,
                function_id,
                instr_id,
                "getitem_hot_shapes",
                "getitem_specialized",
            );
        }
        SpecializationCounterCandidate::SetItem { instr_id } => {
            define_instr_shape_counters(
                counters,
                function_id,
                instr_id,
                "setitem_hot_shapes",
                "setitem_specialized",
            );
        }
        SpecializationCounterCandidate::Call { instr_id } => {
            define_call_counters(counters, function_id, instr_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_core::block_py::{InstrId, RuntimeFunctionId};

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct CallHotTargetsCounterSpec {
        function_id: RuntimeFunctionId,
        instr_id: InstrId,
    }

    impl CounterSpec for CallHotTargetsCounterSpec {
        fn scope(&self) -> CounterScope {
            CounterScope::This
        }

        fn kind(&self) -> &str {
            "call_hot_targets"
        }

        fn site(&self) -> CounterSite {
            CounterSite::Runtime {
                function_id: Some(self.function_id),
                instr_id: Some(self.instr_id),
            }
        }
    }

    #[test]
    fn counter_builder_allocates_sequential_ids() {
        let mut defs = Vec::new();
        let mut builder = CounterBuilder::new(&mut defs);

        let first = builder.define(
            CounterScope::This,
            "call_hot_targets",
            CounterSite::Runtime {
                function_id: Some(RuntimeFunctionId::from_raw_parts(1, 2)),
                instr_id: Some(InstrId::new(4)),
            },
        );
        let second = builder.define(
            CounterScope::Global,
            "global_load_hit",
            CounterSite::Runtime {
                function_id: None,
                instr_id: None,
            },
        );

        assert_eq!(first.id(), CounterId(0));
        assert_eq!(second.id(), CounterId(1));
    }

    #[test]
    fn counter_builder_reuses_existing_definition() {
        let site = CounterSite::Runtime {
            function_id: Some(RuntimeFunctionId::from_raw_parts(1, 2)),
            instr_id: Some(InstrId::new(7)),
        };
        let mut defs = vec![CounterDef {
            id: CounterId(9),
            scope: CounterScope::Function,
            kind: "runtime_incref".to_string(),
            site: site.clone(),
            branches: Vec::new(),
        }];

        let handle = CounterBuilder::new(&mut defs).define_if_missing(
            CounterScope::Function,
            "runtime_incref",
            site,
        );

        assert_eq!(handle.id(), CounterId(9));
        assert_eq!(defs.len(), 1);
    }

    #[test]
    fn counter_builder_defines_from_counter_spec() {
        let spec = CallHotTargetsCounterSpec {
            function_id: RuntimeFunctionId::from_raw_parts(1, 2),
            instr_id: InstrId::new(4),
        };
        let mut defs = Vec::new();
        let handle = CounterBuilder::new(&mut defs).define_spec(&spec);

        assert_eq!(handle.id(), CounterId(0));
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].kind, "call_hot_targets");
        assert_eq!(
            defs[0].site,
            CounterSite::Runtime {
                function_id: Some(RuntimeFunctionId::from_raw_parts(1, 2)),
                instr_id: Some(InstrId::new(4)),
            }
        );
    }
}
