# module:bindings
# soac: module(strict_assign=true, checked_attr=false)

answer = 42

class Ordinary:
    value: int = 0

# soac: class(checked_attr=true)
class Checked:
    value: int = 0

# module:writer

def store(instance, value):
    instance.value = value

# ok

import writer
assert writer.store(Ordinary(), "ordinary") is None
Ordinary.value = "class namespace remains ordinary"
assert Ordinary().value == "class namespace remains ordinary"
writer.extra = 1
writer.extra = 2

# raise:soac.strict.StrictMutationError

module.answer = 7

# raise:TypeError

import writer
instance = Checked()
writer.store(instance, "bad")
