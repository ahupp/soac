from __future__ import annotations

import dataclasses
import importlib
import os
import subprocess
import sys

from soac import import_hook
from tests._integration import transformed_module


def test_transformed_keyword_only_default_is_used_when_omitted(tmp_path):
    source = """
def value(*, marker=3):
    return marker
"""

    with transformed_module(tmp_path, "kwonly_default", source) as module:
        assert module.value() == 3


def test_transformed_function_can_evaluate_multiple_generator_expressions(tmp_path):
    source = """
def convert(value):
    return value

def positive(value):
    return value > 0

def value(items):
    converted = tuple(convert(item) for item in items)
    return all(positive(item) for item in converted)
"""

    with transformed_module(tmp_path, "multiple_genexprs", source) as module:
        assert module.value([1, 2, 3]) is True


def test_transformed_nested_try_can_raise_from_caught_exception(tmp_path):
    source = """
def wrap(should_wrap):
    if should_wrap:
        try:
            raise ValueError("inner")
        except ValueError as exc:
            raise RuntimeError("outer") from exc

try:
    wrap(True)
except RuntimeError as exc:
    WRAPPED = str(exc)
"""

    with transformed_module(tmp_path, "nested_raise_from", source) as module:
        assert module.WRAPPED == "outer"


def test_transformed_complex_literal_uses_literal_value(tmp_path):
    with transformed_module(tmp_path, "complex_literal", "VALUE = 1j\n") as module:
        assert module.VALUE == 1j


def test_import_hook_does_not_transform_reload_of_existing_plain_module(monkeypatch):
    import_hook.install()

    assert not isinstance(dataclasses.__spec__.loader, import_hook.SoacLoader)
    reloaded = importlib.reload(dataclasses)

    assert reloaded is dataclasses
    assert not isinstance(reloaded.__spec__.loader, import_hook.SoacLoader)


def test_module_enabled_path_filter_only_transforms_matching_tree(monkeypatch, tmp_path):
    enabled_dir = tmp_path / "enabled"
    skipped_dir = tmp_path / "skipped"
    enabled_dir.mkdir()
    skipped_dir.mkdir()
    (enabled_dir / "enabled_probe.py").write_text("VALUE = 1\n", encoding="utf-8")
    (skipped_dir / "skipped_probe.py").write_text("VALUE = 2\n", encoding="utf-8")

    monkeypatch.setenv("SOAC_MODULE_ENABLED", f"path:{enabled_dir}")
    import_hook.install()
    sys.path[:0] = [str(enabled_dir), str(skipped_dir)]
    try:
        enabled_probe = importlib.import_module("enabled_probe")
        skipped_probe = importlib.import_module("skipped_probe")

        assert isinstance(enabled_probe.__spec__.loader, import_hook.SoacLoader)
        assert not isinstance(skipped_probe.__spec__.loader, import_hook.SoacLoader)
    finally:
        sys.path.remove(str(enabled_dir))
        sys.path.remove(str(skipped_dir))
        sys.modules.pop("enabled_probe", None)
        sys.modules.pop("skipped_probe", None)


def test_import_hook_can_transform_stdlib_typing_in_fresh_process(monkeypatch, tmp_path):
    script = """
import sys

assert "typing" not in sys.modules
from soac import import_hook

import_hook.install()
import typing

assert isinstance(typing.__spec__.loader, import_hook.SoacLoader)
assert typing.Callable[..., typing.Any].__args__[-1] is typing.Any
"""
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**env, "DIET_PYTHON_COUNTERS_DIR": str(tmp_path / "typing-counters")},
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


def test_import_hook_can_transform_stdlib_import_edge_cases_in_fresh_process(
    monkeypatch, tmp_path
):
    script = """
import sys

assert "encodings.idna" not in sys.modules
assert "string.templatelib" not in sys.modules
from soac import import_hook

import_hook.install()
import string.templatelib as templatelib
import encodings.idna as idna

assert isinstance(templatelib.__spec__.loader, import_hook.SoacLoader)
assert isinstance(idna.__spec__.loader, import_hook.SoacLoader)
assert templatelib.convert("value", "s") == "value"
"""
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**env, "DIET_PYTHON_COUNTERS_DIR": str(tmp_path / "stdlib-edge-counters")},
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


def test_import_hook_can_transform_soac_runtime_in_fresh_process(monkeypatch, tmp_path):
    script = """
import sys

assert "soac.runtime" not in sys.modules
from soac import import_hook

import_hook.install()
import soac.runtime as runtime

assert runtime._SOAC_RUNTIME_READY is True
assert isinstance(runtime.__spec__.loader, import_hook.SoacLoader)
assert runtime.DELETED is runtime.DELETED
assert runtime.ITER_COMPLETE is runtime.ITER_COMPLETE
"""
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**env, "DIET_PYTHON_COUNTERS_DIR": str(tmp_path / "runtime-counters")},
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


def test_import_hook_can_transform_soac_runtime_in_profile_mode(monkeypatch, tmp_path):
    script = """
from soac import import_hook

import_hook.install()
import soac.runtime as runtime

assert runtime._SOAC_RUNTIME_READY is True
assert isinstance(runtime.AsyncGenComplete(), Exception)
"""
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={
            **env,
            "DIET_PYTHON_COUNTERS_DIR": str(tmp_path / "runtime-profile-counters"),
            "DIET_PYTHON_SPECIALIZATION_MODE": "profile",
        },
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


def test_import_hook_can_reload_transformed_temp_module(monkeypatch, tmp_path):
    monkeypatch.setenv("SOAC_MODULE_ENABLED", f"path:{tmp_path}")
    import_hook.install()

    helper_path = tmp_path / "transformed_helper.py"
    helper_path.write_text("VALUE = 1\n", encoding="utf-8")
    module_path = tmp_path / "reload_probe.py"
    module_path.write_text(
        """
import importlib
import transformed_helper

transformed_helper = importlib.reload(transformed_helper)
VALUE = transformed_helper.VALUE + 1
""",
        encoding="utf-8",
    )

    sys.path.insert(0, str(tmp_path))
    try:
        helper = importlib.import_module("transformed_helper")
        assert isinstance(helper.__spec__.loader, import_hook.SoacLoader)

        module = importlib.import_module("reload_probe")
        assert module.VALUE == 2
        assert isinstance(module.__spec__.loader, import_hook.SoacLoader)
    finally:
        sys.path.remove(str(tmp_path))
        sys.modules.pop("reload_probe", None)
        sys.modules.pop("transformed_helper", None)
