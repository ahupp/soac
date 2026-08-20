from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


_SOURCE = """
EVENTS = []


class Base:
    def read_self(self):
        return self.link


class Left(Base):
    def __init__(self, value):
        self.link = value


class Right(Base):
    def __init__(self, value):
        self.link = value


class Third(Base):
    def __init__(self, value):
        self.link = value


class Fourth(Base):
    def __init__(self, value):
        self.link = value


class Packet:
    def __init__(self, value):
        self.link = value


class Unique:
    def __init__(self, value):
        self.padding = None
        self.payload = value


class MixedLeft:
    def __init__(self, value):
        self.mixed = value


class MixedRight:
    def __init__(self, value):
        self.padding = None
        self.mixed = value


class OverOne:
    def __init__(self, value):
        self.overflow = value


class OverTwo:
    def __init__(self, value):
        self.overflow = value


class OverThree:
    def __init__(self, value):
        self.overflow = value


class OverFour:
    def __init__(self, value):
        self.overflow = value


class OverFive:
    def __init__(self, value):
        self.overflow = value


class OverSix:
    def __init__(self, value):
        self.overflow = value


class Slotted:
    __slots__ = ("slotted",)

    def __init__(self, value):
        self.slotted = value


class Unanchored:
    pass


class Cold:
    def __init__(self, value):
        self.cold = value


class Consumer:
    def read(self, owner):
        return owner.link


class Observed(Left):
    def __getattribute__(self, name):
        if name == "link":
            EVENTS.append("observed:get")
        return object.__getattribute__(self, name)

    def __setattr__(self, name, value):
        if name == "link":
            EVENTS.append(("observed:set", value))
        object.__setattr__(self, name, value)


class AlternateBase:
    @property
    def link(self):
        EVENTS.append("alternate:get")
        return "alternate"


class FinalizingValue:
    def __init__(self, owner):
        self.owner = owner

    def __del__(self):
        EVENTS.append(("finalized", read_uniform(self.owner)))


def read_uniform(owner):
    return owner.link


def schedule(owners):
    return [owner.link for owner in owners]


def write_uniform(owner, value):
    owner.link = value


def read_unique(owner):
    return owner.payload


def read_mixed(owner):
    return owner.mixed


def read_overflow(owner):
    return owner.overflow


def read_slotted(owner):
    return owner.slotted


def read_unanchored(owner):
    return owner.unanchored


def read_cold(owner):
    return owner.cold
"""


def _run_mode(tmp_path: Path, module_name: str, work_dir: Path, mode: str) -> None:
    script = textwrap.dedent(
        """
        import builtins
        import gc
        import importlib
        import sys
        import weakref

        root, name, mode = __ROOT__, __NAME__, __MODE__
        source = open(root + "/" + name + ".py", encoding="utf-8").read()
        stock = {"__name__": name, "__builtins__": builtins.__dict__}
        exec(compile(source, "<stock-uniform-polymorphic-fields>", "exec"), stock)

        sys.path.insert(0, root)
        from soac.import_hook import install

        install()
        module = importlib.import_module(name)

        def capture_error(callback):
            try:
                callback()
            except Exception as error:
                return type(error).__name__, str(error)
            raise AssertionError("expected field operation to fail")

        def exercise(namespace):
            consumer = namespace["Consumer"]()
            owners = ("Left", "Right", "Third", "Fourth", "Packet")
            overflow = (
                "OverOne", "OverTwo", "OverThree",
                "OverFour", "OverFive", "OverSix",
            )

            for index in range(32):
                values = []
                objects = []
                for owner_name in owners:
                    original = (owner_name, index)
                    owner = namespace[owner_name](original)
                    assert namespace["read_uniform"](owner) == original
                    assert consumer.read(owner) == original
                    if owner_name != "Packet":
                        assert owner.read_self() == original
                    updated = ("updated", owner_name, index)
                    assert namespace["write_uniform"](owner, updated) is None
                    assert namespace["read_uniform"](owner) == updated
                    objects.append(owner)
                    values.append(updated)
                assert namespace["schedule"](objects) == values

                assert namespace["read_unique"](namespace["Unique"](index)) == index
                assert namespace["read_mixed"](namespace["MixedLeft"](index)) == index
                assert namespace["read_mixed"](namespace["MixedRight"](index)) == index
                for owner_name in overflow:
                    assert namespace["read_overflow"](
                        namespace[owner_name](index)
                    ) == index

                slot = namespace["Slotted"](index)
                assert namespace["read_slotted"](slot) == index
                unanchored = namespace["Unanchored"]()
                unanchored.unanchored = index
                assert namespace["read_unanchored"](unanchored) == index
                if index == 0:
                    assert namespace["read_cold"](namespace["Cold"](index)) == 0

            if mode == "profile":
                return "profile-controls-pass"

            outcomes = {}
            observed = namespace["Observed"]("observed")
            namespace["EVENTS"].clear()
            assert namespace["read_uniform"](observed) == "observed"
            outcomes["getattribute"] = namespace["EVENTS"][:]
            namespace["EVENTS"].clear()
            namespace["write_uniform"](observed, "changed")
            outcomes["setattr"] = namespace["EVENTS"][:]

            owner = namespace["Left"]("instance")
            dictionary = owner.__dict__
            dictionary["link"] = "materialized"
            outcomes["materialized"] = namespace["read_uniform"](owner)
            dictionary[1] = "promoted"
            outcomes["promoted"] = namespace["read_uniform"](owner)
            del dictionary["link"]
            outcomes["deleted"] = capture_error(
                lambda: namespace["read_uniform"](owner)
            )
            namespace["write_uniform"](owner, "reinserted")
            outcomes["reinserted"] = namespace["read_uniform"](owner)

            growing = namespace["Right"]("growing")
            for index in range(40):
                setattr(growing, "extra_" + str(index), index)
            outcomes["grown"] = namespace["read_uniform"](growing)

            descriptor_owner = namespace["Third"]("stored")
            namespace["Third"].link = property(
                lambda instance: "descriptor",
                lambda instance, value: namespace["EVENTS"].append(("descriptor:set", value)),
            )
            try:
                outcomes["descriptor"] = namespace["read_uniform"](descriptor_owner)
                namespace["EVENTS"].clear()
                namespace["write_uniform"](descriptor_owner, "assigned")
                outcomes["descriptor_set"] = namespace["EVENTS"][:]
            finally:
                del namespace["Third"].link
            outcomes["restored"] = namespace["read_uniform"](descriptor_owner)

            inherited = namespace["Fourth"]("inherited")
            original_bases = namespace["Fourth"].__bases__
            namespace["Fourth"].__bases__ = (namespace["AlternateBase"],)
            try:
                namespace["EVENTS"].clear()
                outcomes["mro"] = (
                    namespace["read_uniform"](inherited),
                    namespace["EVENTS"][:],
                )
            finally:
                namespace["Fourth"].__bases__ = original_bases

            class Foreign:
                def __init__(self, value):
                    self.link = value

            outcomes["foreign"] = namespace["read_uniform"](Foreign("foreign"))

            original_packet = namespace["Packet"]

            class ReplacementPacket:
                def __init__(self, value):
                    self.link = value

            namespace["Packet"] = ReplacementPacket
            try:
                outcomes["rebound_owner"] = namespace["read_uniform"](
                    namespace["Packet"]("replacement")
                )
            finally:
                namespace["Packet"] = original_packet

            owner = namespace["Packet"]("old")
            watched = namespace["FinalizingValue"](owner)
            namespace["write_uniform"](owner, watched)
            del watched
            namespace["EVENTS"].clear()
            namespace["write_uniform"](owner, "visible-during-finalizer")
            gc.collect()
            outcomes["finalizer"] = namespace["EVENTS"][:]

            retained = namespace["Packet"]("retained")
            reference = weakref.ref(retained)
            before = sys.getrefcount(retained)
            assert namespace["read_uniform"](retained) == "retained"
            outcomes["receiver_refcount"] = sys.getrefcount(retained) == before
            del retained
            gc.collect()
            outcomes["receiver_released"] = reference() is None
            return outcomes

        expected = exercise(stock)
        actual = exercise(module.__dict__)
        if mode != "profile":
            assert expected["getattribute"] == ["observed:get"], expected
            assert expected["setattr"] == [("observed:set", "changed")], expected
            assert expected["mro"] == ("alternate", ["alternate:get"]), expected
            assert expected["finalizer"] == [
                ("finalized", "visible-during-finalizer")
            ], expected
            assert expected["receiver_refcount"], expected
            assert expected["receiver_released"], expected
            assert actual == expected, (expected, actual)
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
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env=environment,
        timeout=90,
    )
    assert result.returncode == 0, (
        f"{mode} uniform-polymorphic-field subprocess failed:\n"
        f"{result.stdout}{result.stderr}"
    )


def _field_rows(records: list[dict], qualname: str, branch: str) -> list[tuple]:
    selected = {}
    for record in records:
        for row in record["rows"]:
            if row["kind"] != "field_access" or row["function_qualname"] != qualname:
                continue
            key = (row["function_id"], row["instr_id"], row["counter_id"])
            if key not in selected or row["value"] > selected[key]["value"]:
                selected[key] = row
    return sorted(
        (row["instr_id"], row.get("branches", {}).get(branch, 0))
        for row in selected.values()
    )


def test_uniform_polymorphic_nonself_fields_reuse_each_exact_owner_guard(
    tmp_path: Path,
) -> None:
    module_name = "uniform_polymorphic_nonself_fields_case"
    (tmp_path / f"{module_name}.py").write_text(textwrap.dedent(_SOURCE))
    work_dir = tmp_path / "soac-work"

    _run_mode(tmp_path, module_name, work_dir, "profile")

    from soac import _soac_ext

    profile = json.loads(_soac_ext.inspect_counter_dump_json(str(work_dir / "profile.bin")))
    records = [row for row in profile["records"] if row["module_name"] == module_name]
    assert records

    owners = {
        entry["type_id"]: entry["qualname"]
        for record in profile["records"]
        for entry in record["type_table"]
        if entry["module_name"] == module_name
    }
    keys = {
        (owners[key["owner_type_id"]], key["key"], key["index"])
        for record in profile["records"]
        for key in record["type_keys"]
        if key["owner_type_id"] in owners
    }
    uniform = {(owner, index) for owner, key, index in keys if key == "link"}
    assert uniform == {
        ("Left", 0), ("Right", 0), ("Third", 0), ("Fourth", 0), ("Packet", 0)
    }, uniform
    mixed = {(owner, index) for owner, key, index in keys if key == "mixed"}
    assert mixed == {("MixedLeft", 0), ("MixedRight", 1)}, mixed
    overflow = {(owner, index) for owner, key, index in keys if key == "overflow"}
    assert len(overflow) == 6 and {index for _, index in overflow} == {0}, overflow

    for qualname, minimum in (
        ("read_uniform", 320), ("Consumer.read", 160),
        ("read_mixed", 64), ("read_overflow", 192),
        ("read_unique", 32),
    ):
        rows = _field_rows(records, qualname, "generic_getattr")
        assert any(count >= minimum for _, count in rows), (qualname, minimum, rows)
    stores = _field_rows(records, "write_uniform", "generic_setattr")
    assert any(count >= 160 for _, count in stores), stores

    _run_mode(tmp_path, module_name, work_dir, "verify")
    verify = json.loads(_soac_ext.inspect_counter_dump_json(str(work_dir / "verify.bin")))
    verified = [row for row in verify["records"] if row["module_name"] == module_name]
    assert verified

    _run_mode(tmp_path, module_name, work_dir, "apply")

    unique = _field_rows(verified, "read_unique", "indexed_hit")
    assert any(count >= 32 for _, count in unique), unique
    inherited = _field_rows(verified, "Base.read_self", "indexed_hit")
    assert any(count >= 128 for _, count in inherited), inherited

    for qualname in (
        "read_mixed", "read_overflow", "read_slotted", "read_unanchored",
        "read_cold", "write_uniform",
    ):
        hits = _field_rows(verified, qualname, "indexed_hit")
        assert all(count == 0 for _, count in hits), (qualname, hits)

    native = [
        json.loads(line)
        for line in (work_dir / "jit-code-summary.jsonl").read_text().splitlines()
        if line.strip()
    ]
    for qualname in ("read_uniform", "Consumer.read", "read_unique", "Base.read_self"):
        assert any(
            row.get("entry_kind") == "direct_function_body"
            and row.get("function_qualname") == qualname
            for row in native
        ), (qualname, native)

    uniform_hits = {
        qualname: _field_rows(verified, qualname, "indexed_hit")
        for qualname in ("read_uniform", "Consumer.read")
    }
    assert any(count >= 320 for _, count in uniform_hits["read_uniform"]) and any(
        count >= 160 for _, count in uniform_hits["Consumer.read"]
    ), (
        "hot non-self reads shared by four related owners and an unrelated "
        "same-index Packet must retain all five exact owner guards and record "
        "original-source indexed hits",
        uniform_hits,
    )
