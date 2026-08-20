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
    # Source identity is profile evidence, not permission for an unchecked body.
    {
        "function": "caller",
        "kind": "call_hot_targets",
        "observed_value": "present",
        "min": 1,
    },
    {
        "function": "caller",
        "kind": "call_hot_targets",
        "observed_value": 0,
        "equals": 0,
    },
    {
        "function": "caller",
        "kind": "call_direct_targets",
        "observed_value": "present",
        "equals": 0,
    },
    {
        "function": "caller",
        "kind": "call_direct",
        "branch": "hit",
        "equals": 0,
    },
    {
        "function": "caller",
        "kind": "call_direct",
        "branch": "fallback",
        "equals": 0,
    },
]
