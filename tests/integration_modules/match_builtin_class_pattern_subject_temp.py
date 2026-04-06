match "aa":
    case str(slot):
        MATCHED = slot
    case _:
        MATCHED = None

# diet-python: validate

def validate_module(module):
    assert module.MATCHED == "aa"
