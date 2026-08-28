# Authenticated source and independent ordinary validation blocks.
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
# ok
# tests/test_strict_function_boundaries.py::test_annotations_do_not_add_runtime_argument_or_return_checks
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
    checked_attr = True

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

    c_call = api('PyObject_Call', obj, obj, obj, obj)
    marker = object()

    def python_call(function, args, keywords):
        return function(*args, **keywords)

    def binding_error(invoke, function, args, keywords):
        support.events.clear()
        try:
            invoke(function, args, keywords)
        except TypeError as error:
            assert type(error) is TypeError
            assert support.events == [], support.events
            return error.args
        raise AssertionError('ordinary argument binding accepted an invalid call')

    for target in (ordinary, module):
        for invoke in (python_call, c_call):
            support.events.clear()
            assert invoke(target.echo, (marker,), {}) is marker
            assert invoke(target.returned, (marker,), {}) is marker
            assert invoke(target.defaulted, (), {}) is support.default_value
            assert invoke(target.Methods().accept, (marker,), {}) is marker
            actual = invoke(target.shape, (marker, marker, marker),
                            {'named': marker, 'extra': marker})
            assert actual[0] is actual[1] is actual[3] is marker
            assert actual[2] == (marker,) and actual[4] == {'extra': marker}
            assert support.events == [
                ('echo', marker), ('returned', marker), ('method', marker), ('shape',),
            ], support.events
            for name, args, keywords in (
                ('echo', (), {}),
                ('echo', (marker, marker), {}),
                ('echo', (marker,), {'unexpected': marker}),
                ('shape', (marker, marker), {'second': marker}),
            ):
                expected = binding_error(invoke, getattr(ordinary, name), args, keywords)
                assert binding_error(invoke, getattr(target, name), args, keywords) == expected
        support.events.clear()
        try:
            target.fail(marker)
        except LookupError as error:
            assert error is support.body_error
            error.__traceback__ = None
        else:
            raise AssertionError('the original body exception was lost')
        assert support.events == [('fail', marker), ('finally',)]

        released, references = [], []
        class Payload:
            def __del__(self):
                released.append('released')
        def create():
            payload = Payload()
            references.append(weakref.ref(payload))
            return payload
        support.events.clear()
        assert target.finish(marker, create) is marker
        assert support.events == [('finish', marker), ('finally',)]
        gc.collect()
        assert released == ['released'] and references[0]() is None

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_function_boundaries.py::test_annotated_calls_check_protected_fields_at_the_write
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
    checked_attr = True

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

    c_setattr = api('PyObject_SetAttr', ctypes.c_int, obj, obj, obj)
    c_generic_setattr = api('PyObject_GenericSetAttr', ctypes.c_int, obj, obj, obj)
    c_setitem = api('PyDict_SetItem', ctypes.c_int, obj, obj, obj)
    marker = object()
    receiver, control = module.Stored(), ordinary.Stored()
    escaped, ordinary_dictionary = vars(receiver), vars(control)

    def rejected(operation):
        try:
            operation()
        except TypeError as error:
            assert not isinstance(error, StrictMutationError), error
        else:
            raise AssertionError('a protected field accepted a wrong value')

    for setter in (setattr, object.__setattr__, c_setattr, c_generic_setattr):
        setter(control, 'value', marker)
        assert control.value is marker
        rejected(lambda: setter(receiver, 'value', marker))
        assert receiver.value == 1
    for setter in (dict.__setitem__, c_setitem):
        setter(ordinary_dictionary, 'value', marker)
        assert control.value is marker
        rejected(lambda: setter(escaped, 'value', marker))
        assert receiver.value == 1

    support.events.clear()
    control.store(marker)
    assert control.value is marker
    assert support.events == [('before-store', marker), ('after-store', marker)]
    support.events.clear()
    rejected(lambda: receiver.store(marker))
    assert support.events == [('before-store', 1)], support.events
    assert receiver.value == 1
    support.events.clear()
    receiver.store(7)
    assert support.events == [('before-store', 1), ('after-store', 7)]

    for name in ('Record', 'Slotted'):
        kind = getattr(module, name)
        support.produced = marker
        support.events.clear()
        rejected(kind)
        assert support.events == [('factory',)], support.events
        support.events.clear()
        result = kind(9, marker)
        assert result.value == 9
        assert support.events == [('post', marker)], support.events
        rejected(lambda: c_generic_setattr(result, 'value', marker))
        assert result.value == 9

validate_module(module)

_assert_source_function_witnesses()
