from __future__ import annotations

from pathlib import Path

import pytest

from tests._integration import exec_integration_validation, stock_module
from tests._strict_integration import create_strict_project


@pytest.fixture(
    params=[
        pytest.param("stock", id="stock"),
        pytest.param("soac", id="soac"),
        pytest.param("cpython", id="cpython"),
    ]
)
def class_mode(request):
    return request.param


def _retained_cell_refusal_program(module_name, class_identity):
    expected = (
        f"retained class {class_identity}: Store __static_attributes__ "
        "has no canonical native slot access"
    )
    return f"""
import importlib
import sys
from soac.strict import StrictRuntimeUnavailableError

assert {module_name!r} not in sys.modules
try:
    importlib.import_module({module_name!r})
except StrictRuntimeUnavailableError as error:
    assert type(error) is StrictRuntimeUnavailableError
    assert str(error).startswith({expected!r}), str(error)
else:
    raise AssertionError('an unrepresented retained class cell operation was admitted')
assert {module_name!r} not in sys.modules
"""


def _run_class_case(
    tmp_path,
    mode,
    module_name,
    source,
    validation,
    *,
    required_functions,
    checker_diagnostics=(),
    retained_missing_cell_class=None,
):
    if mode == "stock":
        with stock_module(tmp_path, module_name, source) as module:
            exec_integration_validation(
                validation, module, Path(__file__), mode="stock"
            )
        return

    assert mode in {"soac", "cpython"}, mode
    root = tmp_path / "strict"
    filename = f"{module_name}.py"

    def publish():
        return create_strict_project(
            root,
            {filename: "from __future__ import strict\n" + source},
            modules={module_name: filename},
            backend=mode,
        )

    if checker_diagnostics:
        # These exact, otherwise unchanged sources violate ordinary ty rules.
        # This is analysis-only evidence, never native execution acceptance.
        with pytest.raises(AssertionError, match="actual checker rejected fixture"):
            publish()
        errors = (root / "checker.stderr.log").read_text()
        for diagnostic in checker_diagnostics:
            assert diagnostic in errors, errors
        assert not (root / "authority" / "deployment.json").exists()
        return

    project = publish()
    if mode == "soac" and retained_missing_cell_class is not None:
        # Both retained entries must explicitly refuse before source execution.
        # PanicException and unrelated runtime errors are deliberately uncaught.
        for entry_interpreter in (False, True):
            project.run(
                _retained_cell_refusal_program(module_name, retained_missing_cell_class),
                backend="soac",
                entry_interpreter=entry_interpreter,
            )
        return
    # run_case proves native source/generation/body ownership. On the CPython
    # backend it checks all three compilation counters before and after use.
    # Positive branches still accept no runtime exception or missing-receipt panic.
    project.run_case(
        module_name,
        validation,
        Path(__file__),
        required_functions=required_functions,
    )


def test_static_attributes_include_only_compiler_recorded_self_stores(
    tmp_path, class_mode
):
    source = """
class Subject:
    def collect(self, other):
        self.zeta = self.alpha = 1
        self.alpha = 2
        self.__private = 3
        self.annotated_with_value: int = 4
        self.annotated_without_value: int
        self.augmented_only += 1
        del self.deleted_only
        other.alias_only = 5
        self.parent.child_only = 6
        for self.loop_target in ():
            pass
        [value for self.comprehension_target in () for value in ()]

    @staticmethod
    def textual_self(value):
        self = value
        self.textual_name = 7
"""

    validation = """
def validate_module(module):
    assert module.Subject.__static_attributes__ == (
        "__private",
        "alpha",
        "annotated_with_value",
        "comprehension_target",
        "loop_target",
        "textual_name",
        "zeta",
    )
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_store_shapes',
        source,
        validation,
        required_functions=('Subject.collect', 'Subject.textual_self'),
        checker_diagnostics=(
            'unresolved-attribute: Object of type `Self@collect` has no attribute `augmented_only`',
            'unresolved-attribute: Object of type `Self@collect` has no attribute `deleted_only`',
            'unresolved-attribute: Object of type `Self@collect` has no attribute `parent`',
        ),
    )


def test_static_attributes_follow_nearest_lexical_class_through_nested_scopes(
    tmp_path, class_mode
):
    source = """
class Outer:
    def method(self):
        self.outer = 1

        def nested(self):
            self.from_nested_function = 2
            return lambda self: [item for self.from_lambda in () for item in ()]

        class Local:
            def local_method(self):
                self.local_only = 3

    class Inner:
        def method(self):
            self.inner = 4

            def nested(self):
                self.inner_nested = 5
"""

    validation = """
def validate_module(module):
    assert module.Outer.__static_attributes__ == (
        "from_lambda",
        "from_nested_function",
        "outer",
    )
    assert module.Outer.Inner.__static_attributes__ == ("inner", "inner_nested")
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_nested_scopes',
        source,
        validation,
        required_functions=('Outer.method', 'Outer.Inner.method'),
    )


def test_static_attributes_overwrite_explicit_class_body_assignments(
    tmp_path, class_mode
):
    source = """
class Explicit:
    __static_attributes__ = ("manually_chosen",)

    def method(self):
        self.inferred = 1


class Empty:
    __static_attributes__ = ("manually_chosen",)
"""

    validation = """
def validate_module(module):
    assert module.Explicit.__static_attributes__ == ("inferred",)
    assert module.Empty.__static_attributes__ == ()
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_explicit_overwrite',
        source,
        validation,
        required_functions=('Explicit.method',),
    )


def test_static_attributes_are_written_after_body_and_before_metaclass_creation(
    tmp_path, class_mode
):
    source = """
EVENTS = []


class Namespace(dict):
    def __setitem__(self, key, value):
        if key == "__static_attributes__":
            EVENTS.append(("static-write", tuple(value)))
        elif key == "__annotate_func__":
            EVENTS.append(("annotation-write", callable(value)))
        elif key == "marker":
            EVENTS.append(("body-write", value))
        dict.__setitem__(self, key, value)


class Meta(type):
    @staticmethod
    def __prepare__(name, bases):
        return Namespace()

    def __new__(meta, name, bases, namespace):
        EVENTS.append(("create", namespace["__static_attributes__"]))
        return type.__new__(meta, name, bases, namespace)


class Observed(metaclass=Meta):
    __static_attributes__ = ("manual",)
    marker: str = "class body"

    def method(self):
        self.inferred = 1
"""

    validation = """
def validate_module(module):
    assert module.Observed.__static_attributes__ == ("inferred",)
    assert module.EVENTS == [
        ("static-write", ("manual",)),
        ("body-write", "class body"),
        ("annotation-write", True),
        ("static-write", ("inferred",)),
        ("create", ("inferred",)),
    ]
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_namespace_events',
        source,
        validation,
        required_functions=('Namespace.__setitem__', 'Meta.__prepare__', 'Meta.__new__'),
        checker_diagnostics=(
            'invalid-method-override: Invalid override of method `__prepare__`',
        ),
    )


def test_static_attributes_are_not_inherited_from_base_constructors(
    tmp_path, class_mode
):
    source = """
class Base:
    def __init__(self):
        self.from_base = 1


class InheritedOnly(Base):
    pass


class OwnFields(Base):
    def method(self):
        self.from_child = 2
"""

    validation = """
def validate_module(module):
    assert module.Base.__static_attributes__ == ("from_base",)
    assert module.InheritedOnly.__static_attributes__ == ()
    assert module.OwnFields.__static_attributes__ == ("from_child",)
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_inherited_owners',
        source,
        validation,
        required_functions=('Base.__init__', 'OwnFields.method'),
    )


def test_static_attributes_skip_current_class_body_and_attribute_nested_body_to_parent(
    tmp_path, class_mode
):
    source = """
class Carrier:
    pass


class Outer:
    self = Carrier()
    self.outer_body_only = 1

    def method(self):
        self.outer_method = 2

    class Inner:
        self = Carrier()
        self.inner_body = 3

        def method(self):
            self.inner_method = 4
"""

    validation = """
def validate_module(module):
    assert module.Outer.__static_attributes__ == ("inner_body", "outer_method")
    assert module.Outer.Inner.__static_attributes__ == ("inner_method",)
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_direct_class_body',
        source,
        validation,
        required_functions=('Outer.method', 'Outer.Inner.method'),
        checker_diagnostics=(
            'unresolved-attribute: Unresolved attribute `outer_body_only` on type `Carrier`',
            'unresolved-attribute: Unresolved attribute `inner_body` on type `Carrier`',
        ),
    )


def test_static_attributes_compiler_tail_updates_a_captured_enclosing_cell(
    tmp_path, class_mode
):
    source = """
def make_class():
    __static_attributes__ = "from enclosing scope"

    class Subject:
        captured = __static_attributes__

        def method(self):
            self.inferred = 1

    return (
        Subject.captured,
        hasattr(Subject, "__static_attributes__"),
        __static_attributes__,
    )
"""

    validation = """
def validate_module(module):
    assert module.make_class() == ("from enclosing scope", False, ("inferred",))
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_closure_capture',
        source,
        validation,
        required_functions=('make_class',),
        retained_missing_cell_class='make_class.<locals>.Subject',
    )


def test_static_attributes_compiler_tail_updates_cell_captured_only_by_nested_method(
    tmp_path, class_mode
):
    source = """
def make_class():
    __static_attributes__ = "from enclosing scope"

    class Subject:
        def captured(self):
            return __static_attributes__

        def method(self):
            self.inferred = 1

    return (
        hasattr(Subject, "__static_attributes__"),
        __static_attributes__,
        Subject().captured(),
    )
"""

    validation = """
def validate_module(module):
    assert module.make_class() == (False, ("inferred",), ("inferred",))
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_nested_method_capture',
        source,
        validation,
        required_functions=('make_class',),
        retained_missing_cell_class='make_class.<locals>.Subject',
    )


def test_static_attributes_compiler_tail_updates_cell_captured_only_by_lambda(
    tmp_path, class_mode
):
    source = """
def make_class():
    __static_attributes__ = "from enclosing scope"

    class Subject:
        captured = staticmethod(lambda: __static_attributes__)

        def method(self):
            self.inferred = 1

    return (
        hasattr(Subject, "__static_attributes__"),
        __static_attributes__,
        Subject.captured(),
    )
"""

    validation = """
def validate_module(module):
    assert module.make_class() == (False, ("inferred",), ("inferred",))
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_lambda_capture',
        source,
        validation,
        required_functions=('make_class',),
        retained_missing_cell_class='make_class.<locals>.Subject',
    )


def test_static_attributes_compiler_tail_honors_global_nonlocal_and_local_bindings(
    tmp_path, class_mode
):
    source = """
def unread_outer():
    __static_attributes__ = "outer untouched"

    class Subject:
        def method(self):
            self.unread_field = 1

    return Subject.__static_attributes__, __static_attributes__


__static_attributes__ = "global before"


class ExplicitGlobal:
    global __static_attributes__
    captured = __static_attributes__

    def method(self):
        self.global_field = 2


GLOBAL_RESULT = (
    ExplicitGlobal.captured,
    hasattr(ExplicitGlobal, "__static_attributes__"),
    __static_attributes__,
)


def explicit_nonlocal():
    __static_attributes__ = "nonlocal before"

    class Subject:
        nonlocal __static_attributes__
        captured = __static_attributes__

        def method(self):
            self.nonlocal_field = 3

    return (
        Subject.captured,
        hasattr(Subject, "__static_attributes__"),
        __static_attributes__,
    )


def explicit_local():
    __static_attributes__ = "outer untouched"

    class Subject:
        __static_attributes__ = ("manual",)
        captured = __static_attributes__

        def method(self):
            self.local_field = 4

    return Subject.captured, Subject.__static_attributes__, __static_attributes__
"""

    validation = """
def validate_module(module):
    assert module.unread_outer() == (("unread_field",), "outer untouched")
    assert module.GLOBAL_RESULT == ("global before", False, ("global_field",))
    assert module.explicit_nonlocal() == ("nonlocal before", False, ("nonlocal_field",))
    assert module.explicit_local() == (("manual",), ("local_field",), "outer untouched")
"""
    _run_class_case(
        tmp_path,
        class_mode,
        'static_attribute_binding_scopes',
        source,
        validation,
        required_functions=('unread_outer', 'ExplicitGlobal.method', 'explicit_nonlocal', 'explicit_local'),
        retained_missing_cell_class='explicit_nonlocal.<locals>.Subject',
    )


@pytest.mark.parametrize("entry_interpreter", [False, True], ids=["soac", "entry"])
def test_retained_unrepresented_class_cell_refuses_before_module_effects(
    tmp_path, entry_interpreter
):
    source = """
from __future__ import strict
import body_effects

body_effects.events.append("module body entered")

def make_class():
    __static_attributes__ = "outer"
    class Subject:
        captured = __static_attributes__
        def method(self):
            self.inferred = 1
    return Subject
"""
    project = create_strict_project(
        tmp_path,
        {
            "unrepresented_class_cell.py": source,
            "body_effects.py": "events: list[str] = []\n",
        },
        modules={"unrepresented_class_cell": "unrepresented_class_cell.py"},
        backend="soac",
    )
    program = (
        "import body_effects\nassert body_effects.events == []\n"
        + _retained_cell_refusal_program(
            "unrepresented_class_cell", "make_class.<locals>.Subject"
        )
        + "\nassert body_effects.events == []\n"
    )
    project.run(program, backend="soac", entry_interpreter=entry_interpreter)
