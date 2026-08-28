# module:ordinary

from unselected_helper import replacement

value = 1

class Ordinary:
    field: int = 0

def read():
    return value

def removed():
    return 7

saved_removed = removed
del removed

def rebound(value):
    return value

rebound = replacement

# module:unselected_helper

def replacement(value):
    return value

# ok

import unselected_helper
assert "removed" not in vars(module)
assert saved_removed() == 7
assert rebound is unselected_helper.replacement
assert rebound("ordinary call") == "ordinary call"
module.value = 2
assert read() == 2
module.value = 3
assert read() == 3
instance = Ordinary()
instance.field = "no implicit opt-in"
assert instance.field == "no implicit opt-in"
