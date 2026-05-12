def inner(limit):
    for value in range(limit):
        yield value


def outer(limit):
    for value in inner(limit):
        yield value + 1


def collect():
    return list(outer(4))


def tupled(limit):
    yield tuple(value for value in range(limit))


def collect_tupled():
    return list(tupled(3))


def single(limit):
    yield limit


def collect_single():
    return list(single(3))


def peek_genexpr_progress():
    values = (value for value in range(3))
    return [next(values), next(values), next(values)]


# diet-python: validate

def validate_module(module):
    assert module.collect() == [1, 2, 3, 4]
    assert module.collect_tupled() == [(0, 1, 2)]
    assert module.peek_genexpr_progress() == [0, 1, 2]
