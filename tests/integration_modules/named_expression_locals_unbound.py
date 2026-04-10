def named_expr_locals_state():
    a = 1
    b = [1]
    genexp = (c := i + a for i in b)
    has_c_before = "c" in locals()
    values = list(genexp)
    return has_c_before, values, c

# diet-python: validate

def validate_module(module):
    if __dp_integration_soac__:
        try:
            module.named_expr_locals_state()
        except NotImplementedError:
            pass
        else:
            raise AssertionError("expected locals() to be unsupported")
    else:
        assert module.named_expr_locals_state() == (False, [2], 2)
