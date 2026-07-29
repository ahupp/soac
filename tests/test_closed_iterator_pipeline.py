import types

import pytest

from soac import runtime
from tests._integration import soac_module, stock_module


class _BoolRaisesStopIteration:
    def __bool__(self):
        raise StopIteration("truth")


class _RecordingIterable:
    def __init__(self, events, values):
        self.events = events
        self.values = values

    def __iter__(self):
        self.events.append("iter")
        return iter(self.values)


class _SetKey:
    def __init__(self, value, events, fail_mode):
        self.value = value
        self.events = events
        self.fail_mode = fail_mode

    def __hash__(self):
        self.events.append(("hash", self.value))
        if self.fail_mode == "hash" and self.value == 2:
            raise StopIteration("hash")
        return 0

    def __eq__(self, other):
        self.events.append(("eq", self.value, other.value))
        if self.fail_mode == "eq" and other.value == 2:
            raise StopIteration("eq")
        return self.value == other.value


def test_map_from_iter_eagerly_gets_iterator_and_stops_on_callback_stop_iteration():
    events = []
    iterable = _RecordingIterable(events, [0, 1, 2, 3])

    def convert(value):
        events.append(("map", value))
        if value == 2:
            raise StopIteration("map")
        return value + 10

    mapped = runtime.map_from_iter(convert, iterable)
    assert events == ["iter"]
    assert list(mapped) == [10, 11]
    assert events == ["iter", ("map", 0), ("map", 1), ("map", 2)]


@pytest.mark.parametrize("function", [None, lambda _value: _BoolRaisesStopIteration()])
def test_filter_from_iter_stops_on_truth_stop_iteration(function):
    values = [_BoolRaisesStopIteration()] if function is None else [1]
    assert list(runtime.filter_from_iter(function, values)) == []


def test_transformed_closed_map_filter_pipeline_matches_stock(tmp_path):
    source = """
def collect(source, convert, keep, count):
    return tuple(
        filter(
            keep,
            map(convert, (source(index) for index in range(count))),
        )
    )

def collect_list(convert, count):
    return list(map(convert, (index for index in range(count))))

def collect_set(convert, count):
    return set(map(convert, (index for index in range(count))))

def values(count):
    for index in range(count):
        yield index

def collect_named(convert, count):
    return list(map(convert, values(count)))

def escaping_named(count):
    return values(count)

def escaping(convert, count):
    return map(convert, (index for index in range(count)))
"""

    def run(module):
        events = []

        def source(value):
            events.append(("source", value))
            return value

        def convert(value):
            events.append(("map", value))
            return value * 3 + 1

        def keep(value):
            events.append(("filter", value))
            return value % 2 == 0

        result = module.collect(source, convert, keep, 5)
        return result, events

    with stock_module(tmp_path, "closed_pipeline_stock", source) as stock:
        stock_result = run(stock)
    with soac_module(tmp_path, "closed_pipeline_soac", source) as transformed:
        assert run(transformed) == stock_result
        list_result = transformed.collect_list(lambda value: value + 1, 4)
        set_result = transformed.collect_set(lambda value: value & 1, 4)
        assert type(list_result) is list
        assert list_result == [1, 2, 3, 4]
        assert type(set_result) is set
        assert set_result == {0, 1}
        # Source-backed named generators are deliberately excluded from the
        # generalized fusion pass, even when a list/map chain consumes them.
        assert transformed.collect_named(lambda value: value + 1, 4) == [1, 2, 3, 4]
        escaped_named = transformed.escaping_named(2)
        assert type(escaped_named) is types.GeneratorType
        assert list(escaped_named) == [0, 1]
        assert type(transformed.escaping(lambda value: value, 2)) is map


def test_transformed_map_filter_stop_iteration_is_partial_completion(tmp_path):
    source = """
def mapped(callback):
    return list(map(callback, (value for value in range(5))))

def filtered(predicate):
    return tuple(filter(predicate, (value for value in range(5))))
"""

    with soac_module(tmp_path, "closed_pipeline_stop_iteration", source) as module:
        def callback(value):
            if value == 3:
                raise StopIteration("map")
            return value + 10

        def predicate(value):
            if value == 3:
                raise StopIteration("filter")
            return value % 2 == 0

        assert module.mapped(callback) == [10, 11, 12]
        assert module.filtered(predicate) == (0, 2)


def test_transformed_callback_factory_stop_iteration_propagates(tmp_path):
    source = """
def collect(make_callback):
    return list(map(make_callback(), (value for value in range(4))))
"""

    def run(module):
        events = []

        def make_callback():
            events.append("factory")
            raise StopIteration("factory")

        with pytest.raises(StopIteration, match="factory"):
            module.collect(make_callback)
        return events

    with stock_module(tmp_path, "closed_pipeline_factory_stock", source) as stock:
        expected = run(stock)
    with soac_module(tmp_path, "closed_pipeline_factory_soac", source) as transformed:
        assert run(transformed) == expected == ["factory"]


def test_transformed_multiple_pipeline_stop_iteration_is_isolated(tmp_path):
    source = """
def collect(first, second):
    left = list(map(first, (value for value in range(4))))
    right = tuple(map(second, (value for value in range(3))))
    return left, right
"""

    def run(module):
        events = []

        def first(value):
            events.append(("first", value))
            if value == 2:
                raise StopIteration("first")
            return value + 10

        def second(value):
            events.append(("second", value))
            return value + 20

        return module.collect(first, second), events

    with stock_module(tmp_path, "closed_pipeline_roots_stock", source) as stock:
        expected = run(stock)
    with soac_module(tmp_path, "closed_pipeline_roots_soac", source) as transformed:
        assert run(transformed) == expected


def test_transformed_map_filter_other_exceptions_propagate_once(tmp_path):
    source = """
def mapped(callback):
    return list(map(callback, (value for value in range(5))))

def filtered(predicate):
    return list(filter(predicate, (value for value in range(5))))
"""
    events = []

    def fail(value):
        events.append(value)
        if value == 2:
            raise ValueError("boom")
        return value

    with soac_module(tmp_path, "closed_pipeline_errors", source) as module:
        with pytest.raises(ValueError, match="boom"):
            module.mapped(fail)
        assert events == [0, 1, 2]
        events.clear()
        with pytest.raises(ValueError, match="boom"):
            module.filtered(fail)
        assert events == [0, 1, 2]


def test_transformed_filter_truth_stop_iteration_matches_stock(tmp_path):
    source = """
def filtered_none(values):
    return list(filter(None, (value for value in values)))

def filtered(predicate, values):
    return tuple(filter(predicate, (value for value in values)))
"""

    values = [1, _BoolRaisesStopIteration(), 2]

    def run(module):
        events = []

        def predicate(value):
            events.append(value)
            if value == 1:
                return _BoolRaisesStopIteration()
            return True

        none_result = module.filtered_none(values)
        predicate_result = module.filtered(predicate, [0, 1, 2])
        return none_result, predicate_result, events

    with stock_module(tmp_path, "closed_pipeline_truth_stock", source) as stock:
        expected = run(stock)
    with soac_module(tmp_path, "closed_pipeline_truth_soac", source) as transformed:
        assert run(transformed) == expected


def test_transformed_source_stop_iteration_still_uses_pep_479(tmp_path):
    source = """
def collect(source):
    return list(map(lambda value: value, (source(value) for value in range(4))))
"""

    def source_value(value):
        if value == 2:
            raise StopIteration("source")
        return value

    with stock_module(tmp_path, "closed_pipeline_pep479_stock", source) as stock:
        with pytest.raises(RuntimeError, match="generator raised StopIteration") as stock_error:
            stock.collect(source_value)
    with soac_module(tmp_path, "closed_pipeline_pep479_soac", source) as transformed:
        with pytest.raises(
            RuntimeError, match="generator raised StopIteration"
        ) as transformed_error:
            transformed.collect(source_value)

    for error in (stock_error.value, transformed_error.value):
        assert type(error) is RuntimeError
        assert str(error) == "generator raised StopIteration"
        assert type(error.__cause__) is StopIteration
        assert error.__cause__.args == ("source",)
        assert str(error.__cause__) == "source"
        assert error.__context__ is error.__cause__
        assert error.__suppress_context__ is True


def test_named_generator_pipeline_preserves_code_and_default_mutation(tmp_path):
    source = """
def values(limit=2):
    for value in range(limit):
        yield value

def replacement(limit=2):
    for value in range(limit):
        yield value + 100

def collect():
    return list(map(lambda value: value, values()))
"""

    def run(module):
        before = module.collect()
        module.values.__defaults__ = (3,)
        after_defaults = module.collect()
        module.values.__code__ = module.replacement.__code__
        after_code = module.collect()
        return before, after_defaults, after_code

    with stock_module(tmp_path, "closed_pipeline_mutation_stock", source) as stock:
        expected = run(stock)
    with soac_module(tmp_path, "closed_pipeline_mutation_soac", source) as transformed:
        assert run(transformed) == expected


def test_transformed_pep_479_generator_and_coroutine_boundaries_match_stock(tmp_path):
    source = """
def leaking_generator():
    if False:
        yield None
    raise StopIteration("generator-source")

async def leaking_coroutine():
    raise StopIteration("coroutine-source")

def returned_generator():
    if False:
        yield None
    return 17

def async_stop_generator():
    if False:
        yield None
    raise StopAsyncIteration("generator-async-stop")

async def async_stop_coroutine():
    raise StopAsyncIteration("coroutine-async-stop")

def throw_target():
    yield "ready"
"""

    def exception_record(call):
        try:
            call()
        except BaseException as error:
            cause = error.__cause__
            return (
                type(error),
                str(error),
                error.args,
                type(cause) if cause is not None else None,
                cause.args if cause is not None else None,
                error.__context__ is cause and cause is not None,
                error.__suppress_context__,
            )
        raise AssertionError("call did not raise")

    def run(module):
        returned = module.returned_generator()
        try:
            next(returned)
        except StopIteration as error:
            return_value = error.value
        else:
            raise AssertionError("returning generator did not finish")

        thrown = module.throw_target()
        assert next(thrown) == "ready"
        return (
            exception_record(lambda: next(module.leaking_generator())),
            exception_record(lambda: module.leaking_coroutine().send(None)),
            exception_record(lambda: next(module.async_stop_generator())),
            exception_record(lambda: module.async_stop_coroutine().send(None)),
            exception_record(lambda: thrown.throw(StopIteration("throw-source"))),
            return_value,
        )

    with stock_module(tmp_path, "pep479_boundaries_stock", source) as stock:
        expected = run(stock)
    with soac_module(tmp_path, "pep479_boundaries_soac", source) as transformed:
        assert run(transformed) == expected


@pytest.mark.parametrize("fail_mode", ["hash", "eq"])
def test_transformed_set_insertion_stop_iteration_is_not_exhaustion(tmp_path, fail_mode):
    source = """
def collect(factory):
    return set(filter(lambda value: True, map(factory, (value for value in range(4)))))
"""

    def run(module):
        events = []

        def factory(value):
            events.append(("map", value))
            return _SetKey(value, events, fail_mode)

        with pytest.raises(StopIteration, match=fail_mode):
            module.collect(factory)
        return events

    with stock_module(tmp_path, f"closed_pipeline_set_{fail_mode}_stock", source) as stock:
        expected_events = run(stock)
    with soac_module(
        tmp_path, f"closed_pipeline_set_{fail_mode}_soac", source
    ) as transformed:
        assert run(transformed) == expected_events


def test_transformed_long_pipeline_preserves_eager_and_per_item_order(tmp_path):
    source = """
def collect(make_second, make_predicate, make_first, values):
    return tuple(
        map(
            make_second(),
            filter(
                make_predicate(),
                map(make_first(), (value for value in values)),
            ),
        )
    )
"""

    def run(module):
        events = []

        def make_first():
            events.append("make_first")

            def first(value):
                events.append(("first", value))
                return value + 1

            return first

        def make_predicate():
            events.append("make_predicate")

            def predicate(value):
                events.append(("predicate", value))
                return value % 2 == 0

            return predicate

        def make_second():
            events.append("make_second")

            def second(value):
                events.append(("second", value))
                return value * 10

            return second

        values = _RecordingIterable(events, range(4))
        result = module.collect(make_second, make_predicate, make_first, values)
        return result, events

    with stock_module(tmp_path, "closed_pipeline_long_stock", source) as stock:
        expected = run(stock)
    assert expected[1][:4] == [
        "make_second",
        "make_predicate",
        "make_first",
        "iter",
    ]
    with soac_module(tmp_path, "closed_pipeline_long_soac", source) as transformed:
        actual = run(transformed)
        assert type(actual[0]) is tuple
        assert actual == expected
