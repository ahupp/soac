from __future__ import annotations

import pytest

from tests._integration import soac_module, stock_module


@pytest.fixture(
    params=[
        pytest.param(stock_module, id="stock"),
        pytest.param(soac_module, id="soac"),
    ]
)
def class_module_loader(request):
    return request.param


def test_static_attributes_include_only_compiler_recorded_self_stores(
    tmp_path, class_module_loader
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

    with class_module_loader(tmp_path, "static_attribute_store_shapes", source) as module:
        assert module.Subject.__static_attributes__ == (
            "__private",
            "alpha",
            "annotated_with_value",
            "comprehension_target",
            "loop_target",
            "textual_name",
            "zeta",
        )


def test_static_attributes_follow_nearest_lexical_class_through_nested_scopes(
    tmp_path, class_module_loader
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

    with class_module_loader(tmp_path, "static_attribute_nested_scopes", source) as module:
        assert module.Outer.__static_attributes__ == (
            "from_lambda",
            "from_nested_function",
            "outer",
        )
        assert module.Outer.Inner.__static_attributes__ == ("inner", "inner_nested")


def test_static_attributes_overwrite_explicit_class_body_assignments(
    tmp_path, class_module_loader
):
    source = """
class Explicit:
    __static_attributes__ = ("manually_chosen",)

    def method(self):
        self.inferred = 1


class Empty:
    __static_attributes__ = ("manually_chosen",)
"""

    with class_module_loader(tmp_path, "static_attribute_explicit_overwrite", source) as module:
        assert module.Explicit.__static_attributes__ == ("inferred",)
        assert module.Empty.__static_attributes__ == ()


def test_static_attributes_are_written_after_body_and_before_metaclass_creation(
    tmp_path, class_module_loader
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

    with class_module_loader(tmp_path, "static_attribute_namespace_events", source) as module:
        assert module.Observed.__static_attributes__ == ("inferred",)
        assert module.EVENTS == [
            ("static-write", ("manual",)),
            ("body-write", "class body"),
            ("annotation-write", True),
            ("static-write", ("inferred",)),
            ("create", ("inferred",)),
        ]


def test_static_attributes_are_not_inherited_from_base_constructors(
    tmp_path, class_module_loader
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

    with class_module_loader(tmp_path, "static_attribute_inherited_owners", source) as module:
        assert module.Base.__static_attributes__ == ("from_base",)
        assert module.InheritedOnly.__static_attributes__ == ()
        assert module.OwnFields.__static_attributes__ == ("from_child",)


def test_static_attributes_skip_current_class_body_and_attribute_nested_body_to_parent(
    tmp_path, class_module_loader
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

    with class_module_loader(tmp_path, "static_attribute_direct_class_body", source) as module:
        assert module.Outer.__static_attributes__ == ("inner_body", "outer_method")
        assert module.Outer.Inner.__static_attributes__ == ("inner_method",)


def test_static_attributes_compiler_tail_updates_a_captured_enclosing_cell(
    tmp_path, class_module_loader
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

    with class_module_loader(tmp_path, "static_attribute_closure_capture", source) as module:
        assert module.make_class() == ("from enclosing scope", False, ("inferred",))


def test_static_attributes_compiler_tail_updates_cell_captured_only_by_nested_method(
    tmp_path, class_module_loader
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

    with class_module_loader(tmp_path, "static_attribute_nested_method_capture", source) as module:
        assert module.make_class() == (False, ("inferred",), ("inferred",))


def test_static_attributes_compiler_tail_updates_cell_captured_only_by_lambda(
    tmp_path, class_module_loader
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

    with class_module_loader(tmp_path, "static_attribute_lambda_capture", source) as module:
        assert module.make_class() == (False, ("inferred",), ("inferred",))


def test_static_attributes_compiler_tail_honors_global_nonlocal_and_local_bindings(
    tmp_path, class_module_loader
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

    with class_module_loader(tmp_path, "static_attribute_binding_scopes", source) as module:
        assert module.unread_outer() == (("unread_field",), "outer untouched")
        assert module.GLOBAL_RESULT == ("global before", False, ("global_field",))
        assert module.explicit_nonlocal() == ("nonlocal before", False, ("nonlocal_field",))
        assert module.explicit_local() == (("manual",), ("local_field",), "outer untouched")
