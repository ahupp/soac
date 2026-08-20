from __future__ import annotations

import ast
import json
from pathlib import Path
import textwrap

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import (
    StrictValidationCase,
    _VALIDATION_PRELUDE,
    assert_strict_source_rejected,
    create_strict_project,
)

_PROFILE_FUNCTIONS = (
    'DeltaBase.__init__',
    'DeltaBase.read',
    'DeltaBase.direction_value',
    'DeltaBase.write',
    'DeltaLeft.__init__',
    'DeltaRight.__init__',
    'StateBase.__init__',
    'StateBase.is_holding_or_waiting',
    'StateBase.is_waiting_with_packet',
    'StateBase.set_pending',
    'StateRoot.__init__',
    'StateAlpha.__init__',
    'StateBeta.__init__',
    'StateGamma.__init__',
    'StateDelta.__init__',
    'ObservedLeft.__getattribute__',
    'ObservedLeft.__setattr__',
    'SlottedBase.__init__',
    'SlottedBase.read',
    'WatchedValue.__init__',
    'WatchedValue.__del__',
)
_ORDINARY_FUNCTIONS = ('ReplacementDeltaBase.read',)


def test_inherited_split_fields_use_exact_profiled_concrete_owner_guards(
    tmp_path: Path,
) -> None:
    module_name = "inherited_owner_fields_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            EVENTS = []


            class DeltaBase:
                def __init__(self, value):
                    self.direction = True
                    self.value = value

                def read(self):
                    return self.value

                def direction_value(self):
                    if self.direction:
                        return self.value
                    return -self.value

                def write(self, value):
                    self.value = value


            class DeltaLeft(DeltaBase):
                def __init__(self, value):
                    DeltaBase.__init__(self, value)


            class DeltaRight(DeltaBase):
                def __init__(self, value):
                    self.padding = "first"
                    DeltaBase.__init__(self, value)


            class StateBase:
                def __init__(self):
                    self.packet_pending = False
                    self.task_waiting = True
                    self.task_holding = False

                def is_holding_or_waiting(self):
                    return self.task_holding or (
                        not self.packet_pending and self.task_waiting
                    )

                def is_waiting_with_packet(self):
                    return (
                        self.packet_pending
                        and self.task_waiting
                        and not self.task_holding
                    )

                def set_pending(self, value):
                    self.packet_pending = value


            class StateRoot(StateBase):
                def __init__(self, marker):
                    self.marker = marker
                    self.ident = marker
                    self.priority = marker
                    self.input = None
                    StateBase.__init__(self)


            class StateAlpha(StateRoot):
                def __init__(self, marker):
                    StateRoot.__init__(self, marker)


            class StateBeta(StateRoot):
                def __init__(self, marker):
                    StateRoot.__init__(self, marker)


            class StateGamma(StateRoot):
                def __init__(self, marker):
                    StateRoot.__init__(self, marker)


            class StateDelta(StateRoot):
                def __init__(self, marker):
                    StateRoot.__init__(self, marker)


            class ObservedLeft(DeltaLeft):
                def __getattribute__(self, name):
                    if name == "value":
                        EVENTS.append("observed:get")
                    return object.__getattribute__(self, name)

                def __setattr__(self, name, value):
                    if name == "value":
                        EVENTS.append(("observed:set", value))
                    return object.__setattr__(self, name, value)


            class ReplacementDeltaBase:
                def read(self):
                    EVENTS.append("replacement:read")
                    return self.value + 900


            class SlottedBase:
                __slots__ = ("value",)

                def __init__(self, value):
                    self.value = value

                def read(self):
                    return self.value


            class SlottedChild(SlottedBase):
                __slots__ = ()


            class WatchedValue:
                __slots__ = ("owner",)

                def __init__(self, owner):
                    self.owner = owner

                def __del__(self):
                    EVENTS.append(("destroyed", self.owner.read()))
            """
        ),
        encoding="utf-8",
    )

    script = textwrap.dedent(
        """
        import os
        import sys

        import builtins
        from pathlib import Path
        import types

        name = __NAME__
        source = Path(__ORIGINAL_SOURCE__).read_text(encoding="utf-8")
        stock = types.ModuleType(name)
        stock.__dict__["__builtins__"] = builtins.__dict__
        exec(compile(source, "<stock-owner-fields>", "exec"), stock.__dict__)

        # This ordinary arm retains the complete original workload and validator.
        def exercise(module):
            delta_owners = (module.DeltaLeft, module.DeltaRight)
            state_owners = (
                module.StateAlpha,
                module.StateBeta,
                module.StateGamma,
                module.StateDelta,
            )
            for value in range(32):
                for owner in delta_owners:
                    instance = owner(value)
                    assert instance.read() == value
                    assert instance.direction_value() == value
                    assert instance.write(value + 100) is None
                    assert object.__getattribute__(instance, "value") == value + 100

                for owner in state_owners:
                    state = owner(value)
                    assert state.is_holding_or_waiting() is True
                    assert state.set_pending(True) is None
                    assert state.is_waiting_with_packet() is True

                exact_base = module.StateBase()
                assert exact_base.is_holding_or_waiting() is True
                assert exact_base.set_pending(True) is None
                assert exact_base.is_waiting_with_packet() is True

            if os.environ.get("SOAC_OPT_MODE") != "profile":
                observed = module.ObservedLeft(7)
                module.EVENTS.clear()
                assert observed.read() == 7
                assert module.EVENTS == ["observed:get"], module.EVENTS

                module.EVENTS.clear()
                assert observed.write(8) is None
                assert module.EVENTS == [("observed:set", 8)], module.EVENTS

                module.EVENTS.clear()
                assert observed.direction_value() == 8
                assert module.EVENTS == ["observed:get"], module.EVENTS

                materialized = module.DeltaRight(11)
                instance_dict = materialized.__dict__
                assert instance_dict == {
                    "padding": "first",
                    "direction": True,
                    "value": 11,
                }, instance_dict
                instance_dict["value"] = 12
                assert materialized.read() == 12
                instance_dict[1] = "promoted"
                assert materialized.write(13) is None
                assert instance_dict["value"] == 13
                del instance_dict["value"]
                try:
                    materialized.read()
                except AttributeError as error:
                    assert "value" in str(error), error
                else:
                    raise AssertionError("a deleted promoted field must raise")
                instance_dict["value"] = 14
                assert materialized.read() == 14

                growing = module.DeltaLeft(21)
                for index in range(40):
                    setattr(growing, "extra_" + str(index), index)
                assert growing.read() == 21
                assert growing.write(22) is None
                assert growing.read() == 22
                del growing.value
                try:
                    growing.read()
                except AttributeError as error:
                    assert "value" in str(error), error
                else:
                    raise AssertionError("a deleted inherited field must raise")
                assert growing.write(23) is None
                assert growing.read() == 23

                watched_owner = module.DeltaLeft(0)
                old_value = module.WatchedValue(watched_owner)
                assert watched_owner.write(old_value) is None
                del old_value
                module.EVENTS.clear()
                assert watched_owner.write("replacement") is None
                assert module.EVENTS == [
                    ("destroyed", "replacement")
                ], module.EVENTS

                left = module.DeltaLeft(31)

                def leaf_get(instance):
                    module.EVENTS.append("leaf-property:get")
                    return 701

                def leaf_set(instance, value):
                    module.EVENTS.append(("leaf-property:set", value))

                module.DeltaLeft.value = property(leaf_get, leaf_set)
                try:
                    module.EVENTS.clear()
                    assert left.read() == 701
                    assert module.EVENTS == ["leaf-property:get"], module.EVENTS

                    module.EVENTS.clear()
                    assert left.write(33) is None
                    assert module.EVENTS == [
                        ("leaf-property:set", 33)
                    ], module.EVENTS
                finally:
                    del module.DeltaLeft.value
                assert left.read() == 31

                def base_get(instance):
                    module.EVENTS.append("base-property:get")
                    return 811

                def base_set(instance, value):
                    module.EVENTS.append(("base-property:set", value))

                module.DeltaBase.value = property(base_get, base_set)
                try:
                    module.EVENTS.clear()
                    assert left.read() == 811
                    assert module.EVENTS == ["base-property:get"], module.EVENTS

                    module.EVENTS.clear()
                    assert left.write(34) is None
                    assert module.EVENTS == [
                        ("base-property:set", 34)
                    ], module.EVENTS
                finally:
                    del module.DeltaBase.value
                assert left.read() == 31

                module.DeltaLeft.value = "non-data-class-value"
                try:
                    assert left.read() == 31
                finally:
                    del module.DeltaLeft.value

                changed_mro = module.DeltaRight(41)
                original_bases = module.DeltaRight.__bases__
                module.DeltaRight.__bases__ = (module.ReplacementDeltaBase,)
                try:
                    module.EVENTS.clear()
                    assert changed_mro.read() == 941
                    assert module.EVENTS == ["replacement:read"], module.EVENTS
                    assert module.DeltaBase.read(changed_mro) == 41
                finally:
                    module.DeltaRight.__bases__ = original_bases
                assert changed_mro.read() == 41

                original_read = module.DeltaBase.read

                def rebound_read(instance):
                    module.EVENTS.append("base:rebound")
                    return object.__getattribute__(instance, "value") + 500

                module.DeltaBase.read = rebound_read
                try:
                    module.EVENTS.clear()
                    assert left.read() == 531
                    assert module.EVENTS == ["base:rebound"], module.EVENTS
                finally:
                    module.DeltaBase.read = original_read
                assert left.read() == 31

                state = module.StateBeta(1)

                def pending_get(instance):
                    module.EVENTS.append("state-property:get")
                    return True

                def pending_set(instance, value):
                    module.EVENTS.append(("state-property:set", value))

                module.StateBeta.packet_pending = property(
                    pending_get, pending_set
                )
                try:
                    module.EVENTS.clear()
                    assert state.is_waiting_with_packet() is True
                    assert module.EVENTS == ["state-property:get"], module.EVENTS

                    module.EVENTS.clear()
                    assert state.set_pending(False) is None
                    assert module.EVENTS == [
                        ("state-property:set", False)
                    ], module.EVENTS
                finally:
                    del module.StateBeta.packet_pending
                assert state.is_waiting_with_packet() is False

                original_left = module.DeltaLeft

                class ReplacementLeft:
                    def __init__(self, value):
                        self.value = value

                module.DeltaLeft = ReplacementLeft
                try:
                    assert module.DeltaBase.read(module.DeltaLeft(52)) == 52
                finally:
                    module.DeltaLeft = original_left

                slot = module.SlottedChild(61)
                assert slot.read() == 61
                del slot.value
                try:
                    slot.read()
                except AttributeError as error:
                    assert "value" in str(error), error
                else:
                    raise AssertionError("an inherited deleted slot must raise")

        # Selected classes/modules are sealed; only their mutation outcomes differ.
        def exercise_strict(module):
            delta_owners = (module.DeltaLeft, module.DeltaRight)
            state_owners = (
                module.StateAlpha,
                module.StateBeta,
                module.StateGamma,
                module.StateDelta,
            )
            for value in range(32):
                for owner in delta_owners:
                    instance = owner(value)
                    assert instance.read() == value
                    assert instance.direction_value() == value
                    assert instance.write(value + 100) is None
                    assert object.__getattribute__(instance, "value") == value + 100

                for owner in state_owners:
                    state = owner(value)
                    assert state.is_holding_or_waiting() is True
                    assert state.set_pending(True) is None
                    assert state.is_waiting_with_packet() is True

                exact_base = module.StateBase()
                assert exact_base.is_holding_or_waiting() is True
                assert exact_base.set_pending(True) is None
                assert exact_base.is_waiting_with_packet() is True

            if os.environ.get("SOAC_OPT_MODE") != "profile":
                observed = module.ObservedLeft(7)
                module.EVENTS.clear()
                assert observed.read() == 7
                assert module.EVENTS == ["observed:get"], module.EVENTS

                module.EVENTS.clear()
                assert observed.write(8) is None
                assert module.EVENTS == [("observed:set", 8)], module.EVENTS

                module.EVENTS.clear()
                assert observed.direction_value() == 8
                assert module.EVENTS == ["observed:get"], module.EVENTS

                materialized = module.DeltaRight(11)
                instance_dict = materialized.__dict__
                assert _testinternalcapi.dict_has_indexed_keys(instance_dict) is False
                assert instance_dict == {
                    "padding": "first",
                    "direction": True,
                    "value": 11,
                }, instance_dict
                instance_dict["value"] = 12
                assert materialized.read() == 12
                instance_dict[1] = "promoted"
                assert _testinternalcapi.dict_has_indexed_keys(instance_dict) is False
                assert materialized.write(13) is None
                assert instance_dict["value"] == 13
                del instance_dict["value"]
                try:
                    materialized.read()
                except AttributeError as error:
                    assert "value" in str(error), error
                else:
                    raise AssertionError("a deleted promoted field must raise")
                instance_dict["value"] = 14
                assert materialized.read() == 14

                growing = module.DeltaLeft(21)
                for index in range(40):
                    setattr(growing, "extra_" + str(index), index)
                assert growing.read() == 21
                assert growing.write(22) is None
                assert growing.read() == 22
                del growing.value
                try:
                    growing.read()
                except AttributeError as error:
                    assert "value" in str(error), error
                else:
                    raise AssertionError("a deleted inherited field must raise")
                assert growing.write(23) is None
                assert growing.read() == 23

                watched_owner = module.DeltaLeft(0)
                old_value = module.WatchedValue(watched_owner)
                assert watched_owner.write(old_value) is None
                del old_value
                module.EVENTS.clear()
                assert watched_owner.write("replacement") is None
                assert module.EVENTS == [
                    ("destroyed", "replacement")
                ], module.EVENTS

                left = module.DeltaLeft(31)

                def leaf_get(instance):
                    module.EVENTS.append("leaf-property:get")
                    return 701

                def leaf_set(instance, value):
                    module.EVENTS.append(("leaf-property:set", value))

                with reject_sealed_mutation("leaf_descriptor"):
                    module.DeltaLeft.value = property(leaf_get, leaf_set)
                module.EVENTS.clear()
                assert left.read() == 31
                assert module.EVENTS == [], module.EVENTS
                assert left.write(33) is None
                assert module.EVENTS == [], module.EVENTS
                assert left.read() == 33

                def base_get(instance):
                    module.EVENTS.append("base-property:get")
                    return 811

                def base_set(instance, value):
                    module.EVENTS.append(("base-property:set", value))

                with reject_sealed_mutation("base_descriptor"):
                    module.DeltaBase.value = property(base_get, base_set)
                module.EVENTS.clear()
                assert left.read() == 33
                assert module.EVENTS == [], module.EVENTS
                assert left.write(34) is None
                assert module.EVENTS == [], module.EVENTS
                assert left.read() == 34

                with reject_sealed_mutation("leaf_class_binding"):
                    module.DeltaLeft.value = "non-data-class-value"
                assert left.read() == 34

                changed_mro = module.DeltaRight(41)
                original_bases = module.DeltaRight.__bases__
                with reject_sealed_mutation("bases"):
                    module.DeltaRight.__bases__ = (module.ReplacementDeltaBase,)
                assert module.DeltaRight.__bases__ is original_bases
                module.EVENTS.clear()
                assert changed_mro.read() == 41
                assert module.EVENTS == [], module.EVENTS
                assert module.DeltaBase.read(changed_mro) == 41
                assert changed_mro.read() == 41

                original_read = module.DeltaBase.read

                def rebound_read(instance):
                    module.EVENTS.append("base:rebound")
                    return object.__getattribute__(instance, "value") + 500

                with reject_sealed_mutation("base_method"):
                    module.DeltaBase.read = rebound_read
                assert module.DeltaBase.read is original_read
                module.EVENTS.clear()
                assert left.read() == 34
                assert module.EVENTS == [], module.EVENTS

                state = module.StateBeta(1)

                def pending_get(instance):
                    module.EVENTS.append("state-property:get")
                    return True

                def pending_set(instance, value):
                    module.EVENTS.append(("state-property:set", value))

                with reject_sealed_mutation("state_descriptor"):
                    module.StateBeta.packet_pending = property(pending_get, pending_set)
                module.EVENTS.clear()
                assert state.is_waiting_with_packet() is False
                assert module.EVENTS == [], module.EVENTS
                assert state.set_pending(False) is None
                assert module.EVENTS == [], module.EVENTS
                assert state.is_waiting_with_packet() is False

                original_left = module.DeltaLeft

                class ReplacementLeft:
                    def __init__(self, value):
                        self.value = value

                with reject_sealed_mutation("module_class_binding"):
                    module.DeltaLeft = ReplacementLeft
                assert module.DeltaLeft is original_left
                assert module.DeltaBase.read(module.DeltaLeft(52)) == 52

                slot = module.SlottedChild(61)
                assert slot.read() == 61
                del slot.value
                try:
                    slot.read()
                except AttributeError as error:
                    assert "value" in str(error), error
                else:
                    raise AssertionError("an inherited deleted slot must raise")

        assert_ordinary_bindings(stock)
        exercise(stock)
        assert_ordinary_bindings(stock)
        exercise_strict(module)
        if os.environ.get("SOAC_OPT_MODE") != "profile":
            assert sealed_rejections == ['leaf_descriptor', 'base_descriptor', 'leaf_class_binding', 'bases', 'base_method', 'state_descriptor', 'module_class_binding']
        else:
            assert sealed_rejections == []
        """
    )

    # The replacement is valid only after ordinary Python changes an existing
    # instance's MRO: its own class never establishes the value read by read().
    # Preserve the entire original source and validator as the stock control,
    # and explicitly reject selecting that entire source for strict execution.
    relative = f"{module_name}.py"
    original = (tmp_path / relative).read_text(encoding="utf-8")
    rejected = assert_strict_source_rejected(
        tmp_path / "rejected-original-source",
        strict_opt_in(original.encode(), relative)[0].decode(),
        module_name=module_name,
        diagnostic="unresolved-attribute",
    )
    assert "`Self@read` has no attribute `value`" in rejected, rejected

    # Only the exact replacement class is an ordinary dependency. All remaining
    # source classes and functions are selected normally; this is an explicit
    # interoperability boundary, not admission of the rejected original module.
    replacement_name = f"{module_name}_ordinary_replacement"
    replacement = next(
        node for node in ast.parse(original).body
        if isinstance(node, ast.ClassDef) and node.name == "ReplacementDeltaBase"
    )
    lines = original.splitlines(keepends=True)
    replacement_source = "".join(
        lines[replacement.lineno - 1:replacement.end_lineno]
    )
    selected_source = "".join(
        lines[:replacement.lineno - 1]
        + [f"from {replacement_name} import ReplacementDeltaBase\n"]
        + ["\n"] * (replacement.end_lineno - replacement.lineno)
        + lines[replacement.end_lineno:]
    )
    project = create_strict_project(
        tmp_path / "strict-project",
        {
            relative: strict_opt_in(selected_source.encode(), relative)[0].decode(),
            f"{replacement_name}.py": (
                f"from {module_name} import EVENTS\n\n{replacement_source}"
            ),
        },
        modules={module_name: relative},
    )

    script = (
        script.replace("__NAME__", repr(module_name))
        .replace("__ORIGINAL_SOURCE__", repr(str(tmp_path / f"{module_name}.py")))
    )
    witnesses = f"""
import ctypes
import types
from contextlib import contextmanager
import _testinternalcapi
import pytest
from soac import _soac_ext
from soac.strict import StrictMutationError
from tests._strict_integration import _plain_function_witness

# This legacy slot authorizes unchecked direct targets, not source identity.
# Source-owned entries deliberately leave it zero.
unchecked_target_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
unchecked_target_id.argtypes = [ctypes.py_object]
unchecked_target_id.restype = ctypes.c_uint64
sealed_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
sealed_id.argtypes = [ctypes.py_object]
sealed_id.restype = ctypes.c_uint64
native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
native_owner.argtypes = [ctypes.py_object]
native_owner.restype = ctypes.c_void_p
native_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
native_metadata.argtypes = [ctypes.py_object]
native_metadata.restype = ctypes.c_void_p
type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
type_owner.argtypes = [ctypes.py_object]
type_owner.restype = ctypes.c_void_p
type_sealed = ctypes.pythonapi.PyType_IsSoacSealed
type_sealed.argtypes = [ctypes.py_object]
type_sealed.restype = ctypes.c_int
slot_matches = ctypes.pythonapi.PyType_MatchesSoacObjectSlotDescriptor
slot_matches.argtypes = [ctypes.py_object, ctypes.c_void_p,
    ctypes.c_ssize_t, ctypes.py_object]
slot_matches.restype = ctypes.c_int

dynamic_source_methods = (
    'ObservedLeft.__getattribute__', 'ObservedLeft.__setattr__',
)

def assert_native_function(function, path=None):
    assert type(function) is types.FunctionType, (path, type(function))
    path = function.__qualname__ if path is None else path
    actual = (path, unchecked_target_id(function), sealed_id(function),
        native_owner(function), native_metadata(function),
        _soac_ext.strict_function_entry_kind(function))
    assert actual[1] == 0, actual
    # These exact custom-hook methods are owned selected source functions, but
    # their statically dynamic class does not authorize a permanent method seal.
    if path in dynamic_source_methods:
        assert actual[2] == 0, actual
    else:
        assert actual[2] > 0, actual
    assert actual[3], actual
    assert actual[4], actual
    assert actual[5] == "checked_native", actual

def function_snapshot(function, path=None):
    assert_native_function(function, path)
    return (function, function.__code__, function.__defaults__,
        function.__kwdefaults__, unchecked_target_id(function), sealed_id(function),
        native_owner(function))

def assert_function_snapshot(function, saved, path=None):
    assert_native_function(function, path)
    assert function is saved[0] and function.__code__ is saved[1]
    assert function.__defaults__ is saved[2]
    assert function.__kwdefaults__ is saved[3]
    assert unchecked_target_id(function) == saved[4]
    assert sealed_id(function) == saved[5]
    assert native_owner(function) == saved[6]

def type_snapshot(cls):
    assert type(cls) is type
    owner = type_owner(cls)
    assert owner and type_sealed(cls) == 1
    return (cls, owner, cls.__bases__, tuple(cls.__dict__.items()))

def assert_type_snapshot(cls, saved):
    assert cls is saved[0] and type_owner(cls) == saved[1]
    assert type_sealed(cls) == 1 and cls.__bases__ is saved[2]
    actual = cls.__dict__
    assert len(actual) == len(saved[3])
    for name, value in saved[3]:
        assert actual[name] is value, (cls, name)

def assert_object_slot(cls, name):
    owner = type_owner(cls)
    descriptor = cls.__dict__[name]
    assert owner and type_sealed(cls) == 1
    assert type(descriptor) is types.MemberDescriptorType
    assert descriptor.__objclass__ is cls and descriptor.__name__ == name
    assert cls.__dictoffset__ == 0
    assert slot_matches(cls, owner, 0, descriptor) == 1

saved_functions = {{path: function_snapshot(_plain_function_witness(module, path), path)
    for path in {_PROFILE_FUNCTIONS!r}}}
saved_types = {{name: type_snapshot(module.__dict__[name])
    for name in ('DeltaBase', 'DeltaLeft', 'DeltaRight', 'StateBase', 'StateRoot', 'StateAlpha', 'StateBeta', 'StateGamma', 'StateDelta', 'SlottedBase', 'SlottedChild', 'WatchedValue')}}

def assert_ordinary_function(function, path):
    assert type(function) is types.FunctionType, path
    assert unchecked_target_id(function) == 0, path
    assert sealed_id(function) == 0, path
    assert native_owner(function) is None, path
    assert native_metadata(function) is None, path
    assert _soac_ext.strict_function_entry_kind(function) is None, path

def assert_selected_bindings():
    for path, saved in saved_functions.items():
        assert_function_snapshot(_plain_function_witness(module, path), saved, path)
    for name, saved in saved_types.items():
        assert_type_snapshot(module.__dict__[name], saved)
    # Source functions remain owned, but custom attribute hooks select an
    # ordinary subclass. Inherited checks and actual hooks still run below.
    for name in ('ObservedLeft',):
        cls = module.__dict__[name]
        assert type_owner(cls) is None, name
        assert type_sealed(cls) == 0, name
    for name, field in (('SlottedBase', 'value'),):
        assert_object_slot(module.__dict__[name], field)
    assert module.ReplacementDeltaBase.__module__ == {replacement_name!r}
    assert type_owner(module.ReplacementDeltaBase) is None
    assert type_sealed(module.ReplacementDeltaBase) == 0
    for path in {_ORDINARY_FUNCTIONS!r}:
        assert_ordinary_function(_plain_function_witness(module, path), path)

def assert_ordinary_bindings(stock):
    for path in {_PROFILE_FUNCTIONS + _ORDINARY_FUNCTIONS!r}:
        assert_ordinary_function(_plain_function_witness(stock, path), path)

sealed_rejections = []

@contextmanager
def reject_sealed_mutation(name):
    assert_selected_bindings()
    with pytest.raises(StrictMutationError) as caught:
        yield
    assert type(caught.value) is StrictMutationError, (name, caught.value)
    assert_selected_bindings()
    sealed_rejections.append(name)

assert_selected_bindings()
"""
    validation = "def validate_module(module):\n" + textwrap.indent(
        witnesses + script + "\nassert_selected_bindings()\n", "    "
    )
    program = _VALIDATION_PRELUDE + project._validation_program(
        module_name,
        StrictValidationCase(
            validation, Path(__file__), required_functions=_PROFILE_FUNCTIONS,
            
        ),
        entry_interpreter=False,
    )

    work_dir = tmp_path / "soac-work"

    def run_mode(mode: str) -> None:
        result = project.run(
                     program, opt_mode=mode,
                     extra_env={"SOAC_WORK_DIR": str(work_dir)},
                     timeout=90, check=False,
                 )
        assert result.returncode == 0, (
            f"{mode} subprocess failed:\n{result.stdout}{result.stderr}"
        )

    run_mode("profile")

    from soac import _soac_ext

    profile_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    profile_records = [
        record
        for record in profile_dump["records"]
        if record["module_name"] == module_name
    ]
    assert profile_records, profile_dump

    def field_rows(records: list[dict]) -> dict[tuple, dict]:
        rows: dict[tuple, dict] = {}
        for record in records:
            for row in record["rows"]:
                if row["kind"] != "field_access":
                    continue
                key = (
                    row["function_id"],
                    row["instr_id"],
                    row["counter_id"],
                )
                previous = rows.get(key)
                if previous is None or row["value"] >= previous["value"]:
                    rows[key] = row
        return rows

    def branch_rows(
        records: list[dict], qualname: str, branch: str
    ) -> list[tuple[str, int]]:
        return sorted(
            (
                row["instr_id"],
                row.get("branches", {}).get(branch, 0),
            )
            for row in field_rows(records).values()
            if row["function_qualname"] == qualname
        )

    expected_get_sites = {
        "DeltaBase.read": (1, 64),
        "DeltaBase.direction_value": (2, 64),
        "StateBase.is_holding_or_waiting": (3, 160),
        "StateBase.is_waiting_with_packet": (3, 160),
    }
    expected_set_sites = {
        "DeltaBase.write": (1, 64),
        "StateBase.set_pending": (1, 160),
    }

    for qualname, (site_count, calls) in expected_get_sites.items():
        rows = branch_rows(profile_records, qualname, "generic_getattr")
        hot_rows = [(source, count) for source, count in rows if count >= calls]
        assert len(hot_rows) >= site_count, (
            "Profile must expose each genuine inherited-base read source",
            qualname,
            {"required_sites": site_count, "calls_per_site": calls},
            rows,
        )

    for qualname, (site_count, calls) in expected_set_sites.items():
        rows = branch_rows(profile_records, qualname, "generic_setattr")
        hot_rows = [(source, count) for source, count in rows if count >= calls]
        assert len(hot_rows) >= site_count, (
            "Profile must expose each genuine inherited-base write source",
            qualname,
            {"required_sites": site_count, "calls_per_site": calls},
            rows,
        )

    concrete_owners = {
        "DeltaLeft",
        "DeltaRight",
        "StateAlpha",
        "StateBeta",
        "StateGamma",
        "StateDelta",
    }
    profiled_owners = concrete_owners | {"StateBase"}
    owner_type_ids = {
        entry["qualname"]: entry["type_id"]
        for record in profile_dump["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
        and entry["qualname"] in profiled_owners
    }
    assert set(owner_type_ids) == profiled_owners, owner_type_ids

    abstract_owner_entries = {
        entry["qualname"]
        for record in profile_dump["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
        and entry["qualname"] in {"DeltaBase", "StateRoot"}
    }
    assert not abstract_owner_entries, (
        "the lexical base owners must have no misleading exact-owner "
        "type_keys: only the actual concrete receivers are profiled",
        abstract_owner_entries,
    )

    owner_indexes: dict[str, dict[str, int]] = {
        owner: {} for owner in profiled_owners
    }
    for record in profile_dump["records"]:
        for key in record["type_keys"]:
            for owner, type_id in owner_type_ids.items():
                if key["owner_type_id"] == type_id:
                    owner_indexes[owner][key["key"]] = key["index"]

    left = owner_indexes["DeltaLeft"]
    right = owner_indexes["DeltaRight"]
    assert left["direction"] != right["direction"], owner_indexes
    assert left["value"] != right["value"], owner_indexes
    assert right["padding"] < right["direction"], owner_indexes

    exact_state_layout = tuple(
        owner_indexes["StateBase"][name]
        for name in ("packet_pending", "task_waiting", "task_holding")
    )
    assert exact_state_layout == (0, 2, 1), owner_indexes

    state_layouts = {
        owner: tuple(
            owner_indexes[owner][name]
            for name in (
                "marker",
                "packet_pending",
                "task_waiting",
                "task_holding",
            )
        )
        for owner in ("StateAlpha", "StateBeta", "StateGamma", "StateDelta")
    }
    assert len(set(state_layouts.values())) == 1, state_layouts
    assert all(layout == (0, 4, 5, 6) for layout in state_layouts.values()), (
        "all four concrete task subclasses must shift the inherited "
        "exact-base indices (0, 1, 2) to (4, 5, 6)",
        {"exact_base": exact_state_layout, "concrete_owners": state_layouts},
    )

    run_mode("verify")
    verify_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "verify.bin"))
    )
    verify_records = [
        record
        for record in verify_dump["records"]
        if record["module_name"] == module_name
    ]
    assert verify_records, verify_dump

    run_mode("apply")

    expected_sites = {**expected_get_sites, **expected_set_sites}
    indexed_hits = {
        qualname: branch_rows(verify_records, qualname, "indexed_hit")
        for qualname in expected_sites
    }
    # The validator above proves these are real ordinary instance dictionaries,
    # not installed indexed layouts. Their observed shared-key positions do
    # not authorize bypassing the selected native attribute/method policies.
    assert all(
        hits == 0 for rows in indexed_hits.values() for _, hits in rows
    ), ("ordinary checked dictionaries acquired an unchecked field hit", indexed_hits)
    for qualname, (site_count, calls) in expected_get_sites.items():
        rows = branch_rows(verify_records, qualname, "indexed_fallback")
        assert len([source for source, count in rows if count >= calls]) >= site_count, (
            "each profiled inherited read must reach its checked lookup fallback",
            qualname,
            {"required_sites": site_count, "calls_per_site": calls},
            rows,
        )
    for qualname, (site_count, calls) in expected_set_sites.items():
        rows = branch_rows(verify_records, qualname, "generic_setattr")
        assert len([source for source, count in rows if count >= calls]) >= site_count, (
            "each inherited write must retain the native checked attribute operation",
            qualname,
            {"required_sites": site_count, "calls_per_site": calls},
            rows,
        )
