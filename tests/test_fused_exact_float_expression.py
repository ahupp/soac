from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


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

    script = textwrap.dedent(
        f"""
        import math
        import sys

        sys.path.insert(0, {str(tmp_path)!r})
        from soac.import_hook import install
        install()
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
    base_env = {
        **os.environ,
        "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
        "SOAC_WORK_DIR": str(work_dir),
        "SOAC_COMPILE_MODE": "eager",
        "SOAC_BACKGROUND_JIT": "0",
    }

    profile = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env={**base_env, "SOAC_OPT_MODE": "profile"},
        timeout=60,
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

    apply = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env={**base_env, "SOAC_OPT_MODE": "apply"},
        timeout=60,
    )
    assert apply.returncode == 0, apply.stdout + apply.stderr
