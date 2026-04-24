#![deny(unreachable_pub)]

pub mod codegen;
mod config;
mod instrument;
pub mod typed;

pub use config::{
    CounterInstrumentationConfig, ExplicitCounterPlacement, InstrumentationConfig,
    RefcountCounterMode,
};
pub use instrument::{
    CounterBuilder, CounterHandle, CounterSpec, InstrumentInstr, OptBlock, OptInstr,
};
