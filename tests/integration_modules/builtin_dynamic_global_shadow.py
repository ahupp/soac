def install_len_and_call(value):
    globals()["len"] = lambda _value: 123
    try:
        return len(value)
    finally:
        globals().pop("len", None)


# diet-python: validate

def validate_module(module):
    assert module.install_len_and_call([1, 2, 3]) == 123
