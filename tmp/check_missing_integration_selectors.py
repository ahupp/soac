import os
import re
import subprocess
import sys

os.environ["REPO_ROOT"] = "/home/adamh/code/soac"
os.environ["VENV_DIR"] = "/home/adamh/code/soac/.venv"
sys.path.insert(0, "scripts")

import run_pytest_parallel as runner


log_path = sys.argv[1]
code, nodeids, _ = runner.collect_test_nodeids(["tests/"])
if code != 0:
    raise SystemExit(code)
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

selectors = [
    selector
    for batch in batches
    if batch.label not in done
    for selector in batch.selectors
]

env = os.environ.copy()
env["LD_LIBRARY_PATH"] = "/home/adamh/code/soac/vendor/cpython"
env["SOAC_CRANELIFT_OPT_LEVEL"] = "none"
for selector in selectors:
    print("RUN", selector, flush=True)
    try:
        result = subprocess.run(
            [
                "/home/adamh/code/soac/.venv/bin/python",
                "-m",
                "pytest",
                "-q",
                selector,
            ],
            cwd="/home/adamh/code/soac",
            env=env,
            capture_output=True,
            text=True,
            timeout=15,
        )
    except subprocess.TimeoutExpired:
        print("TIMEOUT", selector, flush=True)
        raise
    if result.returncode != 0:
        print("FAIL", selector, result.returncode, flush=True)
        print(result.stdout)
        print(result.stderr)
        raise SystemExit(result.returncode)
    print("PASS", selector, flush=True)
