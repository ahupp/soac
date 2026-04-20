from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path


collect = Path("logs/integration_collect.txt").read_text()
names = re.findall(r"<Function ([^>]+)>", collect)
selectors = [f"tests/test_integration_cases.py::{name}" for name in names]
selectors = [selector for selector in selectors if "[soac-" in selector or "[entry-" in selector]

chunk_size = int(sys.argv[1]) if len(sys.argv) > 1 else 20
timeout = int(sys.argv[2]) if len(sys.argv) > 2 else 45

for index in range(0, len(selectors), chunk_size):
    chunk = selectors[index : index + chunk_size]
    label = f"{index // chunk_size + 1}/{(len(selectors) + chunk_size - 1) // chunk_size}"
    print(f"RUN {label} {chunk[0]} .. {chunk[-1]}", flush=True)
    command = ["just", "pytest-fast", "-q", *chunk]
    try:
        result = subprocess.run(
            command,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as err:
        print(f"TIMEOUT {label}", flush=True)
        print("\n".join(chunk), flush=True)
        if err.stdout:
            print(err.stdout, flush=True)
        raise SystemExit(124)
    if result.returncode != 0:
        print(f"FAIL {label} rc={result.returncode}", flush=True)
        print(result.stdout, flush=True)
        raise SystemExit(result.returncode)
    print(f"PASS {label}", flush=True)
