"""Ordinary pyperf harness around an offline-authenticated strict workload.

The selected module executes normally through its loader, including setup and
its real seal transition. Only then does the unchanged measurement suffix run
in a separate ordinary namespace. Neither this harness nor its source manifest
is runtime optimization authority.
"""

import os
from pathlib import Path
import sys


def sealed_module_evidence(execution):
    from soac import _soac_ext

    source = execution["source"]
    project = Path(source["project"])
    records = {record["relative_path"]: record for record in source["files"]}
    evidence = []
    for name, relative in source["modules"].items():
        module = sys.modules.get(name)
        if module is None:
            if name == "__main__":
                raise RuntimeError("strict benchmark has no initialized main module")
            continue  # A conditional static import need not execute.
        state = _soac_ext.strict_module_diagnostics(module)
        if (
            not state
            or state.get("schema") != 2
            or state.get("ready") is not True
            or state.get("strict_assign") is not True
            or state.get("checked_attr") is not True
            or state.get("sealed") is not True
            or state.get("module_name") != name
            or state.get("source_path") != str((project / relative).resolve())
            or state.get("source_sha256") != records[relative]["strict_sha256"]
            or state.get("artifact_generation")
            != execution["publication"]["generation"]
        ):
            raise RuntimeError(
                f"benchmark module {name!r} has no matching sealed native owner"
            )
        evidence.append({**state, "source_kind": "project"})
    if not any(state["module_name"] == "__main__" for state in evidence):
        raise RuntimeError("strict benchmark source selection omits its main module")
    return evidence


def run(execution, arguments):
    from soac import import_hook

    script = execution["source"]["strict_script"]
    if not arguments or str(Path(arguments[0]).resolve()) != script:
        raise ValueError(
            "worker entry does not match its offline-selected strict script"
        )
    import_hook.main([script, *arguments[1:]])
    sealed_module_evidence(execution)
    module = sys.modules["__main__"]
    # A real ordinary dictionary keeps generic name-based harness dispatch and
    # loop temporaries ordinary. Its functions still carry their actual strict
    # globals/owners; copied membership grants no admission.
    harness_globals = dict(vars(module))
    source = execution["source"]
    code = compile(
        Path(source["harness_script"]).read_bytes(),
        source["stock_script"],
        "exec",
        flags=source["harness_projection"]["compiler_flags"],
        dont_inherit=True,
    )
    exec(code, harness_globals)
    return 0


def main():
    # sitecustomize is the already-loaded repository worker adapter, not an
    # analyzed project module. Revalidation never invokes the offline checker.
    from sitecustomize import _strict_execution

    execution = _strict_execution()
    if (
        execution is None
        or sys._xoptions.get("soac_strict_config") != execution["deployment"]
    ):
        raise RuntimeError(
            "strict worker entry has no matching native startup authority"
        )
    if os.environ.get("SOAC_PYPERFORMANCE_EXEC_WRAPPED") != "1":
        raise RuntimeError("strict worker entry was not selected by the worker adapter")
    return run(execution, sys.argv[1:])


if __name__ == "__main__":
    raise SystemExit(main())
