# module:package
# soac: package(strict_assign=true, checked_attr=true)

"""The module docstring survives strict opt-in."""; from .child import values

# module:package.child

from __future__ import annotations; initial = 1

def values():
    yield initial

# ok

assert list(values()) == [1]
assert module.__doc__ == "The module docstring survives strict opt-in."

# ok

import package.child
assert package.child.initial == 1
