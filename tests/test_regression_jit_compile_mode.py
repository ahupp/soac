from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from soac import runtime as DP


def test_private_clif_compile_helpers_are_not_runtime_surface():
    assert not hasattr(DP, "_bb_enable_lazy_vectorcall")
    assert not hasattr(DP, "_BIND_KIND_FUNCTION")
    assert not hasattr(DP, "_BIND_KIND_GENERATOR_RESUME")
    assert not hasattr(DP, "_BIND_KIND_ASYNC_GENERATOR_RESUME")
