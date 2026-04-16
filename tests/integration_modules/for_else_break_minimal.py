def collect_for_else_break_minimal():
    seen = []
    for outer in range(3):
        for _inner in []:
            seen.append((_inner, outer))
        else:
            break
        seen.append("unreachable")
    return seen


RESULT = collect_for_else_break_minimal()

# diet-python: validate

def validate_module(module):
    assert module.RESULT == []
