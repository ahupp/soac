use soac_config::{ExecTraceConfig, SoacEnvConfig, SpecializationMode};
use soac_core::block_py::CounterScope;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstrumentationConfig {
    pub trace: Option<ExecTraceConfig>,
    pub counters: CounterInstrumentationConfig,
    pub explicit_counter_placement: ExplicitCounterPlacement,
    pub deopt_entry_counters: bool,
    pub specialization_runtime_logging: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CounterInstrumentationConfig {
    pub call_targets: bool,
    pub locality: bool,
    pub profiled_cold_blocks: bool,
    pub refcounts: RefcountCounterMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefcountCounterMode {
    Disabled,
    Scoped(CounterScope),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplicitCounterPlacement {
    Codegen,
    Typed,
}

impl InstrumentationConfig {
    pub fn from_env_config(config: &SoacEnvConfig) -> Self {
        let top_value_counters = config
            .specialization_mode()
            .is_some_and(SpecializationMode::records_counters);
        let explicit_counter_placement = if config
            .runtime_optimization_pipeline()
            .uses_typed_v3_runtime()
        {
            ExplicitCounterPlacement::Typed
        } else {
            ExplicitCounterPlacement::Codegen
        };
        let deopt_entry_counters = if config
            .runtime_optimization_pipeline()
            .uses_typed_v3_runtime()
        {
            false
        } else {
            matches!(
                config.specialization_mode(),
                Some(SpecializationMode::Verify | SpecializationMode::Apply)
            )
        };
        let refcounts = if config.specialization_mode() == Some(SpecializationMode::Verify) {
            RefcountCounterMode::Scoped(CounterScope::Function)
        } else {
            RefcountCounterMode::Disabled
        };
        Self {
            trace: config.soac_exec_trace().cloned(),
            counters: CounterInstrumentationConfig {
                call_targets: top_value_counters,
                locality: top_value_counters,
                profiled_cold_blocks: config.profiled_cold_blocks_enabled(),
                refcounts,
            },
            explicit_counter_placement,
            deopt_entry_counters,
            specialization_runtime_logging: config.specialization_runtime_logging_enabled(),
        }
    }

    pub fn deopt_entry_counters_enabled(&self) -> bool {
        self.deopt_entry_counters
    }

    pub fn specialization_runtime_logging_enabled(&self) -> bool {
        self.specialization_runtime_logging
    }
}

impl RefcountCounterMode {
    pub fn scope(self) -> Option<CounterScope> {
        match self {
            Self::Disabled => None,
            Self::Scoped(scope) => Some(scope),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_config::RuntimeOptimizationPipeline;

    #[test]
    fn profile_mode_records_top_value_counters_without_refcount_counters() {
        let config = SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Profile))
            .with_profiled_cold_blocks_enabled(true);

        let instrumentation = InstrumentationConfig::from_env_config(&config);

        assert!(instrumentation.counters.call_targets);
        assert!(instrumentation.counters.locality);
        assert!(instrumentation.counters.profiled_cold_blocks);
        assert_eq!(
            instrumentation.counters.refcounts,
            RefcountCounterMode::Disabled
        );
        assert!(!instrumentation.deopt_entry_counters_enabled());
    }

    #[test]
    fn typed_v3_runtime_places_explicit_counters_in_typed_ir() {
        let config = SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Profile))
            .with_runtime_optimization_pipeline(RuntimeOptimizationPipeline::TypedV3);

        let instrumentation = InstrumentationConfig::from_env_config(&config);

        assert_eq!(
            instrumentation.explicit_counter_placement,
            ExplicitCounterPlacement::Typed
        );
    }

    #[test]
    fn verify_mode_records_refcount_and_deopt_entry_counters_for_plan_artifacts() {
        let config = SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Verify))
            .with_runtime_optimization_pipeline(RuntimeOptimizationPipeline::PlanArtifacts);

        let instrumentation = InstrumentationConfig::from_env_config(&config);

        assert_eq!(
            instrumentation.counters.refcounts,
            RefcountCounterMode::Scoped(CounterScope::Function)
        );
        assert!(instrumentation.deopt_entry_counters_enabled());
    }

    #[test]
    fn typed_v3_runtime_disables_deopt_entry_counters() {
        let config = SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Apply))
            .with_runtime_optimization_pipeline(RuntimeOptimizationPipeline::TypedV3);

        let instrumentation = InstrumentationConfig::from_env_config(&config);

        assert!(!instrumentation.deopt_entry_counters_enabled());
    }
}
