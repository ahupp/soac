# BlockPy Value Facts

The proposed direction is good, but the shape should preserve two
separate ideas:

1. Treat instruction IDs as a required invariant for codegen-shaped IR.
2. Keep value facts as a sidecar analysis, not embedded behavior or
   state on `BlockPyModule` payload nodes.

## Instruction IDs

`InstrId` already exists as `{ block_label, instr_index_in_block }`, and
`Meta` already carries `instr_id: Option<InstrId>`. The assignment pass
already runs after dense block relabeling in the lowering driver.

The important caveat is that `InstrId` is not module-global by itself.
Because assignment restarts per function, facts keyed across a module
need `(FunctionId, InstrId)`, not just `InstrId`. Counter sites already
generally use that shape.

A useful key type would be:

```rust
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InstrKey {
    pub function_id: FunctionId,
    pub instr_id: InstrId,
}
```

As a first implementation step, make `CodegenModuleShape` validation
require ID presence and uniqueness by `(FunctionId, InstrId)`.

## Sidecar Facts

The starting fact model can be small:

```rust
pub enum ValueFacts {
    PyObj(PyObjFacts),
}

pub struct PyObjFacts {
    pub exact_type: Option<PyExactType>,
    pub truthiness: TruthinessFacts,
    pub constant: Option<RuntimeConstantId>,
    pub provenance: ProvenanceFacts,
}

pub struct FactStore {
    expr_facts: HashMap<InstrKey, ValueFacts>,
    block_entry_facts: HashMap<(FunctionId, BlockLabel), EnvFacts>,
}

pub struct FactContext<'a> {
    pub module: &'a BlockPyModule<CodegenModuleShape>,
    pub function: &'a BlockPyFunction<CodegenModuleShape>,
}
```

Do not make "failed to infer" a compile-time type error. Python is too
dynamic for that. Most uncertainty should become unknown facts, not a
hard error. Reserve analysis errors for malformed IR, such as missing
instruction IDs in a phase that requires them.

```rust
pub enum FactError {
    MissingInstrId,
    MalformedIr(String),
}
```

If we need to model guaranteed runtime exceptions, keep that separate
from analysis failure:

```rust
pub enum RuntimeExceptionFacts {
    MayRaise,
    KnownRaisesPyTypeError,
}
```

## Inference API

Operation-specific inference logic is useful, but the primary API
should be a pass-level analysis over `InstrCodegen`, not methods on
every operation payload. Facts need module/function context, CFG state,
storage layout, counters, and invalidation rules.

The core shape can be:

```rust
pub fn infer_expr(
    ctx: &FactContext<'_>,
    facts: &mut FactStore,
    expr: &InstrCodegen,
) -> Result<ValueFacts, FactError> {
    match expr {
        InstrCodegen::BinOp(op) => infer_binop(ctx, facts, op),
        InstrCodegen::Load(op) => infer_load(ctx, facts, op),
        _ => Ok(ValueFacts::PyObj(PyObjFacts::unknown())),
    }
}
```

For a `BinOp`, inference can still have the local shape:

```rust
let lhs_facts = infer_expr(ctx, facts, &op.left)?;
let rhs_facts = infer_expr(ctx, facts, &op.right)?;
```

But branch narrowing does not fit as only "the test instruction has
facts". For example, `if x is None:` needs different environment facts
on true and false successor edges. That should be modeled as a
function-level CFG analysis with:

- facts at block entry;
- facts after each expression/instruction;
- transfer functions for branch edges;
- widening or conservative fallback for loops.

## Codegen Integration

Run inference once before codegen and pass a read-only `FactStore` into
codegen. Codegen should ask for facts by `InstrKey`; it should not
recursively infer facts on demand as the long-term design.

Use the facts opportunistically:

- Proven static fact: emit specialized code without a guard.
- Profiled or assumed fact: emit guard plus fallback.
- Unknown: emit the generic path.

Unknown calls and arbitrary Python operations should conservatively
invalidate facts about globals, attributes, class dicts, and other
mutable runtime state. For a first version, preserve only facts about
immediate expression results and exact built-in constants across such
operations.

## Suggested Order

1. Add ID-presence and uniqueness validation for codegen-shaped modules.
2. Add `soac-blockpy/src/passes/type_facts.rs` or `facts.rs`.
3. Implement local monotonic facts for literals, `None`, `True`,
   `False`, runtime constants, simple `is None` / `is not None`, and
   exact-int facts from operator profiling.
4. Add branch transfer/narrowing for `IfTerm.test`.
5. Thread a read-only `FactStore` into JIT codegen and use it for the
   first guarded specializations.
