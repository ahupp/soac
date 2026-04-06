import importlib.util
import json
from pathlib import Path


def load_module():
    module_path = Path(__file__).resolve().parents[1] / "scripts" / "perf_report_to_speedscope.py"
    spec = importlib.util.spec_from_file_location("perf_report_to_speedscope", module_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_perf_report_parser_preserves_jit_roots_and_caller_context():
    module = load_module()
    report = """
# Overhead       Samples  Shared Object         Symbol                                                    IPC   [IPC Coverage]
# ........  ............  ....................  ........................................................  ....................
#
     5.41%           102  [JIT] tid 939774      [.] py:d:Proc0                                           -      -
     2.80%            53  libpython3.15.so.1.0  [.] _PyEval_EvalFrameDefault                              -      -
           6
                py:d:pystones [JIT]
                py:d:main [JIT]

           5
                PyObject_Vectorcall
""".splitlines(True)

    entries = module.parse_perf_report(report)
    assert [entry.root_frame for entry in entries] == [
        "py:d:Proc0 [JIT]",
        "_PyEval_EvalFrameDefault libpython3.15.so.1.0",
    ]
    assert entries[0].samples == 102
    assert entries[1].caller_stacks == [
        (["py:d:pystones [JIT]", "py:d:main [JIT]"], 6),
        (["PyObject_Vectorcall"], 5),
    ]

    frames, samples, weights = module.build_speedscope_profile(entries)
    frame_names = [frame["name"] for frame in frames]
    assert "py:d:Proc0 [JIT]" in frame_names
    assert "py:d:pystones [JIT]" in frame_names
    assert "_PyEval_EvalFrameDefault libpython3.15.so.1.0" in frame_names

    serialized = json.dumps(
        {
            "shared": {"frames": frames},
            "profiles": [{"samples": samples, "weights": weights}],
        }
    )
    assert "py:d:Proc0 [JIT]" in serialized
