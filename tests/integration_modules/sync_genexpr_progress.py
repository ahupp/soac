def progress():
    values = (value for value in range(3))
    return [next(values), next(values), next(values)]


def stops():
    values = (value for value in range(3))
    first = [next(values), next(values), next(values)]
    try:
        next(values)
    except StopIteration:
        return first
    raise AssertionError("generator expression did not stop")


# diet-python: validate

def validate_module(module):
    assert module.progress() == [0, 1, 2]
    assert module.stops() == [0, 1, 2]
