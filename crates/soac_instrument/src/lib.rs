#![deny(unreachable_pub)]

pub mod codegen;
mod config;
mod instrument;

pub use config::{CounterInstrumentationConfig, InstrumentationConfig, RefcountCounterMode};
pub use instrument::{
    CounterBuilder, CounterHandle, CounterSpec, InstrumentInstr, OptBlock, OptInstr,
};
