# scripts/collect_warloc_history.py

## File Responsibilities

Collects per-revision source-size metrics using `jj`, `git`, and `warloc`. It can scan a revset, build temporary jj workspaces at each commit, count code/test/runtime lines, and emit a JSONL history suitable for rollups.

## Datatypes

- `CommitMetadata`: immutable commit/change metadata from jj or git history.
- Module constants: repository roots, default revsets, vendor filters, tracked metric keys, and regexes for parsing `jj diff --stat`.

## Functions

- `parse_args`: defines output path, revset, and default-current-line selection options.
- `run`: subprocess helper with stdout/stderr capture and consistent error reporting.
- `jj_cmd`: builds a `jj` command, optionally ignoring the working copy.
- `commit_metadata_from_payload`: converts jj JSON output into `CommitMetadata`.
- `list_jj_commits`: lists commits for a jj revset as JSON.
- `commit_metadata_for_revision`: resolves one revision to commit metadata.
- `current_line_git_base_commit`: finds the git-backed base for the current jj line.
- `git_non_vendor_history`: lists non-vendor git commits reachable from a head revision.
- `commit_touches_non_vendor_paths`: checks whether a commit changes non-vendor files.
- `filter_commits_to_non_vendor_changes`: removes commits that only touch vendor paths.
- `list_default_current_line_commits`: builds the default current-line history window.
- `list_commits`: returns either default current-line commits or an explicit revset.
- `create_workspace`: creates a temporary jj workspace for historical checkout inspection.
- `forget_workspace`: removes a temporary workspace from jj metadata.
- `update_stale_workspace`: refreshes a temporary workspace if jj marks it stale.
- `restore_workspace_from_commit`: updates the temp workspace to a specific commit.
- `is_vendor_path`: tests whether a path is under `vendor/`.
- `warloc_total_from_by_file_jsonl`: parses `warloc --by-file --jsonl` output into aggregate counts.
- `run_warloc`: runs `warloc` on the temporary workspace, excluding vendor files.
- `count_lines`: counts lines in one file, returning zero for missing files.
- `count_python_lines_under`: counts Python lines below a root directory.
- `parse_lines_changed_from_stat`: extracts total changed lines from `jj diff --stat`.
- `lines_changed_for_commit`: calculates changed lines for one commit.
- `collect_commit_record`: combines commit metadata, warloc totals, test/runtime line counts, and churn metrics.
- `collect_history`: iterates commits, restores the temp workspace, collects records, and writes JSONL.
- `main`: CLI entrypoint with cleanup for temporary jj workspaces.

## Context Read

- `scripts/build_history_metrics_rollup.py`
- `AGENTS.md` jj workspace conventions

