# Authenticated source and independent ordinary validation blocks.
# module:async_comprehension_semantics
# soac: module(strict_assign=true, checked_attr=true)

async def collect(outer, values, step, record):
    item = outer
    selected = None

    def read():
        return selected

    record('start', item, read)
    try:
        result = [(selected := await step(item)) async for item in values()]
        record('result', result)
        return item, selected, result, read
    except BaseException as error:
        record('error', error)
        raise
    finally:
        record('finally', item, read)
# module:ordinary_async_comprehension_semantics
async def collect(outer, values, step, record):
    item = outer
    selected = None

    def read():
        return selected

    record('start', item, read)
    try:
        result = [(selected := await step(item)) async for item in values()]
        record('result', result)
        return item, selected, result, read
    except BaseException as error:
        record('error', error)
        raise
    finally:
        record('finally', item, read)
# ok
# tests/test_strict_entry_runtime.py::test_async_comprehension_preserves_suspension_cancellation_and_outer_bindings
import sys
from soac import _soac_ext, import_hook

import ordinary_async_comprehension_semantics as ordinary

def _async_comprehension_semantic_observations(module):
    import asyncio
    import gc
    import weakref

    async def exercise(cancel):
        events, finalized, references = [], [], {}
        arrivals, permits = asyncio.Queue(), asyncio.Queue()
        outer = object()
        saved = {}

        class Payload:
            def __init__(self, label):
                self.label = label
                assert label not in references
                references[label] = weakref.ref(self)

            def __del__(self):
                finalized.append(self.label)

        class Values:
            def __init__(self):
                self.position = 0
                assert 'iterator' not in references
                references['iterator'] = weakref.ref(self)

            def __aiter__(self):
                events.append('aiter')
                return self

            async def __anext__(self):
                if self.position == 2:
                    events.append('stop')
                    raise StopAsyncIteration
                label = ('first', 'second')[self.position]
                self.position += 1
                events.append(('next', label))
                return Payload(label)

            def __del__(self):
                finalized.append('iterator')

        def values():
            events.append('values')
            return Values()

        async def step(value):
            label = value.label
            events.append(('wait', label))
            arrivals.put_nowait(label)
            try:
                await permits.get()
                assert value is references[label]()
                events.append(('resume', label))
                return value
            finally:
                events.append(('step-finally', value.label))

        def record(event, *arguments):
            if event == 'start':
                actual_outer, read = arguments
                assert actual_outer is outer and read() is None
                saved['read'] = read
                events.append('start')
            elif event == 'result':
                events.append(('result', tuple(value.label for value in arguments[0])))
            elif event == 'error':
                error, = arguments
                saved['error'] = error
                events.append(('error', type(error).__name__))
            elif event == 'finally':
                actual_outer, read = arguments
                assert actual_outer is outer, 'the comprehension target escaped its scope'
                assert read is saved['read'], 'the containing captured cell was replaced'
                current = read()
                events.append(('finally', None if current is None else current.label))
            else:
                raise AssertionError(event)

        task = asyncio.create_task(module.collect(outer, values, step, record))
        try:
            assert await asyncio.wait_for(arrivals.get(), 15) == 'first'
            assert not task.done(), 'the first await did not suspend'
            assert saved['read']() is None, 'walrus committed before its await completed'
            permits.put_nowait(None)
            assert await asyncio.wait_for(arrivals.get(), 15) == 'second'
            assert not task.done(), 'the second await did not suspend'
            assert saved['read']() is references['first']()
            if cancel:
                assert task.cancel('cancel semantic comprehension')
                try:
                    await task
                except asyncio.CancelledError as error:
                    assert saved.pop('error') is error
                    assert error.args == ('cancel semantic comprehension',)
                    error.__traceback__ = None
                else:
                    raise AssertionError('cancellation disappeared')
                assert task.cancelled()
            else:
                permits.put_nowait(None)
                result = await task
                assert result[0] is outer and result[3] is saved['read']
                assert result[2][0] is references['first']()
                assert result[2][1] is references['second']()
                assert result[1] is result[2][1] is result[3]()
                assert 'error' not in saved
        finally:
            if not task.done():
                task.cancel()
                try:
                    await task
                except asyncio.CancelledError:
                    pass
            saved.clear()
        assert task.done()
        return events, references, finalized

    observations = []
    # Normal completion after cancellation also proves that no suspended helper
    # activation or containing walrus cell leaked into the next source call.
    for cancel in (True, False):
        events, references, finalized = asyncio.run(exercise(cancel))
        prefix = [
            'start', 'values', 'aiter', ('next', 'first'), ('wait', 'first'),
            ('resume', 'first'), ('step-finally', 'first'),
            ('next', 'second'), ('wait', 'second'),
        ]
        suffix = (
            [('step-finally', 'second'), ('error', 'CancelledError'), ('finally', 'first')]
            if cancel else
            [('resume', 'second'), ('step-finally', 'second'), 'stop',
             ('result', ('first', 'second')), ('finally', 'second')]
        )
        assert events == prefix + suffix, events
        # All coroutine, result, closure and exception handles are now out of
        # scope. Only quiescent eventual cleanup is compared, not drop order.
        gc.collect()
        assert set(references) == {'first', 'second', 'iterator'}
        assert all(reference() is None for reference in references.values())
        assert sorted(finalized) == ['first', 'iterator', 'second']
        observations.append({
            'cancelled': cancel, 'events': events, 'finalized': sorted(finalized),
        })
    return observations

expected = _async_comprehension_semantic_observations(ordinary)
collect = module.collect
def assert_factory_witness():
    assert module.collect is collect
    if __dp_integration_soac__:
        import ctypes
        metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
        metadata.argtypes = [ctypes.py_object]
        metadata.restype = ctypes.c_void_p
        owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        owner.argtypes = [ctypes.py_object]
        owner.restype = ctypes.c_void_p
        assert metadata(collect), 'coroutine factory has no SOAC metadata'
        assert owner(collect), 'coroutine factory has no native strict owner'
        actual_entry = _soac_ext.strict_function_entry_kind(collect)
        assert actual_entry == 'generator_factory', actual_entry
assert_factory_witness()
actual = _async_comprehension_semantic_observations(module)
assert actual == expected, (actual, expected)
assert_factory_witness()
