"""Checked fields through real checker artifacts and native class ownership."""

import hashlib
import json
import textwrap
from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project

_CHECKED = """
# soac: module(strict_assign=true, checked_attr=true)
from typing import Any

class Checked:
    def __init__(self, initial: int = 1):
        self.number: int = initial
        self.flag: bool = False
        self.none_only: None = None
        self.maybe: str | None = None
        self.choice: int | str | None = None
        self.widened: float = 0.0
        self.inferred = initial

    def store_number(self, value: Any) -> None:
        self.number = value

    def store_choice(self, value: Any) -> None:
        self.choice = value

    def read_number(self):
        return self.number

    def read_inferred(self):
        return self.inferred

class SlotChecked:
    __slots__ = ('number', '__weakref__')

    def __init__(self, initial: int = 1):
        self.number: int = initial

    def store_number(self, value: Any) -> None:
        self.number = value

    def read_number(self):
        return self.number

class Defaults:
    number: int = 10

    def read_number(self):
        return self.number

class PredicateFree:
    # Participation does not turn Any or inferred declarations into predicates.
    def __init__(self, initial=1):
        self.payload: Any = initial
        self.inferred = initial

def make_reader(initial):
    class Reader:
        def __init__(self):
            self.value = initial
        def read(self):
            return self.value
    return Reader
"""

_UNCHECKED_BASE = """
# soac: module(strict_assign=true, checked_attr=true)

# soac: class(checked_attr=false)
class UncheckedBase:
    def __init__(self, initial: int = 1):
        self.inferred = initial
        self.annotation_opted_out: int = initial
"""

_DISABLED_CHILD = """
# soac: module(strict_assign=true, checked_attr=true)
from checked import Checked

# soac: class(checked_attr=false)
class DisabledChild(Checked):
    def __init__(self):
        super().__init__()
        self.own: int = 3
"""

_ENABLED_CHILD = """
# soac: module(strict_assign=true, checked_attr=true)
from unchecked_base import UncheckedBase

class EnabledChild(UncheckedBase):
    def __init__(self):
        super().__init__()
        self.own: int = 4
"""


@pytest.fixture(scope="module")
def checked_fields(request, tmp_path_factory):
    backend = getattr(request, "param", "soac")
    return create_strict_project(
        tmp_path_factory.mktemp(f"strict-checked-fields-{backend}"),
        {
            "checked.py": _CHECKED,
            "unchecked_base.py": _UNCHECKED_BASE,
            "disabled_child.py": _DISABLED_CHILD,
            "enabled_child.py": _ENABLED_CHILD,
        },
        modules={
            "checked": "checked.py",
            "unchecked_base": "unchecked_base.py",
            "disabled_child": "disabled_child.py",
            "enabled_child": "enabled_child.py",
        },
        backend=backend,
    )


_PRELUDE = """
import ctypes
import checked
import disabled_child
import enabled_child
import unchecked_base

def api(name, count, result=ctypes.c_int):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object] * count
    function.restype = result
    return function

is_sealed = api('PyType_IsSoacSealed', 1)
has_policy = api('PyDict_HasSoacPolicy', 1)
field_index = api('_PyDict_IndexedKeyIndex', 2, ctypes.c_ssize_t)
no_aliases = api('_PyDict_HasNoLookupAliases', 1)
native_setattr = api('PyObject_SetAttr', 3)
native_generic_setattr = api('PyObject_GenericSetAttr', 3)
native_explicit_setattr = api('_PyObject_GenericSetAttrWithDict', 4)
native_setitem = api('PyDict_SetItem', 3)

def c_setattr(receiver, name, value):
    return native_setattr(*(ctypes.py_object(item) for item in (receiver, name, value)))

def c_generic_setattr(receiver, name, value):
    return native_generic_setattr(*(ctypes.py_object(item) for item in (receiver, name, value)))

def c_explicit_setattr(receiver, name, value):
    return native_explicit_setattr(*(ctypes.py_object(item) for item in (receiver, name, value, vars(receiver))))

def c_setitem(dictionary, key, value):
    return native_setitem(*(ctypes.py_object(item) for item in (dictionary, key, value)))

def assert_ordinary_dictionary(dictionary):
    assert type(dictionary) is dict
    # This native query accepts only an indexed table and an exact string key.
    # A literal exact key leaves the ordinary layout as the sole TypeError case.
    try:
        field_index(dictionary, 'number')
    except TypeError as error:
        assert type(error) is TypeError
    else:
        raise AssertionError('ordinary source storage acquired an indexed table')
    # This is an indexed-table guard, not a general no-alias predicate.
    assert no_aliases(dictionary) == 0

def required_error(operation):
    try:
        operation()
    except TypeError:
        return
    raise AssertionError('required field value check was skipped')

def storage(receiver):
    assert is_sealed(type(receiver)) == 1, 'fixture fell back to an ordinary class'
    dictionary = vars(receiver)
    assert type(dictionary) is dict and has_policy(dictionary) == 1
    return dictionary
"""


_CPYTHON_FIELD_WITNESSES = """
from soac import _soac_ext
from tests._strict_integration import (
    _assert_cpython_function_witness, _assert_cpython_module_witness,
)
from tests.test_strict_type_native import ConstructionInfoV1

get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
get_type_owner.argtypes = [ctypes.py_object]
get_type_owner.restype = ctypes.c_void_p
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int

for name, module, classes, participates in (
    ("checked", checked, (checked.Checked, checked.Defaults, checked.PredicateFree), True),
    ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,), False),
    ("disabled_child", disabled_child, (disabled_child.DisabledChild,), False),
    ("enabled_child", enabled_child, (enabled_child.EnabledChild,), False),
):
    source_path, source_sha256 = expected_modules[name]
    diagnostic = _assert_cpython_module_witness(
        module, module_name=name, source_path=source_path,
        source_sha256=source_sha256, artifact_generation=expected_generation,
    )
    for cls in classes:
        if not participates:
            assert is_sealed(cls) == 0 and get_type_owner(cls) is None
            continue
        info = ConstructionInfoV1()
        assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
        assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
        assert info.phase == 3 and info.permanent_contract_published == 1
        assert info.owner == get_type_owner(cls) and info.owner is not None
    for function in {
        "checked": (
            checked.Checked.__init__,
            checked.Checked.store_number,
            checked.Checked.store_choice,
            checked.Checked.read_number,
            checked.Defaults.read_number,
            checked.PredicateFree.__init__,
        ),
        "unchecked_base": (unchecked_base.UncheckedBase.__init__,),
        "disabled_child": (disabled_child.DisabledChild.__init__,),
        "enabled_child": (enabled_child.EnabledChild.__init__,),
    }[name]:
        observed = _assert_cpython_function_witness(
            function, diagnostic,
        )
        assert observed["finalized"] is participates
"""


def run(project, program, **options):
    if project.backend != "cpython":
        return project.run(_PRELUDE + textwrap.dedent(program), **options)
    expected_modules = {
        name: (
            str(project.project / relative),
            hashlib.sha256((project.project / relative).read_bytes()).hexdigest(),
        )
        for name, relative in project.modules.items()
    }
    witness_inputs = (
        f"expected_modules = {expected_modules!r}\n"
        f"expected_generation = {project.publication['generation']!r}\n"
    )
    return project.run_case(
        "checked",
        _PRELUDE + witness_inputs + _CPYTHON_FIELD_WITNESSES
        + textwrap.dedent(program) + _CPYTHON_FIELD_WITNESSES,
        Path(__file__), backend="cpython", **options,
    )


# Retained harness: Shares the generated profile directory between profile and apply runs;
# independent scenario blocks do not model that artifact lifecycle.
def test_warmed_field_stores_remain_checked_in_profile_and_apply(
    checked_fields, tmp_path
):
    work = tmp_path / "profile-and-apply"
    environment = {"SOAC_WORK_DIR": str(work)}
    training = """
        value = checked.Checked()
        dictionary = storage(value)
        def stock_store(receiver, item):
            receiver.number = item
        for number in range(2000):
            stock_store(value, number)
            value.store_number(number)
        assert value.number == 1999 and dictionary['number'] == 1999
    """
    run(checked_fields, training, opt_mode="profile", extra_env=environment)
    assert (work / "profile.bin").is_file()
    run(
        checked_fields,
        textwrap.dedent(training)
        + """
required_error(lambda: stock_store(value, 'bad after CPython warmup'))
required_error(lambda: value.store_number('bad after SOAC profile training'))
required_error(lambda: c_setitem(dictionary, 'number', 'bad after optimized calls'))
assert value.number == 1999
del value.number
class Alias:
    def __hash__(self):
        return hash('number')
    def __eq__(self, other):
        return other == 'number'
alias = Alias()
dictionary[alias] = 3
required_error(lambda: value.store_number('bad after lookup-alias guard changed'))
value.store_number(17)
assert value.number == 17 and list(dictionary)[-1] is alias
""",
        opt_mode="apply",
        extra_env=environment,
    )


# Retained harness: Checks profile/verify/apply behavior and retained work artifacts across
# modes; keep the explicit artifact harness.
@pytest.mark.parametrize("class_name", ("Checked", "SlotChecked"), ids=("inline", "slots"))
def test_unmaterialized_checked_field_stores_keep_policy_in_profile_verify_and_apply(
    checked_fields, tmp_path, class_name
):
    from tests._integration import exec_integration_validation, stock_module
    from tests._strict_integration import StrictValidationCase, _VALIDATION_PRELUDE

    body = textwrap.dedent(
        """
        import ctypes
        import gc
        import json
        import os
        import weakref
        import _testinternalcapi
        from soac import _soac_ext

        def api(name, result):
            function = getattr(ctypes.pythonapi, name)
            function.argtypes = [ctypes.py_object]
            function.restype = result
            return function

        ordinary_writes = api('_PySOAC_HasOrdinaryInstanceWrites', ctypes.c_int)
        slot_writes = api('_PySOAC_UsesObjectSlotPolicy', ctypes.c_int)
        type_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p)
        sealed = api('PyType_IsSoacSealed', ctypes.c_int)
        source_id = api('PyFunction_GetSoacStrictId', ctypes.c_uint64)
        function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p)
        metadata = api('PyFunction_GetSoacMetadata', ctypes.c_void_p)
        info = _testinternalcapi.get_soac_type_state_info
        strict = __dp_integration_strict__
        selected_class = getattr(module, class_name)
        slot_case = class_name == 'SlotChecked'
        write_policy = slot_writes if slot_case else ordinary_writes

        functions = (
            selected_class.__init__, selected_class.store_number,
            selected_class.read_number,
        )
        def function_snapshot(function):
            return (
                function.__code__, source_id(function), function_owner(function),
                metadata(function), _soac_ext.strict_function_entry_kind(function),
            )
        bindings = tuple(function_snapshot(function) for function in functions)
        for binding in bindings:
            if strict:
                assert all(binding[1:4]) and binding[4] == 'checked_native', binding
            else:
                assert binding[1:] == (0, None, None, None), binding
        checked_owner = type_owner(selected_class)
        assert bool(checked_owner) == strict
        assert sealed(selected_class) == write_policy(selected_class) == int(strict)
        assert ordinary_writes(selected_class) == int(strict and not slot_case)
        assert slot_writes(selected_class) == int(strict and slot_case)
        if strict:
            print(json.dumps({
                'store_source_id': bindings[1][1], 'read_source_id': bindings[2][1],
            }), flush=True)

        check_bad_write = not strict or os.environ.get('SOAC_OPT_MODE') != 'profile'
        classes = [selected_class]
        if check_bad_write:
            class OrdinaryChild(selected_class):
                pass
            # There is no own selected contract on this ordinary subclass.
            # The actual inherited dictionary or slot policy governs its write.
            assert type_owner(OrdinaryChild) is None and sealed(OrdinaryChild) == 0
            assert write_policy(OrdinaryChild) == int(strict)
            classes.append(OrdinaryChild)

        for cls in classes:
            value = cls()
            reference = weakref.ref(value)
            state = info(value)
            if not slot_case:
                assert _testinternalcapi.has_inline_values(value)
            if strict and cls is selected_class:
                assert state['has_slot'] and state['storage_mode'] == 'direct'
                assert state['state_id']
                if slot_case:
                    assert state['native_slot_count'] == 1
                    assert state['dictionary_state_id'] is None
                else:
                    assert state['dictionary_state_id']
                assert not state['terminal']
            owner_before = type_owner(cls)

            # No vars(value), __dict__, storage(value), or dictionary-pointer
            # accessor runs before these stores or the rejected replacement.
            # has_inline_values observes validity, not dictionary-pointer NULL.
            for number in range(2000):
                selected_class.store_number(value, number)
            previous = selected_class.read_number(value)
            assert previous == 1999
            if not slot_case:
                assert _testinternalcapi.has_inline_values(value)
            assert info(value) == state and type_owner(cls) == owner_before

            if check_bad_write:
                released = []
                events = []
                class Invalid:
                    def __del__(self):
                        released.append('invalid')
                invalid = Invalid()
                invalid_reference = weakref.ref(invalid)
                def replacement():
                    events.append('value')
                    return invalid

                try:
                    # The existing method's value parameter is Any; only the
                    # selected self.number storage write can reject this value.
                    selected_class.store_number(value, replacement())
                except TypeError:
                    assert strict
                    events.append('rejected')
                else:
                    events.append('stored')
                assert events == ['value', 'rejected' if strict else 'stored'], events
                assert selected_class.read_number(value) is (previous if strict else invalid)
                assert type(value) is cls and type_owner(cls) == owner_before
                assert write_policy(cls) == int(strict)
                assert info(value) == state
                if not slot_case:
                    assert _testinternalcapi.has_inline_values(value)

                selected_class.store_number(value, 17)
                assert selected_class.read_number(value) == 17
                del invalid
                gc.collect()
                assert invalid_reference() is None and released == ['invalid']

            del value
            gc.collect()
            assert reference() is None
        assert type_owner(selected_class) == checked_owner
        assert tuple(function_snapshot(function) for function in functions) == bindings
        """
    )
    body = f"class_name = {class_name!r}\n" + body
    validation = "def validate_module(module):\n" + textwrap.indent(body, "    ")
    source = _CHECKED.replace("# soac: module(strict_assign=true, checked_attr=true)\n", "", 1)
    with stock_module(tmp_path / "ordinary", "ordinary_unmaterialized_fields", source) as ordinary:
        exec_integration_validation(validation, ordinary, Path(__file__), mode="stock")

    case = StrictValidationCase(
        validation, Path(__file__),
        required_functions=tuple(
            f"{class_name}.{method}" for method in ("__init__", "store_number", "read_number")
        ),
    )
    work = tmp_path / "unmaterialized-store-profile"

    def replay(mode):
        return checked_fields.run(
            _VALIDATION_PRELUDE + checked_fields._validation_program(
                "checked", case, entry_interpreter=False,
            ),
            opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(work)},
            check=False,
        )

    from soac import _soac_ext

    def field_rows(dump, actual_source_id, method):
        rows = {}
        for record in dump["records"]:
            if record["module_name"] != "checked":
                continue
            for row in record["rows"]:
                if (
                    row["kind"] == "field_access"
                    and row["function_qualname"] == f"{class_name}.{method}"
                    and row["function_id"] == actual_source_id
                ):
                    key = (row["instr_id"], row["counter_id"])
                    if key not in rows or row["value"] > rows[key]["value"]:
                        rows[key] = row
        return list(rows.values())

    profiled = replay("profile")
    assert profiled.returncode == 0, profiled.stdout + profiled.stderr
    profile_source_ids = json.loads(profiled.stdout)
    profile = json.loads(_soac_ext.inspect_counter_dump_json(str(work / "profile.bin")))
    trained = field_rows(profile, profile_source_ids["store_source_id"], "store_number")
    trained_reads = field_rows(profile, profile_source_ids["read_source_id"], "read_number")
    assert len(trained_reads) == 1 and trained_reads[0]["value"] >= 1, trained_reads
    assert len(trained) == 1 and trained[0]["branches"]["generic_setattr"] >= 2000, trained
    keys = set()
    if class_name == "Checked":
        owners = {
            entry["type_id"]
            for record in profile["records"]
            for entry in record["type_table"]
            if entry["module_name"] == "checked" and entry["qualname"] == "Checked"
        }
        keys = {
            (entry["owner_type_id"], entry["key"], entry["index"])
            for record in profile["records"]
            for entry in record["type_keys"]
            if entry["owner_type_id"] in owners and entry["key"] == "number"
        }
        assert len(keys) == 1, (owners, keys)

    verified = replay("verify")
    observed = []
    observed_reads = []
    if (work / "verify.bin").is_file() and verified.stdout.strip():
        verify_source_ids = json.loads(verified.stdout)
        verify = json.loads(_soac_ext.inspect_counter_dump_json(str(work / "verify.bin")))
        observed = field_rows(verify, verify_source_ids["store_source_id"], "store_number")
        observed_reads = field_rows(verify, verify_source_ids["read_source_id"], "read_number")
    # Retain actual reachability evidence even if the invalid store failed the
    # semantic assertion. A generic-only replay is not an indexed-path proof.
    (work / "store-path-evidence.json").write_text(json.dumps({
        "class_name": class_name,
        "profile_store_rows": trained,
        "profile_read_rows": trained_reads,
        "profile_type_keys": sorted(keys),
        "verify_store_rows": observed,
        "verify_read_rows": observed_reads,
        "verify_returncode": verified.returncode,
    }, indent=2) + "\n")
    assert verified.returncode == 0, verified.stdout + verified.stderr + repr(observed)
    assert len(observed_reads) == 1 and observed_reads[0]["value"] >= 1, observed_reads
    assert observed_reads[0]["instr_id"] == trained_reads[0]["instr_id"]
    assert len(observed) == 1, observed
    assert observed[0]["instr_id"] == trained[0]["instr_id"]
    branches = observed[0]["branches"]
    assert branches.get("indexed_hit", 0) == 0, observed
    assert branches.get("generic_setattr", 0) + branches.get("indexed_fallback", 0) >= 2000

    applied = replay("apply")
    assert applied.returncode == 0, applied.stdout + applied.stderr


# Retained harness: Asserts structured profile/codegen/counter evidence across profile, apply
# and verify; not a source-only scenario.
def test_sealed_field_reads_keep_generic_fallback_for_ordinary_pending_storage(
    checked_fields, tmp_path
):
    work = tmp_path / "sealed-field-reads"
    training = """
        value = checked.Checked(7)
        storage(value)
        default = checked.Defaults()
        first = checked.make_reader(11)
        second = checked.make_reader(19)
        assert first is not second and is_sealed(first) and is_sealed(second)
        left, right = first(), second()
        for unused in range(250):
            assert value.read_number() == 7
            assert value.read_inferred() == 7
            assert default.read_number() == 10
            assert left.read() == 11 and right.read() == 19
    """
    training = textwrap.dedent(training) + """
from soac import _soac_ext
get_type_owner = api('PyType_GetSoacContractOwner', 1, ctypes.c_void_p)
get_function_owner = api('PyFunction_GetSoacStrictOwner', 1, ctypes.c_void_p)
first_owner, second_owner = get_type_owner(first), get_type_owner(second)
assert first_owner and second_owner and first_owner != second_owner
for method_name in ('__init__', 'read'):
    first_method, second_method = vars(first)[method_name], vars(second)[method_name]
    assert first_method is not second_method
    first_binding = get_function_owner(first_method)
    second_binding = get_function_owner(second_method)
    assert first_binding and second_binding and first_binding != second_binding
    # This query authenticates each actual function and its public entry;
    # a source-equal factory result or an optimization event is not a witness.
    assert _soac_ext.strict_function_entry_kind(first_method) == 'checked_native'
    assert _soac_ext.strict_function_entry_kind(second_method) == 'checked_native'
"""
    run(
        checked_fields,
        training,
        opt_mode="profile",
        extra_env={"SOAC_WORK_DIR": str(work)},
    )
    assert (work / "profile.bin").is_file()
    events_path = tmp_path / "sealed-field-apply.jsonl"
    validation = (
        textwrap.dedent(training)
        + """
# The field's inferred integer type never became an unboxing permission.
marker = object()
value.inferred = marker
assert value.read_inferred() is marker
default.number = 3
assert default.read_number() == 3
del default.number
assert default.read_number() == 10
dictionary = storage(value)
del value.number
try:
    value.read_number()
except AttributeError:
    pass
else:
    raise AssertionError('UNSET must use normal attribute lookup')
original = ValueError('alias lookup must run')
class Alias:
    armed = False
    def __hash__(self):
        return hash('number')
    def __eq__(self, other):
        if self.armed:
            raise original
        return False
alias = Alias()
# Ordinary split dictionaries can compare a deleted shared key on insertion.
# Arm only after setup, so this regression observes the actual field read.
dictionary[alias] = 23
assert list(dictionary)[-1] is alias
alias.armed = True
try:
    value.read_number()
except ValueError as error:
    assert error is original
else:
    raise AssertionError('the raw field path skipped alias-sensitive lookup')

events = []
class Foreign:
    @property
    def number(self):
        events.append('property')
        return marker
assert checked.Checked.read_number(Foreign()) is marker
assert events == ['property']
class OrdinaryChild(checked.Checked):
    @property
    def number(self):
        return 'ordinary override'
child = object.__new__(OrdinaryChild)
assert not is_sealed(OrdinaryChild)
assert checked.Checked.read_number(child) == 'ordinary override'

# A source-equal second construction cannot be mistaken for the first owner;
# the method's actual environment selects its own witness or generic lookup.
assert first.read(right) == 19 and second.read(left) == 11

# Inspect the storage only after the original read/callback cases, preserving
# their inline-versus-materialized setup. Type state grants no indexed layout.
for receiver in (value, default, left, right):
    assert_ordinary_dictionary(vars(receiver))
"""
    )
    run(
        checked_fields,
        validation,
        opt_mode="apply",
        extra_env={
            "SOAC_WORK_DIR": str(work),
            "SOAC_LOG": (
                "soac_jit_codegen=info,soac_specialization_runtime=info"
                f";json={events_path}"
            ),
        },
    )
    events = [json.loads(line) for line in events_path.read_text().splitlines()]
    events = [entry.get("fields", entry) for entry in events]
    emitted = {
        event["function_qualname"]: event
        for event in events
        if event.get("event") == "soac.strict_field_codegen"
    }
    for name in (
        "Checked.read_number",
        "Checked.read_inferred",
        "Defaults.read_number",
        "make_reader.<locals>.Reader.read",
    ):
        assert emitted[name]["sealed_field_site_count"] == 1
        assert emitted[name]["machine_code_size_bytes"] > 0
    bound = [
        event
        for event in events
        if event.get("event") == "soac.strict_field_capabilities"
        and event.get("function_qualname") == "make_reader.<locals>.Reader.read"
    ]
    assert not bound, (
        "ordinary dictionary storage must not publish indexed field capabilities"
    )

    # Apply's committed code shape and behavior are tested above without
    # enabling counter overhead. Verify is the separate diagnostic replay
    # that records which guarded branches actually executed.
    run(
        checked_fields,
        validation,
        opt_mode="verify",
        extra_env={"SOAC_WORK_DIR": str(work)},
    )
    from soac import _soac_ext

    verification = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work / "verify.bin"))
    )
    read_paths = {
        branch
        for record in verification["records"]
        if record["module_name"] == "checked"
        for row in record["rows"]
        if row["function_qualname"] == "Checked.read_number"
        and row["kind"] == "field_access"
        for branch, value in row["branches"].items()
        if value > 0
    }
    assert "indexed_hit" not in read_paths and "indexed_fallback" in read_paths
