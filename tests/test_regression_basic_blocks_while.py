import pytest

from tests._integration import integration_module


@pytest.mark.parametrize("mode", ["soac"])
def test_basic_block_lowering_while_break_continue_else(tmp_path, mode):
    source = """
def run(limit):
    i = 0
    out = []
    while i < limit:
        i = i + 1
        if i == 2:
            continue
        if i == 5:
            break
        out.append(i)
    else:
        out.append(99)
    return out, i
"""
    with integration_module(tmp_path, "basic_blocks_while", source, mode=mode) as module:
        assert module.run(3) == ([1, 3, 99], 3)
        assert module.run(10) == ([1, 3, 4], 5)
