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
        "type": "v3_plan",
        "function": "set_get_global",
        "indexed_globals": {"min": 2},
        "emitted_indexed_globals": {"min": 2},
    },
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
