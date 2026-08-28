# module:consumer
# soac: module(strict_assign=true)

from models import A

def initial() -> int:
    return A().foo

# module:models
# soac: module(checked_attr=true)

class A:
    foo: int = 0

# ok

import models
assert A is models.A
assert initial() == 0
instance = models.A()
instance.foo = 9
assert instance.foo == 9

# raise:TypeError

A().foo = "str"
