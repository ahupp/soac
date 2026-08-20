from __future__ import annotations

import json
from pathlib import Path
import textwrap

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import (
    StrictValidationCase,
    _VALIDATION_PRELUDE,
    create_strict_project,
)


_SOURCE_FUNCTIONS = (
    "exact_a", "exact_b", "defaulted", "keyword_only", "variadic",
    "keyword_variadic", "identity", "explode", "replaceable", "replacement",
    "unary", "zero", "drive",
)


_MODULE_SOURCE = """
def exact_a(value, increment):
    return value + increment


def exact_b(value, increment):
    return value * increment


def defaulted(value, increment=1):
    return value + increment


def keyword_only(value, *, increment=1):
    return value + increment


def variadic(value, *remaining):
    return value + sum(remaining)


def keyword_variadic(value, **remaining):
    return value + remaining.get("increment", 0)


def identity(value, marker):
    return value


def explode(value, marker):
    raise RuntimeError("trampoline body failure")


def replaceable(value, increment):
    return value + increment


def replacement(value, increment):
    return value - increment


def unary(value):
    return value + 1


def zero():
    return 7


def drive(value):
    return exact_a(value, 1) + exact_b(value, 2) + defaulted(value, 3)
"""


def _run_vectorcall_worker(
    project, tmp_path: Path, module_name: str, work_dir: Path, mode: str
) -> dict:
    script = textwrap.dedent(
        """
        import builtins
        import ctypes
        import gc
        import importlib
        import json
        import sys
        import pytest
        from soac.strict import StrictMutationError

        root = __MODULE_ROOT__
        module_name = __MODULE_NAME__
        source = open(root + "/" + module_name + ".py", encoding="utf-8").read()
        stock = {
            "__name__": "stock_exact_positional_vectorcall",
            "__builtins__": builtins.__dict__,
        }
        exec(compile(source, "<stock-exact-positional-vectorcall>", "exec"), stock)

        module = importlib.import_module(module_name)

        get_vectorcall = ctypes.pythonapi.PyVectorcall_Function
        get_vectorcall.argtypes = [ctypes.py_object]
        get_vectorcall.restype = ctypes.c_void_p

        get_function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
        get_function_id.argtypes = [ctypes.py_object]
        get_function_id.restype = ctypes.c_uint64

        native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
        native_owner.argtypes = [ctypes.py_object]
        native_owner.restype = ctypes.c_void_p
        sealed_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
        sealed_id.argtypes = [ctypes.py_object]
        sealed_id.restype = ctypes.c_uint64

        def function_state(function):
            return (function, function.__code__, function.__defaults__,
                function.__kwdefaults__, native_owner(function),
                get_function_id(function), sealed_id(function))

        def unchanged(function, saved):
            assert function is saved[0] and function.__code__ is saved[1]
            assert function.__defaults__ is saved[2]
            assert function.__kwdefaults__ is saved[3]
            assert native_owner(function) == saved[4]
            assert get_function_id(function) == saved[5]
            assert sealed_id(function) == saved[6]

        source_functions = __SOURCE_FUNCTIONS__
        saved_functions = {}
        for name in source_functions:
            function = getattr(module, name)
            saved_functions[name] = function_state(function)
            assert native_owner(function) and get_function_id(function) == 0
            assert sealed_id(function) > 0
            assert native_owner(stock[name]) is None
            assert get_function_id(stock[name]) == sealed_id(stock[name]) == 0

        def rejects_mutation(function, callback):
            saved = function_state(function)
            with pytest.raises(StrictMutationError) as caught:
                callback()
            assert type(caught.value) is StrictMutationError
            unchanged(function, saved)

        pointer_names = (
            "exact_a",
            "exact_b",
            "defaulted",
            "keyword_only",
            "variadic",
            "keyword_variadic",
            "unary",
            "zero",
        )
        pointers = {
            name: int(get_vectorcall(getattr(module, name)) or 0)
            for name in pointer_names
        }
        assert all(pointers.values()), pointers
        assert pointers["exact_a"] == pointers["exact_b"]
        assert pointers["exact_a"] == pointers["defaulted"]
        assert pointers["keyword_only"] == pointers["variadic"]
        assert pointers["keyword_only"] == pointers["keyword_variadic"]
        assert pointers["unary"] != pointers["exact_a"]
        assert pointers["zero"] != pointers["unary"]

        vectorcall = ctypes.pythonapi.PyObject_Vectorcall
        vectorcall.argtypes = [
            ctypes.py_object,
            ctypes.POINTER(ctypes.py_object),
            ctypes.c_size_t,
            ctypes.c_void_p,
        ]
        vectorcall.restype = ctypes.py_object

        def call_with_offset(function):
            scratch = object()
            values = (ctypes.py_object * 3)(scratch, 9, 4)
            arguments = ctypes.cast(
                ctypes.byref(values, ctypes.sizeof(ctypes.py_object)),
                ctypes.POINTER(ctypes.py_object),
            )
            offset = 1 << (8 * ctypes.sizeof(ctypes.c_size_t) - 1)
            result = vectorcall(function, arguments, 2 | offset, None)
            assert values[0] is scratch
            return result

        def capture_error(call):
            try:
                call()
            except Exception as error:
                return type(error).__name__
            raise AssertionError("expected the vectorcall fallback to raise")

        def exercise(namespace, *, transformed):
            exact_a = namespace["exact_a"]
            exact_b = namespace["exact_b"]
            defaulted = namespace["defaulted"]
            keyword_only = namespace["keyword_only"]
            variadic = namespace["variadic"]
            keyword_variadic = namespace["keyword_variadic"]
            replaceable = namespace["replaceable"]

            outcomes = {
                "fully_supplied": exact_a(8, 4),
                "other_exact": exact_b(8, 4),
                "fully_supplied_default": defaulted(8, 4),
                "omitted_default": defaulted(8),
                "keyword_default": defaulted(8, increment=4),
                "keyword_only": keyword_only(8, increment=4),
                "keyword_only_default": keyword_only(8),
                "variadic": variadic(8, 4, 2),
                "keyword_variadic": keyword_variadic(8, increment=4),
                "offset_vectorcall": call_with_offset(exact_a),
                "missing_error": capture_error(lambda: exact_a(8)),
                "extra_error": capture_error(lambda: exact_a(8, 4, 2)),
                "duplicate_error": capture_error(
                    lambda: defaulted(8, 4, increment=2)
                ),
            }

            if transformed:
                # The selected strict contract seals code and both defaults
                # containers. Each operation must fail before its mutation.
                rejects_mutation(defaulted, lambda: setattr(defaulted, "__defaults__", (7,)))
                outcomes["replaced_default"] = defaulted(8)
                outcomes["supplied_after_default_replacement"] = defaulted(8, 4)
                rejects_mutation(keyword_only, lambda: setattr(keyword_only, "__kwdefaults__", {"increment": 6}))
                outcomes["replaced_kwdefault"] = keyword_only(8)
                original = dict(keyword_only.__kwdefaults__)
                rejects_mutation(keyword_only, lambda: keyword_only.__kwdefaults__.__setitem__("increment", 9))
                assert keyword_only.__kwdefaults__ == original
                outcomes["mutated_kwdefault"] = keyword_only(8)
                rejects_mutation(keyword_only, lambda: keyword_only.__kwdefaults__.__delitem__("increment"))
                assert keyword_only.__kwdefaults__ == original
                outcomes["deleted_kwdefault_error"] = "StrictMutationError"
                assert keyword_only(8) == 9
            else:
                defaulted.__defaults__ = (7,)
                outcomes["replaced_default"] = defaulted(8)
                outcomes["supplied_after_default_replacement"] = defaulted(8, 4)

                keyword_only.__kwdefaults__ = {"increment": 6}
                outcomes["replaced_kwdefault"] = keyword_only(8)
                keyword_only.__kwdefaults__["increment"] = 9
                outcomes["mutated_kwdefault"] = keyword_only(8)
                del keyword_only.__kwdefaults__["increment"]
                outcomes["deleted_kwdefault_error"] = capture_error(
                    lambda: keyword_only(8)
                )
                keyword_only.__kwdefaults__["increment"] = 1

            events = []

            class Lifetime:
                def __del__(self):
                    events.append("finalized")

            owned = Lifetime()
            original_refcount = sys.getrefcount(owned)
            assert namespace["identity"](owned, None) is owned
            assert sys.getrefcount(owned) == original_refcount
            assert (
                capture_error(lambda: namespace["explode"](owned, None))
                == "RuntimeError"
            )
            gc.collect()
            assert sys.getrefcount(owned) == original_refcount
            del owned
            gc.collect()
            assert events == ["finalized"], events
            outcomes["lifetime"] = events

            if transformed:
                assert get_function_id(replaceable) == 0
                assert sealed_id(replaceable) > 0
                rejects_mutation(replaceable, lambda: setattr(replaceable, "__code__", namespace["replacement"].__code__))
                outcomes["replaced_code"] = replaceable(8, 4)
            else:
                replaceable.__code__ = namespace["replacement"].__code__
                outcomes["replaced_code"] = replaceable(8, 4)

            return outcomes

        stock_outcomes = exercise(stock, transformed=False)
        soac_outcomes = exercise(module.__dict__, transformed=True)
        assert stock_outcomes["replaced_default"] == 15
        assert stock_outcomes["replaced_kwdefault"] == 14
        assert stock_outcomes["mutated_kwdefault"] == 17
        assert stock_outcomes["deleted_kwdefault_error"] == "TypeError"
        assert stock_outcomes["replaced_code"] == 4
        strict_expected = dict(stock_outcomes, replaced_default=9,
            replaced_kwdefault=9, mutated_kwdefault=9,
            deleted_kwdefault_error="StrictMutationError", replaced_code=12)
        assert soac_outcomes == strict_expected, (stock_outcomes, strict_expected, soac_outcomes)

        for name, saved in saved_functions.items():
            unchanged(getattr(module, name), saved)
            assert native_owner(stock[name]) is None
            assert get_function_id(stock[name]) == sealed_id(stock[name]) == 0

        for value in range(32):
            assert module.drive(value) == 4 * value + 4

        print(
            json.dumps(
                {
                    "mode": __MODE__,
                    "pointers": pointers,
                    "outcomes": soac_outcomes,
                }
            )
        )
        """
    )
    script = (
        script.replace("__MODULE_ROOT__", repr(str(tmp_path)))
        .replace("__MODULE_NAME__", repr(module_name))
        .replace("__MODE__", repr(mode))
        .replace("__SOURCE_FUNCTIONS__", repr(_SOURCE_FUNCTIONS))
    )
    validation = "def validate_module(module):\n" + textwrap.indent(script, "    ")
    program = _VALIDATION_PRELUDE + project._validation_program(
        module_name,
        StrictValidationCase(
            validation, Path(__file__), required_functions=_SOURCE_FUNCTIONS,
            
        ),
        entry_interpreter=False,
    )
    completed = project.run(
        program, opt_mode=mode, extra_env={"SOAC_WORK_DIR": str(work_dir)},
        timeout=90, check=False,
    )
    assert completed.returncode == 0, (
        f"{mode} transformed vectorcall subprocess failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    return json.loads(completed.stdout.splitlines()[-1])


def test_exact_positional_trampolines_preserve_cpython_fallbacks_and_mutations(
    tmp_path: Path,
) -> None:
    module_name = "exact_positional_vectorcall_trampoline_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(_MODULE_SOURCE), encoding="utf-8"
    )
    relative = f"{module_name}.py"
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in((tmp_path / relative).read_bytes(), relative)[0].decode()},
        modules={module_name: relative},
    )
    work_dir = tmp_path / "soac-work"
    results = {
        mode: _run_vectorcall_worker(project, tmp_path, module_name, work_dir, mode)
        for mode in ("profile", "verify", "apply")
    }

    from soac import _soac_ext

    profile = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    records = [
        record for record in profile["records"] if record["module_name"] == module_name
    ]
    assert any(
        row["kind"] == "call_hot_targets"
        and row["function_qualname"] == "drive"
        and row["value"] >= 32
        for record in records
        for row in record["rows"]
    ), records

    native = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text(
            encoding="utf-8"
        ).splitlines()
        if line.strip()
    ]
    for function_name in ("exact_a", "defaulted", "keyword_only", "drive"):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == function_name
            for row in native
        ), (function_name, native)
    assert any(
        row.get("entry_kind") == "default_direct_adapter"
        and row.get("function_qualname") == "defaulted"
        for row in native
    ), native

    for mode, result in results.items():
        assert result["pointers"]["exact_a"] != result["pointers"]["keyword_only"], (
            "fully supplied exact-positional functions must use an installed "
            "trampoline distinct from same-arity keyword-only or variadic functions",
            mode,
            result["pointers"],
        )
