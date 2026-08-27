# module:records

from dataclasses import dataclass, field

@dataclass
class Record:
    value: int = 0
    label: str = field(default_factory=str)

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
