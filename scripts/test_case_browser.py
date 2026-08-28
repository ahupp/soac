"""Local source-scenario browser backed by the real offline type checker.

Only scenario module sections are analyzed. Neither modules nor validation
blocks are executed. Publications are inspection evidence, not runtime proof.
"""

from __future__ import annotations

import argparse
import ast
import bisect
import hashlib
import io
import json
import os
import re
import signal
import subprocess
import sys
import tempfile
import threading
import tokenize
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from tests._strict_scenarios import _declares_package, parse_strict_scenario


def source_lines(source: str) -> list[str]:
    return io.StringIO(source, newline="").readlines()


def class_definitions(source: str) -> list[dict]:
    """Use UTF-8 offsets, as both Python's AST and checker source ranges do."""
    tree = ast.parse(source)
    lines = source_lines(source)
    offsets = [0]
    for line in lines:
        offsets.append(offsets[-1] + len(line.encode("utf-8")))
    comments = {
        token.start[0]: token
        for token in tokenize.generate_tokens(
            io.StringIO(source, newline=None).readline
        )
        if token.type == tokenize.COMMENT and not token.line[: token.start[1]].strip()
    }

    def block_line(node):
        # This only groups displayed source. The checker still owns directive
        # validation and binding; comment-like text in strings is not a prefix.
        first = min([node.lineno, *(item.lineno for item in node.decorator_list)])
        start = first
        indentation = lines[node.lineno - 1][: node.col_offset]
        for line in range(first - 1, 0, -1):
            comment = comments.get(line)
            if comment is None:
                if lines[line - 1].strip():
                    break
                continue
            if comment.line[: comment.start[1]] == indentation and re.match(
                r"#[ \t]*soac[ \t]*:[ \t]*class[ \t]*\(", comment.string
            ):
                start = line
        return start

    return sorted(
        [
            {
                "name": node.name,
                "line": node.lineno,
                "block_line": block_line(node),
                "start": offsets[node.lineno - 1] + node.col_offset,
                "end": offsets[node.end_lineno - 1] + node.end_col_offset,
            }
            for node in ast.walk(tree)
            if isinstance(node, ast.ClassDef)
        ],
        key=lambda item: item["start"],
    )


def definition_records(module: dict, facts: dict | None) -> list[dict]:
    """Place published declarations at their exact source, without name joins.

    A declaration can have several facets (a field and its class default, or
    a function and its method binding). Keep those together, preserving each
    original record in the raw view. Inherited/generated members remain in
    their class record rather than borrowing another declaration's line.
    """
    if facts is None:
        return []
    source = module["source"]
    offsets = [0]
    for line in source_lines(source):
        offsets.append(offsets[-1] + len(line.encode("utf-8")))
    declarations = []

    def visit(node, owner=None):
        if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            item = {
                "start": offsets[node.lineno - 1] + node.col_offset,
                "end": offsets[node.end_lineno - 1] + node.end_col_offset,
                "line": node.lineno,
                "kind": "class" if isinstance(node, ast.ClassDef) else "function",
                "owner": owner,
            }
            declarations.append(item)
            owner = item["start"]
        for child in ast.iter_child_nodes(node):
            visit(child, owner)

    visit(ast.parse(source))
    records = {}

    def locate(identity):
        if not identity or identity.get("module") != facts.get("module"):
            return None
        extent = identity["source_range"]
        start, end = extent["start"], extent["end"]
        if not 0 <= start < end <= offsets[-1]:
            return None
        if identity.get("definition_kind") in ("class", "function"):
            candidates = [
                item
                for item in declarations
                if start <= item["start"]
                and end == item["end"]
                and item["kind"] == identity["definition_kind"]
            ]
            return min(candidates, key=lambda item: item["start"], default=None)
        return {"start": start, "end": end, "line": bisect.bisect_right(offsets, start)}

    def add(kind, fact, identity, **properties):
        location = locate(identity)
        if location is None:
            module["unmatched_facts"].append(fact)
            return
        key = (location["start"], location["end"])
        record = records.setdefault(
            key, {**location, "kind": kind, "fact": fact, "facets": {}, **properties}
        )
        record["facets"].setdefault(kind, []).append(fact)

    # Primary facts come first; overlapping binding facets must not replace
    # the annotated field type with the narrower literal class default.
    for definition in module["classes"]:
        if definition.get("fact"):
            add(
                "class",
                definition["fact"],
                definition["fact"]["identity"],
                checked_attr=definition["checked_attr"],
                block_line=definition["block_line"],
            )
    for fact in facts.get("classes", []):
        owner = locate(fact["identity"])
        for field in fact.get("instance_fields", []):
            identity = field.get("annotation_definition")
            if (
                identity
                and field.get("declaring_class", {}).get("definition")
                == fact["identity"]
            ):
                add("field", field, identity)
        for method in fact.get("methods", []):
            identity = method.get("implementation")
            location = locate(identity)
            if (
                owner
                and location
                and location.get("owner") == owner["start"]
                and method.get("declaring_class", {}).get("definition")
                == fact["identity"]
            ):
                add("method", method, identity)
    for fact in facts.get("functions", []):
        add("function", fact, fact["identity"])
    for fact in facts.get("classes", []):
        for member in fact.get("class_members", []):
            if member.get("definition"):
                add("member", member, member["definition"])
    for fact in facts.get("global_bindings", []):
        add("binding", fact, fact["definition"])
    for record in records.values():
        # JSON text preserves u64 source hashes across the JavaScript boundary.
        record["fact_json"] = json.dumps(
            record.pop("facets"), indent=2, ensure_ascii=False
        )
    return sorted(records.values(), key=lambda item: (item["line"], item["start"]))


def describe_scenario(path: Path) -> dict:
    original = path.read_bytes()
    scenario = parse_strict_scenario(path)
    names = {module.name for module in scenario.modules}
    modules = []
    for module in scenario.modules:
        package = _declares_package(module.source) or any(
            name.startswith(module.name + ".") for name in names
        )
        modules.append(
            {
                "name": module.name,
                "path": module.name.replace(".", "/")
                + ("/__init__.py" if package else ".py"),
                "source": module.source,
                "classes": class_definitions(module.source),
            }
        )
    if path.read_bytes() != original:
        raise ValueError("Scenario changed while being read; reload it")
    return {
        "digest": hashlib.sha256(original).hexdigest(),
        "modes": scenario.modes,
        "modules": modules,
        "blocks": [
            {
                "line": block.line,
                "label": block.label,
                "source": "".join(source_lines(block.source)[block.line :]),
            }
            for block in scenario.blocks
        ],
    }


def attach_facts(document: dict, shards: dict[str, dict]) -> dict:
    """Join by exact definition extent, never by class name (which can repeat)."""
    for module in document["modules"]:
        facts = shards.get(module["name"])
        module["facts"] = facts
        by_start = {}
        for fact in (facts or {}).get("classes", []):
            extent = fact["identity"]["source_range"]
            candidates = [
                definition
                for definition in module["classes"]
                if extent["start"] <= definition["start"]
                and extent["end"] == definition["end"]
            ]
            if candidates:
                # An outer class and its last nested class can have the same
                # end. The first definition inside the extent owns the record.
                owner = min(candidates, key=lambda definition: definition["start"])
                if owner["start"] in by_start:
                    raise ValueError("ambiguous checker class source range")
                by_start[owner["start"]] = fact
        for definition in module["classes"]:
            fact = by_start.get(definition["start"])
            definition["fact"] = fact
            # SourceIdentity contains u64 hashes. Send exact JSON as text as
            # well, so JavaScript's Number cannot round the raw metadata view.
            definition["fact_json"] = (
                json.dumps(fact, indent=2, ensure_ascii=False) if fact else None
            )
            if fact:
                policy = facts["language_policy"]
                source_range = fact["identity"]["source_range"]
                definition["checked_attr"] = next(
                    (
                        rule["checked_attr"]
                        for rule in policy["class_overrides"]
                        if rule["class_range"] == source_range
                    ),
                    policy["checked_attr"],
                )
            definition["status"] = (
                "inferred" if fact else "ordinary" if facts is None else "withheld"
            )
        # A format/range drift must be visible, never silently hide a record.
        mapped = [item["fact"] for item in module["classes"] if item.get("fact")]
        module["unmatched_facts"] = [
            fact for fact in (facts or {}).get("classes", []) if fact not in mapped
        ]
        module["records"] = definition_records(module, facts)
        module["unmatched_json"] = json.dumps(
            module["unmatched_facts"], indent=2, ensure_ascii=False
        )
    return document


def analyze(document: dict, work: Path, python: str) -> dict:
    work.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix="analysis-", dir=work))
    project = run / "project"
    project.mkdir()
    for module in document["modules"]:
        path = project / module["path"]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(module["source"].encode("utf-8"))
    key = run / "signing.key"
    with key.open("xb") as output:
        os.chmod(key, 0o600)
        output.write(os.urandom(32))
    command = [
        python,
        str(ROOT / "scripts/run_ty.py"),
        "--debug-build",
        "--",
        "check",
        "--project",
        str(project),
        "--python",
        python,
        "--signing-key",
        str(key),
        "--output",
        str(run / "artifacts"),
        "--deployment",
        str(run / "deployment.json"),
    ]
    for module in document["modules"]:
        command.extend(["--module", f"{module['name']}={module['path']}"])
    # File-backed logs retain all build/checker diagnostics without unbounded
    # capture. The verified runner owns source locking through checker use.
    with (
        (run / "stdout.log").open("w") as stdout,
        (run / "stderr.log").open("w") as stderr,
        subprocess.Popen(
            command, cwd=ROOT, stdout=stdout, stderr=stderr, start_new_session=True
        ) as process,
    ):
        try:
            returncode = process.wait(timeout=900)
        except subprocess.TimeoutExpired as error:
            # The verified runner launches Cargo/the checker. Terminate
            # that entire group before releasing the analysis slot.
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise RuntimeError(
                f"Checker timed out; retained evidence: {run}"
            ) from error
    with (run / "stderr.log").open("rb") as log:
        log.seek(max(0, log.seek(0, os.SEEK_END) - 24000))
        diagnostics = log.read().decode("utf-8", errors="replace")
    if returncode:
        raise RuntimeError(
            f"Checker failed (exit {returncode}); evidence: {run}\n{diagnostics}"
        )
    publication = json.loads((run / "stdout.log").read_text())
    artifact = Path(publication["artifact_directory"])
    manifest = json.loads((artifact / "manifest.json").read_text())["manifest"]
    shards = {}
    for index in manifest["modules"]:
        contents = (
            artifact / "modules" / f"{index['shard_digest']}.soac-types"
        ).read_bytes()
        if hashlib.sha256(contents).hexdigest() != index["shard_digest"]:
            raise ValueError("checker shard digest mismatch")
        facts = json.loads(contents)
        module = next(
            item
            for item in document["modules"]
            if item["name"] == facts["module"]["module_name"]
        )
        if (
            facts["source_digest"]
            != hashlib.sha256(module["source"].encode("utf-8")).hexdigest()
        ):
            raise ValueError("checker source does not match displayed source")
        shards[module["name"]] = facts
    document.update(publication=publication, diagnostics=diagnostics, evidence=str(run))
    return attach_facts(document, shards)


class Browser:
    def __init__(self, scenarios: Path, web: Path, work: Path, python: str):
        self.scenarios = scenarios.resolve()
        self.web, self.work, self.python = web, work, python
        self.analysis_lock = threading.Lock()
        self.snapshots: dict[str, dict] = {}

    def resolve(self, identifier: str) -> Path:
        path = (self.scenarios / identifier).resolve()
        if (
            not path.is_relative_to(self.scenarios)
            or path.suffix != ".py"
            or not path.is_file()
        ):
            raise ValueError("Unknown scenario")
        return path

    def catalog(self) -> list[dict]:
        result = []
        for path in sorted(self.scenarios.rglob("*.py")):
            identifier = path.relative_to(self.scenarios).as_posix()
            if path.resolve().is_relative_to(self.scenarios):
                try:
                    doc = describe_scenario(path)
                    result.append(
                        {
                            "id": identifier,
                            "cases": len(doc["blocks"]),
                            "classes": [
                                c["name"] for m in doc["modules"] for c in m["classes"]
                            ],
                            "modes": doc["modes"],
                        }
                    )
                except (ValueError, SyntaxError) as error:
                    result.append(
                        {
                            "id": identifier,
                            "cases": 0,
                            "classes": [],
                            "error": str(error),
                        }
                    )
        return result


def handler_for(browser: Browser):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            print(
                f"{self.address_string()} {format % args}", file=sys.stderr, flush=True
            )

        def respond(self, status: int, value, kind="application/json"):
            data = json.dumps(value).encode() if kind == "application/json" else value
            self.send_response(status)
            self.send_header("Content-Type", kind + "; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header(
                "Content-Security-Policy",
                "default-src 'self'; script-src 'self'; style-src 'self'; frame-ancestors 'none'",
            )
            self.end_headers()
            self.wfile.write(data)

        def local_request(self) -> bool:
            host = self.headers.get("Host", "")
            allowed = {
                f"localhost:{self.server.server_port}",
                f"127.0.0.1:{self.server.server_port}",
            }
            origin = self.headers.get("Origin")
            return (
                host in allowed
                and (origin is None or origin == f"http://{host}")
                and self.headers.get("Sec-Fetch-Site")
                not in {"cross-site", "same-site"}
            )

        def route(self, analyze_request=False):
            if not self.local_request():
                self.respond(403, {"error": "Local same-origin requests only"})
                return
            url = urlsplit(self.path)
            try:
                if not analyze_request and url.path in {
                    "/",
                    "/test-cases.html",
                    "/test-cases.js",
                    "/test-cases.css",
                }:
                    name = "test-cases.html" if url.path == "/" else url.path[1:]
                    kind = {
                        ".html": "text/html",
                        ".js": "text/javascript",
                        ".css": "text/css",
                    }[Path(name).suffix]
                    self.respond(200, (browser.web / name).read_bytes(), kind)
                elif not analyze_request and url.path == "/api/cases":
                    self.respond(200, browser.catalog())
                elif url.path == ("/api/analyze" if analyze_request else "/api/case"):
                    identifier = parse_qs(url.query).get("id", [""])[0]
                    document = describe_scenario(browser.resolve(identifier))
                    document["id"] = identifier
                    snapshot = browser.snapshots.get(identifier)
                    if (
                        not analyze_request
                        and snapshot
                        and snapshot["digest"] == document["digest"]
                    ):
                        document = snapshot
                    if analyze_request:
                        if not browser.analysis_lock.acquire(blocking=False):
                            self.respond(
                                409,
                                {
                                    "error": "Another analysis is running. Try again when it finishes."
                                },
                            )
                            return
                        try:
                            document = analyze(document, browser.work, browser.python)
                            browser.snapshots[identifier] = document
                        finally:
                            browser.analysis_lock.release()
                    self.respond(200, document)
                else:
                    self.respond(404, {"error": "Not found"})
            except (ValueError, OSError, RuntimeError, SyntaxError) as error:
                self.respond(
                    400 if isinstance(error, (ValueError, SyntaxError)) else 500,
                    {"error": str(error)},
                )

        def do_GET(self):
            self.route()

        def do_POST(self):
            self.route(analyze_request=True)

    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=8002)
    options = parser.parse_args()
    browser = Browser(
        ROOT / "tests/strict_scenarios",
        ROOT / "web",
        ROOT / "work/test-case-browser",
        sys.executable,
    )
    server = ThreadingHTTPServer(("127.0.0.1", options.port), handler_for(browser))
    print(f"SOAC test cases: http://127.0.0.1:{server.server_port}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
