from __future__ import annotations

import os
import subprocess
import sys
import textwrap
from pathlib import Path


def test_profile_mode_scalar_cleanup_root_runs_with_unmaterialized_i64_retire(
    tmp_path: Path,
) -> None:
    module_name = "scalar_cleanup_root_case"
    module_path = tmp_path / f"{module_name}.py"
    module_path.write_text(
        textwrap.dedent(
            """
            Ident1 = 1
            TRUE = 1
            FALSE = 0

            def Func1(CharPar1, CharPar2):
                CharLoc1 = CharPar1
                CharLoc2 = CharLoc1
                if CharLoc2 != CharPar2:
                    return Ident1
                else:
                    return 2

            def Func2(StrParI1, StrParI2):
                IntLoc = 1
                while IntLoc <= 1:
                    if Func1(StrParI1[IntLoc], StrParI2[IntLoc + 1]) == Ident1:
                        CharLoc = "A"
                        IntLoc = IntLoc + 1
                if CharLoc >= "W" and CharLoc <= "Z":
                    IntLoc = 7
                if CharLoc == "X":
                    return TRUE
                else:
                    if StrParI1 > StrParI2:
                        IntLoc = IntLoc + 7
                        return TRUE
                    else:
                        return FALSE

            def run():
                return Func2("DHRYSTONE PROGRAM, 1'ST STRING", "DHRYSTONE PROGRAM, 2'ND STRING")
            """
        ),
        encoding="utf-8",
    )

    env = os.environ.copy()
    env.pop("SOAC_LOG", None)
    env.update(
        {
            "SOAC_MODULE_ENABLED": f"path:{tmp_path}",
            "SOAC_WORK_DIR": str(tmp_path / "soac-work"),
            "SOAC_OPT_MODE": "profile",
        }
    )
    script = textwrap.dedent(
        f"""
        import sys

        sys.path.insert(0, {str(tmp_path)!r})
        from soac.import_hook import install

        install()
        import {module_name} as module

        print(module.run())
        """
    )
    result = subprocess.run(
        [sys.executable, "-c", script],
        check=False,
        capture_output=True,
        env=env,
        text=True,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert result.stdout.strip() == "0"
