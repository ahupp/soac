# Authenticated source and independent ordinary validation blocks.
# module:fields_disabled
# soac: module(strict_assign=true, checked_attr=false)
from dataclasses import InitVar, dataclass, field
from typing import Any, cast
from annotation_runtime_support import body_error, default_value, events, factory

def echo(value: int) -> int:
    events.append(('echo', value))
    return value

def returned(value: Any) -> int:
    events.append(('returned', value))
    return cast(int, value)

def shape(first: int, /, second: int = 2, *items: int,
          named: int = 3, **extras: int):
    events.append(('shape',))
    return first, second, items, named, extras

def defaulted(value: int = default_value) -> int:
    return value

def finish(value: Any, create) -> int:
    temporary = create()
    try:
        events.append(('finish', value))
        return cast(int, value)
    finally:
        events.append(('finally',))

def fail(value: int) -> int:
    events.append(('fail', value))
    try:
        raise body_error
    finally:
        events.append(('finally',))

class Token:
    pass

class Methods:
    def accept(self, value: Token) -> Token:
        events.append(('method', value))
        return value

class Stored:
    def __init__(self):
        self.value: int = 1

    def store(self, value: int) -> None:
        events.append(('before-store', self.value))
        self.value = value
        events.append(('after-store', self.value))

@dataclass
class Record:
    value: int = field(default_factory=factory)
    seed: InitVar[int] = 0

    def __post_init__(self, seed: int) -> None:
        events.append(('post', seed))

@dataclass(slots=True)
class Slotted:
    value: int = field(default_factory=factory)
    seed: InitVar[int] = 0

    def __post_init__(self, seed: int) -> None:
        events.append(('post', seed))
# module:annotation_runtime_support
from typing import Any

events = []
default_value: Any = 'ordinary default'
produced: Any = 11
body_error = LookupError('original body error')
factory_error = RuntimeError('original factory error')
fail_factory = False

def factory() -> Any:
    events.append(('factory',))
    if fail_factory:
        raise factory_error
    return produced
# module:fields_enabled
# soac: module(strict_assign=true, checked_attr=true)
from dataclasses import InitVar, dataclass, field
from typing import Any, cast
from annotation_runtime_support import body_error, default_value, events, factory

def echo(value: int) -> int:
    events.append(('echo', value))
    return value

def returned(value: Any) -> int:
    events.append(('returned', value))
    return cast(int, value)

def shape(first: int, /, second: int = 2, *items: int,
          named: int = 3, **extras: int):
    events.append(('shape',))
    return first, second, items, named, extras

def defaulted(value: int = default_value) -> int:
    return value

def finish(value: Any, create) -> int:
    temporary = create()
    try:
        events.append(('finish', value))
        return cast(int, value)
    finally:
        events.append(('finally',))

def fail(value: int) -> int:
    events.append(('fail', value))
    try:
        raise body_error
    finally:
        events.append(('finally',))

class Token:
    pass

class Methods:
    def accept(self, value: Token) -> Token:
        events.append(('method', value))
        return value

class Stored:
    def __init__(self):
        self.value: int = 1

    def store(self, value: int) -> None:
        events.append(('before-store', self.value))
        self.value = value
        events.append(('after-store', self.value))

@dataclass
class Record:
    value: int = field(default_factory=factory)
    seed: InitVar[int] = 0

    def __post_init__(self, seed: int) -> None:
        events.append(('post', seed))

@dataclass(slots=True)
class Slotted:
    value: int = field(default_factory=factory)
    seed: InitVar[int] = 0

    def __post_init__(self, seed: int) -> None:
        events.append(('post', seed))
# ok
# tests/test_strict_function_boundaries.py::test_dataclass_annotations_do_not_check_initvars_or_factory_results
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('echo', 'returned', 'shape', 'defaulted', 'finish', 'fail', 'Methods.accept', 'Stored.store', 'Record.__post_init__', 'Slotted.__post_init__'):
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

def validate_module(module):
    ordinary_source = "\nfrom dataclasses import InitVar, dataclass, field\nfrom typing import Any, cast\nfrom annotation_runtime_support import body_error, default_value, events, factory\n\ndef echo(value: int) -> int:\n    events.append(('echo', value))\n    return value\n\ndef returned(value: Any) -> int:\n    events.append(('returned', value))\n    return cast(int, value)\n\ndef shape(first: int, /, second: int = 2, *items: int,\n          named: int = 3, **extras: int):\n    events.append(('shape',))\n    return first, second, items, named, extras\n\ndef defaulted(value: int = default_value) -> int:\n    return value\n\ndef finish(value: Any, create) -> int:\n    temporary = create()\n    try:\n        events.append(('finish', value))\n        return cast(int, value)\n    finally:\n        events.append(('finally',))\n\ndef fail(value: int) -> int:\n    events.append(('fail', value))\n    try:\n        raise body_error\n    finally:\n        events.append(('finally',))\n\nclass Token:\n    pass\n\nclass Methods:\n    def accept(self, value: Token) -> Token:\n        events.append(('method', value))\n        return value\n\nclass Stored:\n    def __init__(self):\n        self.value: int = 1\n\n    def store(self, value: int) -> None:\n        events.append(('before-store', self.value))\n        self.value = value\n        events.append(('after-store', self.value))\n\n@dataclass\nclass Record:\n    value: int = field(default_factory=factory)\n    seed: InitVar[int] = 0\n\n    def __post_init__(self, seed: int) -> None:\n        events.append(('post', seed))\n\n@dataclass(slots=True)\nclass Slotted:\n    value: int = field(default_factory=factory)\n    seed: InitVar[int] = 0\n\n    def __post_init__(self, seed: int) -> None:\n        events.append(('post', seed))\n"
    checked_attr = False

    import ctypes
    import gc
    import sys
    import types
    import weakref
    import annotation_runtime_support as support
    from soac import _soac_ext
    from soac.strict import StrictMutationError

    ordinary = types.ModuleType('ordinary_annotation_runtime')
    sys.modules[ordinary.__name__] = ordinary
    exec(compile(ordinary_source, '<ordinary-annotation-runtime>', 'exec'), vars(ordinary))

    def api(name, result, *arguments):
        function = getattr(ctypes.pythonapi, name)
        function.restype = result
        function.argtypes = arguments
        return function

    obj = ctypes.py_object
    function_owner = api('PyFunction_GetSoacStrictOwner', ctypes.c_void_p, obj)
    class_owner = api('PyType_GetSoacContractOwner', ctypes.c_void_p, obj)
    sealed = api('PyType_IsSoacSealed', ctypes.c_int, obj)
    assert _soac_ext.strict_module_diagnostics(module)['sealed']
    assert _soac_ext.strict_module_diagnostics(ordinary) is None
    for name in ('Token', 'Methods', 'Stored', 'Record', 'Slotted'):
        selected, control = getattr(module, name), getattr(ordinary, name)
        assert bool(class_owner(selected)) is checked_attr, name
        assert sealed(selected) == int(checked_attr), name
        assert not class_owner(control) and sealed(control) == 0, name
    for name in ('Record', 'Slotted'):
        assert bool(function_owner(getattr(module, name).__init__)) is checked_attr
        assert not function_owner(getattr(ordinary, name).__init__)

    import dataclasses
    import fields_enabled

    marker = object()
    support.produced = marker
    for target in (ordinary, module):
        for name in ('Record', 'Slotted'):
            kind = getattr(target, name)
            support.events.clear()
            result = kind(seed=marker)
            assert result.value is marker
            assert support.events == [('factory',), ('post', marker)]
            support.events.clear()
            result = kind(marker, marker)
            assert result.value is marker
            assert support.events == [('post', marker)]
            assert [item.name for item in dataclasses.fields(kind)] == ['value']
            assert 'seed' not in (vars(result) if hasattr(result, '__dict__') else kind.__slots__)
            # Passing CPython's factory sentinel explicitly follows the original
            # generated expression; it is not a separate runtime type obligation.
            support.events.clear()
            result = kind(dataclasses._HAS_DEFAULT_FACTORY, marker)
            assert result.value is marker
            assert support.events == [('factory',), ('post', marker)]

    assignments = []
    class Foreign:
        def __setattr__(self, name, value):
            assignments.append((name, value))
            object.__setattr__(self, name, value)
        def __post_init__(self, seed):
            support.events.append(('foreign-post', seed))

    assert _soac_ext.strict_module_diagnostics(fields_enabled)['sealed']
    for target in (ordinary, module, fields_enabled):
        for name in ('Record', 'Slotted'):
            kind = getattr(target, name)
            if target is fields_enabled:
                assert class_owner(kind) and sealed(kind) == 1
                assert function_owner(kind.__init__)
            foreign = Foreign()
            assignments.clear()
            support.events.clear()
            assert kind.__init__(foreign, seed=marker) is None
            assert assignments == [('value', marker)] and foreign.value is marker
            assert support.events == [('factory',), ('foreign-post', marker)]
            assignments.clear()
            support.events.clear()
            del foreign.value
            support.fail_factory = True
            try:
                kind.__init__(foreign, seed=marker)
            except RuntimeError as error:
                assert error is support.factory_error
                error.__traceback__ = None
            else:
                raise AssertionError('the original factory exception was lost')
            finally:
                support.fail_factory = False
            assert assignments == [] and not hasattr(foreign, 'value')
            assert support.events == [('factory',)]

validate_module(module)

_assert_source_function_witnesses()
