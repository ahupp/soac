def run_plain(items):
    out = []
    for x in items:
        out.append(x)
    return out


def run_getitem(items):
    out = []
    for x in items:
        out.append(x[0])
    return out


RESULT_PLAIN = run_plain([1, 2, 3])
RESULT_GETITEM = run_getitem([[1], [2], [3]])


# diet-python: validate

def validate_module(module):
    assert module.RESULT_PLAIN == [1, 2, 3]
    assert module.RESULT_GETITEM == [1, 2, 3]
