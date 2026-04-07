import _soac_ext

from tests._integration import transformed_module


def test_transformed_modules_use_visible_indexed_module_type(tmp_path):
    assert _soac_ext.IndexedModuleType.__name__ == "IndexedModuleType"

    with transformed_module(tmp_path, "indexed_module_type_visible", "answer = 42\n") as module:
        assert type(module) is _soac_ext.IndexedModuleType
        assert module.answer == 42
        assert module.__dict__["answer"] == 42
