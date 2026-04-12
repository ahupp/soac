from __future__ import annotations

import pytest

from tests._integration import soac_module


def test_entry_maybe_unbound_local_raises_without_stack_seed(tmp_path):
    source = """
def f(flag):
    if flag:
        x = 1
    return x
"""

    with soac_module(tmp_path, "entry_maybe_unbound_local", source) as module:
        assert module.f(True) == 1
        with pytest.raises(UnboundLocalError):
            module.f(False)
