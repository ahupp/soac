VALUE = 42

# diet-python: validate

def validate_module(module):
    assert module.VALUE == 42

    import os
    from pathlib import Path
    import runpy
    import sys
    import tempfile


    tmp = Path(tempfile.mkdtemp(prefix="dp_main_alias_"))
    pkg = tmp / "dp_main_alias_pkg"
    pkg.mkdir()
    (pkg / "__init__.py").write_text("", encoding="utf-8")
    (pkg / "__main__.py").write_text(
        "def get_value():\n"
        "    return 7\n"
        "VALUE = get_value()\n",
        encoding="utf-8",
    )

    prev_enabled_modules = os.environ.get("SOAC_MODULE_ENABLED")
    entry = f"path:{tmp.resolve()}"
    os.environ["SOAC_MODULE_ENABLED"] = (
        f"{prev_enabled_modules},{entry}" if prev_enabled_modules else entry
    )
    sys.path.insert(0, str(tmp))
    sys.modules.pop("__main__", None)
    try:
        namespace = runpy.run_module("dp_main_alias_pkg", run_name="__main__")
        assert namespace["VALUE"] == 7
        assert "_dp_module_init" not in namespace
    finally:
        if sys.path and sys.path[0] == str(tmp):
            sys.path.pop(0)
        else:
            try:
                sys.path.remove(str(tmp))
            except ValueError:
                pass
        if prev_enabled_modules is None:
            os.environ.pop("SOAC_MODULE_ENABLED", None)
        else:
            os.environ["SOAC_MODULE_ENABLED"] = prev_enabled_modules
