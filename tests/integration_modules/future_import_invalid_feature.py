SOURCE = "from __future__ import not_a_feature\nVALUE = 1\n"

# diet-python: validate

def validate_module(module):
    import importlib.util
    import os
    import tempfile
    import pytest
    from soac.import_hook import SoacLoader

    with tempfile.NamedTemporaryFile("w", suffix=".py", delete=False) as handle:
        handle.write(module.SOURCE)
        path = handle.name
    try:
        spec = importlib.util.spec_from_file_location("invalid_future_fixture", path)
        spec.loader = SoacLoader(spec.name, path, spec.loader)
        loaded = importlib.util.module_from_spec(spec)
        with pytest.raises(SyntaxError) as excinfo:
            spec.loader.exec_module(loaded)
        assert "not_a_feature" in str(excinfo.value)
    finally:
        os.remove(path)
