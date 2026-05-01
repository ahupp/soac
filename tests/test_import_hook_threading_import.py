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
    env = os.environ.copy()
    env["SOAC_MODULE_ENABLED"] = f"path:{module_path}"

    result = subprocess.run(
        [sys.executable, "-m", "soac.import_hook", str(module_path)],
        text=True,
        capture_output=True,
        env=env,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "1"


def test_import_hook_path_entrypoint_preserves_script_main_for_multiprocessing(tmp_path):
    module_path = tmp_path / "mp_main_probe.py"
    module_path.write_text(
        "\n".join(
            [
                "import multiprocessing.spawn as spawn",
                "data = spawn.get_preparation_data('ignore')",
                "print(data.get('init_main_from_path'))",
                "print(data.get('init_main_from_name'))",
            ]
        )
    )
    env = os.environ.copy()
    env["SOAC_MODULE_ENABLED"] = f"path:{module_path}"

    result = subprocess.run(
        [sys.executable, "-m", "soac.import_hook", str(module_path)],
        text=True,
        capture_output=True,
        env=env,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == [str(module_path), "None"]


def test_import_hook_path_entrypoint_matches_script_import_context(tmp_path):
    helper_path = tmp_path / "entry_helper.py"
    helper_path.write_text("VALUE = 37\n")
    module_path = tmp_path / "entry_context_probe.py"
    module_path.write_text(
        "\n".join(
            [
                "import builtins",
                "import entry_helper",
                "print(__builtins__ is builtins)",
                "__builtins__.open = open",
                "print(entry_helper.VALUE)",
            ]
        )
    )
    env = os.environ.copy()
    env["SOAC_MODULE_ENABLED"] = f"path:{tmp_path}"

    result = subprocess.run(
        [sys.executable, "-m", "soac.import_hook", str(module_path)],
        text=True,
        capture_output=True,
        env=env,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == ["True", "37"]


def test_import_hook_transforms_resolvable_frozen_module_source():
    env = os.environ.copy()
    ntpath_source = (
        Path(__file__).resolve().parent.parent / "vendor" / "cpython" / "Lib" / "ntpath.py"
    )
    env["SOAC_MODULE_ENABLED"] = f"path:{ntpath_source}"
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
