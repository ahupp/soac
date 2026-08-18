from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import textwrap


def test_synthetic_closure_code_is_reused_without_hiding_runtime_mutations(
    tmp_path: Path,
) -> None:
    module_name = "synthetic_closure_code_cache_case"
    (tmp_path / f"{module_name}.py").write_text(
        textwrap.dedent(
            """
            def cached(offset):
                return [offset + value for value in range(3)]


            def prepatched(offset):
                return [offset + value for value in range(3)]


            def postpatched(offset):
                return [offset + value for value in range(3)]


            def reentrant(offset):
                return [offset + value for value in range(3)]


            def replaced_module(offset):
                return [offset + value for value in range(3)]


            def original_outer(offset):
                def original_inner(value):
                    return offset + value

                return original_inner
            """
        ),
        encoding="utf-8",
    )

    script = textwrap.dedent(
        f"""
        import json
        import sys
        import types

        sys.path.insert(0, {str(tmp_path)!r})
        from soac.import_hook import install
        install()
        import {module_name} as module
        import soac.bootstrap as bootstrap
        import soac.runtime as runtime

        listcomp_code_events = []

        def audit(event, args):
            if event == "code.__new__" and len(args) >= 3 and args[2] == "<listcomp>":
                listcomp_code_events.append(args[2])

        sys.addaudithook(audit)

        cached_values = [module.cached(offset) for offset in (1, 10, 100)]
        assert cached_values == [[1, 2, 3], [10, 11, 12], [100, 101, 102]]
        canonical_creation_count = len(listcomp_code_events)

        original_factory = runtime.code_with_freevars
        assert original_factory is bootstrap.code_with_freevars

        prepatch_calls = []

        def prepatched_factory(names, is_async, is_generator):
            prepatch_calls.append(tuple(names))
            return original_factory(names, is_async, is_generator)

        runtime.code_with_freevars = prepatched_factory
        try:
            prepatch_values = [module.prepatched(offset) for offset in (2, 20, 200)]
        finally:
            runtime.code_with_freevars = original_factory
        assert prepatch_values == [[2, 3, 4], [20, 21, 22], [200, 201, 202]]
        assert len(prepatch_calls) == 3, prepatch_calls

        assert module.postpatched(3) == [3, 4, 5]
        postpatch_calls = []

        def postpatched_factory(names, is_async, is_generator):
            postpatch_calls.append(tuple(names))
            return original_factory(names, is_async, is_generator)

        runtime.code_with_freevars = postpatched_factory
        try:
            postpatch_values = [module.postpatched(offset) for offset in (30, 300)]
        finally:
            runtime.code_with_freevars = original_factory
        assert postpatch_values == [[30, 31, 32], [300, 301, 302]]
        assert len(postpatch_calls) == 2, postpatch_calls
        assert module.postpatched(4) == [4, 5, 6]

        original_cache = bootstrap._DP_CODE_WITH_FREEVARS_CACHE
        nested_results = []

        class ReentrantCodeCache(dict):
            active = False

            def get(self, key, default=None):
                if not self.active:
                    self.active = True
                    try:
                        nested_results.append(module.reentrant(40))
                    finally:
                        self.active = False
                return super().get(key, default)

        bootstrap._DP_CODE_WITH_FREEVARS_CACHE = ReentrantCodeCache(original_cache)
        try:
            assert module.reentrant(4) == [4, 5, 6]
        finally:
            bootstrap._DP_CODE_WITH_FREEVARS_CACHE = original_cache
        assert nested_results == [[40, 41, 42]], nested_results

        original_runtime_module = sys.modules["soac.runtime"]
        replacement = types.ModuleType("soac.runtime")
        replacement.__dict__.update(original_runtime_module.__dict__)
        replacement_calls = []

        def replacement_factory(names, is_async, is_generator):
            replacement_calls.append(tuple(names))
            return original_factory(names, is_async, is_generator)

        replacement.code_with_freevars = replacement_factory
        sys.modules["soac.runtime"] = replacement
        try:
            replacement_values = [
                module.replaced_module(offset) for offset in (5, 50)
            ]
        finally:
            sys.modules["soac.runtime"] = original_runtime_module
        assert replacement_values == [[5, 6, 7], [50, 51, 52]]
        assert len(replacement_calls) == 2, replacement_calls
        assert module.replaced_module(6) == [6, 7, 8]

        first = module.original_outer(7)
        second = module.original_outer(70)
        assert first is not second
        assert first(1) == 8
        assert second(1) == 71
        assert first.__code__ is second.__code__
        assert first.__code__.co_name == "original_inner"
        assert first.__code__.co_qualname == "original_outer.<locals>.original_inner"
        assert first.__code__.co_freevars == ("offset",)

        print(json.dumps({{
            "canonical_creation_count": canonical_creation_count,
            "prepatch_calls": len(prepatch_calls),
            "postpatch_calls": len(postpatch_calls),
            "replacement_calls": len(replacement_calls),
            "nested_results": nested_results,
        }}))
        """
    )
    env = dict(os.environ)
    env.pop("SOAC_LOG", None)
    env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(tmp_path / "soac-work"),
            "SOAC_OPT_MODE": "apply",
            "SOAC_COMPILE_MODE": "eager",
            "SOAC_BACKGROUND_JIT": "0",
        }
    )
    completed = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        text=True,
        env=env,
        timeout=30,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    result = json.loads(completed.stdout.splitlines()[-1])
    assert result["canonical_creation_count"] == 1, (
        "each synthetic captured-list-comprehension template should prepare "
        "one named code object and reuse it across closure instantiations",
        result,
    )
    assert result["prepatch_calls"] == 3
    assert result["postpatch_calls"] == 2
    assert result["replacement_calls"] == 2
    assert result["nested_results"] == [[40, 41, 42]]
