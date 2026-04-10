from __future__ import annotations


def has_exception_name():
    try:
        1 / 0
    except Exception as e:
        pass
    return "e" in locals()

# diet-python: validate

def validate_module(module):

    if __dp_integration_soac__:
        try:
            module.has_exception_name()
        except NotImplementedError:
            pass
        else:
            raise AssertionError("expected locals() to be unsupported")
    else:
        assert module.has_exception_name() is False
