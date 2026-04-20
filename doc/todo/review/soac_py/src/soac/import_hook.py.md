# soac_py/src/soac/import_hook.py

## File Responsibilities

Import hook that replaces eligible source-module loading with SOAC module creation/execution through `_soac_ext`. It supports allow-listing module roots, frozen-module source lookup, and a command-line entrypoint for running a transformed module.

## Datatypes

- `REPO_ROOT`: repository root used to exclude SOAC's own runtime/import-hook sources from transformation.
- `SoacLoader`: `SourceFileLoader` subclass whose `create_module`/`exec_module` delegate to `_soac_ext`.
- `SoacFinder`: `PathFinder` subclass that wraps eligible module specs with `SoacLoader`.

## Functions

- `_enabled_module_roots`: parses `SOAC_MODULE_ENABLED` allow-list entries into resolved path roots.
- `_runtime_bootstrap_in_progress`: detects import of `soac.runtime` before it has completed bootstrapping.
- `_create_module_from_path`: calls `_soac_ext.create_module` for a source path/spec.
- `_module_is_enabled`: checks the allow-list, if present.
- `_should_transform`: filters paths by suffix, existence, repository/runtime exclusions, and allow-list.
- `_source_path_for_frozen_spec`: finds the source file corresponding to a frozen stdlib module spec.
- `_is_cpython_frozen_fixture`: recognizes CPython frozen fixture modules that should be left alone.
- `SoacLoader.create_module`: returns a SOAC extension module for eligible source.
- `SoacLoader.exec_module`: delegates module execution to `_soac_ext.exec_module`.
- `SoacFinder.find_spec`: finds and wraps import specs while avoiding the runtime bootstrap recursion case.
- `SoacFinder.wrap_spec`: resolves source paths for normal/frozen modules and replaces eligible loaders.
- `install`: inserts `SoacFinder` into `sys.meta_path` once.
- `_resolve_target`: resolves a module name or source path to a module spec for the CLI.
- `main`: CLI entrypoint that installs the hook, resolves a target module/path, and executes it as `__main__`.

## Context Read

- `soac-pyo3/src/jit_runtime.rs`
- `soac_py/src/soac/runtime.py`

