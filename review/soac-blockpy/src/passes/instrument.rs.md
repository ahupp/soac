# soac-blockpy/src/passes/instrument.rs

## File Responsibilities
Defines generic instrumentation primitives for counter-backed optimization. It provides counter allocation, a trait for instruction-specific instrumentation/optimization rules, and an `OptBlock` fragment model for replacing hot instructions with specialized block fragments.

## Datatypes
- `CounterHandle`: lightweight id wrapper for a counter definition.
- `CounterSpec`: trait implemented by counter descriptions that can define/reuse counters.
- `CounterBuilder`: allocator/reuser for `CounterDef` entries.
- `OptInstr`: optimization result for one instruction: unchanged, replaced instruction, or block fragment.
- `OptBlock`: validated block fragment with one entry block and fallthrough-only dependencies.
- `InstrumentInstr`: trait for rules that can instrument an instruction and later optimize from collected counter data.

## Functions
- `CounterHandle::new`: creates a handle from a counter id.
- `CounterBuilder::new`: wraps a mutable counter definition vector.
- `CounterBuilder::define`: appends a concrete counter definition.
- `CounterBuilder::define_spec`: appends from a `CounterSpec`.
- `CounterBuilder::define_if_missing`: returns an existing matching counter or defines a new one.
- `CounterBuilder::define_if_missing_spec`: spec-based version of `define_if_missing`.
- `OptBlock::new`: validates a specialization fragment and constructs it.
- `OptBlock` accessors/mutators expose entry/dependency blocks and split the fragment into parts.
- `OptBlock::replace_fallthrough_target`: retargets all fallthrough exits in a fragment.
- `validate_opt_block`: rejects fragments without dependencies, with duplicate labels, or with invalid exits.
- `all_paths_end_in_fallthrough`: verifies that dependency paths terminate in fallthrough to the entry block.
- `InstrumentInstr::instrument_instr`: rule hook for deciding whether an instruction gets a counter.
- `InstrumentInstr::optimize_instr`: rule hook for producing a specialized replacement using counter data.

## Context Read
- `trace/mod.rs` for concrete counter instrumentation passes.
- `soac-blockpy/src/block_py/counters.rs` and CFG block structures.
