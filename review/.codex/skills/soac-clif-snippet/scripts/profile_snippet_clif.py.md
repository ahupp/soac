# .codex/skills/soac-clif-snippet/scripts/profile_snippet_clif.py

## File Responsibilities

CLI helper for the `soac-clif-snippet` skill. It writes or reads a Python snippet, profiles an example workload under SOAC, inspects the specialization counters, renders specialized CLIF/VCode for a selected function, and writes an annotation context file for later human or Codex review.

## Datatypes

- Module constants: `REPO_ROOT`, `DEFAULT_ARTIFACT_ROOT`, and `DEFAULT_PROFILE_MODULE_NAME` define repository-local defaults for generated artifacts and temporary module names.
- No classes or structured datatypes are defined.

## Functions

- `main`: orchestrates argument parsing, source capture, profiling, function-id lookup, specialization inspection, CLIF rendering, and annotation-context output.
- `parse_args`: defines CLI flags for source input, workload expression, target function, artifact location, and inspector rendering options.
- `read_source`: chooses inline snippet source or reads the source file argument.
- `choose_artifact_dir`: selects the output directory, either from `--artifact-dir` or a sanitized target-name directory under the default skill artifact root.
- `infer_workload_target`: extracts the called function name from a simple workload expression.
- `sanitize_name`: converts a name into a filesystem-safe artifact component.
- `profile_workload`: executes the workload with `SOAC_OPT_MODE=profile` and a repo-local `SOAC_WORK_DIR`, then returns the profile dump path.
- `load_soac_module_from_path`: imports a Python source file as a module with `SoacLoader`.
- `lookup_function_id`: imports the snippet module, finds the named function, and reads its SOAC `function_id`.
- `inspect_specializations`: invokes `inspect_counters` on a profile dump.
- `render_specialized_clif`: invokes `render_jit_clif` with the rendered function id and profile dump.
- `run_inspector`: shared subprocess wrapper for `cargo run -p soac-inspector --bin ...`, including stdout capture and error reporting.
- `annotation_context`: builds the Markdown context that combines source, workload, specialization summary, and rendered CLIF.
- `safe_repr`: defensive `repr` helper for annotation metadata.
- `patched_env`: context manager that temporarily applies environment variables and restores prior values.

## Context Read

- `.codex/skills/soac-clif-snippet/SKILL.md`
- `soac_py/src/soac/import_hook.py`
- `soac-inspector/src/bin/inspect_counters.rs`
- `soac-inspector/src/bin/render_jit_clif.rs`

