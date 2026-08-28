# module:methods
# soac: module(checked_attr=true)

from collections.abc import Callable
from ordinary_helper import ordinary

def function(value=1):
    return value

def deleted():
    return 7

saved_deleted = deleted
del deleted

def rebound(value=1):
    return value

# Explicitly replace this def's literal function type with a callable binding.
rebound: Callable[[object], object] = ordinary

# soac: class(checked_attr=false)
class Ordinary:
    def method(self, value=1):
        return value

class Checked:
    def method(self, value=1):
        return value

# module:ordinary_helper

def ordinary(value=1):
    return value

# module:final_bindings
# soac: module(strict_assign=true)

from collections.abc import Callable
from ordinary_helper import ordinary

def temporary():
    return 5

saved_temporary = temporary
del temporary

def rebound(value=1):
    return value

# Initialization may replace a source def before final bindings are frozen.
rebound: Callable[[object], object] = ordinary

# ok

import final_bindings
import ordinary_helper
assert "deleted" not in vars(module)
assert saved_deleted() == 7
assert rebound is ordinary_helper.ordinary
assert rebound("ordinary rebound call") == "ordinary rebound call"
assert "temporary" not in vars(final_bindings)
assert final_bindings.saved_temporary() == 5
assert final_bindings.rebound is ordinary_helper.ordinary
module.function.__defaults__ = ("changed",)
assert module.function() == "changed"
module.function.__code__ = (lambda value: ("new code", value)).__code__
assert module.function() == ("new code", "changed")
Ordinary.method.__defaults__ = ("ordinary default",)
Ordinary.method.__code__ = (lambda self, value: ("ordinary method", value)).__code__
assert Ordinary().method() == ("ordinary method", "ordinary default")

# ok

import ordinary_helper
module.saved_function = module.function
del module.function
module.function = ordinary_helper.ordinary
assert module.saved_function() == 1
assert module.function("ordinary replacement") == "ordinary replacement"

# raise:soac.strict.StrictMutationError

Checked.method.__code__ = (lambda self, value=1: value).__code__
