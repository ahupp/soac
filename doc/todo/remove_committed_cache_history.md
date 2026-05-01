---
title: "Remove committed cache directories from git history"
---

# Remove committed cache directories from git history

Use a fresh mirror clone and rewrite only that clone, leaving the original repo
untouched:

```bash
SRC=/home/adamh/code/soac-beta
DST=/home/adamh/code/soac-beta-scrubbed.git

git clone --mirror --no-local "$SRC" "$DST" &&
git -C "$DST" filter-repo --force \
  --path tmp/ \
  --path soac-module-cache/ \
  --invert-paths
```

This requires `git-filter-repo`. The rewritten history is written to
`$DST`; do not run this in-place on the working repo.
