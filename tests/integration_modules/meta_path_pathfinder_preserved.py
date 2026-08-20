import importlib
import sys
import textwrap
from pathlib import Path
from tempfile import TemporaryDirectory

from soac import import_hook


def import_with_filtered_meta_path() -> bool:
    original_meta_path = list(sys.meta_path)
    try:
        import_hook.install()
        sys.meta_path[:] = [
            item for item in sys.meta_path if item.__module__.startswith("_frozen_importlib")
        ]
        with TemporaryDirectory() as tmp_dir:
            tmp_path = Path(tmp_dir)
            module_name = "dp_meta_path_temp"
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
                importlib.import_module(module_name)
                return True
            finally:
                sys.modules.pop(module_name, None)
                if sys.path and sys.path[0] == str(tmp_path):
                    sys.path.pop(0)
    except ModuleNotFoundError:
        return False
    finally:
        sys.meta_path[:] = original_meta_path

# diet-python: validate

def validate_module(module):
    assert module.import_with_filtered_meta_path() is True
