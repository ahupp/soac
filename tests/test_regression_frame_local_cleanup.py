"""Stock cleanup controls plus SOAC source semantics and quiescent cleanup."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._integration import exec_integration_validation, stock_module
from tests._strict_integration import (
    assert_strict_source_rejected,
    create_strict_project,
)

SOURCE = r'''
events = []


class Watch:
    def __init__(self, name):
        self.name = name

    def __del__(self):
        events.append(f"del:{self.name}")


def reset():
    events.clear()


def branch_local(flag):
    x = Watch("branch")
    if flag:
        events.append("then")
    else:
        events.append("else")
    events.append("after")
    return list(events)


def rebind_local():
    x = Watch("old")
    events.append("before")
    x = Watch("new")
    events.append("after")
    return list(events)


def delete_local():
    x = Watch("deleted")
    events.append("before")
    del x
    events.append("after")
    return list(events)


def raise_local():
    x = Watch("raised")
    events.append("before")
    raise ValueError("boom")


def caught_exception_local():
    x = Watch("caught")
    try:
        1 / 0
    except ZeroDivisionError:
        events.append("handler")
        return list(events)
    return ["missing"]
'''


_SOAC_CLEANUP_VALIDATION = r"""
import gc
from collections import Counter

def explicit_events(events):
    return [event for event in events if not event.startswith('del:')]

def assert_cleanup(events, explicit, released):
    # Calls have returned and retained exception tracebacks have been cleared.
    # Require every release once without prescribing its implicit micro-order.
    gc.collect()
    assert explicit_events(events) == explicit, events
    actual = Counter(event for event in events if event.startswith('del:'))
    assert actual == Counter(released), events
"""


@pytest.fixture(scope="module")
def cleanup_project(tmp_path_factory):
    path = "frame_local_cleanup.py"
    opted_in, _ = strict_opt_in(SOURCE.encode(), path)
    return create_strict_project(
        tmp_path_factory.mktemp("strict-frame-local-cleanup"),
        {path: opted_in.decode()},
        modules={"frame_local_cleanup": path},
    )


@pytest.fixture(params=("stock", "compiled", "entry"))
def run_cleanup_case(request, tmp_path):
    def run(validation, required_functions, *, strict_validation):
        program = "import pytest\n" + validation
        if request.param == "stock":
            with stock_module(tmp_path, "frame_local_cleanup", SOURCE) as module:
                exec_integration_validation(program, module, Path(__file__), mode="stock")
            return
        # The native control does not depend on checker success. Each strict
        # case runs in a fresh process with the actual published authority.
        program = _SOAC_CLEANUP_VALIDATION + strict_validation
        project = request.getfixturevalue("cleanup_project")
        project.run_case(
            "frame_local_cleanup", program, Path(__file__),
            entry_interpreter=request.param == "entry",
            required_functions=required_functions,
        )
    return run


@pytest.mark.integration
def test_branch_transition_preserves_events_and_releases_local(run_cleanup_case):
    run_cleanup_case(
        '''
def validate(module):
    module.reset()
    result = module.branch_local(False)
    assert result == ['else', 'after']
    assert module.events == ['else', 'after', 'del:branch']
''',
        required_functions=('branch_local',),
        strict_validation='''
def validate(module):
    module.reset()
    result = module.branch_local(False)
    assert type(result) is list
    assert explicit_events(result) == ['else', 'after'], result
    assert_cleanup(module.events, ['else', 'after'], ['del:branch'])
''',
    )


@pytest.mark.integration
def test_rebind_and_del_preserve_events_and_release_locals(run_cleanup_case):
    run_cleanup_case(
        '''
def validate(module):
    module.reset()
    rebind_result = module.rebind_local()
    assert rebind_result == ['before', 'del:old', 'after']
    assert module.events == ['before', 'del:old', 'after', 'del:new']
    module.reset()
    delete_result = module.delete_local()
    assert delete_result == ['before', 'del:deleted', 'after']
    assert module.events == ['before', 'del:deleted', 'after']
''',
        required_functions=('rebind_local', 'delete_local'),
        strict_validation='''
def validate(module):
    module.reset()
    rebind_result = module.rebind_local()
    assert type(rebind_result) is list
    assert explicit_events(rebind_result) == ['before', 'after'], rebind_result
    assert_cleanup(module.events, ['before', 'after'], ['del:old', 'del:new'])
    module.reset()
    delete_result = module.delete_local()
    assert type(delete_result) is list
    assert explicit_events(delete_result) == ['before', 'after'], delete_result
    assert_cleanup(module.events, ['before', 'after'], ['del:deleted'])
''',
    )


@pytest.mark.integration
def test_exception_exit_preserves_error_and_releases_local(run_cleanup_case):
    run_cleanup_case(
        '''
def validate(module):
    module.reset()
    with pytest.raises(ValueError):
        module.raise_local()
    module.events.append('caught')
    assert module.events == ['before', 'del:raised', 'caught']
''',
        required_functions=('raise_local',),
        strict_validation='''
def validate(module):
    module.reset()
    try:
        module.raise_local()
    except ValueError as error:
        assert type(error) is ValueError and error.args == ('boom',)
        error.__traceback__ = None
    else:
        raise AssertionError('source exception was lost')
    module.events.append('caught')
    assert_cleanup(module.events, ['before', 'caught'], ['del:raised'])
''',
    )


@pytest.mark.integration
def test_exception_dispatch_preserves_handler_and_releases_local(run_cleanup_case):
    run_cleanup_case(
        '''
def validate(module):
    module.reset()
    result = module.caught_exception_local()
    assert result == ['handler']
    assert module.events == ['handler', 'del:caught']
''',
        required_functions=('caught_exception_local',),
        strict_validation='''
def validate(module):
    module.reset()
    result = module.caught_exception_local()
    assert type(result) is list
    assert explicit_events(result) == ['handler'], result
    assert_cleanup(module.events, ['handler'], ['del:caught'])
''',
    )


class _ExactImplicitFinalizerOrderDifference(AssertionError):
    """An excluded order comparison, after semantic and safety checks pass."""


@pytest.mark.integration
@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
@pytest.mark.xfail(
    raises=_ExactImplicitFinalizerOrderDifference,
    strict=False,
    reason=(
        "2026-08-24 (PDT): SOAC does not promise CPython's exact timing or "
        "relative order of implicit finalizers"
    ),
)
def test_rebind_and_del_exact_implicit_finalizer_order(
    cleanup_project, entry_interpreter
):
    completed = cleanup_project.run_case(
        "frame_local_cleanup",
        _SOAC_CLEANUP_VALIDATION + r'''
import json

def validate(module):
    observed = {}
    for name, released in (
        ('rebind_local', ['del:old', 'del:new']),
        ('delete_local', ['del:deleted']),
    ):
        module.reset()
        result = getattr(module, name)()
        assert type(result) is list
        assert explicit_events(result) == ['before', 'after'], result
        assert_cleanup(module.events, ['before', 'after'], released)
        observed[name] = [result, list(module.events)]
    print(json.dumps(observed))
''',
        Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=('rebind_local', 'delete_local'),
    )
    # Import, authentication, entry witnesses, subprocess success, explicit
    # callbacks and complete cleanup must succeed before an order-only xfail.
    observed = json.loads(completed.stdout.strip().splitlines()[-1])
    assert type(observed) is dict
    assert set(observed) == {'rebind_local', 'delete_local'}
    for snapshots in observed.values():
        assert type(snapshots) is list and len(snapshots) == 2
        for snapshot in snapshots:
            assert type(snapshot) is list
            assert all(type(event) is str for event in snapshot)
    expected = {
        'rebind_local': [
            ['before', 'del:old', 'after'],
            ['before', 'del:old', 'after', 'del:new'],
        ],
        'delete_local': [
            ['before', 'del:deleted', 'after'],
            ['before', 'del:deleted', 'after'],
        ],
    }
    if observed != expected:
        raise _ExactImplicitFinalizerOrderDifference((observed, expected))



_EXPLICIT_DEL_UNBOUND_SOURCE = r"""
def delete_binding(value, observe):
    alias = value
    observe('before', alias)
    del value
    observe('after', alias)
    try:
        value
    except UnboundLocalError:
        observe('unbound-load', alias)
    else:
        raise AssertionError('deleted local remained readable')
    try:
        del value
    except UnboundLocalError:
        observe('unbound-delete', alias)
    else:
        raise AssertionError('deleted local remained deletable')
    return alias
"""


_EXPLICIT_DEL_UNBOUND_VALIDATION = r"""
def validate(module):
    import gc
    import weakref

    events = []
    released = []
    class Value:
        def __del__(self):
            released.append('released')

    value = Value()
    reference = weakref.ref(value)
    def observe(label, actual):
        assert actual is value and reference() is value
        assert not released
        events.append(label)

    result = module.delete_binding(value, observe)
    assert result is value and reference() is value
    assert events == ['before', 'after', 'unbound-load', 'unbound-delete']
    assert not released
    del result, value
    gc.collect()
    assert reference() is None
    assert released == ['released']
"""


@pytest.mark.integration
def test_explicit_del_unbound_errors_original_control(tmp_path):
    # Preserve the entire original source and validator, including both
    # UnboundLocalError checks, independently of strict checker admission.
    with stock_module(tmp_path, "delete_local_binding", _EXPLICIT_DEL_UNBOUND_SOURCE) as module:
        exec_integration_validation(
            _EXPLICIT_DEL_UNBOUND_VALIDATION, module, Path(__file__), mode="stock",
        )


@pytest.mark.integration
def test_explicit_del_unbound_errors_are_rejected_by_strict_checker(tmp_path):
    name = "delete_local_binding"
    path = f"{name}.py"
    opted_in, _ = strict_opt_in(_EXPLICIT_DEL_UNBOUND_SOURCE.encode(), path)
    # The original deliberate read of a deleted name is a blocking checker
    # diagnostic, not a retained-runtime failure or permission to suppress it.
    diagnostics = assert_strict_source_rejected(
        tmp_path / "strict-delete-unbound",
        opted_in.decode(),
        module_name=name,
        diagnostic="unresolved-reference",
    )
    assert "Name `value` used when not defined" in diagnostics, diagnostics


_EXPLICIT_DEL_TRACEBACK_SOURCE = r"""
def delete_binding(value, observe):
    alias = value
    observe('before', alias)
    del value
    observe('after', alias)
    return alias
"""


_EXPLICIT_DEL_TRACEBACK_VALIDATION = r"""
def validate(module):
    import gc
    import weakref

    events = []
    released = []
    class Value:
        def __del__(self):
            released.append('released')

    value = Value()
    reference = weakref.ref(value)
    stop_at = None
    marker = None
    def observe(label, actual):
        assert actual is value and reference() is value
        assert not released
        events.append(label)
        if label == stop_at:
            raise marker

    for stop_at in ('before', 'after'):
        marker = RuntimeError(stop_at)
        try:
            module.delete_binding(value, observe)
        except RuntimeError as error:
            assert error is marker, 'the source callback exception must propagate unchanged'
            traceback = error.__traceback__
            while traceback is not None and traceback.tb_frame.f_code is not module.delete_binding.__code__:
                traceback = traceback.tb_next
            assert traceback is not None, 'callback failure lost its original source frame'
            snapshot = dict(traceback.tb_frame.f_locals)
            assert snapshot['alias'] is value
            if stop_at == 'before':
                assert snapshot['value'] is value
            else:
                assert 'value' not in snapshot, 'del must remove the binding before the next callback'
            # No traceback, frame proxy or locals copy may retain this value
            # at the final quiescent cleanup check.
            del snapshot, traceback
            error.__traceback__ = None
        else:
            raise AssertionError('the explicit callback exception was lost')
        del marker
        assert reference() is value and not released

    stop_at = None
    result = module.delete_binding(value, observe)
    assert result is value and reference() is value
    assert events == ['before', 'before', 'after', 'before', 'after']
    assert not released
    del result, value, observe
    gc.collect()
    assert reference() is None
    assert released == ['released']
"""


_EXPLICIT_DEL_ALIAS_VALIDATION = r"""
def validate(module):
    import gc
    import weakref

    events = []
    released = []
    class Value:
        def __del__(self):
            released.append('released')

    value = Value()
    reference = weakref.ref(value)
    stop_at = None
    marker = None
    def observe(label, actual):
        assert actual is value and reference() is value
        assert not released
        events.append(label)
        if label == stop_at:
            raise marker

    for stop_at in ('before', 'after'):
        marker = RuntimeError(stop_at)
        try:
            module.delete_binding(value, observe)
        except RuntimeError as error:
            assert error is marker, 'the source callback exception must propagate unchanged'
            # Source-frame reconstruction is not part of the SOAC contract.
            # Clear any ordinary callback traceback before the cleanup check.
            error.__traceback__ = None
        else:
            raise AssertionError('the explicit callback exception was lost')
        del marker
        assert reference() is value and not released

    stop_at = None
    result = module.delete_binding(value, observe)
    assert result is value and reference() is value
    assert events == ['before', 'before', 'after', 'before', 'after']
    assert not released
    del result, value, observe
    gc.collect()
    assert reference() is None
    assert released == ['released']
"""


@pytest.mark.integration
@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["compiled", "entry"])
def test_explicit_del_preserves_alias_callbacks_and_cleanup(tmp_path, entry_interpreter):
    # Keep the original source and full stock locals-inspection control. Strict
    # execution checks observable alias/callback/cleanup behavior without
    # requesting unsupported optimized-frame locals. The original post-del
    # read/delete controls above retain their explicit checker-rejection case.
    source = _EXPLICIT_DEL_TRACEBACK_SOURCE
    validation = _EXPLICIT_DEL_TRACEBACK_VALIDATION
    name = "delete_binding_snapshots"
    with stock_module(tmp_path, name, source) as module:
        exec_integration_validation(validation, module, Path(__file__), mode="stock")
    path = f"{name}.py"
    opted_in, _ = strict_opt_in(source.encode(), path)
    project = create_strict_project(
        tmp_path / "strict-delete-snapshots",
        {path: opted_in.decode()},
        modules={name: path},
    )
    project.run_case(
        name, _EXPLICIT_DEL_ALIAS_VALIDATION, Path(__file__),
        entry_interpreter=entry_interpreter,
        required_functions=("delete_binding",),
    )
