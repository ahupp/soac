# module:nominal
# soac: module(checked_attr=true)

class Target:
    pass

class Box:
    value: Target

    def __init__(self, value: Target):
        self.value = value

# ok

original = Target()
box = Box(original)
module.Target = str
box.value = original
assert box.value is original
assert module.Target is str

# raise:TypeError

original = Target()
box = Box(original)
module.Target = str
box.value = "the mutable alias does not retarget the field"
