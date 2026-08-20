

def run():
    s = ('a\udca7'
         "b")
    return s


# diet-python: validate

def validate_module(module):
    assert module.run() == "a\udca7b"
