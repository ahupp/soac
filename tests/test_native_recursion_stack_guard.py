from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


_SOURCE = """
def zero():
    return 7


def exact_one(value):
    return value + 1


def exact_two(value, increment):
    return value + increment


def defaulted(value, increment=3):
    return value + increment


def keyword_only(value, *, increment=4):
    return value + increment


def variadic(value, *remaining):
    return value + sum(remaining)


def nested(value):
    return exact_two(exact_one(value), 2)


def finite_recursion(depth):
    if depth == 0:
        return 0
    return exact_one(finite_recursion(depth - 1))


def invoke_c_callback(callback, value):
    return callback(value)


def retain(value, marker):
    return value


def fail(value, marker):
    raise RuntimeError("native trampoline failure")


def explicit_recursion_error(value):
    raise RecursionError("explicit native recursion error")


def drive(value):
    return exact_two(value, 1) + defaulted(value, 2) + nested(value)
"""


def _run_mode(
    tmp_path: Path, module_name: str, work_dir: Path, mode: str
) -> dict[str, object]:
    script = textwrap.dedent(
        """
        import _testinternalcapi
        import builtins
        import ctypes
        import gc
        import importlib
        import json
        import sys
        import threading

        root, name, mode = __ROOT__, __NAME__, __MODE__
        source = open(root + "/" + name + ".py", encoding="utf-8").read()
        stock = {"__name__": "stock_native_recursion_guard", "__builtins__": builtins.__dict__}
        exec(compile(source, "<stock-native-recursion-guard>", "exec"), stock)

        sys.path.insert(0, root)
        from soac.import_hook import install

        install()
        module = importlib.import_module(name)

        get_thread_state = ctypes.pythonapi.PyThreadState_GetUnchecked
        get_thread_state.argtypes = []
        get_thread_state.restype = ctypes.c_void_p

        get_vectorcall = ctypes.pythonapi.PyVectorcall_Function
        get_vectorcall.argtypes = [ctypes.py_object]
        get_vectorcall.restype = ctypes.c_void_p

        callback_type = ctypes.CFUNCTYPE(ctypes.c_ssize_t, ctypes.c_ssize_t)
        stack_margin = _testinternalcapi.get_stack_margin()
        assert stack_margin == 16 * 1024, stack_margin
        assert _testinternalcapi.get_c_recursion_remaining() > 0
        _testinternalcapi.test_threadstate_set_stack_protection()
        assert _testinternalcapi.get_c_recursion_remaining() > 0

        def exercise(namespace, *, transformed):
            main_thread_state = int(get_thread_state() or 0)
            main_stack_pointer = _testinternalcapi.get_stack_pointer()
            assert main_thread_state and main_stack_pointer

            exact_one = namespace["exact_one"]
            exact_two = namespace["exact_two"]
            defaulted = namespace["defaulted"]
            keyword_only = namespace["keyword_only"]
            variadic = namespace["variadic"]

            outcomes = {
                "zero": namespace["zero"](),
                "one": exact_one(8),
                "two": exact_two(8, 2),
                "nested": namespace["nested"](8),
                "finite_recursion": namespace["finite_recursion"](8),
                "default_omitted": defaulted(8),
                "default_supplied": defaulted(8, 6),
                "default_keyword": defaulted(8, increment=6),
                "keyword_only": keyword_only(8, increment=6),
                "keyword_only_default": keyword_only(8),
                "variadic": variadic(8, 2, 3),
            }

            exact_vectorcall = int(get_vectorcall(exact_two) or 0)
            default_vectorcall = int(get_vectorcall(defaulted) or 0)
            generic_vectorcall = int(get_vectorcall(keyword_only) or 0)
            variadic_vectorcall = int(get_vectorcall(variadic) or 0)
            assert all(
                (exact_vectorcall, default_vectorcall, generic_vectorcall, variadic_vectorcall)
            )
            if transformed:
                assert exact_vectorcall == default_vectorcall
                assert generic_vectorcall == variadic_vectorcall
                assert exact_vectorcall != generic_vectorcall

            callback_states = []

            @callback_type
            def nested_callback(value):
                callback_states.append(int(get_thread_state() or 0))
                return exact_one(value)

            outcomes["c_callback"] = namespace["invoke_c_callback"](
                nested_callback, 11
            )
            assert callback_states == [main_thread_state], callback_states

            events = []

            class OwnedArgument:
                def __del__(self):
                    events.append("finalized")

            owned = OwnedArgument()
            before = sys.getrefcount(owned)
            assert namespace["retain"](owned, None) is owned
            assert sys.getrefcount(owned) == before

            def capture_runtime_failure(value):
                try:
                    namespace["fail"](value, None)
                except RuntimeError as error:
                    error.__traceback__ = None
                    return type(error).__name__, str(error)
                raise AssertionError("native trampoline exception was lost")

            events.append(capture_runtime_failure(owned))
            assert sys.getrefcount(owned) == before
            del owned
            gc.collect()
            outcomes["owned_error_order"] = events

            argument_events = []

            class TemporaryArgument:
                def __del__(self):
                    argument_events.append("finalized")

            try:
                exact_two(TemporaryArgument())
            except TypeError:
                argument_events.append("caught")
            else:
                raise AssertionError("missing positional argument did not fail")
            gc.collect()
            outcomes["argument_error_order"] = argument_events

            try:
                namespace["explicit_recursion_error"](4)
            except RecursionError as error:
                outcomes["explicit_recursion_error"] = (
                    type(error).__name__,
                    str(error),
                )
            else:
                raise AssertionError("explicit RecursionError was lost")

            original_limit = sys.getrecursionlimit()
            try:
                sys.setrecursionlimit(96)
                assert sys.getrecursionlimit() == 96

                calls = 0

                def bounded_by_python_recursion_limit():
                    nonlocal calls
                    calls += 1
                    exact_one(calls)
                    return bounded_by_python_recursion_limit()

                try:
                    bounded_by_python_recursion_limit()
                except RecursionError:
                    outcomes["forced_recursion"] = calls > 3
                else:
                    raise AssertionError("Python recursion limit was ignored")

                outcomes["recovered_after_recursion"] = exact_two(19, 4)
                sys.setrecursionlimit(128)
                outcomes["changed_limit"] = (
                    sys.getrecursionlimit(),
                    namespace["finite_recursion"](10),
                )
            finally:
                sys.setrecursionlimit(original_limit)

            previous_profiler = sys.getprofile()

            def profiler(frame, event, argument):
                return None

            sys.setprofile(profiler)
            try:
                assert sys.getprofile() is profiler
                outcomes["profile_observer"] = namespace["nested"](12)
            finally:
                sys.setprofile(previous_profiler)

            barrier = threading.Barrier(3)
            worker_results = []
            worker_errors = []

            def thread_body(index):
                try:
                    thread_state = int(get_thread_state() or 0)
                    stack_pointer = _testinternalcapi.get_stack_pointer()
                    assert thread_state and stack_pointer
                    assert _testinternalcapi.get_stack_margin() == stack_margin
                    barrier.wait(timeout=15)

                    nested_states = []

                    @callback_type
                    def thread_callback(value):
                        nested_states.append(int(get_thread_state() or 0))
                        return exact_two(value, index)

                    actual = (
                        exact_one(index),
                        defaulted(index),
                        keyword_only(index, increment=6),
                        variadic(index, 2, 3),
                        namespace["finite_recursion"](4),
                        namespace["invoke_c_callback"](thread_callback, 10),
                    )
                    expected = (index + 1, index + 3, index + 6, index + 5, 4, 10 + index)
                    assert actual == expected, (actual, expected)
                    assert nested_states == [thread_state], nested_states
                    if transformed:
                        assert int(get_vectorcall(exact_two) or 0) == exact_vectorcall
                    worker_results.append((index, thread_state, stack_pointer))
                except BaseException as error:
                    worker_errors.append((type(error).__name__, str(error)))
                    barrier.abort()

            workers = [
                threading.Thread(target=thread_body, args=(index,))
                for index in (3, 7)
            ]
            for worker in workers:
                worker.start()
            try:
                barrier.wait(timeout=15)
            finally:
                for worker in workers:
                    worker.join(timeout=15)
            assert not worker_errors, worker_errors
            assert all(not worker.is_alive() for worker in workers)
            assert len(worker_results) == 2, worker_results
            assert len({main_thread_state, *(state for _, state, _ in worker_results)}) == 3
            assert len({main_stack_pointer, *(pointer for _, _, pointer in worker_results)}) == 3
            outcomes["live_thread_state"] = sorted(
                index for index, _, _ in worker_results
            )

            return outcomes

        stock_outcomes = exercise(stock, transformed=False)
        actual_outcomes = exercise(module.__dict__, transformed=True)
        assert actual_outcomes == stock_outcomes, (actual_outcomes, stock_outcomes)

        for value in range(40):
            assert module.drive(value) == 3 * value + 6

        print(json.dumps({"mode": mode, "outcomes": actual_outcomes}))
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
        f"{mode} native-recursion worker failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    return json.loads(completed.stdout.splitlines()[-1])


def test_native_recursion_stack_guard_preserves_cpython_calls_and_thread_state(
    tmp_path: Path,
) -> None:
    module_name = "native_recursion_stack_guard_case"
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
    assert any(
        row["kind"] == "call_hot_targets"
        and row["function_qualname"] == "drive"
        and row["value"] >= 40
        for record in records
        for row in record["rows"]
    ), records

    native_rows = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text(
            encoding="utf-8"
        ).splitlines()
        if line.strip()
    ]
    for function in (
        "exact_one",
        "exact_two",
        "defaulted",
        "keyword_only",
        "finite_recursion",
        "invoke_c_callback",
        "drive",
    ):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == function
            for row in native_rows
        ), (function, native_rows)

    assert all(
        result["outcomes"]["forced_recursion"] is True
        and result["outcomes"]["live_thread_state"] == [3, 7]
        for result in results.values()
    ), results
