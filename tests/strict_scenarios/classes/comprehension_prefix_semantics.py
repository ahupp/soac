# modes:soac,entry
# module:prefix_model
# soac: module(strict_assign=true, checked_attr=true)
def build(sink, prefix, source, later):
    class C:
        result = sink(prefix(), [lambda: item for item in source()], later())
    return C
# module:ordinary_prefix_model
def build(sink, prefix, source, later):
    class C:
        result = sink(prefix(), [lambda: item for item in source()], later())
    return C
# ok
# test_class_comprehension_prefix_preserves_evaluation_errors_and_cleanup
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('build',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
import prefix_model as actual
import ordinary_prefix_model as ordinary
from soac import _soac_ext
assert _soac_ext.strict_module_diagnostics(ordinary) is None

import gc
import sys
import weakref

def observe_class_prefix(build, outcome):
    events = []
    refs = {}
    marker = ValueError('comprehension-error')

    def handled():
        error = sys.exception()
        return None if error is None else str(error.args[0])

    def live():
        return tuple(bool(refs.get(name) and refs[name]() is not None)
                     for name in ('prefix', 'item', 'iterator'))

    class Value:
        def __init__(self, name):
            self.name = name
            refs[name] = weakref.ref(self)
            events.append(('made', name, handled()))
        def __del__(self):
            events.append(('drop', self.name, handled(), live()))

    class Iterator:
        def __init__(self):
            self.started = False
            refs['iterator'] = weakref.ref(self)
        def __iter__(self):
            events.append(('iter', handled()))
            return self
        def __next__(self):
            if not self.started:
                self.started = True
                return Value('item')
            if outcome == 'next-error':
                raise marker
            raise StopIteration
        def __del__(self):
            events.append(('drop', 'iterator', handled(), live()))

    def source():
        events.append(('source', handled()))
        if outcome == 'source-error':
            raise marker
        return Iterator()

    def prefix():
        return Value('prefix')

    def later():
        assert refs['prefix']() is not None and refs['item']() is not None
        events.append(('later', handled(), live()))
        return None

    def sink(first, callbacks, last):
        assert first is refs['prefix']() and callbacks[0]() is refs['item']()
        events.append(('sink', handled(), live(), callbacks[0]() is refs['item']()))
        return None

    try:
        raise KeyError('caller')
    except KeyError:
        try:
            build(sink, prefix, source, later)
        except ValueError as error:
            assert outcome != 'success' and error is marker
            events.append(('caught', handled(), live()))
            error.__traceback__ = None
            events.append(('traceback-cleared', handled(), live()))
        else:
            assert outcome == 'success'
            events.append(('returned', handled(), live()))
        events.append(('after-call', handled(), live()))
    gc.collect()
    events.append(('after-handler', handled(), live()))
    return events

def class_prefix_semantics(events):
    assert events[-1] == ('after-handler', None, (False, False, False)), events
    drops = sorted(event[1] for event in events if event[0] == 'drop')
    assert len(drops) == len(set(drops)), events
    return [
        event if event[0] == 'made' else event[:2]
        for event in events if event[0] != 'drop'
    ], drops

for outcome in ('success', 'source-error', 'next-error'):
    expected = class_prefix_semantics(observe_class_prefix(ordinary.build, outcome))
    observed = class_prefix_semantics(observe_class_prefix(actual.build, outcome))
    assert observed == expected, (outcome, observed, expected)
from tests._strict_integration import _plain_function_witness, _assert_cpython_function_witness
for _path in ('build',):
    _function = _plain_function_witness(module, _path)
    if __dp_integration_mode__ == 'cpython':
        _assert_cpython_function_witness(_function, _soac_ext.strict_module_diagnostics(module))
    else:
        assert _soac_ext.strict_function_entry_kind(_function) == expected_entry
