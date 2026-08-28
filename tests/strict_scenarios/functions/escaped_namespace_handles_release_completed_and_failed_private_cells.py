# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:private_capture_model
# soac: module(strict_assign=true, checked_attr=true)
from private_capture_support import argument, observe_namespace, observe_target, pause, replace_public_closure

class Base:
    def __init_subclass__(cls, *, flag: bool = False):
        super().__init_subclass__()

def namespace_factory():
    marker = object()
    class Captured:
        captured = marker
    return Captured

def build(should_fail: bool = False):
    class Target:
        pass
    observe_target(Target)
    class Holder(Base, flag=argument(should_fail)):
        def __init__(self, value):
            self.payload: Target = value
    return Target, Holder

def nested_namespace(should_fail: bool = False):
    class Target:
        pass
    observe_target(Target)
    class Outer:
        observe_namespace(False)
        class Holder:
            observe_namespace(should_fail)
            def __init__(self, value):
                self.payload: Target = value
    return Target, Outer.Holder

def make_public_bridge():
    class Target:
        pass
    observe_target(Target)
    def create():
        current = Target
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return current, Holder
    return create

# An ordinary callback changes the native closure while this module is still
# initializing. The eligible source function freezes only at module sealing.
public_bridge = replace_public_closure(make_public_bridge())

def private_bridge_family():
    class Target:
        pass
    def create():
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Holder
    return Target, create

def private_generator_family():
    class Target:
        pass
    def create():
        yield None
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        yield Holder
    return Target, create

def private_coroutine_family():
    class Target:
        pass
    async def create():
        await pause()
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        return Holder
    return Target, create

def private_async_generator_family():
    class Target:
        pass
    async def create():
        yield None
        class Holder:
            def __init__(self, value):
                self.payload: Target = value
        yield Holder
    return Target, create

def terminal_generator_family(payload):
    def create():
        yield None
        return payload is None
    return create

def terminal_coroutine_family(payload):
    async def create():
        await pause()
        return payload is None
    return create

def terminal_async_generator_family(payload):
    async def create():
        yield None
        if payload is None:
            return
    return create
# module:private_capture_support
from collections.abc import Callable
import gc
import weakref
import ctypes

targets: list[weakref.ReferenceType[type]] = []
replay: Callable[[], None] | None = None
namespace_handles: list[object] = []

class PublicReplacement:
    pass

class PublicAfter:
    pass

class Pause:
    def __await__(self):
        yield "paused"
        return None

def pause():
    return Pause()

def make_cell(value):
    return (lambda: value).__closure__[0]

def replace_public_closure(function):
    setter = ctypes.pythonapi.PyFunction_SetClosure
    setter.argtypes = [ctypes.py_object, ctypes.py_object]
    setter.restype = ctypes.c_int
    assert function.__code__.co_freevars == ("Target",)
    assert setter(function, (make_cell(PublicReplacement),)) == 0
    return function

def observe_target(value: type) -> None:
    targets.append(weakref.ref(value))

def observe_namespace(should_fail: bool) -> None:
    for value in gc.get_objects():
        if type(value).__name__ == "_StrictNamespaceExecution":
            if not any(value is previous for previous in namespace_handles):
                namespace_handles.append(value)
    if should_fail:
        raise RuntimeError("nested namespace failed")

def argument(should_fail: bool) -> bool:
    callback = replay
    if callback is not None:
        callback()
    if should_fail:
        raise RuntimeError("class argument failed")
    return False
# ok
# tests/test_strict_function_boundaries.py::test_escaped_namespace_handles_release_completed_and_failed_private_cells
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('nested_namespace',):
        _scenario_function = _plain_function_witness(module, _scenario_name)
        if __dp_integration_mode__ == "cpython":
            _assert_cpython_function_witness(
                _scenario_function, _soac_ext.strict_module_diagnostics(module),
            )
        else:
            import ctypes
            _scenario_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
            _scenario_metadata.argtypes = [ctypes.py_object]
            _scenario_metadata.restype = ctypes.c_void_p
            _scenario_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            _scenario_owner.argtypes = [ctypes.py_object]
            _scenario_owner.restype = ctypes.c_void_p
            assert _scenario_metadata(_scenario_function), _scenario_name
            assert _scenario_owner(_scenario_function), _scenario_name
            _scenario_expected = ("entry_interpreter" if __dp_integration_entry__ else "checked_native")
            assert _soac_ext.strict_function_entry_kind(_scenario_function) == _scenario_expected
        del _scenario_function

_assert_source_function_witnesses()

def validate(module):
    import ctypes
    import gc
    import weakref
    import private_capture_support as support

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    target, holder = module.nested_namespace()
    assert owner(holder), "nested field class did not obtain its actual contract"
    instance = holder(target())
    assert type(instance.payload) is target
    try:
        holder(object())
    except TypeError:
        pass
    else:
        raise AssertionError("nested field predicate was not enforced")
    target_ref, holder_ref = weakref.ref(target), weakref.ref(holder)
    del target, holder, instance
    gc.collect()
    assert support.namespace_handles
    assert target_ref() is None and holder_ref() is None

    try:
        module.nested_namespace(True)
    except RuntimeError as error:
        assert str(error) == "nested namespace failed"
    else:
        raise AssertionError("nested class body failure was lost")
    gc.collect()
    assert support.targets[-1]() is None, "failed namespace retained its original cell"
    for handle in support.namespace_handles:
        assert all(
            value is None or value is type(handle)
            for value in gc.get_referents(handle)
        ), "escaped namespace handle kept temporary Python edges"

validate(module)

_assert_source_function_witnesses()
