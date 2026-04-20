from pathlib import Path
import tempfile
import time
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tests._integration import soac_module


source = """
def identity(value):
    return value
"""


with tempfile.TemporaryDirectory() as directory:
    with soac_module(Path(directory), "runtime_return_arg_sleep_repro", source) as module:
        print("loaded", flush=True)
        time.sleep(1.0)
        print(module.identity(4), flush=True)
