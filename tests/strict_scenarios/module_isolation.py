# module:mod1

events: list[int] = []
answer = 42

def read_answer() -> int:
    return answer

# ok

assert read_answer() == 42
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
