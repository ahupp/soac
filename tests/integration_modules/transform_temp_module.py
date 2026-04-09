import importlib
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
        assert module.import_temp_module(tmp_path) is True
