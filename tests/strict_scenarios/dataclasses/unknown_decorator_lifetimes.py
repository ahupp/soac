# modes:soac,entry,cpython
# module:decline_setup
import asyncio
import ctypes
import dataclasses
import gc
import types
import decline_support as support

original = dataclasses.dataclass
setattr(dataclasses, 'dataclass', support.factory)
try:
    import decline_models as model
    stock = types.ModuleType('ordinary_decline_models')
    exec(compile("\n# soac: module(strict_assign=true, checked_attr=true)\nfrom dataclasses import dataclass\nimport decline_support as support\n\nclass Base:\n    pass\n\ndef build():\n    @dataclass(eq=False)\n    class Item(Base):\n        support.reached('body')\n    return Item\n\nasync def build_async():\n    @dataclass(eq=False)\n    class Item(Base if await support.pause() else Base):\n        support.reached('body')\n    return Item\n".replace('# soac: module(strict_assign=true, checked_attr=true)\n', ''),
                 '<ordinary decorator decline>', 'exec'), vars(stock))
finally:
    setattr(dataclasses, 'dataclass', original)
# module:decline_models
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
# module:decline_support
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
# ok
# test_unknown_dataclass_factory_runs_once_and_cleans_its_preparation [default]
import sys
from soac import _soac_ext
if __dp_integration_mode__ == 'cpython':

    import asyncio
    import ctypes
    import dataclasses
    import gc
    import types
    import decline_support as support

    from decline_setup import model, stock


    sys.path.insert(0, str(__import__('tests._strict_integration', fromlist=['ROOT']).ROOT))
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
            model, module_name="decline_models", source_path=str(__import__('pathlib').Path(sys.modules['decline_models'].__file__)),
            source_sha256='16fcdc9de5ef95e058ac605183e72a735b38cd8325ac5e9c0efc537e3e9ecd5a',
            artifact_generation=_soac_ext.strict_module_diagnostics(sys.modules['decline_models'])['artifact_generation'],
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

    native_source_witness(False)
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
                if True and module is model:
                    info = ConstructionInfoV1()
                    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
            support.events.append('returned')
            del cls
        gc.collect()
        if support.last_decorator_ref is not None:
            assert support.last_decorator_ref() is None, 'escaped preparation retained decorator'
        if not True and module is model and failure not in ('factory', 'await'):
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
    native_source_witness(True)

else:

    import asyncio
    import ctypes
    import dataclasses
    import gc
    import types
    import decline_support as support

    from decline_setup import model, stock


    assert _soac_ext.strict_function_entry_kind(model.build) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
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
                if False and module is model:
                    info = ConstructionInfoV1()
                    assert construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
            support.events.append('returned')
            del cls
        gc.collect()
        if support.last_decorator_ref is not None:
            assert support.last_decorator_ref() is None, 'escaped preparation retained decorator'
        if not False and module is model and failure not in ('factory', 'await'):
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
    assert _soac_ext.strict_function_entry_kind(model.build) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
