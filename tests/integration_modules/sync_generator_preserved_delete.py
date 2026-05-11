def make_gen():
    def gen():
        x = 1
        yield x
        del x
        try:
            return x
        except UnboundLocalError:
            return "deleted"

    return gen()


# diet-python: validate

def validate_module(module):
    gen_obj = module.make_gen()
    assert next(gen_obj) == 1
    try:
        next(gen_obj)
    except StopIteration as exc:
        assert exc.value == "deleted"
    else:
        raise AssertionError("generator should stop after deleted-local check")
