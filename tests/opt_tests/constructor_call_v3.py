# soac: opt-plan-mode=v3


class Box:
    def __init__(self, value):
        self.value = value


def make(value):
    return Box(value)


# soac: verify
def _soac_opt_verify():
    for i in range(80):
        box = make(i)
        assert box.value == i


# soac: verify-counters
[
    {
        "type": "v3_plan",
        "function": "make",
        "constructor_calls": 0,
        "emitted_constructor_calls": 0,
    },
]
