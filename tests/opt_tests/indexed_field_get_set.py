class Record:
    def __init__(self, left=0, right=0):
        self.left = left
        self.right = right


def read_fields():
    record = Record(10, 20)
    return record.left + record.right


def write_fields():
    record = Record(1, 2)
    record.left = 30
    record.right = record.left + 12
    return record.left + record.right


def exercise_indexed_fields():
    assert read_fields() == 30
    assert write_fields() == 72


# soac: verify
for _ in range(80):
    exercise_indexed_fields()


# soac: verify-counters
[
    {
        "function": "read_fields",
        "kind": "field_indexed_hit",
        "min": 2,
    },
    {
        "function": "write_fields",
        "kind": "field_indexed_hit",
        "min": 3,
    },
    {
        "function": "write_fields",
        "kind": "field_indexed_fallback",
        "max": 0,
    },
]
