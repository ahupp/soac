from __future__ import annotations

import subprocess
from types import MethodType

from soac.ipython import ProfileRecord, SoacOptimizationExplorer, load_ipython_extension


class FakeShell:
    def __init__(self, namespace):
        self.user_ns = namespace
        self.user_global_ns = namespace


class FakeMagicShell(FakeShell):
    def register_magics(self, magics_class):
        self.magics = magics_class(shell=self)


def sample_add(a, b):
    return a + b


def test_soac_profile_materializes_function_and_writes_counters(tmp_path):
    shell = FakeShell({"sample_add": sample_add, "rhs": 2})
    explorer = SoacOptimizationExplorer(shell, artifact_root=tmp_path)

    assert explorer.profile("sample_add(1, rhs)") == 3

    record = explorer.records["sample_add"]
    assert record.source_path.exists()
    assert record.profile_dump.exists()
    assert "def sample_add" in record.source_path.read_text(encoding="utf-8")


class StubExplorer(SoacOptimizationExplorer):
    def __init__(self, shell, *, artifact_root):
        super().__init__(shell, artifact_root=artifact_root)
        self.render_env = None
        self.codex_prompt = None

    def _run_inspector(self, bin_name, *args, env=None):
        if bin_name == "list_jit_functions":
            return subprocess.CompletedProcess(
                args=[bin_name, *args],
                returncode=0,
                stdout="1\tother\n2\tsample_add\n",
                stderr="",
            )
        if bin_name == "render_jit_clif":
            self.render_env = env
            vcode_path = self.records["sample_add"].vcode_path
            vcode_path.write_text("; fake vcode\n", encoding="utf-8")
            return subprocess.CompletedProcess(
                args=[bin_name, *args],
                returncode=0,
                stdout="; fake clif\n",
                stderr="",
            )
        if bin_name == "inspect_counters":
            return subprocess.CompletedProcess(
                args=[bin_name, *args],
                returncode=0,
                stdout="fake specialization counters\n",
                stderr="",
            )
        raise AssertionError(f"unexpected inspector bin: {bin_name}")

    def _run_codex_annotator(self, prompt, output_path):
        self.codex_prompt = prompt
        output_path.write_text("; annotated clif\n", encoding="utf-8")
        return output_path.read_text(encoding="utf-8")


def test_soac_vcode_renders_specialized_vcode_for_profiled_function(tmp_path, capsys):
    shell = FakeShell({"sample_add": sample_add})
    explorer = StubExplorer(shell, artifact_root=tmp_path)
    source_path = tmp_path / "sample.py"
    source_path.write_text("def sample_add(a, b):\n    return a + b\n", encoding="utf-8")
    work_dir = tmp_path / "counters"
    work_dir.mkdir()
    explorer.records["sample_add"] = ProfileRecord(
        name="sample_add",
        module_name="_soac_ipython_sample_add",
        source_path=source_path,
        work_dir=work_dir,
    )

    assert explorer.vcode("sample_add") == "; fake vcode\n"

    captured = capsys.readouterr()
    assert "; fake vcode" in captured.out
    assert explorer.render_env is not None
    assert explorer.render_env["SOAC_WORK_DIR"] == str(work_dir)
    assert explorer.render_env["SOAC_OPT_MODE"] == "apply"


def test_soac_clif_prints_specialized_clif_for_profiled_function(tmp_path, capsys):
    shell = FakeShell({"sample_add": sample_add})
    explorer = StubExplorer(shell, artifact_root=tmp_path)
    source_path = tmp_path / "sample.py"
    source_path.write_text("def sample_add(a, b):\n    return a + b\n", encoding="utf-8")
    work_dir = tmp_path / "counters"
    work_dir.mkdir()
    explorer.records["sample_add"] = ProfileRecord(
        name="sample_add",
        module_name="_soac_ipython_sample_add",
        source_path=source_path,
        work_dir=work_dir,
    )

    assert explorer.clif("sample_add") == "; fake clif\n"

    captured = capsys.readouterr()
    assert "; fake clif" in captured.out
    assert explorer.render_env is not None
    assert explorer.render_env["SOAC_WORK_DIR"] == str(work_dir)
    assert explorer.render_env["SOAC_OPT_MODE"] == "apply"


def test_soac_clif_annotate_uses_codex_context_for_profiled_function(tmp_path, capsys):
    shell = FakeShell({"sample_add": sample_add})
    explorer = StubExplorer(shell, artifact_root=tmp_path)
    source_path = tmp_path / "sample.py"
    source_path.write_text("def sample_add(a, b):\n    return a + b\n", encoding="utf-8")
    work_dir = tmp_path / "counters"
    work_dir.mkdir()
    (work_dir / "profile.bin").write_bytes(b"fake counters")
    explorer.records["sample_add"] = ProfileRecord(
        name="sample_add",
        module_name="_soac_ipython_sample_add",
        source_path=source_path,
        work_dir=work_dir,
    )

    assert explorer.clif_annotate("sample_add") == "; annotated clif\n"

    captured = capsys.readouterr()
    assert "; annotated clif" in captured.out
    assert explorer.codex_prompt is not None
    assert "def sample_add" in explorer.codex_prompt
    assert "; fake clif" in explorer.codex_prompt
    assert "fake specialization counters" in explorer.codex_prompt


def test_render_magics_do_not_return_printed_payload(capsys):
    shell = FakeMagicShell({})
    load_ipython_extension(shell)

    def fake_vcode(self, line):
        print(f"vcode:{line}")
        return "; fake vcode\n"

    def fake_clif(self, line):
        print(f"clif:{line}")
        return "; fake clif\n"

    def fake_clif_annotate(self, line):
        print(f"annotate:{line}")
        return "; annotated clif\n"

    shell.soac_explorer.vcode = MethodType(fake_vcode, shell.soac_explorer)
    shell.soac_explorer.clif = MethodType(fake_clif, shell.soac_explorer)
    shell.soac_explorer.clif_annotate = MethodType(
        fake_clif_annotate,
        shell.soac_explorer,
    )

    assert shell.magics.soac_vcode("sample_add") is None
    assert shell.magics.soac_clif("sample_add") is None
    assert shell.magics.soac_clif_annotate("sample_add") is None

    captured = capsys.readouterr()
    assert "vcode:sample_add" in captured.out
    assert "clif:sample_add" in captured.out
    assert "annotate:sample_add" in captured.out
