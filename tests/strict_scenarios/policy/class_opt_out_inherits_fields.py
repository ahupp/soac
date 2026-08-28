# module:hierarchy
# soac: module(checked_attr=true)

class Base:
    value: int = 0

# soac: class(checked_attr=false)
class Child(Base):
    own: int = 0

# ok

instance = Child()
instance.own = "local field is ordinary"
instance.value = 2
assert instance.value == 2
assert instance.own == "local field is ordinary"
Child.own = "ordinary class namespace"

# raise:TypeError

instance = Child()
instance.value = "base constraint survives opt-out"

# raise:TypeError

instance = Child()
instance.value = 1
dictionary = vars(instance)
del instance
dictionary["value"] = "escaped inherited storage stays checked"
