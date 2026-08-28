# Authenticated source and independent ordinary validation blocks.
# module:dynamic_context
# soac: module(strict_assign=true, checked_attr=true)
import builtins as _ordinary_builtins

LIMIT = 17
CAPTURED_BUILTINS = dict(vars(_ordinary_builtins))
CAPTURED_BUILTINS["DYNAMIC_SENTINEL"] = object()
__builtins__ = CAPTURED_BUILTINS

def invoke(target, args, kwargs):
    return target(*args, **kwargs)

def execute(target, code, global_namespace, local_namespace):
    return target(code, global_namespace, local_namespace)

def execute_without_locals(target, code, global_namespace):
    return target(code, global_namespace)

def compile_detached(target, source, flags, dont_inherit):
    return target(source, "<explicit-ordinary>", "exec",
                  flags=flags, dont_inherit=dont_inherit)

def compile_inherited(target, source):
    return target(source, "<inherited>", "exec")

def actual_globals():
    return globals()

def captured_values():
    x = 2
    def inner(y):
        z = x + y
        return x, y, z
    return inner(4)

def closure_values():
    x = 2
    def inner(y):
        def read(z):
            return y + z
        w = x + y
        y += 3
        return x, y, w, read(5)
    return inner(4)

def walrus_values(values, initial):
    last = initial
    def read():
        return last
    generator = (last := value + 1 for value in values)
    return generator, read

def caught_exception(raise_error, observe):
    try:
        raise_error()
    except Exception as error:
        observe("caught", error)
    observe("after", None)
    return "done"

# Function creation captures its own builtins mapping. A later module binding
# must not change that mapping or stand in for it at a native call boundary.
__builtins__ = vars(_ordinary_builtins)
# module:ordinary_dynamic_context
import builtins as _ordinary_builtins

LIMIT = 17
CAPTURED_BUILTINS = dict(vars(_ordinary_builtins))
CAPTURED_BUILTINS["DYNAMIC_SENTINEL"] = object()
__builtins__ = CAPTURED_BUILTINS

def invoke(target, args, kwargs):
    return target(*args, **kwargs)

def execute(target, code, global_namespace, local_namespace):
    return target(code, global_namespace, local_namespace)

def execute_without_locals(target, code, global_namespace):
    return target(code, global_namespace)

def compile_detached(target, source, flags, dont_inherit):
    return target(source, "<explicit-ordinary>", "exec",
                  flags=flags, dont_inherit=dont_inherit)

def compile_inherited(target, source):
    return target(source, "<inherited>", "exec")

def actual_globals():
    return globals()

def captured_values():
    x = 2
    def inner(y):
        z = x + y
        return x, y, z
    return inner(4)

def closure_values():
    x = 2
    def inner(y):
        def read(z):
            return y + z
        w = x + y
        y += 3
        return x, y, w, read(5)
    return inner(4)

def walrus_values(values, initial):
    last = initial
    def read():
        return last
    generator = (last := value + 1 for value in values)
    return generator, read

def caught_exception(raise_error, observe):
    try:
        raise_error()
    except Exception as error:
        observe("caught", error)
    observe("after", None)
    return "done"

# Function creation captures its own builtins mapping. A later module binding
# must not change that mapping or stand in for it at a native call boundary.
__builtins__ = vars(_ordinary_builtins)
# ok
# tests/test_strict_call_context.py::test_ordinary_code_uses_explicit_namespaces_and_captured_builtins
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
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

    import __future__
    import builtins
    import ctypes
    import gc
    import sys
    import types
    import weakref
    import pytest
    from pathlib import Path
    from soac import _soac_ext, StrictMutationError, StrictRuntimeUnavailableError

    import ordinary_dynamic_context as ordinary
    assert _soac_ext.strict_module_diagnostics(ordinary) is None
    source_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    source_owner.argtypes = [ctypes.py_object]
    source_owner.restype = ctypes.c_void_p
    for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
        function = vars(ordinary)[name]
        assert source_owner(function) is None, name
        assert _soac_ext.strict_function_entry_kind(function) is None, name
    for subject in (ordinary, module):
        assert vars(subject)["__builtins__"] is builtins.__dict__
        assert subject.CAPTURED_BUILTINS is not builtins.__dict__
        for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
            assert vars(subject)[name].__builtins__ is subject.CAPTURED_BUILTINS, name
        assert subject.actual_globals() is vars(subject)

    call_style = 'fixed'

    def call(subject, target, code, namespace, local_namespace, omitted=False):
        if call_style == "fixed":
            if omitted:
                return subject.execute_without_locals(target, code, namespace)
            return subject.execute(target, code, namespace, local_namespace)
        keywords = {'globals': namespace}
        if not omitted:
            keywords['locals'] = local_namespace
        return subject.invoke(target, (code,), keywords)

    statement = builtins.compile(
        "value = DYNAMIC_SENTINEL", "<ordinary-statement>", "exec",
        dont_inherit=True,
    )
    expression = builtins.compile(
        "DYNAMIC_SENTINEL", "<ordinary-expression>", "eval",
        dont_inherit=True,
    )
    for subject in (ordinary, module):
        marker = subject.CAPTURED_BUILTINS["DYNAMIC_SENTINEL"]
        for omitted in (False, True):
            namespace = {}
            assert call(subject, builtins.exec, statement, namespace, None, omitted) is None
            assert namespace["__builtins__"] is subject.CAPTURED_BUILTINS
            assert namespace["value"] is marker
            assert call(subject, builtins.eval, expression, namespace, None, omitted) is marker
        # Preserve an explicit pre-existing mapping, rather than installing
        # either the source capture or the unrelated driver mapping.
        explicit_marker = object()
        explicit_builtins = dict(builtins.__dict__, DYNAMIC_SENTINEL=explicit_marker)
        namespace = {"__builtins__": explicit_builtins}
        local_namespace = {}
        assert call(subject, builtins.exec, statement, namespace, local_namespace) is None
        assert namespace["__builtins__"] is explicit_builtins
        assert "value" not in namespace
        assert local_namespace["value"] is explicit_marker
        assert call(subject, builtins.eval, expression, namespace, local_namespace) is explicit_marker

        events = []
        class LocalMapping(dict):
            def __getitem__(self, key):
                events.append(("get", key))
                return super().__getitem__(key)
            def __setitem__(self, key, value):
                events.append(("set", key, value))
                return super().__setitem__(key, value)
        mapping = LocalMapping()
        namespace = {"__builtins__": explicit_builtins}
        assert call(subject, builtins.exec, statement, namespace, mapping) is None
        assert events == [
            ("get", "DYNAMIC_SENTINEL"),
            ("set", "value", explicit_marker),
        ], events

        primary = ValueError("ordinary code failure")
        caller = LookupError("caller handler")
        error_code = builtins.compile(
            "raise failure", "<ordinary-error>", "exec", dont_inherit=True
        )
        try:
            raise caller
        except LookupError:
            try:
                call(subject, builtins.exec, error_code,
                     {"failure": primary, "__builtins__": builtins.__dict__}, None)
            except ValueError as actual:
                assert actual is primary
                assert actual.__context__ is caller
                assert sys.exception() is primary
            else:
                raise AssertionError("ordinary code did not raise its original exception")
            assert sys.exception() is caller
        primary.__traceback__ = None
        caller.__traceback__ = None

    # Ordinary code still reaches protected storage at the actual write.
    write = builtins.compile(
        "events.append('before')\nglobal LIMIT\nLIMIT = replacement\nevents.append('after')",
        "<ordinary-write>", "exec", dont_inherit=True,
    )
    replacement = object()
    ordinary_events = []
    assert call(ordinary, builtins.exec, write, vars(ordinary),
                {"events": ordinary_events, "replacement": replacement}) is None
    assert ordinary.LIMIT is replacement
    assert ordinary_events == ["before", "after"]
    ordinary.LIMIT = 17
    selected_events = []
    with pytest.raises(StrictMutationError):
        call(module, builtins.exec, write, vars(module),
             {"events": selected_events, "replacement": replacement})
    assert selected_events == ["before"]
    assert module.LIMIT == 17

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_call_context.py::test_ordinary_code_uses_explicit_namespaces_and_captured_builtins
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
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

    import __future__
    import builtins
    import ctypes
    import gc
    import sys
    import types
    import weakref
    import pytest
    from pathlib import Path
    from soac import _soac_ext, StrictMutationError, StrictRuntimeUnavailableError

    import ordinary_dynamic_context as ordinary
    assert _soac_ext.strict_module_diagnostics(ordinary) is None
    source_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    source_owner.argtypes = [ctypes.py_object]
    source_owner.restype = ctypes.c_void_p
    for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
        function = vars(ordinary)[name]
        assert source_owner(function) is None, name
        assert _soac_ext.strict_function_entry_kind(function) is None, name
    for subject in (ordinary, module):
        assert vars(subject)["__builtins__"] is builtins.__dict__
        assert subject.CAPTURED_BUILTINS is not builtins.__dict__
        for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
            assert vars(subject)[name].__builtins__ is subject.CAPTURED_BUILTINS, name
        assert subject.actual_globals() is vars(subject)

    call_style = 'expanded'

    def call(subject, target, code, namespace, local_namespace, omitted=False):
        if call_style == "fixed":
            if omitted:
                return subject.execute_without_locals(target, code, namespace)
            return subject.execute(target, code, namespace, local_namespace)
        keywords = {'globals': namespace}
        if not omitted:
            keywords['locals'] = local_namespace
        return subject.invoke(target, (code,), keywords)

    statement = builtins.compile(
        "value = DYNAMIC_SENTINEL", "<ordinary-statement>", "exec",
        dont_inherit=True,
    )
    expression = builtins.compile(
        "DYNAMIC_SENTINEL", "<ordinary-expression>", "eval",
        dont_inherit=True,
    )
    for subject in (ordinary, module):
        marker = subject.CAPTURED_BUILTINS["DYNAMIC_SENTINEL"]
        for omitted in (False, True):
            namespace = {}
            assert call(subject, builtins.exec, statement, namespace, None, omitted) is None
            assert namespace["__builtins__"] is subject.CAPTURED_BUILTINS
            assert namespace["value"] is marker
            assert call(subject, builtins.eval, expression, namespace, None, omitted) is marker
        # Preserve an explicit pre-existing mapping, rather than installing
        # either the source capture or the unrelated driver mapping.
        explicit_marker = object()
        explicit_builtins = dict(builtins.__dict__, DYNAMIC_SENTINEL=explicit_marker)
        namespace = {"__builtins__": explicit_builtins}
        local_namespace = {}
        assert call(subject, builtins.exec, statement, namespace, local_namespace) is None
        assert namespace["__builtins__"] is explicit_builtins
        assert "value" not in namespace
        assert local_namespace["value"] is explicit_marker
        assert call(subject, builtins.eval, expression, namespace, local_namespace) is explicit_marker

        events = []
        class LocalMapping(dict):
            def __getitem__(self, key):
                events.append(("get", key))
                return super().__getitem__(key)
            def __setitem__(self, key, value):
                events.append(("set", key, value))
                return super().__setitem__(key, value)
        mapping = LocalMapping()
        namespace = {"__builtins__": explicit_builtins}
        assert call(subject, builtins.exec, statement, namespace, mapping) is None
        assert events == [
            ("get", "DYNAMIC_SENTINEL"),
            ("set", "value", explicit_marker),
        ], events

        primary = ValueError("ordinary code failure")
        caller = LookupError("caller handler")
        error_code = builtins.compile(
            "raise failure", "<ordinary-error>", "exec", dont_inherit=True
        )
        try:
            raise caller
        except LookupError:
            try:
                call(subject, builtins.exec, error_code,
                     {"failure": primary, "__builtins__": builtins.__dict__}, None)
            except ValueError as actual:
                assert actual is primary
                assert actual.__context__ is caller
                assert sys.exception() is primary
            else:
                raise AssertionError("ordinary code did not raise its original exception")
            assert sys.exception() is caller
        primary.__traceback__ = None
        caller.__traceback__ = None

    # Ordinary code still reaches protected storage at the actual write.
    write = builtins.compile(
        "events.append('before')\nglobal LIMIT\nLIMIT = replacement\nevents.append('after')",
        "<ordinary-write>", "exec", dont_inherit=True,
    )
    replacement = object()
    ordinary_events = []
    assert call(ordinary, builtins.exec, write, vars(ordinary),
                {"events": ordinary_events, "replacement": replacement}) is None
    assert ordinary.LIMIT is replacement
    assert ordinary_events == ["before", "after"]
    ordinary.LIMIT = 17
    selected_events = []
    with pytest.raises(StrictMutationError):
        call(module, builtins.exec, write, vars(module),
             {"events": selected_events, "replacement": replacement})
    assert selected_events == ["before"]
    assert module.LIMIT == 17

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_call_context.py::test_noninheriting_compile_preserves_native_argument_conversion
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
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

    import __future__
    import builtins
    import ctypes
    import gc
    import sys
    import types
    import weakref
    import pytest
    from pathlib import Path
    from soac import _soac_ext, StrictMutationError, StrictRuntimeUnavailableError

    import ordinary_dynamic_context as ordinary
    assert _soac_ext.strict_module_diagnostics(ordinary) is None
    source_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    source_owner.argtypes = [ctypes.py_object]
    source_owner.restype = ctypes.c_void_p
    for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
        function = vars(ordinary)[name]
        assert source_owner(function) is None, name
        assert _soac_ext.strict_function_entry_kind(function) is None, name
    for subject in (ordinary, module):
        assert vars(subject)["__builtins__"] is builtins.__dict__
        assert subject.CAPTURED_BUILTINS is not builtins.__dict__
        for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
            assert vars(subject)[name].__builtins__ is subject.CAPTURED_BUILTINS, name
        assert subject.actual_globals() is vars(subject)

    import ast

    for subject in (ordinary, module):
        for expanded in (False, True):
            events = []
            class Flags:
                def __index__(self):
                    events.append("flags")
                    return 0
            class DontInherit:
                def __bool__(self):
                    events.append("dont_inherit")
                    return True
            if expanded:
                code = subject.invoke(
                    builtins.compile,
                    ("answer = 41", "<explicit-ordinary>", "exec", Flags(), DontInherit()),
                    {},
                )
            else:
                code = subject.compile_detached(
                    builtins.compile, "answer = 41", Flags(), DontInherit()
                )
            assert type(code) is types.CodeType
            assert not (code.co_flags & __future__.strict.compiler_flag)
            assert events == ["flags", "dont_inherit"], events
            namespace = {}
            builtins.exec(code, namespace)
            assert namespace["answer"] == 41

        # Every converter matters: flags select an AST, optimize folds
        # __debug__, and the feature version admits/rejects match syntax.
        source = "debug_value = __debug__\nmatch 1:\n    case 1:\n        answer = 41\n"
        filename = "<converted-ordinary>"
        for all_keywords in (False, True):
            for feature_version in (10, 9):
                events = []
                class Filename:
                    def __fspath__(self):
                        events.append("filename")
                        return filename
                class Index:
                    def __init__(self, name, value):
                        self.name, self.value = name, value
                    def __index__(self):
                        events.append(self.name)
                        return self.value
                class Truth:
                    def __bool__(self):
                        events.append("dont_inherit")
                        return True
                flags = Index("flags", ast.PyCF_ONLY_AST | ast.PyCF_OPTIMIZED_AST)
                optimize = Index("optimize", 1)
                feature = Index("feature_version", feature_version)
                arguments = (source, Filename(), "exec", flags, Truth(), optimize)
                keywords = {"_feature_version": feature}
                if all_keywords:
                    keywords.update(zip(
                        ("source", "filename", "mode", "flags", "dont_inherit", "optimize"),
                        arguments,
                    ))
                    arguments = ()
                if feature_version == 9:
                    with pytest.raises(SyntaxError) as syntax:
                        subject.invoke(builtins.compile, arguments, keywords)
                    assert syntax.value.filename == filename
                    assert "Pattern matching" in syntax.value.msg
                else:
                    tree = subject.invoke(builtins.compile, arguments, keywords)
                    assert type(tree) is ast.Module
                    assert isinstance(tree.body[0], ast.Assign)
                    assert isinstance(tree.body[0].value, ast.Constant)
                    assert tree.body[0].value.value is False
                    assert isinstance(tree.body[1], ast.Match)
                    code = builtins.compile(tree, filename, "exec", dont_inherit=True)
                    assert not (code.co_flags & __future__.strict.compiler_flag)
                    namespace = {}
                    builtins.exec(code, namespace)
                    assert namespace["debug_value"] is False
                    assert namespace["answer"] == 41
                assert events == [
                    "filename", "flags", "dont_inherit", "optimize", "feature_version",
                ], events

        marker = ValueError("dont_inherit conversion")
        events = []
        class FailedDontInherit:
            def __bool__(self):
                events.append("dont_inherit")
                raise marker
        with pytest.raises(ValueError) as raised:
            subject.compile_detached(
                builtins.compile, "pass", 0, FailedDontInherit()
            )
        assert raised.value is marker
        assert events == ["dont_inherit"]
        marker.__traceback__ = None
        with pytest.raises(SyntaxError) as invalid:
            subject.compile_detached(
                builtins.compile, "class Bad:\n    nonlocal x\n", 0, True
            )
        assert "nonlocal" in invalid.value.msg

        for target in (builtins.compile, builtins.eval, builtins.exec):
            with pytest.raises(TypeError) as expected:
                target()
            with pytest.raises(TypeError) as actual:
                subject.invoke(target, (), {})
            assert str(actual.value) == str(expected.value)
        with pytest.raises(TypeError) as expected:
            builtins.compile("pass", "<invalid>", "exec", dont_inherit=True, unknown=1)
        with pytest.raises(TypeError) as actual:
            subject.invoke(
                builtins.compile, ("pass", "<invalid>", "exec"),
                {"dont_inherit": True, "unknown": 1},
            )
        assert str(actual.value) == str(expected.value)

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_call_context.py::test_dynamic_code_does_not_inherit_execution_authority
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
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

    import __future__
    import builtins
    import ctypes
    import gc
    import sys
    import types
    import weakref
    import pytest
    from pathlib import Path
    from soac import _soac_ext, StrictMutationError, StrictRuntimeUnavailableError

    import ordinary_dynamic_context as ordinary
    assert _soac_ext.strict_module_diagnostics(ordinary) is None
    source_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    source_owner.argtypes = [ctypes.py_object]
    source_owner.restype = ctypes.c_void_p
    for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
        function = vars(ordinary)[name]
        assert source_owner(function) is None, name
        assert _soac_ext.strict_function_entry_kind(function) is None, name
    for subject in (ordinary, module):
        assert vars(subject)["__builtins__"] is builtins.__dict__
        assert subject.CAPTURED_BUILTINS is not builtins.__dict__
        for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
            assert vars(subject)[name].__builtins__ is subject.CAPTURED_BUILTINS, name
        assert subject.actual_globals() is vars(subject)

    source = "events.append('executed')"
    ordinary_code = ordinary.compile_inherited(builtins.compile, source)
    assert not (ordinary_code.co_flags & __future__.strict.compiler_flag)
    ordinary_events = []
    builtins.exec(ordinary_code, {"events": ordinary_events})
    assert ordinary_events == ["executed"]

    if __dp_integration_soac__:
        with pytest.raises(NotImplementedError, match="authenticated dynamic-code protocol"):
            module.compile_inherited(builtins.compile, source)
    else:
        inherited = module.compile_inherited(builtins.compile, source)
        assert inherited.co_flags & __future__.strict.compiler_flag
        with pytest.raises(StrictRuntimeUnavailableError):
            builtins.exec(inherited, {"events": []})

    # A real compiler flag, even with matching strict globals, is not an
    # authenticated runtime entry for this separately compiled code tree.
    unowned = builtins.compile(
        source, "<unowned-strict>", "exec",
        flags=__future__.strict.compiler_flag, dont_inherit=True,
    )
    for subject in (ordinary, module):
        for namespace in ({}, vars(module)):
            events = []
            before = {name: id(value) for name, value in namespace.items()}
            # Provide builtins explicitly so refusal is about code authority,
            # not missing context. Existing strict globals are unchanged.
            if namespace is not vars(module):
                namespace["__builtins__"] = builtins.__dict__
                before = {name: id(value) for name, value in namespace.items()}
            with pytest.raises(StrictRuntimeUnavailableError):
                subject.execute(builtins.exec, unowned, namespace, {"events": events})
            assert events == []
            assert {name: id(value) for name, value in namespace.items()} == before

    # A source-created function/code/closure likewise cannot be replayed by
    # discarding its actual function owner and invoking the code object.
    with pytest.raises(StrictRuntimeUnavailableError):
        module.execute(
            builtins.eval, module.actual_globals.__code__, vars(module), vars(module)
        )
    assert module.LIMIT == 17

validate_module(module)

_assert_source_function_witnesses()
# ok
# tests/test_strict_call_context.py::test_frame_free_capture_walrus_and_exception_cleanup
import sys
from soac import _soac_ext, import_hook
def _assert_source_function_witnesses():
    from tests._strict_integration import (
        _plain_function_witness, _assert_cpython_function_witness,
    )
    for _scenario_name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
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

    import __future__
    import builtins
    import ctypes
    import gc
    import sys
    import types
    import weakref
    import pytest
    from pathlib import Path
    from soac import _soac_ext, StrictMutationError, StrictRuntimeUnavailableError

    import ordinary_dynamic_context as ordinary
    assert _soac_ext.strict_module_diagnostics(ordinary) is None
    source_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    source_owner.argtypes = [ctypes.py_object]
    source_owner.restype = ctypes.c_void_p
    for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
        function = vars(ordinary)[name]
        assert source_owner(function) is None, name
        assert _soac_ext.strict_function_entry_kind(function) is None, name
    for subject in (ordinary, module):
        assert vars(subject)["__builtins__"] is builtins.__dict__
        assert subject.CAPTURED_BUILTINS is not builtins.__dict__
        for name in ('invoke', 'execute', 'execute_without_locals', 'compile_detached', 'compile_inherited', 'actual_globals', 'captured_values', 'closure_values', 'walrus_values', 'caught_exception'):
            assert vars(subject)[name].__builtins__ is subject.CAPTURED_BUILTINS, name
        assert subject.actual_globals() is vars(subject)

    for subject in (ordinary, module):
        assert subject.captured_values() == (2, 4, 6)
        assert subject.closure_values() == (2, 7, 6, 12)
        events = []
        class Values:
            def __iter__(self):
                events.append("iter")
                return iter((1, 2, 3, 4))
        initial = object()
        generator, read = subject.walrus_values(Values(), initial)
        assert events == ["iter"]
        assert read() is initial
        assert next(generator) == 2 and read() == 2
        assert next(generator) == 3 and read() == 3
        assert list(generator) == [4, 5]
        assert read() == 5
        assert events == ["iter"]
        generator.close()
        assert read() == 5

        handler_events = []
        references = []
        drops = []
        class Failure(Exception):
            def __del__(self):
                drops.append("failure")
        def raise_error():
            error = Failure("owned exception")
            references.append(weakref.ref(error))
            raise error
        def observe(phase, value):
            if phase == "caught":
                assert value is references[0]()
                assert sys.exception() is value
            else:
                assert value is None and sys.exception() is None
            handler_events.append(phase)
        assert subject.caught_exception(raise_error, observe) == "done"
        assert handler_events == ["caught", "after"]
        gc.collect()
        assert references[0]() is None
        assert drops == ["failure"]

validate_module(module)

_assert_source_function_witnesses()
