import os
import re
import sys

os.environ["REPO_ROOT"] = "/home/adamh/code/soac"
os.environ["VENV_DIR"] = "/home/adamh/code/soac/.venv"
sys.path.insert(0, "scripts")

import run_pytest_parallel as runner


log_path = sys.argv[1]
code, nodeids, _ = runner.collect_test_nodeids(["tests/"])
batches = runner.make_nodeid_batches(
    nodeids, runner.parse_jobs("auto", max(1, len(nodeids)))
)
done = set()
pattern = re.compile(
    r"^\[diet-python test-all\]\[pytest\] \[(?:PASS|FAIL)\] (.*) \([0-9.]+s\)$"
)
with open(log_path, encoding="utf-8", errors="replace") as handle:
    for line in handle:
        match = pattern.match(line.rstrip())
        if match:
            done.add(match.group(1))
missing = [batch.label for batch in batches if batch.label not in done]
print(
    "collect",
    code,
    "nodeids",
    len(nodeids),
    "batches",
    len(batches),
    "done",
    len(done),
    "missing",
    len(missing),
)
for batch in batches:
    if batch.label in done:
        continue
    print(batch.label)
    for selector in batch.selectors:
        print("  ", selector)
