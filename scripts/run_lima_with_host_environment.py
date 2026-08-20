#!/usr/bin/env python3
"""Run a Lima guest command with narrowly forwarded host installer settings.

Stdin carries the environment transport and is exhausted before the guest
command. Use a guest script file or python -c, not a host heredoc to python -.
"""

from __future__ import annotations

import argparse
import ipaddress
import json
import os
from pathlib import PurePosixPath
import re
import subprocess
import sys
from typing import Mapping, TextIO
from urllib.parse import urlsplit, urlunsplit


_INSTALLER_ENVIRONMENT_NAMES = (
    "PIP_INDEX_URL",
    "PIP_EXTRA_INDEX_URL",
    "PIP_FIND_LINKS",
    "PIP_NO_INDEX",
    "PIP_TRUSTED_HOST",
    "PIP_PROXY",
    "PIP_CERT",
    "PIP_CLIENT_CERT",
    "PIP_CONFIG_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "PYPERFORMANCE_INHERIT_ENV_EXTRA",
)
_PROXY_ENVIRONMENT_NAMES = frozenset(
    {
        "PIP_PROXY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    }
)
_ENVIRONMENT_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z", re.ASCII)


def _extra_environment_names(value: str) -> list[str]:
    names = [name.strip() for name in value.split(",") if name.strip()]
    if any(_ENVIRONMENT_NAME.fullmatch(name) is None for name in names):
        raise ValueError("extra installer environment name is not permitted")
    return names


def rewrite_proxy_url(value: str) -> str:
    """Replace host-only loopback proxy addresses with Lima's host gateway."""
    try:
        parsed = urlsplit(value)
        hostname = parsed.hostname
        if hostname is None:
            return value

        if hostname.lower() != "localhost":
            try:
                address = ipaddress.ip_address(hostname)
            except ValueError:
                return value
            if not address.is_loopback:
                return value

        user_information, separator, _host = parsed.netloc.rpartition("@")
        netloc = f"{user_information}@" if separator else ""
        netloc += "host.lima.internal"
        if parsed.port is not None:
            netloc += f":{parsed.port}"
        return urlunsplit(
            (parsed.scheme, netloc, parsed.path, parsed.query, parsed.fragment)
        )
    except ValueError:
        raise ValueError("invalid installer proxy URL") from None


def forwarded_environment(environment: Mapping[str, str]) -> dict[str, str]:
    """Select only installer configuration and explicitly requested extras."""
    extra_names = _extra_environment_names(
        environment.get("PYPERFORMANCE_INHERIT_ENV_EXTRA", "")
    )
    names = dict.fromkeys((*_INSTALLER_ENVIRONMENT_NAMES, *extra_names))
    forwarded = {}
    for name in names:
        if name not in environment:
            continue
        value = environment[name]
        forwarded[name] = (
            rewrite_proxy_url(value) if name in _PROXY_ENVIRONMENT_NAMES else value
        )
    return forwarded


def launch_guest(
    instance: str,
    workdir: str,
    command: list[str],
    environment: Mapping[str, str],
) -> int:
    """Send selected configuration over stdin, never guest process arguments."""
    guest_script = str(
        PurePosixPath(workdir) / "scripts" / "run_lima_with_host_environment.py"
    )
    arguments = [
        "limactl",
        "shell",
        "--workdir",
        workdir,
        instance,
        "--",
        "python3",
        guest_script,
        "--receive",
        "--workdir",
        workdir,
        "--",
        *command,
    ]
    payload = json.dumps(
        forwarded_environment(environment), separators=(",", ":")
    ).encode("utf-8")
    process = subprocess.Popen(arguments, stdin=subprocess.PIPE)
    process.communicate(input=payload)
    return process.returncode


def receive_guest_environment(
    stream: TextIO,
    workdir: str,
    command: list[str],
) -> None:
    """Validate stdin configuration, then replace this process with a login shell."""
    environment = json.load(stream)
    if not isinstance(environment, dict):
        raise ValueError("guest installer environment must be an object")
    if any(not isinstance(name, str) for name in environment):
        raise ValueError("guest installer environment name is not permitted")
    if any(not isinstance(value, str) for value in environment.values()):
        raise ValueError("guest installer environment values must be strings")

    extra_names = _extra_environment_names(
        environment.get("PYPERFORMANCE_INHERIT_ENV_EXTRA", "")
    )
    allowed_names = set(_INSTALLER_ENVIRONMENT_NAMES).union(extra_names)
    if any(name not in allowed_names for name in environment):
        raise ValueError("guest installer environment name is not permitted")

    os.environ.update(environment)
    os.chdir(workdir)
    os.execvp("bash", ["bash", "-lc", 'exec "$@"', "soac-lima", *command])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--instance", default="ubuntu24")
    parser.add_argument("--workdir", required=True)
    parser.add_argument("--receive", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    options = parser.parse_args()
    command = options.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("a guest command is required")

    if options.receive:
        receive_guest_environment(sys.stdin, options.workdir, command)
        return 0
    return launch_guest(options.instance, options.workdir, command, os.environ)


if __name__ == "__main__":
    sys.exit(main())
