# soac: opt-plan-mode=v3


def target(left, right):
    return left + right


def caller():
    return target(20, 22)


def exercise_direct_call():
    assert caller() == 42


# soac: verify
for _ in range(80):
    exercise_direct_call()


# soac: verify-counters
[
    {
        "type": "v3_plan",
        "function": "caller",
        "direct_calls": {"min": 1},
        "emitted_direct_calls": {"min": 1},
    },
]
