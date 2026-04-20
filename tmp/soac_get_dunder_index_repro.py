from pathlib import Path
import faulthandler
import tempfile
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tests._integration import soac_module


source = """
def get_dunder(value):
    return value.__index__
"""


faulthandler.dump_traceback_later(5, repeat=True)

with tempfile.TemporaryDirectory() as directory:
    with soac_module(Path(directory), "runtime_get_dunder_index_repro", source) as module:
        print("loaded", flush=True)
        print(module.get_dunder(4), flush=True)
