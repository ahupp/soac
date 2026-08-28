# module:records
# soac: module(checked_attr=true)

from dataclasses import dataclass, field
from typing import Any

@dataclass
class Record:
    value: int = 0
    label: str = field(default_factory=str)

@dataclass(slots=True)
class FlexibleRecord:
    required: Any
    optional: Any = None
    count: int = 0

# ok

record = Record(1)
assert record.value == 1
assert record.label == ""
record.value = 2
assert record.value == 2

# raise:TypeError

Record("bad")

# raise:TypeError

Record().label = None

# ok

import ctypes
owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
sealed = ctypes.pythonapi.PyType_IsSoacSealed
sealed.argtypes = [ctypes.py_object]
sealed.restype = ctypes.c_int
assert owner(FlexibleRecord) and sealed(FlexibleRecord)
payload = object()
record = FlexibleRecord(payload)
assert record.required is payload and record.optional is None
record.required = "an Any field stays unrestricted"
record.optional = payload
assert record.required == "an Any field stays unrestricted"
assert record.optional is payload and record.count == 0

# raise:TypeError

record = FlexibleRecord(None)
record.count = "the independent int field still checks writes"
