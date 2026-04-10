from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

from tests._integration import transformed_module


def test_transformed_functions_expose_original_code_objects(tmp_path: Path) -> None:
    source = '''
def outer(a):
    x = 10

    def inner(b):
        return a + b + x

    return inner


class Example:
    def method(self):
        return 42
'''

    with transformed_module(tmp_path, "original_code_object", source) as module:
        inner = module.outer(3)

        assert module.outer.__code__.co_name == "outer"
        assert module.outer.__code__.co_qualname == "outer"
        assert module.outer.__code__.co_firstlineno == 2
        assert module.outer.__code__.co_filename.endswith("original_code_object.py")

        assert inner(4) == 17
        assert inner.__code__.co_name == "inner"
        assert inner.__code__.co_qualname == "outer.<locals>.inner"
        assert inner.__code__.co_firstlineno == 5
        assert inner.__code__.co_freevars == ("a", "x")

        assert module.Example().method() == 42
        assert module.Example.method.__code__.co_name == "method"
        assert module.Example.method.__code__.co_qualname == "Example.method"
        assert module.Example.method.__code__.co_firstlineno == 12


def test_generated_class_helpers_do_not_lazy_jit_during_import(tmp_path: Path) -> None:
    log_path = tmp_path / "events.jsonl"
    module_path = tmp_path / "class_helper_import_storm.py"
    class_defs = "\n".join(
        f"""
class C{index}:
    value = {index}

    def method(self):
        return self.value
"""
        for index in range(8)
    )
    module_path.write_text(class_defs, encoding="utf-8")
    env = {
        **os.environ,
        "DIET_PYTHON_ALLOW_TEMP": "1",
        "DIET_PYTHON_INTEGRATION_ONLY": "0",
        "DIET_PYTHON_MODE": "transform",
        "SOAC_LOG": f"soac_jit_codegen=info;json={log_path}",
    }

    subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                f"sys.path.insert(0, {str(tmp_path)!r}); "
                "from soac.import_hook import install; "
                "install(); "
                "import class_helper_import_storm as module; "
                "assert [getattr(module, f'C{i}')().method() for i in range(8)] == list(range(8))"
            ),
        ],
        check=True,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )

    rows = [
        json.loads(line)
        for line in log_path.read_text(encoding="utf-8").splitlines()
        if line.strip()
    ]
    class_helper_codegen = [
        row
        for row in rows
        if row.get("event") == "soac.jit_codegen"
        and row["module_name"].endswith("class_helper_import_storm")
        and row["function_qualname"].startswith(("_dp_class_ns_", "_dp_define_class_"))
    ]
    assert class_helper_codegen == []
