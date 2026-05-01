---
title: "Completed"
---


## Codex TODO Intake

 * Complete migration of ast-to-instr pass
 * Clean up the instruction metadata passing cruft so metadata ownership and propagation are explicit instead of threaded ad hoc through lowering helpers.
 * If caching BlockPy before codegen, recover FunctionNameGen.next_tmp_id from generated `_dp_*` names.
 * Add mutation watchers for `f.__defaults__` and `f.__kwdefaults__` so direct/JIT default slots stay fresh after in-place edits.
 * Simplify class-cell capture by unconditionally treating `super()` as capturing `__class__`.
 * Treat `exec` as a keyword explicitly instead of handling it as a runtime helper name.
 * Review stable identifiers for modules, functions, types, and pyc-to-function mapping.
 * Replace the function-instantiation fallback for `co_freevars`/capture mismatches with a cleaner explicit closure entry/code-object alignment.
 * Remove global state use in `reset_lowering_state`.
 * Audit `panic!` and `unreachable!` usage.
 * Audit remaining uses of the `blockpy`/`block_py` name for consistency after IR crate extraction.
 * Audit all compiler/runtime special names and fill out `doc/SPECIAL_NAMES.md`, including each name's producer,
   consumer contract, collision/visibility story, and whether any prefix checks should become structured IR facts.
 * Revisit effect-only setitem lowering as a general result-demand/runtime-helper design instead of exact-list codegen special cases.
 * Track slow CPython fast-suite cases that pass with longer timeouts, including `test_dataclasses` and `test_bytes -m test_count`.
 * Submit an upstream Cranelift patch to make `cranelift-jit`'s x86 PC-relative
   relocation panic descriptive: use checked target-address arithmetic and report
   relocation kind, target, callsite, delta, base, and addend when a JIT helper
   or runtime symbol is more than 2GiB away.
 * If Python blocks on a queued background function compilation, steal the queued job and run it
   synchronously; if it is already running, wait for it to complete.
 * Implement `doc/todo/typed_local_values.md`: make typed codegen carry explicit
   SOAC value representations through locals so exact-int scalar locals do not
   need PyObject materialization or cleanup-root traffic until a Python boundary.


## Perf-to-investigate

 * Statically linking against libpython.a
 * Try bpftime for userspace counters.
 * Decide how much codegen should depend on known imports versus relying only on specialization feedback for cross-module assumptions.

 * Value tracing (types, escape analysis)
   * If a local is always set, skip unbound checks
   * Make closure cells function constants if they are never written after capture
   * Stack allocate values if they don't escape
   * unbocked ints when possible and convert at border

 * Inlining
   * Is there only one caller?
   * Is there only one implementation?
   * Is it below < size?
   * Does it unlock other optimizations?

 * Minimize refcounting
   * Follow up on the refcount/inline-ownership code-size theme: the compact refcount helper-body
     experiment reduced tiny blocks but grew bytes by trading `sub`/`test` DECREFs for larger
     `lea`/`cmp` sequences and per-site cold dealloc edges. Rework toward machine-pattern-friendly
     DECREF emission or real shared cold dealloc outlining, and avoid storage-layout stack temps for
     inline-only values by carrying them through guard hot/fallback edges as SSA/block params instead
     of `typed_inline_arg` locals.
 * Code size/locality
   * Counters pass on block entry or perf / last-branch, annotate with cold/unlikely
   
 * Use registers for locals
 * Avoid constant exception checking
 * Converte exceptions to jumps
 * Compile-time symbolic execution
 * Known subclasses
   * No overrides to function
 * Type hints enforcement
 * Green threads for async
 * Guard elision
   * No shadowed attributes
   * No externally writable modules
   * Immutable builtins



# Completed


- Collapse the repeated Ruff/Semantic/Core BlockPy alias families into one stage-oriented representation, ideally via associated types on a stage trait or wrapper type.
- Remove the fallback await-lowering path so all awaits use one explicit pass, and make that pass appear as a top-level step in `rewrite_module`.
- Add an evaluation-order-explicit pass that hoists composite subexpressions into temps while preserving left-to-right evaluation, e.g. `a = foo(b(), c)` -> `tmp = b(); a = foo(tmp, c)`.
- Remove local `StmtBody` usage and move back to upstream Ruff structures.
- Implement a BlockPyModuleVisitor, analagous to BlockPyModuleMap.  This will visit everything in order, taking by reference not value.  It should have a &mut self reciever.  Then move all the summarize_ stuff in basic_block/mod.rs to it's own module, and use a BlockPyModuleVisitor to do that generically.
- I don't think flatten_stmt_boxes and flatten_stmt do anything anymore, remove
- merge bound_names into ast_symbol_analysis
- There is pretty-print logic in bb_ir.rs, web_inspector.rs, and block_py/pretty.rs. \ Determine if all those can be merged into a single implementation, possibly with BlockPyModuleVisitor.
- move bb_ir into blockpy_to_bb/mod.rs
- move "block_py" to be a top-level module.
- rename the "basic_block" module to "passes"
- Move `codegen_trace` to be a generic transform over `CfgModule`.
- Remove the “start label” concept and always make the first block the callable entry block.
- Determine if codegen_trace.rs and cfg_trace.rs are doing similar things, and merge if so.
- Simplify should remove literals for true/false/none/ellipsis, replacing them with their _dp_ versions, remove that from codegen_normalize.  Remove those from the expr ast.
- Should we linearize in the BlockPy pass so the whole block structure is uniform?
- Clean up the conversions and related glue in `block_py/mod.rs`.
- Compute `ClosureLayout` in `name_binding`, and keep all closure data semantic before that.
- Add a pass for specific storage decisions, closure slot offsets, and stack offsets.
- Use Ruff for scope analysis and see if it can be computed once and preserved through transform layers.
- Consolidate the bootstrap module in `build_soac_runtime_bootstrap_module` with `runtime.py`.
- Consider moving `LocalEnvEntry` construction into a name-binding-like BlockPy pass so local ownership/storage entries are decided before JIT codegen.
- Revisit Cranelift compile caching by relocating constant values instead of embedding per-run object/counter pointers in CLIF.
