VALUE = 0


def set_get_global(value):
    global VALUE
    VALUE = value
    return VALUE


def exercise_global_indexed():
    assert set_get_global(42) == 42


# soac: verify
for _ in range(80):
    exercise_global_indexed()


# soac: verify-counters
[
    {
        "function": "set_get_global",
        "kind": "global_indexed",
        "branch": "hit",
        "min": 2,
    },
    {
        "function": "set_get_global",
        "kind": "global_indexed",
        "branch": "fallback",
        "max": 0,
    },
]
