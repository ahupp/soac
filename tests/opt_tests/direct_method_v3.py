# soac: opt-plan-mode=v3


class Box:
    def __init__(self, value):
        self.value = value

    def get(self):
        return self.value


def caller(box):
    return box.get()


# soac: verify
box = Box(42)
for _ in range(80):
    assert caller(box) == 42


# soac: verify-counters
[
    {
        "type": "v3_plan",
        "function": "caller",
        "method_calls": {"min": 1},
        "emitted_method_calls": {"min": 1},
    },
]
