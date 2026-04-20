# Predecoded Interpreter Plan

The entry/deopt interpreter currently executes directly from the rich
`BlockPyFunction<CodegenModuleShape>` tree. `execute_from_cursor` walks the
function blocks and `execute_expr_owned` repeatedly matches on `InstrCodegen`,
then digs through nested payloads such as names, locations, call arguments,
block labels, and edge args.

Add a compact interpreter plan computed once per `BlockPyFunction`. Keep the
semantic `BlockPyFunction` as the source of truth, but execute a slot-indexed
and block-indexed plan.

Rough shape:

```rust
struct InterpPlan {
    blocks: Box<[InterpBlock]>,
    ops: Box<[InterpOp]>,
}

struct InterpBlock {
    body: Range<usize>,
    term: InterpTerm,
    exc_edge: Option<InterpEdge>,
}

enum InterpOp {
    LoadLocal { slot: u32, name: &'static str },
    StoreLocal { slot: u32, value: OpId },
    LoadGlobal { name_id: RuntimeNameOrInternedName, indexed_slot: i64 },
    LoadModuleConstant { index: u32 },
    BinOp { kind: BinOpKind, lhs: OpId, rhs: OpId },
    Call { callable: OpId, args: Range<usize>, kwargs: Range<usize> },
    RuntimeName { name: RuntimeName },
}
```

The important properties:

- Fetch blocks by dense block index rather than rediscovering them.
- Store local operands as `LocalLocation`/slot indexes, not names.
- Store runtime names as `RuntimeName` ids.
- Store module constants as constant indexes.
- Store jump targets as target block indexes and pre-resolved target local
  slots.
- Store call arguments as preclassified ranges/shapes instead of walking
  nested `CallArg*<InstrCodegen>` trees on every execution.
- Include a map from `RuntimeJitDeoptCursor` to compact block/op offsets so
  deopt can resume directly at the planned location.

This should help both the forced entry-interpreter benchmark and real deopt
replay. The entry interpreter will benefit most from direct call-shape and
argument setup work; real deopt replay should benefit most from cheaper
dispatch, dense block lookup, and pre-resolved jump-edge/local operands.
