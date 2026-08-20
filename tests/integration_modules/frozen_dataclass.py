import dataclasses
import importlib
import os
from pathlib import Path
import sys

stdlib_path = (
    Path(__file__).resolve().parents[2]
    / os.environ.get("CPYTHON_SOURCE_DIR", "vendor/cpython")
    / "Lib"
)
sys.path.insert(0, str(stdlib_path))
try:
    dataclasses = importlib.reload(dataclasses)
finally:
    sys.path.remove(str(stdlib_path))


@dataclasses.dataclass(frozen=True)
class Example:
    value: int


# diet-python: validate

def validate_module(module):
    instance = module.Example(value=1)
    assert instance.value == 1
