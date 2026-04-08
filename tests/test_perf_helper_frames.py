import subprocess
from pathlib import Path

import pytest
import _soac_ext


REPO_ROOT = Path(__file__).resolve().parents[1]
EXTENSION_LIB = Path(_soac_ext.__file__).resolve()
RELEASE_DIR = REPO_ROOT / "target" / "release"
REQUIRES_RELEASE_EXTENSION = pytest.mark.skipif(
    EXTENSION_LIB.parent != RELEASE_DIR,
    reason=f"requires installed release _soac_ext, got {EXTENSION_LIB}",
)


def _symbol_body(symbol: str) -> str:
    out = subprocess.check_output(
        ["objdump", "-d", "--demangle", str(EXTENSION_LIB)],
        text=True,
    )
    start = f"<{symbol}>:"
    lines = out.splitlines()
    collecting = False
    body: list[str] = []
    for line in lines:
        if line.endswith(start):
            collecting = True
            continue
        if collecting and line and not line.startswith(" "):
            break
        if collecting:
            body.append(line)
    assert body, f"missing disassembly for {symbol}"
    return "\n".join(body)


def test_disassembles_installed_extension():
    assert EXTENSION_LIB.exists()
    assert EXTENSION_LIB.name == "lib_soac_ext.so"


@REQUIRES_RELEASE_EXTENSION
def test_default_dp_jit_py_vectorcall_stays_tail_call_fast_path():
    body = _symbol_body("soac_jit::jit::specialized_helpers::dp_jit_py_vectorcall")
    assert "jmp" in body


@REQUIRES_RELEASE_EXTENSION
def test_dp_jit_py_vectorcall_with_frame_keeps_helper_frame():
    body = _symbol_body("soac_jit::jit::specialized_helpers::dp_jit_py_vectorcall_with_frame")
    assert "call" in body
    assert "jmp" not in body


@REQUIRES_RELEASE_EXTENSION
def test_default_dp_jit_exact_long_binary_op_stays_tail_call_fast_path():
    body = _symbol_body("soac_jit::jit::specialized_helpers::dp_jit_exact_long_binary_op")
    assert "jmp" in body


@REQUIRES_RELEASE_EXTENSION
def test_dp_jit_exact_long_binary_op_with_frame_keeps_helper_frame():
    body = _symbol_body(
        "soac_jit::jit::specialized_helpers::dp_jit_exact_long_binary_op_with_frame"
    )
    assert "call" in body
    assert "jmp" not in body


@REQUIRES_RELEASE_EXTENSION
def test_default_dp_jit_exact_long_unary_op_stays_tail_call_fast_path():
    body = _symbol_body("soac_jit::jit::specialized_helpers::dp_jit_exact_long_unary_op")
    assert "jmp" in body


@REQUIRES_RELEASE_EXTENSION
def test_dp_jit_exact_long_unary_op_with_frame_keeps_helper_frame():
    body = _symbol_body(
        "soac_jit::jit::specialized_helpers::dp_jit_exact_long_unary_op_with_frame"
    )
    assert "call" in body
    assert "jmp" not in body
