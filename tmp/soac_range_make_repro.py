from pathlib import Path
import faulthandler
import tempfile
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tests._integration import soac_module


source = """
def make(stop):
    return range(stop)
"""


faulthandler.dump_traceback_later(5, repeat=True)

with tempfile.TemporaryDirectory() as directory:
    with soac_module(Path(directory), "runtime_range_make_repro", source) as module:
        print("loaded", flush=True)
        print(module.make(4), flush=True)
