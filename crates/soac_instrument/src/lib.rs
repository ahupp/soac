#![deny(unreachable_pub)]

pub mod codegen;
mod instrument;

pub use instrument::{
    CounterBuilder, CounterHandle, CounterSpec, InstrumentInstr, OptBlock, OptInstr,
};
