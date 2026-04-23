def list_get_set(items, index, value):
    items[index] = value
    return items[index]


def exercise_exact_list_items():
    items = [1, 2, 3]
    assert list_get_set(items, 1, 42) == 42
    assert items == [1, 42, 3]


# soac: verify
for _ in range(80):
    exercise_exact_list_items()


# soac: verify-counters
[
    {
        "type": "v3_plan",
        "function": "list_get_set",
        "exact_list_items": {"min": 2},
        "emitted_exact_list_items": {"min": 2},
    },
    {
        "function": "list_get_set",
        "kind": "getitem_specialized",
        "branch": "hit",
        "min": 1,
    },
    {
        "function": "list_get_set",
        "kind": "setitem_specialized",
        "branch": "hit",
        "min": 1,
    },
    {
        "function": "list_get_set",
        "kind": "getitem_specialized",
        "branch": "fallback",
        "max": 0,
    },
    {
        "function": "list_get_set",
        "kind": "setitem_specialized",
        "branch": "fallback",
        "max": 0,
    },
]
