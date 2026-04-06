# class_with_method


class C:
    x: int = 1

    def m(self):
        return self.x


# ==

# function C.m(self):
#     function_id: 0:1
#     block bb1:
#         return GetAttr(self, "x")

# function C.__annotate_func__(_dp_format, __soac__):
#     function_id: 0:2
#     block bb1:
#         if_term eq(_dp_format, 4):
#             then:
#                 block bb5:
#                     return dict(tuple_values(tuple_values("x", "int")))
#             else:
#                 block bb2:
#                     if_term gt(_dp_format, 2):
#                         then:
#                             block bb4:
#                                 raise GetAttr(builtins, "NotImplementedError")
#                         else:
#                             block bb3:
#                                 return dict(tuple_values(tuple_values("x", int)))

# function _dp_class_ns_C(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:3
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_1", __name__)
#         StoreName("_dp_assign_obj_2", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_3", "__module__")
#         SetItem(_dp_assign_obj_2, _dp_assign_index_3, _dp_assign_value_1)
#         StoreName("_dp_assign_value_4", "C")
#         StoreName("_dp_assign_obj_5", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_6", "__qualname__")
#         SetItem(_dp_assign_obj_5, _dp_assign_index_6, _dp_assign_value_4)
#         StoreName("x", 1)
#         StoreName("m", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("__annotate_func__", MakeFunction(0:2, Function, tuple_values(__import__("soac.runtime", globals(), dict(), tuple_values("runtime"), 0)), NONE))
#         return NONE

# function _dp_define_class_C(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:4
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("C", _dp_class_ns_fn, tuple_values(), _dp_prepare_dict, FALSE, 3, tuple_values())

# function _dp_module_init():
#     function_id: 0:5
#     block bb1:
#         StoreName("_dp_class_ns_C", MakeFunction(0:3, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_C", MakeFunction(0:4, Function, tuple_values(NONE), NONE))
#         StoreName("C", _dp_define_class_C(_dp_class_ns_C, globals()))
#         return NONE

# class_method_named_open_calls_builtin


class Wrapper:
    def open(self, mode: str = "r", *, encoding: str = "utf8"):
        return open(mode, encoding=encoding)


# ==

# function Wrapper.open(self, mode, *, encoding):
#     function_id: 0:1
#     block bb1:
#         return open(mode, encoding=encoding)

# function _dp_class_ns_Wrapper(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_1", __name__)
#         StoreName("_dp_assign_obj_2", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_3", "__module__")
#         SetItem(_dp_assign_obj_2, _dp_assign_index_3, _dp_assign_value_1)
#         StoreName("_dp_assign_value_4", "Wrapper")
#         StoreName("_dp_assign_obj_5", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_6", "__qualname__")
#         SetItem(_dp_assign_obj_5, _dp_assign_index_6, _dp_assign_value_4)
#         StoreName("open", MakeFunction(0:1, Function, tuple_values("r", "utf8"), NONE))
#         return NONE

# function _dp_define_class_Wrapper(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:3
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("Wrapper", _dp_class_ns_fn, tuple_values(), _dp_prepare_dict, FALSE, 3, tuple_values())

# function _dp_module_init():
#     function_id: 0:4
#     block bb1:
#         StoreName("_dp_class_ns_Wrapper", MakeFunction(0:2, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_Wrapper", MakeFunction(0:3, Function, tuple_values(NONE), NONE))
#         StoreName("Wrapper", _dp_define_class_Wrapper(_dp_class_ns_Wrapper, globals()))
#         return NONE

# class_with_base


class D(Base):
    pass


# ==

# function _dp_class_ns_D(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_1", __name__)
#         StoreName("_dp_assign_obj_2", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_3", "__module__")
#         SetItem(_dp_assign_obj_2, _dp_assign_index_3, _dp_assign_value_1)
#         StoreName("_dp_assign_value_4", "D")
#         StoreName("_dp_assign_obj_5", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_6", "__qualname__")
#         SetItem(_dp_assign_obj_5, _dp_assign_index_6, _dp_assign_value_4)
#         return NONE

# function _dp_define_class_D(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("D", _dp_class_ns_fn, tuple_values(Base), _dp_prepare_dict, FALSE, 3, tuple_values())

# function _dp_module_init():
#     function_id: 0:3
#     block bb1:
#         StoreName("_dp_class_ns_D", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_D", MakeFunction(0:2, Function, tuple_values(NONE), NONE))
#         StoreName("D", _dp_define_class_D(_dp_class_ns_D, globals()))
#         return NONE

# class_scope_inner_capture


def outer():
    x = "outer"

    class Inner:
        y = x

    return Inner.y


# ==

# function outer.<locals>._dp_class_ns_Inner(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_1", __name__)
#         StoreName("_dp_assign_obj_2", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_3", "__module__")
#         SetItem(_dp_assign_obj_2, _dp_assign_index_3, _dp_assign_value_1)
#         StoreName("_dp_assign_value_4", "outer.<locals>.Inner")
#         StoreName("_dp_assign_obj_5", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_6", "__qualname__")
#         SetItem(_dp_assign_obj_5, _dp_assign_index_6, _dp_assign_value_4)
#         StoreName("y", x)
#         return NONE

# function outer.<locals>._dp_define_class_Inner(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("Inner", _dp_class_ns_fn, tuple_values(), _dp_prepare_dict, FALSE, 6, tuple_values())

# function outer():
#     function_id: 0:3
#     block bb1:
#         StoreName("x", "outer")
#         StoreName("_dp_class_ns_Inner", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_Inner", MakeFunction(0:2, Function, tuple_values(NONE), NONE))
#         StoreName("Inner", _dp_define_class_Inner(_dp_class_ns_Inner, globals()))
#         return GetAttr(Inner, "y")

# function _dp_module_init():
#     function_id: 0:4
#     block bb1:
#         StoreName("outer", MakeFunction(0:3, Function, tuple_values(), NONE))
#         return NONE

# class_super_empty_classcell


class X:
    def f(x):
        nonlocal __class__
        del __class__
        super()


# ==

# function X.f(x):
#     function_id: 0:1
#     block bb1:
#         Del { name: "__class__", quietly: false }
#         call_super(super, CellRefForName("__class__"), x)
#         return NONE

# function _dp_class_ns_X(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_1", __name__)
#         StoreName("_dp_assign_obj_2", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_3", "__module__")
#         SetItem(_dp_assign_obj_2, _dp_assign_index_3, _dp_assign_value_1)
#         StoreName("_dp_assign_value_4", "X")
#         StoreName("_dp_assign_obj_5", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_6", "__qualname__")
#         SetItem(_dp_assign_obj_5, _dp_assign_index_6, _dp_assign_value_4)
#         StoreName("f", MakeFunction(0:1, Function, tuple_values(), NONE))
#         return NONE

# function _dp_define_class_X(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:3
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("X", _dp_class_ns_fn, tuple_values(), _dp_prepare_dict, TRUE, 3, tuple_values())

# function _dp_module_init():
#     function_id: 0:4
#     block bb1:
#         StoreName("_dp_class_ns_X", MakeFunction(0:2, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_X", MakeFunction(0:3, Function, tuple_values(NONE), NONE))
#         StoreName("X", _dp_define_class_X(_dp_class_ns_X, globals()))
#         return NONE

# nested classes


class A:
    class B:
        pass


# ==

# function A._dp_class_ns_B(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_1", __name__)
#         StoreName("_dp_assign_obj_2", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_3", "__module__")
#         SetItem(_dp_assign_obj_2, _dp_assign_index_3, _dp_assign_value_1)
#         StoreName("_dp_assign_value_4", "A.B")
#         StoreName("_dp_assign_obj_5", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_6", "__qualname__")
#         SetItem(_dp_assign_obj_5, _dp_assign_index_6, _dp_assign_value_4)
#         return NONE

# function A._dp_define_class_B(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("B", _dp_class_ns_fn, tuple_values(), _dp_prepare_dict, FALSE, 4, tuple_values())

# function _dp_class_ns_A(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:3
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_7", __name__)
#         StoreName("_dp_assign_obj_8", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_9", "__module__")
#         SetItem(_dp_assign_obj_8, _dp_assign_index_9, _dp_assign_value_7)
#         StoreName("_dp_assign_value_10", "A")
#         StoreName("_dp_assign_obj_11", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_12", "__qualname__")
#         SetItem(_dp_assign_obj_11, _dp_assign_index_12, _dp_assign_value_10)
#         StoreName("_dp_class_ns_B", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_B", MakeFunction(0:2, Function, tuple_values(NONE), NONE))
#         StoreName("B", _dp_define_class_B(_dp_class_ns_B, _dp_class_ns))
#         return NONE

# function _dp_define_class_A(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:4
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("A", _dp_class_ns_fn, tuple_values(), _dp_prepare_dict, FALSE, 3, tuple_values())

# function _dp_module_init():
#     function_id: 0:5
#     block bb1:
#         StoreName("_dp_class_ns_A", MakeFunction(0:3, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_A", MakeFunction(0:4, Function, tuple_values(NONE), NONE))
#         StoreName("A", _dp_define_class_A(_dp_class_ns_A, globals()))
#         return NONE

# nested classes with weird scoping


def foo():
    class A:
        global B

        class B:
            pass


# ==

# function foo.<locals>.A._dp_class_ns_B(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_1", __name__)
#         StoreName("_dp_assign_obj_2", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_3", "__module__")
#         SetItem(_dp_assign_obj_2, _dp_assign_index_3, _dp_assign_value_1)
#         StoreName("_dp_assign_value_4", "B")
#         StoreName("_dp_assign_obj_5", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_6", "__qualname__")
#         SetItem(_dp_assign_obj_5, _dp_assign_index_6, _dp_assign_value_4)
#         return NONE

# function foo.<locals>.A._dp_define_class_B(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("B", _dp_class_ns_fn, tuple_values(), _dp_prepare_dict, FALSE, 7, tuple_values())

# function foo.<locals>._dp_class_ns_A(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:3
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_7", __name__)
#         StoreName("_dp_assign_obj_8", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_9", "__module__")
#         SetItem(_dp_assign_obj_8, _dp_assign_index_9, _dp_assign_value_7)
#         StoreName("_dp_assign_value_10", "foo.<locals>.A")
#         StoreName("_dp_assign_obj_11", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_12", "__qualname__")
#         SetItem(_dp_assign_obj_11, _dp_assign_index_12, _dp_assign_value_10)
#         StoreName("_dp_class_ns_B", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_B", MakeFunction(0:2, Function, tuple_values(NONE), NONE))
#         StoreName("B", _dp_define_class_B(_dp_class_ns_B, _dp_class_ns))
#         return NONE

# function foo.<locals>._dp_define_class_A(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:4
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("A", _dp_class_ns_fn, tuple_values(), _dp_prepare_dict, FALSE, 4, tuple_values())

# function foo():
#     function_id: 0:5
#     block bb1:
#         StoreName("_dp_class_ns_A", MakeFunction(0:3, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_A", MakeFunction(0:4, Function, tuple_values(NONE), NONE))
#         StoreName("A", _dp_define_class_A(_dp_class_ns_A, globals()))
#         return NONE

# function _dp_module_init():
#     function_id: 0:6
#     block bb1:
#         StoreName("foo", MakeFunction(0:5, Function, tuple_values(), NONE))
#         return NONE
