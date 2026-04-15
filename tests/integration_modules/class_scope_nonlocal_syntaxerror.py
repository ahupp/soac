
def nonlocal_in_class_body_error():
    try:
        exec("class Bad:\n    nonlocal x\n", globals())
    except SyntaxError as exc:
        return exc.msg
    except NotImplementedError as exc:
        return exc
    return None


result = nonlocal_in_class_body_error()

# diet-python: validate

def validate_module(module):

    if __dp_integration_soac__:
        assert isinstance(module.result, NotImplementedError)
        assert "frame-sensitive globals/locals/eval/exec" in str(module.result)
        return

    assert module.result is not None
    assert module.result
