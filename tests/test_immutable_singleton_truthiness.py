from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


_MODULE_SOURCE = """
class FlagBox:
    def __init__(self, flag):
        self.flag = flag


class BoolProbe:
    def __init__(self, events, value):
        self.events = events
        self.value = value

    def __bool__(self):
        self.events.append(("bool", self.value))
        return self.value


class LengthProbe:
    def __init__(self, events, value):
        self.events = events
        self.value = value

    def __len__(self):
        self.events.append(("len", self.value))
        return self.value


class IntProbe(int):
    def __bool__(self):
        return False


class ListProbe(list):
    def __len__(self):
        return 0


class InvalidBool:
    def __bool__(self):
        return 1


class InvalidLength:
    def __len__(self):
        return -1


class RaisingBool:
    def __bool__(self):
        raise LookupError("bool callback failed")


class RaisingLength:
    def __len__(self):
        raise LookupError("length callback failed")


class TracingFlag:
    def __get__(self, instance, owner):
        if instance is None:
            return self
        instance.events.append("descriptor")
        return instance.value


class DescriptorBox:
    flag = TracingFlag()

    def __init__(self, events, value):
        self.events = events
        self.value = value


class MutatingBool:
    def __init__(self, owner, events):
        self.owner = owner
        self.events = events

    def __bool__(self):
        self.events.append("mutated")
        self.owner.flag = False
        return True


class FinalizingBool:
    def __init__(self, events, value):
        self.events = events
        self.value = value

    def __bool__(self):
        self.events.append("bool")
        return self.value

    def __del__(self):
        self.events.append("finalized")


def branch(value):
    if value:
        return "true"
    return "false"


def inverted(value):
    return not value


def dynamic_branch(owner):
    if owner.flag:
        return "true"
    return "false"


def short_circuit(owner):
    return owner.flag and "left" or "right"


def descriptor_branch(owner):
    if owner.flag:
        owner.events.append("branch:true")
        return True
    owner.events.append("branch:false")
    return False


def temporary_branch(events, value):
    if FinalizingBool(events, value):
        events.append("branch:true")
        return True
    events.append("branch:false")
    return False


def hot(owner):
    if owner.flag:
        return 1
    return 0
"""


def _run_truthiness_worker(
    tmp_path: Path, module_name: str, work_dir: Path, mode: str
) -> dict:
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
        stock = {"__name__": name, "__builtins__": builtins.__dict__}
        exec(compile(source, "<stock-singleton-truthiness>", "exec"), stock)

        sys.path.insert(0, root)
        from soac.import_hook import install

        install()
        module = importlib.import_module(name)

        def error(callback):
            try:
                callback()
            except Exception as failure:
                return type(failure).__name__, str(failure)
            raise AssertionError("expected truthiness callback to raise")

        def exercise(namespace):
            branch = namespace["branch"]
            results = {
                "singletons": [branch(value) for value in (True, False, None)],
                "inverted": [namespace["inverted"](value) for value in (True, False, None)],
                "ordinary": [branch(value) for value in (0, 1, 0.0, 1.5, "", "x", [], [1])],
                "int_subclass": branch(namespace["IntProbe"](1)),
                "list_subclass": branch(namespace["ListProbe"]([1])),
                "bool_subclass": error(lambda: type("BoolSubclass", (bool,), {})),
                "invalid_bool": error(lambda: branch(namespace["InvalidBool"]())),
                "invalid_length": error(lambda: branch(namespace["InvalidLength"]())),
                "raising_bool": error(lambda: branch(namespace["RaisingBool"]())),
                "raising_length": error(lambda: branch(namespace["RaisingLength"]())),
            }

            box = namespace["FlagBox"](True)
            values = []
            for value in (True, False, None, 1, 0, "yes", ""):
                box.flag = value
                values.append((namespace["dynamic_branch"](box), namespace["short_circuit"](box)))
            results["dynamic_flags"] = values

            events = []
            probe = namespace["BoolProbe"](events, True)
            reference = weakref.ref(probe)
            before = sys.getrefcount(probe)
            assert branch(probe) == "true"
            after = sys.getrefcount(probe)
            results["custom_bool"] = (events[:], before == after)
            del probe
            gc.collect()
            results["custom_bool_released"] = reference() is None

            events = []
            assert branch(namespace["LengthProbe"](events, 0)) == "false"
            assert branch(namespace["LengthProbe"](events, 2)) == "true"
            results["custom_length"] = events

            events = []
            descriptor = namespace["DescriptorBox"](
                events, namespace["BoolProbe"](events, True)
            )
            assert namespace["descriptor_branch"](descriptor) is True
            results["descriptor_order"] = events[:]

            descriptor.value = False
            events.clear()
            assert namespace["descriptor_branch"](descriptor) is False
            results["descriptor_mutation"] = events[:]

            original_descriptor = namespace["DescriptorBox"].flag
            namespace["DescriptorBox"].flag = property(lambda self: not self.value)
            try:
                results["class_descriptor_mutation"] = namespace["dynamic_branch"](
                    descriptor
                )
            finally:
                namespace["DescriptorBox"].flag = original_descriptor

            events = []
            owner = namespace["FlagBox"](None)
            owner.flag = namespace["MutatingBool"](owner, events)
            results["callback_mutation"] = (
                namespace["dynamic_branch"](owner),
                namespace["dynamic_branch"](owner),
                events,
            )

            finalizer_results = []
            for value in (True, False):
                events = []
                outcome = namespace["temporary_branch"](events, value)
                gc.collect()
                finalizer_results.append((outcome, events))
            results["finalizers"] = finalizer_results

            return results

        expected = exercise(stock)
        actual = exercise(module.__dict__)
        assert expected["singletons"] == ["true", "false", "false"], expected
        assert expected["inverted"] == [False, True, True], expected
        assert expected["int_subclass"] == "false", expected
        assert expected["list_subclass"] == "false", expected
        assert expected["custom_bool_released"], expected
        assert expected["custom_bool"][1], expected
        assert expected["descriptor_order"] == [
            "descriptor", ("bool", True), "branch:true"
        ], expected
        assert expected["callback_mutation"] == ("true", "false", ["mutated"]), expected
        assert actual == expected, (expected, actual)

        owner = module.FlagBox(False)
        for index in range(64):
            owner.flag = bool(index & 1)
            assert module.hot(owner) == index & 1

        print(json.dumps({"mode": __MODE__, "outcomes": actual}))
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
        check=False,
        capture_output=True,
        text=True,
        env=environment,
        timeout=90,
    )
    assert completed.returncode == 0, (
        f"{mode} transformed singleton-truthiness subprocess failed:\n"
        f"{completed.stdout}{completed.stderr}"
    )
    return json.loads(completed.stdout.splitlines()[-1])


def test_immutable_singleton_truthiness_preserves_cpython_behavior(
    tmp_path: Path,
) -> None:
    module_name = "immutable_singleton_truthiness_case"
    (tmp_path / f"{module_name}.py").write_text(textwrap.dedent(_MODULE_SOURCE))
    work_dir = tmp_path / "soac-work"
    results = {
        mode: _run_truthiness_worker(tmp_path, module_name, work_dir, mode)
        for mode in ("profile", "verify", "apply")
    }
    assert set(results) == {"profile", "verify", "apply"}

    from soac import _soac_ext

    profile = json.loads(_soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin")))
    records = [record for record in profile["records"] if record["module_name"] == module_name]
    branches = [
        row
        for record in records
        for row in record["rows"]
        if row["kind"] == "branch_outcomes"
        and row["function_qualname"] == "hot"
        and row["value"]
    ]
    assert {row["observed_value"] for row in branches} == {0, 1}, branches

    native = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text().splitlines()
        if line.strip()
    ]
    for name in ("branch", "dynamic_branch", "descriptor_branch", "hot"):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == name
            for row in native
        ), (name, native)
