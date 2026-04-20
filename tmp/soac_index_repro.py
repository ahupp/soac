from pathlib import Path
import faulthandler
import tempfile
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tests._integration import soac_module


source = """
from soac.runtime import _index

def call_index(value):
    return _index(value)
"""


faulthandler.dump_traceback_later(5, repeat=True)

with tempfile.TemporaryDirectory() as directory:
    with soac_module(Path(directory), "runtime_index_repro", source) as module:
        print("loaded", flush=True)
        print(module.call_index(4), flush=True)
