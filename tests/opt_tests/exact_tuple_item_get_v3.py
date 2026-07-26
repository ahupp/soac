class OverrideTuple(tuple):
    def __getitem__(self, index):
        return super().__getitem__(index) + 100


class CustomIndex:
    def __index__(self):
        return 1


def tuple_get(items, index):
    return items[index]


def tuple_set(items, index, value):
    items[index] = value


def exercise_exact_tuple_items():
    items = (10, 20, 30)

    assert tuple_get(items, 1) == 20
    assert tuple_get(items, -1) == 30
    assert tuple_get(OverrideTuple(items), 1) == 120
    assert tuple_get(items, CustomIndex()) == 20
    assert tuple_get(items, True) == 20

    for index in (3, -4):
        try:
            tuple_get(items, index)
        except IndexError:
            pass
        else:
            raise AssertionError("out-of-bounds tuple indexes must raise IndexError")

    try:
        tuple_set(items, 1, 99)
    except TypeError:
        pass
    else:
        raise AssertionError("tuple item stores must remain generic and raise TypeError")


# soac: verify
for _ in range(80):
    exercise_exact_tuple_items()


# soac: verify-counters
[
    {
        "function": "tuple_get",
        "kind": "getitem_specialized",
        "branch": "hit",
        "min": 2,
    },
    {
        "function": "tuple_get",
        "kind": "getitem_specialized",
        "branch": "fallback",
        "min": 5,
    },
]
