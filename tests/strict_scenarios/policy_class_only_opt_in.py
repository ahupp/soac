# module:classes

answer = 1

def value(arg=1):
    return arg

class Ordinary:
    field: int = 0

# soac: class(checked_attr=true)
class Checked:
    field: int = 0

    def method(self, value=1):
        return value

# ok

module.answer = 2
module.answer = 3
module.value.__defaults__ = ("ordinary call",)
assert module.value() == "ordinary call"
ordinary = Ordinary()
ordinary.field = "unchecked"
assert ordinary.field == "unchecked"

# raise:TypeError

instance = Checked()
instance.field = "bad"

# raise:soac.strict.StrictMutationError

Checked.method.__defaults__ = ("cannot change a protected method",)
