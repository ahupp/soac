VALUE = 42

# diet-python: validate

def validate_module(module):
    assert module.VALUE == 42

    import ctypes
    import os
    import runpy
    import sys
    import tempfile
    from pathlib import Path

    from soac import _soac_ext


    with tempfile.TemporaryDirectory(prefix="dp_main_alias_") as directory:
        tmp = Path(directory)
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
        previous_main = sys.modules["__main__"]
        previous_package = sys.modules.pop("dp_main_alias_pkg", None)
        entry = f"path:{tmp.resolve()}"
        os.environ["SOAC_MODULE_ENABLED"] = (
            f"{prev_enabled_modules},{entry}" if prev_enabled_modules else entry
        )
        sys.path.insert(0, str(tmp))
        try:
            namespace = runpy.run_module("dp_main_alias_pkg", run_name="__main__")
            assert namespace["VALUE"] == 7
            assert "_dp_module_init" not in namespace
            assert sys.modules["__main__"] is previous_main
            # A runpy name alias and an ambient allow-list do not confer
            # ownership on this ordinary, unselected source function.
            owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner
            owner.argtypes = [ctypes.py_object]
            owner.restype = ctypes.c_void_p
            assert owner(namespace["get_value"]) is None
            assert _soac_ext.strict_function_entry_kind(namespace["get_value"]) is None
        finally:
            sys.path.remove(str(tmp))
            sys.modules.pop("dp_main_alias_pkg", None)
            if previous_package is not None:
                sys.modules["dp_main_alias_pkg"] = previous_package
            if prev_enabled_modules is None:
                os.environ.pop("SOAC_MODULE_ENABLED", None)
            else:
                os.environ["SOAC_MODULE_ENABLED"] = prev_enabled_modules
