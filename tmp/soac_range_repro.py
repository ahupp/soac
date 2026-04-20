from pathlib import Path
import tempfile
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tests._integration import soac_module


source = """
def make(stop):
    return range(stop)

def fields(stop):
    r = range(stop)
    return r.start, r.stop, r.step

def make_then_const(stop):
    r = range(stop)
    return 123

def field_start(stop):
    r = range(stop)
    return r.start

def iter_method(stop):
    r = range(stop)
    return r.__iter__()

def iter_builtin(stop):
    return iter(range(stop))

def iter_state(stop):
    iterator = iter(range(stop))
    return iterator.current, iterator.stop, iterator.step

def first_next(stop):
    iterator = iter(range(stop))
    return next(iterator)

def collect_stop(stop):
    return list(range(stop))
"""


with tempfile.TemporaryDirectory() as directory:
    with soac_module(Path(directory), "runtime_range_repro", source) as module:
        print("loaded", flush=True)
        print("make", module.make(4), flush=True)
        print("make_then_const", module.make_then_const(4), flush=True)
        print("field_start", module.field_start(4), flush=True)
        print("fields", module.fields(4), flush=True)
        print("iter_method", module.iter_method(4), flush=True)
        print("iter_builtin", module.iter_builtin(4), flush=True)
        print("state", module.iter_state(4), flush=True)
        print("first", module.first_next(4), flush=True)
        print("collect", module.collect_stop(4), flush=True)
