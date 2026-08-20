from __future__ import annotations

import json
from pathlib import Path
import textwrap

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import _VALIDATION_PRELUDE, create_strict_project


_FUNCTION_WITNESS = """
_native_function_id = ctypes.pythonapi.PyFunction_GetSoacFunctionId
_native_function_id.argtypes = [ctypes.py_object]
_native_function_id.restype = ctypes.c_uint64
_native_seal_id = ctypes.pythonapi.PyFunction_GetSoacStrictId
_native_seal_id.argtypes = [ctypes.py_object]
_native_seal_id.restype = ctypes.c_uint64

def _assert_selected_functions(module):
    diagnostic = _soac_ext.strict_module_diagnostics(module)
    assert diagnostic is not None, 'selected source executed as ordinary Python'
    assert diagnostic['sealed'] is True
    assert diagnostic['module_name'] == __EXPECTED_MODULE__
    assert diagnostic['source_path'] == __EXPECTED_SOURCE__
    assert diagnostic['artifact_generation'] == __EXPECTED_GENERATION__
    assert diagnostic['initializer_entry_kind'] == 'entry_interpreter'
    observed = []
    for name in ('pair', 'nested', 'enumerate_pairs', 'zip_pairs', 'source_once', 'assign_targets', 'mutate_during_assignment', 'starred', 'with_pair', 'direct_runtime_unpack_fixed'):
        function = _plain_function_witness(module, name)
        assert metadata(function), name
        actual_owner = owner(function)
        assert actual_owner, name
        function_id = _native_function_id(function)
        seal_id = _native_seal_id(function)
        # Sealing does not publish the optional unchecked direct-call ID.
        assert function_id == 0 and seal_id > 0, (name, function_id, seal_id)
        actual_entry = _soac_ext.strict_function_entry_kind(function)
        assert actual_entry == 'checked_native', (name, actual_entry)
        observed.append((name, actual_owner, function_id, seal_id))
    return tuple(observed)

def _assert_ordinary_functions(namespace):
    for name in ('pair', 'nested', 'enumerate_pairs', 'zip_pairs', 'source_once', 'assign_targets', 'mutate_during_assignment', 'starred', 'with_pair', 'direct_runtime_unpack_fixed'):
        function = namespace[name]
        assert owner(function) is None and metadata(function) is None, name
        assert _native_function_id(function) == 0, name
        assert _native_seal_id(function) == 0, name
"""


def test_fixed_sequence_unpacking_is_a_cpython_language_operation(
    tmp_path: Path,
) -> None:
    module_name = "fixed_sequence_unpacking_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            def pair(value):
                first, second = value
                return first, second


            def nested(value):
                first, (second, third) = value
                return first, second, third


            def enumerate_pairs(values):
                total = 0
                for index, value in enumerate(values):
                    total += index * value
                return total


            def zip_pairs(left, right):
                result = []
                for first, second in zip(left, right):
                    result.append(first + second)
                return result


            def source_once(factory):
                first, second = factory()
                return first, second


            def assign_targets(values, target):
                target.first, target.second = values


            def mutate_during_assignment(values, target):
                target.first, second = values
                return second


            def starred(values):
                first, *middle, last = values
                return first, middle, last


            def with_pair(manager):
                with manager as (first, second):
                    return first, second


            def direct_runtime_unpack_fixed(value):
                import soac.runtime as runtime

                return runtime.unpack_fixed(value, 2)
            """
        ),
        encoding="utf-8",
    )

    # Preserve the exact ordinary source; authenticate only the separate copy.
    relative = f"{module_name}.py"
    original_source = (tmp_path / relative).read_bytes()
    project = create_strict_project(
        tmp_path / "strict-project",
        {relative: strict_opt_in(original_source, relative)[0].decode()},
        modules={module_name: relative},
    )

    script = (
        textwrap.dedent(
            """
            import builtins
            import gc
            import json
            import sys

            from soac import _soac_ext

            import soac.runtime as runtime

            helper_calls = []
            for ordinary_helper in (runtime.unpack, runtime.unpack_fixed):
                assert owner(ordinary_helper) is None
                assert metadata(ordinary_helper) is None
                assert _native_function_id(ordinary_helper) == 0
                assert _native_seal_id(ordinary_helper) == 0
            original_unpack = runtime.unpack
            original_unpack_code = original_unpack.__code__


            def tracked_unpack(value, spec):
                helper_calls.append((type(value).__name__, len(spec)))
                return original_unpack(value, spec)


            runtime.unpack = tracked_unpack
            try:
                import MODULE_NAME_TOKEN as module
                source_owners = _assert_selected_functions(module)
                helper_calls.clear()

                results = {}
                assert module.pair((1, 2)) == (1, 2)
                assert module.pair([3, 4]) == (3, 4)
                assert module.nested((5, [6, 7])) == (5, 6, 7)
                assert module.enumerate_pairs((2, 3, 4)) == 11
                assert module.zip_pairs((1, 2), (10, 20)) == [11, 22]

                evaluated = []

                def make_source():
                    evaluated.append("source")
                    return (8, 9)

                assert module.source_once(make_source) == (8, 9)
                assert evaluated == ["source"]

                subclass_events = []

                class TupleOverride(tuple):
                    def __iter__(self):
                        subclass_events.append("tuple")
                        return builtins.iter((10, 11))

                class ListOverride(list):
                    def __iter__(self):
                        subclass_events.append("list")
                        return builtins.iter((12, 13))

                assert module.pair(TupleOverride((100, 200, 300))) == (10, 11)
                assert module.pair(ListOverride([100, 200, 300])) == (12, 13)
                assert subclass_events == ["tuple", "list"]

                class CountingIterator:
                    def __init__(self, values):
                        self.values = list(values)
                        self.index = 0
                        self.events = []

                    def __iter__(self):
                        self.events.append("iter")
                        return self

                    def __next__(self):
                        self.events.append("next")
                        if self.index == len(self.values):
                            raise StopIteration
                        value = self.values[self.index]
                        self.index += 1
                        return value

                iterator = CountingIterator((14, 15))
                assert module.pair(iterator) == (14, 15)
                assert iterator.events == ["iter", "next", "next", "next"]

                class CollectingIterator(CountingIterator):
                    def __next__(self):
                        gc.collect()
                        return super().__next__()

                collecting = CollectingIterator((14, 15))
                assert module.pair(collecting) == (14, 15)
                assert collecting.events == ["iter", "next", "next", "next"]

                context_events = []

                class PairContext:
                    def __enter__(self):
                        context_events.append("enter")
                        return (18, 19)

                    def __exit__(self, exc_type, exc_value, traceback):
                        context_events.append(("exit", exc_type is None))

                assert module.with_pair(PairContext()) == (18, 19)
                assert context_events == ["enter", ("exit", True)]

                assert helper_calls == [], {
                    "fixed_operations_must_not_call_runtime_unpack": helper_calls,
                    "subclass_events": subclass_events,
                    "iterator_events": iterator.events,
                }

                class MutatingTarget:
                    def __init__(self, values):
                        self.values = values
                        self.written = []

                    @property
                    def first(self):
                        return self.written[0]

                    @first.setter
                    def first(self, value):
                        self.written.append(value)
                        self.values[1] = 99

                values = [16, 17]
                target = MutatingTarget(values)
                assert module.mutate_during_assignment(values, target) == 17
                assert target.written == [16]
                assert values == [16, 99]

                class RecordingTarget:
                    def __init__(self):
                        object.__setattr__(self, "writes", [])

                    def __setattr__(self, name, value):
                        self.writes.append((name, value))

                untouched = RecordingTarget()
                try:
                    module.assign_targets((18,), untouched)
                except ValueError as error:
                    assert str(error) == (
                        "not enough values to unpack (expected 2, got 1)"
                    ), error
                else:
                    raise AssertionError("short sequences must raise ValueError")
                assert untouched.writes == []

                for value, expected in (
                    ((), "not enough values to unpack (expected 2, got 0)"),
                    ((1,), "not enough values to unpack (expected 2, got 1)"),
                    ((1, 2, 3), "too many values to unpack (expected 2, got 3)"),
                    ([1, 2, 3], "too many values to unpack (expected 2, got 3)"),
                ):
                    try:
                        module.pair(value)
                    except ValueError as error:
                        assert str(error) == expected, (value, str(error))
                    else:
                        raise AssertionError(("wrong arity did not fail", value))

                too_many = CountingIterator((1, 2, 3, 4))
                try:
                    module.pair(too_many)
                except ValueError as error:
                    assert str(error) == "too many values to unpack (expected 2)"
                else:
                    raise AssertionError("long iterator did not fail")
                assert too_many.events == ["iter", "next", "next", "next"]

                try:
                    module.pair(123)
                except TypeError as error:
                    assert str(error) == "cannot unpack non-iterable int object"
                else:
                    raise AssertionError("non-iterable input did not fail")

                class Marker(Exception):
                    pass

                class BrokenIterator:
                    def __iter__(self):
                        return self

                    def __next__(self):
                        raise Marker("broken iterator")

                try:
                    module.pair(BrokenIterator())
                except Marker as error:
                    assert str(error) == "broken iterator"
                else:
                    raise AssertionError("iterator failure did not propagate")

                released = []

                class PartialValue:
                    def __del__(self):
                        released.append("partial")

                class PartialFailure:
                    def __init__(self):
                        self.index = 0

                    def __iter__(self):
                        return self

                    def __next__(self):
                        gc.collect()
                        if self.index == 0:
                            self.index += 1
                            return PartialValue()
                        raise Marker("partial failure")

                try:
                    module.pair(PartialFailure())
                except Marker as error:
                    assert str(error) == "partial failure"
                else:
                    raise AssertionError("partial iterator failure did not propagate")
                assert released == ["partial"]

                original_unpack_fixed = runtime.unpack_fixed
                fixed_helper_calls = []

                def rebound_unpack_fixed(value, arity):
                    fixed_helper_calls.append((value, arity))
                    return "visible", arity

                runtime.unpack_fixed = rebound_unpack_fixed
                try:
                    assert module.pair((19, 20)) == (19, 20)
                    assert fixed_helper_calls == []
                    assert module.direct_runtime_unpack_fixed((19, 20)) == ("visible", 2)
                    assert fixed_helper_calls == [((19, 20), 2)]
                finally:
                    runtime.unpack_fixed = original_unpack_fixed

                before_explicit = len(helper_calls)
                assert runtime.unpack((20, 21), (True, True)) == (20, 21)
                assert len(helper_calls) == before_explicit + 1

                before_starred = len(helper_calls)
                assert module.starred((1, 2, 3, 4)) == (1, [2, 3], 4)
                assert len(helper_calls) == before_starred + 1

                def modified_unpack(value, spec):
                    return (80, 81)

                original_unpack.__code__ = modified_unpack.__code__
                try:
                    before_modified = len(helper_calls)
                    assert module.pair((22, 23)) == (22, 23)
                    assert len(helper_calls) == before_modified
                    assert runtime.unpack((1, 2), (True, True)) == (80, 81)
                    assert len(helper_calls) == before_modified + 1
                finally:
                    original_unpack.__code__ = original_unpack_code

                original_runtime_iter = runtime.iter
                original_runtime_next = runtime.next
                original_runtime_tuple = runtime.tuple

                def unexpected_runtime_global(*args, **kwargs):
                    raise AssertionError("fixed unpack consulted mutable runtime globals")

                runtime.iter = unexpected_runtime_global
                runtime.next = unexpected_runtime_global
                runtime.tuple = unexpected_runtime_global
                try:
                    before_mutated_globals = len(helper_calls)
                    assert module.pair((24, 25)) == (24, 25)
                    assert module.pair([26, 27]) == (26, 27)
                    assert module.pair(CountingIterator((28, 29))) == (28, 29)

                    previous_entry_mode = _soac_ext.force_entry_interpreter_for_tests(True)
                    try:
                        assert module.pair((30, 31)) == (30, 31)
                        assert module.pair([32, 33]) == (32, 33)
                        assert module.nested((34, (35, 36))) == (34, 35, 36)
                    finally:
                        _soac_ext.force_entry_interpreter_for_tests(previous_entry_mode)

                    assert len(helper_calls) == before_mutated_globals
                finally:
                    runtime.iter = original_runtime_iter
                    runtime.next = original_runtime_next
                    runtime.tuple = original_runtime_tuple

                results["helper_calls"] = helper_calls
                results["subclass_events"] = subclass_events
                results["iterator_events"] = iterator.events
                results["source_evaluations"] = evaluated
                assert _assert_selected_functions(module) == source_owners
                print(json.dumps(results))
            finally:
                original_unpack.__code__ = original_unpack_code
                runtime.unpack = original_unpack
            """
        )
        .replace("MODULE_NAME_TOKEN", module_name)
    )

    witness = (
        _FUNCTION_WITNESS.replace("__EXPECTED_MODULE__", repr(module_name))
        .replace("__EXPECTED_SOURCE__", repr(str(project.project / f"{module_name}.py")))
        .replace("__EXPECTED_GENERATION__", repr(project.publication["generation"]))
    )
    program = _VALIDATION_PRELUDE + witness + script

    for mode in ("profile", "apply"):
        completed = project.run(
            program,
            opt_mode=mode,
            extra_env={"SOAC_WORK_DIR": str(tmp_path / "soac-work")},
            timeout=90,
            check=False,
        )
        assert completed.returncode == 0, (
            f"{mode} fixed unpack must preserve CPython language semantics",
            completed.stdout,
            completed.stderr,
        )
        result = json.loads(completed.stdout.splitlines()[-1])
        assert len(result["helper_calls"]) == 3, result
        assert result["subclass_events"] == ["tuple", "list"], result
        assert result["iterator_events"] == ["iter", "next", "next", "next"], result
        assert result["source_evaluations"] == ["source"], result
