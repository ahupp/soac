# module:storage
# soac: module(checked_attr=true)

class A:
    foo: int = 0

# ok

instance = A()
instance.foo = 1
dictionary = vars(instance)
del instance
dictionary["foo"] = 2
assert dictionary["foo"] == 2
try:
    dictionary.update(foo="bad")
except TypeError:
    pass
else:
    raise AssertionError("escaped dictionary lost its field check")
assert dictionary["foo"] == 2

# raise:TypeError

import ctypes
instance = A()
instance.foo = 1
write = ctypes.pythonapi.PyDict_SetItem
write.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.py_object]
write.restype = ctypes.c_int
write(vars(instance), "foo", "bad")
