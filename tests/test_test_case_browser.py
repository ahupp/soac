"""Source/range and HTTP boundaries of the local scenario metadata browser."""

import copy
import http.client
import json
import signal
import subprocess
import threading
from unittest.mock import patch

from scripts import test_case_browser as browser


def test_checker_timeout_terminates_descendants_and_retains_source_evidence(tmp_path):
    doc = {
        "modules": [
            {
                "name": "m",
                "path": "m.py",
                "source": "# soac: module(checked_attr=true)\nclass C: pass\n",
            }
        ]
    }
    with (
        patch.object(browser.subprocess, "Popen") as popen,
        patch.object(browser.os, "killpg") as killpg,
    ):
        process = popen.return_value.__enter__.return_value
        process.pid = 12345
        process.wait.side_effect = [subprocess.TimeoutExpired("checker", 900), -9]
        try:
            browser.analyze(doc, tmp_path, "python")
        except RuntimeError as error:
            assert "Checker timed out; retained evidence:" in str(error)
        else:
            raise AssertionError("timeout must be reported")
        killpg.assert_called_once_with(12345, signal.SIGKILL)
        assert popen.call_args.kwargs["start_new_session"] is True
    (run,) = tmp_path.iterdir()
    assert (run / "project/m.py").read_text() == doc["modules"][0]["source"]
    assert (run / "signing.key").stat().st_mode & 0o777 == 0o600


def write_scenario(root, source):
    path = root / "case.py"
    path.write_bytes(source.encode("utf-8"))
    return path


def test_ranges_match_decorated_nested_and_repeated_names_with_utf8_crlf():
    source = "label = 'é'\r\n@decorator\r\nclass Same:\r\n    class Same:\r\n        pass\r\n\r\nclass Same:\r\n    pass\r\n"
    definitions = browser.class_definitions(source)
    assert [item["line"] for item in definitions] == [3, 4, 7]
    encoded = source.encode()
    for item in definitions:
        assert encoded[item["start"] : item["end"]].startswith(b"class Same:")
        assert encoded[item["start"] : item["end"]].endswith(b"pass")
    facts = []
    for index, definition in enumerate(definitions):
        facts.append(
            {
                "identity": {
                    "source_range": {
                        "start": definition["start"],
                        "end": definition["end"],
                    },
                    "lexical_qualname": f"class-{index}",
                },
                "instance_fields": [{"name": f"field-{index}"}],
            }
        )
    # A checker range may include the decorator. Joining still uses the exact
    # end position to distinguish the nested class and later same-name class.
    facts[0]["identity"]["source_range"]["start"] = encoded.index(b"@decorator")
    raw = copy.deepcopy(facts)
    policy = {
        "checked_attr": False,
        "class_overrides": [
            {"class_range": facts[0]["identity"]["source_range"], "checked_attr": True}
        ],
    }
    doc = {"modules": [{"name": "example", "source": source, "classes": definitions}]}
    browser.attach_facts(
        doc, {"example": {"classes": facts, "language_policy": policy}}
    )
    assert [item["fact"] for item in definitions] == raw
    assert [item["checked_attr"] for item in definitions] == [True, False, False]
    assert doc["modules"][0]["unmatched_facts"] == []


def test_raw_metadata_preserves_hashes_larger_than_javascript_integers():
    (definition,) = browser.class_definitions("class C:\n    pass\n")
    fact = {
        "identity": {
            "source_range": {"start": definition["start"], "end": definition["end"]},
            "module": {"source_hash": 18446744073709551615},
        }
    }
    doc = {
        "modules": [
            {
                "name": "example",
                "source": "class C:\n    pass\n",
                "classes": [definition],
            }
        ]
    }
    browser.attach_facts(
        doc,
        {
            "example": {
                "classes": [fact],
                "language_policy": {"checked_attr": True, "class_overrides": []},
            }
        },
    )
    assert (
        json.loads(definition["fact_json"])["identity"]["module"]["source_hash"]
        == 18446744073709551615
    )


def test_class_blocks_include_attached_policy_comments_and_decorators():
    source = '''text = """
# soac: class(checked_attr=false)
"""
class Ordinary:
    pass

# soac: class(
#     checked_attr=true,
# )

@decorate(
    option=True,
)
class Selected:
    # soac: class(checked_attr=false)
    @decorate
    class Nested:
        pass

# soac: class(checked_attr=true)
value = 1
class Unrelated:
    pass
'''
    for newline in ("\n", "\r\n"):
        definitions = browser.class_definitions(source.replace("\n", newline))
        assert [(item["line"], item["block_line"]) for item in definitions] == [
            (4, 4),
            (14, 7),
            (17, 15),
            (22, 22),
        ]


def test_ordinary_modules_and_withheld_records_are_distinct():
    doc = {
        "modules": [
            {
                "name": name,
                "source": "class C:\n    pass\n",
                "classes": browser.class_definitions("class C:\n    pass\n"),
            }
            for name in ("ordinary", "selected")
        ]
    }
    browser.attach_facts(doc, {"selected": {"classes": []}})
    assert doc["modules"][0]["classes"][0]["status"] == "ordinary"
    assert doc["modules"][1]["classes"][0]["status"] == "withheld"


def test_unmatched_facts_are_visible_instead_of_lost():
    fact = {"identity": {"source_range": {"start": 99, "end": 100}}}
    doc = {"modules": [{"name": "example", "source": "", "classes": []}]}
    browser.attach_facts(doc, {"example": {"classes": [fact]}})
    assert doc["modules"][0]["unmatched_facts"] == [fact]


def test_definition_records_keep_fields_methods_and_bindings_at_their_source():
    source = "label = 'é'\r\n@decorate\r\ndef shared():\r\n    pass\r\n\r\nclass Base:\r\n    field: int = 0\r\n    @decorate\r\n    def method(self):\r\n        pass\r\n\r\nclass Child(Base):\r\n    alias = shared\r\n"
    encoded = source.encode()
    module_identity = {"module_name": "example", "source_hash": 18446744073709551615}

    def identity(kind, start, end):
        return {
            "module": module_identity,
            "definition_kind": kind,
            "source_range": {"start": start, "end": end},
        }

    classes = browser.class_definitions(source)
    base, child = [identity("class", item["start"], item["end"]) for item in classes]
    field_id = identity(
        "assignment",
        encoded.index(b"field:"),
        encoded.index(b"field:") + len(b"field: int = 0"),
    )
    function_id = identity(
        "function", encoded.index(b"@decorate"), encoded.index(b"pass") + 4
    )
    method_id = identity(
        "function",
        encoded.index(b"@decorate", function_id["source_range"]["end"]),
        base["source_range"]["end"],
    )
    field = {
        "name": "field",
        "annotation_definition": field_id,
        "declaring_class": {"definition": base},
        "value_type": {"kind": "nominal_builtin", "data": {"builtin": "int"}},
    }
    method = {
        "name": "method",
        "implementation": method_id,
        "declaring_class": {"definition": base},
    }
    alias = {
        "name": "alias",
        "implementation": function_id,
        "declaring_class": {"definition": child},
    }
    generated = {
        "name": "__init__",
        "implementation": None,
        "declaring_class": {"definition": child},
    }
    facts = {
        "module": module_identity,
        "language_policy": {"checked_attr": True, "class_overrides": []},
        "classes": [
            {
                "identity": base,
                "instance_fields": [field],
                "methods": [method],
                "class_members": [
                    {
                        "name": "field",
                        "definition": field_id,
                        "value_type": {
                            "kind": "literal",
                            "data": {"kind": "int", "value": "0"},
                        },
                    }
                ],
            },
            {
                "identity": child,
                "instance_fields": [field],
                "methods": [method, alias, generated],
                "class_members": [],
            },
        ],
        "functions": [{"identity": function_id}, {"identity": method_id}],
        "global_bindings": [{"definition": base}, {"definition": function_id}],
    }
    module = {"name": "example", "source": source, "classes": classes}
    browser.attach_facts({"modules": [module]}, {"example": facts})
    records = module["records"]
    assert [(item["line"], item["kind"]) for item in records] == [
        (3, "function"),
        (6, "class"),
        (7, "field"),
        (9, "method"),
        (12, "class"),
    ]
    field_record = next(item for item in records if item["kind"] == "field")
    assert field_record["fact"]["value_type"]["kind"] == "nominal_builtin"
    assert set(json.loads(field_record["fact_json"])) == {"field", "member"}
    assert len(json.loads(field_record["fact_json"])["field"]) == 1
    method_record = next(item for item in records if item["kind"] == "method")
    assert set(json.loads(method_record["fact_json"])) == {"method", "function"}
    function_record = records[0]
    assert set(json.loads(function_record["fact_json"])) == {"function", "binding"}
    assert (
        json.loads(function_record["fact_json"])["function"][0]["identity"]["module"][
            "source_hash"
        ]
        == 18446744073709551615
    )
    assert module["unmatched_facts"] == []


def test_definition_records_do_not_attach_foreign_or_invalid_ranges():
    source = "answer = 1\n"
    module_identity = {"module_name": "example", "source_hash": 1}
    foreign = {
        "definition": {
            "module": {"module_name": "other", "source_hash": 1},
            "source_range": {"start": 0, "end": 10},
        }
    }
    invalid = {
        "identity": {
            "module": module_identity,
            "definition_kind": "function",
            "source_range": {"start": 0, "end": 10},
        }
    }
    module = {"name": "example", "source": source, "classes": []}
    browser.attach_facts(
        {"modules": [module]},
        {
            "example": {
                "module": module_identity,
                "functions": [invalid],
                "global_bindings": [foreign],
            }
        },
    )
    assert module["records"] == []
    assert module["unmatched_facts"] == [invalid, foreign]
    assert json.loads(module["unmatched_json"]) == [invalid, foreign]


def test_description_preserves_module_bytes_package_layout_and_case_boundaries(
    tmp_path,
):
    source = '# module:pkg\r\n# soac: package(checked_attr=true)\r\n\r\n# module:pkg.child\r\ntext = """\r\n# module:fake\r\n"""\r\nclass C:\r\n    pass\r\n\r\n# ok\r\nassert C\r\n# raise:TypeError\r\nraise TypeError()\r\n'
    doc = browser.describe_scenario(write_scenario(tmp_path, source))
    assert [module["path"] for module in doc["modules"]] == [
        "pkg/__init__.py",
        "pkg/child.py",
    ]
    assert doc["modules"][0]["source"] == "# soac: package(checked_attr=true)\r\n\r\n"
    assert "# module:fake\r\n" in doc["modules"][1]["source"]
    assert [case["source"] for case in doc["blocks"]] == [
        "assert C\r\n",
        "raise TypeError()\r\n",
    ]
    assert [case["label"] for case in doc["blocks"]] == ["ok", "raise:TypeError"]
    assert doc["modules"][1]["classes"][0]["line"] == 4


def test_http_allowlist_path_containment_busy_and_source_snapshot_invalidation(
    tmp_path,
):
    scenarios, web = tmp_path / "cases", tmp_path / "web"
    scenarios.mkdir()
    web.mkdir()
    (web / "test-cases.html").write_text("<!doctype html><title>Test</title>")
    path = write_scenario(
        scenarios, "# module:example\nclass C:\n    pass\n# ok\nassert C\n"
    )
    outside = tmp_path / "outside.py"
    outside.write_text("private")
    (scenarios / "link.py").symlink_to(outside)
    app = browser.Browser(scenarios, web, tmp_path / "work", "unused")
    server = browser.ThreadingHTTPServer(("127.0.0.1", 0), browser.handler_for(app))
    thread = threading.Thread(target=server.serve_forever)
    thread.start()

    def request(method, path, headers=None):
        connection = http.client.HTTPConnection(
            "127.0.0.1", server.server_port, timeout=5
        )
        try:
            connection.request(method, path, headers=headers or {})
            response = connection.getresponse()
            return response.status, response.read()
        finally:
            connection.close()

    try:
        with patch.object(browser, "analyze") as analyze:
            status, body = request("GET", "/api/cases")
            assert status == 200
            assert [item["id"] for item in json.loads(body)] == ["case.py"]
            status, body = request("GET", "/api/case?id=case.py")
            doc = json.loads(body)
            assert status == 200 and doc["modules"][0]["classes"][0]["name"] == "C"
            analyze.assert_not_called()
            for identifier in ("../outside.py", "link.py", str(outside), "missing.py"):
                assert request("GET", "/api/case?id=" + identifier)[0] == 400
            assert request("GET", "/signing.key")[0] == 404
            assert request("GET", "/api/cases", {"Host": "attacker.example"})[0] == 403
            assert (
                request(
                    "POST",
                    "/api/analyze?id=case.py",
                    {"Origin": "https://attacker.example"},
                )[0]
                == 403
            )
            assert (
                request(
                    "POST", "/api/analyze?id=case.py", {"Sec-Fetch-Site": "cross-site"}
                )[0]
                == 403
            )
            analyze.assert_not_called()
            app.analysis_lock.acquire()
            try:
                assert request("POST", "/api/analyze?id=case.py")[0] == 409
            finally:
                app.analysis_lock.release()
            analyze.return_value = {**doc, "publication": {"generation": "snapshot"}}
            assert request("POST", "/api/analyze?id=case.py")[0] == 200
            assert "publication" in json.loads(
                request("GET", "/api/case?id=case.py")[1]
            )
            path.write_text(path.read_text() + "# edit\n")
            assert "publication" not in json.loads(
                request("GET", "/api/case?id=case.py")[1]
            )
            analyze.assert_called_once()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
