import pytest

from tests._integration import integration_module


@pytest.mark.parametrize("mode", ["soac"])
def test_magicmock_richcompare_uses_bound_special_method(tmp_path, mode):
    source = """
from unittest import mock


def run():
    value = mock.MagicMock()
    return value == 1, value != 1
"""

    with integration_module(tmp_path, "magicmock_richcompare", source, mode=mode) as module:
        assert module.run() == (False, True)
