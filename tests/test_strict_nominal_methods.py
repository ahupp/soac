"""Nominal annotations preserve ordinary calls and authenticated class ownership."""

import textwrap

import pytest

from tests._strict_integration import create_strict_project

_SOURCE = """
from __future__ import strict
from nominal_support import (
    annotation_trap, arbitrary_result, assert_method_provider_frozen, events,
)

class Token:
    def accept(self, value: Token) -> Token:
        events.append("accept")
        return value

    def optional(self, value: Token | None) -> Token | None:
        return value

    def wrong_return(self) -> Token:
        events.append("return")
        return arbitrary_result()

class Child(Token):
    def accept_base(self, value: Token) -> Token:
        return value

GlobalAlias = Token

def accept_global(value: GlobalAlias) -> Token:
    return value

# The free function is still initializing. Its replaceable provider is not
# executed or trusted for required contracts during adoption or calls.
accept_global.__annotate__ = annotation_trap
# Token already completed its actual class Store, before this module seals.
assert_method_provider_frozen(Token.accept, module_sealed=False)

def factory():
    class Local:
        def accept(self, value: Local) -> Local:
            return value
    return Local
"""

_SUPPORT = """
from typing import Any

events = []

def annotation_trap(format: int) -> Any:
    events.append("annotation evaluated")
    raise AssertionError("nominal binding evaluated an annotation provider")

def arbitrary_result() -> Any:
    return object()


def assert_method_provider_frozen(method: Any, *, module_sealed: bool) -> None:
    import sys
    from soac import _soac_ext
    from soac.strict import StrictMutationError

    namespace = method.__globals__
    module = sys.modules[namespace["__name__"]]
    assert vars(module) is namespace
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    assert diagnostic is not None and diagnostic["sealed"] is module_sealed
    provider = method.__annotate__
    assert provider is not None and provider is not annotation_trap
    try:
        method.__annotate__ = annotation_trap
    except StrictMutationError:
        pass
    else:
        raise AssertionError("admitted method accepted a replacement annotation provider")
    assert method.__annotate__ is provider
"""


@pytest.fixture(scope="module")
def nominal_methods(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-nominal-methods"),
        {"nominal_methods.py": _SOURCE, "nominal_support.py": _SUPPORT},
        modules={"nominal_methods": "nominal_methods.py"},
    )


@pytest.fixture(scope="module")
def quoted_nominals(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-quoted-nominals"),
        {
            "quoted_nominals.py": """
from __future__ import strict
from typing import Optional
from quoted_nominal_support import before_ready, arbitrary_result, annotation_trap

class Token:
    def accept(self, value: "Token") -> "Token":
        return value

GlobalAlias = Token

def accept_global(value: "GlobalAlias") -> "Token":
    return value

def optional(value: "Optional[Token | GlobalAlias]") -> "Optional[Token]":
    return value

def two(first: "Token", second: "GlobalAlias") -> "GlobalAlias":
    return second

def wrong_return() -> "Token":
    return arbitrary_result()

accept_global.__annotate__ = annotation_trap

class Base:
    def __init_subclass__(cls):
        before_ready(cls)

class Ready(Base):
    def accept(self, value: "Ready") -> "Ready":
        return value

def factory():
    class Local:
        def accept(self, value: "Local") -> "Local":
            return value
    return Local
""",
            "quoted_nominal_support.py": """
from typing import Any
events = []

def before_ready(cls: Any) -> None:
    from soac.strict import StrictMutationError

    try:
        cls()
    except StrictMutationError:
        events.append("pending allocation rejected")
    else:
        raise AssertionError("quoted callback allocated a pending type")
    # The type is Pending, but a method with no protected write remains an
    # ordinary call even when its annotations name that unfinished class.
    value = object()
    assert cls.accept(None, value) is value
    events.append("pre-ready ordinary call")

def arbitrary_result() -> Any:
    return object()

def annotation_trap(format: int) -> Any:
    events.append("annotation evaluated")
    raise AssertionError("nominal binding evaluated the annotation provider")
""",
        },
        modules={"quoted_nominals": "quoted_nominals.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_quoted_nominals_keep_ordinary_calls_without_provider_evaluation(
    quoted_nominals, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    quoted_nominals.run(
        f"""
from soac import _soac_ext
import quoted_nominals as module
from quoted_nominal_support import annotation_trap, events

diagnostic = _soac_ext.strict_module_diagnostics(module)
assert diagnostic['sealed']
assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
assert diagnostic['artifact_generation'] == {quoted_nominals.publication["generation"]!r}
for function in (module.Token.accept, module.Ready.accept, module.accept_global,
                 module.optional, module.two, module.wrong_return, module.factory):
    assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}
assert events == ["pending allocation rejected", "pre-ready ordinary call"], events
assert module.accept_global.__annotate__ is annotation_trap

# The original successful self-call now occurs after actual final admission.
ready = module.Ready()
assert ready.accept(ready) is ready
marker = object()
assert ready.accept(marker) is marker

class OrdinaryChild(module.Token):
    pass

for value in (module.Token(), OrdinaryChild()):
    assert module.Token().accept(value) is value
    assert module.accept_global(value) is value
    assert module.optional(value) is value
    assert module.two(module.Token(), value) is value
assert module.optional(None) is None

for function, arguments in (
    (module.accept_global, (object(),)),
    (module.optional, (object(),)),
    (module.two, (object(), module.Token())),
    (module.two, (module.Token(), object())),
    (module.wrong_return, ()),
):
    result = function(*arguments)
    if arguments:
        assert result is arguments[-1]
    else:
        assert type(result) is object

def ordinary_factory():
    class Local:
        def accept(self, value: "Local") -> "Local":
            return value
    return Local

ordinary = ordinary_factory()
assert _soac_ext.strict_function_entry_kind(ordinary.accept) is None
native_captures = ordinary.accept.__annotate__.__code__.co_freevars
first, second = module.factory(), module.factory()
assert first is not second
assert first.__qualname__ == second.__qualname__
for actual, other in ((first, second), (second, first)):
    assert _soac_ext.strict_function_entry_kind(actual.accept) == {expected_entry!r}
    # Keep exactly the native provider layout, including special class-dict
    # captures. Do not manufacture a lexical cell from the quoted class name.
    assert actual.accept.__annotate__.__code__.co_freevars == native_captures
    value = actual()
    assert value.accept(value) is value
    other_value = other()
    assert value.accept(other_value) is other_value
assert events == ["pending allocation rejected", "pre-ready ordinary call"], events
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_owned_method_nominals_do_not_constrain_values_or_consult_membership_hooks(
    nominal_methods, entry_interpreter
):
    nominal_methods.run(
        """
import ctypes
import nominal_methods as module
from nominal_support import annotation_trap, assert_method_provider_frozen, events

assert events == []
assert module.accept_global.__annotate__ is annotation_trap
assert_method_provider_frozen(module.Token.accept, module_sealed=True)

class OrdinaryChild(module.Token):
    pass

is_sealed = ctypes.pythonapi.PyType_IsSoacSealed
is_sealed.argtypes = [ctypes.py_object]
is_sealed.restype = ctypes.c_int
assert is_sealed(module.Token) == 1
assert is_sealed(module.Child) == 1
assert is_sealed(OrdinaryChild) == 0

receiver = module.Token()
for value in (module.Token(), module.Child(), OrdinaryChild()):
    assert receiver.accept(value) is value
    assert receiver.optional(value) is value
    assert module.Child().accept_base(value) is value
    assert module.accept_global(value) is value
assert receiver.optional(None) is None
assert events == ["accept"] * 3

class Spoof:
    @property
    def __class__(self):
        events.append("spoof consulted")
        return module.Token

for value in (object(), Spoof()):
    before = list(events)
    assert receiver.accept(value) is value
    assert events == before + ["accept"], "an overridable membership hook ran"

assert type(receiver.wrong_return()) is object
assert events[-1] == "return"
assert "annotation evaluated" not in events
print("owned-method-nominal-boundaries")
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_same_source_factory_methods_keep_distinct_classes_and_collectable_owners(
    nominal_methods, entry_interpreter
):
    nominal_methods.run(
        """
import gc
import weakref
import nominal_methods as module

first = module.factory()
second = module.factory()
assert first is not second
assert first.__qualname__ == second.__qualname__
left = first()
right = second()
assert left.accept(left) is left
assert right.accept(right) is right

# Changing the original annotation cell does not alter the function body or
# merge the identities of two executions of the same class source.
provider = first.accept.__annotate__
cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
assert "Local" in cells
cells["Local"].cell_contents = second
assert left.accept(left) is left

for receiver, value in ((left, right), (right, left)):
    assert receiver.accept(value) is value

class OrdinaryChild(first):
    pass

child = OrdinaryChild()
assert left.accept(child) is child
assert right.accept(child) is child

def collectable_contract_cycle():
    local = module.factory()
    return weakref.ref(local), weakref.ref(local.accept)

references = collectable_contract_cycle()
gc.collect()
assert all(reference() is None for reference in references)
print("factory-nominal-isolation")
""",
        entry_interpreter=entry_interpreter,
    )



def _nominal_validation(project, program, *, entry_interpreter=False):
    expected_entry = (
        "original_code" if project.backend == "cpython"
        else "entry_interpreter" if entry_interpreter else "checked_native"
    )
    witnesses = """
import ctypes
from types import FunctionType
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

def native_api(name, result):
    function = getattr(ctypes.pythonapi, name)
    function.argtypes = [ctypes.py_object]
    function.restype = result
    return function

function_owner = native_api("PyFunction_GetSoacStrictOwner", ctypes.c_void_p)
strict_id = native_api("PyFunction_GetSoacStrictId", ctypes.c_uint64)
function_metadata = native_api("PyFunction_GetSoacMetadata", ctypes.c_void_p)

def assert_adopted_function(function, *, entered=None):
    assert type(function) is FunctionType
    assert function_owner(function), "function lost its actual creation owner"
    assert strict_id(function) != 0, "adopted function is not natively sealed"
    assert _soac_ext.strict_function_entry_kind(function) == expected_entry
    if expected_entry == "original_code":
        diagnostic = _assert_cpython_function_witness(
            function, _soac_ext.strict_module_diagnostics(module),
        )
        assert diagnostic["finalized"] is True, diagnostic
        if entered is not None:
            assert diagnostic["original_code_entered"] is entered, diagnostic
    else:
        assert function_metadata(function), "retained function lacks entry metadata"

"""
    return "def validate(module):\n" + textwrap.indent(
        f"expected_entry = {expected_entry!r}\n" + witnesses + textwrap.dedent(program),
        "    ",
    )


@pytest.fixture(scope="module")
def alias_nominals(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-nominal-aliases"),
        {
            "nominal_aliases.py": """
from __future__ import strict
from nominal_alias_support import retarget_aliases

def factory():
    class Local:
        def accepts_referenced(self, value: Alias) -> Alias:
            return value
    Alias = Local
    First = Local
    Second = Local
    def two(first: First, second: Second) -> Second:
        return second
    def either(value: First | Second) -> First | Second:
        return value
    def wrong_return(first: First, second: Second) -> Second:
        return first
    retarget_aliases(Local.accepts_referenced.__annotate__, two.__annotate__,
                     either.__annotate__, Local)
    return Local, two, either, wrong_return

first = factory()
second = factory()
unresolved = factory()
""",
            "nominal_alias_support.py": """
from typing import Any

previous: Any = None
calls = 0

def retarget_aliases(method: Any, two: Any, either: Any, current: Any) -> None:
    global previous, calls
    if previous is not None:
        # Real ordinary metadata mutation before strict adoption. These are
        # actual original provider cells, not fabricated source/type facts.
        for provider, expected_name in ((method, "Alias"), (two, "Second"),
                                        (either, "Second")):
            cells = dict(zip(provider.__code__.co_freevars,
                             provider.__closure__ or ()))
            assert expected_name in cells, (expected_name, cells)
            cells[expected_name].cell_contents = previous if calls == 1 else None
    previous = current
    calls += 1
""",
        },
        modules={"nominal_aliases": "nominal_aliases.py"},
        backend=getattr(request, "param", "soac"),
    )


def _check_alias_nominals(alias_nominals, *, entry_interpreter=False):
    alias_nominals.run_case(
        'nominal_aliases',
        _nominal_validation(
            alias_nominals,
            """
import nominal_aliases as module

left_class, left_two, left_either, _ = module.first
right_class, right_two, right_either, right_wrong = module.second
left = left_class()
right = right_class()
assert type(left) is not type(right)
assert type(left).__qualname__ == type(right).__qualname__

# These methods froze when their own class completed, before the source
# assigned Alias. Later annotation-cell contents do not constrain calls.
method_rows = (
    (left_class.accepts_referenced, left, left_class),
    (right_class.accepts_referenced, right, left_class),
    (module.unresolved[0].accepts_referenced, module.unresolved[0](), None),
)
for function, receiver, actual_alias in method_rows:
    assert_adopted_function(function, entered=False)
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    assert cells["Alias"].cell_contents is actual_alias
    for value in (left, right, object()):
        assert function(receiver, value) is value
    assert_adopted_function(function, entered=True)

# Free functions still finish adoption at this initializing module's seal.
# Actual aliases and unions remain static facts, not runtime call predicates.
for function in (left_two, right_two, left_either, right_either, right_wrong):
    assert_adopted_function(function)
for function, accepted, rejected in (
    (left_either, left, right),
):
    assert function(accepted) is accepted
    assert function(rejected) is rejected

assert right_two(right, left) is left
assert left_two(left, left) is left
for arguments in ((left, left), (right, right)):
    assert right_two(*arguments) is arguments[-1]
for value in (left, right):
    assert right_either(value) is value
assert right_wrong(right, left) is right
marker = object()
assert right_either(marker) is marker
unresolved_class, _, unresolved_either, _ = module.unresolved
unresolved_value = unresolved_class()
assert unresolved_either(unresolved_value) is unresolved_value

# Metadata seals do not freeze the contents of annotation cells or turn those
# contents into a runtime call contract or layout proof.
for function, name in ((right_two, "Second"), (right_either, "First"),
                       (right_class.accepts_referenced, "Alias")):
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    assert name in cells
    cells[name].cell_contents = None
assert right_two(right, left) is left
assert right_class.accepts_referenced(right, left) is left
for value in (left, right):
    assert right_either(value) is value
# Mutating every genuine method-provider cell leaves body values and actual
# function ownership unchanged, including cells containing None.
for function, receiver, _ in method_rows:
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    for replacement in (right_class, left_class, None):
        cells["Alias"].cell_contents = replacement
        assert cells["Alias"].cell_contents is replacement
        for value in (left, right):
            assert function(receiver, value) is value
        assert_adopted_function(function, entered=True)
for function in (left_two, right_two, left_either, right_either, right_wrong):
    assert_adopted_function(function)
print("same-source-alias-bindings")
""",
            entry_interpreter=entry_interpreter,
        ),
        alias_nominals.project / 'nominal_aliases.py',
        required_functions=('factory',),
        
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_nominal_aliases_preserve_actual_cells_without_runtime_call_predicates(
    alias_nominals, entry_interpreter
):
    _check_alias_nominals(alias_nominals, entry_interpreter=entry_interpreter)


@pytest.mark.parametrize("alias_nominals", ["cpython"], indirect=True)
def test_cpython_nominal_aliases_preserve_actual_cells_without_runtime_call_predicates(
    alias_nominals,
):
    _check_alias_nominals(alias_nominals)


@pytest.fixture(scope="module")
def prebound_closure_nominals(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-nominal-prebound-closures"),
        {"nominal_prebound_closures.py": """
from __future__ import strict

def factory():
    class Target:
        pass
    Alias = Target
    class Holder:
        def accept(self, value: Alias) -> Alias:
            return value
    return Target, Holder

first = factory()
second = factory()
"""},
        modules={"nominal_prebound_closures": "nominal_prebound_closures.py"},
        backend=getattr(request, "param", "soac"),
    )


def _check_prebound_closure_nominals(prebound_closure_nominals, *, entry_interpreter=False):
    prebound_closure_nominals.run_case(
        'nominal_prebound_closures',
        _nominal_validation(
            prebound_closure_nominals,
            """
left_target, left_holder = module.first
right_target, right_holder = module.second
assert left_target is not right_target and left_holder is not right_holder
assert left_target.__qualname__ == right_target.__qualname__
assert left_holder.__qualname__ == right_holder.__qualname__
left = left_target()
right = right_target()
rows = (
    (left_holder.accept, left_holder(), left, right, left_target, right_target),
    (right_holder.accept, right_holder(), right, left, right_target, left_target),
)
for function, receiver, accepted, rejected, selected, other in rows:
    assert_adopted_function(function, entered=False)
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    assert cells["Alias"].cell_contents is selected
    assert function(receiver, accepted) is accepted
    assert function(receiver, rejected) is rejected
    marker = object()
    assert function(receiver, marker) is marker
    assert_adopted_function(function, entered=True)
    for replacement in (other, None):
        cells["Alias"].cell_contents = replacement
        assert cells["Alias"].cell_contents is replacement
        assert function(receiver, accepted) is accepted
        assert function(receiver, rejected) is rejected
        assert function(receiver, marker) is marker
        assert_adopted_function(function, entered=True)
print("prebound-closure-calls-remain-ordinary")
""",
            entry_interpreter=entry_interpreter,
        ),
        prebound_closure_nominals.project / 'nominal_prebound_closures.py',
        required_functions=('factory',),
        
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_prebound_closure_aliases_keep_owned_calls_after_cell_mutation(
    prebound_closure_nominals, entry_interpreter
):
    _check_prebound_closure_nominals(
        prebound_closure_nominals, entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("prebound_closure_nominals", ["cpython"], indirect=True)
def test_cpython_prebound_closure_aliases_keep_owned_calls_after_cell_mutation(
    prebound_closure_nominals,
):
    _check_prebound_closure_nominals(prebound_closure_nominals)


@pytest.fixture(scope="module")
def construction_nominals(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-nominal-construction"),
        {
            "nominal_construction.py": """
from __future__ import strict
from nominal_construction_support import check_before_name_store

class Base:
    def __init_subclass__(cls):
        check_before_name_store(cls)

class Child(Base):
    def accept(self, value: Child) -> Child:
        return value
    alias = accept
""",
            "nominal_construction_support.py": """
from typing import Any

events = []

def check_before_name_store(cls: Any) -> None:
    # type_new has not returned and the module has not stored Child yet.
    namespace = cls.accept.__globals__
    assert "Child" not in namespace
    from soac.strict import StrictMutationError
    try:
        cls()
    except StrictMutationError:
        events.append("pending-allocation")
    else:
        raise AssertionError("construction callback allocated an unfinished type")
    for method in (cls.accept, cls.alias):
        value = object()
        assert method(object(), value) is value
        events.append("pending ordinary call")
""",
        },
        modules={"nominal_construction": "nominal_construction.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_pending_self_type_rejects_allocation_but_not_ordinary_method_calls(
    construction_nominals, entry_interpreter
):
    construction_nominals.run(
        """
import nominal_construction as module
from nominal_construction_support import events

assert events == ["pending-allocation", "pending ordinary call", "pending ordinary call"]
value = module.Child()
assert value.accept(value) is value and value.alias(value) is value
marker = object()
assert value.accept(marker) is marker
""",
        entry_interpreter=entry_interpreter,
    )


def _initializing_nominals_project(root, *, backend="soac"):
    return create_strict_project(
        root,
        {
            "nominal_initializing.py": """
from __future__ import strict
from nominal_initializing_support import before_adoption, move_alias

def factory():
    class Local:
        pass
    Alias = Local
    def accept(value: Alias) -> Alias:
        move_alias(accept.__annotate__, Local)
        return value
    before_adoption(accept, Local)
    return Local, accept

first = factory()
second = factory()
""",
            "nominal_initializing_support.py": """
from typing import Any

previous: Any = None
events = []

def alias_cell(function: Any) -> Any:
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    return cells["Alias"]

def move_alias(provider: Any, current: Any) -> None:
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    cells["Alias"].cell_contents = current
    events.append("body")

def before_adoption(function: Any, current: Any) -> None:
    global previous
    if previous is None:
        value = current()
        assert function(value) is value
    else:
        old_value = previous()
        class Keyword(str):
            __hash__ = str.__hash__
            def __eq__(self, other):
                # Normal binding runs this callback before the source body.
                alias_cell(function).cell_contents = previous
                events.append("keyword")
                return str.__eq__(self, other)
        assert function(**{Keyword("value"): old_value}) is old_value
        assert alias_cell(function).cell_contents is current
        # The first body changed the annotation cell. Calls still pass the
        # original values through independently of that cell's contents.
        new_value = current()
        assert function(new_value) is new_value
        assert function(old_value) is old_value
        events.append("next-call-ordinary")
    previous = current
""",
        },
        modules={"nominal_initializing": "nominal_initializing.py"},
        backend=backend,
    )


@pytest.fixture(scope="module")
def initializing_nominals(tmp_path_factory):
    return _initializing_nominals_project(
        tmp_path_factory.mktemp("strict-nominal-initializing")
    )


@pytest.fixture(scope="module")
def initializing_cpython_nominals(tmp_path_factory):
    return _initializing_nominals_project(
        tmp_path_factory.mktemp("strict-cpython-nominal-initializing"),
        backend="cpython",
    )


def test_cpython_initializing_nominals_preserve_binding_callbacks_and_return_identity(
    initializing_cpython_nominals,
):
    initializing_cpython_nominals.run_case(
        "nominal_initializing",
        """
def validate(module):
    from soac import _soac_ext
    from nominal_initializing_support import events

    # The keyword callback precedes the body, which restores its annotation
    # cell. Neither annotation value changes ordinary argument/result identity.
    assert events == ["body", "keyword", "body", "body", "body", "next-call-ordinary"], events
    left_class, left_accept = module.first
    right_class, right_accept = module.second
    left = left_class()
    right = right_class()
    assert left_accept(left) is left
    assert right_accept(right) is right
    module_diagnostic = _soac_ext.strict_module_diagnostics(module)
    for function in (left_accept, right_accept):
        diagnostic = _soac_ext.strict_function_diagnostics(function)
        assert diagnostic is not None, "nested function lacks its actual native owner"
        assert diagnostic["backend"] == "cpython", diagnostic
        assert diagnostic["entry_kind"] == "original_code", diagnostic
        assert diagnostic["original_code_entered"] is True, diagnostic
        for key in ("source_path", "source_sha256", "artifact_generation"):
            assert diagnostic[key] == module_diagnostic[key], (key, diagnostic)
    assert _soac_ext.runtime_compilation_activity() == {
        "schema": 1, "lowering_entries": 0, "blockpy_cache_entries": 0,
        "jit_engine_entries": 0,
    }
""",
        initializing_cpython_nominals.project / "nominal_initializing.py",
        required_functions=("factory",),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_initializing_nominals_preserve_binding_callbacks_and_return_identity(
    initializing_nominals, entry_interpreter
):
    initializing_nominals.run(
        """
import nominal_initializing as module
from nominal_initializing_support import events

assert events == ["body", "keyword", "body", "body", "body", "next-call-ordinary"], events
left_class, left_accept = module.first
right_class, right_accept = module.second
left = left_class()
right = right_class()
assert left_accept(left) is left
assert right_accept(right) is right
print("initializing-nominal-snapshots")
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def class_dictionary_nominals(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-nominal-class-dictionary"),
        {
            "nominal_class_dictionary.py": """
from __future__ import strict
from nominal_class_dictionary_support import before_ready, events

class Token:
    pass

class Base:
    def __init_subclass__(cls):
        before_ready(cls)

class Child(Base):
    Alias = Token

    def accept(self, value: Alias) -> Alias:
        events.append("body")
        return value
""",
            "nominal_class_dictionary_support.py": """
from typing import Any

events = []

def class_dictionary_cell(function: Any) -> Any:
    provider = function.__annotate__
    cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
    return cells["__classdict__"]

def before_ready(cls: Any) -> None:
    import ctypes
    from soac.strict import StrictMutationError
    from tests.test_strict_type_native import ConstructionInfoV1

    construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
    construction.argtypes = [
        ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
    ]
    construction.restype = ctypes.c_int
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.abi_version == 1 and info.struct_size == ctypes.sizeof(info)
    assert info.phase == 1 and info.permanent_contract_published == 0
    assert info.owner is not None and info.root_construction is not None
    try:
        cls()
    except StrictMutationError as error:
        assert type(error) is StrictMutationError
        events.append("pending-allocation")
    else:
        raise AssertionError("class namespace observer allocated a pending type")

    function = cls.accept
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(function)
    cell = class_dictionary_cell(function)
    actual = cell.cell_contents
    assert type(actual) is dict
    assert actual["accept"] is function
    value = actual["Alias"]()
    # No Child instance exists before admission. This unbound source method
    # does not access protected storage, so its call remains ordinary.
    receiver = object()
    assert function(receiver, value) is value
    events.append("before-ready")

    # A replacement annotation dictionary is not the type's namespace and
    # does not affect a body that merely returns its argument.
    cell.cell_contents = dict(actual)
    before = list(events)
    try:
        assert function(receiver, value) is value
    finally:
        cell.cell_contents = actual
    assert events == before + ["body"]
    assert function(receiver, value) is value
    events.append("restored")
""",
        },
        modules={"nominal_class_dictionary": "nominal_class_dictionary.py"},
        backend=getattr(request, "param", "soac"),
    )


def _check_class_dictionary_nominals(class_dictionary_nominals, *, entry_interpreter=False):
    class_dictionary_nominals.run_case(
        'nominal_class_dictionary',
        _nominal_validation(
            class_dictionary_nominals,
            """
import nominal_class_dictionary as module
from nominal_class_dictionary_support import class_dictionary_cell, events

assert events == ["pending-allocation", "body", "before-ready", "body", "body", "restored"]
assert_adopted_function(module.Child.accept, entered=True)
receiver = module.Child()
value = module.Token()
assert receiver.accept(value) is value

class Foreign:
    pass

function = module.Child.accept
cell = class_dictionary_cell(function)
cell.cell_contents = {"Alias": Foreign}

# Annotation evaluation observes its actual cell, while calls remain ordinary
# and actual function ownership stays sealed.
assert function.__annotate__(1)["value"] is Foreign
assert receiver.accept(value) is value
before = list(events)
foreign = Foreign()
assert receiver.accept(foreign) is foreign
assert events == before + ["body"]
assert_adopted_function(module.Child.accept, entered=True)
print("class-dictionary-nominal-boundaries")
""",
            entry_interpreter=entry_interpreter,
        ),
        class_dictionary_nominals.project / 'nominal_class_dictionary.py',
        required_functions=('Child.accept',),
        
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_class_scoped_annotations_use_actual_cells_without_constraining_calls(
    class_dictionary_nominals, entry_interpreter
):
    _check_class_dictionary_nominals(
        class_dictionary_nominals, entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("class_dictionary_nominals", ["cpython"], indirect=True)
def test_cpython_class_scoped_annotations_use_actual_cells_without_constraining_calls(
    class_dictionary_nominals,
):
    _check_class_dictionary_nominals(class_dictionary_nominals)


@pytest.fixture(scope="module")
def dynamic_nominals(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-dynamic-nominals"),
        {
            "dynamic_nominals.py": """
from __future__ import strict
from dynamic_nominal_support import Meta, Outside, events, wrong_value

class Record(metaclass=Meta):
    def echo(self, value: int) -> int:
        return value

def accept(value: Record) -> Record:
    events.append('accept')
    return value

def external(value: Outside) -> Outside:
    events.append('external')
    return value

def optional(value: Record | None) -> Record | None:
    return value

def wrong_return() -> Record:
    return wrong_value()

def invoke(value: Record, argument):
    return value.echo(argument)

def factory():
    class Local(metaclass=Meta):
        pass
    def accept_local(value: Local) -> Local:
        return value
    return Local, accept_local
""",
            "dynamic_nominal_support.py": """
from typing import Any

events = []

class Meta(type):
    def __instancecheck__(cls, value):
        raise AssertionError('nominal checking called __instancecheck__')

    def __subclasscheck__(cls, value):
        raise AssertionError('nominal checking called __subclasscheck__')

class Outside(metaclass=Meta):
    pass

def wrong_value() -> Any:
    return object()
""",
        },
        modules={"dynamic_nominals": "dynamic_nominals.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_dynamic_and_external_nominals_do_not_require_a_layout_contract(
    dynamic_nominals, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    dynamic_nominals.run(
        f"""
import ctypes
import dynamic_nominals as module
from dynamic_nominal_support import Outside, events

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
function_owner.argtypes = [ctypes.py_object]
function_owner.restype = ctypes.c_void_p
sealed_function = ctypes.pythonapi.PyFunction_GetSoacStrictId
sealed_function.argtypes = [ctypes.py_object]
sealed_function.restype = ctypes.c_uint64
assert _soac_ext.strict_module_diagnostics(module)['sealed']
for cls in (module.Record, Outside):
    assert owner(cls) is None
for function in (module.accept, module.external, module.optional,
                 module.wrong_return, module.invoke):
    assert function_owner(function) and sealed_function(function)
    assert _soac_ext.strict_function_entry_kind(function) == {expected_entry!r}
assert sealed_function(module.Record.echo) == 0

class Child(module.Record):
    pass

for value in (module.Record(), Child()):
    assert module.accept(value) is value
    assert module.optional(value) is value
assert module.optional(None) is None
outside = Outside()
assert module.external(outside) is outside
assert events == ['accept', 'accept', 'external']

class Spoof:
    @property
    def __class__(self):
        raise AssertionError('nominal checking called the __class__ property')

for value in (object(), outside, Spoof()):
    before = list(events)
    assert module.accept(value) is value
    assert events == before + ['accept']
for function, arguments in ((module.external, (module.Record(),)),
                            (module.optional, (object(),)),
                            (module.wrong_return, ())):
    result = function(*arguments)
    if arguments:
        assert result is arguments[-1]
    else:
        assert type(result) is object

# A function annotation grants no class, field, or method capability.
record = module.Record()
assert module.invoke(record, 'ordinary method') == 'ordinary method'
module.Record.echo = lambda self, value: ('replacement', value)
assert module.invoke(record, 3) == ('replacement', 3)
record.echo = lambda value: ('instance', value)
assert module.invoke(record, 4) == ('instance', 4)
assert owner(module.Record) is None
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_dynamic_factory_nominals_keep_distinct_classes_and_collectable_cycles(
    dynamic_nominals, entry_interpreter
):
    dynamic_nominals.run(
        """
import gc
import weakref
import dynamic_nominals as module

first, accept_first = module.factory()
second, accept_second = module.factory()
assert first is not second and first.__qualname__ == second.__qualname__
for actual, accepted, rejected in ((accept_first, first(), second()),
                                   (accept_second, second(), first())):
    assert actual(accepted) is accepted
    assert actual(rejected) is rejected

provider = accept_first.__annotate__
cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
assert 'Local' in cells
cells['Local'].cell_contents = second
value = first()
assert accept_first(value) is value
other_value = second()
assert accept_first(other_value) is other_value

def collectable():
    cls, function = module.factory()
    return weakref.ref(cls), weakref.ref(function)

references = collectable()
gc.collect()
assert all(reference() is None for reference in references)
""",
        entry_interpreter=entry_interpreter,
    )



@pytest.fixture(scope="module")
def cpython_nominal_methods(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("cpython-nominal-methods"),
        {"nominal_methods.py": _SOURCE, "nominal_support.py": _SUPPORT},
        modules={"nominal_methods": "nominal_methods.py"}, backend="cpython",
    )


def test_cpython_backend_factory_methods_keep_actual_class_ownership_and_ordinary_calls(cpython_nominal_methods):
    from pathlib import Path

    cpython_nominal_methods.run_case(
        "nominal_methods",
        """
import ctypes
import gc
import weakref
import nominal_methods as module
from nominal_support import annotation_trap, assert_method_provider_frozen, events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

diagnostic = _soac_ext.strict_module_diagnostics(module)
assert events == []
assert module.accept_global.__annotate__ is annotation_trap
assert_method_provider_frozen(module.Token.accept, module_sealed=True)
first = module.factory()
second = module.factory()
assert first is not second
assert first.__qualname__ == second.__qualname__
assert first.accept.__code__ is second.accept.__code__
left = first()
right = second()
for function in (first.accept, second.accept):
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed['original_code_entered'] is False
assert left.accept(right) is right
assert _soac_ext.strict_function_diagnostics(first.accept)['original_code_entered'] is True
assert left.accept(left) is left
assert _soac_ext.strict_function_diagnostics(first.accept)['original_code_entered'] is True
assert _soac_ext.strict_function_diagnostics(second.accept)['original_code_entered'] is False
assert right.accept(right) is right
assert _soac_ext.strict_function_diagnostics(second.accept)['original_code_entered'] is True

# Preserve the original native annotation provider and its actual closure.
# A mutable provider cell does not change the actual class or call body.
provider = first.accept.__annotate__
cells = dict(zip(provider.__code__.co_freevars, provider.__closure__ or ()))
assert 'Local' in cells
cells['Local'].cell_contents = second
assert left.accept(left) is left
for receiver, value in ((left, right), (right, left)):
    assert receiver.accept(value) is value

class OrdinaryChild(first):
    pass
child = OrdinaryChild()
assert left.accept(child) is child
assert right.accept(child) is child
class_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
class_owner.argtypes = [ctypes.py_object]
class_owner.restype = ctypes.c_void_p
assert class_owner(first) and class_owner(second)
assert not class_owner(OrdinaryChild)

call = ctypes.pythonapi.PyObject_CallOneArg
call.argtypes = [ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
for _ in range(128):
    assert left.accept(left) is left
    assert right.accept(right) is right
assert call(left.accept, child) is child
assert call(right.accept, child) is child

base = module.Token()
assert base.accept(base) is base
assert module.accept_global(base) is base
assert call(module.accept_global, base) is base
for invoke in (module.accept_global, lambda value: call(module.accept_global, value)):
    marker = object()
    assert invoke(marker) is marker
assert base.optional(None) is None
assert type(base.wrong_return()) is object
assert 'annotation evaluated' not in events
assert _soac_ext.strict_function_diagnostics(module.factory)['original_code_entered'] is True

def collectable_contract_cycle():
    local = module.factory()
    return weakref.ref(local), weakref.ref(local.accept)
references = collectable_contract_cycle()
gc.collect()
assert all(reference() is None for reference in references)
""",
        Path(__file__),
        required_functions=(
            "factory", "Token.accept", "Token.optional", "Token.wrong_return", "accept_global",
        ),
        
        backend="cpython",
    )


_CPYTHON_CLASS_SCOPE_NOMINAL_SOURCE = """
from __future__ import strict
from cpython_class_scope_support import arbitrary_result, events

class Token:
    pass

class Holder:
    Alias = Token

    def accept(self, value: Alias) -> Alias:
        events.append("accept")
        return value

    def wrong_return(self) -> Alias:
        events.append("wrong return")
        return arbitrary_result()

def factory():
    class LocalToken:
        pass

    class LocalHolder:
        Alias = LocalToken

        def accept(self, value: Alias) -> Alias:
            events.append("factory body")
            return value

    return LocalToken, LocalHolder
"""

_CPYTHON_CLASS_SCOPE_NOMINAL_SUPPORT = """
from typing import Any

events = []

def arbitrary_result() -> Any:
    return object()

def class_dictionary_cell(function: Any) -> Any:
    # This fixture has exactly one native capture. The test mutates that actual
    # cell; neither its spelling nor its value grants runtime binding authority.
    provider = function.__annotate__
    cells = provider.__closure__
    assert cells is not None and len(cells) == 1
    return cells[0]
"""


@pytest.fixture(scope="module")
def cpython_class_scope_nominals(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("cpython-class-scope-nominals"),
        {
            "cpython_class_scope.py": _CPYTHON_CLASS_SCOPE_NOMINAL_SOURCE,
            "cpython_class_scope_support.py": _CPYTHON_CLASS_SCOPE_NOMINAL_SUPPORT,
        },
        modules={"cpython_class_scope": "cpython_class_scope.py"},
        backend="cpython",
    )


def test_cpython_class_scope_annotations_do_not_run_during_ordinary_calls(
    cpython_class_scope_nominals,
):
    from pathlib import Path

    cpython_class_scope_nominals.run_case(
        "cpython_class_scope",
        """
import ctypes
import cpython_class_scope as module
from cpython_class_scope_support import class_dictionary_cell, events
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError
from tests._strict_integration import _assert_cpython_function_witness

diagnostic = _soac_ext.strict_module_diagnostics(module)
functions = (module.Holder.accept, module.Holder.wrong_return)
providers = tuple(function.__annotate__ for function in functions)
for function, provider in zip(functions, providers):
    witness = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert witness["finalized"] is True
    assert witness["original_code_entered"] is False
    observed = _assert_cpython_function_witness(
        provider, diagnostic,
    )
    assert observed["original_code_entered"] is False
    namespace = class_dictionary_cell(function).cell_contents
    assert type(namespace) is dict
    assert namespace["Alias"] is module.Token

receiver = module.Holder()
value = module.Token()
before = list(events)
marker = object()
assert receiver.accept(marker) is marker
assert events == before + ["accept"]
assert _soac_ext.strict_function_diagnostics(module.Holder.accept)["original_code_entered"] is True
assert receiver.accept(value) is value

class OrdinaryChild(module.Token):
    pass

child = OrdinaryChild()
assert receiver.accept(child) is child
for _ in range(128):
    assert receiver.accept(value) is value

call = ctypes.pythonapi.PyObject_CallOneArg
call.argtypes = [ctypes.py_object, ctypes.py_object]
call.restype = ctypes.py_object
assert call(receiver.accept, child) is child
before = list(events)
assert call(receiver.accept, marker) is marker
assert events == before + ["accept"]

before = list(events)
assert type(receiver.wrong_return()) is object
assert events == before + ["wrong return"]
assert _soac_ext.strict_function_diagnostics(module.Holder.wrong_return)["original_code_entered"] is True
for provider in providers:
    assert _assert_cpython_function_witness(
        provider, diagnostic,
    )["original_code_entered"] is False
""",
        Path(__file__),
        required_functions=("Holder.accept", "Holder.wrong_return", "factory"),
        
        backend="cpython",
    )


def test_cpython_class_scope_factory_calls_ignore_annotation_dictionary_replacement(
    cpython_class_scope_nominals,
):
    from pathlib import Path

    cpython_class_scope_nominals.run_case(
        "cpython_class_scope",
        """
import cpython_class_scope as module
from cpython_class_scope_support import class_dictionary_cell, events
from soac import _soac_ext
from soac.strict import StrictRuntimeUnavailableError
from tests._strict_integration import _assert_cpython_function_witness

diagnostic = _soac_ext.strict_module_diagnostics(module)
FirstToken, FirstHolder = module.factory()
SecondToken, SecondHolder = module.factory()
assert FirstToken is not SecondToken and FirstHolder is not SecondHolder
assert FirstHolder.__qualname__ == SecondHolder.__qualname__
first_function, second_function = FirstHolder.accept, SecondHolder.accept
first_provider, second_provider = first_function.__annotate__, second_function.__annotate__
assert first_function is not second_function
assert first_function.__code__ is second_function.__code__
assert first_provider is not second_provider
assert first_provider.__code__ is second_provider.__code__
first_cell = class_dictionary_cell(first_function)
second_cell = class_dictionary_cell(second_function)
assert first_cell is not second_cell
first_namespace, second_namespace = first_cell.cell_contents, second_cell.cell_contents
assert type(first_namespace) is dict and type(second_namespace) is dict
assert first_namespace is not second_namespace
assert first_namespace["Alias"] is FirstToken
assert second_namespace["Alias"] is SecondToken
assert first_namespace["accept"] is first_function
assert second_namespace["accept"] is second_function
for function in (first_function, second_function):
    assert _assert_cpython_function_witness(
        function, diagnostic,
    )["finalized"] is True
for provider in (first_provider, second_provider):
    assert _assert_cpython_function_witness(
        provider, diagnostic,
    )["original_code_entered"] is False

left, right = FirstHolder(), SecondHolder()
left_value, right_value = FirstToken(), SecondToken()
assert left.accept(left_value) is left_value
assert right.accept(right_value) is right_value
copied = dict(first_namespace)
copied["Alias"] = SecondToken
try:
    for replacement in (copied, second_namespace, {"Alias": SecondToken}):
        first_cell.cell_contents = replacement
        # Replacing annotation cells does not change original body execution.
        assert left.accept(left_value) is left_value
        assert right.accept(right_value) is right_value
        for method, value in ((left.accept, right_value), (right.accept, left_value)):
            before = list(events)
            assert method(value) is value
            assert events == before + ["factory body"]
finally:
    first_cell.cell_contents = first_namespace
assert left.accept(left_value) is left_value
for provider in (first_provider, second_provider):
    assert _assert_cpython_function_witness(
        provider, diagnostic,
    )["original_code_entered"] is False
""",
        Path(__file__),
        required_functions=("factory",),
        
        backend="cpython",
    )


_CPYTHON_PENDING_CLASS_SCOPE_SOURCE = """
from __future__ import strict
from cpython_pending_class_scope_support import inspect_pending, events

class Base:
    def __init_subclass__(cls):
        inspect_pending(cls)

def factory():
    class Token:
        pass

    class Holder(Base):
        Alias = Token

        def accept(self, value: Alias) -> Alias:
            events.append("body")
            return value

    return Token, Holder

first = factory()
second = factory()
"""

_CPYTHON_PENDING_CLASS_SCOPE_SUPPORT = """
from typing import Any

events = []
namespaces = []

def inspect_pending(cls: Any) -> None:
    from soac import _soac_ext
    from soac.strict import StrictRuntimeUnavailableError

    function = cls.accept
    diagnostic = _soac_ext.strict_function_diagnostics(function)
    assert diagnostic["backend"] == "cpython"
    assert diagnostic["entry_kind"] == "original_code"
    assert diagnostic["finalized"] is False
    assert diagnostic["original_code_entered"] is False
    provider = function.__annotate__
    provider_diagnostic = _soac_ext.strict_function_diagnostics(provider)
    assert provider_diagnostic["backend"] == "cpython"
    assert provider_diagnostic["entry_kind"] == "original_code"
    assert provider_diagnostic["original_code_entered"] is False
    for key in ("source_path", "source_sha256", "artifact_generation"):
        assert provider_diagnostic[key] == diagnostic[key]
    cells = provider.__closure__
    assert cells is not None and len(cells) == 1
    cell = cells[0]
    actual = cell.cell_contents
    assert type(actual) is dict
    assert actual["accept"] is function
    value = actual["Alias"]()
    # Do not instantiate the pending class. The receiver is unused and this
    # unbound call does not write protected storage or evaluate annotations.
    assert function(None, value) is value
    assert _soac_ext.strict_function_diagnostics(function)["finalized"] is False

    alternatives = [dict(actual)]
    if namespaces:
        previous = namespaces[-1]
        assert previous is not actual
        assert previous["accept"].__code__ is function.__code__
        assert previous["accept"].__annotate__.__code__ is provider.__code__
        alternatives.append(previous)
    try:
        for replacement in alternatives:
            cell.cell_contents = replacement
            before = list(events)
            assert function(None, value) is value
            assert events == before + ["body"]
            assert _soac_ext.strict_function_diagnostics(provider)["original_code_entered"] is False
    finally:
        cell.cell_contents = actual
    assert function(None, value) is value
    assert _soac_ext.strict_function_diagnostics(provider)["original_code_entered"] is False
    namespaces.append(actual)
    events.append(("pending ordinary call", len(namespaces)))
"""


def test_cpython_pending_class_calls_keep_source_ownership_without_annotation_lookup(tmp_path):
    project = create_strict_project(
        tmp_path,
        {
            "cpython_pending_class_scope.py": _CPYTHON_PENDING_CLASS_SCOPE_SOURCE,
            "cpython_pending_class_scope_support.py": _CPYTHON_PENDING_CLASS_SCOPE_SUPPORT,
        },
        modules={"cpython_pending_class_scope": "cpython_pending_class_scope.py"},
        backend="cpython",
    )
    project.run_case(
        "cpython_pending_class_scope",
        """
import cpython_pending_class_scope as module
from cpython_pending_class_scope_support import events
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness

assert events == [
    "body", "body", "body", ("pending ordinary call", 1),
    "body", "body", "body", "body", ("pending ordinary call", 2),
]
diagnostic = _soac_ext.strict_module_diagnostics(module)
FirstToken, FirstHolder = module.first
SecondToken, SecondHolder = module.second
assert FirstToken is not SecondToken and FirstHolder is not SecondHolder
for Token, Holder in (module.first, module.second):
    observed = _assert_cpython_function_witness(
        Holder.accept, diagnostic,
    )
    assert observed["finalized"] is True
    assert observed["original_code_entered"] is True
    provider = Holder.accept.__annotate__
    assert _assert_cpython_function_witness(
        provider, diagnostic,
    )["original_code_entered"] is False
    value = Token()
    assert Holder().accept(value) is value
""",
        tmp_path / "cpython_pending_class_scope_validation.py",
        required_functions=("factory",),
        
        backend="cpython",
    )


_RETAINED_EARLY_MODULE_NOMINAL_SOURCE = """
from __future__ import strict
from retained_early_nominal_support import exercise, move_alias

class Token:
    pass

Alias = Token

class Holder:
    def accept(self, /, value: Alias) -> Alias:
        move_alias(Holder.accept, Token)
        return value

# The actual class result has already enabled instances. Metadata must be
# immutable now, while this module's actual alias values remain per-call.
holder = Holder()
exercise(holder, Token)
"""

_RETAINED_EARLY_MODULE_NOMINAL_SUPPORT = """
import ctypes

expect_strict = True
events = []
old_values = []
shifting = False

def move_alias(function, current):
    if shifting:
        function.__globals__['Alias'] = current
    events.append('body')

def exercise(receiver, current):
    global shifting
    shifting = True
    from soac.strict import StrictMutationError

    function = type(receiver).accept
    strict_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
    strict_id.argtypes = [ctypes.py_object]
    strict_id.restype = ctypes.c_uint64
    sealed = ctypes.pythonapi.PyType_IsSoacSealed
    sealed.argtypes = [ctypes.py_object]
    sealed.restype = ctypes.c_int
    if expect_strict:
        assert strict_id(function), 'instances opened before function metadata sealing'
        assert sealed(type(receiver)) == 1
        try:
            function.__defaults__ = (object(),)
        except StrictMutationError:
            pass
        else:
            raise AssertionError('admitted method metadata remained mutable')
    else:
        assert not strict_id(function) and not sealed(type(receiver))

    class Previous:
        pass
    old = Previous()
    old_values.append(old)

    class Keyword(str):
        __hash__ = str.__hash__
        def __eq__(self, other):
            # This is the same actual binder callback as the initializing
            # nominal fixture, now on a mandatorily frozen class method.
            function.__globals__['Alias'] = Previous
            events.append('keyword')
            return str.__eq__(self, other)

    assert receiver.accept(**{Keyword('value'): old}) is old
    assert function.__globals__['Alias'] is current
    fresh = current()
    assert receiver.accept(fresh) is fresh
    before = list(events)
    assert receiver.accept(old) is old
    assert events == before + ['body']
    events.append('ordinary-next-call')
    shifting = False
"""


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_soac_early_sealed_method_keeps_metadata_seals_and_ordinary_binding_callbacks(
    tmp_path, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "retained_early_nominals.py": _RETAINED_EARLY_MODULE_NOMINAL_SOURCE,
            "ordinary_early_nominals.py": _RETAINED_EARLY_MODULE_NOMINAL_SOURCE.replace(
                "from __future__ import strict\n", ""
            ),
            "retained_early_nominal_support.py": _RETAINED_EARLY_MODULE_NOMINAL_SUPPORT,
        },
        modules={"retained_early_nominals": "retained_early_nominals.py"},
        backend="soac",
    )
    project.run_case(
        "retained_early_nominals",
        """
import retained_early_nominals as module
import retained_early_nominal_support as support

assert support.events == ['keyword', 'body', 'body', 'body', 'ordinary-next-call']
old = support.old_values[0]
value = module.Token()
assert module.holder.accept(value) is value
assert module.holder.accept(old) is old
assert module.Holder.accept.__defaults__ is None

support.expect_strict = False
support.events.clear()
import ordinary_early_nominals as ordinary
assert support.events == [
    'keyword', 'body', 'body', 'body', 'ordinary-next-call',
]
assert type(ordinary.holder) is ordinary.Holder
""",
        tmp_path / "retained_early_nominal_validation.py",
        required_functions=("Holder.accept",),
        
        entry_interpreter=entry_interpreter,
        backend="soac",
        opt_mode="none",
    )


_RETAINED_PENDING_CLASS_SCOPE_SUPPORT = """
import ctypes

events = []
namespaces = []

def inspect_pending(cls):
    from soac import _soac_ext
    from soac.strict import StrictRuntimeUnavailableError

    function = cls.accept
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    strict_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
    strict_id.argtypes = [ctypes.py_object]
    strict_id.restype = ctypes.c_uint64
    own = ctypes.pythonapi.PyType_HasSoacContract
    own.argtypes = [ctypes.py_object]
    own.restype = ctypes.c_int
    assert owner(function) and strict_id(function) == 0
    assert own(cls) == 0
    assert _soac_ext.strict_function_entry_kind(function) in (
        'checked_native', 'entry_interpreter',
    )
    provider = function.__annotate__
    # Observe the actual provider cell, not a reconstructed module dictionary.
    cells = [
        cell for cell in provider.__closure__ or ()
        if type(cell.cell_contents) is dict
        and cell.cell_contents.get('accept') is function
    ]
    assert len(cells) == 1
    cell = cells[0]
    actual = cell.cell_contents
    value = actual['Alias']()
    assert function(None, value) is value
    alternatives = [dict(actual)]
    if namespaces:
        previous = namespaces[-1]
        assert previous is not actual
        assert previous['accept'].__code__ is function.__code__
        alternatives.append(previous)
    try:
        for replacement in alternatives:
            cell.cell_contents = replacement
            before = list(events)
            assert function(None, value) is value
            assert events == before + ['body']
    finally:
        cell.cell_contents = actual
    assert function(None, value) is value
    namespaces.append(actual)
    events.append(('pending ordinary call', len(namespaces)))
"""


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_soac_pending_class_calls_keep_source_ownership_without_annotation_lookup(
    tmp_path, entry_interpreter
):
    project = create_strict_project(
        tmp_path,
        {
            "cpython_pending_class_scope.py": _CPYTHON_PENDING_CLASS_SCOPE_SOURCE,
            "cpython_pending_class_scope_support.py": _RETAINED_PENDING_CLASS_SCOPE_SUPPORT,
        },
        modules={"cpython_pending_class_scope": "cpython_pending_class_scope.py"},
        backend="soac",
    )
    project.run_case(
        "cpython_pending_class_scope",
        """
import cpython_pending_class_scope as module
from cpython_pending_class_scope_support import events

assert events == [
    'body', 'body', 'body', ('pending ordinary call', 1),
    'body', 'body', 'body', 'body', ('pending ordinary call', 2),
]
FirstToken, FirstHolder = module.first
SecondToken, SecondHolder = module.second
assert FirstToken is not SecondToken and FirstHolder is not SecondHolder
for Token, Holder in (module.first, module.second):
    value = Token()
    assert Holder().accept(value) is value
other = SecondToken()
assert FirstHolder().accept(other) is other
""",
        tmp_path / "retained_pending_namespace_validation.py",
        required_functions=("factory",),
        
        entry_interpreter=entry_interpreter,
        backend="soac",
        opt_mode="none",
    )
