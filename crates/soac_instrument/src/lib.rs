#![deny(unreachable_pub)]

mod codegen;
mod config;
mod instrument;
mod typed;

pub use codegen::instrument_module_with_tracker as instrument_codegen_module_with_tracker;
pub use config::{
    CounterInstrumentationConfig, ExplicitCounterPlacement, InstrumentationConfig,
    RefcountCounterMode,
};
pub use instrument::{
    CounterBuilder, CounterHandle, CounterSpec, InstrumentInstr, OptBlock, OptInstr,
};
pub use typed::{
    define_module_counter_defs as define_typed_module_counter_defs,
    instrument_module as instrument_typed_module,
    instrument_module_with_tracker as instrument_typed_module_with_tracker,
};
