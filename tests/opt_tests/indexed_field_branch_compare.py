class Record:
    def __init__(self, left=0, right=0):
        self.left = left
        self.right = right


def branch_fields():
    record = Record(10, 20)
    if record.left < record.right:
        return 1
    return 0


def exercise_branch_fields():
    assert branch_fields() == 1


# soac: verify
for _ in range(80):
    exercise_branch_fields()


# soac: verify-counters
[
    {
        "function": "branch_fields",
        "kind": "field_access",
        "branch": "indexed_hit",
        "min": 2,
    },
    {
        "function": "branch_fields",
        "kind": "field_access",
        "branch": "indexed_fallback",
        "max": 0,
    },
]
