# scripts/build_history_metrics_rollup.py

## File Responsibilities

Builds a static history report combining commit/line-count history with Codex token-usage sessions. It reads JSONL inputs, rolls them up by local day, and optionally renders an HTML report from a template.

## Datatypes

- `DailyTokenTotals`: immutable aggregate for token counts and session count on one local day.
- Module constants: default repository paths, Codex session roots, timezone, and HTML output/template paths.

## Functions

- `parse_args`: defines CLI inputs for commit metrics, token/session source, timezone, JSONL outputs, and HTML report generation.
- `parse_timestamp`: parses ISO timestamps, including `Z` UTC suffixes.
- `local_day`: converts an ISO timestamp to a local date string.
- `load_jsonl`: reads newline-delimited JSON objects from a file.
- `write_jsonl`: writes JSON records as JSONL with parent-directory creation.
- `build_daily_rollup`: groups commit records by local day and aggregates code/test/comment/blank/file and churn metrics.
- `normalize_cwd_prefixes`: expands and normalizes repository path prefixes used to match Codex sessions.
- `session_cwd_matches_prefixes`: tests whether a session cwd belongs to this repository.
- `iter_token_events`: scans Codex session JSONL for events with token usage in matching cwd sessions.
- `collect_daily_tokens`: walks Codex session logs and aggregates input/output tokens by local day.
- `format_number`: comma-formats integer values for the HTML summary.
- `build_summary_replacements`: computes template replacement strings for headline metrics and serialized chart data.
- `render_html_from_template`: applies placeholder replacements to an HTML template.
- `write_static_report`: writes the generated HTML report.
- `main`: coordinates input loading, rollups, optional token collection, output JSONL writes, and HTML rendering.

## Context Read

- `scripts/collect_warloc_history.py`
- `web/history_metrics_template.html`

