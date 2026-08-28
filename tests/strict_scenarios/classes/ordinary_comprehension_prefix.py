# modes:cpython
# module:ordinary_prefix_model
def build(sink, prefix, source, later):
    class C:
        result = sink(prefix(), [lambda: item for item in source()], later())
    return C
# ok
# test_class_comprehension_prefix_cleanup_native_control[success]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
outcome = 'success'
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

events = observe_class_prefix(module.build, outcome)
assert events[-1] == ("after-handler", None, (False, False, False))
if outcome == "next-error":
    drops = [event for event in events if event[:1] == ("drop",)]
    item = next(event for event in drops if event[1] == "item")
    assert item[3][0], (
        "native cell restoration happens while the older prefix is owned"
    )
    assert [event[1] for event in drops].index("item") < [
        event[1] for event in drops
    ].index("prefix")
# ok
# test_class_comprehension_prefix_cleanup_native_control[source-error]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
outcome = 'source-error'
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

events = observe_class_prefix(module.build, outcome)
assert events[-1] == ("after-handler", None, (False, False, False))
if outcome == "next-error":
    drops = [event for event in events if event[:1] == ("drop",)]
    item = next(event for event in drops if event[1] == "item")
    assert item[3][0], (
        "native cell restoration happens while the older prefix is owned"
    )
    assert [event[1] for event in drops].index("item") < [
        event[1] for event in drops
    ].index("prefix")
# ok
# test_class_comprehension_prefix_cleanup_native_control[next-error]
import sys
import pytest
from soac import _soac_ext
expected_entry = ('original_code' if __dp_integration_mode__ == 'cpython' else 'entry_interpreter' if __dp_integration_entry__ else 'checked_native')
outcome = 'next-error'
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

events = observe_class_prefix(module.build, outcome)
assert events[-1] == ("after-handler", None, (False, False, False))
if outcome == "next-error":
    drops = [event for event in events if event[:1] == ("drop",)]
    item = next(event for event in drops if event[1] == "item")
    assert item[3][0], (
        "native cell restoration happens while the older prefix is owned"
    )
    assert [event[1] for event in drops].index("item") < [
        event[1] for event in drops
    ].index("prefix")
