"""Annotation formats through genuine checker and native startup authority."""

import textwrap
from pathlib import Path

import pytest

from tests._strict_integration import create_strict_project


@pytest.fixture(scope="module")
def unrelated_annotation_members(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-unrelated-annotation-members"),
        {
            "annotation_members.py": """
# soac: module(strict_assign=true, checked_attr=true)
from annotation_member_support import install

class Unrelated:
    install(locals(), False)
    def method(self) -> int:
        return 7

class FakeParameters:
    install(locals(), True)
    def method(self) -> int:
        return 8
""",
            "annotation_member_support.py": """
events = []

class Trap:
    def __getattribute__(self, name):
        events.append(('getattr', name))
        raise AssertionError('annotation lookup inspected an unrelated member')
    def __iter__(self):
        events.append(('iter',))
        raise AssertionError('annotation lookup iterated an unrelated member')

def install(namespace, parameters):
    namespace['__type_params__' if parameters else 'unrelated'] = Trap()
""",
        },
        modules={"annotation_members": "annotation_members.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_class_annotation_lookup_never_scans_unrelated_members_for_type_parameters(
    unrelated_annotation_members, entry_interpreter
):
    unrelated_annotation_members.run(
        """
import annotationlib
import annotation_members as module
from annotation_member_support import events

assert _soac_ext.strict_module_diagnostics(module)['sealed']
for cls in (module.Unrelated, module.FakeParameters):
    for format in (annotationlib.Format.VALUE, annotationlib.Format.FORWARDREF):
        assert annotationlib.get_annotations(cls.method, format=format) == {'return': int}
assert events == [], events
""",
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def minimal_annotations(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-minimal-annotations"),
        {
            "minimal.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                number: int = 1
                def identity(value: int) -> int:
                    return value
            """,
        },
        modules={"minimal": "minimal.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_minimal_original_providers_keep_public_and_body_parameter_layouts(
    minimal_annotations, entry_interpreter
):
    minimal_annotations.run(
        """
        import annotationlib, inspect
        import minimal

        for owner, expected in [(minimal, {'number': int}),
                                (minimal.identity, {'value': int, 'return': int})]:
            provider = owner.__annotate__
            assert provider.__code__.co_flags & 0x10000000
            parameter, = inspect.signature(provider).parameters.values()
            assert parameter.name == 'format'
            assert parameter.kind is inspect.Parameter.POSITIONAL_ONLY
            assert parameter.default is inspect.Parameter.empty
            assert provider(1) == expected
            assert annotationlib.get_annotations(owner, format=annotationlib.Format.VALUE) == expected
            assert annotationlib.get_annotations(owner, format=annotationlib.Format.STRING) == {
                name: 'int' for name in expected
            }
        assert minimal.identity(3) == 3
        print('verified-minimal-original-providers')
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def strict_annotations(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-annotation-replay"),
        {
            "annotated.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                from typing import TYPE_CHECKING
                from annotation_probe import remember, annotation_values
                if TYPE_CHECKING:
                    class Missing:
                        pass
                number: int
                items: list[Missing]
                format = str

                def factory():
                    class Local:
                        pass
                    def target(value: Local, other: format) -> list[Missing]:
                        return []
                    return target, Local

                first, FirstLocal = factory()
                second, SecondLocal = factory()

                class Item:
                    Alias = bytes
                    remember(locals())
                    item: Alias
                    if format:
                        reached: Alias
                    def method(self, value: Alias):
                        return value
                    early = annotation_values(method)
            """,
            "annotation_probe.py": """
                prepared = []
                def remember(namespace):
                    prepared.append(namespace)
                def annotation_values(function):
                    return function.__annotate__(1)
            """,
        },
        modules={"annotated": "annotated.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_function_replay_preserves_real_lexical_cells_and_source_format_name(
    strict_annotations, entry_interpreter
):
    strict_annotations.run(
        """
        import annotationlib
        import annotated

        first = annotated.first.__annotate__
        second = annotated.second.__annotate__
        assert first.__code__ is second.__code__
        assert first.__closure__ is not second.__closure__
        assert first.__code__.co_freevars == ('Local',)
        for function, expected in [(annotated.first, annotated.FirstLocal),
                                   (annotated.second, annotated.SecondLocal)]:
            values = annotationlib.get_annotations(function, format=annotationlib.Format.FORWARDREF)
            assert values['value'] is expected
            assert values['other'] is str
            assert isinstance(values['return'].__args__[0], annotationlib.ForwardRef)
            assert annotationlib.get_annotations(function, format=annotationlib.Format.STRING) == {
                'value': 'Local', 'other': 'format', 'return': 'list[Missing]'
            }
            # The contextual owner is not an authorization token. This public
            # annotationlib entrypoint deliberately supports a missing owner.
            assert annotationlib.call_annotate_function(
                function.__annotate__, annotationlib.Format.FORWARDREF, owner=None
            )['value'] is expected
        print('verified-function-annotation-cells')
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_class_replay_uses_the_copied_dictionary_not_the_prepared_namespace(
    strict_annotations, entry_interpreter
):
    strict_annotations.run(
        """
        import annotationlib
        import annotated
        import annotation_probe

        cls = annotated.Item
        assert cls.early == {'value': bytes}
        prepared, = annotation_probe.prepared
        prepared['Alias'] = float
        assert cls.Alias is bytes
        assert cls.__annotate__.__code__.co_freevars == (
            '__classdict__', '__conditional_annotations__'
        )
        assert cls.method.__annotate__.__code__.co_freevars == ('__classdict__',)
        for format in [annotationlib.Format.VALUE, annotationlib.Format.FORWARDREF]:
            assert annotationlib.get_annotations(cls, format=format) == {
                'item': bytes, 'reached': bytes
            }
            assert annotationlib.get_annotations(cls.method, format=format) == {'value': bytes}
        assert annotationlib.get_annotations(cls, format=annotationlib.Format.STRING) == {
            'item': 'Alias', 'reached': 'Alias'
        }
        assert annotationlib.get_annotations(cls.method, format=annotationlib.Format.STRING) == {
            'value': 'Alias'
        }
        assert not hasattr(cls, '__classdictcell__')
        print('verified-class-annotation-cell')
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_nested_forwardref_uses_native_annotationlib_replay(
    strict_annotations, entry_interpreter
):
    strict_annotations.run(
        """
        import annotationlib, inspect, marshal, types, _typing
        import annotated
        from soac.strict import StrictRuntimeUnavailableError

        provider = annotated.__annotate__
        parameters = list(inspect.signature(provider).parameters.values())
        assert [(item.name, item.kind, item.default) for item in parameters] == [
            ("format", inspect.Parameter.POSITIONAL_ONLY, inspect.Parameter.empty)
        ]
        assert provider.__code__.co_flags & 0x10000000
        result = annotationlib.get_annotations(
            annotated, format=annotationlib.Format.FORWARDREF
        )
        assert result["number"] is int
        assert isinstance(result["items"], types.GenericAlias), result
        assert result["items"].__origin__ is list
        missing, = result["items"].__args__
        assert isinstance(missing, annotationlib.ForwardRef), result
        assert missing.__forward_arg__ == "Missing"
        assert annotationlib.get_annotations(
            annotated, format=annotationlib.Format.STRING
        ) == {"number": "int", "items": "list[Missing]"}

        for code in (provider.__code__, provider.__code__.replace(),
                     marshal.loads(marshal.dumps(provider.__code__))):
            forged = types.FunctionType(code, annotated.__dict__)
            try:
                forged(2)
            except StrictRuntimeUnavailableError:
                pass
            else:
                raise AssertionError("replay authorized original strict bytecode")
            try:
                _typing._soac_annotation_replay_code(forged, None, 4)
            except StrictRuntimeUnavailableError:
                pass
            else:
                raise AssertionError("copied strict code gained replay provenance")
        try:
            _typing._soac_annotation_replay_code(annotated.factory, None, 4)
        except StrictRuntimeUnavailableError:
            pass
        else:
            raise AssertionError("a source-function owner was accepted as an annotation provider")
        print("verified-annotation-replay")
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def future_annotations(tmp_path_factory):
    source = '''
        """future annotation module"""
        from __future__ import annotations
        from typing import Literal
        from future_probe import snapshot

        start = snapshot(globals())
        value: list[tuple[int, str]] = []
        if False:
            absent: str

        class Item:
            """future annotation class"""
            start = snapshot(locals())
            quoted: "Item"
            if True:
                selected: Literal["a", "b"]
            def method(self, item: "Item") -> "Item":
                return item

        def shape(left: "Item", /, right: tuple["Item", ...]) -> list["Item"]:
            return []
    '''
    return create_strict_project(
        tmp_path_factory.mktemp("strict-future-annotations"),
        {
            "future_subject.py": (
                "# soac: module(strict_assign=true, checked_attr=true)\n"
                + textwrap.dedent(source)
            ),
            "future_control.py": source,
            "future_probe.py": """
                def snapshot(namespace):
                    return {
                        "has_annotations": "__annotations__" in namespace,
                        "annotations": dict(namespace.get("__annotations__", {})),
                        "has_provider": "__annotate__" in namespace,
                        "doc": namespace.get("__doc__"),
                    }
            """,
        },
        modules={"future_subject": "future_subject.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_future_annotations_match_native_scope_timing_strings_and_provider_layout(
    future_annotations, entry_interpreter
):
    future_annotations.run(
        """
        import annotationlib, inspect
        import future_control as control
        import future_subject as subject

        for candidate, ordinary in [(subject, control),
                                    (subject.Item, control.Item)]:
            assert candidate.start == ordinary.start
            assert candidate.start["has_annotations"]
            assert candidate.start["annotations"] == {}
            assert not candidate.start["has_provider"]
            assert candidate.__annotate__ is ordinary.__annotate__ is None
            assert list(candidate.__annotations__.items()) == list(ordinary.__annotations__.items())
            for format in [annotationlib.Format.VALUE, annotationlib.Format.STRING,
                           annotationlib.Format.FORWARDREF]:
                assert annotationlib.get_annotations(candidate, format=format) == (
                    annotationlib.get_annotations(ordinary, format=format)
                )

        for candidate, ordinary in [(subject.shape, control.shape),
                                    (subject.Item.method, control.Item.method)]:
            provider = candidate.__annotate__
            baseline = ordinary.__annotate__
            assert provider is not None and baseline is not None
            assert str(inspect.signature(provider)) == str(inspect.signature(baseline))
            assert provider.__code__.co_freevars == baseline.__code__.co_freevars == ()
            assert provider.__closure__ is baseline.__closure__ is None
            assert provider.__code__.co_flags & 0x10000000
            assert list(provider(1).items()) == list(baseline(1).items())
            for format in [annotationlib.Format.VALUE, annotationlib.Format.STRING,
                           annotationlib.Format.FORWARDREF]:
                assert annotationlib.get_annotations(candidate, format=format) == (
                    annotationlib.get_annotations(ordinary, format=format)
                )
        print("verified-future-annotation-native-protocol")
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def lazy_aliases(tmp_path_factory):
    source = """
        type Forward = list[Later]
        type Recursive = tuple[int, Recursive]

        class Later:
            pass
    """
    return create_strict_project(
        tmp_path_factory.mktemp("strict-lazy-aliases"),
        {
            "alias_subject.py": "# soac: module(strict_assign=true, checked_attr=true)\n"
            + textwrap.dedent(source),
            "alias_control.py": source,
        },
        modules={"alias_subject": "alias_subject.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_type_alias_values_are_lazy_and_use_native_evaluator_replay(
    lazy_aliases, entry_interpreter
):
    lazy_aliases.run(
        """
        import annotationlib, inspect, marshal, types
        import alias_control as control
        import alias_subject as subject

        assert subject.Forward.__value__ == list[subject.Later]
        assert subject.Recursive.__value__ == tuple[int, subject.Recursive]
        assert subject.Forward.__value__ is subject.Forward.__value__
        for candidate, ordinary in [(subject.Forward, control.Forward),
                                    (subject.Recursive, control.Recursive)]:
            evaluator = candidate.evaluate_value
            baseline = ordinary.evaluate_value
            signature_errors = []
            for provider in (evaluator, baseline):
                code = provider.__code__
                assert (code.co_argcount, code.co_posonlyargcount, code.co_kwonlyargcount) == (1, 1, 0)
                assert code.co_varnames[0] == '.format'
                try:
                    inspect.signature(provider)
                except ValueError as error:
                    signature_errors.append(error.args)
                else:
                    raise AssertionError('native type evaluator has no valid inspect parameter name')
            assert signature_errors[0] == signature_errors[1]
            assert evaluator.__defaults__ == baseline.__defaults__ == (1,)
            assert evaluator.__code__.co_freevars == baseline.__code__.co_freevars == ()
            assert evaluator.__closure__ is baseline.__closure__ is None
            assert evaluator.__code__.co_flags & 0x10000000
            assert annotationlib.call_evaluate_function(
                evaluator, annotationlib.Format.STRING, owner=candidate
            ) == annotationlib.call_evaluate_function(
                baseline, annotationlib.Format.STRING, owner=ordinary
            )
            assert annotationlib.call_evaluate_function(
                evaluator, annotationlib.Format.FORWARDREF, owner=None
            ) == candidate.__value__
            for code in [evaluator.__code__, evaluator.__code__.replace(),
                         marshal.loads(marshal.dumps(evaluator.__code__))]:
                forged = types.FunctionType(code, evaluator.__globals__, argdefs=(1,))
                try:
                    forged()
                except ImportError as error:
                    assert "strict code execution" in str(error)
                else:
                    raise AssertionError("copied strict evaluator bytecode executed natively")
        print("verified-lazy-type-alias-evaluators")
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def generic_type_expressions(tmp_path_factory):
    alias_source = """
        import typing

        T = str
        type Forward[T: Later = Later] = list[T]
        type Constrained[T: (First, Second) = First] = tuple[T]
        type CallableAlias[**P = [int, str]] = typing.Callable[P, int]

        def repeated():
            type Piece[T] = list[T]
            return Piece

        class Later:
            pass

        class First:
            pass

        class Second:
            pass
    """
    scope_source = """
        T = str

        def identity[T: Later = Later](value: T) -> T:
            return value

        def default_from_outer[T](value=T):
            return value

        class Generic[T: Later = Later]:
            item: T

        class Later:
            pass
    """
    pack_source = "type TupleAlias[*Ts = *tuple[int, str]] = tuple[*Ts]\n"
    class_context_source = """
        T = bytes

        class Holder:
            class Local:
                pass

            def method[T](self, value: Local) -> Local:
                return value

            def identity[T](self, value: T) -> T:
                return value

            def from_global(self, value: T) -> T:
                return value
    """
    files = {}
    modules = {}
    for stem, source in [
        ("generic", alias_source),
        ("generic_scope", scope_source),
        ("generic_pack", pack_source),
        ("generic_context", class_context_source),
    ]:
        files[f"{stem}_subject.py"] = (
            "# soac: module(strict_assign=true, checked_attr=true)\n" + textwrap.dedent(source)
        )
        files[f"{stem}_control.py"] = source
        modules[f"{stem}_subject"] = f"{stem}_subject.py"
    return create_strict_project(
        tmp_path_factory.mktemp("strict-generic-type-expressions"),
        files,
        modules=modules,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generic_alias_bounds_constraints_and_defaults_are_lazy_native_evaluators(
    generic_type_expressions, entry_interpreter
):
    generic_type_expressions.run(
        """
        import annotationlib, inspect, types, typing
        import generic_control as control
        import generic_subject as subject

        assert subject.T is control.T is str
        assert 'P' not in vars(subject) and 'Ts' not in vars(subject)
        bound, = subject.Forward.__type_params__
        assert bound.__bound__ is subject.Later
        assert bound.__default__ is subject.Later
        assert subject.Forward.__value__ == list[bound]
        constrained, = subject.Constrained.__type_params__
        assert constrained.__constraints__ == (subject.First, subject.Second)
        assert constrained.__default__ is subject.First
        assert subject.Constrained.__value__ == tuple[constrained]

        for name in ['Forward', 'Constrained', 'CallableAlias']:
            alias, ordinary = getattr(subject, name), getattr(control, name)
            parameter, = alias.__type_params__
            baseline, = ordinary.__type_params__
            for attribute in ['evaluate_bound', 'evaluate_constraints', 'evaluate_default']:
                evaluator = getattr(parameter, attribute, None)
                previous = getattr(baseline, attribute, None)
                if evaluator is None:
                    assert previous is None
                    continue
                assert isinstance(evaluator, types.FunctionType)
                for provider in (evaluator, previous):
                    code = provider.__code__
                    assert (code.co_argcount, code.co_posonlyargcount, code.co_kwonlyargcount) == (1, 1, 0)
                    assert code.co_varnames[0] == '.format'
                assert evaluator.__defaults__ == previous.__defaults__ == (1,)
                assert evaluator.__code__.co_flags & 0x10000000
                assert annotationlib.call_evaluate_function(
                    evaluator, annotationlib.Format.STRING, owner=parameter
                ) == annotationlib.call_evaluate_function(
                    previous, annotationlib.Format.STRING, owner=baseline
                )
            assert parameter.__default__ is parameter.__default__
        paramspec, = subject.CallableAlias.__type_params__
        assert paramspec.__default__ == [int, str]
        print('verified-generic-lazy-type-evaluators')
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generic_function_and_class_providers_keep_the_parameter_scope_private(
    generic_type_expressions, entry_interpreter
):
    generic_type_expressions.run(
        """
        import annotationlib
        import generic_scope_control as control
        import generic_scope_subject as subject

        assert subject.T is control.T is str
        assert subject.default_from_outer() is control.default_from_outer() is str
        parameter, = subject.identity.__type_params__
        assert parameter.__bound__ is subject.Later
        assert parameter.__default__ is subject.Later
        annotations = annotationlib.get_annotations(subject.identity)
        assert annotations == {'value': parameter, 'return': parameter}
        value = subject.Later()
        assert subject.identity(value) is value
        class_parameter, = subject.Generic.__type_params__
        assert class_parameter is not parameter
        assert class_parameter.__bound__ is subject.Later
        assert class_parameter.__default__ is subject.Later
        assert 'T' not in vars(subject.Generic)
        assert annotationlib.get_annotations(subject.Generic) == {'item': class_parameter}
        assert annotationlib.get_annotations(
            subject.Generic, format=annotationlib.Format.STRING
        ) == annotationlib.get_annotations(control.Generic, format=annotationlib.Format.STRING)
        print('verified-private-generic-parameter-scope')
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generic_typevartuple_starred_default_uses_the_native_single_unpack(
    generic_type_expressions, entry_interpreter
):
    generic_type_expressions.run(
        """
        import annotationlib, types
        import generic_pack_control as control
        import generic_pack_subject as subject

        pack, = subject.TupleAlias.__type_params__
        previous, = control.TupleAlias.__type_params__
        assert 'Ts' not in vars(subject)
        evaluator = pack.evaluate_default
        assert isinstance(evaluator, types.FunctionType)
        assert evaluator.__code__.co_varnames[0] == '.format'
        assert evaluator.__defaults__ == previous.evaluate_default.__defaults__ == (1,)
        assert evaluator.__code__.co_flags & 0x10000000
        assert pack.__default__ == previous.__default__
        assert pack.__default__ is pack.__default__
        assert annotationlib.call_evaluate_function(
            evaluator, annotationlib.Format.STRING, owner=pack
        ) == annotationlib.call_evaluate_function(
            previous.evaluate_default, annotationlib.Format.STRING, owner=previous
        )
        assert subject.TupleAlias.__value__ == tuple[*pack]
        print('verified-starred-type-parameter-default')
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_provenance_only_generic_scopes_and_evaluators_do_not_grow_pending_targets(
    generic_type_expressions, entry_interpreter
):
    generic_type_expressions.run(
        """
        import annotationlib, gc, weakref
        import generic_subject as subject

        def pending_edges():
            # Observe the dictionary's GC-visible owner graph, without using
            # its metadata or Python attributes as execution authority.
            return sum(isinstance(edge, weakref.ReferenceType)
                       for owner in gc.get_referents(vars(subject))
                       for edge in gc.get_referents(owner))

        initial = pending_edges()
        for _ in range(200):
            alias = subject.repeated()
            parameter, = alias.__type_params__
            assert alias.__value__ == list[parameter]
            assert annotationlib.call_evaluate_function(
                alias.evaluate_value, annotationlib.Format.STRING, owner=alias
            ) == 'list[T]'
            del alias, parameter
        gc.collect()
        assert pending_edges() == initial
        print('verified-bounded-provenance-only-type-factories')
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_generic_method_providers_inherit_only_the_active_class_execution(
    generic_type_expressions, entry_interpreter
):
    generic_type_expressions.run(
        """
        import annotationlib
        import generic_context_subject as subject

        method = subject.Holder.method
        parameter, = method.__type_params__
        assert parameter.__name__ == 'T'
        assert annotationlib.get_annotations(method) == {
            'value': subject.Holder.Local, 'return': subject.Holder.Local,
        }
        own_parameter, = subject.Holder.identity.__type_params__
        assert own_parameter is not parameter
        assert annotationlib.get_annotations(subject.Holder.identity) == {
            'value': own_parameter, 'return': own_parameter,
        }
        # A sibling's same-named type parameter is not a lexical binding for
        # this nongeneric method. Lookup must use the actual global alias.
        assert subject.T is bytes
        assert annotationlib.get_annotations(subject.Holder.from_global) == {
            'value': bytes, 'return': bytes,
        }
        instance = subject.Holder()
        arbitrary = object()
        assert instance.identity(arbitrary) is arbitrary
        assert instance.from_global(b'value') == b'value'
        local = subject.Holder.Local()
        assert instance.method(local) is local
        assert instance.method(arbitrary) is arbitrary
        print('verified-generic-active-class-execution')
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def decorated_class_annotations(tmp_path_factory, request):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-decorated-class-annotations"),
        {
            "decorated_classes.py": """
# soac: module(strict_assign=true, checked_attr=true)
from typing import final

@final
# Native class providers start at the first decorator, not this header.
class Item:
    value: int = 1
    @final
    def method(self, number: int) -> int:
        return number

def factory():
    @final
    # The same projection must survive a real factory execution.
    class Local:
        value: int = 2
        @final
        def method(self, number: int) -> int:
            return number
    return Local

def identity(value):
    return value

@identity
# A generic wrapper must preserve the same class/provider start line.
class Generic[T]:
    value: T

def generic_factory():
    @identity
    class Local[T]:
        value: T
    return Local

@identity
def generic_function[T](value: T) -> T:
    return value

@identity
async def generic_async[T](value: T) -> T:
    return value
""",
        },
        modules={"decorated_classes": "decorated_classes.py"},
        backend=getattr(request, "param", "soac"),
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_decorated_class_and_method_providers_match_their_distinct_native_lines(
    decorated_class_annotations, entry_interpreter
):
    decorated_class_annotations.run(
        f"""
        import annotationlib, asyncio
        import types
        from pathlib import Path
        import decorated_classes as subject

        assert _soac_ext.strict_module_diagnostics(subject)['sealed']
        expected_entry = {"entry_interpreter" if entry_interpreter else "checked_native"!r}
        assert _soac_ext.strict_function_entry_kind(subject.factory) == expected_entry
        local = subject.factory()
        # Native compilation is a control only: these code objects are never
        # executed and never supply runtime admission or annotation targets.
        root = compile(Path(subject.__file__).read_text(), subject.__file__, 'exec')
        codes = []
        def collect(code):
            codes.append(code)
            for constant in code.co_consts:
                if isinstance(constant, types.CodeType):
                    collect(constant)
        collect(root)
        for cls in (subject.Item, local):
            class_code, = [code for code in codes if code.co_qualname == cls.__qualname__]
            native_class_provider, = [code for code in class_code.co_consts
                                      if isinstance(code, types.CodeType)
                                      and code.co_name == '__annotate__'
                                      and code.co_firstlineno == class_code.co_firstlineno]
            provider = cls.__annotate__
            assert provider.__code__.co_firstlineno == native_class_provider.co_firstlineno
            assert provider.__code__.co_freevars == native_class_provider.co_freevars
            assert provider(1) == {{'value': int}}
            assert annotationlib.get_annotations(cls, format=annotationlib.Format.STRING) == {{
                'value': 'int',
            }}
            method = vars(cls)['method']
            assert _soac_ext.strict_function_entry_kind(method) == expected_entry
            assert annotationlib.get_annotations(method) == {{'number': int, 'return': int}}
            assert method.__annotate__.__code__.co_firstlineno > method.__code__.co_firstlineno
            assert cls().method(7) == 7
        assert _soac_ext.strict_function_entry_kind(subject.generic_factory) == expected_entry
        for cls in (subject.Generic, subject.generic_factory()):
            parameter, = cls.__type_params__
            class_code, = [code for code in codes if code.co_qualname == cls.__qualname__]
            native_provider, = [code for code in class_code.co_consts
                                if isinstance(code, types.CodeType)
                                and code.co_name == '__annotate__']
            provider = cls.__annotate__
            assert provider.__code__.co_firstlineno == native_provider.co_firstlineno
            assert provider.__code__.co_firstlineno == class_code.co_firstlineno
            assert provider.__code__.co_freevars == native_provider.co_freevars
            assert provider(1) == {{'value': parameter}}
            assert annotationlib.get_annotations(cls, format=annotationlib.Format.STRING) == {{
                'value': 'T',
            }}
        assert _soac_ext.strict_function_entry_kind(subject.generic_function) == expected_entry
        for function in (subject.generic_function, subject.generic_async):
            parameter, = function.__type_params__
            assert annotationlib.get_annotations(function) == {{'value': parameter, 'return': parameter}}
            assert function.__annotate__.__code__.co_firstlineno > function.__code__.co_firstlineno
        value = object()
        assert subject.generic_function(value) is value
        assert _soac_ext.strict_function_entry_kind(subject.generic_async) == 'generator_factory'
        assert asyncio.run(subject.generic_async(value)) is value
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.fixture(scope="module")
def user_annotation_callbacks(tmp_path_factory):
    source = """
# soac: module(strict_assign=true, checked_attr=true)
from annotationlib import Format

def annotate(format, /, __Format=Format, __Unsupported=NotImplementedError):
    if format == __Format.VALUE:
        return {'x': str}
    if format == __Format.VALUE_WITH_FAKE_GLOBALS:
        return {'x': int}
    raise __Unsupported(format)

def make_callback(local):
    def callback(format, /, __Format=Format, __Unsupported=NotImplementedError):
        if format == __Format.VALUE:
            return {'x': local}
        if format == __Format.VALUE_WITH_FAKE_GLOBALS:
            return {'x': (lambda: local)()}
        raise __Unsupported(format)
    return callback

def checked_callback(format: int, /, __Format=Format, __Unsupported=NotImplementedError):
    if format == __Format.VALUE:
        return {'x': str}
    if format == __Format.VALUE_WITH_FAKE_GLOBALS:
        return {'x': int}
    raise __Unsupported(format)

def nested_checked_callback(format, /):
    def checked(value: int) -> int:
        return value
    return {'x': checked}

def nested_class_callback(format, /):
    class Local:
        value: int
    return {'x': Local}
"""
    return create_strict_project(
        tmp_path_factory.mktemp("strict-user-annotation-callbacks"),
        {
            "user_annotations.py": source,
            "user_annotations_control.py": source.replace(
                "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1
            ),
        },
        modules={"user_annotations": "user_annotations.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_user_annotation_callback_replay_uses_ordinary_fake_globals(
    user_annotation_callbacks, entry_interpreter
):
    user_annotation_callbacks.run(
        f"""
        import annotationlib
        import user_annotations as subject
        import user_annotations_control as control

        expected_entry = {"entry_interpreter" if entry_interpreter else "checked_native"!r}
        for selected, ordinary in (
            (subject.annotate, control.annotate),
            (subject.checked_callback, control.checked_callback),
            (subject.make_callback(int), control.make_callback(int)),
            (subject.make_callback(str), control.make_callback(str)),
        ):
            assert _soac_ext.strict_function_entry_kind(selected) == expected_entry
            assert _soac_ext.strict_function_entry_kind(ordinary) is None
            for format in (
                annotationlib.Format.VALUE,
                annotationlib.Format.FORWARDREF,
                annotationlib.Format.STRING,
            ):
                expected = annotationlib.call_annotate_function(ordinary, format)
                assert annotationlib.call_annotate_function(selected, format) == expected
            assert _soac_ext.strict_function_entry_kind(selected) == expected_entry
        assert annotationlib.call_annotate_function(
            subject.annotate, annotationlib.Format.STRING
        ) == {{'x': 'int'}}
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_user_annotation_replay_tree_does_not_receive_source_or_jit_authority(
    user_annotation_callbacks, entry_interpreter
):
    user_annotation_callbacks.run(
        """
        import ctypes, types, _typing
        import user_annotations as subject

        source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
        source_id.argtypes = [ctypes.py_object]
        source_id.restype = ctypes.c_uint64
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p

        for callback in (subject.annotate, subject.make_callback(int),
                         subject.checked_callback, subject.nested_checked_callback):
            original = callback.__code__
            replay = _typing._soac_annotation_replay_code(callback, None, 4)
            pending = [replay]
            while pending:
                code = pending.pop()
                assert source_id(code) == 0
                assert not code.co_flags & 0x10000000
                pending.extend(item for item in code.co_consts if type(item) is types.CodeType)
            copied = types.FunctionType(
                replay, {'__builtins__': callback.__builtins__, 'int': bytes},
                argdefs=callback.__defaults__, closure=callback.__closure__,
                kwdefaults=callback.__kwdefaults__,
            )
            assert owner(copied) is None
            assert metadata(copied) is None
            assert _soac_ext.strict_function_entry_kind(copied) is None
            if callback is subject.nested_checked_callback:
                nested = copied(2)['x']
                assert type(nested) is types.FunctionType
                assert owner(nested) is None and metadata(nested) is None
                assert source_id(nested.__code__) == 0
                assert _soac_ext.strict_function_entry_kind(nested) is None
                marker = object()
                assert nested(marker) is marker
            else:
                expected = bytes if callback in (subject.annotate, subject.checked_callback) else int
                assert copied(2) == {'x': expected}
            assert callback.__code__ is original
            assert source_id(original) != 0
            assert original.co_flags & 0x10000000
            assert owner(callback) is not None
            assert metadata(callback) is not None
        """,
        entry_interpreter=entry_interpreter,
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_user_annotation_replay_cannot_copy_class_contracts_or_unowned_code(
    user_annotation_callbacks, entry_interpreter
):
    user_annotation_callbacks.run(
        """
        import annotationlib, marshal, types, _typing
        import user_annotations as subject
        import user_annotations_control as control
        from soac.strict import StrictRuntimeUnavailableError

        # A CodeType-only replay result cannot enforce a selected class
        # contract. Function annotations alone no longer cause this refusal.
        assert annotationlib.call_annotate_function(
            control.checked_callback, annotationlib.Format.STRING
        ) == {'x': 'int'}
        for function in (
            subject.nested_class_callback,
        ):
            try:
                _typing._soac_annotation_replay_code(function, None, 4)
            except StrictRuntimeUnavailableError:
                pass
            else:
                raise AssertionError('replay stripped a required source contract')

        for code in (
            subject.annotate.__code__,
            subject.annotate.__code__.replace(),
            marshal.loads(marshal.dumps(subject.annotate.__code__)),
        ):
            copied = types.FunctionType(
                code, subject.annotate.__globals__, argdefs=subject.annotate.__defaults__
            )
            for operation in (
                lambda: copied(2),
                lambda: _typing._soac_annotation_replay_code(copied, None, 4),
            ):
                try:
                    operation()
                except StrictRuntimeUnavailableError:
                    pass
                else:
                    raise AssertionError('a copied function acquired source replay authority')
        """,
        entry_interpreter=entry_interpreter,
    )


# Both native providers keep the same outer cell even when their own class
# dictionaries choose different values. Nothing mutates a sealed class.
_ANNOTATION_CELL_SHADOW_SOURCE = """
def build():
    class Alias:
        pass

    class Shadow:
        locals()['Alias'] = bytes
        value: Alias

        def method(self, value: Alias) -> Alias:
            return value

    class Fallback:
        value: Alias

        def method(self, value: Alias) -> Alias:
            return value

    return Shadow, Fallback, Alias
"""

_ANNOTATION_CELL_SHADOW_OBSERVER = """
def observe(build):
    shadow, fallback, outer = build()
    fallback_cells = []
    rows = []
    for cls, expected, has_shadow in (
        (shadow, bytes, True),
        (fallback, outer, False),
    ):
        class_provider = cls.__annotate__
        method_provider = cls.method.__annotate__
        class_cells = dict(zip(
            class_provider.__code__.co_freevars,
            class_provider.__closure__,
        ))
        method_cells = dict(zip(
            method_provider.__code__.co_freevars,
            method_provider.__closure__,
        ))
        fallback_cells.extend((class_cells['Alias'], method_cells['Alias']))
        namespace_cell = class_cells['__classdict__']
        namespace = namespace_cell.cell_contents
        class_values = class_provider(1)
        method_values = method_provider(1)
        rows.append({
            'class_value': class_values['value'] is expected,
            'method_value': method_values['value'] is expected,
            'method_return': method_values['return'] is expected,
            'class_dictionary': (
                ('Alias' in namespace) == has_shadow
                and namespace.get('Alias', outer) is expected
            ),
            'shared_dictionary_cell': (
                method_cells['__classdict__'] is namespace_cell
            ),
        })
    return {
        'rows': rows,
        'shared_outer_cell': all(
            cell is fallback_cells[0] for cell in fallback_cells
        ),
        'outer_cell_contents': all(
            cell.cell_contents is outer for cell in fallback_cells
        ),
    }
"""

_ANNOTATION_CELL_SHADOW_EXPECTED = {
    "rows": [
        {
            "class_value": True,
            "method_value": True,
            "method_return": True,
            "class_dictionary": True,
            "shared_dictionary_cell": True,
        },
        {
            "class_value": True,
            "method_value": True,
            "method_return": True,
            "class_dictionary": True,
            "shared_dictionary_cell": True,
        },
    ],
    "shared_outer_cell": True,
    "outer_cell_contents": True,
}


def test_annotation_dictionary_cell_fallback_native():
    namespace = {"__name__": "annotation_cell_native"}
    observer = {}
    exec(  # noqa: S102 - compile the fixed fixture with the native interpreter
        compile(_ANNOTATION_CELL_SHADOW_SOURCE, "<annotation-cell-native>", "exec", dont_inherit=True),
        namespace,
    )
    exec(  # noqa: S102 - the observer is a fixed, non-transformed fixture
        compile(_ANNOTATION_CELL_SHADOW_OBSERVER, "<annotation-cell-observer>", "exec", dont_inherit=True),
        observer,
    )
    assert observer["observe"](namespace["build"]) == _ANNOTATION_CELL_SHADOW_EXPECTED


@pytest.fixture(scope="module")
def annotation_dictionary_cell_fallback(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-annotation-dictionary-cell"),
        {
            "annotation_cell_subject.py": (
                "# soac: module(strict_assign=true, checked_attr=true)\n" + _ANNOTATION_CELL_SHADOW_SOURCE
            ),
            "annotation_cell_control.py": _ANNOTATION_CELL_SHADOW_SOURCE,
            "annotation_cell_observer.py": _ANNOTATION_CELL_SHADOW_OBSERVER,
        },
        modules={"annotation_cell_subject": "annotation_cell_subject.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_annotation_dictionary_cell_fallback_matches_native(
    annotation_dictionary_cell_fallback, entry_interpreter
):
    annotation_dictionary_cell_fallback.run(
        f"""
        import annotation_cell_control as control
        import annotation_cell_subject as subject
        from annotation_cell_observer import observe

        assert _soac_ext.strict_function_entry_kind(control.build) is None
        expected = observe(control.build)
        assert expected == {_ANNOTATION_CELL_SHADOW_EXPECTED!r}, expected
        assert _soac_ext.strict_module_diagnostics(subject)['sealed']
        expected_entry = {"entry_interpreter" if entry_interpreter else "checked_native"!r}
        assert _soac_ext.strict_function_entry_kind(subject.build) == expected_entry
        actual = observe(subject.build)
        assert actual == expected, (actual, expected)
        """,
        entry_interpreter=entry_interpreter,
    )


_CLASS_ALIAS_SHADOW_SOURCE = """
def build():
    class Alias:
        pass

    class Shadow:
        Alias = bytes
        type Selected = Alias

    class Fallback:
        type Selected = Alias

    return Shadow, Fallback, Alias
"""


def test_class_type_alias_uses_native_dictionary_and_cell_fallbacks():
    namespace = {"__name__": "class_alias_native"}
    exec(  # noqa: S102 - compile the fixed fixture with the native interpreter
        compile(_CLASS_ALIAS_SHADOW_SOURCE, "<class-alias-native>", "exec", dont_inherit=True),
        namespace,
    )
    shadow, fallback, outer = namespace["build"]()
    assert shadow.Selected.__value__ is bytes
    assert fallback.Selected.__value__ is outer


@pytest.fixture(scope="module")
def class_alias_shadow(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-class-alias-shadow"),
        {
            "class_alias_subject.py": (
                "# soac: module(strict_assign=true, checked_attr=true)\n" + _CLASS_ALIAS_SHADOW_SOURCE
            ),
            "class_alias_control.py": _CLASS_ALIAS_SHADOW_SOURCE,
        },
        modules={"class_alias_subject": "class_alias_subject.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_class_type_alias_shadow_matches_native(class_alias_shadow, entry_interpreter):
    class_alias_shadow.run(
        f"""
        import class_alias_control as control
        import class_alias_subject as subject

        assert _soac_ext.strict_function_entry_kind(control.build) is None
        assert _soac_ext.strict_module_diagnostics(subject)['sealed']
        expected_entry = {"entry_interpreter" if entry_interpreter else "checked_native"!r}
        assert _soac_ext.strict_function_entry_kind(subject.build) == expected_entry
        for build in (control.build, subject.build):
            shadow, fallback, outer = build()
            assert shadow.Selected.__value__ is bytes
            assert fallback.Selected.__value__ is outer
        """,
        entry_interpreter=entry_interpreter,
    )



@pytest.mark.parametrize("mutation", ["code", "closure"])
def test_native_common_owner_replay_rechecks_actual_provider_after_code_watcher(tmp_path, mutation):
    source = """
def build():
    class Local:
        pass
    def target(value: Local) -> Local:
        return value
    inspect_before_seal(target)
    return target, Local

target, Local = build()
"""
    support = """
import ctypes
import types

events = []

def inspect_before_seal(function):
    import _typing
    from soac import _soac_ext
    from soac.strict import StrictRuntimeUnavailableError
    provider = function.__annotate__
    assert _soac_ext.strict_function_diagnostics(provider)["backend"] == "cpython"
    assert _soac_ext.strict_function_diagnostics(provider)["finalized"] is False
    original = provider.__code__
    closure = provider.__closure__
    assert len(closure) == 1
    replacement_code = original.replace()
    replacement_closure = (types.CellType(str),)
    setter = ctypes.pythonapi.PyFunction_SetClosure
    setter.argtypes = [ctypes.py_object, ctypes.py_object]
    setter.restype = ctypes.c_int
    watcher_type = ctypes.PYFUNCTYPE(ctypes.c_int, ctypes.c_int, ctypes.py_object)
    created_codes = []
    callback_errors = []

    @watcher_type
    def callback(event, created):
        if event == 0 and not created_codes:
            created_codes.append(created)
            try:
                if MUTATION == "code":
                    provider.__code__ = replacement_code
                else:
                    setter(provider, replacement_closure)
            except BaseException as error:
                callback_errors.append(error)
        return 0

    add = ctypes.pythonapi.PyCode_AddWatcher
    add.argtypes = [watcher_type]
    add.restype = ctypes.c_int
    clear = ctypes.pythonapi.PyCode_ClearWatcher
    clear.argtypes = [ctypes.c_int]
    clear.restype = ctypes.c_int
    watcher = add(callback)
    try:
        try:
            _typing._soac_annotation_replay_code(provider, None, 4)
        except StrictRuntimeUnavailableError:
            events.append("watcher mutation refused")
        else:
            raise AssertionError("replay accepted a callback-mutated original provider")
        assert callback_errors == [], callback_errors
        assert len(created_codes) == 1
        source_id = ctypes.pythonapi.PyCode_GetSoacStrictSourceId
        source_id.argtypes = [ctypes.py_object]
        source_id.restype = ctypes.c_uint64
        assert source_id(created_codes[0]) == 0
        assert not created_codes[0].co_flags & 0x10000000
        if MUTATION == "code":
            assert provider.__code__ is replacement_code
        else:
            assert provider.__closure__ is replacement_closure
    finally:
        clear(watcher)
        provider.__code__ = original
        setter(provider, closure)
    assert provider.__code__ is original and provider.__closure__ is closure
"""
    project = create_strict_project(
        tmp_path,
        {
            "native_replay_owner.py": (
                "# soac: module(strict_assign=true, checked_attr=true)\n"
                "from native_replay_probe import inspect_before_seal\n" + source
            ),
            "native_replay_probe.py": f"MUTATION = {mutation!r}\n" + support,
            "ordinary_replay_owner.py": "def inspect_before_seal(function):\n    pass\n" + source,
        },
        modules={"native_replay_owner": "native_replay_owner.py"},
        backend="cpython",
    )
    project.run_case(
        "native_replay_owner",
        textwrap.dedent("""
        def validate(module):
            import annotationlib
            import ordinary_replay_owner as ordinary
            from native_replay_probe import events
            from soac import _soac_ext
            assert events == ["watcher mutation refused"]
            for function, Local in [(module.target, module.Local), (ordinary.target, ordinary.Local)]:
                assert annotationlib.get_annotations(function) == {"value": Local, "return": Local}
                value = Local()
                assert function(value) is value
            provider = module.target.__annotate__
            assert _soac_ext.strict_function_diagnostics(provider)["finalized"] is True
            assert annotationlib.get_annotations(
                module.target, format=annotationlib.Format.STRING,
            ) == {"value": "Local", "return": "Local"}
        """),
        tmp_path / "native_replay_owner_validation.py",
        required_functions=("build", "target"),
        
    )


@pytest.mark.parametrize("decorated_class_annotations", ["cpython"], indirect=True)
def test_cpython_class_final_decorator_remains_dynamic_without_a_supported_class_adapter(
    decorated_class_annotations,
):
    decorated_class_annotations.run_case(
        "decorated_classes",
        """
import ctypes
from pathlib import Path
import decorated_classes as subject
from soac import _soac_ext
from tests._strict_integration import _assert_cpython_function_witness
from tests.test_strict_type_native import ConstructionInfoV1

get_type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
get_type_owner.argtypes = [ctypes.py_object]
get_type_owner.restype = ctypes.c_void_p
get_construction = ctypes.pythonapi.PyType_GetSoacConstructionInfoV1
get_construction.argtypes = [
    ctypes.py_object, ctypes.POINTER(ConstructionInfoV1), ctypes.c_size_t,
]
get_construction.restype = ctypes.c_int
module_witness = _soac_ext.strict_module_diagnostics(subject)
ordinary_source = Path(subject.__file__).read_text().replace(
    "# soac: module(strict_assign=true, checked_attr=true)\\n", "", 1,
)
ordinary = {"__name__": "ordinary_final_class_control"}
exec(compile(ordinary_source, "<ordinary-final-class-control>", "exec"), ordinary)
first, second = subject.factory(), subject.factory()
assert first is not second
for cls, control in (
    (subject.Item, ordinary["Item"]),
    (first, ordinary["factory"]()),
    (second, ordinary["factory"]()),
):
    # The annotation remains visible, but an unsupported class decorator
    # declines before any Pending or permanent instance/finality contract.
    assert vars(cls)["__final__"] is True
    assert vars(cls)["method"].__final__ is True
    assert get_type_owner(cls) is None
    info = ConstructionInfoV1()
    assert get_construction(cls, ctypes.byref(info), ctypes.sizeof(info)) == 0
    assert (
        info.abi_version, info.struct_size, info.phase,
        info.permanent_contract_published, info.owner, info.root_construction,
    ) == (0, 0, 0, 0, None, None)
    function = vars(cls)["method"]
    witness = _assert_cpython_function_witness(
        function, module_witness,
    )
    assert witness["finalized"] is False
    assert cls().method("ordinary") == control().method("ordinary") == "ordinary"
    assert _soac_ext.strict_function_diagnostics(function)["original_code_entered"] is True
    child = type("ExternalChild", (cls,), {"method": lambda self, number: number})
    ordinary_child = type("OrdinaryChild", (control,), {"method": lambda self, number: number})
    assert child().method("overridden") == ordinary_child().method("overridden") == "overridden"
    child.method = lambda self, number: ("changed", number)
    ordinary_child.method = lambda self, number: ("changed", number)
    assert child().method(2) == ordinary_child().method(2) == ("changed", 2)
    assert get_type_owner(child) is None
""",
        Path(__file__),
        required_functions=("Item.method", "factory"),
        
    )
