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
# module:ordinary_private_capture
# ordinary metadata control
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
# tests/test_strict_function_boundaries.py::test_private_field_forwarding_uses_the_current_native_closure_after_preseal_replacement
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('make_public_bridge', 'public_bridge'):
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
    import ordinary_private_capture
    import private_capture_support as support
    from soac import _soac_ext
    from soac.strict import StrictMutationError

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    setter = ctypes.pythonapi.PyFunction_SetClosure
    setter.argtypes = [ctypes.py_object, ctypes.py_object]
    setter.restype = ctypes.c_int
    ordinary = ordinary_private_capture.public_bridge
    actual = module.public_bridge
    assert actual.__code__.co_freevars == ordinary.__code__.co_freevars == ("Target",)
    assert _soac_ext.strict_function_entry_kind(actual) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
    assert actual.__closure__[0].cell_contents is support.PublicReplacement
    expected_type, _ = ordinary()
    selected, first = actual()
    assert selected is expected_type is support.PublicReplacement
    assert owner(first)
    first(support.PublicReplacement())
    try:
        first(support.PublicAfter())
    except TypeError:
        pass
    else:
        raise AssertionError("field used the original pre-replacement cell")

    # Full freeze forbids another tuple substitution, but never freezes
    # the contents of the actual, originally adopted cell.
    saved = actual.__closure__
    try:
        setter(actual, (support.make_cell(support.PublicAfter),))
    except StrictMutationError:
        pass
    else:
        raise AssertionError("sealed source function allowed closure replacement")
    assert actual.__closure__ is saved
    saved[0].cell_contents = support.PublicAfter
    ordinary.__closure__[0].cell_contents = support.PublicAfter
    expected_type, _ = ordinary()
    selected, second = actual()
    assert selected is expected_type is support.PublicAfter
    assert owner(second)
    second(support.PublicAfter())
    first(support.PublicReplacement())
    for cls, wrong in ((first, support.PublicAfter), (second, support.PublicReplacement)):
        try:
            cls(wrong())
        except TypeError:
            pass
        else:
            raise AssertionError("a later cell mutation retargeted an existing field policy")

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_private_lexical_cells_live_with_their_function_or_suspended_frame_only
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('private_bridge_family',):
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
    import ordinary_private_capture
    from soac import _soac_ext

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    factory = getattr(module, 'private_bridge_family')
    ordinary_factory = getattr(ordinary_private_capture, 'private_bridge_family')
    ordinary_target, ordinary_create = ordinary_factory()
    target, create = factory()
    assert factory.__code__.co_cellvars == ordinary_factory.__code__.co_cellvars == ()
    assert create.__code__.co_freevars == ordinary_create.__code__.co_freevars == ()
    assert create.__closure__ is ordinary_create.__closure__ is None
    assert create.__annotate__ is ordinary_create.__annotate__ is None
    expected = "generator_factory" if 'bridge' != "bridge" else ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
    assert _soac_ext.strict_function_entry_kind(create) == expected
    target_ref = weakref.ref(target)
    del ordinary_target, ordinary_create, target
    gc.collect()
    assert target_ref() is not None, "returned private bridge lost its required original cell"
    frame = None
    if 'bridge' == "bridge":
        holder = create()
        del create
    elif 'bridge' == "generator":
        frame = create()
        del create
        assert next(frame) is None
        gc.collect()
        assert target_ref() is not None, "suspension lost its private cell"
        holder = next(frame)
    else:
        frame = create()
        del create
        assert frame.send(None) == "paused"
        gc.collect()
        assert target_ref() is not None, "coroutine suspension lost its private cell"
        try:
            frame.send(None)
        except StopIteration as completed:
            holder = completed.value
        else:
            raise AssertionError("coroutine did not return its class")

    assert owner(holder)
    instance = holder(target_ref()())
    assert type(instance.payload) is target_ref()
    try:
        holder(object())
    except TypeError:
        pass
    else:
        raise AssertionError("private field predicate was lost")
    if frame is not None:
        frame.close()
    holder_ref = weakref.ref(holder)
    del holder, instance
    gc.collect()
    assert holder_ref() is None, "closed private frame retained the constructed class"
    assert target_ref() is None, "private cells outlived every consumer, including a closed frame"

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_private_lexical_cells_live_with_their_function_or_suspended_frame_only
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('private_generator_family',):
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
    import ordinary_private_capture
    from soac import _soac_ext

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    factory = getattr(module, 'private_generator_family')
    ordinary_factory = getattr(ordinary_private_capture, 'private_generator_family')
    ordinary_target, ordinary_create = ordinary_factory()
    target, create = factory()
    assert factory.__code__.co_cellvars == ordinary_factory.__code__.co_cellvars == ()
    assert create.__code__.co_freevars == ordinary_create.__code__.co_freevars == ()
    assert create.__closure__ is ordinary_create.__closure__ is None
    assert create.__annotate__ is ordinary_create.__annotate__ is None
    expected = "generator_factory" if 'generator' != "bridge" else ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
    assert _soac_ext.strict_function_entry_kind(create) == expected
    target_ref = weakref.ref(target)
    del ordinary_target, ordinary_create, target
    gc.collect()
    assert target_ref() is not None, "returned private bridge lost its required original cell"
    frame = None
    if 'generator' == "bridge":
        holder = create()
        del create
    elif 'generator' == "generator":
        frame = create()
        del create
        assert next(frame) is None
        gc.collect()
        assert target_ref() is not None, "suspension lost its private cell"
        holder = next(frame)
    else:
        frame = create()
        del create
        assert frame.send(None) == "paused"
        gc.collect()
        assert target_ref() is not None, "coroutine suspension lost its private cell"
        try:
            frame.send(None)
        except StopIteration as completed:
            holder = completed.value
        else:
            raise AssertionError("coroutine did not return its class")

    assert owner(holder)
    instance = holder(target_ref()())
    assert type(instance.payload) is target_ref()
    try:
        holder(object())
    except TypeError:
        pass
    else:
        raise AssertionError("private field predicate was lost")
    if frame is not None:
        frame.close()
    holder_ref = weakref.ref(holder)
    del holder, instance
    gc.collect()
    assert holder_ref() is None, "closed private frame retained the constructed class"
    assert target_ref() is None, "private cells outlived every consumer, including a closed frame"

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_private_lexical_cells_live_with_their_function_or_suspended_frame_only
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('private_coroutine_family',):
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
    import ordinary_private_capture
    from soac import _soac_ext

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    factory = getattr(module, 'private_coroutine_family')
    ordinary_factory = getattr(ordinary_private_capture, 'private_coroutine_family')
    ordinary_target, ordinary_create = ordinary_factory()
    target, create = factory()
    assert factory.__code__.co_cellvars == ordinary_factory.__code__.co_cellvars == ()
    assert create.__code__.co_freevars == ordinary_create.__code__.co_freevars == ()
    assert create.__closure__ is ordinary_create.__closure__ is None
    assert create.__annotate__ is ordinary_create.__annotate__ is None
    expected = "generator_factory" if 'coroutine' != "bridge" else ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
    assert _soac_ext.strict_function_entry_kind(create) == expected
    target_ref = weakref.ref(target)
    del ordinary_target, ordinary_create, target
    gc.collect()
    assert target_ref() is not None, "returned private bridge lost its required original cell"
    frame = None
    if 'coroutine' == "bridge":
        holder = create()
        del create
    elif 'coroutine' == "generator":
        frame = create()
        del create
        assert next(frame) is None
        gc.collect()
        assert target_ref() is not None, "suspension lost its private cell"
        holder = next(frame)
    else:
        frame = create()
        del create
        assert frame.send(None) == "paused"
        gc.collect()
        assert target_ref() is not None, "coroutine suspension lost its private cell"
        try:
            frame.send(None)
        except StopIteration as completed:
            holder = completed.value
        else:
            raise AssertionError("coroutine did not return its class")

    assert owner(holder)
    instance = holder(target_ref()())
    assert type(instance.payload) is target_ref()
    try:
        holder(object())
    except TypeError:
        pass
    else:
        raise AssertionError("private field predicate was lost")
    if frame is not None:
        frame.close()
    holder_ref = weakref.ref(holder)
    del holder, instance
    gc.collect()
    assert holder_ref() is None, "closed private frame retained the constructed class"
    assert target_ref() is None, "private cells outlived every consumer, including a closed frame"

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_closing_one_frame_keeps_another_frames_original_private_cells
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('private_generator_family',):
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
    import ordinary_private_capture
    from soac import _soac_ext

    def finish_awaitable(awaitable):
        iterator = awaitable.__await__()
        try:
            unexpected = next(iterator)
        except StopIteration as completed:
            return completed.value
        raise AssertionError(("unexpected async suspension", unexpected))

    def start(frame):
        if 'generator' == "async_generator":
            assert finish_awaitable(frame.__anext__()) is None
        elif 'generator' == "coroutine":
            assert frame.send(None) == "paused"
        else:
            assert next(frame) is None

    def close(frame):
        if 'generator' == "async_generator":
            assert finish_awaitable(frame.aclose()) is None
        else:
            frame.close()

    def code_of(frame):
        if 'generator' == "async_generator":
            return frame.ag_code
        if 'generator' == "coroutine":
            return frame.cr_code
        return frame.gi_code

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    ordinary_target, ordinary_create = getattr(ordinary_private_capture, 'private_generator_family')()
    target, create = getattr(module, 'private_generator_family')()
    assert create.__code__.co_freevars == ordinary_create.__code__.co_freevars == ()
    assert create.__closure__ is ordinary_create.__closure__ is None
    assert create.__annotate__ is ordinary_create.__annotate__ is None
    assert _soac_ext.strict_function_entry_kind(create) == "generator_factory"
    original_code = create.__code__
    target_ref, create_ref = weakref.ref(target), weakref.ref(create)
    first, second = create(), create()
    del ordinary_target, ordinary_create, target, create
    start(first)
    start(second)
    close(first)
    assert code_of(first) is original_code
    gc.collect()
    assert target_ref() is not None and create_ref() is not None, "one close cleared another frame's cells"

    if 'generator' == "async_generator":
        holder = finish_awaitable(second.__anext__())
    elif 'generator' == "coroutine":
        try:
            second.send(None)
        except StopIteration as completed:
            holder = completed.value
        else:
            raise AssertionError("coroutine did not return its class")
    else:
        holder = next(second)
    assert owner(holder)
    instance = holder(target_ref()())
    assert type(instance.payload) is target_ref()
    try:
        holder(object())
    except TypeError:
        pass
    else:
        raise AssertionError("the second frame lost its required field predicate")
    close(second)
    assert code_of(first) is code_of(second) is original_code
    holder_ref = weakref.ref(holder)
    del holder, instance
    gc.collect()
    assert holder_ref() is None
    assert create_ref() is None, "closed wrappers retained their source function"
    assert target_ref() is None, "closed wrappers retained their private target"

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_closing_one_frame_keeps_another_frames_original_private_cells
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('private_coroutine_family',):
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
    import ordinary_private_capture
    from soac import _soac_ext

    def finish_awaitable(awaitable):
        iterator = awaitable.__await__()
        try:
            unexpected = next(iterator)
        except StopIteration as completed:
            return completed.value
        raise AssertionError(("unexpected async suspension", unexpected))

    def start(frame):
        if 'coroutine' == "async_generator":
            assert finish_awaitable(frame.__anext__()) is None
        elif 'coroutine' == "coroutine":
            assert frame.send(None) == "paused"
        else:
            assert next(frame) is None

    def close(frame):
        if 'coroutine' == "async_generator":
            assert finish_awaitable(frame.aclose()) is None
        else:
            frame.close()

    def code_of(frame):
        if 'coroutine' == "async_generator":
            return frame.ag_code
        if 'coroutine' == "coroutine":
            return frame.cr_code
        return frame.gi_code

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    ordinary_target, ordinary_create = getattr(ordinary_private_capture, 'private_coroutine_family')()
    target, create = getattr(module, 'private_coroutine_family')()
    assert create.__code__.co_freevars == ordinary_create.__code__.co_freevars == ()
    assert create.__closure__ is ordinary_create.__closure__ is None
    assert create.__annotate__ is ordinary_create.__annotate__ is None
    assert _soac_ext.strict_function_entry_kind(create) == "generator_factory"
    original_code = create.__code__
    target_ref, create_ref = weakref.ref(target), weakref.ref(create)
    first, second = create(), create()
    del ordinary_target, ordinary_create, target, create
    start(first)
    start(second)
    close(first)
    assert code_of(first) is original_code
    gc.collect()
    assert target_ref() is not None and create_ref() is not None, "one close cleared another frame's cells"

    if 'coroutine' == "async_generator":
        holder = finish_awaitable(second.__anext__())
    elif 'coroutine' == "coroutine":
        try:
            second.send(None)
        except StopIteration as completed:
            holder = completed.value
        else:
            raise AssertionError("coroutine did not return its class")
    else:
        holder = next(second)
    assert owner(holder)
    instance = holder(target_ref()())
    assert type(instance.payload) is target_ref()
    try:
        holder(object())
    except TypeError:
        pass
    else:
        raise AssertionError("the second frame lost its required field predicate")
    close(second)
    assert code_of(first) is code_of(second) is original_code
    holder_ref = weakref.ref(holder)
    del holder, instance
    gc.collect()
    assert holder_ref() is None
    assert create_ref() is None, "closed wrappers retained their source function"
    assert target_ref() is None, "closed wrappers retained their private target"

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_closing_one_frame_keeps_another_frames_original_private_cells
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('private_async_generator_family',):
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
    import ordinary_private_capture
    from soac import _soac_ext

    def finish_awaitable(awaitable):
        iterator = awaitable.__await__()
        try:
            unexpected = next(iterator)
        except StopIteration as completed:
            return completed.value
        raise AssertionError(("unexpected async suspension", unexpected))

    def start(frame):
        if 'async_generator' == "async_generator":
            assert finish_awaitable(frame.__anext__()) is None
        elif 'async_generator' == "coroutine":
            assert frame.send(None) == "paused"
        else:
            assert next(frame) is None

    def close(frame):
        if 'async_generator' == "async_generator":
            assert finish_awaitable(frame.aclose()) is None
        else:
            frame.close()

    def code_of(frame):
        if 'async_generator' == "async_generator":
            return frame.ag_code
        if 'async_generator' == "coroutine":
            return frame.cr_code
        return frame.gi_code

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    ordinary_target, ordinary_create = getattr(ordinary_private_capture, 'private_async_generator_family')()
    target, create = getattr(module, 'private_async_generator_family')()
    assert create.__code__.co_freevars == ordinary_create.__code__.co_freevars == ()
    assert create.__closure__ is ordinary_create.__closure__ is None
    assert create.__annotate__ is ordinary_create.__annotate__ is None
    assert _soac_ext.strict_function_entry_kind(create) == "generator_factory"
    original_code = create.__code__
    target_ref, create_ref = weakref.ref(target), weakref.ref(create)
    first, second = create(), create()
    del ordinary_target, ordinary_create, target, create
    start(first)
    start(second)
    close(first)
    assert code_of(first) is original_code
    gc.collect()
    assert target_ref() is not None and create_ref() is not None, "one close cleared another frame's cells"

    if 'async_generator' == "async_generator":
        holder = finish_awaitable(second.__anext__())
    elif 'async_generator' == "coroutine":
        try:
            second.send(None)
        except StopIteration as completed:
            holder = completed.value
        else:
            raise AssertionError("coroutine did not return its class")
    else:
        holder = next(second)
    assert owner(holder)
    instance = holder(target_ref()())
    assert type(instance.payload) is target_ref()
    try:
        holder(object())
    except TypeError:
        pass
    else:
        raise AssertionError("the second frame lost its required field predicate")
    close(second)
    assert code_of(first) is code_of(second) is original_code
    holder_ref = weakref.ref(holder)
    del holder, instance
    gc.collect()
    assert holder_ref() is None
    assert create_ref() is None, "closed wrappers retained their source function"
    assert target_ref() is None, "closed wrappers retained their private target"

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_terminal_source_owners_preserve_outer_handler_and_release_payloads
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('terminal_generator_family',):
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
    import gc
    import sys
    import weakref
    import ordinary_private_capture
    from soac import _soac_ext

    def finish_awaitable(awaitable):
        iterator = awaitable.__await__()
        try:
            unexpected = next(iterator)
        except StopIteration as completed:
            return completed.value
        raise AssertionError(("unexpected async suspension", unexpected))

    def exercise(factory, termination):
        events = []
        class Payload:
            def __del__(self):
                current = sys.exception()
                events.append((type(current).__name__, str(current)))
        payload = Payload()
        create = factory(payload)
        assert create.__code__.co_freevars == ("payload",)
        payload_ref, create_ref = weakref.ref(payload), weakref.ref(create)
        original_code = create.__code__
        frame = create()
        del payload, create
        if 'generator' == "async_generator":
            assert finish_awaitable(frame.__anext__()) is None
        elif 'generator' == "coroutine":
            assert frame.send(None) == "paused"
        else:
            assert next(frame) is None
        assert payload_ref() is not None
        try:
            raise ValueError("surrounding caller")
        except ValueError as outer:
            if termination == "close":
                if 'generator' == "async_generator":
                    assert finish_awaitable(frame.aclose()) is None
                else:
                    frame.close()
            else:
                try:
                    if 'generator' == "async_generator":
                        finish_awaitable(frame.__anext__())
                    else:
                        frame.send(None)
                except (StopIteration, StopAsyncIteration):
                    pass
                else:
                    raise AssertionError("suspended function did not finish")
            assert sys.exception() is outer
            gc.collect()
            assert len(events) == 1, (termination, events)
            assert payload_ref() is None, "terminal frame retained its captured payload"
            assert create_ref() is None, "terminal wrapper retained its source function"
        if 'generator' == "async_generator":
            assert frame.ag_code is original_code
        elif 'generator' == "coroutine":
            assert frame.cr_code is original_code
        else:
            assert frame.gi_code is original_code
        # Finalization is required exactly once; its implicit-release
        # handler context is not compared between execution engines.
        return len(events)

    actual_factory = getattr(module, 'terminal_generator_family')
    ordinary_factory = getattr(ordinary_private_capture, 'terminal_generator_family')
    for termination in ("close", "complete"):
        expected = exercise(ordinary_factory, termination)
        actual = exercise(actual_factory, termination)
        assert actual == expected

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_terminal_source_owners_preserve_outer_handler_and_release_payloads
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('terminal_coroutine_family',):
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
    import gc
    import sys
    import weakref
    import ordinary_private_capture
    from soac import _soac_ext

    def finish_awaitable(awaitable):
        iterator = awaitable.__await__()
        try:
            unexpected = next(iterator)
        except StopIteration as completed:
            return completed.value
        raise AssertionError(("unexpected async suspension", unexpected))

    def exercise(factory, termination):
        events = []
        class Payload:
            def __del__(self):
                current = sys.exception()
                events.append((type(current).__name__, str(current)))
        payload = Payload()
        create = factory(payload)
        assert create.__code__.co_freevars == ("payload",)
        payload_ref, create_ref = weakref.ref(payload), weakref.ref(create)
        original_code = create.__code__
        frame = create()
        del payload, create
        if 'coroutine' == "async_generator":
            assert finish_awaitable(frame.__anext__()) is None
        elif 'coroutine' == "coroutine":
            assert frame.send(None) == "paused"
        else:
            assert next(frame) is None
        assert payload_ref() is not None
        try:
            raise ValueError("surrounding caller")
        except ValueError as outer:
            if termination == "close":
                if 'coroutine' == "async_generator":
                    assert finish_awaitable(frame.aclose()) is None
                else:
                    frame.close()
            else:
                try:
                    if 'coroutine' == "async_generator":
                        finish_awaitable(frame.__anext__())
                    else:
                        frame.send(None)
                except (StopIteration, StopAsyncIteration):
                    pass
                else:
                    raise AssertionError("suspended function did not finish")
            assert sys.exception() is outer
            gc.collect()
            assert len(events) == 1, (termination, events)
            assert payload_ref() is None, "terminal frame retained its captured payload"
            assert create_ref() is None, "terminal wrapper retained its source function"
        if 'coroutine' == "async_generator":
            assert frame.ag_code is original_code
        elif 'coroutine' == "coroutine":
            assert frame.cr_code is original_code
        else:
            assert frame.gi_code is original_code
        # Finalization is required exactly once; its implicit-release
        # handler context is not compared between execution engines.
        return len(events)

    actual_factory = getattr(module, 'terminal_coroutine_family')
    ordinary_factory = getattr(ordinary_private_capture, 'terminal_coroutine_family')
    for termination in ("close", "complete"):
        expected = exercise(ordinary_factory, termination)
        actual = exercise(actual_factory, termination)
        assert actual == expected

validate(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_terminal_source_owners_preserve_outer_handler_and_release_payloads
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('terminal_async_generator_family',):
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
    import gc
    import sys
    import weakref
    import ordinary_private_capture
    from soac import _soac_ext

    def finish_awaitable(awaitable):
        iterator = awaitable.__await__()
        try:
            unexpected = next(iterator)
        except StopIteration as completed:
            return completed.value
        raise AssertionError(("unexpected async suspension", unexpected))

    def exercise(factory, termination):
        events = []
        class Payload:
            def __del__(self):
                current = sys.exception()
                events.append((type(current).__name__, str(current)))
        payload = Payload()
        create = factory(payload)
        assert create.__code__.co_freevars == ("payload",)
        payload_ref, create_ref = weakref.ref(payload), weakref.ref(create)
        original_code = create.__code__
        frame = create()
        del payload, create
        if 'async_generator' == "async_generator":
            assert finish_awaitable(frame.__anext__()) is None
        elif 'async_generator' == "coroutine":
            assert frame.send(None) == "paused"
        else:
            assert next(frame) is None
        assert payload_ref() is not None
        try:
            raise ValueError("surrounding caller")
        except ValueError as outer:
            if termination == "close":
                if 'async_generator' == "async_generator":
                    assert finish_awaitable(frame.aclose()) is None
                else:
                    frame.close()
            else:
                try:
                    if 'async_generator' == "async_generator":
                        finish_awaitable(frame.__anext__())
                    else:
                        frame.send(None)
                except (StopIteration, StopAsyncIteration):
                    pass
                else:
                    raise AssertionError("suspended function did not finish")
            assert sys.exception() is outer
            gc.collect()
            assert len(events) == 1, (termination, events)
            assert payload_ref() is None, "terminal frame retained its captured payload"
            assert create_ref() is None, "terminal wrapper retained its source function"
        if 'async_generator' == "async_generator":
            assert frame.ag_code is original_code
        elif 'async_generator' == "coroutine":
            assert frame.cr_code is original_code
        else:
            assert frame.gi_code is original_code
        # Finalization is required exactly once; its implicit-release
        # handler context is not compared between execution engines.
        return len(events)

    actual_factory = getattr(module, 'terminal_async_generator_family')
    ordinary_factory = getattr(ordinary_private_capture, 'terminal_async_generator_family')
    for termination in ("close", "complete"):
        expected = exercise(ordinary_factory, termination)
        actual = exercise(actual_factory, termination)
        assert actual == expected

validate(module)

_assert_source_function_witnesses()
