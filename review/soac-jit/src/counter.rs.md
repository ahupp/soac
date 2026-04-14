# soac-jit/src/counter.rs

## File Responsibilities

Implements small fixed-capacity heavy-hitter counters used by runtime specialization profiling. It provides a generic
space-saving counter for Rust-side use and a C-layout two-slot top-value counter that generated code can update through raw
pointers while the GIL serializes access.

## Datatypes

- `CounterEntry<T>`: public snapshot row with the observed value, approximate count, and maximum overcount.
- `CounterSlot<T>`: internal generic counter slot storing value and space-saving accounting.
- `Counter<N, T>`: fixed-capacity generic space-saving heavy-hitter counter.
- `TopValueCounterSlot`: C-layout slot used by generated/JIT code-visible top-value counters.
- `TopValueCounter`: two-slot C-layout heavy-hitter counter for hot observed values such as branch outcomes and call targets.
- `GilTopValueCounter`: `UnsafeCell<TopValueCounter>` wrapper marked `Send`/`Sync` because mutation is expected to happen under
  the CPython GIL.

## Functions

- `CounterEntry::lower_bound`: returns `approx_count - max_overcount` with saturation.
- `Counter::default` / `Counter::new`: create an empty generic counter.
- `Counter::capacity`, `len`, `is_empty`: expose basic occupancy information.
- `Counter::entries`: returns borrowed entries sorted by descending approximate count.
- `Counter::snapshot`: returns cloned entries sorted by descending approximate count.
- `Counter::min_slot_index`: finds the occupied slot with the smallest approximate count.
- `Counter::record`: records a value, incrementing existing slots, filling empty slots, or replacing the minimum slot.
- `Counter::approx_count`: returns the approximate count for a value if it is currently tracked.
- `TopValueCounter::default` / `new`: create an empty two-slot top-value counter.
- `TopValueCounter::capacity`, `is_empty`: expose fixed capacity and occupancy.
- `TopValueCounter::snapshot`: returns tracked values sorted by descending approximate count.
- `TopValueCounter::min_slot_index`: finds the occupied minimum slot.
- `TopValueCounter::record`: C-layout equivalent of the space-saving record operation for `u64` values.
- `GilTopValueCounter::default` / `new`: create a GIL-protected top-value counter wrapper.
- `GilTopValueCounter::as_raw_ptr`: exposes a raw mutable pointer for JIT/runtime helper use.
- `GilTopValueCounter::snapshot_with_gil`: snapshots through the raw cell; caller must ensure GIL serialization.

## Context Read

- `soac-jit/src/module_type.rs`
- `soac-jit/src/counter_dump.rs`
- `soac-jit/src/jit/specialized_helpers.rs`

