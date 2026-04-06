import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
RELEASE_LIB = REPO_ROOT / "target" / "release" / "lib_soac_ext.so"


def _symbol_body(symbol: str) -> str:
    out = subprocess.check_output(
        ["objdump", "-d", "--demangle", str(RELEASE_LIB)],
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


def test_dp_jit_py_vectorcall_keeps_helper_frame():
    body = _symbol_body("soac_jit::jit::specialized_helpers::dp_jit_py_vectorcall")
    assert "call" in body
    assert "jmp" not in body

