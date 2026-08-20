from __future__ import annotations

import ast
import json
from pathlib import Path
import textwrap

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._strict_integration import (
    StrictValidationCase,
    _VALIDATION_PRELUDE,
    assert_strict_source_rejected,
    create_strict_project,
)

_PROFILE_FUNCTIONS = ('Base.observe', 'Base.observe_positional', 'Base.observe_two', 'Base.value', 'Base.echo', 'Base.explode', 'Child.super_value', 'Child.super_echo', 'immediate', 'positional', 'two_positional', 'builtin_positional', 'captured_builtin_positional', 'nested_builtin_positional', 'stored_builtin', 'stored', 'super_call', 'super_positional', 'descriptor_before_unbound', 'argument_effect', 'effectful_argument', 'keyword_argument', 'starred_argument', 'missing', 'failed', 'value', 'temporary', 'temporary_positional', 'hot')


_SOURCE = """
import gc
import types


class Base:
    def observe(self):
        for reference in gc.get_referrers(self):
            if (
                isinstance(reference, types.MethodType)
                and reference.__self__ is self
                and reference.__func__ is Base.observe
            ):
                return True
        return False

    def observe_positional(self, value):
        for reference in gc.get_referrers(self):
            if (
                isinstance(reference, types.MethodType)
                and reference.__self__ is self
                and reference.__func__ is Base.observe_positional
            ):
                return True
        return False

    def observe_two(self, first, second):
        for reference in gc.get_referrers(self):
            if (
                isinstance(reference, types.MethodType)
                and reference.__self__ is self
                and reference.__func__ is Base.observe_two
            ):
                return True
        return False

    def value(self):
        return "base"

    def echo(self, value):
        return value

    def explode(self):
        raise RuntimeError("method failed")


class Middle(Base):
    pass


class Child(Middle):
    def super_value(self):
        return super().value()

    def super_echo(self, value):
        return super().echo(value)


class DataDescriptor:
    def __get__(self, instance, owner):
        return lambda: "data descriptor"

    def __set__(self, instance, value):
        instance.__dict__["ignored"] = value


class DescriptorChild(Child):
    observe = DataDescriptor()


class PropertyChild(Child):
    @property
    def observe(self):
        return lambda: "property"


class CustomChild(Child):
    def __getattribute__(self, name):
        if name == "observe":
            return lambda: "custom attribute"
        return object.__getattribute__(self, name)


class StaticChild(Child):
    @staticmethod
    def observe():
        return "static"


class ClassChild(Child):
    @classmethod
    def observe(cls):
        return cls.__name__


class RaisingDescriptor:
    def __get__(self, instance, owner):
        raise RuntimeError("descriptor failed")


class RaisingChild(Child):
    observe = RaisingDescriptor()


class LookupDescriptor:
    def __get__(self, instance, owner):
        instance.events.append("lookup")
        return lambda value: value


class LookupChild:
    echo = LookupDescriptor()


class ProbeKey:
    def __init__(self, mapping, seen):
        self.mapping = mapping
        self.seen = seen

    def __hash__(self):
        self.seen.append(
            any(
                isinstance(reference, types.BuiltinMethodType)
                and reference.__self__ is self.mapping
                and reference.__name__ == "get"
                for reference in gc.get_referrers(self.mapping)
            )
        )
        return hash("present")

    def __eq__(self, other):
        return other == "present"


class Ephemeral:
    def __init__(self, events):
        self.events = events

    def value(self):
        self.events.append("called")
        return "lifetime"

    def echo(self, value):
        self.events.append("called:" + value)
        return value

    def __del__(self):
        self.events.append("finalized")


def immediate(owner):
    return owner.observe()


def positional(owner, value):
    return owner.observe_positional(value)


def two_positional(owner, first, second):
    return owner.observe_two(first, second)


def builtin_positional(mapping, key):
    return mapping.get(key)


def captured_builtin_positional(mapping, key):
    def inner(value):
        return mapping.get(value)

    return inner(key)


def nested_builtin_positional(mapping, key):
    return [mapping.get(value) for value in (key,)][0]


def stored_builtin(mapping, key):
    bound = mapping.get
    return bound(key)


def stored(owner):
    bound = owner.observe
    return bound()


def super_call(owner):
    return owner.super_value()


def super_positional(owner, value):
    return owner.super_echo(value)


def descriptor_before_unbound(owner, available):
    if available:
        argument = "available"
    return owner.echo(argument)


def argument_effect(events):
    events.append("argument")
    return "effect"


def effectful_argument(owner, events):
    return owner.echo(argument_effect(events))


def keyword_argument(owner, value):
    return owner.echo(value=value)


def starred_argument(owner, values):
    return owner.echo(*values)


def missing(owner):
    return owner.missing()


def failed(owner):
    return owner.explode()


def value(owner):
    return owner.value()


def temporary(events):
    return Ephemeral(events).value()


def temporary_positional(events, value):
    return Ephemeral(events).echo(value)


def hot(owner):
    return owner.value()
"""


def _run_worker(project, tmp_path: Path, module_name: str, work_dir: Path, mode: str) -> dict:
    script = textwrap.dedent(
        """
        import builtins
        import gc
        import importlib
        import json
        import sys
        import weakref

        root, name = __ROOT__, __NAME__
        source = open(root + "/" + name + ".py", encoding="utf-8").read()
        stock = {"__name__": "stock_immediate_methods", "__builtins__": builtins.__dict__}
        exec(compile(source, "<stock-immediate-methods>", "exec"), stock)
        assert all(function_id(stock[path]) == 0 and sealed_id(stock[path]) == 0
            and native_owner(stock[path]) is None for path in ('immediate', 'positional', 'two_positional', 'builtin_positional', 'captured_builtin_positional', 'nested_builtin_positional', 'stored_builtin', 'stored', 'super_call', 'super_positional', 'descriptor_before_unbound', 'argument_effect', 'effectful_argument', 'keyword_argument', 'starred_argument', 'missing', 'failed', 'value', 'temporary', 'temporary_positional', 'hot'))

        module = importlib.import_module(name)

        def capture_error(callback):
            try:
                callback()
            except Exception as error:
                return type(error).__name__, str(error)
            raise AssertionError("expected method dispatch to raise")

        def exercise(namespace):
            child = namespace["Child"]()
            direct = namespace["immediate"](child)

            observations = {
                "python_positional": namespace["positional"](child, 1),
                "python_two_positional": namespace["two_positional"](child, 1, 2),
            }

            def observe_builtin(function):
                mapping, seen = {"present": 7}, []
                key = namespace["ProbeKey"](mapping, seen)
                assert function(mapping, key) == 7
                assert len(seen) == 1, seen
                return seen[0]

            observations["builtin_positional"] = observe_builtin(
                namespace["builtin_positional"]
            )
            observations["captured_builtin_positional"] = observe_builtin(
                namespace["captured_builtin_positional"]
            )
            observations["nested_builtin_positional"] = observe_builtin(
                namespace["nested_builtin_positional"]
            )

            bound = child.observe
            standalone = bound()
            del bound

            shadowed = namespace["Child"]()
            shadowed.observe = lambda: "instance shadow"

            outcomes = {
                "standalone": standalone,
                "stored": namespace["stored"](child),
                "super": namespace["super_call"](child),
                "shadow": namespace["immediate"](shadowed),
                "data_descriptor": namespace["immediate"](
                    namespace["DescriptorChild"]()
                ),
                "property": namespace["immediate"](namespace["PropertyChild"]()),
                "custom_attribute": namespace["immediate"](
                    namespace["CustomChild"]()
                ),
                "static": namespace["immediate"](namespace["StaticChild"]()),
                "classmethod": namespace["immediate"](namespace["ClassChild"]()),
                "missing": capture_error(lambda: namespace["missing"](child)),
                "raising_descriptor": capture_error(
                    lambda: namespace["immediate"](namespace["RaisingChild"]())
                ),
                "raising_method": capture_error(lambda: namespace["failed"](child)),
                "super_positional": namespace["super_positional"](child, "super"),
                "keyword": namespace["keyword_argument"](child, "keyword"),
                "starred": namespace["starred_argument"](child, ("starred",)),
                "stored_builtin": observe_builtin(namespace["stored_builtin"]),
            }

            ordering = namespace["LookupChild"]()
            ordering.events = []
            error = capture_error(
                lambda: namespace["descriptor_before_unbound"](ordering, False)
            )
            outcomes["lookup_before_unbound"] = (error[0], ordering.events[:])

            ordering.events.clear()
            assert namespace["effectful_argument"](ordering, ordering.events) == "effect"
            outcomes["effectful_order"] = ordering.events[:]

            original = namespace["Middle"].__dict__.get("value")
            namespace["Middle"].value = lambda self: "mutated inherited"
            try:
                outcomes["mutated_mro"] = namespace["value"](child)
            finally:
                if original is None:
                    del namespace["Middle"].value
                else:
                    namespace["Middle"].value = original

            events = []
            assert namespace["temporary"](events) == "lifetime"
            gc.collect()
            outcomes["receiver_lifetime"] = events

            positional_events = []
            assert namespace["temporary_positional"](positional_events, "positional") == (
                "positional"
            )
            gc.collect()
            outcomes["positional_receiver_lifetime"] = positional_events

            failing = namespace["Child"]()
            reference = weakref.ref(failing)
            assert capture_error(lambda: namespace["failed"](failing))[0] == "RuntimeError"
            del failing
            gc.collect()
            outcomes["exception_receiver_released"] = reference() is None

            return direct, observations, outcomes

        def exercise_strict(namespace):
            child = namespace["Child"]()
            direct = namespace["immediate"](child)

            observations = {
                "python_positional": namespace["positional"](child, 1),
                "python_two_positional": namespace["two_positional"](child, 1, 2),
            }

            def observe_builtin(function):
                mapping, seen = {"present": 7}, []
                key = namespace["ProbeKey"](mapping, seen)
                assert function(mapping, key) == 7
                assert len(seen) == 1, seen
                return seen[0]

            observations["builtin_positional"] = observe_builtin(
                namespace["builtin_positional"]
            )
            observations["captured_builtin_positional"] = observe_builtin(
                namespace["captured_builtin_positional"]
            )
            observations["nested_builtin_positional"] = observe_builtin(
                namespace["nested_builtin_positional"]
            )

            bound = child.observe
            standalone = bound()
            del bound

            shadowed = namespace["Child"]()
            original_observe = function_snapshot(namespace["Base"].observe)
            child_type = type_snapshot(namespace["Child"])
            with pytest.raises(StrictMutationError) as caught:
                shadowed.observe = lambda: "instance shadow"
            assert type(caught.value) is StrictMutationError
            assert "observe" not in vars(shadowed)
            assert_function_snapshot(namespace["Base"].observe, original_observe)
            assert_type_snapshot(namespace["Child"], child_type)
            sealed_rejections = ["instance_method_shadow"]

            outcomes = {
                "standalone": standalone,
                "stored": namespace["stored"](child),
                "super": namespace["super_call"](child),
                "shadow": namespace["immediate"](shadowed),
                "data_descriptor": namespace["immediate"](
                    namespace["DescriptorChild"]()
                ),
                "property": namespace["immediate"](namespace["PropertyChild"]()),
                "custom_attribute": namespace["immediate"](
                    namespace["CustomChild"]()
                ),
                "static": namespace["immediate"](namespace["StaticChild"]()),
                "classmethod": namespace["immediate"](namespace["ClassChild"]()),
                "missing": capture_error(lambda: namespace["missing"](child)),
                "raising_descriptor": capture_error(
                    lambda: namespace["immediate"](namespace["RaisingChild"]())
                ),
                "raising_method": capture_error(lambda: namespace["failed"](child)),
                "super_positional": namespace["super_positional"](child, "super"),
                "keyword": namespace["keyword_argument"](child, "keyword"),
                "starred": namespace["starred_argument"](child, ("starred",)),
                "stored_builtin": observe_builtin(namespace["stored_builtin"]),
            }

            ordering = namespace["LookupChild"]()
            ordering.events = []
            error = capture_error(
                lambda: namespace["descriptor_before_unbound"](ordering, False)
            )
            outcomes["lookup_before_unbound"] = (error[0], ordering.events[:])

            ordering.events.clear()
            assert namespace["effectful_argument"](ordering, ordering.events) == "effect"
            outcomes["effectful_order"] = ordering.events[:]

            original = namespace["Middle"].__dict__.get("value")
            middle_type = type_snapshot(namespace["Middle"])
            base_type = type_snapshot(namespace["Base"])
            original_value = function_snapshot(namespace["Base"].value)
            with pytest.raises(StrictMutationError) as caught:
                namespace["Middle"].value = lambda self: "mutated inherited"
            assert type(caught.value) is StrictMutationError
            assert namespace["Middle"].__dict__.get("value") is original
            assert_function_snapshot(namespace["Base"].value, original_value)
            assert_type_snapshot(namespace["Middle"], middle_type)
            sealed_rejections.append("class_method_replace")
            with pytest.raises(StrictMutationError) as caught:
                del namespace["Base"].value
            assert type(caught.value) is StrictMutationError
            assert_function_snapshot(namespace["Base"].value, original_value)
            assert_type_snapshot(namespace["Base"], base_type)
            sealed_rejections.append("class_method_delete")
            outcomes["mutated_mro"] = namespace["value"](child)

            events = []
            assert namespace["temporary"](events) == "lifetime"
            gc.collect()
            outcomes["receiver_lifetime"] = events

            positional_events = []
            assert namespace["temporary_positional"](positional_events, "positional") == (
                "positional"
            )
            gc.collect()
            outcomes["positional_receiver_lifetime"] = positional_events

            failing = namespace["Child"]()
            reference = weakref.ref(failing)
            assert capture_error(lambda: namespace["failed"](failing))[0] == "RuntimeError"
            del failing
            gc.collect()
            outcomes["exception_receiver_released"] = reference() is None

            outcomes["sealed_rejections"] = sealed_rejections
            return direct, observations, outcomes

        stock_direct, stock_observations, stock_outcomes = exercise(stock)
        assert all(function_id(stock[path]) == 0 and sealed_id(stock[path]) == 0
            and native_owner(stock[path]) is None for path in ('immediate', 'positional', 'two_positional', 'builtin_positional', 'captured_builtin_positional', 'nested_builtin_positional', 'stored_builtin', 'stored', 'super_call', 'super_positional', 'descriptor_before_unbound', 'argument_effect', 'effectful_argument', 'keyword_argument', 'starred_argument', 'missing', 'failed', 'value', 'temporary', 'temporary_positional', 'hot'))
        soac_direct, soac_observations, soac_outcomes = exercise_strict(module.__dict__)
        assert stock_direct is False, stock_direct
        assert stock_outcomes["standalone"] is True, stock_outcomes
        assert stock_outcomes["stored"] is True, stock_outcomes
        assert stock_outcomes["super"] == "base", stock_outcomes
        assert stock_outcomes["super_positional"] == "super", stock_outcomes
        assert stock_outcomes["stored_builtin"] is True, stock_outcomes
        assert stock_outcomes["lookup_before_unbound"] == (
            "UnboundLocalError", ["lookup"]
        ), stock_outcomes
        assert stock_outcomes["effectful_order"] == ["lookup", "argument"]
        assert stock_outcomes["receiver_lifetime"] == ["called", "finalized"]
        assert stock_outcomes["positional_receiver_lifetime"] == [
            "called:positional", "finalized"
        ]
        assert stock_outcomes["exception_receiver_released"]
        # Only these named mutations differ under the documented seal.
        assert stock_outcomes["shadow"] == "instance shadow"
        assert stock_outcomes["mutated_mro"] == "mutated inherited"
        strict_expected = dict(
            stock_outcomes, shadow=False, mutated_mro="base",
            sealed_rejections=["instance_method_shadow", "class_method_replace", "class_method_delete"],
        )
        assert soac_outcomes == strict_expected, (stock_outcomes, strict_expected, soac_outcomes)

        for _ in range(32):
            assert module.hot(module.Child()) == "base"

        print(json.dumps({"mode": __MODE__, "stock_direct": stock_direct,
            "soac_direct": soac_direct, "stock_observations": stock_observations,
            "soac_observations": soac_observations, "outcomes": soac_outcomes}))
        """
    )
    script = (
        script.replace("__ROOT__", repr(str(tmp_path)))
        .replace("__NAME__", repr(module_name))
        .replace("__MODE__", repr(mode))
    )
    witnesses = f"""
import ctypes
import pytest
from soac.strict import StrictMutationError
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
native_metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
native_metadata.argtypes = [ctypes.py_object]
native_metadata.restype = ctypes.c_void_p
type_owner = ctypes.pythonapi.PyType_GetSoacContractOwner
type_owner.argtypes = [ctypes.py_object]
type_owner.restype = ctypes.c_void_p
type_sealed = ctypes.pythonapi.PyType_IsSoacSealed
type_sealed.argtypes = [ctypes.py_object]
type_sealed.restype = ctypes.c_int
def assert_profile_functions():
    for path in {_PROFILE_FUNCTIONS!r}:
        function = _plain_function_witness(module, path)
        # The old ID grants unchecked dispatch, not source admission.
        assert function_id(function) == 0, path
        assert sealed_id(function) > 0, path
        assert native_owner(function), path
    # This deliberately incompatible override is an ordinary dependency, not
    # an admitted strict class or an unchecked method target.
    override = module.StaticChild.__dict__["observe"].__func__
    assert type_owner(module.StaticChild) is None
    assert function_id(override) == sealed_id(override) == 0
    assert native_owner(override) is None
    assert native_metadata(override) is None
def function_snapshot(function):
    assert function_id(function) == 0 and sealed_id(function) > 0
    assert native_owner(function)
    return (function, function.__code__, function.__defaults__, function.__kwdefaults__,
        native_owner(function), function_id(function), sealed_id(function))
def assert_function_snapshot(function, saved):
    assert function is saved[0] and function.__code__ is saved[1]
    assert function.__defaults__ is saved[2] and function.__kwdefaults__ is saved[3]
    assert native_owner(function) == saved[4]
    assert function_id(function) == saved[5] and sealed_id(function) == saved[6]
def type_snapshot(cls):
    owner = type_owner(cls)
    assert owner and type_sealed(cls) == 1
    return (cls, owner)
def assert_type_snapshot(cls, saved):
    assert cls is saved[0] and type_owner(cls) == saved[1]
    assert type_sealed(cls) == 1
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
    completed = project.run(
        program, opt_mode=mode, extra_env={"SOAC_WORK_DIR": str(work_dir)},
        timeout=90, check=False,
    )
    assert completed.returncode == 0, (
        f"{mode} transformed immediate-method subprocess failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    return json.loads(completed.stdout.splitlines()[-1])


def test_incompatible_static_method_override_rejects_strict_source(
    tmp_path: Path,
) -> None:
    # An instance-to-static override with a different signature is ordinary
    # Python, but is not valid strict source. Do not suppress the ty diagnostic.
    relative = "incompatible_static_override.py"
    assert_strict_source_rejected(
        tmp_path / "rejected-source",
        strict_opt_in(_SOURCE.encode(), relative)[0].decode(),
        module_name="incompatible_static_override",
        diagnostic="invalid-method-override",
    )


def test_immediate_method_call_preserves_cpython_descriptor_dispatch(
    tmp_path: Path,
) -> None:
    module_name = "immediate_method_call_dispatch_case"
    original = textwrap.dedent(_SOURCE)
    (tmp_path / f"{module_name}.py").write_text(original)
    # Keep the complete original stock source and validator. The strict
    # rejection above covers its incompatible override. This run exercises
    # the same dispatch through that one exact class in an ordinary module;
    # all remaining source functions/classes are selected normally.
    override_name = f"{module_name}_ordinary_override"
    override = next(
        node for node in ast.parse(original).body
        if isinstance(node, ast.ClassDef) and node.name == "StaticChild"
    )
    lines = original.splitlines(keepends=True)
    override_source = "".join(lines[override.lineno - 1:override.end_lineno])
    selected_source = "".join(
        lines[:override.lineno - 1]
        + [f"from {override_name} import StaticChild\n"]
        + ["\n"] * (override.end_lineno - override.lineno)
        + lines[override.end_lineno:]
    )
    relative = f"{module_name}.py"
    project = create_strict_project(
        tmp_path / "strict-project",
        {
            relative: strict_opt_in(selected_source.encode(), relative)[0].decode(),
            f"{override_name}.py": f"from {module_name} import Child\n\n{override_source}",
        },
        modules={module_name: relative},
    )

    work_dir = tmp_path / "soac-work"
    results = {
        mode: _run_worker(project, tmp_path, module_name, work_dir, mode)
        for mode in ("profile", "verify", "apply")
    }

    from soac import _soac_ext

    profile = json.loads(_soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin")))
    records = [record for record in profile["records"] if record["module_name"] == module_name]
    assert any(
        row["kind"] == "call_hot_targets"
        and row["function_qualname"] == "hot"
        and row["value"] >= 32
        for record in records
        for row in record["rows"]
    ), records

    native = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text().splitlines()
        if line.strip()
    ]
    for name in (
        "Base.observe",
        "Base.observe_positional",
        "immediate",
        "positional",
        "captured_builtin_positional.<locals>.inner",
        "nested_builtin_positional",
        "stored",
        "hot",
    ):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == name
            for row in native
        ), (name, native)
    for mode, result in results.items():
        assert result["stock_direct"] is False, (mode, result)
        assert result["soac_direct"] is False, (
            "an immediate inherited method call must not expose a temporary "
            "bound-method wrapper to the callee",
            mode,
            result,
        )
        assert result["stock_observations"] == {
            "python_positional": False,
            "python_two_positional": False,
            "builtin_positional": False,
            "captured_builtin_positional": False,
            "nested_builtin_positional": False,
        }, (mode, result)
        original_soac_observations = {
            name: value
            for name, value in result["soac_observations"].items()
            if name != "nested_builtin_positional"
        }
        original_stock_observations = {
            name: value
            for name, value in result["stock_observations"].items()
            if name != "nested_builtin_positional"
        }
        assert original_soac_observations == original_stock_observations, (
            "immediate positional Python and captured builtin method calls "
            "must not expose temporary bound-method wrappers during dispatch",
            mode,
            result["stock_observations"],
            result["soac_observations"],
        )

    assert results["profile"]["soac_observations"]["nested_builtin_positional"] is False
    for mode in ("apply", "verify"):
        result = results[mode]
        assert (
            result["soac_observations"]["nested_builtin_positional"]
            == result["stock_observations"]["nested_builtin_positional"]
        ), (
            "a nested comprehension's captured builtin method call must not "
            "lose source-resolved dispatch when hot continuations are cloned",
            mode,
            result["stock_observations"],
            result["soac_observations"],
        )
