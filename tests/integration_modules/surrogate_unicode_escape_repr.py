def repr_value():
    char = "\uDCBA"
    return repr(char)


def ascii_value():
    char = "\uDCBA"
    return ascii(char)

# diet-python: validate

def validate_module(module):
    expected_repr = "'\ufffd'" if __dp_integration_soac__ else "'\\udcba'"
    expected_ascii = "'\\ufffd'" if __dp_integration_soac__ else "'\\udcba'"
    assert module.repr_value() == expected_repr
    assert module.ascii_value() == expected_ascii
