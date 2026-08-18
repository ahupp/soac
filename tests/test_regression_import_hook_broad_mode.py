from __future__ import annotations

import dataclasses
import importlib
import json
import os
import subprocess
import sys

import pytest

from soac import import_hook
from tests._integration import soac_module

BROAD_MODE_SLOW = pytest.mark.slow(
    reason="broad import-hook mode transforms stdlib dependency graphs in fresh processes"
)


def test_soac_keyword_only_default_is_used_when_omitted(tmp_path):
    source = """
def value(*, marker=3):
    return marker
"""

    with soac_module(tmp_path, "kwonly_default", source) as module:
        assert module.value() == 3


def test_soac_function_can_evaluate_multiple_generator_expressions(tmp_path):
    source = """
def convert(value):
    return value

def positive(value):
    return value > 0

def value(items):
    converted = tuple(convert(item) for item in items)
    return all(positive(item) for item in converted)
"""

    with soac_module(tmp_path, "multiple_genexprs", source) as module:
        assert module.value([1, 2, 3]) is True
        assert sys.exception() is None


def test_soac_nested_try_can_raise_from_caught_exception(tmp_path):
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

    with soac_module(tmp_path, "nested_raise_from", source) as module:
        assert module.WRAPPED == "outer"


def test_soac_complex_literal_uses_literal_value(tmp_path):
    with soac_module(tmp_path, "complex_literal", "VALUE = 1j\n") as module:
        assert module.VALUE == 1j


def test_soac_builtin_pow_accepts_mod_argument(tmp_path):
    with soac_module(tmp_path, "pow_mod_argument", "VALUE = pow(2, 5, 7)\n") as module:
        assert module.VALUE == 4


def test_soac_missing_from_import_attribute_raises_import_error(tmp_path):
    source = """
try:
    from math import __soac_missing_attr__
except ImportError as exc:
    VALUE = exc.name
else:
    VALUE = "imported"
"""

    with soac_module(tmp_path, "missing_from_import_attr", source) as module:
        assert module.VALUE == "math"


def test_soac_from_import_sys_modules_fallback_does_not_set_parent_attr(tmp_path):
    package_dir = tmp_path / "from_import_cached_pkg"
    package_dir.mkdir()
    (package_dir / "__init__.py").write_text("", encoding="utf-8")
    (package_dir / "child.py").write_text("VALUE = 42\n", encoding="utf-8")
    source = f"""
import sys
sys.path.insert(0, {str(tmp_path)!r})
try:
    import from_import_cached_pkg.child as child
    import from_import_cached_pkg as parent
    del parent.child
    from from_import_cached_pkg import child as imported
    VALUE = (imported is child, hasattr(parent, "child"))
finally:
    sys.path.remove({str(tmp_path)!r})
"""

    with soac_module(tmp_path, "from_import_cached_submodule", source) as module:
        assert module.VALUE == (True, False)


def test_soac_module_helper_does_not_transform_stdlib_imports_by_default(tmp_path):
    source = """
import sys

sys.modules.pop("colorsys", None)
import colorsys

VALUE = type(colorsys.__spec__.loader).__name__
"""

    with soac_module(tmp_path, "helper_scoped_to_temp_module", source) as module:
        assert module.VALUE != "SoacLoader"


def test_soac_package_relative_import_star_binds_submodule_name(
    monkeypatch, tmp_path
):
    package_dir = tmp_path / "relative_star_pkg"
    package_dir.mkdir()
    (package_dir / "child.py").write_text(
        '__all__ = ["EXPORTED"]\nMARKER = "child"\nEXPORTED = 3\n',
        encoding="utf-8",
    )
    (package_dir / "__init__.py").write_text(
        "from .child import *\nVALUE = child.MARKER\n",
        encoding="utf-8",
    )

    monkeypatch.setenv("SOAC_MODULE_ENABLED", f"path:{package_dir}")
    import_hook.install()
    sys.path.insert(0, str(tmp_path))
    try:
        module = importlib.import_module("relative_star_pkg")
        assert isinstance(module.__spec__.loader, import_hook.SoacLoader)
        assert module.VALUE == "child"
        assert module.EXPORTED == 3
    finally:
        sys.path.remove(str(tmp_path))
        sys.modules.pop("relative_star_pkg", None)
        sys.modules.pop("relative_star_pkg.child", None)


def test_soac_function_locals_fails_explicitly(tmp_path):
    source = """
def value():
    optdict = {"verbose": ""}
    expected_opts = {"verbose": ""}
    args = []
    return "%(optdict)s %(expected_opts)s %(args)s" % locals()
"""

    with soac_module(tmp_path, "locals_recent_assignment", source) as module:
        with pytest.raises(
            NotImplementedError, match="frame-sensitive globals/locals/eval/exec"
        ):
            module.value()


def test_soac_eval_fails_explicitly(tmp_path):
    source = """
def value():
    left = 3
    right = 4
    return eval("left + right")
"""

    with soac_module(tmp_path, "eval_current_locals", source) as module:
        with pytest.raises(
            NotImplementedError, match="frame-sensitive globals/locals/eval/exec"
        ):
            module.value()


def test_soac_eval_in_loop_fails_explicitly(tmp_path):
    source = """
def value():
    for item in [12]:
        bad_format_spec = "%M"
        try:
            eval("f'xx{item:{bad_format_spec}}yy'")
        except ValueError as exc:
            return "Invalid format specifier" in str(exc)
    return False
"""

    with soac_module(tmp_path, "eval_for_loop_target_local", source) as module:
        with pytest.raises(
            NotImplementedError, match="frame-sensitive globals/locals/eval/exec"
        ):
            module.value()


def test_soac_nested_coroutine_nonlocal_capture_in_method(tmp_path):
    source = """
def build():
    cancelled = False

    class Test:
        async def test_leaking_task(self):
            async def coro():
                nonlocal cancelled
                cancelled = True

            await coro()

        def was_cancelled(self):
            return cancelled

    return Test()
"""

    with soac_module(tmp_path, "nested_coroutine_nonlocal_method", source) as module:
        instance = module.build()
        coroutine = instance.test_leaking_task()
        try:
            coroutine.send(None)
        except StopIteration as exc:
            assert exc.value is None
        else:
            raise AssertionError("coroutine should finish without suspension")
        assert instance.was_cancelled() is True


def test_soac_lambda_in_function_decorator_is_lowered(tmp_path):
    source = """
def keep(value):
    def decorator(func):
        return value
    return decorator

sentinel = object()

@keep(lambda: sentinel)
def chosen():
    return None

VALUE = chosen()
"""

    with soac_module(tmp_path, "lambda_function_decorator", source) as module:
        assert module.VALUE is module.sentinel


def test_soac_coroutine_global_store_updates_module(tmp_path):
    source = """
flag = False

async def set_flag():
    global flag
    flag = True

def value():
    coroutine = set_flag()
    try:
        coroutine.send(None)
    except StopIteration as exc:
        assert exc.value is None
    else:
        raise AssertionError("coroutine should finish without suspension")
    return flag
"""

    with soac_module(tmp_path, "coroutine_global_store", source) as module:
        assert module.value() is True
        assert module.flag is True


def test_soac_function_uses_updated_positional_defaults(tmp_path):
    source = """
def first_func(a, b):
    return a + b

first_func.__defaults__ = (1, 2)
VALUE = first_func()
"""

    with soac_module(tmp_path, "updated_positional_defaults", source) as module:
        assert module.VALUE == 3


def test_soac_dict_value_can_use_conditional_expression(tmp_path):
    source = 'VALUE = {"flags": tuple([1]) if True else None}\n'

    with soac_module(tmp_path, "dict_conditional_value", source) as module:
        assert module.VALUE == {"flags": (1,)}


def test_soac_while_condition_can_use_generator_expression(tmp_path):
    source = """
class Worker:
    def is_alive(self):
        return True

def value():
    count = 0
    workers = [Worker()]
    while count < 1 and all(worker.is_alive() for worker in workers):
        count += 1
    return count
"""

    with soac_module(tmp_path, "while_generator_condition", source) as module:
        assert module.value() == 1


def test_soac_nested_class_base_uses_enclosing_function_local(tmp_path):
    source = """
def value():
    class A:
        pass

    class B:
        pass

    class C(B):
        pass

    C.__bases__ = (A,)
    return C.__bases__[0].__name__
"""

    with soac_module(tmp_path, "nested_class_base_local", source) as module:
        assert module.value() == "A"


def test_soac_nested_class_nonlocal_classcell_keeps_inner_method_classcell(tmp_path):
    source = """
class Outer:
    def value(self):
        class Inner:
            nonlocal __class__
            __class__ = 42

            def cls():
                return __class__

        outer_classcell_value = __class__
        return outer_classcell_value, Inner.cls(), Inner
"""

    with soac_module(tmp_path, "nested_nonlocal_classcell", source) as module:
        outer_value, inner_value, inner_cls = module.Outer().value()
        assert outer_value == 42
        assert inner_value is inner_cls


def test_soac_nested_class_body_dunder_class_does_not_steal_inner_method_classcell(tmp_path):
    source = """
class Host:
    def value(self):
        class Inner:
            outer = __class__

            def cls():
                return __class__

        return Inner.outer, Inner.cls(), Inner
"""

    with soac_module(tmp_path, "nested_class_body_dunder_class", source) as module:
        outer_value, inner_value, inner_cls = module.Host().value()
        assert outer_value is module.Host
        assert inner_value is inner_cls


def test_soac_class_body_dunder_class_assignment_does_not_leak_method_classcell(tmp_path):
    source = """
class Base:
    def marker(self):
        return "base"

class Host:
    def value(self):
        class First(Base):
            def marker(self):
                return super().marker()

            __class__ = 413

        class Second:
            outer = __class__

            def cls():
                return __class__

        return First().marker(), First().__class__, Second.outer, Second.cls(), Second
"""

    with soac_module(tmp_path, "class_body_dunder_class_assignment", source) as module:
        first_marker, first_class_attr, second_outer, second_value, second_cls = (
            module.Host().value()
        )
        assert first_marker == "base"
        assert first_class_attr == 413
        assert second_outer is module.Host
        assert second_value is second_cls


def test_soac_class_body_dunder_class_assignment_keeps_method_super_classcell(tmp_path):
    source = """
class Base:
    def marker(self):
        return "base"

def value():
    class Derived(Base):
        def marker(self):
            return super().marker()

        __class__ = 413

    instance = Derived()
    return instance.marker(), instance.__class__
"""

    with soac_module(tmp_path, "assigned_dunder_class_method_super", source) as module:
        assert module.value() == ("base", 413)


def test_soac_nested_method_super_class_attr_does_not_require_outer_classcell_capture(tmp_path):
    source = """
class Host:
    def value(self):
        class Inner:
            def method(self):
                return super().__class__

        return Inner().method()
"""

    with soac_module(tmp_path, "nested_method_super_class_attr", source) as module:
        assert module.Host().value() is super


def test_soac_init_subclass_for_loop_uses_iterator(tmp_path):
    source = """
class Base:
    def __init_subclass__(cls, /, **kwargs):
        cls.SEEN = []
        for item in cls.__mro__:
            cls.SEEN.append(item.__name__)

class Child(Base):
    pass
"""

    with soac_module(tmp_path, "init_subclass_for_loop", source) as module:
        assert module.Child.SEEN[:2] == ["Child", "Base"]


def test_soac_classcell_missing_raises_runtime_error(tmp_path):
    source = """
def value():
    class Meta(type):
        def __new__(cls, name, bases, namespace):
            namespace.pop("__classcell__", None)
            return super().__new__(cls, name, bases, namespace)

    class WithClassRef(metaclass=Meta):
        def f(self):
            return __class__
"""

    with soac_module(tmp_path, "classcell_missing", source) as module:
        with pytest.raises(RuntimeError, match="__class__ not set.*__classcell__ propagated"):
            module.value()


def test_soac_classcell_wrong_cell_raises_type_error(tmp_path):
    source = """
def value():
    class Meta(type):
        def __new__(cls, name, bases, namespace):
            cls = super().__new__(cls, name, bases, namespace)
            type("Other", (), namespace)
            return cls

    class WithClassRef(metaclass=Meta):
        def f(self):
            return __class__
"""

    with soac_module(tmp_path, "classcell_wrong_cell", source) as module:
        with pytest.raises(TypeError):
            module.value()


def test_soac_zero_arg_super_uses_dynamic_global_super(tmp_path):
    source = """
class MySuper:
    msg = "super super"

class C:
    def method(self):
        return super().msg

def value():
    global super
    previous = super
    super = MySuper
    try:
        return C().method()
    finally:
        super = previous
"""

    with soac_module(tmp_path, "dynamic_global_super", source) as module:
        assert module.value() == "super super"


def test_soac_nested_zero_arg_super_without_self_reports_no_arguments(tmp_path):
    source = """
class Host:
    def value(self):
        def nested():
            super()

        nested()
"""

    with soac_module(tmp_path, "nested_zero_arg_super_no_args", source) as module:
        with pytest.raises(RuntimeError, match="no arguments"):
            module.Host().value()


def test_soac_deleted_super_first_arg_reports_arg_deleted(tmp_path):
    source = """
class Host:
    def value(self):
        def nested(x):
            del x
            super()

        nested(self)
"""

    with soac_module(tmp_path, "deleted_super_first_arg", source) as module:
        with pytest.raises(RuntimeError, match=r"arg\[0\] deleted"):
            module.Host().value()


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


def test_soac_modules_ignore_module_enabled_path_filter(monkeypatch, tmp_path):
    enabled_dir = tmp_path / "enabled"
    enabled_dir.mkdir()
    disabled_path = tmp_path / "outside" / "runtime.py"

    monkeypatch.setenv("SOAC_MODULE_ENABLED", f"path:{enabled_dir}")

    assert import_hook._should_transform_module("soac", str(disabled_path))
    assert import_hook._should_transform_module("soac.runtime", str(disabled_path))
    assert not import_hook._should_transform_module("outside.runtime", str(disabled_path))


def test_background_jit_does_not_recursively_compile_import_tree(tmp_path):
    log_path = tmp_path / "jit-events.jsonl"
    package_dir = tmp_path / "recursive_bg_pkg"
    package_dir.mkdir()
    (package_dir / "__init__.py").write_text("", encoding="utf-8")
    (package_dir / "child.py").write_text(
        """
VALUE = 5

def child_value():
    return VALUE
""",
        encoding="utf-8",
    )
    (package_dir / "parent.py").write_text(
        """
from recursive_bg_pkg import child

VALUE = child.VALUE + 1

def parent_value():
    return VALUE
""",
        encoding="utf-8",
    )

    script = f"""
import json
import pathlib
import sys
import time

sys.path.insert(0, {str(tmp_path)!r})
from soac import import_hook

import_hook.install()
from recursive_bg_pkg import parent

assert parent.parent_value() == 6
log_path = pathlib.Path({str(log_path)!r})
deadline = time.monotonic() + 5
while time.monotonic() < deadline:
    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ] if log_path.exists() else []
    if any(
        row.get("message") == "jit_background_module_compile_done"
        and row.get("module_name") == "recursive_bg_pkg.parent"
        for row in rows
    ):
        break
    time.sleep(0.05)
else:
    raise AssertionError("parent background JIT compile did not finish")

# Give a wrongly recursive child background compile a chance to show up.
time.sleep(0.25)
"""
    env = os.environ.copy()
    env.pop("SOAC_OPT_MODE", None)
    env["SOAC_BACKGROUND_JIT"] = "1"
    env["SOAC_MODULE_ENABLED"] = f"path:{tmp_path}"
    env["SOAC_WORK_DIR"] = str(tmp_path / "background-jit-work")
    env["SOAC_LOG"] = f"soac_jit_codegen=info;json={log_path}"
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env=env,
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    background_modules = {
        row["module_name"]
        for row in rows
        if row.get("message") == "jit_background_module_compile_done"
    }
    assert "recursive_bg_pkg.parent" in background_modules
    assert "recursive_bg_pkg.child" not in background_modules


def test_immediate_call_does_not_deadlock_with_background_jit(tmp_path):
    package_dir = tmp_path / "immediate_bg_pkg"
    package_dir.mkdir()
    (package_dir / "__init__.py").write_text("", encoding="utf-8")
    (package_dir / "mod.py").write_text(
        """
def identity(value):
    return value
""",
        encoding="utf-8",
    )

    script = f"""
import sys

sys.path.insert(0, {str(tmp_path)!r})
from soac import import_hook

import_hook.install()
from immediate_bg_pkg import mod

assert mod.identity(4) == 4
"""
    env = os.environ.copy()
    env.pop("SOAC_OPT_MODE", None)
    env["SOAC_BACKGROUND_JIT"] = "1"
    env["SOAC_MODULE_ENABLED"] = f"path:{package_dir}"
    env["SOAC_WORK_DIR"] = str(tmp_path / "immediate-call-work")
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env=env,
        text=True,
        timeout=10,
    )

    assert result.returncode == 0, result.stdout + result.stderr


@BROAD_MODE_SLOW
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
        env={**env, "SOAC_WORK_DIR": str(tmp_path / "typing-counters")},
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


@BROAD_MODE_SLOW
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
        env={**env, "SOAC_WORK_DIR": str(tmp_path / "stdlib-edge-counters")},
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


@BROAD_MODE_SLOW
def test_import_hook_can_transform_shutil_rmtree_in_fresh_process(tmp_path):
    target = tmp_path / "to-remove"
    script = f"""
import os

root = {str(target)!r}
os.makedirs(os.path.join(root, "child"))
with open(os.path.join(root, "child", "marker.txt"), "w", encoding="utf-8") as file:
    file.write("marker")

from soac import import_hook

import_hook.install()
import shutil

assert isinstance(shutil.__spec__.loader, import_hook.SoacLoader)
shutil.rmtree(root)
assert not os.path.exists(root)
"""
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**env, "SOAC_WORK_DIR": str(tmp_path / "rmtree-counters")},
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


@BROAD_MODE_SLOW
def test_import_hook_can_transform_soac_runtime_in_fresh_process(monkeypatch, tmp_path):
    script = """
import sys
import ctypes
import _testinternalcapi

assert "soac.runtime" not in sys.modules
from soac import import_hook

import_hook.install()
import soac.runtime as runtime

assert runtime._SOAC_RUNTIME_READY is True
assert isinstance(runtime.__spec__.loader, import_hook.SoacLoader)
assert _testinternalcapi.has_indexed_values(runtime.__dict__)
for name in ("globals", "locals", "eval", "exec"):
    try:
        getattr(runtime, name)()
    except NotImplementedError as exc:
        assert "frame-sensitive globals/locals/eval/exec" in str(exc)
    else:
        raise AssertionError(f"soac.runtime.{name} should fail explicitly")

get_function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
get_function_id.argtypes = [ctypes.py_object]
get_function_id.restype = ctypes.c_uint64
get_function_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
get_function_metadata.argtypes = [ctypes.py_object]
get_function_metadata.restype = ctypes.c_void_p
get_type_function_id = ctypes.pythonapi.PyType_GetSoacFunctionId
get_type_function_id.argtypes = [ctypes.py_object]
get_type_function_id.restype = ctypes.c_uint64

assert runtime.range is range
assert runtime.range.__module__ == range.__module__
assert runtime.range.__name__ == range.__name__

runtime_functions = (
    runtime.IterRange.__dict__["__init__"],
    runtime.IterRange.__dict__["__next__"],
)
for function in runtime_functions:
    assert get_function_id(function) != 0
    assert get_function_metadata(function) is not None

assert get_type_function_id(runtime.IterRange) != 0
assert get_type_function_id(runtime.IterRange) != get_function_id(
    runtime.IterRange.__dict__["__init__"]
)
"""
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**env, "SOAC_WORK_DIR": str(tmp_path / "runtime-counters")},
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


@BROAD_MODE_SLOW
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
            "SOAC_WORK_DIR": str(tmp_path / "runtime-profile-counters"),
            "SOAC_OPT_MODE": "profile",
        },
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


@BROAD_MODE_SLOW
def test_import_hook_can_transform_soac_runtime_in_verify_mode(tmp_path):
    script = """
from soac import import_hook

import_hook.install()
import soac.runtime as runtime

assert runtime._SOAC_RUNTIME_READY is True
assert runtime.typing_Generic is not None
"""
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    base_env = {
        **env,
        "SOAC_WORK_DIR": str(tmp_path / "runtime-profile-counters"),
    }
    profile_result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        text=True,
    )
    assert profile_result.returncode == 0, profile_result.stdout + profile_result.stderr

    verify_result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**base_env, "SOAC_OPT_MODE": "verify"},
        text=True,
    )
    assert verify_result.returncode == 0, verify_result.stdout + verify_result.stderr


def test_import_hook_can_reload_soac_temp_module(monkeypatch, tmp_path):
    monkeypatch.setenv("SOAC_MODULE_ENABLED", f"path:{tmp_path}")
    import_hook.install()

    helper_path = tmp_path / "soac_helper.py"
    helper_path.write_text("VALUE = 1\n", encoding="utf-8")
    module_path = tmp_path / "reload_probe.py"
    module_path.write_text(
        """
import importlib
import soac_helper

soac_helper = importlib.reload(soac_helper)
VALUE = soac_helper.VALUE + 1
""",
        encoding="utf-8",
    )

    sys.path.insert(0, str(tmp_path))
    try:
        helper = importlib.import_module("soac_helper")
        assert isinstance(helper.__spec__.loader, import_hook.SoacLoader)

        module = importlib.import_module("reload_probe")
        assert module.VALUE == 2
        assert isinstance(module.__spec__.loader, import_hook.SoacLoader)
    finally:
        sys.path.remove(str(tmp_path))
        sys.modules.pop("reload_probe", None)
        sys.modules.pop("soac_helper", None)


def test_cross_module_nested_function_creation_uses_callee_module_metadata(tmp_path):
    helper_path = tmp_path / "nested_function_helper.py"
    helper_path.write_text(
        """
def outer():
    def inner():
        return 7
    return inner()
""",
        encoding="utf-8",
    )
    main_path = tmp_path / "nested_function_main.py"
    main_path.write_text(
        """
import nested_function_helper

VALUE = nested_function_helper.outer()
""",
        encoding="utf-8",
    )
    script = f"""
import sys
from soac import import_hook

sys.path.insert(0, {str(tmp_path)!r})
import_hook.install()
import nested_function_main

assert nested_function_main.VALUE == 7
"""
    env = os.environ.copy()
    env["SOAC_MODULE_ENABLED"] = f"path:{tmp_path}"
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env=env,
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


def test_soac_except_body_sets_cpython_handled_exception(tmp_path):
    source = """
import sys
import traceback

def capture(flag):
    try:
        raise ValueError("boom")
    except:
        if flag:
            text = traceback.format_exc()
            active_type = type(sys.exception()).__name__
        else:
            text = "missing"
            active_type = type(sys.exception()).__name__
    return "ValueError: boom" in text, active_type, sys.exception()
"""

    with soac_module(tmp_path, "except_handled_exception_state", source) as module:
        assert module.capture(True) == (True, "ValueError", None)


def test_soac_typed_except_catches_return_expression_error(tmp_path):
    source = """
def f():
    try:
        return {}["x"]
    except KeyError:
        return "ok"
"""

    with soac_module(tmp_path, "typed_except_return_expr", source) as module:
        assert module.f() == "ok"


def test_soac_outer_try_catches_exception_from_with_body(tmp_path):
    source = """
class Manager:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

def f():
    try:
        with Manager():
            open(None)
    except TypeError as exc:
        return str(exc).split(":")[0]
    return "miss"
"""

    with soac_module(tmp_path, "outer_try_with_body_exception", source) as module:
        assert module.f() == "expected str, bytes or os.PathLike object, not NoneType"


def test_soac_generator_contextmanager_with_body_reraises_thrown_exception(tmp_path):
    source = """
from contextlib import contextmanager

class Manager:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

@contextmanager
def manager():
    with Manager():
        yield

class MarkerError(Exception):
    pass

def check_exit():
    cm = manager()
    cm.__enter__()
    try:
        raise MarkerError("boom")
    except MarkerError as exc:
        return cm.__exit__(type(exc), exc, exc.__traceback__)
"""

    with soac_module(
        tmp_path, "generator_contextmanager_with_body_reraise", source
    ) as module:
        assert module.check_exit() is False


def test_soac_lone_surrogate_string_literal_uses_replacement_character(tmp_path):
    source = r"""
VALUE = "\uD82A"
"""

    with soac_module(tmp_path, "lone_surrogate_string_literal", source) as module:
        assert module.VALUE == "\ufffd"


def test_soac_attribute_assignment_temps_do_not_keep_cycle_alive(tmp_path):
    source = """
import ast
import gc
import weakref

def value():
    class X:
        pass
    a = ast.AST()
    a.x = X()
    a.x.a = a
    ref = weakref.ref(a.x)
    del a
    gc.collect()
    return ref()
"""

    with soac_module(tmp_path, "assignment_temp_gc_cycle", source) as module:
        assert module.value() is None


def test_soac_except_body_exception_keeps_implicit_context(tmp_path):
    source = r"""
import ast
import unittest

def value():
    case = unittest.TestCase()
    try:
        1 / 0
    except Exception:
        with case.assertRaises(SyntaxError) as caught:
            ast.literal_eval(r"'\U'")
        return type(caught.exception.__context__).__name__
    return "missing"
"""

    with soac_module(tmp_path, "except_body_implicit_context", source) as module:
        assert module.value() == "ZeroDivisionError"


def test_soac_try_finally_preserves_callers_handled_exception(tmp_path):
    source = """
import sys

def inner():
    try:
        pass
    finally:
        marker = 1
    return marker

def value():
    try:
        1 / 0
    except Exception:
        before = type(sys.exception()).__name__
        inner()
        after = sys.exception()
        return before, type(after).__name__ if after is not None else None
"""

    with soac_module(tmp_path, "try_finally_keeps_handled_exception", source) as module:
        assert module.value() == ("ZeroDivisionError", "ZeroDivisionError")


@BROAD_MODE_SLOW
def test_import_hook_broad_assert_raises_keeps_implicit_exception_context(tmp_path):
    script = r"""
from soac import import_hook

import_hook.install()

import ast
import unittest

case = unittest.TestCase()
try:
    1 / 0
except Exception:
    with case.assertRaises(SyntaxError) as caught:
        ast.literal_eval(r"'\U'")
    assert caught.exception.__context__ is not None
    assert type(caught.exception.__context__).__name__ == "ZeroDivisionError"
"""
    env = os.environ.copy()
    env.pop("SOAC_MODULE_ENABLED", None)
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env={**env, "SOAC_WORK_DIR": str(tmp_path / "assert-raises-context")},
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr


def test_soac_nested_closure_sees_rebound_argument(tmp_path):
    source = """
def f(value=None):
    value = "updated"

    def inner():
        return value

    return value, inner()
"""

    with soac_module(tmp_path, "nested_closure_rebound_argument", source) as module:
        assert module.f() == ("updated", "updated")


def test_soac_function_uses_replaced_code_object(tmp_path):
    source = """
def replacement():
    return 3

def target():
    pass

target.__code__ = replacement.__code__
VALUE = target()
"""

    with soac_module(tmp_path, "function_replaced_code", source) as module:
        assert module.VALUE == 3


def test_soac_function_dict_starts_empty(tmp_path):
    source = """
def value():
    pass

VALUE = value.__dict__
"""

    with soac_module(tmp_path, "function_dict_empty", source) as module:
        assert module.VALUE == {}


def test_soac_generic_function_exposes_type_params(tmp_path):
    source = """
import typing

def generic[T]():
    pass

T, = generic.__type_params__
VALUE = isinstance(T, typing.TypeVar), generic.__type_params__
"""

    with soac_module(tmp_path, "generic_function_type_params", source) as module:
        is_type_var, type_params = module.VALUE
        assert is_type_var is True
        assert type_params == (type_params[0],)


def test_soac_empty_closure_cell_raises_value_error(tmp_path):
    source = """
def value():
    def f():
        return a

    try:
        f.__closure__[0].cell_contents
    except ValueError:
        return "empty"
    else:
        return "filled"

    a = 12
"""

    with soac_module(tmp_path, "empty_closure_cell", source) as module:
        assert module.value() == "empty"


def test_soac_owned_cell_survives_jump_to_cell_backed_condition(tmp_path):
    source = """
def outer(reason):
    def decorator(test_item):
        return reason

    if isinstance(reason, int):
        return decorator(reason)
    return decorator

VALUE = outer("why")(object)
"""

    with soac_module(tmp_path, "owned_cell_condition", source) as module:
        assert module.VALUE == "why"


def test_soac_empty_cell_comparison_matches_cpython(tmp_path):
    source = """
def cell(value):
    def f():
        return a

    a = value
    return f.__closure__[0]

def empty_cell():
    def f():
        return a

    if False:
        a = 1729
    return f.__closure__[0]

VALUE = empty_cell() < cell("saturday")
"""

    with soac_module(tmp_path, "empty_cell_comparison", source) as module:
        assert module.VALUE is True


def test_soac_mutating_closure_cell_updates_function_and_outer(tmp_path):
    source = """
def value():
    a = 12

    def f():
        return a

    c = f.__closure__
    c[0].cell_contents = 9
    return c[0].cell_contents, f(), a
"""

    with soac_module(tmp_path, "mutating_closure_cell", source) as module:
        assert module.value() == (9, 9, 9)


def test_soac_deleted_closure_cell_raises_name_errors(tmp_path):
    source = """
def value():
    a = 12

    def f():
        return a

    cell = f.__closure__[0]
    del cell.cell_contents

    try:
        f()
    except NameError:
        inner = True
    else:
        inner = False

    try:
        a
    except UnboundLocalError:
        outer = True
    else:
        outer = False

    return inner, outer
"""

    with soac_module(tmp_path, "deleted_closure_cell", source) as module:
        assert module.value() == (True, True)


def test_soac_method_local_class_base_does_not_leak_to_outer_class(tmp_path):
    source = """
class Container:
    def method(self):
        class RawBase:
            pass

        class Derived(RawBase):
            pass

        return Derived.__mro__[1] is RawBase

VALUE = Container().method()
"""

    with soac_module(tmp_path, "method_local_class_base", source) as module:
        assert module.VALUE is True
