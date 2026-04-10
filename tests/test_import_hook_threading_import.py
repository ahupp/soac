import os
import subprocess
import sys
from pathlib import Path


def test_import_hook_does_not_import_threading():
    code = "from soac import import_hook; import sys; print('threading' in sys.modules)"
    result = subprocess.run(
        [sys.executable, "-c", code],
        check=True,
        text=True,
        capture_output=True,
    )
    assert result.stdout.strip() == "False"


def test_import_hook_entry_module_bootstraps_runtime():
    module_path = (
        Path(__file__).resolve().parent
        / "integration_modules"
        / "import_hook_entry_bootstrap.py"
    )

    result = subprocess.run(
        [sys.executable, "-m", "soac.import_hook", str(module_path)],
        text=True,
        capture_output=True,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "1"


def test_import_hook_transforms_resolvable_frozen_module_source():
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    code = "\n".join(
        [
            "from soac import import_hook",
            "import_hook.install()",
            "import sys",
            "sys.modules.pop('ntpath', None)",
            "import ntpath",
            "print(type(ntpath.__spec__.loader).__name__)",
            'print(ntpath.__spec__.origin.endswith("/Lib/ntpath.py"))',
            'print(ntpath.__file__.endswith("/Lib/ntpath.py"))',
        ]
    )
    result = subprocess.run(
        [sys.executable, "-c", code],
        check=True,
        text=True,
        capture_output=True,
        env=env,
    )
    assert result.stdout.splitlines() == ["SoacLoader", "True", "True"]


def test_import_hook_preserves_cpython_frozen_fixture_loader():
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    code = "\n".join(
        [
            "import importlib.machinery",
            "from soac import import_hook",
            "import_hook.install()",
            "import __phello__.spam as spam",
            "print(spam.__spec__.loader is importlib.machinery.FrozenImporter)",
            "print(spam.__spec__.origin)",
        ]
    )
    result = subprocess.run(
        [sys.executable, "-c", code],
        check=True,
        text=True,
        capture_output=True,
        env=env,
    )
    assert result.stdout.splitlines() == ["True", "frozen"]
