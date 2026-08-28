# modes:soac,entry
# Authenticated source and independent ordinary validation blocks.
# module:expanded_arguments
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
# ok
# tests/test_strict_call_context.py::test_strict_expanded_argument_callback_order_and_cleanup
import sys
from soac import _soac_ext, import_hook

import expanded_arguments as actual
assert _soac_ext.strict_function_entry_kind(actual.prefix) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
assert _soac_ext.strict_function_entry_kind(actual.suspended_prefix) == 'generator_factory'
assert _soac_ext.strict_function_entry_kind(actual.suspended_singleton) == 'generator_factory'
ordinary = {}
exec("\n\n\ndef prefix(callee, source, predicate, value, first):\n    return callee()(*source(), value() if predicate() else None)\n\ndef singleton(callee, source, predicate, value, first):\n    return callee()(*source(), tail=value() if predicate() else None)\n\ndef mapping(callee, source, predicate, value, first):\n    return callee()(**source(), tail=value() if predicate() else None)\n\ndef grouped_duplicate(callee, source, predicate, value, first):\n    return callee()(**source(), duplicate=first(), tail=value() if predicate() else None)\n\ndef suspended_prefix(callee, source, predicate, value, first):\n    return callee()(*source(), (yield 'ready'))\n\ndef suspended_singleton(callee, source, predicate, value, first):\n    return callee()(*source(), tail=(yield 'ready'))\n", ordinary)

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

cases = ('prefix', 'singleton', 'mapping', 'grouped_duplicate', 'singleton_failure', 'suspended_prefix', 'suspended_singleton_failure')
def semantic_events(result):
    return [event for event in result['events'] if event[0] not in ('drop', 'drop-source')]
def released_resources(result):
    return sorted(event[:2] if event[0] == 'drop' else event[:1]
                  for event in result['events'] if event[0] in ('drop', 'drop-source'))
failures = []
for case in cases:
    expected = observe_expanded_argument_case(ordinary, case)
    observed = observe_expanded_argument_case(actual.__dict__, case)
    if (observed['outcome'] != expected['outcome']
            or semantic_events(observed) != semantic_events(expected)
            or released_resources(observed) != released_resources(expected)
            or observed['raw_alive'] or any(observed['payloads_alive'])):
        failures.append((case, expected, observed))
assert not failures, failures
