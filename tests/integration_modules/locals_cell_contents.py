def outer():
    x = 2
    def inner(y):
        z = x + y
        return locals()
    return inner(4)

# diet-python: validate

def validate_module(module):

    if __dp_integration_soac__:
        try:
            module.outer()
        except NotImplementedError:
            pass
        else:
            raise AssertionError("expected locals() to be unsupported")
    else:
        result = module.outer()
        assert result == {"x": 2, "y": 4, "z": 6}
