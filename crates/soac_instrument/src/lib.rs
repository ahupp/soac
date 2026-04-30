#![deny(unreachable_pub)]

mod config;
mod instrument;
mod typed;

pub use config::{CounterInstrumentationConfig, InstrumentationConfig, RefcountCounterMode};
pub use instrument::{
    CounterBuilder, CounterHandle, CounterSpec, RUNTIME_DECREF_LOCATION_COUNTER_KIND,
};
pub use typed::{
    define_typed_module_counter_defs, instrument_typed_module, instrument_typed_module_with_tracker,
};
