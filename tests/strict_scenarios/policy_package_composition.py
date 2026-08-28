# module:rules
# soac: package(strict_assign=true, checked_attr=true)
# soac: module(strict_assign=false)

value = 1

# module:rules.child

value = 2

class Checked:
    value: int = 0

# module:rules.inner
# soac: package(checked_attr=false)

value = 3

# module:rules.inner.child

class Ordinary:
    value: int = 0

# soac: class(checked_attr=true)
class Checked:
    value: int = 0

# module:rules.inner.override
# soac: module(strict_assign=false, checked_attr=true)

value = 4

# soac: class(checked_attr=false)
class Ordinary:
    value: int = 0

class Checked:
    value: int = 0

# ok

import rules.inner.child
import rules.inner.override
module.value = 5
rules.inner.override.value = 6
ordinary = rules.inner.child.Ordinary()
ordinary.value = "ordinary package default"
opted_out = rules.inner.override.Ordinary()
opted_out.value = "local opt-out"
assert ordinary.value != opted_out.value

# raise:soac.strict.StrictMutationError

import rules.child
rules.child.value = 5

# raise:TypeError

import rules.child
rules.child.Checked().value = "checked package default"

# raise:TypeError

import rules.inner.child
rules.inner.child.Checked().value = "explicit class opt-in"

# raise:TypeError

import rules.inner.override
rules.inner.override.Checked().value = "module opt-in"
