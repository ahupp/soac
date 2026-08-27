# module:mod1

class A:
    foo: int = 0

# ok

instance = A()
instance.foo = 1
assert instance.foo == 1

# raise:TypeError

A().foo = "str"

# ok

assert A().foo == 0
