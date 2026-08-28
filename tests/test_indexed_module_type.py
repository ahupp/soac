"""Strict storage does not replace CPython's visible module type."""

import pytest

from tests._strict_integration import create_strict_project


@pytest.fixture(scope="module")
def native_module_type_project(tmp_path_factory):
    return create_strict_project(
        tmp_path_factory.mktemp("strict-native-module-type"),
        {
            "observed.py": """
                # soac: module(strict_assign=true, checked_attr=true)
                answer = 42

                def read():
                    return answer

                def update(value):
                    global answer
                    answer = value
            """,
        },
        modules={"observed": "observed.py"},
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_strict_modules_keep_native_visible_type_and_sealed_bindings(
    native_module_type_project, entry_interpreter
):
    expected = "entry_interpreter" if entry_interpreter else "checked_native"
    native_module_type_project.run(
        f"""
        import types
        import observed
        from soac import _soac_ext
        from soac.strict import StrictMutationError

        assert type(observed) is types.ModuleType
        assert _soac_ext.strict_module_diagnostics(observed)['sealed'] is True
        assert _soac_ext.strict_function_entry_kind(observed.read) == {expected!r}
        assert observed.answer == 42
        assert observed.__dict__['answer'] == 42
        assert observed.read() == 42
        observed.update(63)
        assert observed.__dict__['answer'] == observed.read() == 63
        observed.answer = 84
        assert observed.__dict__['answer'] == observed.read() == 84
        original = observed.read
        for replace in (
            lambda: setattr(observed, 'read', lambda: 0),
            lambda: observed.__dict__.__setitem__('read', lambda: 0),
        ):
            try:
                replace()
            except StrictMutationError:
                pass
            else:
                raise AssertionError('sealed function binding was replaced')
            assert observed.read is original
        """,
        entry_interpreter=entry_interpreter,
    )
