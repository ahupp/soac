# module:mod1
# soac: module(strict_assign=true)

events: list[int] = []
answer = 42

def read_answer() -> int:
    return answer

# ok

assert read_answer() == 42
assert module.__annotations__["events"] == list[int]
events.append(1)
import builtins
builtins._strict_scenario_mutation = True
validator_local = True

# raise:TypeError

module.answer = 7

# ok

import builtins
assert events == []
assert not hasattr(builtins, "_strict_scenario_mutation")
assert "validator_local" not in globals()
assert read_answer() == 42
