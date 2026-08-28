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


