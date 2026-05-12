def make_values(limit=3):
    for value in range(limit):
        yield value


def collect_default():
    return list(make_values())


def collect_explicit():
    return list(make_values(2))


# diet-python: validate

def validate_module(module):
    assert module.collect_default() == [0, 1, 2]
    assert module.collect_explicit() == [0, 1]
