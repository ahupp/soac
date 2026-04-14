# soac_py/src/soac/ipython.py

## File Responsibilities

IPython extension for interactive SOAC optimization exploration. It provides magics to profile a function call, render VCode/CLIF for the profiled function, and ask Codex to annotate CLIF with source/counter context.

## Datatypes

- `MAX_CODEX_CONTEXT_CHARS`: maximum prompt context size sent to Codex.
- `ProfileRecord`: dataclass containing target name, module path/name, artifact directory, and function id for one profiled snippet.
- `SoacOptimizationExplorer`: stateful helper behind the IPython line magics.
- Nested `SoacMagics`: IPython `Magics` class registered by `load_ipython_extension`.

## Functions and Methods

- `ProfileRecord.profile_dump`, `ProfileRecord.vcode_path`, `ProfileRecord.annotated_clif_path`: derived artifact paths.
- `SoacOptimizationExplorer.__init__`: captures shell and artifact root.
- `SoacOptimizationExplorer.profile`: parses a call expression, writes a module, runs it under profile mode, resolves function id, renders VCode, and stores the latest profile record.
- `SoacOptimizationExplorer.vcode`: returns VCode for a target or latest profile.
- `SoacOptimizationExplorer.clif_annotate`: builds a prompt and runs Codex annotation for rendered CLIF.
- `SoacOptimizationExplorer.clif`: returns rendered CLIF for a target or latest profile.
- `SoacOptimizationExplorer._parse_profile_call`: parses a simple function call line into function name, args, and kwargs.
- `SoacOptimizationExplorer._eval_ast_expr`: evaluates literal/name/attribute AST fragments in the IPython user namespace.
- `SoacOptimizationExplorer._lookup_function`: resolves a named function from the shell namespace.
- `SoacOptimizationExplorer._source_for_function`: gets source text for a Python function.
- `SoacOptimizationExplorer._write_profile_module`: writes a temporary module containing the target function source.
- `SoacOptimizationExplorer._lookup_function_id`: imports the generated module through SOAC and reads the function id.
- `SoacOptimizationExplorer._render_vcode`: invokes inspector rendering for VCode.
- `SoacOptimizationExplorer._render_clif`: invokes inspector rendering for CLIF with optional profile/specialization inputs.
- `SoacOptimizationExplorer._counter_summary`: invokes `inspect_counters` for the profile dump.
- `SoacOptimizationExplorer._run_codex_annotator`: runs `codex exec` with the annotation prompt.
- `SoacOptimizationExplorer._run_inspector`: subprocess wrapper for `cargo run -p soac-inspector --bin ...`.
- `_parse_render_target`: parses optional magic target names.
- `_build_clif_annotation_prompt`: builds Codex prompt text from source, counters, CLIF, and VCode.
- `_bounded_context`: truncates long context blocks.
- `_load_soac_module_from_path`: imports a generated source module through `SoacLoader`.
- `_patched_env`: temporarily overrides environment variables.
- `load_ipython_extension`: registers `%soac-profile`, `%soac-vcode`, `%soac-clif`, and `%soac-clif-annotate`.
- `SoacMagics.soac_profile`, `soac_vcode`, `soac_clif`, `soac_clif_annotate`: IPython magic wrappers around the explorer methods.

## Context Read

- `.codex/skills/soac-clif-snippet/scripts/profile_snippet_clif.py`
- `soac_py/src/soac/import_hook.py`
- `soac-inspector/src/bin/render_jit_clif.rs`

