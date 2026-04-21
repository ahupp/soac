

def run():
    s = ('a\udca7'
         "b")
    return s


# diet-python: validate

def validate_module(module):
    expected = "a\ufffdb" if __dp_integration_soac__ else "a\udca7b"
    assert module.run() == expected
