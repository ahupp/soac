def run(flag):
    if flag:
        x = 1
    if flag:
        return x
    return 0


RESULT_TRUE = run(True)
RESULT_FALSE = run(False)


# diet-python: validate

def validate_module(module):
    assert module.RESULT_TRUE == 1
    assert module.RESULT_FALSE == 0
