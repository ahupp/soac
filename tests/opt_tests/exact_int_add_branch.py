# soac: opt-plan-mode=v3


def add(a, b):
    c = a + b
    if c > 0:
        return True
    else:
        return False


def exercise_add():
    assert add(1, 1) is True


# soac: verify
for _ in range(80):
    exercise_add()


# soac: verify-counters
[
    {
        "type": "v3_plan",
        "function": "add",
        "regions": {"min": 2},
        "emitted_regions": {"min": 2},
        "scalar_threads": {"min": 1},
    },
]
