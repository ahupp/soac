import importlib.util
import io
import json
from pathlib import Path

import pytest


def _load_lima_environment_module():
    script = (
        Path(__file__).resolve().parents[1]
        / "scripts"
        / "run_lima_with_host_environment.py"
    )
    spec = importlib.util.spec_from_file_location("run_lima_with_host_environment", script)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize(
    ("original", "expected"),
    [
        (
            "http://user:fake-password@127.0.0.1:8123/proxy",
            "http://user:fake-password@host.lima.internal:8123/proxy",
        ),
        ("http://localhost:8123", "http://host.lima.internal:8123"),
        ("http://[::1]:8123", "http://host.lima.internal:8123"),
        (
            "https://fake-user:fake-password@[::1]:8123/path?key=value#fragment",
            "https://fake-user:fake-password@host.lima.internal:8123/path?key=value#fragment",
        ),
        ("http://proxy.example.invalid:8123", "http://proxy.example.invalid:8123"),
    ],
)
def test_lima_bridge_rewrites_only_host_loopback_proxy_urls(
    original: str,
    expected: str,
) -> None:
    module = _load_lima_environment_module()

    assert module.rewrite_proxy_url(original) == expected


def test_lima_bridge_forwards_only_network_settings_and_explicit_extras() -> None:
    module = _load_lima_environment_module()
    environment = {
        "PIP_INDEX_URL": "https://packages.example.invalid/simple",
        "HTTP_PROXY": "http://127.0.0.1:8123",
        "https_proxy": "http://localhost:9443",
        "ALL_PROXY": "socks5h://127.0.0.1:1080",
        "all_proxy": "socks5h://127.0.0.1:1081",
        "PYPERFORMANCE_INHERIT_ENV_EXTRA": "CUSTOM_INSTALLER_CERT",
        "CUSTOM_INSTALLER_CERT": "/fake/ca.pem",
        "UNRELATED_SECRET": "never-forward",
        "XDG_CONFIG_HOME": "/must-not-override-guest-defaults",
    }

    forwarded = module.forwarded_environment(environment)

    assert forwarded["PIP_INDEX_URL"] == environment["PIP_INDEX_URL"]
    assert forwarded["HTTP_PROXY"] == "http://host.lima.internal:8123"
    assert forwarded["https_proxy"] == "http://host.lima.internal:9443"
    assert forwarded["CUSTOM_INSTALLER_CERT"] == "/fake/ca.pem"
    assert "UNRELATED_SECRET" not in forwarded
    assert "XDG_CONFIG_HOME" not in forwarded
    assert "ALL_PROXY" not in forwarded
    assert "all_proxy" not in forwarded


@pytest.mark.parametrize("proxy_name", ["ALL_PROXY", "all_proxy"])
def test_lima_bridge_preserves_explicit_all_proxy_opt_in(proxy_name: str) -> None:
    module = _load_lima_environment_module()
    environment = {
        "ALL_PROXY": "socks5h://127.0.0.1:1080",
        "all_proxy": "socks5h://127.0.0.1:1081",
        "PYPERFORMANCE_INHERIT_ENV_EXTRA": proxy_name,
    }

    forwarded = module.forwarded_environment(environment)

    port = "1080" if proxy_name == "ALL_PROXY" else "1081"
    assert forwarded[proxy_name] == f"socks5h://host.lima.internal:{port}"
    assert ({"ALL_PROXY", "all_proxy"} - {proxy_name}).isdisjoint(forwarded)


@pytest.mark.parametrize("extra_names", ["BAD-NAME", "GOOD,ALSO-BAD", "A=bad"])
def test_lima_bridge_rejects_invalid_extra_environment_names(extra_names: str) -> None:
    module = _load_lima_environment_module()

    with pytest.raises(ValueError, match="environment name"):
        module.forwarded_environment(
            {"PYPERFORMANCE_INHERIT_ENV_EXTRA": extra_names}
        )


def test_lima_bridge_sends_credentials_only_through_guest_stdin(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    module = _load_lima_environment_module()
    fake_secret = "fake-credential-never-in-arguments"
    captured: dict[str, object] = {}

    class FakeProcess:
        returncode = 0

        def communicate(self, input: bytes):
            captured["stdin"] = input
            return None, None

    def fake_popen(arguments, **kwargs):
        captured["arguments"] = arguments
        captured["kwargs"] = kwargs
        return FakeProcess()

    monkeypatch.setattr(module.subprocess, "Popen", fake_popen)

    result = module.launch_guest(
        "ubuntu24",
        "/home/example/project",
        ["just", "pyperformance", "stock"],
        {
            "PIP_INDEX_URL": f"https://user:{fake_secret}@packages.example.invalid/simple",
            "PIP_PROXY": "http://127.0.0.1:8123",
            "UNRELATED_SECRET": "not-forwarded",
        },
    )

    assert result == 0
    assert captured["kwargs"] == {"stdin": module.subprocess.PIPE}
    arguments = captured["arguments"]
    assert arguments == [
        "limactl",
        "shell",
        "ubuntu24",
        "--",
        "python3",
        "/home/example/project/scripts/run_lima_with_host_environment.py",
        "--receive",
        "--workdir",
        "/home/example/project",
        "--",
        "just",
        "pyperformance",
        "stock",
    ]
    assert fake_secret not in " ".join(arguments)

    payload = json.loads(captured["stdin"])
    assert fake_secret in payload["PIP_INDEX_URL"]
    assert payload["PIP_PROXY"] == "http://host.lima.internal:8123"
    assert "UNRELATED_SECRET" not in payload
    output = capsys.readouterr()
    assert fake_secret not in output.out
    assert fake_secret not in output.err


@pytest.mark.parametrize(
    "payload",
    [
        {"UNRELATED_SECRET": "fake-secret"},
        {"PIP_INDEX_URL": 123},
        {"CUSTOM_INSTALLER_CERT": "/not-explicitly-declared"},
        {"PYPERFORMANCE_INHERIT_ENV_EXTRA": "BAD-NAME"},
    ],
)
def test_lima_bridge_rejects_undeclared_or_nonstring_environment(
    tmp_path: Path,
    payload: dict[str, object],
) -> None:
    module = _load_lima_environment_module()

    with pytest.raises(ValueError, match="not permitted|string"):
        module.receive_guest_environment(
            io.StringIO(json.dumps(payload)),
            str(tmp_path),
            ["just", "pyperformance"],
        )


def test_lima_bridge_receives_environment_and_executes_guest_login_shell(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    module = _load_lima_environment_module()
    monkeypatch.delenv("PIP_INDEX_URL", raising=False)
    observed: dict[str, object] = {}

    monkeypatch.setattr(module.os, "chdir", lambda path: observed.update(workdir=path))
    monkeypatch.setattr(
        module.os,
        "execvp",
        lambda executable, arguments: observed.update(
            executable=executable,
            arguments=arguments,
        ),
    )

    module.receive_guest_environment(
        io.StringIO(
            json.dumps({"PIP_INDEX_URL": "https://packages.example.invalid/simple"})
        ),
        str(tmp_path),
        ["just", "pyperformance-compare", "all", "1"],
    )

    assert module.os.environ["PIP_INDEX_URL"] == "https://packages.example.invalid/simple"
    assert observed["workdir"] == str(tmp_path)
    assert observed["executable"] == "bash"
    assert observed["arguments"] == [
        "bash",
        "-lc",
        'exec "$@"',
        "soac-lima",
        "just",
        "pyperformance-compare",
        "all",
        "1",
    ]
