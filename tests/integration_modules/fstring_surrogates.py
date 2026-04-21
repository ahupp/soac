

def run():
    s1 = "X"
    s2 = "Y"
    return f"\ud83d{s1}\udc0d{s2}"


# diet-python: validate

def validate_module(module):
    expected = "\ufffdX\ufffdY" if __dp_integration_soac__ else "\ud83dX\udc0dY"
    assert module.run() == expected
