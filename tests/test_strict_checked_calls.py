"""Source-owned native calls and ordinary value flow through genuine admission."""

import json
from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project

SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)
from typing import final

EVENTS = []

class Calls:
    def checked(self, value: int) -> int:
        EVENTS.append('body')
        return value

    def forward(self, value: int) -> int:
        return self.checked(value)

    def rebind(self, value: int, replacement) -> int:
        value = replacement
        return self.checked(value)

    def after_callback(self, value: int, callback) -> int:
        callback()
        return self.checked(value)

    def computed_argument(self, value: int, callback) -> int:
        return self.checked(callback(value))

    def with_keyword(self, value: int) -> int:
        return self.checked(value=value)

    def defaulted(self, value: int = 7000) -> int:
        return value

    def without_argument(self) -> int:
        return self.defaulted()

    def broken_return(self, callback) -> int:
        return callback()

    def call_broken_return(self, callback) -> int:
        return self.broken_return(callback)

class Override(Calls):
    def checked(self, value: int) -> int:
        EVENTS.append('override')
        return value + 1

class FinalCalls:
    @final
    def checked(self, value: int) -> int:
        return value + 2

    def forward(self, value: int) -> int:
        return self.checked(value)

def make_calls(offset: int):
    class Local:
        def checked(self, value: int) -> int:
            return value + offset

        def forward(self, value: int) -> int:
            return self.checked(value)
    return Local
"""


@pytest.fixture(scope="module")
def checked_calls(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-checked-calls"),
        {"checked_calls.py": SOURCE},
        modules={"checked_calls": "checked_calls.py"},
    )


@pytest.fixture(scope="module")
def nominal_calls(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-nominal-call-sites"),
        {
            "targets.py": """
# soac: module(strict_assign=true, checked_attr=true)

class Box:
    def __init__(self, value: int):
        self.value = value

    def read(self, extra: int) -> int:
        return self.value + extra

class Derived(Box):
    def read(self, extra: int) -> int:
        return self.value + extra + 1
""",
            "callers.py": """
# soac: module(strict_assign=true, checked_attr=true)
from targets import Box

def earlier(owner: Later, extra: int) -> int:
    return owner.read(extra)

class Before:
    def indirect(self, owner: Later, extra: int) -> int:
        return owner.read(extra)

class Later:
    def read(self, extra: int) -> int:
        return extra + 2

def field(owner: Box):
    return owner.value

def call(owner: Box, extra: int) -> int:
    return owner.read(extra)

def make(offset: int):
    class Local:
        def read(self, extra: int) -> int:
            return offset + extra

    def call_local(owner: Local, extra: int) -> int:
        return owner.read(extra)
    return Local, call_local
""",
        },
        modules={"targets": "targets.py", "callers": "callers.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_free_calls_bind_actual_nominal_class_capabilities(
    nominal_calls, entry_interpreter
):
    training = """
import _testinternalcapi
import callers
import targets

for cls in (targets.Box, targets.Derived):
    owner = cls(1000)
    for unused in range(40):
        assert callers.field(owner) == 1000
        assert callers.call(owner, 2000) == 3000 + int(cls is targets.Derived)
    # Observe the actual receiver storage after its original field calls, so
    # this witness does not materialize a dictionary before measured lookup.
    assert _testinternalcapi.dict_has_indexed_keys(vars(owner)) is False

first, call_first = callers.make(10)
second, call_second = callers.make(20)
assert call_first(first(), 1000) == 1010
assert call_second(second(), 1000) == 1020
assert callers.earlier(callers.Later(), 1000) == 1002
assert callers.Before().indirect(callers.Later(), 1000) == 1002
"""
    profiled = nominal_calls.run(training, opt_mode="profile")
    work = Path(profiled.args[-1]).parent / "soac-work"
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    validation = f"""
EXPECTED = {expected!r}
OPTIMIZED = {int(not entry_interpreter)}
for function in (callers.call, callers.field, callers.earlier, callers.Before.indirect, call_first, call_second):
    assert _soac_ext.strict_function_entry_kind(function) == EXPECTED

for cls in (targets.Box, targets.Derived):
    owner = cls(1000)
    before = _soac_ext.strict_function_call_statistics(cls.read)
    assert callers.call(owner, 2000) == 3000 + int(cls is targets.Derived)
    after = _soac_ext.strict_function_call_statistics(cls.read)
    assert after['direct_body_calls'] - before['direct_body_calls'] == OPTIMIZED

for cls, call in ((first, call_first), (second, call_second)):
    before = _soac_ext.strict_function_call_statistics(cls.read)
    assert call(cls(), 1000) in (1010, 1020)
    after = _soac_ext.strict_function_call_statistics(cls.read)
    assert after['direct_body_calls'] - before['direct_body_calls'] == OPTIMIZED

for invoke in (
    lambda: callers.earlier(callers.Later(), 1000),
    lambda: callers.Before().indirect(callers.Later(), 1000),
):
    before = _soac_ext.strict_function_call_statistics(callers.Later.read)
    assert invoke() == 1002
    after = _soac_ext.strict_function_call_statistics(callers.Later.read)
    assert after['direct_body_calls'] - before['direct_body_calls'] == OPTIMIZED

for call, other_owner, expected in ((call_first, second(), 1020),
                                    (call_second, first(), 1010)):
    assert call(other_owner, 1000) == expected, 'dispatch did not use the actual owner'

events = []
class Ordinary(targets.Box):
    @property
    def read(self):
        events.append('lookup')
        return lambda value: value + 9
assert callers.call(Ordinary(1), 1000) == 1009
assert events == ['lookup']
"""
    nominal_calls.run(
        training + validation,
        opt_mode="verify",
        entry_interpreter=entry_interpreter,
        extra_env={"SOAC_WORK_DIR": str(work)},
    )
    if not entry_interpreter:
        from soac import _soac_ext

        counters = json.loads(
            _soac_ext.inspect_counter_dump_json(str(work / "verify.bin"))
        )
        fields = [
            row
            for record in counters["records"]
            if record["module_name"] == "callers"
            for row in record["rows"]
            if row["function_qualname"] == "field" and row["kind"] == "field_access"
        ]
        # These source classes retain ordinary dictionaries and inline layout;
        # no installed indexed field capability exists for callers.field.
        assert any(row["branches"].get("indexed_fallback", 0) > 0 for row in fields), fields
        assert all(row["branches"].get("indexed_hit", 0) == 0 for row in fields), fields


TRAINING = """
import checked_calls as subject

for cls in (subject.Calls, subject.Override):
    instance = cls()
    extra = int(cls is subject.Override)
    for unused in range(50):
        assert instance.forward(3000) == 3000 + extra
        assert instance.rebind(3000, 4000) == 4000 + extra
        assert instance.after_callback(3000, lambda: None) == 3000 + extra
        assert instance.computed_argument(3000, lambda value: value) == 3000 + extra
        assert instance.with_keyword(3000) == 3000 + extra
        assert instance.without_argument() == 7000
        assert instance.call_broken_return(lambda: 9000) == 9000

assert subject.FinalCalls().forward(3000) == 3002
first, second = subject.make_calls(10), subject.make_calls(20)
left, right = first(), second()
for unused in range(50):
    assert left.forward(3000) == 3010 and right.forward(3000) == 3020
"""


VALIDATION = """
def counts(function):
    result = _soac_ext.strict_function_call_statistics(function)
    assert result is not None
    return result

def assert_dispatch(function, invoke, *, direct):
    before = counts(function)
    result = invoke()
    after = counts(function)
    assert after['direct_body_calls'] - before['direct_body_calls'] == direct, (before, after)
    return result

class IntegerSubclass(int):
    pass

for cls in (subject.Calls, subject.Override):
    instance = cls()
    extra = int(cls is subject.Override)
    checked = cls.checked
    assert assert_dispatch(checked, lambda: instance.forward(3000),
                           direct=OPTIMIZED) == 3000 + extra

    # The actual argument remains live across callbacks without granting
    # callable/module authority or turning its annotation into a predicate.
    effects = []
    assert assert_dispatch(checked, lambda: instance.after_callback(3000, lambda: effects.append('callback')),
                           direct=OPTIMIZED) == 3000 + extra
    assert effects == ['callback']

    assert assert_dispatch(checked, lambda: instance.rebind(3000, 4000),
                           direct=OPTIMIZED) == 4000 + extra
    assert assert_dispatch(checked, lambda: instance.forward(IntegerSubclass(3000)),
                           direct=OPTIMIZED) == 3000 + extra
    assert assert_dispatch(checked, lambda: instance.computed_argument(3000, lambda value: value),
                           direct=OPTIMIZED) == 3000 + extra
    # Keywords and omitted defaults retain normal binding/public entries.
    assert assert_dispatch(checked, lambda: instance.with_keyword(3000),
                           direct=0) == 3000 + extra
    assert assert_dispatch(subject.Calls.defaulted, instance.without_argument,
                           direct=0) == 7000

    for invoke in (
        lambda: instance.rebind(3000, 'wrong'),
        lambda: instance.computed_argument(3000, lambda value: 'wrong'),
        lambda: instance.forward('wrong'),
    ):
        subject.EVENTS.clear()
        if cls is subject.Override:
            try:
                invoke()
            except TypeError:
                pass
            else:
                raise AssertionError('the override lost its original addition error')
            assert subject.EVENTS == ['override']
        else:
            assert invoke() == 'wrong'
            assert subject.EVENTS == ['body']

    original = ValueError('original body error')
    def raising():
        raise original
    try:
        instance.call_broken_return(raising)
    except ValueError as error:
        assert error is original
    else:
        raise AssertionError('body error was replaced')
    assert instance.call_broken_return(lambda: 'wrong') == 'wrong'

# An ordinary subclass/property must execute its own lookup and public call.
events = []
class Ordinary(subject.Calls):
    @property
    def checked(self):
        events.append('lookup')
        return lambda value: ('ordinary', value)
assert Ordinary().forward(3000) == ('ordinary', 3000)
assert events == ['lookup']

for function in (subject.Calls.forward, subject.Calls.checked, subject.Override.checked,
                 subject.Calls.rebind, subject.Calls.after_callback):
    assert _soac_ext.strict_function_entry_kind(function) == ENTRY
snapshot = counts(subject.Calls.checked)
assert set(snapshot) == {'direct_body_calls', 'fixed_body_calls'}
snapshot['direct_body_calls'] = -1
assert counts(subject.Calls.checked)['direct_body_calls'] >= 0
assert _soac_ext.strict_function_call_statistics(lambda: None) is None
"""


FIXED_TARGET_VALIDATION = """
def assert_fixed(function, invoke, expected, fixed):
    before = _soac_ext.strict_function_call_statistics(function)
    assert invoke() == expected
    after = _soac_ext.strict_function_call_statistics(function)
    assert after['fixed_body_calls'] - before['fixed_body_calls'] == fixed, (before, after)

base = subject.Calls()
override = subject.Override()
assert_fixed(subject.Calls.checked, lambda: base.forward(3000), 3000, OPTIMIZED)
assert_fixed(subject.Override.checked, lambda: override.forward(3000), 3001, 0)
assert_fixed(subject.FinalCalls.checked, lambda: subject.FinalCalls().forward(3000), 3002, OPTIMIZED)
# Equal code is safe only with the actual activation's environment. Different
# executions of one lexical class still get different family capabilities.
assert first is not second
assert_fixed(first.checked, lambda: left.forward(3000), 3010, OPTIMIZED)
assert_fixed(second.checked, lambda: right.forward(3000), 3020, OPTIMIZED)
assert_fixed(second.checked, lambda: first.forward(right, 3000), 3020, 0)
assert_fixed(first.checked, lambda: second.forward(left, 3000), 3010, 0)
for function in (subject.Calls.forward, subject.FinalCalls.forward, first.forward, second.forward):
    assert _soac_ext.strict_function_entry_kind(function) == ENTRY

try:
    subject.FinalCalls.checked.__code__ = (lambda self, value: 'bad').__code__
except TypeError:
    pass
else:
    raise AssertionError('a sealed fixed target was replaced')
"""


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_source_owned_body_calls_preserve_dispatch_and_fixed_target_selection(
    checked_calls, tmp_path, entry_interpreter
):
    work = tmp_path / "checked-call-profile"
    entry = "entry_interpreter" if entry_interpreter else "checked_native"
    checked_calls.run(
        TRAINING,
        opt_mode="profile",
        entry_interpreter=entry_interpreter,
        extra_env={"SOAC_WORK_DIR": str(work)},
    )
    for mode in ("none", "apply", "verify"):
        optimized = int(mode != "none" and not entry_interpreter)
        events_path = tmp_path / f"fixed-target-{mode}.jsonl"
        checked_calls.run(
            f"OPTIMIZED = {optimized}\nENTRY = {entry!r}\n"
            + TRAINING
            + VALIDATION
            + FIXED_TARGET_VALIDATION,
            opt_mode=mode,
            entry_interpreter=entry_interpreter,
            extra_env={
                "SOAC_WORK_DIR": str(work),
                "SOAC_LOG": f"soac_jit_codegen=info;json={events_path}",
            },
        )
        if optimized:
            events = [json.loads(line) for line in events_path.read_text().splitlines()]
            emitted = {
                fields["function_qualname"]: fields
                for event in events
                if (fields := event.get("fields", event)).get("event")
                == "soac.strict_method_codegen"
            }
            for name in (
                "Calls.forward",
                "FinalCalls.forward",
                "make_calls.<locals>.Local.forward",
            ):
                assert emitted[name]["checked_fixed_body_site_count"] == 1
                assert emitted[name]["machine_code_size_bytes"] > 0


