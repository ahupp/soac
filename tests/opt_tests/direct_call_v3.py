def target(left, right):
    return left + right


def caller(fn, left, right):
    return fn(left, right)


def exercise_direct_call():
    assert caller(target, 20, 22) == 42


# soac: verify
for _ in range(80):
    exercise_direct_call()


# soac: verify-counters
[
    {
        "function": "caller",
        "kind": "call_direct_targets",
        "observed_value": "present",
        "min": 1,
    },
    {
        "function": "caller",
        "kind": "call_direct",
        "branch": "hit",
        "min": 1,
    },
]
