from __future__ import annotations

from pathlib import Path

import pytest

from tests._integration import integration_module


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


@pytest.mark.integration
@pytest.mark.parametrize("mode", ["stock", "soac"], ids=["stock", "soac"])
def test_branch_transition_keeps_frame_local_until_return(
    tmp_path: Path, mode: str
) -> None:
    with integration_module(tmp_path, "frame_local_branch_cleanup", SOURCE, mode=mode) as module:
        module.reset()

        result = module.branch_local(False)

        assert result == ["else", "after"]
        assert module.events == ["else", "after", "del:branch"]


@pytest.mark.integration
@pytest.mark.parametrize("mode", ["stock", "soac"], ids=["stock", "soac"])
def test_rebind_and_del_cleanup_at_statement_boundary(
    tmp_path: Path, mode: str
) -> None:
    with integration_module(tmp_path, "frame_local_rebind_cleanup", SOURCE, mode=mode) as module:
        module.reset()

        rebind_result = module.rebind_local()

        assert rebind_result == ["before", "del:old", "after"]
        assert module.events == ["before", "del:old", "after", "del:new"]

        module.reset()

        delete_result = module.delete_local()

        assert delete_result == ["before", "del:deleted", "after"]
        assert module.events == ["before", "del:deleted", "after"]


@pytest.mark.integration
@pytest.mark.parametrize("mode", ["stock", "soac"], ids=["stock", "soac"])
def test_exception_exit_cleans_frame_local_before_handler(
    tmp_path: Path, mode: str
) -> None:
    with integration_module(tmp_path, "frame_local_exception_cleanup", SOURCE, mode=mode) as module:
        module.reset()

        with pytest.raises(ValueError):
            module.raise_local()
        module.events.append("caught")

        assert module.events == ["before", "del:raised", "caught"]


@pytest.mark.integration
@pytest.mark.parametrize("mode", ["stock", "soac"], ids=["stock", "soac"])
def test_exception_dispatch_keeps_frame_local_until_handler_exit(
    tmp_path: Path, mode: str
) -> None:
    with integration_module(
        tmp_path, "frame_local_caught_exception_cleanup", SOURCE, mode=mode
    ) as module:
        module.reset()

        result = module.caught_exception_local()

        assert result == ["handler"]
        assert module.events == ["handler", "del:caught"]
