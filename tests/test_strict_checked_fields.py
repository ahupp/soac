"""Checked fields through real checker artifacts and native class ownership."""

import hashlib
import json
import textwrap
from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project

_CHECKED = """
from __future__ import strict
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

def make_reader(initial):
    class Reader:
        def __init__(self):
            self.value = initial
        def read(self):
            return self.value
    return Reader
"""

_UNCHECKED_BASE = """
from __future__ import strict

class UncheckedBase:
    def __init__(self, initial: int = 1):
        self.inferred = initial
        self.annotation_disabled: int = initial
"""

_DISABLED_CHILD = """
from __future__ import strict
from checked import Checked

class DisabledChild(Checked):
    def __init__(self):
        super().__init__()
        self.own: int = 3
"""

_ENABLED_CHILD = """
from __future__ import strict
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
        policy="""
[tool.soac.strict]
include = ["checked.py", "unchecked_base.py", "disabled_child.py", "enabled_child.py"]
checked_fields = "supported_annotations"

[[tool.soac.strict.overrides]]
include = ["unchecked_base.py", "disabled_child.py"]
checked_fields = "disabled"
""",
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

for name, module, classes in (
    ("checked", checked, (checked.Checked, checked.Defaults)),
    ("unchecked_base", unchecked_base, (unchecked_base.UncheckedBase,)),
    ("disabled_child", disabled_child, (disabled_child.DisabledChild,)),
    ("enabled_child", enabled_child, (enabled_child.EnabledChild,)),
):
    source_path, source_sha256 = expected_modules[name]
    diagnostic = _assert_cpython_module_witness(
        module, module_name=name, source_path=source_path,
        source_sha256=source_sha256, artifact_generation=expected_generation,
    )
    for cls in classes:
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
        ),
        "unchecked_base": (unchecked_base.UncheckedBase.__init__,),
        "disabled_child": (disabled_child.DisabledChild.__init__,),
        "enabled_child": (enabled_child.EnabledChild.__init__,),
    }[name]:
        observed = _assert_cpython_function_witness(
            function, diagnostic,
        )
        assert observed["finalized"]
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


@pytest.mark.parametrize(
    ("checked_fields", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["checked_fields"],
    scope="module",
)
def test_explicit_fields_check_without_coercion_or_inferred_requirements(
    checked_fields, entry_interpreter
):
    program = """
        value = checked.Checked()
        dictionary = storage(value)
        assert_ordinary_dictionary(dictionary)
        class Integer(int):
            pass
        for accepted in (True, 17, Integer(19), 10 ** 100):
            value.number = accepted
            assert value.number is accepted
            value.widened = accepted
            assert value.widened is accepted
        floating = float('2.5')
        value.widened = floating
        assert value.widened is floating
        for name, invalid in [('number', 1.5), ('flag', 1), ('none_only', 0),
                              ('maybe', 1), ('choice', []), ('widened', '2.5')]:
            previous = getattr(value, name)
            required_error(lambda: setattr(value, name, invalid))
            assert getattr(value, name) is previous
        for accepted in (None, 'text'):
            value.maybe = accepted
            assert value.maybe is accepted
        for accepted in (None, 'text', 8, True):
            value.store_choice(accepted)
            assert value.choice is accepted
        required_error(lambda: value.store_choice([]))
        required_error(lambda: value.store_number('bad through a transformed method'))
        assert dictionary is vars(value)
        assert_ordinary_dictionary(dictionary)

        # This ordinary field has an inferred integer type, but the source
        # never selected a mandatory annotation or indexed storage for it.
        marker = object()
        value.inferred = marker
        assert value.inferred is marker
        dictionary['inferred'] = 'still unchecked'
        assert value.inferred == 'still unchecked'

        defaults = checked.Defaults()
        default_storage = storage(defaults)
        assert defaults.number == 10 and default_storage == {}
        required_error(lambda: setattr(defaults, 'number', 'bad default shadow'))
        defaults.number = 2
        del defaults.number
        assert defaults.number == 10 and default_storage == {}
        """
    if checked_fields.backend == "cpython":
        program = textwrap.dedent(program) + """
# Repeated original-code operations must retain checks without SOAC compilation.
warmed = checked.Checked()
for item in range(128):
    warmed.store_number(item)
    assert warmed.read_number() == item
required_error(lambda: warmed.store_number("bad after native warmup"))
assert warmed.number == 127
required_error(lambda: c_generic_setattr(warmed, "number", "bad generic C write"))
assert c_generic_setattr(warmed, "number", 129) == 0
assert warmed.read_number() == 129
"""
    run(checked_fields, program, entry_interpreter=entry_interpreter)


@pytest.mark.parametrize(
    ("checked_fields", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["checked_fields"],
    scope="module",
)
def test_inheritance_keeps_actual_base_checks_without_retroactive_upgrade(
    checked_fields, entry_interpreter
):
    run(
        checked_fields,
        """
        child = disabled_child.DisabledChild()
        child_storage = storage(child)
        base_storage = storage(checked.Checked())
        assert_ordinary_dictionary(child_storage)
        assert_ordinary_dictionary(base_storage)
        required_error(lambda: setattr(child, 'number', 'base contract still applies'))
        required_error(lambda: child_storage.__setitem__('choice', []))
        child.number = True
        child.own = 'this declaration belongs to the disabled child module'
        assert child.number is True and isinstance(child.own, str)

        enabled = enabled_child.EnabledChild()
        enabled_storage = storage(enabled)
        unchecked = unchecked_base.UncheckedBase()
        assert is_sealed(type(unchecked)) == 1
        unchecked_storage = vars(unchecked)
        # Its module explicitly disables fields and it has no checked base.
        # Permanent class sealing does not require an empty instance policy.
        assert type(unchecked_storage) is dict and has_policy(unchecked_storage) == 0
        assert_ordinary_dictionary(enabled_storage)
        assert_ordinary_dictionary(unchecked_storage)
        for name in ('inferred', 'annotation_disabled'):
            setattr(enabled, name, 'no retroactive source requirement')
            assert getattr(enabled, name) == 'no retroactive source requirement'
            ordinary_value = object()
            setattr(unchecked, name, ordinary_value)
            assert getattr(unchecked, name) is ordinary_value
            native_value = object()
            assert c_generic_setattr(unchecked, name, native_value) == 0
            assert getattr(unchecked, name) is native_value
            dictionary_value = object()
            assert c_setitem(unchecked_storage, name, dictionary_value) == 0
            assert getattr(unchecked, name) is dictionary_value
        assert vars(unchecked) is unchecked_storage and has_policy(unchecked_storage) == 0
        required_error(lambda: setattr(enabled, 'own', 'own explicit annotation is selected'))
        enabled.own = 11
        assert enabled.own == 11
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("checked_fields", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["checked_fields"],
    scope="module",
)
def test_attribute_unicode_payload_and_original_lookup_errors(
    checked_fields, entry_interpreter
):
    run(
        checked_fields,
        """
        operations = (setattr, object.__setattr__, c_setattr,
                      c_generic_setattr, c_explicit_setattr)
        for operation in operations:
            value = checked.Checked()
            dictionary = storage(value)
            original_items = tuple(dictionary.items())
            del value.number
            events = []
            class Name(str):
                def __hash__(self):
                    events.append('hash')
                    return str.__hash__(self)
                def __eq__(self, other):
                    events.append('eq')
                    return str.__eq__(self, other)
                def __str__(self):
                    raise AssertionError('checking invoked user string conversion')
            name = Name('number')
            class Ordinary:
                pass
            ordinary = Ordinary()
            # Equal contents alone omit the deleted name in shared-key history.
            # Reproduce original insertion order, materialization, then deletion.
            for attr_name, original_value in original_items:
                setattr(ordinary, attr_name, original_value)
            ordinary_dictionary = vars(ordinary)
            del ordinary.number
            assert ordinary_dictionary == dictionary
            operation(ordinary, name, 'ordinary value')
            expected = list(events)
            events.clear()
            required_error(lambda: operation(value, name, 'bad subclass-name value'))
            assert events == expected, (events, expected)
            assert 'number' not in dictionary
            events.clear()
            operation(value, name, 7)
            assert events == expected, (events, expected)
            assert list(dictionary)[-1] is name and not no_aliases(dictionary)

            original = ValueError('name hash failure')
            class BadHash(str):
                def __hash__(self):
                    raise original
            try:
                operation(value, BadHash('number'), 'bad value')
            except ValueError as error:
                assert error is original
            else:
                raise AssertionError('native name hash failure was replaced')

        # The stored alias is resolved by the dictionary, after descriptor
        # lookup. Its error must precede a required field-value error, including
        # through the transformed source method's own attribute store.
        for operation in (*operations, lambda obj, name, item: obj.store_number(item)):
            value = checked.Checked()
            dictionary = storage(value)
            del value.number
            events = []
            original = ValueError('original equality failure')
            reject_lookup = False
            class Alias:
                def __hash__(self):
                    return hash('number')
                def __eq__(self, other):
                    events.append('eq')
                    if reject_lookup:
                        raise original
                    return other == 'number'
            alias = Alias()
            # Shared-key insertion may compare the deleted canonical name.
            # Arm the intended lookup failure only after the alias exists.
            dictionary[alias] = 1
            reject_lookup = True
            for unused in range(2):
                events.clear()
                try:
                    operation(value, 'number', 'bad field value')
                except ValueError as error:
                    assert error is original
                else:
                    raise AssertionError('lookup exception was hidden by a value precheck')
                assert events == ['eq'], events
                assert list(dictionary)[-1] is alias
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("checked_fields", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["checked_fields"],
    scope="module",
)
def test_mapping_aliases_and_supported_c_writes_use_the_actual_policy(
    checked_fields, entry_interpreter
):
    run(
        checked_fields,
        """
        value = checked.Checked()
        dictionary = storage(value)
        assert_ordinary_dictionary(dictionary)
        events = []
        class Alias:
            def __hash__(self):
                return hash('number')
            def __eq__(self, other):
                events.append('eq')
                return other == 'number'
        alias = Alias()
        for operation in (lambda item: dictionary.__setitem__(alias, item),
                          lambda item: c_setitem(dictionary, alias, item)):
            previous = dictionary['number']
            events.clear()
            required_error(lambda: operation('bad canonical field value'))
            assert events == ['eq'], events
            assert dictionary['number'] == previous
            assert all(type(key) is str for key in dictionary)
            events.clear()
            operation(2)
            assert events == ['eq'] and dictionary['number'] == 2
        required_error(lambda: c_setattr(value, 'number', 'bad C attribute value'))
        required_error(lambda: dictionary.update({'number': 'bad bulk value'}))
        assert dictionary.setdefault('number', 'ignored non-write') == 2
        del value.number
        required_error(lambda: dictionary.setdefault('number', 'now a checked insert'))
        assert_ordinary_dictionary(dictionary)

        # A new arbitrary mapping key is not normalized into an attribute name.
        # Once it aliases reads, a source attribute write still checks its
        # original name; a resolved attribute operation must not become SET.
        dictionary[alias] = 'ordinary alias-sensitive overflow'
        assert list(dictionary)[-1] is alias and not no_aliases(dictionary)
        for operation in (lambda: setattr(value, 'number', 'bad attribute value'),
                          lambda: value.store_number('bad transformed attribute value'),
                          lambda: c_setattr(value, 'number', 'bad C attribute value')):
            events.clear()
            required_error(operation)
            assert events == ['eq'], events
        value.store_number(9)
        assert value.number == 9 and list(dictionary)[-1] is alias

        copied = dictionary.copy()
        assert type(copied) is dict and not has_policy(copied)
        copied['number'] = 'a copy has no source storage authority'
        dictionary.clear()
        assert dictionary is vars(value) and dictionary == {}
        assert_ordinary_dictionary(dictionary)
        required_error(lambda: c_setitem(dictionary, 'number', 'bad after clear'))
        value.number = 12
        assert value.number == 12
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("checked_fields", "entry_interpreter"),
    [
        pytest.param("soac", False, id="compiled"),
        pytest.param("soac", True, id="entry"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["checked_fields"],
    scope="module",
)
def test_string_subclass_dictionary_keys_keep_selected_field_checks(
    checked_fields, entry_interpreter
):
    run(
        checked_fields,
        """
        events = []
        class Name(str):
            def __str__(self):
                raise AssertionError('field checking must use the Unicode payload')
            def __hash__(self):
                events.append('hash')
                return str.__hash__(self)
            def __eq__(self, other):
                events.append('eq')
                return str.__eq__(self, other)

        writes = (
            lambda mapping, item: mapping.__setitem__('number', item),
            lambda mapping, item: c_setitem(mapping, 'number', item),
            lambda mapping, item: mapping.update({'number': item}),
            lambda mapping, item: mapping.update([('number', item)]),
            lambda mapping, item: mapping.__ior__({'number': item}),
        )
        for write in writes:
            value = checked.Checked()
            dictionary = storage(value)
            dictionary.clear()
            key = Name('number')
            dictionary[key] = 7
            assert list(dictionary)[0] is key

            # The ordinary control supplies the lookup callback schedule. The
            # selected policy must not rerun hashing, equality or conversion.
            ordinary = {key: 7}
            events.clear()
            write(ordinary, 'ordinary untyped value')
            expected_events = events.copy()
            assert ordinary['number'] == 'ordinary untyped value'
            events.clear()
            required_error(lambda: write(dictionary, 'bad selected value'))
            assert events == expected_events, (events, expected_events)
            assert dictionary['number'] == 7 and value.number == 7
            assert list(dictionary)[0] is key
            write(dictionary, 9)
            assert value.number == 9

        dictionary.clear()
        required_error(lambda: c_setitem(dictionary, Name('number'), 'bad C insert'))
        assert dictionary == {}
        required_error(lambda: dictionary.setdefault(Name('number'), 'bad insert'))
        assert dictionary == {}
        assert dictionary.setdefault(Name('number'), 11) == 11
        # A setdefault hit is a read, not a forbidden write of its default.
        assert dictionary.setdefault('number', 'unused default') == 11
        assert value.number == 11

        # Initial policy validation must check subclass keys in the actual
        # incoming dictionary, before replacing the receiver's storage.
        incoming_key = Name('number')
        rejected = {incoming_key: 'bad incoming value'}
        required_error(lambda: setattr(value, '__dict__', rejected))
        assert vars(value) is dictionary and value.number == 11
        assert list(rejected)[0] is incoming_key
        accepted = {incoming_key: 13}
        value.__dict__ = accepted
        assert vars(value) is accepted and value.number == 13
        required_error(lambda: c_setitem(accepted, 'number', 'bad after admission'))
        assert accepted['number'] == 13 and list(accepted)[0] is incoming_key
        """,
        entry_interpreter=entry_interpreter,
    )


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
    source = _CHECKED.replace("from __future__ import strict\n", "", 1)
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


def test_ordinary_deleted_field_key_can_compare_during_alias_setup(tmp_path):
    from tests._integration import stock_module

    # Use the same class source with ordinary CPython, without field policy.
    source = _CHECKED.replace("from __future__ import strict\n", "", 1)
    with stock_module(tmp_path, "ordinary_field_alias_setup", source) as module:
        value = module.Checked(7)
        dictionary = vars(value)
        del value.number
        original = ValueError("alias lookup must run")
        events = []

        class Alias:
            armed = True

            def __hash__(self):
                return hash("number")

            def __eq__(self, other):
                events.append((self.armed, other))
                if self.armed:
                    raise original
                return False

        alias = Alias()
        with pytest.raises(ValueError) as insertion:
            dictionary[alias] = 23
        assert insertion.value is original
        assert events == [(True, "number")]
        assert all(key is not alias for key in dictionary)

        alias.armed = False
        dictionary[alias] = 23
        assert list(dictionary)[-1] is alias
        alias.armed = True
        with pytest.raises(ValueError) as read:
            value.read_number()
        assert read.value is original
        assert events[-1] == (True, "number")


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


@pytest.mark.parametrize(
    ("checked_fields", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["checked_fields"],
    scope="module",
)
def test_fresh_checked_storage_has_shared_direct_state_and_plain_copies(
    checked_fields, entry_interpreter
):
    run(
        checked_fields,
        """
import _testinternalcapi
import gc
import weakref
info = _testinternalcapi.get_soac_type_state_info

first, second = checked.Checked(), checked.Checked()
assert _testinternalcapi.has_inline_values(first)
first_info, second_info = info(first), info(second)
assert first_info['has_slot'] and second_info['has_slot']
assert first_info['storage_mode'] == second_info['storage_mode'] == 'direct'
assert first_info['extra_slot_bytes'] == ctypes.sizeof(ctypes.c_void_p)
assert first_info['state_id'] == second_info['state_id']
assert first_info['dictionary_state_id'] == second_info['dictionary_state_id']

def store(receiver, value):
    receiver.number = value

for value in range(400):
    store(first, value)
required_error(lambda: store(first, 'wrong after warmup'))
assert first.number == 399
assert _testinternalcapi.has_inline_values(first)
dictionary, sibling = vars(first), vars(second)
assert info(dictionary)['has_slot'] and info(sibling)['has_slot']
assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
assert info(sibling)['state_id'] == first_info['dictionary_state_id']
assert info(dictionary)['storage_mode'] == 'direct'
assert_ordinary_dictionary(dictionary)
required_error(lambda: c_setitem(dictionary, 'number', 'wrong'))
assert dictionary['number'] == first.number == 399

# Force the actual dictionary tp_clear path, not merely refcount destruction
# or dict.clear(). Retiring one attachment must not clear shared rule owners.
class Marker:
    pass

third = checked.Checked()
cyclic = vars(third)
assert info(cyclic)['state_id'] == first_info['dictionary_state_id']
marker = Marker()
marker_ref, third_ref = weakref.ref(marker), weakref.ref(third)
cyclic['cycle'] = cyclic
cyclic['marker'] = marker
del marker, third, cyclic
gc.collect()
assert marker_ref() is third_ref() is None
assert info(sibling)['state_id'] == first_info['dictionary_state_id']
assert info(dictionary)['state_id'] == first_info['dictionary_state_id']
second.number = 41
c_setitem(sibling, 'number', 43)
required_error(lambda: setattr(second, 'number', 'wrong after sibling GC'))
required_error(lambda: c_setitem(sibling, 'number', 'wrong after sibling GC'))
assert second.number == 43

# The same exact Python dict type has two allocation forms. Copying values
# does not adopt the source dictionary's constraints or allocate a null tail.
plain = {}
copied = dictionary.copy()
for ordinary in (plain, copied):
    assert type(ordinary) is type(dictionary) is dict
    assert not info(ordinary)['has_slot']
    assert info(ordinary)['extra_slot_bytes'] == 0
    assert not info(ordinary)['state_id']
    assert info(ordinary)['storage_mode'] == 'ordinary'
    assert has_policy(ordinary) == 0
    ordinary['number'] = 'ordinary value'

# Even empty/atomic-valued selected storage needs its state before exposure.
empty = checked.Defaults()
assert info(empty)['has_slot'] and _testinternalcapi.has_inline_values(empty)
empty_dictionary = vars(empty)
assert not empty_dictionary and info(empty_dictionary)['has_slot']
required_error(lambda: c_setitem(empty_dictionary, 'number', 'wrong'))
assert not empty_dictionary and empty.number == 10

# No storage bit is not a waiver of an independently installed class contract.
unchecked = unchecked_base.UncheckedBase()
assert is_sealed(type(unchecked)) == 1
assert not info(unchecked)['has_slot']
assert not info(vars(unchecked))['has_slot']
unchecked.annotation_disabled = 'allowed by the original disabled policy'
from soac.strict import StrictMutationError
original_code = unchecked_base.UncheckedBase.__init__.__code__
try:
    unchecked_base.UncheckedBase.__init__.__code__ = original_code
except StrictMutationError:
    pass
else:
    raise AssertionError('no storage bit disabled the installed method seal')
assert unchecked_base.UncheckedBase.__init__.__code__ is original_code
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("checked_fields", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["checked_fields"],
    scope="module",
)
def test_type_state_keeps_legacy_replacement_and_custom_allocation_enforced(
    checked_fields, entry_interpreter
):
    run(
        checked_fields,
        """
import _testinternalcapi
import gc
import weakref
info = _testinternalcapi.get_soac_type_state_info

first, second = checked.Checked(), checked.Checked()
escaped = vars(first)
escaped_state = info(escaped)['state_id']
incoming = {'number': 7}
incoming_identity = id(incoming)
assert not info(incoming)['has_slot'] and has_policy(incoming) == 0
object.__setattr__(first, '__dict__', incoming)
object.__setattr__(second, '__dict__', incoming)
assert vars(first) is vars(second) is incoming and id(incoming) == incoming_identity
assert not info(incoming)['has_slot']
assert info(incoming)['extra_slot_bytes'] == 0
assert info(incoming)['storage_mode'] == 'legacy'
assert has_policy(incoming) == has_policy(escaped) == 1
assert info(escaped)['state_id'] == escaped_state
assert info(escaped)['storage_mode'] == 'direct'
c_setitem(incoming, 'number', 9)
assert first.number == second.number == 9
for dictionary in (incoming, escaped):
    required_error(lambda: c_setitem(dictionary, 'number', 'wrong'))
    dictionary.clear()
    required_error(lambda: c_setitem(dictionary, 'number', 'wrong after clear'))
    c_setitem(dictionary, 'number', 11)
first_ref, second_ref = weakref.ref(first), weakref.ref(second)
del first, second
gc.collect()
assert first_ref() is second_ref() is None
required_error(lambda: c_setitem(incoming, 'number', 'wrong after receiver death'))
required_error(lambda: c_setitem(escaped, 'number', 'wrong after receiver death'))

# A custom constructor is explicitly outside fresh direct-state admission.
# Calling the ordinary allocator inside it cannot waive inherited constraints.
events = []
class Custom(checked.Checked):
    def __new__(cls):
        result = object.__new__(cls)
        events.append('created')
        required_error(lambda: setattr(result, 'number', 'wrong before return'))
        return result

custom = Custom()
assert events == ['created'] and custom.number == 1
assert is_sealed(Custom) == 0
assert not info(custom)['has_slot']
legacy = vars(custom)
assert not info(legacy)['has_slot'] and has_policy(legacy) == 1
assert info(legacy)['storage_mode'] == 'legacy'
required_error(lambda: c_setattr(custom, 'number', 'wrong'))
required_error(lambda: c_setitem(legacy, 'number', 'wrong'))
assert custom.number == 1
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def nominal_field_project(request, tmp_path_factory):
    backend = getattr(request, "param", "soac")
    return create_strict_project(
        tmp_path_factory.mktemp(f"strict-nominal-fields-{backend}"),
        {
            "nominal_fields.py": """
from __future__ import strict
from nominal_field_probe import OrdinaryTarget, change_target, observe

class MutableHolder:
    payload: OrdinaryTarget

    def read(self) -> OrdinaryTarget:
        return self.payload

def family():
    class Target:
        pass
    class Holder:
        payload: Target
    def replace_target(value: type[Target]):
        nonlocal Target
        Target = value
    return Target, Holder, replace_target

def method_family():
    class Target:
        pass
    class Holder:
        def __init__(self, value):
            self.payload: Target = value
    return Target, Holder

def method_family_with_body_callback():
    class Target:
        pass
    original = Target
    def replace_target(value: type[Target]):
        nonlocal Target
        Target = value
    class Holder:
        change_target(replace_target)
        def __init__(self, value):
            self.payload: Target = value
    return original, Holder, replace_target

def captured_method_family():
    class Target:
        pass
    def make_holder():
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Target, Holder
    return make_holder

def mixed_storage_family():
    class DictionaryTarget:
        pass
    class MemberTarget:
        pass
    class Holder:
        __slots__ = ('native', '__dict__')
        payload: DictionaryTarget
        native: MemberTarget
    return DictionaryTarget, MemberTarget, Holder

def uncaptured_method_family():
    class Target:
        pass
    def make_holder():
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Holder
    return Target, make_holder

def namespace_method_family():
    class Target:
        pass
    class Outer:
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
    return Target, Outer.Holder

class ProbeBase:
    def __init_subclass__(cls):
        observe(cls)

def self_family():
    class SelfHolder(ProbeBase):
        payload: SelfHolder | None
    return SelfHolder
""",
            "nominal_field_probe.py": """
from typing import Any

observed = []

class OrdinaryTarget:
    pass

def change_target(replace: Any) -> None:
    replace(OrdinaryTarget)

def observe(cls: Any) -> None:
    import ctypes
    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    from soac.strict import StrictMutationError
    assert not owner(cls), 'the provisional field target was permanently admitted'
    try:
        object.__new__(cls)
    except StrictMutationError:
        pass
    else:
        raise AssertionError('self field allowed allocation before final selection')
    observed.append(cls)
""",
        },
        modules={"nominal_fields": "nominal_fields.py"},
        policy="""
[tool.soac.strict]
include = ["nominal_fields.py"]
checked_fields = "supported_annotations"
""",
        backend=backend,
    )


_NOMINAL_FIELD_PRELUDE = """
import ctypes
import nominal_fields as module
from soac.strict import StrictMutationError

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
set_item = ctypes.pythonapi.PyDict_SetItem
set_item.argtypes = [ctypes.py_object] * 3
set_item.restype = ctypes.c_int
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object] * 3
generic_set.restype = ctypes.c_int
assert _soac_ext.strict_module_diagnostics(module)['sealed']

def rejected(operation):
    try:
        operation()
    except TypeError as error:
        assert not isinstance(error, StrictMutationError), error
    else:
        raise AssertionError('a required nominal field accepted a foreign value')

def reject_all_writes(instance, wrong):
    previous = instance.payload
    for operation in (
        lambda: setattr(instance, 'payload', wrong),
        lambda: object.__setattr__(instance, 'payload', wrong),
        lambda: generic_set(instance, 'payload', wrong),
        lambda: vars(instance).__setitem__('payload', wrong),
        lambda: vars(instance).update(payload=wrong),
        lambda: set_item(vars(instance), 'payload', wrong),
    ):
        rejected(operation)
        assert instance.payload is previous
"""


_CPYTHON_NOMINAL_FIELD_WITNESSES = """
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int

def assert_native_class(cls):
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner == owner(cls) and info.owner is not None
    assert is_sealed(cls) == 1

assert_native_class(module.MutableHolder)
diagnostic = _soac_ext.strict_module_diagnostics(module)
observed_read = _assert_cpython_function_witness(
    module.MutableHolder.read, diagnostic,
)
assert observed_read["finalized"]
"""


def _run_nominal_field_case(project, program, function, *, entry_interpreter):
    if project.backend != "cpython":
        expected = "entry_interpreter" if entry_interpreter else "checked_native"
        return project.run(
            _NOMINAL_FIELD_PRELUDE
            + f"assert _soac_ext.strict_function_entry_kind(module.{function}) == {expected!r}\n"
            + program,
            entry_interpreter=entry_interpreter,
        )
    return project.run_case(
        "nominal_fields",
        "from soac import _soac_ext\n"
        + _NOMINAL_FIELD_PRELUDE + _CPYTHON_NOMINAL_FIELD_WITNESSES + program
        + f"""
observed = _assert_cpython_function_witness(
    module.{function}, diagnostic,
)
assert observed["original_code_entered"]
""",
        Path(__file__),
        required_functions=(function, "MutableHolder.read"),
        
        backend="cpython",
    )


@pytest.mark.parametrize(
    ("nominal_field_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["nominal_field_project"],
    scope="module",
)
def test_nominal_fields_bind_actual_factory_targets_and_survive_alias_mutation(
    nominal_field_project, entry_interpreter
):
    program = """
first_target, first_holder, replace = module.family()
second_target, second_holder, unused = module.family()
assert first_target is not second_target and first_holder is not second_holder
assert owner(first_holder) and owner(second_holder)
first, second = first_holder(), second_holder()
first.payload = first_target()
second.payload = second_target()
reject_all_writes(first, second.payload)
reject_all_writes(second, first.payload)
class Ordinary(first_target):
    pass
assert not owner(Ordinary)
first.payload = Ordinary()
replace(second_target)
first.payload = first_target()
reject_all_writes(first, second_target())
assert set_item(vars(first), 'payload', Ordinary()) == 0
assert isinstance(first.payload, Ordinary)
"""
    if nominal_field_project.backend == "cpython":
        program += """
assert_native_class(first_holder)
assert_native_class(second_holder)
has_policy = ctypes.pythonapi.PyDict_HasSoacPolicy
has_policy.argtypes = [ctypes.py_object]
has_policy.restype = ctypes.c_int
set_dictionary = ctypes.pythonapi.PyObject_GenericSetDict
set_dictionary.argtypes = [ctypes.py_object, ctypes.py_object, ctypes.c_void_p]
set_dictionary.restype = ctypes.c_int

# The actual supplied dictionary, not a cloned or indexed surrogate, acquires
# this class execution's predicate. An already-escaped old dictionary keeps it.
escaped = vars(first)
accepted = first_target()
incoming = {"payload": accepted, "extra": object()}
assert has_policy(incoming) == 0
first.__dict__ = incoming
assert vars(first) is incoming and first.payload is accepted
assert has_policy(incoming) == 1 and has_policy(escaped) == 1
reject_all_writes(first, second.payload)
rejected(lambda: set_item(escaped, "payload", second.payload))

# Compatible receivers may share that exact dictionary through the public C
# setter; both ordinary attribute and raw dictionary C writes stay checked.
alias = first_holder()
assert set_dictionary(alias, incoming, None) == 0
assert vars(alias) is incoming and vars(first) is incoming
replacement = first_target()
assert generic_set(alias, "payload", replacement) == 0
assert first.payload is replacement and alias.payload is replacement
assert set_item(incoming, "payload", accepted) == 0
assert first.payload is accepted and alias.payload is accepted
reject_all_writes(alias, second.payload)

# Refusal validates the incoming contents before installing a policy or
# replacing either receiver's authoritative dictionary.
invalid = {"payload": second.payload}
invalid_items = tuple(invalid.items())
rejected(lambda: set_dictionary(first, invalid, None))
assert vars(first) is incoming and vars(alias) is incoming
assert tuple(invalid.items()) == invalid_items and has_policy(invalid) == 0
unrestricted = object()
assert set_item(invalid, "payload", unrestricted) == 0
assert invalid["payload"] is unrestricted
assert first.payload is accepted and alias.payload is accepted
"""
    _run_nominal_field_case(
        nominal_field_project, program, "family",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("nominal_field_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["nominal_field_project"],
    scope="module",
)
def test_inherited_nominal_field_constraints_do_not_merge_equal_source_targets(
    nominal_field_project, entry_interpreter
):
    _run_nominal_field_case(
        nominal_field_project, """
first_target, first_holder, unused = module.family()
second_target, second_holder, unused = module.family()
assert owner(first_holder) and owner(second_holder)
class BothHolders(first_holder, second_holder):
    pass
class BothTargets(first_target, second_target):
    pass
assert not owner(BothHolders) and not owner(BothTargets)
instance = BothHolders()
instance.payload = BothTargets()
reject_all_writes(instance, first_target())
reject_all_writes(instance, second_target())
assert set_item(vars(instance), 'payload', BothTargets()) == 0
""",
        "family",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("nominal_field_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["nominal_field_project"],
    scope="module",
)
def test_self_nominal_field_binds_only_after_pending_class_callbacks(
    nominal_field_project, entry_interpreter
):
    program = """
import nominal_field_probe as probe
first, second = module.self_family(), module.self_family()
assert first is not second and owner(first) and owner(second)
assert len(probe.observed) == 2
assert probe.observed == [first, second]
left, right = first(), second()
left.payload, right.payload = left, right
assert left.payload is left and right.payload is right
vars(left)['payload'] = None
vars(right)['payload'] = None
assert left.payload is None and right.payload is None
left.payload = first()
right.payload = second()
reject_all_writes(left, right)
reject_all_writes(right, left)
"""
    if nominal_field_project.backend == "cpython":
        program += """
assert_native_class(first)
assert_native_class(second)
"""
    _run_nominal_field_case(
        nominal_field_project, program, "self_family",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("nominal_field_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["nominal_field_project"],
    scope="module",
)
def test_nominal_field_write_does_not_prove_a_mutable_referents_future_type(
    nominal_field_project, entry_interpreter
):
    _run_nominal_field_case(
        nominal_field_project, """
import nominal_field_probe as probe
assert owner(module.MutableHolder) and not owner(probe.OrdinaryTarget)
instance = module.MutableHolder()
value = probe.OrdinaryTarget()
instance.payload = value
assert instance.read() is value
class Foreign:
    pass
value.__class__ = Foreign
assert type(value) is Foreign
assert instance.payload is value
# A protected write does not guarantee the referent's future type, and
# annotations do not impose an additional check when that value is returned.
assert instance.read() is value
reject_all_writes(instance, value)
""",
        "MutableHolder.read",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("nominal_field_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["nominal_field_project"],
    scope="module",
)
def test_nominal_field_dictionary_retains_only_its_required_type_targets(
    nominal_field_project, entry_interpreter
):
    _run_nominal_field_case(
        nominal_field_project, """
import gc
import weakref
import nominal_field_probe as probe
target, holder, replace = module.family()
assert owner(holder)
instance = holder()
dictionary = vars(instance)
target_ref, holder_ref = weakref.ref(target), weakref.ref(holder)
del target, holder, replace, instance
gc.collect()
assert holder_ref() is None, 'an escaped dictionary retained its receiver class'
assert target_ref() is not None, 'a required nominal target was not retained'
rejected(lambda: set_item(dictionary, 'payload', object()))
del dictionary
gc.collect()
assert target_ref() is None, 'a dropped field policy retained its nominal target'

self_type = module.self_family()
assert probe.observed.pop() is self_type
instance = self_type()
dictionary = vars(instance)
self_ref = weakref.ref(self_type)
del self_type, instance
gc.collect()
assert self_ref() is not None, 'a direct-self field lost its required target'
rejected(lambda: set_item(dictionary, 'payload', object()))
del dictionary
gc.collect()
assert self_ref() is None, 'the direct-self policy cycle was not traversed'
""",
        "family",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize(
    ("nominal_field_project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["nominal_field_project"],
    scope="module",
)
def test_dictionary_type_state_drops_unrelated_native_slot_nominal_targets(
    nominal_field_project, entry_interpreter
):
    _run_nominal_field_case(
        nominal_field_project, """
import _testinternalcapi
import gc
import weakref
info = _testinternalcapi.get_soac_type_state_info

dictionary_target, member_target, holder = module.mixed_storage_family()
assert owner(holder) and owner(dictionary_target) and owner(member_target)
first, second = holder(), holder()
assert info(first)['has_slot'] and info(second)['has_slot']
assert info(first)['state_id'] == info(second)['state_id']
first.payload = dictionary_target()
first.native = member_target()
reject_all_writes(first, member_target())
rejected(lambda: generic_set(first, 'native', dictionary_target()))
escaped = vars(first)
assert info(escaped)['state_id'] == info(first)['dictionary_state_id']
assert info(escaped)['state_id'] != info(first)['state_id']
assert info(escaped)['storage_mode'] == 'direct'
# A hidden mapping entry is not the actual native member, and must not acquire
# its nominal target or predicate merely because the names are equal.
escaped['native'] = object()
assert isinstance(first.native, member_target)
del first.native
escaped.clear()
dictionary_ref, member_ref, holder_ref = (
    weakref.ref(dictionary_target), weakref.ref(member_target), weakref.ref(holder),
)
del first, second, dictionary_target, member_target, holder
gc.collect()
assert holder_ref() is None, 'dictionary state retained its receiver class'
assert member_ref() is None, 'dictionary state retained an unrelated native-slot target'
assert dictionary_ref() is not None, 'dictionary state dropped its required nominal target'
rejected(lambda: set_item(escaped, 'payload', object()))
set_item(escaped, 'native', object())
del escaped
gc.collect()
assert dictionary_ref() is None, 'released dictionary state leaked its remaining target'
""",
        "mixed_storage_family",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_method_only_field_annotation_uses_an_explicit_construction_capture(
    nominal_field_project, entry_interpreter
):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    nominal_field_project.run(
        _NOMINAL_FIELD_PRELUDE
        + f"assert _soac_ext.strict_function_entry_kind(module.method_family) == {expected!r}\n"
        + """
first_target, first_holder = module.method_family()
second_target, second_holder = module.method_family()
assert owner(first_holder) and owner(second_holder)
assert '__annotate__' not in vars(first_holder)
assert first_holder.__init__.__annotate__ is None
assert first_holder.__init__.__closure__ is None
first, second = first_holder(first_target()), second_holder(second_target())
reject_all_writes(first, second.payload)
reject_all_writes(second, first.payload)
rejected(lambda: first_holder(second_target()))

# Private compiler cells must not become extra lifetime edges from the class
# or its source function. Only the required nominal target is retained by an
# escaped dictionary's permanent write policy.
import gc
import weakref
def escaped_dictionary():
    target, holder = module.method_family()
    instance = holder(target())
    dictionary = vars(instance)
    del dictionary['payload']
    return weakref.ref(target), weakref.ref(holder), dictionary
target_ref, holder_ref, dictionary = escaped_dictionary()
gc.collect()
assert holder_ref() is None
assert target_ref() is not None
rejected(lambda: set_item(dictionary, 'payload', object()))
del dictionary
gc.collect()
assert target_ref() is None
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_private_field_captures_read_original_cells_after_the_namespace_body(
    nominal_field_project, entry_interpreter
):
    nominal_field_project.run(
        _NOMINAL_FIELD_PRELUDE
        + """
import nominal_field_probe as probe
original, holder, replace = module.method_family_with_body_callback()
assert owner(holder)
assert holder.__init__.__closure__ is None
assert holder.__init__.__annotate__ is None
instance = holder(probe.OrdinaryTarget())
reject_all_writes(instance, original())

# Cell identities were captured before construction, but their values were
# read after the ordinary class body changed Target. A later change cannot
# revise the committed predicate.
replace(original)
instance.payload = probe.OrdinaryTarget()
reject_all_writes(instance, original())
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize("scope", ["captured_function", "private_function", "class_namespace"])
def test_nominal_field_cells_forward_through_the_actual_lexical_owner(
    nominal_field_project, entry_interpreter, scope
):
    nominal_field_project.run(
        _NOMINAL_FIELD_PRELUDE
        + f"scope = {scope!r}\n"
        + """
def construct():
    if scope == 'captured_function':
        factory = module.captured_method_family()
        assert factory.__code__.co_freevars == ('Target',)
        target, holder = factory()
        assert factory.__closure__[0].cell_contents is target
        return target, holder
    if scope == 'private_function':
        target, factory = module.uncaptured_method_family()
        assert factory.__code__.co_freevars == ()
        assert factory.__closure__ is None
        assert factory.__annotate__ is None
        assert module.uncaptured_method_family.__code__.co_cellvars == ()
        return target, factory()
    assert module.namespace_method_family.__code__.co_cellvars == ()
    return module.namespace_method_family()

first_target, first_holder = construct()
second_target, second_holder = construct()
assert owner(first_holder) and owner(second_holder)
assert first_target is not second_target and first_holder is not second_holder
assert first_holder.__init__.__closure__ is None
assert first_holder.__init__.__annotate__ is None
first, second = first_holder(first_target()), second_holder(second_target())
reject_all_writes(first, second.payload)
reject_all_writes(second, first.payload)
rejected(lambda: first_holder(second_target()))
""",
        entry_interpreter=entry_interpreter,
    )
