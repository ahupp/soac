"""Native generator protocol callbacks retain their caller's exception state."""

from pathlib import Path
from types import ModuleType

import pytest

from tests._integration import exec_integration_validation
from tests._strict_integration import create_strict_project

SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
def make_delegate(delegate, observe):
    def values():
        try:
            raise KeyError('source handler')
        except KeyError:
            try:
                return (yield from delegate)
            finally:
                observe('finally')
    return values()

def make_plain(observe):
    def values():
        try:
            raise KeyError('source handler')
        except KeyError:
            try:
                observe('start')
                value = yield 'ready'
                observe('sent', value)
                yield 'again'
            except GeneratorExit:
                observe('close')
                return 73
            except ValueError as error:
                observe('caught', error.args)
                yield 'caught'
            finally:
                observe('finally')
    return values()

def injection_plain(observe):
    try:
        yield 'ready'
    except BaseException as error:
        observe(error)

def injection_handled(observe):
    try:
        raise KeyError('source handler')
    except KeyError:
        try:
            yield 'ready'
        except BaseException as error:
            observe(error)

def injection_delegated(delegate, observe):
    try:
        yield from delegate
    except BaseException as error:
        observe(error)

def make_injection(mode, delegate, observe):
    if mode == 'handled_throw':
        return injection_handled(observe)
    if mode == 'delegate_error':
        return injection_delegated(delegate, observe)
    return injection_plain(observe)
"""

CASES = (
    "raw_throw_yields",
    "raw_throw_completes_delegate",
    "throw_lookup_error",
    "missing_throw_normalizes_after_lookup",
    "delegate_receives_invalid_exception_type",
    "close_delegate",
    "close_return_value",
    "invalid_throw_keeps_suspended",
    "created_throw",
    "created_invalid_throw",
    "reentrant_send",
)

VALIDATE = """
def validate(module):
    import sys
    import warnings

    events = []
    generator = None
    reentry_attempted = False

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    def observe(label, value=None):
        nonlocal reentry_attempted
        events.append((label, value, handled()))
        if CASE == 'reentrant_send' and label == 'start':
            assert not reentry_attempted, 'reentry executed the body before its guard'
            reentry_attempted = True
            try:
                generator.send(None)
            except ValueError as error:
                assert str(error) == 'generator already executing'
                events.append(('reentrant-rejected',))
            else:
                raise AssertionError('an executing generator accepted another resume')

    class Thrown(ValueError):
        def __init__(self, *args):
            observe('exception-constructor', args)
            super().__init__(*args)

    class Delegate:
        def __iter__(self):
            return self

        def __next__(self):
            observe('delegate-next')
            return 'delegated-ready'

        @property
        def throw(self):
            observe('delegate-lookup')
            if CASE == 'throw_lookup_error':
                raise LookupError('throw lookup')
            if CASE == 'missing_throw_normalizes_after_lookup':
                raise AttributeError('throw')

            def invoke(*args):
                first = args[0]
                shape = (
                    first.__name__ if isinstance(first, type) else first,
                    args[1:],
                )
                observe('delegate-throw', shape)
                if CASE == 'raw_throw_completes_delegate':
                    raise StopIteration(('delegate result', 42))
                return 'delegated-throw'

            return invoke

        def close(self):
            observe('delegate-close')

    def capture(call):
        try:
            result = call()
        except BaseException as error:
            events.append(('error', type(error).__name__, error.args))
        else:
            events.append(('result', result))

    try:
        raise RuntimeError('caller handler')
    except RuntimeError as caller:
        if CASE in ('close_return_value', 'invalid_throw_keeps_suspended', 'created_throw', 'created_invalid_throw', 'reentrant_send'):
            generator = module.make_plain(observe)
            if not CASE.startswith('created_'):
                assert next(generator) == 'ready'
        else:
            generator = module.make_delegate(Delegate(), observe)
            assert next(generator) == 'delegated-ready'
        assert sys.exception() is caller

        with warnings.catch_warnings():
            warnings.simplefilter('ignore', DeprecationWarning)
            if CASE.startswith('raw_throw_'):
                capture(lambda: generator.throw(Thrown, 'payload', None))
            elif CASE in ('throw_lookup_error', 'missing_throw_normalizes_after_lookup'):
                capture(lambda: generator.throw(Thrown))
            elif CASE == 'delegate_receives_invalid_exception_type':
                capture(lambda: generator.throw(17))
            elif CASE.startswith('close_'):
                capture(generator.close)
            elif CASE in ('invalid_throw_keeps_suspended', 'created_invalid_throw'):
                capture(lambda: generator.throw(17))
            elif CASE == 'created_throw':
                capture(lambda: generator.throw(Thrown))
            elif CASE == 'reentrant_send':
                capture(lambda: generator.send(11))
            else:
                raise AssertionError(CASE)
        assert sys.exception() is caller

        if CASE in ('throw_lookup_error', 'invalid_throw_keeps_suspended', 'created_invalid_throw'):
            capture(lambda: generator.send(None))
        assert sys.exception() is caller
        capture(generator.close)
        assert sys.exception() is caller

    # Keep a full native parity assertion, including lookup/construction order,
    # raw deprecated throw arguments, finally callbacks, and completion values.
    if EXPECTED is not None:
        assert events == EXPECTED, (CASE, events, EXPECTED)
    RESULTS.append(events)
"""


def ordinary_events(case, validation=VALIDATE):
    module = ModuleType("ordinary_generator_protocol")
    exec(  # noqa: S102 - the ordinary control is the literal source above.
        compile(
            SOURCE.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
            str(Path(__file__)),
            "exec",
            dont_inherit=True,
        ),
        vars(module),
    )
    module.CASE = case
    module.EXPECTED = None
    module.RESULTS = []
    exec_integration_validation(validation, module, Path(__file__), mode="stock")
    return module.RESULTS[0]


@pytest.mark.parametrize("case", CASES)
def test_generator_protocol_native_control(case):
    events = ordinary_events(case)
    assert events
    if case == "close_return_value":
        assert ("result", 73) in events
    if case == "raw_throw_completes_delegate":
        assert ("error", "StopIteration", (("delegate result", 42),)) in events
    if case == "created_throw":
        assert not any(event[0] in ("start", "finally") for event in events)
        assert ("error", "Thrown", ()) in events
    for event in events:
        if event[0] in (
            "delegate-lookup",
            "delegate-throw",
            "delegate-close",
            "exception-constructor",
        ):
            assert event[-1] == ("RuntimeError", ("caller handler",))


@pytest.fixture(scope="module")
def generator_protocol_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-generator-protocol"),
        {"generator_protocol.py": SOURCE},
        modules={"generator_protocol": "generator_protocol.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("case", CASES)
def test_generator_protocol_preserves_native_callbacks(
    generator_protocol_project, entry_interpreter, case
):
    expected = ordinary_events(case)
    validation = VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    CASE = {case!r}\n"
        f"    EXPECTED = {expected!r}\n"
        "    RESULTS = []",
        1,
    )
    generator_protocol_project.run_case(
        "generator_protocol",
        validation,
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("make_delegate", "make_plain"),
    )


def test_cpython_generator_close_preserves_completion_value_and_handler(tmp_path):
    case = "close_return_value"
    expected = ordinary_events(case)
    assert ("result", 73) in expected, expected
    project = create_strict_project(
        tmp_path,
        {"generator_protocol.py": SOURCE},
        modules={"generator_protocol": "generator_protocol.py"},
        backend="cpython",
    )
    validation = VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    CASE = {case!r}\n"
        f"    EXPECTED = {expected!r}\n"
        "    RESULTS = []",
        1,
    )
    project.run_case(
        "generator_protocol",
        validation,
        Path(__file__),
        required_functions=(
            "make_delegate", "make_plain", "make_injection",
            "injection_plain", "injection_handled", "injection_delegated",
        ),
        
        backend="cpython",
    )


INJECTION_CASES = ("plain_throw", "handled_throw", "created_throw", "delegate_error")

INJECTION_VALIDATE = """
def validate(module):
    import sys

    events = []
    injected = ValueError('injected')

    def observe(error):
        context = error.__context__
        events.append((
            type(error).__name__, error.args,
            None if context is None else (type(context).__name__, context.args),
            sys.exception() is error,
        ))

    class Delegate:
        def __iter__(self):
            return self

        def __next__(self):
            return 'ready'

        def throw(self, error):
            assert error is injected
            raise LookupError('delegate failure')

    generator = module.make_injection(CASE, Delegate(), observe)

    try:
        raise RuntimeError('caller handler')
    except RuntimeError as caller:
        if CASE != 'created_throw':
            assert next(generator) == 'ready'
        try:
            generator.throw(injected)
        except ValueError as error:
            assert CASE == 'created_throw'
            assert error is injected
            observe(error)
        except StopIteration as complete:
            assert CASE != 'created_throw'
            assert complete.value is None
        else:
            raise AssertionError('injected generator did not complete')
        assert sys.exception() is caller
        assert generator.close() is None

    if EXPECTED is not None:
        assert events == EXPECTED, (CASE, events, EXPECTED)
    RESULTS.append(events)
"""


@pytest.mark.parametrize("case", INJECTION_CASES)
def test_generator_injected_exception_native_control(case):
    events = ordinary_events(case, INJECTION_VALIDATE)
    expected_context = {
        "plain_throw": None,
        "handled_throw": ("KeyError", ("source handler",)),
        "created_throw": None,
        "delegate_error": ("RuntimeError", ("caller handler",)),
    }[case]
    assert len(events) == 1
    assert events[0][2] == expected_context
    assert events[0][3] is True


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("case", INJECTION_CASES)
def test_generator_injected_exception_uses_own_handled_item(
    generator_protocol_project, entry_interpreter, case
):
    expected = ordinary_events(case, INJECTION_VALIDATE)
    validation = INJECTION_VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    CASE = {case!r}\n"
        f"    EXPECTED = {expected!r}\n"
        "    RESULTS = []",
        1,
    )
    generator_protocol_project.run_case(
        "generator_protocol",
        validation,
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("make_injection",),
    )


DELIVERY_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
def delegated_delivery(delegate_factory, events):
    try:
        result = yield from delegate_factory()
        events.append(('after-yieldfrom', result))
        return ('returned', result)
    finally:
        events.append(('finally',))

def handled_delivery(events):
    try:
        yield 'ready'
    except ValueError:
        events.append(('handled',))
    events.append(('after-handler',))
    yield 'after'

def make_delivery(case, delegate_factory, events):
    if case == 'normalized_exception_lifetime':
        return handled_delivery(events)
    return delegated_delivery(delegate_factory, events)
"""

DELIVERY_EXPECTED = {
    "missing_throw_stop_iteration": (
        [("after-yieldfrom", 40), ("finally",)],
        ("stop", ("returned", 40)),
    ),
    "close_stop_iteration": (
        [("delegate-close",), ("after-yieldfrom", 41), ("finally",)],
        ("return", ("returned", 41)),
    ),
    "throw_exit_close_stop_iteration": (
        [("delegate-close",), ("after-yieldfrom", 41), ("finally",)],
        ("stop", ("returned", 41)),
    ),
    "delegate_throw_lifetime": (
        [
            ("delegate-throw",),
            ("delegate-finalized",),
            ("after-yieldfrom", 7),
            ("finally",),
        ],
        ("stop", ("returned", 7)),
    ),
    "delegate_close_lifetime": (
        [("delegate-close",), ("delegate-finalized",), ("finally",)],
        ("return", None),
    ),
    "normalized_exception_lifetime": (
        [("handled",), ("injection-finalized",), ("after-handler",)],
        ("yield", "after"),
    ),
}

DELIVERY_VALIDATE = """
def validate(module):
    import gc
    events = []

    class MissingThrow:
        def __iter__(self):
            return self

        def __next__(self):
            return 'ready'

    class CloseStops(MissingThrow):
        def close(self):
            events.append(('delegate-close',))
            raise StopIteration(41)

    class TemporaryDelegate(MissingThrow):
        def throw(self, *args):
            events.append(('delegate-throw',))
            raise StopIteration(7)

        def close(self):
            events.append(('delegate-close',))

        def __del__(self):
            events.append(('delegate-finalized',))

    class Injection(ValueError):
        def __del__(self):
            events.append(('injection-finalized',))

    if CASE in ('close_stop_iteration', 'throw_exit_close_stop_iteration'):
        delegate_factory = CloseStops
    elif CASE in ('delegate_throw_lifetime', 'delegate_close_lifetime'):
        delegate_factory = TemporaryDelegate
    else:
        delegate_factory = MissingThrow

    generator = module.make_delivery(CASE, delegate_factory, events)
    try:
        assert next(generator) == 'ready'
        if CASE in ('close_stop_iteration', 'delegate_close_lifetime'):
            outcome = ('return', generator.close())
        else:
            if CASE == 'missing_throw_stop_iteration':
                injected = StopIteration(40)
            elif CASE == 'throw_exit_close_stop_iteration':
                injected = GeneratorExit
            else:
                # Only the exception class is retained by this caller. The
                # normalized instance must be released by completed cleanup.
                injected = Injection
            try:
                value = generator.throw(injected)
            except StopIteration as complete:
                outcome = ('stop', complete.value)
            else:
                outcome = ('yield', value)
    finally:
        generator.close()
    del generator
    gc.collect()
    observed = (events.copy(), outcome)
    if EXPECTED is not None:
        implicit = {'delegate-finalized', 'injection-finalized'}
        def semantics(observation):
            recorded, result = observation
            return ([event for event in recorded if event[0] not in implicit], result)
        # Delegate calls, handler/finally order and completion values are exact.
        assert semantics(observed) == semantics(EXPECTED), (CASE, observed, EXPECTED)
        # Required finalizers must each run once, not on CPython's schedule.
        assert sorted(event for event in events if event[0] in implicit) == sorted(
            event for event in EXPECTED[0] if event[0] in implicit
        ), (CASE, observed, EXPECTED)
    RESULTS.append(observed)
"""


def ordinary_delivery_events(case):
    module = ModuleType("ordinary_generator_delivery")
    exec(  # noqa: S102 - the ordinary control is the separate literal source above.
        compile(
            DELIVERY_SOURCE.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
            str(Path(__file__)),
            "exec",
            dont_inherit=True,
        ),
        vars(module),
    )
    module.CASE = case
    module.EXPECTED = None
    module.RESULTS = []
    exec_integration_validation(
        DELIVERY_VALIDATE, module, Path(__file__), mode="stock"
    )
    return module.RESULTS[0]


@pytest.mark.parametrize("case", DELIVERY_EXPECTED)
def test_generator_delivery_native_control(case):
    assert ordinary_delivery_events(case) == DELIVERY_EXPECTED[case]


@pytest.fixture(scope="module")
def generator_delivery_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-generator-delivery"),
        {"generator_delivery.py": DELIVERY_SOURCE},
        modules={"generator_delivery": "generator_delivery.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("case", DELIVERY_EXPECTED)
def test_generator_delivery_preserves_completion_callbacks_and_cleanup(
    generator_delivery_project, entry_interpreter, case
):
    expected = ordinary_delivery_events(case)
    assert expected == DELIVERY_EXPECTED[case]
    validation = DELIVERY_VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    CASE = {case!r}\n"
        f"    EXPECTED = {expected!r}\n"
        "    RESULTS = []",
        1,
    )
    generator_delivery_project.run_case(
        "generator_delivery",
        validation,
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("make_delivery",),
    )


TERMINAL_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
def terminal_values(mode, make_payload):
    payload = make_payload()
    yield 'ready'
    if payload is None:
        raise AssertionError('source local was not initialized')
    if mode == 'raise':
        raise LookupError('source failure')
    return 71

def make_terminal(mode, make_payload):
    return terminal_values(mode, make_payload)
"""

TERMINAL_CASES = ("return", "raise", "close")
TERMINAL_EXPECTED = [
    (
        ("value", False),
        ("value", "GEN_CLOSED"),
        ("value", False),
        ("value", True),
        ("value", None),
        ("error", "StopIteration", (), False),
        ("error", "RuntimeError", ("reentrant throw",), True),
        ("value", None),
    )
]

TERMINAL_VALIDATE = """
def validate(module):
    import gc
    events = []
    holder = []
    injected = RuntimeError('reentrant throw')

    def observe(call):
        try:
            result = call()
        except BaseException as error:
            return ('error', type(error).__name__, error.args, error is injected)
        return ('value', result)

    class Payload:
        def __del__(self):
            generator = holder[0]
            observed = (
                observe(lambda: generator.gi_running),
                observe(lambda: generator.gi_state),
                observe(lambda: generator.gi_suspended),
            )
            if EXPECTED is None:
                # Frame inspection belongs only to the ordinary control.
                observed += (observe(lambda: generator.gi_frame is None),)
            events.append(observed + (
                observe(lambda: generator.gi_yieldfrom),
                observe(lambda: generator.send(None)),
                observe(lambda: generator.throw(injected)),
                observe(generator.close),
            ))

    generator = module.make_terminal(CASE, Payload)
    holder.append(generator)
    assert next(generator) == 'ready'
    assert events == [], 'the local must remain live while its generator is suspended'
    if CASE == 'close':
        assert generator.close() is None
    elif CASE == 'raise':
        try:
            next(generator)
        except LookupError as error:
            assert error.args == ('source failure',)
        else:
            raise AssertionError('source failure was lost')
    else:
        try:
            next(generator)
        except StopIteration as complete:
            assert complete.value == 71
        else:
            raise AssertionError('generator did not return')
    gc.collect()
    assert len(events) == 1, ('terminal local was not released', CASE, events)
    # Completion is a semantic boundary, independent of when implicit release
    # caused the reentrant finalizer to run.
    assert generator.gi_running is False
    assert generator.gi_state == 'GEN_CLOSED'
    assert generator.gi_suspended is False
    if EXPECTED is None:
        assert generator.gi_frame is None
    assert generator.gi_yieldfrom is None
    if EXPECTED is not None:
        observed = events[0]
        expected_state = EXPECTED[0][:3] + EXPECTED[0][4:]
        if observed[0] == ('value', False):
            assert observed == expected_state, (CASE, events, expected_state)
        else:
            # A safe SOAC cleanup point may precede the terminal-state flag.
            # Such reentry must refuse execution, not resume a retiring body.
            assert observed[0] == ('value', True), observed
            assert observed[1] == ('value', 'GEN_RUNNING'), observed
            assert observed[2] == ('value', False), observed
            assert observed[3] == ('value', None), observed
            for result in observed[4:]:
                assert result[0] == 'error' and result[1] == 'ValueError', observed
    RESULTS.append(events)
    holder.clear()
"""


def ordinary_terminal_events(case):
    module = ModuleType("ordinary_generator_terminal")
    exec(  # noqa: S102 - the ordinary control is the separate literal source above.
        compile(
            TERMINAL_SOURCE.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
            str(Path(__file__)),
            "exec",
            dont_inherit=True,
        ),
        vars(module),
    )
    module.CASE = case
    module.EXPECTED = None
    module.RESULTS = []
    exec_integration_validation(
        TERMINAL_VALIDATE, module, Path(__file__), mode="stock"
    )
    return module.RESULTS[0]


@pytest.mark.parametrize("case", TERMINAL_CASES)
def test_generator_terminal_finalizer_native_control(case):
    assert ordinary_terminal_events(case) == TERMINAL_EXPECTED


@pytest.fixture(scope="module")
def generator_terminal_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-generator-terminal"),
        {"generator_terminal.py": TERMINAL_SOURCE},
        modules={"generator_terminal": "generator_terminal.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("case", TERMINAL_CASES)
def test_generator_terminal_cleanup_is_reentry_safe_and_finishes_closed(
    generator_terminal_project, entry_interpreter, case
):
    expected = ordinary_terminal_events(case)
    assert expected == TERMINAL_EXPECTED
    validation = TERMINAL_VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    CASE = {case!r}\n"
        f"    EXPECTED = {expected!r}\n"
        "    RESULTS = []",
        1,
    )
    generator_terminal_project.run_case(
        "generator_terminal",
        validation,
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("make_terminal",),
    )


SUSPENDED_NATIVE_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
async def source_coroutine(delegate, observe):
    observe('enter')
    try:
        result = await delegate
        observe('after-await')
        return result
    finally:
        observe('finally')

async def source_async_generator(delegate, observe):
    observe('enter')
    try:
        result = await delegate
        observe('after-await')
        yield result
    finally:
        observe('finally')

def make_suspended(kind, delegate, observe):
    if kind == 'coroutine':
        return source_coroutine(delegate, observe)
    return source_async_generator(delegate, observe)
"""

SUSPENDED_NATIVE_VALIDATE = """
def validate(module):
    import types

    events = []
    holder = []

    def observe(label):
        value = holder[0]
        events.append((label, value.cr_running if KIND == 'coroutine' else value.ag_running))

    class Delegate:
        def __await__(self):
            yield 'waiting'
            return 'source-value'

    value = module.make_suspended(KIND, Delegate(), observe)
    holder.append(value)

    def finish(awaitable):
        iterator = awaitable.__await__()
        try:
            next(iterator)
        except StopIteration as complete:
            return complete.value
        else:
            raise AssertionError('cleanup unexpectedly suspended')

    def live_frame():
        frame = value.cr_frame if KIND == 'coroutine' else value.ag_frame
        assert isinstance(frame, types.FrameType), 'a live frame must not be fabricated as None'
        expected = module.source_coroutine if KIND == 'coroutine' else module.source_async_generator
        assert frame.f_code is expected.__code__
        assert frame.f_generator is value

    try:
        if CASE == 'identity':
            expected = types.CoroutineType if KIND == 'coroutine' else types.AsyncGeneratorType
            assert type(value) is expected, (type(value), expected)
            return

        if KIND == 'coroutine':
            assert value.cr_running is False
            if CASE == 'state':
                if NATIVE:
                    live_frame()
                assert value.send(None) == 'waiting'
                assert events == [('enter', True)], events
                assert value.cr_running is False
                assert value.cr_suspended is True
                assert value.cr_await is not None
                if NATIVE:
                    live_frame()
                try:
                    value.send(None)
                except StopIteration as complete:
                    assert complete.value == 'source-value'
                else:
                    raise AssertionError('coroutine did not complete')
                assert events == [('enter', True), ('after-await', True), ('finally', True)], events
                if NATIVE:
                    assert value.cr_frame is None
                assert value.cr_await is None
            else:
                async def await_same():
                    return await value
                first, second = await_same(), await_same()
                try:
                    assert first.send(None) == 'waiting'
                    try:
                        second.send(None)
                    except RuntimeError as error:
                        assert str(error) == 'coroutine is being awaited already'
                    else:
                        raise AssertionError('concurrent await was accepted')
                finally:
                    first.close()
                    second.close()
        else:
            assert value.ag_running is False
            if CASE == 'state' and NATIVE:
                live_frame()
            first = value.__anext__()
            try:
                assert first.send(None) == 'waiting'
                if CASE == 'state':
                    assert events == [('enter', True)], events
                    # The native ASend operation owns running state across an await.
                    assert value.ag_running is True
                    assert value.ag_await is not None
                    if NATIVE:
                        live_frame()
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
                    else:
                        raise AssertionError('async yield was not delivered')
                    assert events == [('enter', True), ('after-await', True)], events
                    assert value.ag_running is False
                    assert value.ag_await is None
                else:
                    second = value.__anext__()
                    try:
                        try:
                            second.send(None)
                        except RuntimeError as error:
                            assert str(error) == 'anext(): asynchronous generator is already running'
                        else:
                            raise AssertionError('concurrent async-generator operation was accepted')
                    finally:
                        second.close()
                    # Finish the first operation before closing the generator.
                    try:
                        first.send(None)
                    except StopIteration as complete:
                        assert complete.value == 'source-value'
            finally:
                first.close()
            assert finish(value.aclose()) is None
            assert value.ag_running is False
            if NATIVE:
                assert value.ag_frame is None
            assert value.ag_await is None
    finally:
        if KIND == 'coroutine':
            value.close()
        else:
            finish(value.aclose())
"""


def suspended_native_validation(kind, case, *, native):
    return SUSPENDED_NATIVE_VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    KIND = {kind!r}\n"
        f"    CASE = {case!r}\n"
        f"    NATIVE = {native!r}",
        1,
    )


@pytest.mark.parametrize("kind", ["coroutine", "async_generator"])
@pytest.mark.parametrize("case", ["identity", "state", "concurrent"])
def test_suspended_native_identity_and_state_control(kind, case):
    module = ModuleType("ordinary_suspended_native")
    exec(  # noqa: S102 - ordinary control uses the same source without strict opt-in.
        compile(
            SUSPENDED_NATIVE_SOURCE.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
            str(Path(__file__)),
            "exec",
            dont_inherit=True,
        ),
        vars(module),
    )
    exec_integration_validation(
        suspended_native_validation(kind, case, native=True),
        module,
        Path(__file__),
        mode="stock",
    )


@pytest.fixture(scope="module")
def suspended_native_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-suspended-native"),
        {"suspended_native.py": SUSPENDED_NATIVE_SOURCE},
        modules={"suspended_native": "suspended_native.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("kind", ["coroutine", "async_generator"])
@pytest.mark.parametrize("case", ["identity", "state", "concurrent"])
def test_suspended_objects_preserve_native_identity_and_state(
    suspended_native_project, entry_interpreter, kind, case
):
    suspended_native_project.run_case(
        "suspended_native",
        suspended_native_validation(kind, case, native=False),
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("make_suspended",),
    )


# Keep the original normal and suspended programs. Traceback-local retention
# is an ordinary CPython control; SOAC preserves their explicit calls, errors,
# replacement/deletion semantics and eventual cleanup without a source frame.
TRACEBACK_LIFETIME_BODY = """    payload = make_payload('first')
    if mode == 'escape':
        raise ValueError('source failure')
    try:
        raise LookupError('retained source failure')
    except LookupError as error:
        save(error)
    if mode == 'replace':
        payload = make_payload('second')
    elif mode == 'delete':
        del payload
"""

TRACEBACK_LIFETIME_SOURCE = (
    "# soac: module(strict_assign=true, checked_attr=true)\n"
    "def lifetime_function(mode, make_payload, save, delegate):\n"
    + TRACEBACK_LIFETIME_BODY
    + "    return 41\n\n"
    "def lifetime_generator(mode, make_payload, save, delegate):\n"
    "    yield 'ready'\n"
    + TRACEBACK_LIFETIME_BODY
    + "    return 41\n\n"
    "async def lifetime_coroutine(mode, make_payload, save, delegate):\n"
    "    await delegate\n"
    + TRACEBACK_LIFETIME_BODY
    + "    return 41\n\n"
    "async def lifetime_async_generator(mode, make_payload, save, delegate):\n"
    "    yield 'ready'\n"
    + TRACEBACK_LIFETIME_BODY
    + "\n"
    "def make_lifetime(kind, mode, make_payload, save, delegate):\n"
    "    if kind == 'function':\n"
    "        return lifetime_function(mode, make_payload, save, delegate)\n"
    "    if kind == 'generator':\n"
    "        return lifetime_generator(mode, make_payload, save, delegate)\n"
    "    if kind == 'coroutine':\n"
    "        return lifetime_coroutine(mode, make_payload, save, delegate)\n"
    "    return lifetime_async_generator(mode, make_payload, save, delegate)\n"
)

TRACEBACK_LIFETIME_VALIDATE = """
def validate(module):
    import gc
    import weakref
    events = []
    errors = []
    callbacks = []
    references = []

    class Payload:
        def __init__(self, label):
            self.label = label
            callbacks.append(('make', label))
            references.append(weakref.ref(self))

        def __del__(self):
            events.append(self.label)

    class Delegate:
        def __await__(self):
            yield 'ready'

    def save(error):
        assert type(error) is LookupError and error.args == ('retained source failure',)
        callbacks.append(('save',))
        errors.append(error)

    def finish(operation, completion, expected):
        try:
            operation.send(None)
        except completion as complete:
            if completion is StopIteration:
                assert complete.value == expected
        else:
            raise AssertionError('source operation did not finish')

    try:
        value = module.make_lifetime(KIND, CASE, Payload, save, Delegate())
        if KIND == 'function':
            assert value == 41
        elif KIND == 'generator':
            assert next(value) == 'ready'
            finish(value, StopIteration, 41)
        elif KIND == 'coroutine':
            assert value.send(None) == 'ready'
            finish(value, StopIteration, 41)
        else:
            finish(value.__anext__(), StopIteration, 'ready')
            finish(value.__anext__(), StopAsyncIteration, None)
    except ValueError as error:
        assert CASE == 'escape'
        assert error.args == ('source failure',)
        errors.append(error)

    try:
        assert len(errors) == 1, ('source exception was not retained', KIND, CASE)
        expected_callbacks = [('make', 'first')]
        if CASE != 'escape':
            expected_callbacks.append(('save',))
        if CASE == 'replace':
            expected_callbacks.append(('make', 'second'))
        assert callbacks == expected_callbacks, (KIND, CASE, callbacks)
        expected = ['first'] if CASE in ('replace', 'delete') else []
        gc.collect()
        if not __dp_integration_soac__:
            assert events == expected, ('ordinary traceback lost source owners', KIND, CASE, events)
        # Retire any ordinary callback traceback before the quiescent check.
        # Retained SOAC need not root source locals through the exception.
        errors[0].__traceback__ = None
        expected = ['first', 'second'] if CASE == 'replace' else ['first']
        gc.collect()
        assert sorted(events) == sorted(expected), ('source values did not release once', KIND, CASE, events)
        assert all(reference() is None for reference in references)
    finally:
        for error in errors:
            error.__traceback__ = None
        errors.clear()
"""


def traceback_lifetime_validation(kind, case):
    return TRACEBACK_LIFETIME_VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    KIND = {kind!r}\n"
        f"    CASE = {case!r}",
        1,
    )


@pytest.mark.parametrize("kind", ["function", "generator", "coroutine", "async_generator"])
@pytest.mark.parametrize("case", ["escape", "retain", "replace", "delete"])
def test_source_traceback_lifetime_native_control(kind, case):
    module = ModuleType("ordinary_source_traceback_lifetime")
    exec(  # noqa: S102 - ordinary control uses the identical literal source.
        compile(
            TRACEBACK_LIFETIME_SOURCE.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
            str(Path(__file__)),
            "exec",
            dont_inherit=True,
        ),
        vars(module),
    )
    exec_integration_validation(
        traceback_lifetime_validation(kind, case), module, Path(__file__), mode="stock"
    )


@pytest.fixture(scope="module")
def traceback_lifetime_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-source-traceback-lifetime"),
        {"traceback_lifetime.py": TRACEBACK_LIFETIME_SOURCE},
        modules={"traceback_lifetime": "traceback_lifetime.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("kind", ["function", "generator", "coroutine", "async_generator"])
@pytest.mark.parametrize("case", ["escape", "retain", "replace", "delete"])
def test_source_exceptions_preserve_callbacks_and_cleanup_without_frame_retention(
    traceback_lifetime_project, entry_interpreter, kind, case
):
    traceback_lifetime_project.run_case(
        "traceback_lifetime",
        traceback_lifetime_validation(kind, case),
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("make_lifetime",),
    )



# Fresh delegation never reuses the previous native resume packet.
_INITIAL_DELEGATION_SOURCE = """
# soac: module(strict_assign=true, checked_attr=true)

def after_send(delegate):
    marker = yield "first"
    result = yield from delegate
    return marker, result

def after_caught_throw(delegate):
    try:
        yield "first"
    except ValueError:
        return (yield from delegate)

async def after_await(first, second):
    first_result = await first
    second_result = await second
    return first_result, second_result

async def after_caught_await(first, second):
    try:
        await first
    except ValueError:
        return await second

def make_after_send(delegate):
    return after_send(delegate)

def make_after_caught_throw(delegate):
    return after_caught_throw(delegate)

def make_after_await(first, second):
    return after_await(first, second)

def make_after_caught_await(first, second):
    return after_caught_await(first, second)
"""

_INITIAL_DELEGATION_VALIDATION = """
import ctypes
import pytest
import ordinary_initial_entry
from soac import _soac_ext

def delegate():
    yield "delegated"
    return "delegate-result"

def exercise(module, case):
    class Awaitable:
        def __init__(self, token):
            self.token = token

        def __await__(self):
            result = yield self.token
            return result

    if case == "after_send":
        generator = module.make_after_send(delegate())
        assert next(generator) == "first"
        assert generator.send("sent-value") == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == ("sent-value", "delegate-result")
    elif case == "after_caught_throw":
        generator = module.make_after_caught_throw(delegate())
        assert next(generator) == "first"
        assert generator.throw(ValueError("injected")) == "delegated"
        with pytest.raises(StopIteration) as done:
            next(generator)
        assert done.value.value == "delegate-result"
    elif case == "after_await":
        coroutine = module.make_after_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.send("first-result") == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == ("first-result", "second-result")
    elif case == "after_caught_await":
        coroutine = module.make_after_caught_await(Awaitable("first"), Awaitable("second"))
        assert coroutine.send(None) == "first"
        assert coroutine.throw(ValueError("injected")) == "second"
        with pytest.raises(StopIteration) as done:
            coroutine.send("second-result")
        assert done.value.value == "second-result"
    else:
        raise AssertionError(case)

def validate_module(module):
    case = __CASE__
    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    factory_name = "make_" + case
    assert not owner(getattr(ordinary_initial_entry, factory_name))
    assert _soac_ext.strict_module_diagnostics(ordinary_initial_entry) is None
    exercise(ordinary_initial_entry, case)
    assert owner(getattr(module, factory_name))
    # run_case already requires the exact requested native/entry execution kind.
    exercise(module, case)
"""

@pytest.fixture(scope="module")
def initial_delegation_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-initial-delegation"),
        {
            "initial_entry.py": _INITIAL_DELEGATION_SOURCE,
            "ordinary_initial_entry.py": _INITIAL_DELEGATION_SOURCE.replace(
                "# soac: module(strict_assign=true, checked_attr=true)\n", "", 1
            ),
        },
        modules={"initial_entry": "initial_entry.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
@pytest.mark.parametrize(
    "case", ["after_send", "after_caught_throw", "after_await", "after_caught_await"]
)
def test_fresh_delegation_does_not_reuse_the_previous_resume_packet(
    initial_delegation_project, entry_interpreter, case
):
    initial_delegation_project.run_case(
        "initial_entry",
        _INITIAL_DELEGATION_VALIDATION.replace("__CASE__", repr(case)),
        Path(__file__),
        required_functions=("make_" + case,),
        entry_interpreter=entry_interpreter,
    )


CYCLIC_TRACEBACK_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
def make(save, payload_factory, connect):
    async def source():
        payload = payload_factory()
        try:
            raise ValueError('retained before suspension')
        except ValueError as error:
            save(error)
        yield 'ready'
    value = source()
    connect(source, value)
    return value
"""

CYCLIC_TRACEBACK_VALIDATE = """
def validate(module):
    import gc
    import sys
    import weakref

    saved = []
    events = []
    function_refs = []
    source_codes = []

    class Payload:
        def __init__(self):
            events.append('payload-created')

        def __del__(self):
            events.append('payload-finalized')

    def connect(function, value):
        function.cycle = value
        function_refs.append(weakref.ref(function))
        source_codes.append(function.__code__)

    def finalize(value):
        # A supported hook may decline to close or resurrect this generator.
        events.append('async-finalizer-hook')

    original_hooks = sys.get_asyncgen_hooks()
    old_enabled = gc.isenabled()
    gc.disable()
    value = None
    operation = None
    try:
        sys.set_asyncgen_hooks(firstiter=None, finalizer=finalize)
        value = module.make(saved.append, Payload, connect)
        value_ref = weakref.ref(value)
        operation = value.__anext__()
        try:
            operation.send(None)
        except StopIteration as completed:
            assert completed.value == 'ready'
        else:
            raise AssertionError('initial async yield did not complete the ASend')
        operation = None
        assert len(saved) == 1
        assert type(saved[0]) is ValueError and saved[0].args == ('retained before suspension',)
        if not __dp_integration_soac__:
            assert events == ['payload-created']
            # Preserve the ordinary CPython frame control only. SOAC errors
            # need no reconstructed source frame or matching local retention.
            traceback = saved[0].__traceback__
            try:
                while traceback is not None and traceback.tb_frame.f_code is not source_codes[0]:
                    traceback = traceback.tb_next
                assert traceback is not None, 'ordinary error omitted its original source frame'
            finally:
                del traceback

        value = None
        if __dp_integration_soac__:
            # SOAC cleanup is checked after releasing the retained traceback,
            # without requiring a particular source-frame retention policy.
            saved[0].__traceback__ = None
            saved.clear()
        gc.collect()
        assert events[0] == 'payload-created', events
        assert sorted(events[1:]) == ['async-finalizer-hook', 'payload-finalized'], (
            'cyclic async cleanup must run each required finalizer once', events
        )
        assert value_ref() is None, 'quiescent cleanup retained the generator'
        assert function_refs[0]() is None, 'quiescent cleanup retained the function'

        if not __dp_integration_soac__:
            expected = events.copy()
            saved[0].__traceback__ = None
            saved.clear()
            gc.collect()
            assert events == expected, 'clearing the traceback repeated a GC finalizer'
    finally:
        operation = None
        value = None
        for error in saved:
            error.__traceback__ = None
        saved.clear()
        gc.collect()
        sys.set_asyncgen_hooks(*original_hooks)
        if old_enabled:
            gc.enable()
"""


def test_cyclic_async_traceback_native_gc_control():
    module = ModuleType("ordinary_cyclic_async_traceback")
    exec(  # noqa: S102 - ordinary control uses the identical literal source.
        compile(
            CYCLIC_TRACEBACK_SOURCE.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
            str(Path(__file__)),
            "exec",
            dont_inherit=True,
        ),
        vars(module),
    )
    exec_integration_validation(
        CYCLIC_TRACEBACK_VALIDATE, module, Path(__file__), mode="stock"
    )


@pytest.fixture(scope="module")
def cyclic_traceback_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-cyclic-async-traceback"),
        {"cyclic_traceback.py": CYCLIC_TRACEBACK_SOURCE},
        modules={"cyclic_traceback": "cyclic_traceback.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_cyclic_async_traceback_does_not_root_suspended_source_state(
    cyclic_traceback_project, entry_interpreter
):
    cyclic_traceback_project.run_case(
        "cyclic_traceback",
        CYCLIC_TRACEBACK_VALIDATE,
        Path(__file__),
        required_functions=("make",),
        entry_interpreter=entry_interpreter,
    )


SUSPENDED_OPERAND_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
def suspended_call(make, consume, later):
    local = make('local')
    consume(make('operand'), (yield 'ready'), later())
    return 73

def make_suspended_call(make, consume, later):
    return suspended_call(make, consume, later)
"""

SUSPENDED_OPERAND_CASES = (
    "resume", "later_error", "throw", "close", "gc", "retained_traceback",
)

SUSPENDED_OPERAND_VALIDATE = """
def validate(module):
    import gc
    import sys
    import types
    import weakref

    events = []
    refs = {}
    retained = []
    failure = ValueError('later operand failed')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __init__(self, label):
            self.label = label

        def __del__(self):
            events.append(('drop', self.label, handled()))

    def make(label):
        value = Payload(label)
        refs[label] = weakref.ref(value)
        events.append(('make', label))
        return value

    def later():
        assert refs['operand']() is not None, 'suspended operand was released before use'
        events.append(('later', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        if CASE in ('later_error', 'retained_traceback'):
            raise failure
        return 'tail'

    def consume(value, sent, tail):
        assert value is refs['operand'](), 'resumed call changed operand identity'
        events.append(('call', value.label, sent, tail, sys.getrefcount(value)))

    generator = module.make_suspended_call(make, consume, later)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_call.__code__
    assert generator.gi_code is source_code
    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        # Only the suspended expression stack owns this value. The source
        # local has its independent activation lifetime.
        assert refs['operand']() is not None, 'yield lost its unevaluated call operand'
        events.append(('suspended', sys.getrefcount(refs['operand']()), refs['local']() is not None))
        try:
            if CASE in ('resume', 'later_error', 'retained_traceback'):
                generator.send('sent')
            elif CASE == 'throw':
                generator.throw(failure)
            elif CASE == 'close':
                generator.close()
            else:
                del generator
                gc.collect()
        except StopIteration as complete:
            assert CASE == 'resume'
            assert complete.value == 73
            events.append(('returned', complete.value))
        except ValueError as error:
            assert CASE in ('later_error', 'retained_traceback', 'throw')
            assert error is failure
            if CASE == 'throw' and EXPECTED is None:
                source_lines = []
                traceback = error.__traceback__
                while traceback is not None:
                    if traceback.tb_frame.f_code is source_code:
                        source_lines.append(traceback.tb_lineno)
                    traceback = traceback.tb_next
                # Keep only line numbers here: retaining the traceback/frame
                # in the observer would mask the clear-time lifetime check.
                assert source_lines == [source_code.co_firstlineno + 2], (
                    'throw must attach exactly once at the suspended yield',
                    source_lines,
                )
            gc.collect()
            events.append(('raised', refs['operand']() is None, refs['local']() is not None))
            if EXPECTED is None:
                assert refs['operand']() is None, 'ordinary traceback retained an evaluated operand'
            if CASE == 'retained_traceback':
                retained.append(error)
            else:
                error.__traceback__ = None
        assert sys.exception() is caller
        if CASE != 'gc':
            del generator
        if retained:
            if EXPECTED is None:
                assert refs['local']() is not None, 'ordinary traceback must keep source locals'
            retained.pop().__traceback__ = None
        gc.collect()
        assert refs['operand']() is None
        assert refs['local']() is None
        events.append(('complete', handled()))
    assert sum(event[:2] == ('drop', 'operand') for event in events) == 1
    assert sum(event[:2] == ('drop', 'local') for event in events) == 1
    drops = [event[1] for event in events if event[0] == 'drop']
    assert sorted(drops) == ['local', 'operand'], events
    if EXPECTED is not None:
        def semantics(recorded):
            result = []
            for event in recorded:
                if event[0] == 'drop':
                    continue
                if event[0] in {'later', 'suspended', 'raised'}:
                    # Source-local lifetime and traceback retention are not
                    # SOAC observations; explicit operation order still is.
                    result.append((event[0],))
                elif event[0] == 'call':
                    result.append(event[:-1])
                else:
                    result.append(event)
            return result
        assert semantics(events) == semantics(EXPECTED), (CASE, events, EXPECTED)
    else:
        # Retain the stock-only schedule observation separately from SOAC.
        assert drops == ['operand', 'local'], events
    RESULTS.append(events)
"""


def ordinary_suspended_operand_events(case):
    module = ModuleType("ordinary_suspended_operands")
    exec(  # noqa: S102 - native control uses the same source without strict opt-in.
        compile(
            SUSPENDED_OPERAND_SOURCE.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
            str(Path(__file__)), "exec", dont_inherit=True,
        ),
        vars(module),
    )
    module.CASE = case
    module.EXPECTED = None
    module.RESULTS = []
    exec_integration_validation(
        SUSPENDED_OPERAND_VALIDATE, module, Path(__file__), mode="stock",
    )
    return module.RESULTS[0]


@pytest.mark.parametrize("case", SUSPENDED_OPERAND_CASES)
def test_suspended_expression_operand_native_lifetime_control(case):
    assert ordinary_suspended_operand_events(case)


@pytest.fixture(scope="module")
def suspended_operand_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-suspended-operands"),
        {"suspended_operands.py": SUSPENDED_OPERAND_SOURCE},
        modules={"suspended_operands": "suspended_operands.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("case", SUSPENDED_OPERAND_CASES)
def test_suspended_expression_operands_preserve_semantics_and_required_cleanup(
    suspended_operand_project, entry_interpreter, case,
):
    expected = ordinary_suspended_operand_events(case)
    validation = SUSPENDED_OPERAND_VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    CASE = {case!r}\n"
        f"    EXPECTED = {expected!r}\n"
        "    RESULTS = []",
        1,
    )
    suspended_operand_project.run_case(
        "suspended_operands", validation, Path(__file__),
        required_functions=("make_suspended_call",),
        entry_interpreter=entry_interpreter,
    )


DELEGATED_THROW_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
def suspended_delegation(make, values):
    local = make()
    return (yield from values)

def make_suspended_delegation(make, values):
    return suspended_delegation(make, values)
"""

DELEGATED_THROW_CASES = ("missing_throw", "delegate_raises", "delegate_returns")

DELEGATED_THROW_VALIDATE = """
def validate(module):
    import gc
    import sys
    import types
    import weakref

    drops = []
    refs = []
    failure = ValueError('delegated throw')

    def handled():
        error = sys.exception()
        return None if error is None else (type(error).__name__, error.args)

    class Payload:
        def __del__(self):
            drops.append(handled())

    def make():
        value = Payload()
        refs.append(weakref.ref(value))
        return value

    def raising_delegate():
        yield 'ready'

    def returning_delegate():
        try:
            yield 'ready'
        except ValueError:
            return 73

    if CASE == 'missing_throw':
        delegate = iter(('ready',))
    elif CASE == 'delegate_raises':
        delegate = raising_delegate()
    else:
        delegate = returning_delegate()
    generator = module.make_suspended_delegation(make, delegate)
    assert type(generator) is types.GeneratorType
    source_code = module.suspended_delegation.__code__
    assert generator.gi_code is source_code

    def source_lines(error):
        lines = []
        traceback = error.__traceback__
        while traceback is not None:
            if traceback.tb_frame.f_code is source_code:
                lines.append(traceback.tb_lineno - source_code.co_firstlineno)
            traceback = traceback.tb_next
        return lines

    try:
        raise KeyError('caller handler')
    except KeyError as caller:
        assert next(generator) == 'ready'
        if EXPECTED is None:
            assert refs[0]() is not None
        assert generator.gi_yieldfrom is delegate
        try:
            generator.throw(failure)
        except ValueError as error:
            assert CASE != 'delegate_returns'
            assert error is failure
            if EXPECTED is None:
                lines = source_lines(error)
                assert lines == [2], ('ordinary delegated error needs one original source TB', lines)
                assert refs[0]() is not None, 'ordinary traceback must retain the source local'
                assert drops == [], drops
            error.__traceback__ = None
            gc.collect()
            if EXPECTED is None:
                assert refs[0]() is None, 'the last traceback must release its source local'
                assert len(drops) == 1, drops
                assert drops == [('ValueError', ('delegated throw',))], drops
            result = ('raised', type(error).__name__, error.args)
        except StopIteration as complete:
            assert CASE == 'delegate_returns'
            assert complete.value == 73
            if EXPECTED is None:
                lines = source_lines(failure)
                assert lines == [], ('consumed ordinary delegation error must not gain a source TB', lines)
            result = ('returned', complete.value)
            # A retained delegate traceback can own its ordinary f_back.
            # This control distinguishes source TB events, not that separate
            # callback-parent lifetime. Release the actual traceback explicitly.
            failure.__traceback__ = None
        else:
            raise AssertionError('throw must terminate this source activation')
        assert sys.exception() is caller
        del generator
        del delegate
        gc.collect()
        assert refs[0]() is None
        assert len(drops) == 1, drops
    if EXPECTED is not None:
        assert result == EXPECTED, (CASE, result, EXPECTED)
    RESULTS.append(result)
"""


def ordinary_delegated_throw_result(case):
    module = ModuleType("ordinary_delegated_throw")
    exec(  # noqa: S102 - same original source without strict opt-in.
        compile(
            DELEGATED_THROW_SOURCE.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
            str(Path(__file__)), "exec", dont_inherit=True,
        ),
        vars(module),
    )
    module.CASE = case
    module.EXPECTED = None
    module.RESULTS = []
    exec_integration_validation(
        DELEGATED_THROW_VALIDATE, module, Path(__file__), mode="stock",
    )
    return module.RESULTS[0]


@pytest.mark.parametrize("case", DELEGATED_THROW_CASES)
def test_delegated_throw_source_traceback_native_control(case):
    assert ordinary_delegated_throw_result(case)


@pytest.fixture(scope="module")
def delegated_throw_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-delegated-throw"),
        {"delegated_throw.py": DELEGATED_THROW_SOURCE},
        modules={"delegated_throw": "delegated_throw.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.parametrize("case", DELEGATED_THROW_CASES)
def test_delegated_throw_preserves_results_exceptions_and_cleanup(
    delegated_throw_project, entry_interpreter, case,
):
    expected = ordinary_delegated_throw_result(case)
    validation = DELEGATED_THROW_VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    CASE = {case!r}\n"
        f"    EXPECTED = {expected!r}\n"
        "    RESULTS = []",
        1,
    )
    delegated_throw_project.run_case(
        "delegated_throw", validation, Path(__file__),
        required_functions=("make_suspended_delegation",),
        entry_interpreter=entry_interpreter,
    )


ITERATOR_ACTIVATION_SOURCE = """# soac: module(strict_assign=true, checked_attr=true)
def values(make, observe):
    observe('source-enter', None)
    for value in make():
        yield value
    observe('source-exit', None)

def relay(make, observe):
    for value in values(make, observe):
        observe('relay', value)
        yield value

def consume(make, observe):
    total = 0
    for value in values(make, observe):
        observe('local', value)
        total += value
    return total

def make_relay(make, observe):
    return relay(make, observe)
"""

ITERATOR_ACTIVATION_VALIDATE = """
def validate(module):
    import types

    events = []

    def observe(label, value):
        events.append((label, value))

    class Values:
        def __init__(self):
            self.index = 0

        def __iter__(self):
            observe('iter', None)
            return self

        def __next__(self):
            index = self.index
            observe('next', index)
            self.index += 1
            if index == 2:
                raise StopIteration
            return (4, 7)[index]

    def make():
        observe('acquire', None)
        return Values()

    assert module.consume(make, observe) == 11
    local_events = list(events)
    events.clear()
    generator = module.make_relay(make, observe)
    assert type(generator) is types.GeneratorType
    assert generator.gi_code is module.relay.__code__
    assert events == [], 'generator creation must not acquire the source iterable'
    assert next(generator) == 4
    assert next(generator) == 7
    try:
        next(generator)
    except StopIteration as complete:
        assert complete.value is None
    else:
        raise AssertionError('relay must exhaust after two values')

    for observed, consumer in ((local_events, 'local'), (events, 'relay')):
        assert observed == [
            ('source-enter', None), ('acquire', None), ('iter', None),
            ('next', 0), (consumer, 4),
            ('next', 1), (consumer, 7),
            ('next', 2), ('source-exit', None),
        ], observed
    result = (local_events, events)
    if EXPECTED is not None:
        assert result == EXPECTED, (result, EXPECTED)
    RESULTS.append(result)
"""


def ordinary_iterator_activation_events():
    module = ModuleType("ordinary_iterator_activation")
    exec(  # noqa: S102 - native control uses the same original source.
        compile(
            ITERATOR_ACTIVATION_SOURCE.removeprefix("# soac: module(strict_assign=true, checked_attr=true)\n"),
            str(Path(__file__)), "exec", dont_inherit=True,
        ),
        vars(module),
    )
    module.EXPECTED = None
    module.RESULTS = []
    exec_integration_validation(
        ITERATOR_ACTIVATION_VALIDATE, module, Path(__file__), mode="stock",
    )
    return module.RESULTS[0]


def test_iterator_activation_native_acquisition_control():
    assert ordinary_iterator_activation_events()


@pytest.fixture(scope="module")
def iterator_activation_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-iterator-activation"),
        {"iterator_activation.py": ITERATOR_ACTIVATION_SOURCE},
        modules={"iterator_activation": "iterator_activation.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_iterator_activation_survives_consuming_and_borrowed_reads(
    iterator_activation_project, tmp_path, entry_interpreter,
):
    from tests._strict_integration import _VALIDATION_PRELUDE, StrictValidationCase

    expected = ordinary_iterator_activation_events()
    validation = ITERATOR_ACTIVATION_VALIDATE.replace(
        "def validate(module):",
        "def validate(module):\n"
        f"    EXPECTED = {expected!r}\n"
        "    RESULTS = []",
        1,
    )
    # Reuse run_case's exact native source/generation/entry witnesses while
    # replaying all modes against one explicit profile directory.
    program = _VALIDATION_PRELUDE + iterator_activation_project._validation_program(
        "iterator_activation",
        StrictValidationCase(validation, Path(__file__), ("consume", "make_relay")),
        entry_interpreter=entry_interpreter,
    )
    work = tmp_path / "iterator-activation-profile"
    for mode in ("none", "profile", "apply"):
        iterator_activation_project.run(
            program,
            entry_interpreter=entry_interpreter,
            opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(work)},
        )
    assert (work / "profile.bin").is_file()
