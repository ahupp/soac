def nested_progress():
    values = (value for value in range(3))
    yield [next(values), next(values), next(values)]


def nested_stops():
    values = (value for value in range(3))
    first = [next(values), next(values), next(values)]
    try:
        next(values)
    except StopIteration:
        yield first
        return
    raise AssertionError("nested generator expression did not stop")


def collect_progress():
    return list(nested_progress())


def collect_stops():
    return list(nested_stops())


def nested_progress_second_step():
    values = nested_progress()
    first = next(values)
    try:
        second = next(values)
    except StopIteration:
        return first, "stopped"
    return first, second


def nested_progress_first_step():
    return next(nested_progress())


# diet-python: validate

def validate_module(module):
    assert module.nested_progress_first_step() == [0, 1, 2]
    assert module.nested_progress_second_step() == ([0, 1, 2], "stopped")
    assert module.collect_progress() == [[0, 1, 2]]
    assert module.collect_stops() == [[0, 1, 2]]
