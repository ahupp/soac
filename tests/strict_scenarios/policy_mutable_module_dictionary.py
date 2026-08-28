# module:mutable
# soac: module(checked_attr=true)

value = 1

class Checked:
    value: int = 0

# ok

namespace = vars(module)
namespace[1] = "integer key"
namespace[("tuple",)] = "tuple key"
assert namespace[1] == "integer key"
assert namespace[("tuple",)] == "tuple key"
namespace["value"] = 2
del namespace["value"]
namespace["value"] = 3
assert module.value == 3

# ok

namespace = vars(module)
try:
    namespace.update([("first", 1), ("malformed",)])
except ValueError:
    pass
else:
    raise AssertionError("malformed update was accepted")
assert namespace["first"] == 1

# ok

namespace = vars(module)
events = []
class Key:
    def __hash__(self):
        events.append("hash")
        namespace["from_hash"] = 7
        return 42
key = Key()
namespace[key] = "stored"
assert events == ["hash"]
assert namespace["from_hash"] == 7

# raise:TypeError

instance = Checked()
namespace = vars(module)
namespace.clear()
assert namespace == {}
instance.value = "clearing module globals does not revoke installed checks"
