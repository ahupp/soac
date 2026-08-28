"""An unrecognized actual decorator keeps ordinary call and temporary lifetimes."""

import hashlib
from pathlib import Path

import pytest

from tests._strict_integration import ROOT, create_strict_project

_SUPPORT = """
import gc
import weakref

events = []
failure = ''
discard_class = False
class_ref = None
replacement = object()
last_decorator_ref = None
escaped_preparations = []

def capture_preparation():
    decorator = last_decorator_ref() if last_decorator_ref is not None else None
    if decorator is not None:
        for owner in gc.get_referrers(decorator):
            if type(owner).__name__ == '_ClassDecoratorPreparation':
                escaped_preparations.append(owner)

def reached(stage: str) -> None:
    if stage == 'body':
        capture_preparation()
    events.append(stage)
    if failure == stage:
        raise RuntimeError(stage)

class Decorator:
    def __call__(self, cls):
        global class_ref
        reached('apply')
        class_ref = weakref.ref(cls)
        if discard_class:
            return replacement
        return cls

    def __del__(self):
        events.append('decorator_del')
        if discard_class and class_ref is not None:
            gc.collect()
            events.append(('class_alive_at_decorator_del', class_ref() is not None))

def factory(*args, **kwargs):
    global last_decorator_ref
    assert args == () and kwargs == {'eq': False}
    reached('factory')
    decorator = Decorator()
    last_decorator_ref = weakref.ref(decorator)
    return decorator

async def pause() -> bool:
    reached('await')
    return True
"""

_MODELS = """
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
import decline_support as support

class Base:
    pass

def build():
    @dataclass(eq=False)
    class Item(Base):
        support.reached('body')
    return Item

async def build_async():
    @dataclass(eq=False)
    class Item(Base if await support.pause() else Base):
        support.reached('body')
    return Item
"""


@pytest.fixture(scope="module")
def project(request, tmp_path_factory):
    backend = getattr(request, "param", "soac")
    return create_strict_project(
        tmp_path_factory.mktemp(f"strict-dataclass-decline-{backend}"),
        {"decline_models.py": _MODELS, "decline_support.py": _SUPPORT},
        modules={"decline_models": "decline_models.py"},
        backend=backend,
    )


@pytest.mark.parametrize(
    ("project", "entry_interpreter"),
    [
        pytest.param("soac", False, id="False"),
        pytest.param("soac", True, id="True"),
        pytest.param("cpython", False, id="cpython"),
    ],
    indirect=["project"],
    scope="module",
)
def test_unknown_dataclass_factory_runs_once_and_cleans_its_preparation(
    project, entry_interpreter
):
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    native_backend = project.backend == "cpython"
    native_witness = ""
    if native_backend:
        source_path = project.project / "decline_models.py"
        source_hash = hashlib.sha256(source_path.read_bytes()).hexdigest()
        native_witness = f"""
sys.path.insert(0, {str(ROOT)!r})
from tests._strict_integration import (
    _assert_cpython_function_witness, _assert_cpython_module_witness,
)
from tests.test_strict_type_native import ConstructionInfoV1

construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
construction.restype = ctypes.c_int

def native_source_witness(entered):
    diagnostic = _assert_cpython_module_witness(
        model, module_name="decline_models", source_path={str(source_path)!r},
        source_sha256={source_hash!r},
        artifact_generation={project.publication["generation"]!r},
    )
    for function in (model.build, model.build_async):
        observed = _assert_cpython_function_witness(
            function, diagnostic,
        )
        assert observed["original_code_entered"] is entered
    info = ConstructionInfoV1()
    assert construction(model.Base, ctypes.byref(info), ctypes.sizeof(info)) == 1
    assert info.phase == 3 and info.permanent_contract_published == 1
    assert info.owner is not None
"""
    before_witness = (
        "native_source_witness(False)" if native_backend else
        f"assert _soac_ext.strict_function_entry_kind(model.build) == {expected_entry!r}"
    )
    after_witness = (
        "native_source_witness(True)" if native_backend else before_witness
    )
    result = project.run(
        f"""
import asyncio
import ctypes
import dataclasses
import gc
import types
import decline_support as support

original = dataclasses.dataclass
dataclasses.dataclass = support.factory
try:
    import decline_models as model
    stock = types.ModuleType('ordinary_decline_models')
    exec(compile({_MODELS!r}.replace('# soac: module(strict_assign=true, checked_attr=true)\\n', ''),
                 '<ordinary decorator decline>', 'exec'), vars(stock))
finally:
    dataclasses.dataclass = original

{native_witness}
{before_witness}
owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p  # borrowed native owner, never ctypes.py_object

def exercise(module, asynchronous, failure, discard):
    gc.collect()
    support.events.clear()
    support.failure = failure
    support.discard_class = discard
    support.class_ref = None
    support.last_decorator_ref = None
    support.escaped_preparations.clear()
    try:
        cls = asyncio.run(module.build_async()) if asynchronous else module.build()
    except RuntimeError as error:
        assert str(error) == failure
        support.events.append('caught')
    else:
        assert not failure
        if discard:
            assert cls is support.replacement
        else:
            assert owner(cls) is None, 'unknown actual decorator acquired class authority'
            if {native_backend!r} and module is model:
                info = ConstructionInfoV1()
                assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
        support.events.append('returned')
        del cls
    gc.collect()
    if support.last_decorator_ref is not None:
        assert support.last_decorator_ref() is None, 'escaped preparation retained decorator'
    if not {native_backend!r} and module is model and failure not in ('factory', 'await'):
        # The retained preparation carrier is not part of original-code execution.
        # Its original assertion remains required for both retained variants.
        assert support.escaped_preparations, 'the selected decorator path was not exercised'
    return support.events.copy()

for asynchronous in (False, True):
    failures = ('', 'factory', 'body', 'apply', 'await') if asynchronous else (
        '', 'factory', 'body', 'apply'
    )
    for discard in (False, True):
        for failure in failures:
            expected = exercise(stock, asynchronous, failure, discard)
            actual = exercise(model, asynchronous, failure, discard)
            assert actual == expected, (asynchronous, failure, discard, actual, expected)
            assert actual.count('factory') == 1
            assert actual.count('apply') == (failure not in ('factory', 'body', 'await'))
            if failure in ('body', 'await'):
                assert actual.index('decorator_del') < actual.index('caught')
            if discard and not failure:
                assert ('class_alive_at_decorator_del', False) in actual
{after_witness}
""",
        entry_interpreter=entry_interpreter,
    )
    assert result.returncode == 0, result.stdout + result.stderr


_UNKNOWN_OPTION_MODELS = """
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import dataclass
import unknown_option_support as support

def checked(value: int) -> int:
    return value

def build():
    @dataclass(eq=support.option())
    class Item:
        support.events.append("body")
        value: int

        def echo(self, value: int) -> int:
            return value

    return Item
"""

_UNKNOWN_OPTION_SUPPORT = """
from typing import Any

events = []

class Truth:
    def __bool__(self):
        events.append("truth")
        return False

def option() -> Any:
    events.append("option")
    return Truth()
"""


def test_cpython_unknown_dataclass_option_preserves_stdlib_truth_and_dynamic_class(
    tmp_path,
):
    project = create_strict_project(
        tmp_path,
        {
            "unknown_option_model.py": _UNKNOWN_OPTION_MODELS,
            "unknown_option_support.py": _UNKNOWN_OPTION_SUPPORT,
        },
        modules={"unknown_option_model": "unknown_option_model.py"},
        backend="cpython",
    )
    project.run_case(
        "unknown_option_model",
        f"source = {_UNKNOWN_OPTION_MODELS!r}\n"
        + """
import ctypes
import dataclasses
import sys
import types
import unknown_option_model as model
import unknown_option_support as support
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
function_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
function_owner.argtypes = [ctypes.py_object]
function_owner.restype = ctypes.c_void_p
construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
construction.restype = ctypes.c_int
generic_set = ctypes.pythonapi.PyObject_GenericSetAttr
generic_set.argtypes = [ctypes.py_object] * 3
generic_set.restype = ctypes.c_int
call_one = ctypes.pythonapi.PyObject_CallOneArg
call_one.argtypes = [ctypes.py_object, ctypes.py_object]
call_one.restype = ctypes.py_object

stock = types.ModuleType("ordinary_unknown_dataclass_option")
sys.modules[stock.__name__] = stock
exec(compile(source.replace("# soac: module(strict_assign=true, checked_attr=true)", ""),
             "<ordinary unknown dataclass option>", "exec"), vars(stock))

def exercise(source_module):
    support.events.clear()
    cls = source_module.build()
    assert dataclasses.is_dataclass(cls)
    assert owner(cls) is None, "an unknown option granted permanent class authority"
    info = ConstructionInfoV1()
    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert not function_owner(cls.__init__)
    value = object()
    instance = cls(value)
    assert instance.value is value
    assert instance.echo(value) is value
    replacement = object()
    assert generic_set(instance, "value", replacement) == 0
    assert instance.value is replacement
    assert call_one(cls, value).value is value
    if source_module is model:
        diagnostic = _soac_ext.strict_module_diagnostics(model)
        observed = _assert_cpython_function_witness(
            cls.echo, diagnostic,
        )
        assert observed["original_code_entered"]
    # Do not guess how often the real dataclass implementation consults eq.
    # Its original truth calls must happen, in exactly the ordinary order.
    events = tuple(support.events)
    assert events.count("option") == 1 and events.count("body") == 1
    assert events.count("truth") > 0
    return events

expected = exercise(stock)
actual = exercise(model)
assert actual == expected, (actual, expected)
assert model.checked(3) == 3
assert model.checked("ordinary annotated value") == "ordinary annotated value"
diagnostic = _soac_ext.strict_module_diagnostics(model)
for function in (model.build, model.checked):
    observed = _assert_cpython_function_witness(
        function, diagnostic,
    )
    assert observed["original_code_entered"]
""",
        Path(__file__),
        required_functions=("build", "checked"),
        
        backend="cpython",
    )
