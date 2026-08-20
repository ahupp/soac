"""Prepare explicitly pinned, ordinary local dependencies before driver execution.

The manifest is dependency provenance, never a source-selection policy or an
execution grant. No benchmark module is imported to discover or prepare a package.
"""

from __future__ import annotations

import base64
import csv
import gzip
import hashlib
import io
import json
import os
import sys
import tarfile
import tempfile
from email.parser import Parser
from pathlib import Path, PurePosixPath
from urllib.parse import unquote, urlsplit

from packaging.specifiers import SpecifierSet
from packaging.utils import canonicalize_name
from packaging.version import Version
from pyperformance._venv import RequirementsInstallationFailedError

MANIFEST = Path(__file__).with_suffix(".json")
CACHE_DIRECTORY = ".soac-pyperformance-local-packages-v1"


def _fail(message, cause=None):
    # Upstream catches this exception for common/unique-venv fallback. Unlike
    # pip's own failures, our preflight has no subprocess stderr to retain.
    print(message, file=sys.stderr)
    raise RequirementsInstallationFailedError(message) from cause


def _digest(data):
    return hashlib.sha256(data).hexdigest()


def _json(data):
    return (json.dumps(data, sort_keys=True, separators=(",", ":")) + "\n").encode()


def _relative(value):
    if not isinstance(value, str):
        raise ValueError("local package path must be a string")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or not path.parts
        or any(p in {".", ".."} for p in path.parts)
    ):
        raise ValueError("local package path must stay inside the original driver tree")
    if path.as_posix() != value:
        raise ValueError("local package path must be canonical")
    return path


def _archive(root, files):
    output = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
        with tarfile.open(
            fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT
        ) as archive:
            for record in files:
                path = root / record["path"]
                data = path.read_bytes()
                if path.is_symlink() or _digest(data) != record["sha256"]:
                    raise ValueError("local package changed while archiving")
                member = tarfile.TarInfo("source/" + record["path"])
                member.size = len(data)
                member.mode = record["mode"]
                archive.addfile(member, io.BytesIO(data))
    return output.getvalue()


def resolve(benchmark, source_tools):
    """Select declarative metadata, then validate its entire original source tree."""
    try:
        raw = MANIFEST.read_bytes()
        manifest = json.loads(raw)
        if manifest.get("schema") != 1 or set(manifest) != {"schema", "benchmarks"}:
            raise ValueError("unknown local-package manifest schema")
        entries = manifest["benchmarks"]
        if not isinstance(entries, dict):
            raise ValueError("local-package manifest benchmarks must be a mapping")
        entry = entries.get(str(getattr(benchmark, "name", "")))
        if entry is None:
            return None
        if set(entry) != {"script", "stock_source_fingerprint", "packages"}:
            raise ValueError("invalid local-package benchmark declaration")
        script = Path(benchmark.runscript).absolute()
        if (
            not script.is_file()
            or script.is_symlink()
            or script.name != entry["script"]
        ):
            raise ValueError("local-package declaration belongs to another driver")
        root = script.parent
        records = [
            {
                "relative_path": path.as_posix(),
                "stock_sha256": _digest((root / path).read_bytes()),
            }
            for path in source_tools._source_inventory(root)
        ]
        source_hash = source_tools._stock_fingerprint(records)
        if source_hash != entry["stock_source_fingerprint"]:
            raise ValueError("local-package original source fingerprint mismatch")
        packages = []
        names = set()
        for package in entry["packages"]:
            if set(package) != {
                "path",
                "distribution",
                "version",
                "when_python",
                "source_sha256",
            }:
                raise ValueError("invalid local-package declaration")
            relative = _relative(package["path"])
            name = canonicalize_name(package["distribution"], validate=True)
            Version(package["version"])
            SpecifierSet(package["when_python"])
            if name in names:
                raise ValueError("duplicate local distribution declaration")
            names.add(name)
            package_root = root / relative
            files = [
                {
                    "path": path.as_posix(),
                    "sha256": _digest((package_root / path).read_bytes()),
                    "mode": (package_root / path).stat().st_mode & 0o777,
                }
                for path in source_tools._source_inventory(package_root)
            ]
            if not files or _digest(_json(files)) != package["source_sha256"]:
                raise ValueError("local package source fingerprint mismatch")
            archive = _archive(package_root, files)
            packages.append(
                {
                    **package,
                    "distribution": name,
                    "files": files,
                    "archive_sha256": _digest(archive),
                    "_archive": archive,
                }
            )
        if not packages:
            raise ValueError("local-package declaration is empty")
        return {
            "schema": 1,
            "manifest_sha256": _digest(raw),
            "benchmark": benchmark.name,
            "script": entry["script"],
            "stock_source_fingerprint": source_hash,
            "source_files": records,
            "packages": packages,
        }
    except (
        OSError,
        UnicodeError,
        ValueError,
        TypeError,
        KeyError,
        AttributeError,
    ) as error:
        _fail(f"local-package manifest: {error}", error)


def for_environment(plan, venv):
    if plan is None:
        return None
    major, minor, micro, level, serial = venv.info.sys.version_info
    suffix = {"alpha": "a", "beta": "b", "candidate": "rc", "final": ""}[level]
    version = Version(
        f"{major}.{minor}.{micro}" + (f"{suffix}{serial}" if suffix else "")
    )
    if "fingerprint" in plan:
        if plan.get("python_version") != str(version):
            _fail("cannot retarget a selected local-package plan; resolve the original manifest again")
        if plan["fingerprint"] != _digest(_json(public(plan))):
            _fail("selected local-package plan changed after environment binding")
        return plan
    packages = [
        p
        for p in plan["packages"]
        if SpecifierSet(p["when_python"]).contains(version, prereleases=True)
    ]
    selected = {**plan, "packages": packages, "python_version": str(version)}
    selected["fingerprint"] = _digest(_json(public(selected)))
    return selected


def public(plan):
    """Path-independent data suitable for retained receipts and comparison."""
    return {
        **{key: value for key, value in plan.items() if key != "fingerprint"},
        "packages": [
            {key: value for key, value in package.items() if key != "_archive"}
            for package in plan["packages"]
        ],
    }


def _directory(venv, plan):
    return Path(venv.root) / CACHE_DIRECTORY / plan["fingerprint"]


def _artifact(venv, plan, package):
    return _directory(venv, plan) / (package["archive_sha256"] + ".tar.gz")


def _retain(path, contents):
    if path.exists():
        if path.is_symlink() or path.read_bytes() != contents:
            raise ValueError(f"retained local-package artifact changed: {path}")
        return
    _replace(path, contents)


def _replace(path, contents):
    """Publish one complete file, used only for the current receipt pointer."""
    if path.is_symlink():
        raise ValueError(f"local-package state path is a symlink: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as temporary:
        temporary.write(contents)
        name = temporary.name
    try:
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def _installed(venv, package, roots):
    """Read installation metadata and payload hashes; never import the package."""
    try:
        matches = []
        for root in roots:
            for directory in root.glob("*.dist-info"):
                stem = directory.name.removesuffix(".dist-info").rsplit("-", 1)[0]
                if canonicalize_name(stem) != package["distribution"]:
                    continue
                metadata = Parser().parsestr((directory / "METADATA").read_text())
                if (
                    canonicalize_name(metadata.get("Name", ""))
                    == package["distribution"]
                ):
                    matches.append((directory, metadata))
        if len(matches) != 1:
            raise ValueError("local distribution is absent or ambiguously visible")
        directory, metadata = matches[0]
        venv_root = Path(venv.root).resolve()
        if not directory.resolve().is_relative_to(venv_root):
            raise ValueError(
                "local distribution was not installed in the selected venv"
            )
        if Version(metadata["Version"]) != Version(package["version"]):
            raise ValueError("installed local distribution has another version")
        direct = (directory / "direct_url.json").read_bytes()
        direct_data = json.loads(direct)
        archive_info = direct_data.get("archive_info", {})
        if archive_info.get("hashes", {}).get("sha256") != package["archive_sha256"]:
            raise ValueError("installed local distribution has another archive origin")
        url = urlsplit(direct_data["url"])
        archive = Path(unquote(url.path))
        if (url.scheme != "file" or url.netloc not in {"", "localhost"}
            or url.query or url.fragment or archive.is_symlink()
            or not archive.resolve().is_relative_to(venv_root / CACHE_DIRECTORY)
            or _digest(archive.read_bytes()) != package["archive_sha256"]):
            raise ValueError("installed local distribution does not refer to its retained archive")
        # Only the installer's venv-specific archive path is normalized for
        # comparison. All other direct-URL metadata and the exact archive hash
        # remain part of the portable installed-payload identity.
        normalized_direct = {**direct_data, "url": "file:///<retained-archive>/" + package["archive_sha256"] + ".tar.gz"}
        record_bytes = (directory / "RECORD").read_bytes()
        checked = []
        for row in csv.reader(io.StringIO(record_bytes.decode())):
            if len(row) != 3:
                raise ValueError("invalid installed distribution RECORD")
            relative, recorded_hash, _size = row
            if not recorded_hash:
                # RECORD itself and interpreter-generated .pyc files are unhashed.
                if relative != directory.name + "/RECORD" and not relative.endswith(
                    ".pyc"
                ):
                    raise ValueError("local distribution contains an unhashed payload")
                continue
            path = directory.parent / relative
            if path.is_symlink() or not path.resolve().is_relative_to(venv_root):
                raise ValueError("installed local package payload leaves its venv")
            algorithm, expected = recorded_hash.split("=", 1)
            data = path.read_bytes()
            actual = (
                base64.urlsafe_b64encode(hashlib.new(algorithm, data).digest())
                .rstrip(b"=")
                .decode()
            )
            if actual != expected:
                raise ValueError("installed local package payload hash mismatch")
            checked.append([relative, _digest(data), path.stat().st_mode & 0o777])
        if not checked:
            raise ValueError("local distribution has no hashed payload")
        return {
            "distribution": package["distribution"],
            "version": str(Version(metadata["Version"])),
            "direct_url_sha256": _digest(direct),
            "direct_url_file": directory.name + "/direct_url.json",
            "normalized_direct_url": normalized_direct,
            "record_sha256": _digest(record_bytes),
            "payload_files": sorted(checked),
        }
    except (OSError, UnicodeError, ValueError, TypeError, KeyError) as error:
        return {"error": str(error)}


def _observed_state(venv, plan, roots):
    if plan is None:
        return None
    manifest_path = _directory(venv, plan) / "source-manifest.json"
    manifest_ok = (
        manifest_path.is_file()
        and not manifest_path.is_symlink()
        and manifest_path.read_bytes() == _json(public(plan))
    )
    packages = []
    for package in plan["packages"]:
        path = _artifact(venv, plan, package)
        archive_ok = (
            path.is_file()
            and not path.is_symlink()
            and _digest(path.read_bytes()) == package["archive_sha256"]
        )
        packages.append(
            {
                "archive": str(path),
                "archive_ok": archive_ok,
                "installed": _installed(venv, package, roots),
            }
        )
    return {
        "provenance": public(plan),
        "source_manifest": str(manifest_path),
        "packages": packages,
        "ready": manifest_ok
        and all(p["archive_ok"] and "error" not in p["installed"] for p in packages),
    }


def _installation_snapshot(plan, observed):
    return {
        "schema": 1,
        "plan_fingerprint": plan["fingerprint"],
        "installed_packages": [package["installed"] for package in observed["packages"]],
    }


def _portable_payload_fingerprint(plan, observed):
    packages = []
    for package, current in zip(plan["packages"], observed["packages"], strict=True):
        installed = current["installed"]
        packages.append({
            "distribution": installed["distribution"],
            "version": installed["version"],
            "archive_sha256": package["archive_sha256"],
            "payload_files": [
                [relative, _digest(_json(installed["normalized_direct_url"]))
                    if relative == installed["direct_url_file"] else digest, mode]
                for relative, digest, mode in installed["payload_files"]
            ],
        })
    # RECORD bytes include the path-specific direct_url hash. Every listed
    # payload is independently checked above; RECORD itself is retained only
    # in the local accepted snapshot, never the cross-venv comparison digest.
    return _digest(_json(packages))


def state(venv, plan, roots):
    current = _observed_state(venv, plan, roots)
    if current is None:
        return None
    accepted = {}
    try:
        pointer = _directory(venv, plan) / "accepted-installation.json"
        if pointer.is_symlink():
            raise ValueError("accepted local installation pointer is a symlink")
        entry = json.loads(pointer.read_bytes())
        digest = entry["sha256"]
        if (set(entry) != {"schema", "sha256"} or entry["schema"] != 1
            or not isinstance(digest, str) or len(digest) != 64
            or any(char not in "0123456789abcdef" for char in digest)):
            raise ValueError("invalid accepted local installation pointer")
        receipt = _directory(venv, plan) / "installations" / (digest + ".json")
        retained = receipt.read_bytes()
        if receipt.is_symlink() or _digest(retained) != digest:
            raise ValueError("accepted local installation receipt changed")
        if retained != _json(_installation_snapshot(plan, current)):
            raise ValueError("current payload differs from the accepted local installation")
        accepted = {"receipt": str(receipt), "sha256": digest}
    except (OSError, UnicodeError, ValueError, TypeError, KeyError) as error:
        accepted = {"error": str(error)}
    current["accepted_installation"] = accepted
    current["ready"] = current["ready"] and "error" not in accepted
    if current["ready"]:
        payload = _portable_payload_fingerprint(plan, current)
        current["payload_fingerprint"] = payload
        current["fingerprint"] = _digest(_json({
            "plan": plan["fingerprint"], "installed_payload": payload,
        }))
    return current


def require_ready(venv, plan, roots):
    current = state(venv, plan, roots)
    if current is not None and not current["ready"]:
        details = [
            p["installed"]["error"]
            for p in current["packages"]
            if "error" in p["installed"]
        ]
        if "error" in current["accepted_installation"]:
            details.append(current["accepted_installation"]["error"])
        if not details:
            details = ["retained source manifest or archive is absent or changed"]
        _fail(
            "declared local packages are not ready in the selected venv: "
            + "; ".join(details)
            + "; run benchmark requirement preparation"
        )
    return current


def prepare(venv, plan, original, roots, base_requirements):
    """Use the same upstream installer and environment for stock and strict."""
    if plan is None:
        return
    try:
        _retain(_directory(venv, plan) / "source-manifest.json", _json(public(plan)))
        archives = []
        for package in plan["packages"]:
            path = _artifact(venv, plan, package)
            if _digest(package["_archive"]) != package["archive_sha256"]:
                raise ValueError("prepared local-package archive changed")
            _retain(path, package["_archive"])
            archives.append(str(path))
        if archives:
            # A same-version distribution from some other origin is insufficient.
            # Resolve all declared archives jointly with the unchanged upstream
            # requirements, so a local dependency cannot silently override a pin.
            # Dependencies and build isolation otherwise retain upstream pip policy.
            original(venv, ["--force-reinstall", *base_requirements, *archives])
        observed = _observed_state(venv, plan, roots)
        if not observed["ready"]:
            raise ValueError("installer did not produce the declared local-package payload: " + str(observed["packages"]))
        # Only a completed real preparation accepts new installed bytes. The
        # cache/execution readiness path can compare but never refresh this
        # receipt, even if payload and RECORD were coherently rewritten.
        receipt = _json(_installation_snapshot(plan, observed))
        digest = _digest(receipt)
        _retain(_directory(venv, plan) / "installations" / (digest + ".json"), receipt)
        _replace(_directory(venv, plan) / "accepted-installation.json", _json({"schema": 1, "sha256": digest}))
        require_ready(venv, plan, roots)
    except (OSError, UnicodeError, ValueError, TypeError) as error:
        _fail(f"local-package preparation: {error}", error)
