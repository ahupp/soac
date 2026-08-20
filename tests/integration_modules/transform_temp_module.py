import importlib
import os
import sys
import textwrap
from pathlib import Path


def import_temp_module(tmp_path: Path) -> bool:
    module_name = "dp_transform_temp"
    module_path = tmp_path / f"{module_name}.py"
    module_path.write_text(
        textwrap.dedent(
            """\
            VALUE = 1
            """
        ),
        encoding="utf-8",
    )
    sys.path.insert(0, str(tmp_path))
    try:
        sys.modules.pop(module_name, None)
        module = importlib.import_module(module_name)
        return "runtime" in module.__dict__
    finally:
        sys.modules.pop(module_name, None)
        if sys.path and sys.path[0] == str(tmp_path):
            sys.path.pop(0)

# diet-python: validate

def validate_module(module):
    import tempfile
    from pathlib import Path


    with tempfile.TemporaryDirectory() as tmp_dir:
        tmp_path = Path(tmp_dir)
        prior_enabled_modules = os.environ.get("SOAC_MODULE_ENABLED")
        entry = f"path:{tmp_path.resolve()}"
        os.environ["SOAC_MODULE_ENABLED"] = (
            f"{prior_enabled_modules},{entry}" if prior_enabled_modules else entry
        )
        try:
            # An allow-list entry is not strict source opt-in, checker
            # publication, or native startup authority. The temporary module
            # must remain ordinary in both the stock and strict callers.
            assert module.import_temp_module(tmp_path) is False
        finally:
            if prior_enabled_modules is None:
                os.environ.pop("SOAC_MODULE_ENABLED", None)
            else:
                os.environ["SOAC_MODULE_ENABLED"] = prior_enabled_modules
