"""Late-resolved builtins use the actual strict caller, never a native frame."""

from __future__ import annotations

from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_late_method_callable_keeps_actual_context(
    tmp_path: Path, entry_interpreter: bool
):
    project = create_strict_project(
        tmp_path,
        {
            "context_target.py": """
                # soac: module(strict_assign=true, checked_attr=true)

                def read(receiver):
                    return receiver.method()

                def discard(receiver):
                    receiver.method()

                def with_argument(receiver, argument):
                    return receiver.method(argument())

                def expanded(receiver, args, kwargs):
                    return receiver.method(*args, **kwargs)
            """,
        },
        modules={"context_target": "context_target.py"},
    )
    program = """
        import builtins
        import context_target

        class Receiver:
            pass

        receiver = Receiver()
        receiver.method = builtins.globals
        for _ in range(32):
            assert context_target.read(receiver) is context_target.__dict__
            assert context_target.discard(receiver) is None

        events = []
        def argument():
            events.append('argument')
            return 1

        try:
            context_target.with_argument(receiver, argument)
        except TypeError:
            pass
        else:
            raise AssertionError('globals argument error was replaced by a context result')
        assert events == ['argument']

        # A missing eval expression is an ordinary argument error, not a
        # request for a function-local namespace.
        receiver.method = builtins.eval
        for call in (
            context_target.read,
            context_target.discard,
            lambda receiver: context_target.expanded(receiver, (), {}),
        ):
            try:
                builtins.eval()
            except TypeError as expected:
                expected_message = str(expected)
            else:
                raise AssertionError('ordinary eval unexpectedly accepted no expression')
            try:
                call(receiver)
            except TypeError as actual:
                assert str(actual) == expected_message
            else:
                raise AssertionError('eval argument error was replaced by a context result')

        # One-argument dir and its native keyword errors are not contextual
        # zero-argument operations. Preserve their ordinary argument effects.
        receiver.method = builtins.dir
        assert context_target.with_argument(receiver, argument) == dir(1)
        assert events == ['argument', 'argument']
        assert context_target.expanded(receiver, (1,), {}) == dir(1)
        try:
            context_target.expanded(receiver, (), {'object': 1})
        except TypeError as actual:
            try:
                dir(object=1)
            except TypeError as expected:
                assert str(actual) == str(expected)
            else:
                raise AssertionError('ordinary dir unexpectedly accepted a keyword')
        else:
            raise AssertionError('dir keyword error was replaced by a context result')

        def dir():
            ordinary_marker = 42
            return builtins.dir()

        receiver.method = dir
        assert context_target.read(receiver) == ['ordinary_marker']

        def globals():
            events.append('ordinary')
            return 'not a builtin'

        receiver.method = globals
        assert context_target.read(receiver) == 'not a builtin'
        context_target.discard(receiver)
        assert events == ['argument', 'argument', 'ordinary', 'ordinary']
    """
    profile = project.run(
        program, entry_interpreter=entry_interpreter, opt_mode="profile"
    )
    work = Path(profile.args[-1]).parent / "soac-work"
    project.run(
        program,
        entry_interpreter=entry_interpreter,
        opt_mode="apply",
        extra_env={"SOAC_WORK_DIR": str(work)},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_dir_alias_uses_actual_module_and_class_namespaces(
    tmp_path: Path, entry_interpreter: bool
):
    project = create_strict_project(
        tmp_path,
        {
            "dir_context.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                from builtins import dir as aliased_dir

                MODULE_MARKER = object()
                module_names = aliased_dir()

                class Namespace:
                    CLASS_MARKER = object()
                    names = aliased_dir()

                def function_names():
                    return aliased_dir()
            """,
        },
        modules={"dir_context": "dir_context.py"},
    )
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    project.run(
        f"""
        import dir_context as module

        assert _soac_ext.strict_module_diagnostics(module)["sealed"] is True
        assert _soac_ext.strict_function_entry_kind(module.function_names) == {expected_entry!r}
        assert "MODULE_MARKER" in module.module_names
        assert "CLASS_MARKER" not in module.module_names
        assert module.module_names == sorted(module.module_names)
        assert "CLASS_MARKER" in module.Namespace.names
        assert "MODULE_MARKER" not in module.Namespace.names
        assert module.Namespace.names == sorted(module.Namespace.names)
        # Function-local inspection is excluded. Its definition still must not
        # block admission of the actual module/class namespace operations above.
        """,
        entry_interpreter=entry_interpreter,
    )


_EXPANDED_ARGUMENT_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def prefix(callee, source, predicate, value, first):
    return callee()(*source(), value() if predicate() else None)

def singleton(callee, source, predicate, value, first):
    return callee()(*source(), tail=value() if predicate() else None)

def mapping(callee, source, predicate, value, first):
    return callee()(**source(), tail=value() if predicate() else None)

def grouped_duplicate(callee, source, predicate, value, first):
    return callee()(**source(), duplicate=first(), tail=value() if predicate() else None)

def suspended_prefix(callee, source, predicate, value, first):
    return callee()(*source(), (yield 'ready'))

def suspended_singleton(callee, source, predicate, value, first):
    return callee()(*source(), tail=(yield 'ready'))
"""


_EXPANDED_ARGUMENT_OBSERVER = """
def observe_expanded_argument_case(namespace, case):
    import gc
    import sys
    import types
    import weakref

    events = []
    references = []
    raw_reference = None
    caller = RuntimeError('caller context')
    failure = ValueError('star conversion failed')

    def context():
        active = sys.exception()
        if active is caller:
            return 'caller'
        if active is failure:
            return 'failure'
        return None if active is None else type(active).__name__

    def raw_alive():
        return raw_reference is not None and raw_reference() is not None

    class Payload:
        def __init__(self, label):
            self.label = label
            references.append(weakref.ref(self))
        def __del__(self):
            events.append(('drop', self.label, context(), raw_alive()))

    class Items:
        def __iter__(self):
            events.append(('iter', context()))
            yield Payload('item')
        def __del__(self):
            events.append(('drop-source', context()))

    class BrokenItems:
        def __iter__(self):
            events.append(('iter', context()))
            raise failure
        def __del__(self):
            events.append(('drop-source', context()))

    class Mapping:
        def keys(self):
            events.append(('keys', context()))
            return ['duplicate' if case == 'grouped_duplicate' else 'mapped']
        def __getitem__(self, key):
            events.append(('getitem', key, context()))
            return Payload('mapping.' + key)
        def __del__(self):
            events.append(('drop-source', context()))

    def target(*args, **kwargs):
        events.append(('call', tuple(value.label for value in args),
                       tuple((key, value.label) for key, value in kwargs.items()), context()))
        return 'returned'

    def callee():
        events.append(('callee', context()))
        return target

    def source():
        nonlocal raw_reference
        events.append(('source', context()))
        if case in ('singleton_failure', 'suspended_singleton_failure'):
            result = BrokenItems()
        elif case in ('mapping', 'grouped_duplicate'):
            result = Mapping()
        else:
            result = Items()
        raw_reference = weakref.ref(result)
        return result

    def predicate():
        events.append(('predicate', context()))
        return True

    def value():
        events.append(('value', context()))
        return Payload('tail')

    def first():
        events.append(('first', context()))
        return Payload('duplicate')

    function_name = (
        'suspended_singleton' if case == 'suspended_singleton_failure'
        else 'singleton' if case == 'singleton_failure' else case
    )
    function = namespace[function_name]
    try:
        raise caller
    except RuntimeError:
        try:
            result = function(callee, source, predicate, value, first)
            if case.startswith('suspended_'):
                assert type(result) is types.GeneratorType
                assert next(result) == 'ready'
                events.append(('suspended', context()))
                try:
                    result.send(value())
                except StopIteration as finished:
                    result = finished.value
                else:
                    raise AssertionError('source generator did not complete')
            outcome = ('returned', result)
        except (ValueError, TypeError) as caught:
            outcome = ('raised', type(caught).__name__, str(caught),
                       caught.__context__ is caller, caught is failure)
            events.append(('error', type(caught).__name__, context()))
            caught.__traceback__ = None
            events.append(('traceback-cleared', context()))
        events.append(('after-call', context()))
    events.append(('after-handler', context()))
    gc.collect()
    return {
        'outcome': outcome,
        'events': events,
        'raw_alive': raw_alive(),
        'payloads_alive': [reference() is not None for reference in references],
    }
"""


_EXPANDED_ARGUMENT_CASES = (
    "prefix",
    "singleton",
    "mapping",
    "grouped_duplicate",
    "singleton_failure",
    "suspended_prefix",
    "suspended_singleton_failure",
)


def test_native_expanded_argument_phase_and_cleanup_oracle():
    namespace = {}
    exec(_EXPANDED_ARGUMENT_SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)", ""), namespace)
    exec(_EXPANDED_ARGUMENT_OBSERVER, namespace)
    observe = namespace["observe_expanded_argument_case"]
    for case in _EXPANDED_ARGUMENT_CASES:
        result = observe(namespace, case)
        events = result["events"]
        labels = [event[0] for event in events]
        assert labels.index("callee") < labels.index("source")
        assert not result["raw_alive"]
        assert not any(result["payloads_alive"])
        assert events[-2:] == [("after-call", "caller"), ("after-handler", None)]
        if case == "suspended_prefix":
            assert labels.index("iter") < labels.index("suspended") < labels.index("value")
        elif case == "suspended_singleton_failure":
            assert labels.index("suspended") < labels.index("value") < labels.index("iter")
        elif case == "prefix":
            assert labels.index("iter") < labels.index("predicate")
        elif case in ("singleton", "singleton_failure"):
            assert labels.index("value") < labels.index("iter")
        else:
            assert labels.index("getitem") < labels.index("predicate")
        if case == "grouped_duplicate":
            assert labels.index("first") < labels.index("predicate") < labels.index("value") < labels.index("error")
            assert "call" not in labels
        elif case in ("singleton_failure", "suspended_singleton_failure"):
            tail = next(index for index, event in enumerate(events) if event[:2] == ("drop", "tail"))
            assert tail < labels.index("drop-source")
            assert events[tail][2:] == ("caller", True)


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_strict_expanded_argument_callback_order_and_cleanup(
    tmp_path: Path, entry_interpreter: bool
):
    project = create_strict_project(
        tmp_path,
        {"expanded_arguments.py": _EXPANDED_ARGUMENT_SOURCE},
        modules={"expanded_arguments": "expanded_arguments.py"},
    )
    ordinary_source = _EXPANDED_ARGUMENT_SOURCE.replace("# soac: module(strict_assign=true, checked_attr=true)", "")
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    program = "\n".join(
        [
            "import expanded_arguments as actual",
            f"assert _soac_ext.strict_function_entry_kind(actual.prefix) == {expected_entry!r}",
            "assert _soac_ext.strict_function_entry_kind(actual.suspended_prefix) == 'generator_factory'",
            "assert _soac_ext.strict_function_entry_kind(actual.suspended_singleton) == 'generator_factory'",
            "ordinary = {}",
            f"exec({ordinary_source!r}, ordinary)",
            _EXPANDED_ARGUMENT_OBSERVER,
            f"cases = {_EXPANDED_ARGUMENT_CASES!r}",
            "def semantic_events(result):",
            "    return [event for event in result['events'] if event[0] not in ('drop', 'drop-source')]",
            "def released_resources(result):",
            "    return sorted(event[:2] if event[0] == 'drop' else event[:1]",
            "                  for event in result['events'] if event[0] in ('drop', 'drop-source'))",
            "failures = []",
            "for case in cases:",
            "    expected = observe_expanded_argument_case(ordinary, case)",
            "    observed = observe_expanded_argument_case(actual.__dict__, case)",
            # Explicit callback order and exception context remain required.
            # Implicit finalizer timing, order and observed context do not.
            "    if (observed['outcome'] != expected['outcome']",
            "            or semantic_events(observed) != semantic_events(expected)",
            "            or released_resources(observed) != released_resources(expected)",
            "            or observed['raw_alive'] or any(observed['payloads_alive'])):",
            "        failures.append((case, expected, observed))",
            "assert not failures, failures",
        ]
    )
    project.run(program, entry_interpreter=entry_interpreter, opt_mode="none")


_DYNAMIC_CONTEXT_SOURCE = """
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
"""

_DYNAMIC_CONTEXT_FUNCTIONS = (
    "invoke", "execute", "execute_without_locals", "compile_detached",
    "compile_inherited", "actual_globals", "captured_values", "closure_values",
    "walrus_values", "caught_exception",
)


@pytest.fixture(params=("compiled", "entry", "cpython"))
def strict_dynamic_context(tmp_path, request):
    from scripts.strict_pyperformance_sources import strict_opt_in

    mode = request.param
    source = strict_opt_in(
        _DYNAMIC_CONTEXT_SOURCE.encode(), "dynamic_context.py"
    )[0].decode()
    project = create_strict_project(
        tmp_path / "selected",
        {"dynamic_context.py": source},
        modules={"dynamic_context": "dynamic_context.py"},
        backend="cpython" if mode == "cpython" else "soac",
    )
    return project, mode


def _run_dynamic_context_validation(case, body):
    import textwrap

    project, mode = case
    prelude = f"""
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
        from tests._integration import stock_module

        with stock_module(
            Path({str(project.root / "ordinary")!r}),
            "ordinary_dynamic_context",
            {_DYNAMIC_CONTEXT_SOURCE!r},
        ) as ordinary:
            assert _soac_ext.strict_module_diagnostics(ordinary) is None
            source_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            source_owner.argtypes = [ctypes.py_object]
            source_owner.restype = ctypes.c_void_p
            for name in {_DYNAMIC_CONTEXT_FUNCTIONS!r}:
                function = vars(ordinary)[name]
                assert source_owner(function) is None, name
                assert _soac_ext.strict_function_entry_kind(function) is None, name
        for subject in (ordinary, module):
            assert vars(subject)["__builtins__"] is builtins.__dict__
            assert subject.CAPTURED_BUILTINS is not builtins.__dict__
            for name in {_DYNAMIC_CONTEXT_FUNCTIONS!r}:
                assert vars(subject)[name].__builtins__ is subject.CAPTURED_BUILTINS, name
            assert subject.actual_globals() is vars(subject)
    """
    validation = "def validate_module(module):"
    validation += "\n" + textwrap.indent(
        textwrap.dedent(prelude) + "\n" + textwrap.dedent(body), "    "
    )
    project.run_case(
        "dynamic_context",
        validation,
        project.project / "dynamic_context.py",
        entry_interpreter=mode == "entry",
        required_functions=_DYNAMIC_CONTEXT_FUNCTIONS,
    )


@pytest.mark.parametrize("call_style", ("fixed", "expanded"))
def test_ordinary_code_uses_explicit_namespaces_and_captured_builtins(
    strict_dynamic_context, call_style
):
    _run_dynamic_context_validation(
        strict_dynamic_context,
        f"""
        call_style = {call_style!r}

        def call(subject, target, code, namespace, local_namespace, omitted=False):
            if call_style == "fixed":
                if omitted:
                    return subject.execute_without_locals(target, code, namespace)
                return subject.execute(target, code, namespace, local_namespace)
            keywords = {{'globals': namespace}}
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
                namespace = {{}}
                assert call(subject, builtins.exec, statement, namespace, None, omitted) is None
                assert namespace["__builtins__"] is subject.CAPTURED_BUILTINS
                assert namespace["value"] is marker
                assert call(subject, builtins.eval, expression, namespace, None, omitted) is marker
            # Preserve an explicit pre-existing mapping, rather than installing
            # either the source capture or the unrelated driver mapping.
            explicit_marker = object()
            explicit_builtins = dict(builtins.__dict__, DYNAMIC_SENTINEL=explicit_marker)
            namespace = {{"__builtins__": explicit_builtins}}
            local_namespace = {{}}
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
            namespace = {{"__builtins__": explicit_builtins}}
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
                         {{"failure": primary, "__builtins__": builtins.__dict__}}, None)
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
            "events.append('before')\\nglobal LIMIT\\nLIMIT = replacement\\nevents.append('after')",
            "<ordinary-write>", "exec", dont_inherit=True,
        )
        replacement = object()
        ordinary_events = []
        assert call(ordinary, builtins.exec, write, vars(ordinary),
                    {{"events": ordinary_events, "replacement": replacement}}) is None
        assert ordinary.LIMIT is replacement
        assert ordinary_events == ["before", "after"]
        ordinary.LIMIT = 17
        selected_events = []
        with pytest.raises(StrictMutationError):
            call(module, builtins.exec, write, vars(module),
                 {{"events": selected_events, "replacement": replacement}})
        assert selected_events == ["before"]
        assert module.LIMIT == 17
        """,
    )


def test_noninheriting_compile_preserves_native_argument_conversion(
    strict_dynamic_context,
):
    _run_dynamic_context_validation(
        strict_dynamic_context,
        """
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
            source = "debug_value = __debug__\\nmatch 1:\\n    case 1:\\n        answer = 41\\n"
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
                    builtins.compile, "class Bad:\\n    nonlocal x\\n", 0, True
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
        """,
    )


def test_dynamic_code_does_not_inherit_execution_authority(strict_dynamic_context):
    _run_dynamic_context_validation(
        strict_dynamic_context,
        """
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
        """,
    )


def test_frame_free_capture_walrus_and_exception_cleanup(strict_dynamic_context):
    _run_dynamic_context_validation(
        strict_dynamic_context,
        """
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
        """,
    )
