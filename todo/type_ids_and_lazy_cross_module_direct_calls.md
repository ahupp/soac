# Type IDs And Lazy Cross-Module Direct Calls

## Goal

Cross-module direct calls should not need the callee module globals at compile
time. Function global semantics already come from the callee `FunctionEnv` at
runtime, so static direct-body compilation should be based on `FunctionId` and
lowered module metadata only.

The remaining compile-time globals dependency is field-index specialization:
profile data currently records owner type names, then apply mode resolves those
names through the function module globals. That is the wrong boundary. The type
being specialized can come from any module, and the generated fast path only
needs the current live `PyTypeObject*` plus a version guard.

## Target Shape

Use a process-local type table in profile/verify output:

```rust
pub struct ProfileTypeId(pub u64);

pub struct TypeKey {
    pub module: String,
    pub qualname: String,
}

pub struct TypeTableEntry {
    pub id: ProfileTypeId,
    pub key: TypeKey,
}

pub struct FieldLayoutRecord {
    pub owner: ProfileTypeId,
    pub key: String,
    pub index: u32,
}
```

`ProfileTypeId` is stable only inside one counter dump. It is a compact handle
for an observed `PyTypeObject*`, not a cross-run identity. `TypeKey` is the
persistent identity used by apply mode to resolve a current live type object.

Generated field fast paths should still guard on the live type pointer and
`tp_version_tag`; the type table is only how apply mode finds that live type.

## Specific Steps

1. Back out module-globals-as-static-module-state.

   Do not store module globals on `SharedModuleState`. Keep `SharedModuleState`
   as lowered/static module metadata plus runtime counters/constants. Runtime
   globals belong to `FunctionEnv` and module runtime context, not the static
   module state.

2. Make field-index specialization optional without globals.

   Keep `module_globals: Option<ObjPtr>` as a temporary optional input only long
   enough to preserve existing same-module behavior. If it is absent, skip
   field-index specialization rather than trying to recover globals from another
   module. This keeps cross-module direct compilation correct while the type
   table lands.

3. Add a process-local type registry for profiling.

   Introduce a profiling registry keyed by raw `PyTypeObject*`:

   ```rust
   struct ProfileTypeRegistry {
       next_id: u64,
       by_type: HashMap<usize, ProfileTypeId>,
       entries: Vec<TypeTableEntry>,
   }
   ```

   The registry should run under the GIL. When a field-layout event observes a
   type, allocate or reuse its `ProfileTypeId`, snapshot `__module__` and
   `__qualname__`, and record a `TypeTableEntry`.

4. Change type key-layout counter rows to use `ProfileTypeId`.

   Replace `CounterDumpKeyLayout.owner: String` for type layouts with a type id
   reference. Keep module-key layouts separate; module dict specialization can
   still use module names because modules, unlike arbitrary receiver types, are
   the owner.

5. Extend the counter dump schema with a type table.

   Persist `Vec<TypeTableEntry>` alongside counter rows and field-layout rows.
   Keep old reader compatibility only if needed for local migration; otherwise
   update tests and scripts to the new schema in the same logical change.

6. Resolve field owner types by `TypeKey`, not function module globals.

   Add an apply-mode resolver:

   ```rust
   fn resolve_type_key(key: &TypeKey) -> Option<*mut ffi::PyTypeObject>
   ```

   Start with `sys.modules[key.module]` plus qualname traversal. Reject
   `"<locals>"`, non-type results, missing modules, and ambiguous/dynamic
   owners. Do not import modules as a side effect in the first version; only
   resolve modules that are already loaded.

7. Revalidate specialization safety after resolving the type.

   For each resolved owner type and field key:

   - require generic get/set attr
   - reject class bindings for the field name
   - assign/read a nonzero `tp_version_tag`
   - use the recorded field index only after the current type layout still
     matches that key/index

   Then emit the existing field fast path guarded by live type pointer and type
   version.

8. Remove the function-module ownership filter.

   Delete the assumption that field layout owners must be under the compiled
   function's module name. The receiver type's `TypeKey` controls resolution;
   the function module is irrelevant to field layout ownership.

9. Split cross-module direct calls from process-JIT batch predeclaration.

   Same-module direct calls can keep using predeclared Cranelift `FuncId`s while
   that is useful for recursion. Cross-module direct calls should use the
   callable value:

   - load or create the callable's `FunctionEnv`
   - ensure `FunctionEnv.direct_code_ptr` is compiled
   - indirect-call that pointer with the same `FunctionEnv`

   This lets `FunctionId`, actual runtime captures/defaults/closure state, and
   direct code pointer travel together at the call boundary.

10. Add an ensure-direct-code helper for `FunctionEnv`.

    The helper should take function metadata, compile session, and the concrete
    `FunctionEnv`. It should compile by `FunctionId` and lowered module metadata,
    then store the direct code pointer into the env. It must handle recursive or
    already-in-progress compilation by returning null/fallback rather than
    recursively compiling forever.

11. Keep static direct-body compilation independent from globals.

    The direct body entry ABI remains:

    ```text
    direct(fn_env, tstate, args...) -> PyObject*
    ```

    Body code loads globals from `fn_env`. Compile-time specialization inputs
    must either be static lowered data, counter dumps, or separately resolved
    runtime identities such as `TypeKey -> PyTypeObject*`.

12. Retire `module_globals` from field specialization.

    After `TypeKey` resolution is in place, remove `module_globals` from
    `compile_cranelift_run_bb_specialized_cached`,
    `ProcessJitEngine::compile_direct_function`, and
    `build_cranelift_run_bb_specialized_function`. Any remaining need for a
    module dict at compile time should be treated as a design bug.

## Validation

- Unit-test the type table:
  - repeated observations of the same `PyTypeObject*` reuse one `ProfileTypeId`
  - different types get different ids
  - persisted field layout rows join through the type table

- Integration-test cross-module fields:
  - module `a` defines a class
  - module `b` reads/stores fields on instances of `a.C`
  - profile/verify records the type layout
  - apply mode emits field-index hits without using `b`'s globals to resolve
    `a.C`

- Integration-test unresolved owners:
  - local classes or `<locals>` qualnames are recorded but skipped in apply mode
  - missing modules or mutated bindings fall back without changing behavior

- Integration-test lazy cross-module direct calls:
  - caller module invokes a transformed function from another module
  - first call compiles or ensures the callee direct code through its
    `FunctionEnv`
  - later calls reuse `FunctionEnv.direct_code_ptr`
  - callee global loads still observe the callee function's globals

## Open Questions

- Whether type resolution should ever import an unloaded module, or only inspect
  `sys.modules`.
- Whether to preserve short-term same-module field specialization through
  `module_globals` until the type table lands, or disable field specialization
  when the resolver is unavailable.
- Whether same-module direct calls should also move to the indirect
  `FunctionEnv.direct_code_ptr` path, or keep predeclared direct `FuncId`s for
  recursive groups until there is a measured reason to simplify them.
