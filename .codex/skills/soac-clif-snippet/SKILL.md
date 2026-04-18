---
name: soac-clif-snippet
description: Profile a small Python code snippet with an example workload, render specialized SOAC CLIF, annotate the CLIF inline, and answer follow-up questions about the generated blocks, guards, counters, and optimization decisions.
---

# SOAC CLIF Snippet

Use this when the user gives Python source plus an example workload and asks to
annotate or explain the generated SOAC CLIF.

This workflow executes the workload. Treat the source as code the user intends
to run locally.

## Workflow

1. Run the bundled collector from the repo root, passing the source on stdin:

```bash
just py .codex/skills/soac-clif-snippet/scripts/profile_snippet_clif.py \
  --workload '"add(1, 1)"' <<'PY'
def add(a, b):
    return a + b
PY
```

`just py` forwards arguments through its shell recipe, so keep the extra inner
quotes around workloads containing parentheses or spaces.

If the workload call does not directly identify the function to inspect, pass
`--function <qualname>`.

2. Read the printed `annotation_context.md` path. It contains:

- original Python source
- workload and result repr
- module/function metadata
- decoded specialization counters
- specialized CLIF

3. Produce annotated CLIF in the answer:

- Return CLIF with comments, not a prose-only summary.
- Preserve CLIF order and original instructions as much as practical.
- Add a short comment before each block explaining what the block does.
- Add inline comments for guards, fast paths, slow paths, exception edges,
  direct indexed dict access, refcount cleanup, helper calls, and fallback
  calls when visible.
- Tie comments to Python source and counter evidence where possible.
- Mark uncertain block purposes as inferred.

4. For follow-up questions in the same conversation, reuse the artifact
directory from the prior run rather than reprofiling unless the source,
workload, counters, or requested function changed.

## Useful Artifacts

The script writes one directory under `work/logs/soac-clif-snippets/`:

- `source.py`
- `workload.txt`
- `result_repr.txt`
- `counters/profile.bin`
- `specializations.txt`
- `specialized.clif`
- `annotation_context.md`
- `metadata.json`
