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
    'Left.__init__',
    'Right.__init__',
    'Third.__init__',
    'Fourth.__init__',
    'Packet.__init__',
    'Unique.__init__',
    'MixedLeft.__init__',
    'MixedRight.__init__',
    'OverOne.__init__',
    'OverTwo.__init__',
    'OverThree.__init__',
    'OverFour.__init__',
    'OverFive.__init__',
    'OverSix.__init__',
    'Slotted.__init__',
    'Cold.__init__',
    'Consumer.read',
    'Observed.__getattribute__',
    'Observed.__setattr__',
    'FinalizingValue.__init__',
    'FinalizingValue.__del__',
    'read_uniform',
    'schedule',
    'write_uniform',
    'read_unique',
    'read_mixed',
    'read_overflow',
    'read_slotted',
    'read_unanchored',
    'read_cold',
)
_ORDINARY_FUNCTIONS = ('Base.read_self',)


_SOURCE = """
EVENTS = []


class Base:
    def read_self(self):
        return self.link


class Left(Base):
    def __init__(self, value):
        self.link = value


class Right(Base):
    def __init__(self, value):
        self.link = value


class Third(Base):
    def __init__(self, value):
        self.link = value


class Fourth(Base):
    def __init__(self, value):
        self.link = value


class Packet:
    def __init__(self, value):
        self.link = value


class Unique:
    def __init__(self, value):
        self.padding = None
        self.payload = value


class MixedLeft:
    def __init__(self, value):
        self.mixed = value


class MixedRight:
    def __init__(self, value):
        self._padding = None
        self.mixed = value


class OverOne:
    def __init__(self, value):
        self.overflow = value


class OverTwo:
    def __init__(self, value):
        self.overflow = value


class OverThree:
    def __init__(self, value):
        self.overflow = value


class OverFour:
    def __init__(self, value):
        self.overflow = value


class OverFive:
    def __init__(self, value):
        self.overflow = value


class OverSix:
    def __init__(self, value):
        self.overflow = value


class Slotted:
    __slots__ = ("slotted",)

    def __init__(self, value):
        self.slotted = value


class Unanchored:
    pass


class Cold:
    def __init__(self, value):
        self.cold = value


class Consumer:
    def read(self, owner):
        return owner.link


class Observed(Left):
    def __getattribute__(self, name):
        if name == "link":
            EVENTS.append("observed:get")
        return object.__getattribute__(self, name)

    def __setattr__(self, name, value):
        if name == "link":
            EVENTS.append(("observed:set", value))
        object.__setattr__(self, name, value)


class AlternateBase:
    @property
    def link(self):
        EVENTS.append("alternate:get")
        return "alternate"


class FinalizingValue:
    def __init__(self, owner):
        self.owner = owner

    def __del__(self):
        EVENTS.append(("finalized", read_uniform(self.owner)))


def read_uniform(owner):
    return owner.link


def schedule(owners):
    return [owner.link for owner in owners]


def write_uniform(owner, value):
    owner.link = value


def read_unique(owner):
    return owner.payload


def read_mixed(owner):
    return owner.mixed


def read_overflow(owner):
    return owner.overflow


def read_slotted(owner):
    return owner.slotted


def read_unanchored(owner):
    return owner.unanchored


def read_cold(owner):
    return owner.cold
"""


def _run_mode(project, tmp_path: Path, module_name: str, work_dir: Path, mode: str) -> None:
    script = textwrap.dedent(
        """

        import builtins
        import gc
        import types
        import sys
        import weakref

        root, name, mode = __ROOT__, __NAME__, __MODE__
        source = open(root + "/" + name + ".py", encoding="utf-8").read()
        stock_module = types.ModuleType(name)
        stock = stock_module.__dict__
        stock["__builtins__"] = builtins.__dict__
        exec(compile(source, "<stock-uniform-polymorphic-fields>", "exec"), stock)

        def capture_error(callback):
            try:
                callback()
            except Exception as error:
                return type(error).__name__, str(error)
            raise AssertionError("expected field operation to fail")

        def exercise(namespace):
            consumer = namespace["Consumer"]()
            owners = ("Left", "Right", "Third", "Fourth", "Packet")
            overflow = (
                "OverOne", "OverTwo", "OverThree",
                "OverFour", "OverFive", "OverSix",
            )

            for index in range(32):
                values = []
                objects = []
                for owner_name in owners:
                    original = (owner_name, index)
                    owner = namespace[owner_name](original)
                    assert namespace["read_uniform"](owner) == original
                    assert consumer.read(owner) == original
                    if owner_name != "Packet":
                        assert owner.read_self() == original
                    updated = ("updated", owner_name, index)
                    assert namespace["write_uniform"](owner, updated) is None
                    assert namespace["read_uniform"](owner) == updated
                    objects.append(owner)
                    values.append(updated)
                assert namespace["schedule"](objects) == values

                assert namespace["read_unique"](namespace["Unique"](index)) == index
                assert namespace["read_mixed"](namespace["MixedLeft"](index)) == index
                assert namespace["read_mixed"](namespace["MixedRight"](index)) == index
                for owner_name in overflow:
                    assert namespace["read_overflow"](
                        namespace[owner_name](index)
                    ) == index

                slot = namespace["Slotted"](index)
                assert namespace["read_slotted"](slot) == index
                unanchored = namespace["Unanchored"]()
                unanchored.unanchored = index
                assert namespace["read_unanchored"](unanchored) == index
                if index == 0:
                    assert namespace["read_cold"](namespace["Cold"](index)) == 0

            if mode == "profile":
                return "profile-controls-pass"

            outcomes = {}
            observed = namespace["Observed"]("observed")
            namespace["EVENTS"].clear()
            assert namespace["read_uniform"](observed) == "observed"
            outcomes["getattribute"] = namespace["EVENTS"][:]
            namespace["EVENTS"].clear()
            namespace["write_uniform"](observed, "changed")
            outcomes["setattr"] = namespace["EVENTS"][:]

            owner = namespace["Left"]("instance")
            dictionary = owner.__dict__
            dictionary["link"] = "materialized"
            outcomes["materialized"] = namespace["read_uniform"](owner)
            dictionary[1] = "promoted"
            outcomes["promoted"] = namespace["read_uniform"](owner)
            del dictionary["link"]
            outcomes["deleted"] = capture_error(
                lambda: namespace["read_uniform"](owner)
            )
            namespace["write_uniform"](owner, "reinserted")
            outcomes["reinserted"] = namespace["read_uniform"](owner)

            growing = namespace["Right"]("growing")
            for index in range(40):
                setattr(growing, "extra_" + str(index), index)
            outcomes["grown"] = namespace["read_uniform"](growing)

            descriptor_owner = namespace["Third"]("stored")
            namespace["Third"].link = property(
                lambda instance: "descriptor",
                lambda instance, value: namespace["EVENTS"].append(("descriptor:set", value)),
            )
            try:
                outcomes["descriptor"] = namespace["read_uniform"](descriptor_owner)
                namespace["EVENTS"].clear()
                namespace["write_uniform"](descriptor_owner, "assigned")
                outcomes["descriptor_set"] = namespace["EVENTS"][:]
            finally:
                del namespace["Third"].link
            outcomes["restored"] = namespace["read_uniform"](descriptor_owner)

            inherited = namespace["Fourth"]("inherited")
            original_bases = namespace["Fourth"].__bases__
            namespace["Fourth"].__bases__ = (namespace["AlternateBase"],)
            try:
                namespace["EVENTS"].clear()
                outcomes["mro"] = (
                    namespace["read_uniform"](inherited),
                    namespace["EVENTS"][:],
                )
            finally:
                namespace["Fourth"].__bases__ = original_bases

            class Foreign:
                def __init__(self, value):
                    self.link = value

            outcomes["foreign"] = namespace["read_uniform"](Foreign("foreign"))

            original_packet = namespace["Packet"]

            class ReplacementPacket:
                def __init__(self, value):
                    self.link = value

            namespace["Packet"] = ReplacementPacket
            try:
                outcomes["rebound_owner"] = namespace["read_uniform"](
                    namespace["Packet"]("replacement")
                )
            finally:
                namespace["Packet"] = original_packet

            owner = namespace["Packet"]("old")
            watched = namespace["FinalizingValue"](owner)
            namespace["write_uniform"](owner, watched)
            del watched
            namespace["EVENTS"].clear()
            namespace["write_uniform"](owner, "visible-during-finalizer")
            gc.collect()
            outcomes["finalizer"] = namespace["EVENTS"][:]

            retained = namespace["Packet"]("retained")
            reference = weakref.ref(retained)
            before = sys.getrefcount(retained)
            assert namespace["read_uniform"](retained) == "retained"
            outcomes["receiver_refcount"] = sys.getrefcount(retained) == before
            del retained
            gc.collect()
            outcomes["receiver_released"] = reference() is None
            return outcomes

        # The complete original ordinary exercise above is retained unchanged.
        def exercise_strict(namespace):
            consumer = namespace["Consumer"]()
            owners = ("Left", "Right", "Third", "Fourth", "Packet")
            overflow = (
                "OverOne", "OverTwo", "OverThree",
                "OverFour", "OverFive", "OverSix",
            )

            for index in range(32):
                values = []
                objects = []
                for owner_name in owners:
                    original = (owner_name, index)
                    owner = namespace[owner_name](original)
                    assert namespace["read_uniform"](owner) == original
                    assert consumer.read(owner) == original
                    if owner_name != "Packet":
                        assert owner.read_self() == original
                    updated = ("updated", owner_name, index)
                    assert namespace["write_uniform"](owner, updated) is None
                    assert namespace["read_uniform"](owner) == updated
                    objects.append(owner)
                    values.append(updated)
                assert namespace["schedule"](objects) == values

                assert namespace["read_unique"](namespace["Unique"](index)) == index
                assert namespace["read_mixed"](namespace["MixedLeft"](index)) == index
                assert namespace["read_mixed"](namespace["MixedRight"](index)) == index
                for owner_name in overflow:
                    assert namespace["read_overflow"](
                        namespace[owner_name](index)
                    ) == index

                slot = namespace["Slotted"](index)
                assert namespace["read_slotted"](slot) == index
                unanchored = namespace["Unanchored"]()
                unanchored.unanchored = index
                assert namespace["read_unanchored"](unanchored) == index
                if index == 0:
                    assert namespace["read_cold"](namespace["Cold"](index)) == 0

            if mode == "profile":
                return "profile-controls-pass"

            outcomes = {}
            observed = namespace["Observed"]("observed")
            namespace["EVENTS"].clear()
            assert namespace["read_uniform"](observed) == "observed"
            outcomes["getattribute"] = namespace["EVENTS"][:]
            namespace["EVENTS"].clear()
            namespace["write_uniform"](observed, "changed")
            outcomes["setattr"] = namespace["EVENTS"][:]

            owner = namespace["Left"]("instance")
            dictionary = owner.__dict__
            assert _testinternalcapi.dict_has_indexed_keys(dictionary) is False
            dictionary["link"] = "materialized"
            outcomes["materialized"] = namespace["read_uniform"](owner)
            dictionary[1] = "promoted"
            assert _testinternalcapi.dict_has_indexed_keys(dictionary) is False
            outcomes["promoted"] = namespace["read_uniform"](owner)
            del dictionary["link"]
            outcomes["deleted"] = capture_error(
                lambda: namespace["read_uniform"](owner)
            )
            namespace["write_uniform"](owner, "reinserted")
            outcomes["reinserted"] = namespace["read_uniform"](owner)

            growing = namespace["Right"]("growing")
            for index in range(40):
                setattr(growing, "extra_" + str(index), index)
            outcomes["grown"] = namespace["read_uniform"](growing)

            descriptor_owner = namespace["Third"]("stored")
            namespace["Third"].link = property(
                lambda instance: "descriptor",
                lambda instance, value: namespace["EVENTS"].append(("descriptor:set", value)),
            )
            try:
                outcomes["descriptor"] = namespace["read_uniform"](descriptor_owner)
                namespace["EVENTS"].clear()
                namespace["write_uniform"](descriptor_owner, "assigned")
                outcomes["descriptor_set"] = namespace["EVENTS"][:]
            finally:
                del namespace["Third"].link
            outcomes["restored"] = namespace["read_uniform"](descriptor_owner)

            inherited = namespace["Fourth"]("inherited")
            fourth = namespace["Fourth"]
            original_bases, original_mro = fourth.__bases__, fourth.__mro__
            original_namespace = tuple(fourth.__dict__.items())
            inherited_dictionary = inherited.__dict__
            original_items = tuple(inherited_dictionary.items())
            namespace["EVENTS"].clear()
            # Fourth is dynamic, but AlternateBase has its own permanent
            # contract. Adding that strict ancestry requires construction-time
            # admission; dynamic fallback is not permission to add it later.
            with reject_sealed_mutation("strict_base_adoption"):
                fourth.__bases__ = (namespace["AlternateBase"],)
            assert fourth.__bases__ is original_bases
            assert fourth.__mro__ is original_mro
            assert len(fourth.__dict__) == len(original_namespace)
            for name, value in original_namespace:
                assert fourth.__dict__[name] is value, name
            assert type(inherited) is fourth
            assert inherited.__dict__ is inherited_dictionary
            assert tuple(inherited_dictionary.items()) == original_items
            outcomes["mro"] = (
                namespace["read_uniform"](inherited),
                namespace["EVENTS"][:],
            )
            assert outcomes["mro"] == ("inherited", []), outcomes

            # Keep a separate positive MRO-invalidation control through the
            # selected reader. This ordinary target does not replace the
            # original strict-target rejection above.
            ordinary_alternate = stock["AlternateBase"]
            assert ordinary_alternate is not namespace["AlternateBase"]
            assert type_owner(ordinary_alternate) is None
            assert type_sealed(ordinary_alternate) == 0
            assert_ordinary_function(
                ordinary_alternate.__dict__["link"].fget,
                "ordinary AlternateBase.link",
            )
            stock["EVENTS"].clear()
            fourth.__bases__ = (ordinary_alternate,)
            try:
                assert fourth.__bases__ == (ordinary_alternate,)
                assert type_owner(fourth) is None and type_sealed(fourth) == 0
                assert namespace["read_uniform"](inherited) == "alternate"
                assert stock["EVENTS"] == ["alternate:get"]
                assert namespace["EVENTS"] == []
                assert type(inherited) is fourth
                assert inherited.__dict__ is inherited_dictionary
                assert tuple(inherited_dictionary.items()) == original_items
            finally:
                fourth.__bases__ = original_bases
            assert fourth.__bases__ is original_bases
            assert fourth.__mro__ == original_mro
            assert namespace["read_uniform"](inherited) == "inherited"
            assert stock["EVENTS"] == ["alternate:get"]
            assert namespace["EVENTS"] == []

            class Foreign:
                def __init__(self, value):
                    self.link = value

            outcomes["foreign"] = namespace["read_uniform"](Foreign("foreign"))

            original_packet = namespace["Packet"]

            class ReplacementPacket:
                def __init__(self, value):
                    self.link = value

            with reject_sealed_mutation("module_class_binding"):
                namespace["Packet"] = ReplacementPacket
            assert namespace["Packet"] is original_packet
            outcomes["rebound_owner"] = namespace["read_uniform"](
                namespace["Packet"]("replacement")
            )

            owner = namespace["Packet"]("old")
            watched = namespace["FinalizingValue"](owner)
            namespace["write_uniform"](owner, watched)
            del watched
            namespace["EVENTS"].clear()
            namespace["write_uniform"](owner, "visible-during-finalizer")
            gc.collect()
            outcomes["finalizer"] = namespace["EVENTS"][:]

            retained = namespace["Packet"]("retained")
            reference = weakref.ref(retained)
            before = sys.getrefcount(retained)
            assert namespace["read_uniform"](retained) == "retained"
            outcomes["receiver_refcount"] = sys.getrefcount(retained) == before
            del retained
            gc.collect()
            outcomes["receiver_released"] = reference() is None
            return outcomes

        assert_ordinary_bindings(stock_module)
        expected = exercise(stock)
        assert_ordinary_bindings(stock_module)
        actual = exercise_strict(module.__dict__)
        if mode != "profile":
            assert expected["getattribute"] == ["observed:get"], expected
            assert expected["setattr"] == [("observed:set", "changed")], expected
            assert expected["mro"] == ("alternate", ["alternate:get"]), expected
            assert expected["finalizer"] == [
                ("finalized", "visible-during-finalizer")
            ], expected
            assert expected["receiver_refcount"], expected
            assert expected["receiver_released"], expected
            # Ordinary-only mutations still execute on the dynamic descendants.
            # The original strict-base adoption and sealed module rebinding are
            # rejected; every other original outcome still matches stock.
            strict_expected = dict(expected)
            strict_expected["mro"] = ("inherited", [])
            assert actual == strict_expected, (expected, actual)
            assert sealed_rejections == [
                "strict_base_adoption", "module_class_binding",
            ]
        else:
            assert actual == expected == "profile-controls-pass"
            assert sealed_rejections == []
        """
    )
    script = (
        script.replace("__ROOT__", repr(str(tmp_path)))
        .replace("__NAME__", repr(module_name))
        .replace("__MODE__", repr(mode))
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
    'Left.__init__', 'Right.__init__', 'Third.__init__', 'Fourth.__init__',
    'Observed.__getattribute__', 'Observed.__setattr__',
)

def assert_native_function(function, path=None):
    assert type(function) is types.FunctionType, (path, type(function))
    path = function.__qualname__ if path is None else path
    actual = (path, unchecked_target_id(function), sealed_id(function),
        native_owner(function), native_metadata(function),
        _soac_ext.strict_function_entry_kind(function))
    assert actual[1] == 0, actual
    # The exact MutableBase/custom-hook methods remain owned checked entries;
    # their automatic class fallback does not grant a permanent method seal.
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
    for name in ('Packet', 'Unique', 'MixedLeft', 'MixedRight', 'OverOne', 'OverTwo', 'OverThree', 'OverFour', 'OverFive', 'OverSix', 'Slotted', 'Unanchored', 'Cold', 'Consumer', 'AlternateBase', 'FinalizingValue')}}

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
    # Base is the explicit ordinary dependency. Its descendants automatically
    # stay dynamic, while their selected source functions remain checked above.
    # Existing profiled field guards still have to honor their actual mutations.
    for name in ('Base', 'Left', 'Right', 'Third', 'Fourth', 'Observed'):
        cls = module.__dict__[name]
        assert type_owner(cls) is None, name
        assert type_sealed(cls) == 0, name
    for name, field in (('Slotted', 'slotted'),):
        assert_object_slot(module.__dict__[name], field)
    assert module.Base.__module__ == {f"{module_name}_ordinary_base"!r}
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
    result = project.run(
        program, opt_mode=mode, extra_env={"SOAC_WORK_DIR": str(work_dir)},
        timeout=90, check=False,
    )
    assert result.returncode == 0, (
        f"{mode} uniform-polymorphic-field subprocess failed:\n"
        f"{result.stdout}{result.stderr}"
    )


def _field_rows(records: list[dict], qualname: str, branch: str) -> list[tuple]:
    selected = {}
    for record in records:
        for row in record["rows"]:
            if row["kind"] != "field_access" or row["function_qualname"] != qualname:
                continue
            key = (row["function_id"], row["instr_id"], row["counter_id"])
            if key not in selected or row["value"] > selected[key]["value"]:
                selected[key] = row
    return sorted(
        (row["instr_id"], row.get("branches", {}).get(branch, 0))
        for row in selected.values()
    )


def test_base_field_initialized_only_by_subclasses_rejects_strict_source(
    tmp_path: Path,
) -> None:
    # The base method's receiver does not acquire a link field merely because
    # subclasses write one. Keep the checker's rejection of this exact source.
    relative = "subclass_initialized_base_field.py"
    rejected = assert_strict_source_rejected(
        tmp_path / "rejected-source",
        strict_opt_in(_SOURCE.encode(), relative)[0].decode(),
        module_name="subclass_initialized_base_field",
        diagnostic="unresolved-attribute",
    )
    assert "`Self@read_self` has no attribute `link`" in rejected, rejected


def test_strict_uniform_fields_preserve_checked_execution_and_ordinary_interoperability(
    tmp_path: Path,
) -> None:
    module_name = "uniform_polymorphic_nonself_fields_case"
    original = textwrap.dedent(_SOURCE)
    (tmp_path / f"{module_name}.py").write_text(original)
    # Preserve the complete original stock source and validator. Only the exact
    # unsupported Base body moves to an ordinary dependency; every other source
    # function is selected. This intentionally tests interoperability rather
    # than claiming that the rejected all-selected source has been admitted.
    base_name = f"{module_name}_ordinary_base"
    base = next(
        node for node in ast.parse(original).body
        if isinstance(node, ast.ClassDef) and node.name == "Base"
    )
    lines = original.splitlines(keepends=True)
    base_source = "".join(lines[base.lineno - 1:base.end_lineno])
    selected_source = "".join(
        lines[:base.lineno - 1]
        + [f"from {base_name} import Base\n"]
        + ["\n"] * (base.end_lineno - base.lineno)
        + lines[base.end_lineno:]
    )
    relative = f"{module_name}.py"
    project = create_strict_project(
        tmp_path / "strict-project",
        {
            relative: strict_opt_in(selected_source.encode(), relative)[0].decode(),
            f"{base_name}.py": base_source,
        },
        modules={module_name: relative},
    )
    work_dir = tmp_path / "soac-work"

    _run_mode(project, tmp_path, module_name, work_dir, "profile")

    from soac import _soac_ext

    profile = json.loads(_soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin")))
    records = [row for row in profile["records"] if row["module_name"] == module_name]
    assert records

    owners = {
        entry["type_id"]: entry["qualname"]
        for record in profile["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
    }
    keys = {
        (owners[key["owner_type_id"]], key["key"], key["index"])
        for record in profile["records"]
        for key in record["type_keys"]
        if key["owner_type_id"] in owners
    }
    uniform = {(owner, index) for owner, key, index in keys if key == "link"}
    assert uniform == {
        ("Left", 0), ("Right", 0), ("Third", 0), ("Fourth", 0), ("Packet", 0)
    }, uniform
    mixed = {(owner, index) for owner, key, index in keys if key == "mixed"}
    assert mixed == {("MixedLeft", 0), ("MixedRight", 1)}, mixed
    overflow = {(owner, index) for owner, key, index in keys if key == "overflow"}
    assert len(overflow) == 6 and {index for _, index in overflow} == {0}, overflow

    for qualname, minimum in (
        ("read_uniform", 320), ("Consumer.read", 160),
        ("read_mixed", 64), ("read_overflow", 192),
        ("read_unique", 32),
    ):
        rows = _field_rows(records, qualname, "generic_getattr")
        assert any(count >= minimum for _, count in rows), (qualname, minimum, rows)
    stores = _field_rows(records, "write_uniform", "generic_setattr")
    assert any(count >= 160 for _, count in stores), stores

    _run_mode(project, tmp_path, module_name, work_dir, "verify")
    verify = json.loads(_soac_ext.inspect_counter_dump_json(str(work_dir / "verify.bin")))
    verified = [row for row in verify["records"] if row["module_name"] == module_name]
    assert verified

    _run_mode(project, tmp_path, module_name, work_dir, "apply")

    # Authenticated source methods bind CompleteFunctionDefinition results,
    # while the existing optional late-owner catalog recognizes only bare
    # MakeFunctionWithClosure bindings. These actual checked bodies therefore
    # stay generic; a zero indexed-hit count is not a taken indexed fallback.
    # Positive unique/mixed/five-owner selection and its bounds remain covered
    # by pipeline_v3's hot_nonself_* structured planner tests; the JIT's
    # uniform_polymorphic_late_owner_loads_share_one_live_split_key_probe test
    # independently preserves the selected five-owner codegen shape.
    for qualname, minimum in (
        ("read_uniform", 320), ("Consumer.read", 160),
        ("read_mixed", 64), ("read_overflow", 192),
        ("read_unique", 32), ("read_slotted", 32),
        ("read_unanchored", 32), ("read_cold", 1),
    ):
        rows = _field_rows(verified, qualname, "generic_getattr")
        assert any(count >= minimum for _, count in rows), (qualname, minimum, rows)
    stores = _field_rows(verified, "write_uniform", "generic_setattr")
    assert any(count >= 160 for _, count in stores), stores

    inherited = _field_rows(verified, "Base.read_self", "indexed_hit")
    assert inherited == [], (
        "the explicitly ordinary Base.read_self has no selected field body; "
        "its original inherited reads are exercised by both behavior controls",
        inherited,
    )
    for qualname in (
        "read_uniform", "Consumer.read", "read_mixed", "read_unique",
        "read_overflow", "read_slotted", "read_unanchored",
        "read_cold", "write_uniform",
    ):
        for branch in ("indexed_hit", "indexed_fallback"):
            rows = _field_rows(verified, qualname, branch)
            assert rows and all(count == 0 for _, count in rows), (qualname, branch, rows)

    native = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text().splitlines()
        if line.strip()
    ]
    for qualname in (
        "read_uniform", "Consumer.read", "read_mixed", "read_unique"
    ):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == qualname
            for row in native
        ), (qualname, native)
    assert not any(
        row.get("function_qualname") == "Base.read_self" for row in native
    ), native
