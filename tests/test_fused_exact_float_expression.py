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

_PROFILE_FUNCTIONS = ('consume', 'under_call', 'under_power', 'inverse_distance', 'returned_tree', 'addition_tree', 'guarded_unbound', 'record', 'ordered', 'ordered_failure')


def test_profiled_exact_float_expression_trees_preserve_python_semantics(
    tmp_path: Path,
) -> None:
    module_name = "fused_exact_float_expression_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            EVENTS = []


            def consume(value):
                return value


            def under_call(first, second, third, fourth):
                return consume(first * second + third * fourth)


            def under_power(first, second, third):
                return (first * first + second * second + third * third) ** 0.5


            def inverse_distance(first, second, third, scale):
                return scale * (
                    first * first + second * second + third * third
                ) ** -1.5


            def returned_tree(first, second, third, fourth):
                return first * second + third * fourth


            def addition_tree(first, second, third):
                return (first + second) * third


            def guarded_unbound(first, second, bind):
                if bind:
                    missing = 4.0
                return first * second + missing * second


            def record(name, value):
                EVENTS.append(name)
                if value is None:
                    raise ValueError(name)
                return value


            def ordered():
                return consume(
                    record("first", 2.0) * record("second", 3.0)
                    + record("third", 4.0) * record("fourth", 5.0)
                )


            def ordered_failure():
                return consume(
                    record("first", 2.0) * record("second", 3.0)
                    + record("third", None) * record("fourth", 5.0)
                )


            class ObservableFloat(float):
                def __mul__(self, other):
                    EVENTS.append(("subclass_mul", float(self), other))
                    return 30.0


            class ReflectedMultiply:
                def __rmul__(self, other):
                    EVENTS.append(("reflected_mul", other))
                    return 7.0


            class ReflectedAddition:
                def __radd__(self, other):
                    EVENTS.append(("reflected_add", other))
                    return 11.0


            class RaisingFloat(float):
                def __mul__(self, other):
                    EVENTS.append(("raising_mul", float(self), other))
                    raise ValueError("first multiplication")
            """
        ),
        encoding="utf-8",
    )

    # Keep the original ordinary file for the stock control. Only the
    # separately analyzed copy carries the strict future and startup authority.
    relative = f"{module_name}.py"
    original_source = (tmp_path / relative).read_bytes()
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in(original_source, relative)[0].decode()},
        modules={module_name: relative},
    )

    script = textwrap.dedent(
        f"""
        import math
        import sys

        import {module_name} as module

        for index in range(12):
            assert module.under_call(2.0, 3.0, 4.0, 5.0) == 26.0
            assert module.under_power(3.0, 4.0, 0.0) == 5.0
            assert module.inverse_distance(3.0, 4.0, 0.0, 2.0) == 0.016
            assert module.returned_tree(2.0, 3.0, 4.0, 5.0) == 26.0
            assert module.addition_tree(2.0, 3.0, 4.0) == 20.0
            assert module.guarded_unbound(2.0, 3.0, True) == 18.0

        module.EVENTS.clear()
        assert module.under_call(module.ObservableFloat(2.0), 3.0, 4.0, 5.0) == 50.0
        assert module.EVENTS == [("subclass_mul", 2.0, 3.0)], module.EVENTS

        module.EVENTS.clear()
        assert module.under_call(2.0, module.ReflectedMultiply(), 4.0, 5.0) == 27.0
        assert module.EVENTS == [("reflected_mul", 2.0)], module.EVENTS

        module.EVENTS.clear()
        assert module.addition_tree(2.0, module.ReflectedAddition(), 3.0) == 33.0
        assert module.EVENTS == [("reflected_add", 2.0)], module.EVENTS

        module.EVENTS.clear()
        try:
            module.guarded_unbound(module.RaisingFloat(2.0), 3.0, False)
        except ValueError as error:
            assert str(error) == "first multiplication", error
        else:
            raise AssertionError("earlier multiplication must precede unbound later operand")
        assert module.EVENTS == [("raising_mul", 2.0, 3.0)], module.EVENTS

        assert module.under_call(2.0, 3, 4.0, 5.0) == 26.0
        assert module.returned_tree(2, 3.0, 4.0, 5) == 26.0

        assert math.isnan(module.returned_tree(math.nan, 1.0, 2.0, 3.0))
        assert math.isnan(module.returned_tree(math.inf, 0.0, 2.0, 3.0))
        assert module.returned_tree(math.inf, 1.0, 2.0, 3.0) == math.inf

        negative_zero = module.returned_tree(-0.0, 1.0, -0.0, 1.0)
        assert negative_zero == 0.0
        assert math.copysign(1.0, negative_zero) == -1.0

        first_rounded = 1.0 + 2.0 ** -27
        second_rounded = 1.0 - 2.0 ** -27
        separately_rounded = module.returned_tree(
            first_rounded, second_rounded, -1.0, 1.0
        )
        assert separately_rounded == 0.0, separately_rounded
        assert math.copysign(1.0, separately_rounded) == 1.0

        try:
            module.inverse_distance(0.0, 0.0, 0.0, 1.0)
        except ZeroDivisionError:
            pass
        else:
            raise AssertionError("negative float power must preserve ZeroDivisionError")

        module.EVENTS.clear()
        assert module.ordered() == 26.0
        assert module.EVENTS == ["first", "second", "third", "fourth"], module.EVENTS

        module.EVENTS.clear()
        try:
            module.ordered_failure()
        except ValueError as error:
            assert str(error) == "third", error
        else:
            raise AssertionError("operand exception must stop expression evaluation")
        assert module.EVENTS == ["first", "second", "third"], module.EVENTS
        """
    )

    work_dir = tmp_path / "soac-work"
    witnesses = f"""
import ctypes
from tests._strict_integration import _plain_function_witness
function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
function_id.argtypes = [ctypes.py_object]
function_id.restype = ctypes.c_uint64
sealed_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
sealed_id.argtypes = [ctypes.py_object]
sealed_id.restype = ctypes.c_uint64
native_owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
native_owner.argtypes = [ctypes.py_object]
native_owner.restype = ctypes.c_void_p
def assert_profile_functions():
    for path in {_PROFILE_FUNCTIONS!r}:
        function = _plain_function_witness(module, path)
        # The old ID grants unchecked dispatch, not source admission.
        assert function_id(function) == 0, path
        assert sealed_id(function) > 0, path
        assert native_owner(function), path
assert_profile_functions()
"""
    validation = "def validate_module(module):\n" + textwrap.indent(
        witnesses + script + "\nassert_profile_functions()\n", "    "
    )
    program = _VALIDATION_PRELUDE + project._validation_program(
        module_name,
        StrictValidationCase(
            validation, Path(__file__), required_functions=_PROFILE_FUNCTIONS,
            
        ),
        entry_interpreter=False,
    )

    profile = project.run(
        program, opt_mode="profile", extra_env={"SOAC_WORK_DIR": str(work_dir)},
        timeout=60, check=False,
    )
    assert profile.returncode == 0, profile.stdout + profile.stderr

    from soac import _soac_ext

    counter_dump = json.loads(
        _soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin"))
    )
    tree_functions = {
        "under_call",
        "under_power",
        "inverse_distance",
        "returned_tree",
        "guarded_unbound",
    }
    operator_rows = [
        row
        for record in counter_dump["records"]
        if record["module_name"] == module_name
        for row in record["rows"]
        if row["function_qualname"] in tree_functions
        and row["kind"] == "operator_hot_shapes"
    ]
    exact_float_rows = [
        row
        for row in operator_rows
        if row.get("observed_value") == 0x0303 and row["value"] >= 8
    ]
    covered_functions = {row["function_qualname"] for row in exact_float_rows}
    assert tree_functions <= covered_functions, {
        "required_exact_float_pair_shape": 0x0303,
        "covered_functions": sorted(covered_functions),
        "observed_operator_shapes": [
            {
                "function": row["function_qualname"],
                "instr_id": row["instr_id"],
                "shape": row.get("observed_value"),
                "count": row["value"],
            }
            for row in operator_rows
        ],
    }
    assert all(
        sum(row["function_qualname"] == function for row in exact_float_rows) >= 3
        for function in tree_functions
    ), exact_float_rows

    apply = project.run(
        program, opt_mode="apply", extra_env={"SOAC_WORK_DIR": str(work_dir)},
        timeout=60, check=False,
    )
    assert apply.returncode == 0, apply.stdout + apply.stderr
