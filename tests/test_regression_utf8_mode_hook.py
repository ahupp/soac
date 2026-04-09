import os
import subprocess
import sys
from pathlib import Path


def test_utf8_mode_cmd_line_with_hook():
    repo_root = Path(__file__).resolve().parents[1]
    script = (
        "from soac.import_hook import install; "
        "install(); "
        "import encodings; "
        "print('ok')"
    )
    env = os.environ.copy()
    env["LC_ALL"] = "C"
    pythonpath = env.get("PYTHONPATH", "")
    if pythonpath:
        env["PYTHONPATH"] = f"{repo_root}{os.pathsep}{pythonpath}"
    else:
        env["PYTHONPATH"] = str(repo_root)
    result = subprocess.run(
        [sys.executable, "-X", "utf8=0", "-c", script],
        env=env,
        text=True,
        capture_output=True,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "ok"
