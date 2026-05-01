import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  site: 'http://localhost:8001',
  outDir: './work/docs-site',
  cacheDir: './work/tmp/astro-cache',
  integrations: [
    starlight({
      title: 'SOAC',
      social: [],
      disable404Route: true,
      sidebar: [
        {
          label: 'Core',
          items: [
            { label: 'Module Lifecycle', slug: 'MODULE_LIFECYCLE' },
            { label: 'Optimization', slug: 'OPTIMIZATION' },
            { label: 'Specialization', slug: 'SPECIALIZATION' },
            { label: 'Runtime Functions', slug: 'RUNTIME_FUNCTIONS' },
            { label: 'Performance Log', slug: 'PERF_LOG' },
            { label: 'Overnight Optimization Explain', slug: 'OVERNIGHT_OPT_EXPLAIN' },
            { label: 'Overnight Performance Log', slug: 'OVERNIGHT_PERF_LOG' },
          ],
        },
        {
          label: 'Planning',
          items: [
            { label: 'TODO Index', slug: 'todo/TODO' },
            { label: 'Opt v3', slug: 'todo/opt_v3' },
            { label: 'Opt v2', slug: 'todo/opt_v2' },
            { label: 'Crate Decomposition', slug: 'todo/crate_decomposition' },
            { label: 'Component Coalescing', slug: 'todo/component_coalescing' },
            {
              label: 'Type IDs and Lazy Cross-Module Direct Calls',
              slug: 'todo/type_ids_and_lazy_cross_module_direct_calls',
            },
            { label: 'Result Demand Lowering', slug: 'todo/result_demand_lowering' },
            { label: 'Python Operator Resolution', slug: 'todo/python_operator_resolution' },
            { label: 'Predecoded Interpreter Plan', slug: 'todo/predecoded_interpreter_plan' },
            { label: 'Value Facts for SSA Locals', slug: 'todo/value_facts_ssa_locals' },
            { label: 'Typed Runtime Builtins', slug: 'todo/typed_runtime_builtins' },
            { label: 'Function Identity', slug: 'todo/function_identity' },
            { label: 'Length Specialization', slug: 'todo/len_specialization' },
            { label: 'Code Size', slug: 'todo/codesize' },
            { label: 'JIT Tracebacks from EH Frame', slug: 'todo/jit_tracebacks_from_eh_frame' },
            { label: 'Remove Committed Cache History', slug: 'todo/remove_committed_cache_history' },
          ],
        },
        {
          label: 'Optimization Ideas',
          items: [
            { label: 'Background Compile and Cache', slug: 'todo/opt/background_compile_and_cache' },
            { label: 'Baseline Template JIT', slug: 'todo/opt/baseline_template_jit' },
            { label: 'Branch Predicate Lowering', slug: 'todo/opt/branch_predicate_lowering' },
            { label: 'Compile Runtime Stubs from IR', slug: 'todo/opt/compile_runtime_stubs_from_ir' },
            { label: 'Deopt to BlockPy Executor', slug: 'todo/opt/deopt_to_blockpy_executor' },
            { label: 'Escape Analysis for Python Temps', slug: 'todo/opt/escape_analysis_for_python_temps' },
            { label: 'Fact-Driven Typed Specializations', slug: 'todo/opt/fact_driven_typed_specializations' },
            { label: 'Guard and Refcount Elimination', slug: 'todo/opt/guard_and_refcount_elimination' },
            { label: 'Lazy Per-Function Lowering', slug: 'todo/opt/lazy_per_function_lowering' },
            { label: 'Per-Function Feedback Vectors', slug: 'todo/opt/per_function_feedback_vectors' },
            { label: 'Polymorphic Inline Caches', slug: 'todo/opt/polymorphic_inline_caches' },
            { label: 'Shape Transition Feedback', slug: 'todo/opt/shape_transition_feedback' },
            { label: 'Tiered Specialization Policy', slug: 'todo/opt/tiered_specialization_policy' },
            { label: 'Unboxed Numeric Values', slug: 'todo/opt/unboxed_numeric_values' },
          ],
        },
        {
          label: 'Done-ish',
          items: [
            { label: 'Generator Fixathon', slug: 'todo/doneish/GEN_FIXATHON' },
            { label: 'Intrinsics', slug: 'todo/doneish/Intrinsics' },
            { label: 'Type Bundle', slug: 'todo/doneish/TypeBundle' },
            { label: 'Generators', slug: 'todo/doneish/generators' },
            { label: 'Global JIT Module', slug: 'todo/doneish/global_jitmodule' },
            { label: 'Restore Lazy Annotations', slug: 'todo/doneish/restore_lazy_annotations' },
            { label: 'Scope', slug: 'todo/doneish/scope' },
            { label: 'Traversal', slug: 'todo/doneish/traversal' },
          ],
        },
        {
          label: 'Notes',
          items: [
            { label: 'Yield From', slug: 'lols/yield_from' },
            { label: 'Codex Quotes', slug: 'lols/codex_quotes' },
          ],
        },
      ],
    }),
  ],
});
