from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


_SOURCE = """
def invoke_one(callback, value):
    return callback(value)


def invoke_two(callback, first, second):
    return callback(first, second)


def invoke_keyword(callback, value, increment):
    return callback(value, increment=increment)


def ordinary_one(value):
    return value + 1


def ordinary_two(value, increment):
    return value + increment


class Owner:
    def __init__(self, bias):
        self.bias = bias

    def method(self, value):
        return self.bias + value

    def method_two(self, first, second):
        return self.bias + first + second


class CustomCallable:
    def __call__(self, value):
        return value + 30


def consume_any(values):
    return any(value for value in values)


def consume_all(values):
    return all(value for value in values)


def drive(value):
    return invoke_one(ordinary_one, value) + invoke_two(ordinary_two, value, 2)
"""


def _run_mode(
    tmp_path: Path, module_name: str, work_dir: Path, mode: str
) -> dict[str, object]:
    script = textwrap.dedent(
        """
        import builtins
        import importlib
        import json
        import sys

        root, name, mode = __ROOT__, __NAME__, __MODE__
        source = open(root + "/" + name + ".py", encoding="utf-8").read()
        stock = {
            "__name__": "stock_vectorcall_callable_kind_partition",
            "__builtins__": builtins.__dict__,
        }
        exec(compile(source, "<stock-vectorcall-callable-kind>", "exec"), stock)

        sys.path.insert(0, root)
        from soac.import_hook import install

        install()
        module = importlib.import_module(name)

        def exercise(namespace):
            invoke_one = namespace["invoke_one"]
            invoke_two = namespace["invoke_two"]
            owner = namespace["Owner"](10)
            return {
                "exact_one": invoke_one(namespace["ordinary_one"], 4),
                "exact_two": invoke_two(namespace["ordinary_two"], 4, 6),
                "bound_one": invoke_one(owner.method, 4),
                "bound_two": invoke_two(owner.method_two, 4, 6),
                "custom": invoke_one(namespace["CustomCallable"](), 9),
                "builtin_c_method": invoke_two({"present": 11}.get, "present", 0),
                "builtin_other": invoke_one(len, (1, 2, 3)),
                "next_one": invoke_one(builtins.next, iter(range(3))),
                "next_two": invoke_two(builtins.next, iter(range(3)), 99),
                "next_default": invoke_two(builtins.next, iter(range(0)), 99),
                "any_source_generator": namespace["consume_any"]((0, 1, 2)),
                "all_source_generator": namespace["consume_all"]((1, 1, 0)),
                "keyword": namespace["invoke_keyword"](
                    namespace["ordinary_two"], 5, 7
                ),
            }

        stock_outcomes = exercise(stock)
        transformed_outcomes = exercise(module.__dict__)
        assert transformed_outcomes == stock_outcomes, (
            transformed_outcomes,
            stock_outcomes,
        )

        for value in range(48):
            assert module.drive(value) == 2 * value + 3
            assert module.invoke_one(module.ordinary_one, value) == value + 1
            assert module.invoke_two(module.ordinary_two, value, 2) == value + 2

        print(json.dumps({"mode": mode, "outcomes": transformed_outcomes}))
        """
    )
    script = (
        script.replace("__ROOT__", repr(str(tmp_path)))
        .replace("__NAME__", repr(module_name))
        .replace("__MODE__", repr(mode))
    )
    environment = {
        **os.environ,
        "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
        "SOAC_WORK_DIR": str(work_dir),
        "SOAC_OPT_MODE": mode,
        "SOAC_COMPILE_MODE": "eager",
        "SOAC_BACKGROUND_JIT": "0",
    }
    completed = subprocess.run(
        [sys.executable, "-c", script],
        capture_output=True,
        check=False,
        env=environment,
        text=True,
        timeout=90,
    )
    assert completed.returncode == 0, (
        f"{mode} callable-kind partition worker failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    return json.loads(completed.stdout.splitlines()[-1])


def test_rebound_builtin_next_does_not_change_another_builtin() -> None:
    script = textwrap.dedent(
        """
        import builtins
        import ctypes
        import json

        import _soac_ext

        get_thread_state = ctypes.pythonapi.PyThreadState_Get
        get_thread_state.argtypes = []
        get_thread_state.restype = ctypes.c_void_p
        dispatch = ctypes.PyDLL(_soac_ext.__file__).dp_jit_py_vectorcall
        dispatch.argtypes = [ctypes.c_void_p] * 5
        dispatch.restype = ctypes.py_object

        def stock_outcome():
            try:
                return len(iter(range(3)))
            except TypeError:
                return "TypeError"

        expected = stock_outcome()
        original = builtins.next

        def dispatch_one(callback):
            iterator = iter(range(3))
            arguments = (ctypes.py_object * 1)(iterator)
            try:
                return dispatch(
                    get_thread_state(),
                    id(callback),
                    ctypes.addressof(arguments),
                    1,
                    None,
                )
            except TypeError:
                return "TypeError"

        try:
            builtins.next = len
            actual = dispatch_one(len)
            builtins.next = original
            restored = dispatch_one(original)
            builtins.next = len
            rebound = dispatch_one(len)
        finally:
            builtins.next = original

        print(
            json.dumps(
                {
                    "stock": expected,
                    "soac": actual,
                    "restored": restored,
                    "rebound": rebound,
                }
            )
        )
        """
    )
    completed = subprocess.run(
        [sys.executable, "-c", script],
        capture_output=True,
        check=False,
        text=True,
        timeout=30,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    outcomes = json.loads(completed.stdout.splitlines()[-1])
    assert outcomes["soac"] == outcomes["stock"] == "TypeError", outcomes
    assert outcomes["restored"] == 0, outcomes
    assert outcomes["rebound"] == "TypeError", outcomes


def test_vectorcall_callable_kind_partition_preserves_cpython_dispatch(
    tmp_path: Path,
) -> None:
    module_name = "vectorcall_callable_kind_partition_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(_SOURCE), encoding="utf-8"
    )
    work_dir = tmp_path / "soac-work"
    results = {
        mode: _run_mode(tmp_path, module_name, work_dir, mode)
        for mode in ("profile", "verify", "apply")
    }

    from soac import _soac_ext

    profile = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    records = [
        record
        for record in profile["records"]
        if record["module_name"] == module_name
    ]
    assert records, profile
    for function in ("drive", "invoke_one", "invoke_two"):
        assert any(
            row["kind"] == "call_hot_targets"
            and row["function_qualname"] == function
            and row["value"] >= 48
            for record in records
            for row in record["rows"]
        ), (function, records)

    native_rows = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text(
            encoding="utf-8"
        ).splitlines()
        if line.strip()
    ]
    for function in (
        "invoke_one",
        "invoke_two",
        "ordinary_one",
        "ordinary_two",
        "Owner.method",
        "Owner.method_two",
        "CustomCallable.__call__",
        "consume_any",
        "consume_all",
        "drive",
    ):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == function
            for row in native_rows
        ), (function, native_rows)

    for mode, result in results.items():
        outcomes = result["outcomes"]
        assert outcomes["any_source_generator"] is True, (mode, outcomes)
        assert outcomes["all_source_generator"] is False, (mode, outcomes)
        assert outcomes["next_default"] == 99, (mode, outcomes)
