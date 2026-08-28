# module:ordinary_package

value = 1

def read():
    return value

# module:ordinary_package.child
# soac: module(checked_attr=true)

class Checked:
    value: int = 0

# ok

import ordinary_package.child
module.value = 2
assert module.read() == 2
module.value = 3
assert module.read() == 3
assert ordinary_package.child.Checked().value == 0

# raise:TypeError

import ordinary_package.child
ordinary_package.child.Checked().value = "bad"
