---
name: soac-selfdoc
description: Update SOAC repository self-documentation by refreshing README.md crate, skill, and CLI inventories, and creating or updating doc/MODULE_LIFECYCLE.md with module dataflow, crate dependency, lowering, optimization, and codegen walkthroughs.
---

# SOAC Selfdoc

Use this when asked to refresh SOAC's own repository documentation: component
inventories in `README.md` and the module lifecycle walkthrough under `doc/`.

The output is documentation, but it must be grounded in the current checkout.
Do not rely on stale README text when source files disagree.

## Required Outputs

1. Update `README.md` with:
   - Rust crates, split into `Important Crates` and `Utility / Helper Crates`.
   - Codex skills under `.codex/skills/`, each with a short purpose summary.
   - CLI tools, including `soac_inspector` and Cargo bin targets, each with a
     short purpose summary.

2. Create or update `doc/MODULE_LIFECYCLE.md` with:
   - A high-level module dataflow diagram.
   - A crate/module dependency graph.
   - A walkthrough of the major phases: import/runtime entry, lowering,
     pre-optimization cache, profiling/counters, optimization planning,
     optimized BlockPy emission, instrumentation, typed lowering/JIT planning,
     Cranelift codegen, deopt/refcount/exception handling, and runtime
     artifacts.

## Inventory

First run the bundled inventory helper from the repo root:

```bash
python3 .codex/skills/soac-selfdoc/scripts/inventory_selfdoc.py --out work/logs/soac-selfdoc-inventory.md
```

Use the generated inventory as a checklist, not as final prose. It lists the
current crates, local crate dependencies, CLI bin targets, and Codex skills.

If the script fails, fall back to:

```bash
cargo metadata --no-deps --format-version 1
find crates -maxdepth 3 -type f \( -name Cargo.toml -o -path '*/src/bin/*.rs' -o -path '*/src/main.rs' \)
find .codex/skills -maxdepth 2 -type f -name SKILL.md
```

## Source Checks

Inspect these files or nearby modules before writing summaries:

- `crates/soac_pyo3/src/jit_runtime.rs` for CPython/import-hook entry into
  SOAC.
- `crates/soac_driver/src/lib.rs` for runtime orchestration, cache handling,
  and optimized-module preparation.
- `crates/soac_lowering/src/driver.rs` for the lowering pass order.
- `crates/soac_opt/src/` for optimization plans, v3 emission, typed IR, and
  optimization pass ownership.
- `crates/soac_instrument/src/` for profile/verify/apply instrumentation.
- `crates/soac_jit/src/jit/mod.rs` and `crates/soac_jit/src/jit/planning.rs`
  for typed planning, local state, exception dispatch, deopt, and Cranelift
  emission.
- `crates/soac_inspector/src/` and `crates/soac_inspector/src/bin/` for CLI
  tool behavior.
- `.codex/skills/*/SKILL.md` for skill names and descriptions.

## README Guidance

Keep the README inventory concise. Prefer one short paragraph per crate or
tool. Do not turn README into the full architecture document.

Suggested crate split:

- Important crates: `soac_config`, `soac_core`, `soac_lowering`, `soac_opt`,
  `soac_instrument`, `soac_driver`, `soac_jit`, `soac_pyo3`.
- Utility / helper crates: `build_support`, `soac_cpython`, `soac_inspector`,
  `soac_jit_runtime`, `soac_macros`.

Adjust the split if the current checkout clearly changes responsibilities, and
explain the choice briefly in the final summary.

When updating the skill list, include `soac-selfdoc` itself.

## Module Lifecycle Doc Guidance

Use Mermaid fenced code blocks for diagrams when practical. Keep diagrams
high-level enough to survive ordinary refactors.

The lifecycle doc should explain:

- What a "module" means at each stage: Python source/imported module,
  pre-optimization `BlockPyModule<CodegenModuleShape>`, profile counter
  evidence, `ModuleOptimizationPlanV3`, optimized BlockPy artifact, typed module,
  JIT module/runtime state, and generated functions.
- Which crate owns each stage.
- Which artifacts are read or written under `$SOAC_WORK_DIR/modules`.
- Where validation happens and what codegen is expected to emit mechanically.
- Which parts are production runtime paths versus inspection/testing tools.

Avoid exact rendered BlockPy/CLIF snapshots unless the doc is explicitly about
rendering. Prefer stable type/function names and file references.

## Validation

For docs-only updates, do not run the full code gate by default. Do run:

```bash
python3 .codex/skills/soac-selfdoc/scripts/inventory_selfdoc.py --out work/logs/soac-selfdoc-inventory.md
rg -n "MODULE_LIFECYCLE|Important Crates|Utility / Helper Crates|soac-selfdoc" README.md doc/MODULE_LIFECYCLE.md
```

Run `jj diff --stat` and report it. If code changed, follow the normal
`AGENTS.md` code validation rules instead.
