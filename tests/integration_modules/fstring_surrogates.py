

def run():
    s1 = "X"
    s2 = "Y"
    return f"\ud83d{s1}\udc0d{s2}"


# diet-python: validate

def validate_module(module):
    assert module.run() == "\ud83dX\udc0dY"
