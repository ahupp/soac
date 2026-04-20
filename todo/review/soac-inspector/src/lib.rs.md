# soac-inspector/src/lib.rs

## File Responsibilities

Shared inspector library and web API. It prepares embedded Python, lowers source to BlockPy/codegen modules, renders AST/IR/JIT payloads, serves the inspector web UI, and exposes HTTP endpoints for pipeline inspection, JIT CLIF rendering, and speedscope profiles.

## Datatypes

- `AppState`: repository root and web asset directory used by the Axum app.
- `InspectPipelineRequest`: JSON request body containing Python source to inspect.
- `JitClifRequest`: JSON request body for source, packed function id, optional profile dump, and render flags.
- `SpeedscopeProfileRequest`: JSON request body for a profile path.
- `JitClifResponse`: rendered CLIF, optional VCode, function metadata, and debug-plan text returned by APIs.
- `JitClifRenderOptions`: public render options for profile dumps and optimization toggles.
- `ApiError`: HTTP/API error wrapper with status code and message.
- `NEXT_WEB_MODULE_ID`, `PYTHON_INIT`: process-global counters/guards for unique synthetic module names and one-time Python initialization.

## Functions

- `ApiError::bad_request`, `ApiError::internal`: constructors for typed API failures.
- `IntoResponse for ApiError::into_response`: serializes API errors as JSON responses.
- `repo_root`: computes the repository root from the Cargo manifest directory.
- `web_dir`: returns the inspector web asset directory.
- `app`: builds the default Axum app.
- `app_with_state`: constructs routes for static assets and JSON APIs.
- `prepare_python`: initializes embedded Python and support paths once.
- `configure_embedded_python_env`: configures Python home/path environment for vendored CPython and repo modules.
- `find_python_build_lib_dir`: locates the vendored CPython build library directory.
- `find_venv_site_packages`: locates the repo venv site-packages directory.
- `ensure_python_support_paths`: inserts repo Python paths into `sys.path`.
- `lower_source_recorded`: lowers Python source while recording pass outputs.
- `inspector_function_payload`: converts one lowered function into JSON metadata.
- `render_inspector_payload`: builds the full pipeline JSON payload from lowering output.
- `lower_source_to_codegen_module`: lowers source into a codegen-shaped `BlockPyModule`.
- `lower_source_to_codegen_module_with_module_id`: same as above but uses an explicit profile module id.
- `profile_module_id_from_env`: reads a profile module id override from environment.
- `next_web_module_name`: allocates a unique synthetic module name for web-inspected snippets.
- `inspect_pipeline_payload`: lowers source and serializes all inspector views.
- `jit_debug_plan`: renders the JIT debug plan for one function.
- `render_jit_clif_for_module`: renders CLIF with default options.
- `module_package_name`: derives a package name from a module name.
- `execute_module_for_runtime_render_state`: executes lowered module setup enough to collect runtime state for rendering.
- `corresponding_runtime_function`: maps a source function to an existing runtime function when rendering `soac.runtime`.
- `render_jit_clif_for_module_with_options`: prepares runtime/profile state, specialization config, and renders CLIF/VCode.
- `render_jit_clif`: lower-level function render implementation for one function.
- `handle_inspect_pipeline`: Axum handler for pipeline inspection.
- `handle_jit_clif`: Axum handler for CLIF rendering.
- `handle_speedscope_profile`: Axum handler for serving speedscope files from allowed paths.
- `parse_packed_function_id`: parses packed function ids from request strings.
- `resolve_speedscope_profile_path`: validates that requested profile paths remain under repo/logs or `/tmp`.

## Context Read

- `soac_blockpy` lowering APIs
- `soac_jit` render/debug/profile APIs
- `soac_py/src/soac/import_hook.py`
- `soac-inspector/src/bin/render_jit_clif.rs`

