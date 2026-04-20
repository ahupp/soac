# Function Identity Types And Serialization

## Problem

`FunctionId` currently carries several meanings:

- process-local packed runtime identity: `(runtime_module_id, local_function_id)`;
- local function identity within a `BlockPyModule`;
- persistent cross-run identity for optimization plans and precompile inputs;
- compact transport for counter dumps and inspector APIs.

Those are different lifetimes. Reusing one packed type makes it easy to store a
process-local id in a persistent artifact, or to treat a persistent target as if
it already names a function in the current `CompileSession`.

## Type split

Use distinct types for the major lifetimes.

```rust
pub struct LocalFunctionId(u32);

pub struct RuntimeModuleId(u32);

// Valid only inside one running process / CompileSession.
pub struct RuntimeFunctionId(u64);

// Valid only inside one serialized artifact after reading its module side table.
pub struct SerializedModuleId(u32);
pub struct SerializedFunctionId(u64);

pub struct ModuleContentId {
    pub module_name: String,
    pub source_hash: u64,
}

pub struct PersistentFunctionId {
    pub module: ModuleContentId,
    pub local: LocalFunctionId,
}
```

`RuntimeFunctionId` is the only type that should expose a packed `u64`
representation of `(RuntimeModuleId, LocalFunctionId)`. Persisted artifacts
should not store `RuntimeFunctionId` except in raw counter rows that are resolved
through a module table before optimization decisions are built.

## Serialized side tables

Do not repeat `{module_name, source_hash, local_function_id}` at every use site.
Persist compact ids plus side tables. The serialized function id can stay
packed, but its module half is a serialized module-table index, not a
process-local runtime module id:

```rust
pub struct SerializedIdentityTables {
    pub modules: Vec<SerializedModuleIdentity>,
    pub debug_names: Vec<SerializedFunctionDebugName>,
}

pub struct SerializedModuleIdentity {
    pub module_name: String,
    pub source_hash: u64,
    pub cache_identity: Option<String>,
}

// high 32 bits = SerializedModuleId, low 32 bits = LocalFunctionId.
pub struct SerializedFunctionId(u64);

pub struct SerializedFunctionDebugName {
    pub function: SerializedFunctionId,
    pub qualname: String,
}
```

Important invariant: `SerializedFunctionId` is artifact-local identity only. Do
not put `qualname` or other display/debug metadata in it. Debug metadata is
looked up through a separate side table keyed by `SerializedFunctionId`.

This keeps the hot/persistent shape compact while making it clear that qualnames
are not part of identity. Duplicate local/generated/nested qualnames are allowed
and should not affect lookup.

## Resolution boundaries

Use explicit resolution steps at artifact boundaries:

1. Counter/profile load:
   - read raw rows containing compact runtime ids;
   - read the module table mapping `RuntimeModuleId -> ModuleContentId`;
   - resolve observed call targets into `PersistentFunctionId`.

2. Optimization decision output:
   - write one module side table for the artifact;
   - decisions refer to `SerializedFunctionId`;
   - optional debug output resolves `SerializedFunctionId` through
     `debug_names`.

3. Runtime apply:
   - resolve `PersistentFunctionId` to `RuntimeFunctionId` through
     `CompileSession` / `SharedModuleState`;
   - unresolved persistent ids mean "do not apply that specialization", not
     "guess by qualname".

4. Offline precompile:
   - resolve `PersistentFunctionId` to the module's loaded `LocalFunctionId`
     and generated object symbol through the precompile module index;
   - symbol names should be built from persistent module identity plus local
     function id, not from process-local packed runtime ids.

## Migration plan

1. Add the newtypes while keeping the existing `FunctionId` storage. Done.
2. Rename the existing packed accessors to expose their lifetime:
   - `module_id()` -> `runtime_module_id()`;
   - `function_id()` -> `local_function_id()`;
   - `packed()` -> `to_packed_runtime_u64()`.
   Done.
3. Introduce `PersistentFunctionId` and use it for optimization-plan targets.
   Done for runtime/precompile resolution; serialized targets now resolve to
   `PersistentFunctionId` before they are mapped to process-local ids.
4. Change `FunctionOptimizationPlan` to store `LocalFunctionId`; the enclosing
   plan already identifies the module. Done as an intermediate step; `.opt`
   artifacts now store a compact `SerializedFunctionId` so future cross-module
   plan sections can use the same representation.
5. Add serialized module/debug side tables for `.opt` and counter-derived
   decision artifacts; use packed `SerializedFunctionId` values whose module
   half indexes the serialized module table. Done for `.opt` decisions.
6. Teach `ProfileEvidenceStore` to resolve raw counter ids once at load time.
   Done for optimization-decision input: counter-derived call targets are now
   stored as `PersistentFunctionId` values before `.opt` decisions are built.
7. Change precompile indexing and symbol generation to accept persistent ids.
   Done for shared-library direct-function symbol generation: object symbol
   scopes are now built from `PersistentFunctionId` rather than accepting a
   process-local `FunctionId`.
8. Finally rename the old `FunctionId` to `RuntimeFunctionId` once remaining
   call sites are forced through the right conversion boundary.
   Done: runtime identity is now named `RuntimeFunctionId`; persistent
   optimization artifacts use `PersistentFunctionId`/`SerializedFunctionId`
   at their boundaries.
