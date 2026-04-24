SOURCE = "from __future__ import not_a_feature\nVALUE = 1\n"

# diet-python: validate

def validate_module(module):
    import importlib.util
    import os
    import tempfile
    import pytest
    import _soac_ext

    with tempfile.NamedTemporaryFile("w", suffix=".py", delete=False) as handle:
        handle.write(module.SOURCE)
        path = handle.name
    try:
        spec = importlib.util.spec_from_file_location("invalid_future_fixture", path)
        with pytest.raises(SyntaxError) as excinfo:
            _soac_ext.create_module(path, spec)
        assert "not_a_feature" in str(excinfo.value)
    finally:
        os.remove(path)
