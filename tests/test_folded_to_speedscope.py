import importlib.util
import json
from pathlib import Path


def load_module():
    module_path = Path(__file__).resolve().parents[1] / "scripts" / "folded_to_speedscope.py"
    spec = importlib.util.spec_from_file_location("folded_to_speedscope", module_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_folded_parser_preserves_jit_and_c_api_context():
    module = load_module()
    folded = [
        "python;soac_jit::lazy_clif_vectorcall;py:v:main;py:d:main;py:d:pystones;py:d:Proc0 17\n",
        "python;soac_jit::lazy_clif_vectorcall;py:v:main;py:d:main;py:d:pystones;py:d:Proc0;PyObject_SetAttr 5\n",
        "python;soac_jit::lazy_clif_vectorcall;py:v:main;py:d:main;py:d:pystones;py:d:Proc0;py:d:Proc1 9\n",
    ]

    frames, samples, weights = module.parse_folded_stacks(folded)
    frame_names = [frame["name"] for frame in frames]
    assert "py:v:main" in frame_names
    assert "py:d:Proc0" in frame_names
    assert "PyObject_SetAttr" in frame_names

    proc0_frame = frame_names.index("py:d:Proc0")
    setattr_frame = frame_names.index("PyObject_SetAttr")
    assert any(sample[-1] == proc0_frame for sample in samples)
    assert any(sample[-1] == setattr_frame for sample in samples)
    assert weights == [17, 5, 9]

    serialized = json.dumps(
        {
            "shared": {"frames": frames},
            "profiles": [{"samples": samples, "weights": weights}],
        }
    )
    assert "py:d:Proc0" in serialized
    assert "PyObject_SetAttr" in serialized
