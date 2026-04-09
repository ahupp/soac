# import_simple

import a

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("a", import_("a", __spec__))
#         return NONE

# import_dotted_alias

import a.b as c

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("c", import_attr(import_("a.b", __spec__), "b"))
#         return NONE

# import_from_alias

from pkg.mod import name as alias

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_import_1", import_("pkg.mod", __spec__, list(tuple_values("name"))))
#         StoreName("alias", import_attr(_dp_import_1, "name"))
#         return NONE

# decorator_function


@dec
def f():
    pass


# ==

# function f():
#     function_id: 0:1
#     block bb1:
#         return NONE

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("f", dec(MakeFunction(0:1, Function, tuple_values(), NONE)))
#         return NONE

# assign_attr

obj.x = 1

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_assign_value_1", 1)
#         StoreName("_dp_assign_obj_2", load_deleted_name("obj", obj))
#         SetAttr(_dp_assign_obj_2, "x", _dp_assign_value_1)
#         return NONE

# assign_subscript

obj[i] = v

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_assign_value_1", v)
#         StoreName("_dp_assign_obj_2", load_deleted_name("obj", obj))
#         StoreName("_dp_assign_index_3", i)
#         SetItem(_dp_assign_obj_2, _dp_assign_index_3, _dp_assign_value_1)
#         return NONE

# assign_tuple_unpack

a, b = it

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_assign_value_1", it)
#         StoreName("_dp_unpack_2", unpack(_dp_assign_value_1, tuple_values(TRUE, TRUE)))
#         StoreName("a", GetItem(_dp_unpack_2, 0))
#         StoreName("b", GetItem(_dp_unpack_2, 1))
#         Del { name: "_dp_unpack_2", quietly: false }
#         return NONE

# assign_star_unpack

a, *b = it

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_assign_value_1", it)
#         StoreName("_dp_unpack_2", unpack(_dp_assign_value_1, tuple_values(TRUE, FALSE)))
#         StoreName("a", GetItem(_dp_unpack_2, 0))
#         StoreName("b", list(GetItem(_dp_unpack_2, 1)))
#         Del { name: "_dp_unpack_2", quietly: false }
#         return NONE

# assign_multi_targets

a = b = f()

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_assign_value_1", f())
#         StoreName("a", _dp_assign_value_1)
#         StoreName("b", _dp_assign_value_1)
#         return NONE

# ann_assign_simple

x: int = 1

# ==

# function __annotate__.<locals>.<lambda>():
#     function_id: 0:1
#     display_name: <lambda>
#     block bb1:
#         return int

# function __annotate__(_dp_format, __soac__):
#     function_id: 0:2
#     block bb8:
#         if_term eq(_dp_format, 4):
#             then:
#                 block bb9:
#                     return dict(tuple_values(tuple_values("x", "int")))
#             else:
#                 block bb10:
#                     jump bb5
#                     block bb5:
#                         if_term eq(_dp_format, 3):
#                             then:
#                                 block bb6:
#                                     return dict(tuple_values(tuple_values("x", annotation_forwardref_value(MakeFunction(0:1, Function, tuple_values(), NONE), "int", __name__))))
#                             else:
#                                 block bb7:
#                                     jump bb2
#                                     block bb2:
#                                         if_term gt(_dp_format, 2):
#                                             then:
#                                                 block bb3:
#                                                     raise GetAttr(builtins, "NotImplementedError")
#                                             else:
#                                                 block bb4:
#                                                     return dict(tuple_values(tuple_values("x", int)))

# function _dp_module_init():
#     function_id: 0:3
#     block bb1:
#         StoreName("x", 1)
#         StoreName("__annotate__", MakeFunction(0:2, Function, tuple_values(__import__("soac.runtime", globals(), dict(), tuple_values("runtime"), 0)), NONE))
#         return NONE

# ann_assign_attr

obj.x: int = 1

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_assign_value_1", 1)
#         StoreName("_dp_assign_obj_2", load_deleted_name("obj", obj))
#         SetAttr(_dp_assign_obj_2, "x", _dp_assign_value_1)
#         return NONE

# aug_assign_attr

obj.x += 1

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_augassign_obj_1", load_deleted_name("obj", obj))
#         StoreName("_dp_augassign_value_2", GetAttr(_dp_augassign_obj_1, "x"))
#         SetAttr(_dp_augassign_obj_1, "x", BinOp(InplaceAdd, _dp_augassign_value_2, 1))
#         return NONE

# delete_mixed

del obj.x, obj[i], x

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_delete_obj_1", load_deleted_name("obj", obj))
#         delattr(_dp_delete_obj_1, "x")
#         StoreName("_dp_delete_obj_2", load_deleted_name("obj", obj))
#         StoreName("_dp_delete_index_3", i)
#         DelItem(_dp_delete_obj_2, _dp_delete_index_3)
#         Del { name: "x", quietly: false }
#         return NONE

# assert_no_msg

assert cond

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         if_term __debug__:
#             then:
#                 block bb2:
#                     jump bb3
#                     block bb3:
#                         if_term UnaryOp(Not, cond):
#                             then:
#                                 block bb4:
#                                     raise AssertionError
#                             else:
#                                 block bb5:
#                                     jump bb6
#                                     block bb6:
#                                         return NONE
#             else:
#                 block bb7:
#                     return NONE

# assert_with_msg

assert cond, "oops"

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         if_term __debug__:
#             then:
#                 block bb2:
#                     jump bb3
#                     block bb3:
#                         if_term UnaryOp(Not, cond):
#                             then:
#                                 block bb4:
#                                     raise AssertionError("oops")
#                             else:
#                                 block bb5:
#                                     jump bb6
#                                     block bb6:
#                                         return NONE
#             else:
#                 block bb7:
#                     return NONE

# raise_from

raise E from cause

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         raise raise_from(E, cause)

# try_except_typed

try:
    f()
except E as e:
    g(e)
except:
    h()

# ==

# snapshot regeneration failed
# panic: py_stmt template must produce exactly one statement, got 2

# for_else

for x in it:
    body()
else:
    done()

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb3:
#         StoreName("_dp_iter_0_1_0", iter(it))
#         jump bb1
#         block bb1:
#             StoreName("_dp_tmp_0_1_1", next_or_sentinel(_dp_iter_0_1_0))
#             if_term BinOp(Is, _dp_tmp_0_1_1, ITER_COMPLETE):
#                 then:
#                     block bb4:
#                         done()
#                         return NONE
#                 else:
#                     block bb2:
#                         StoreName("_dp_tmp_0_1_1", _dp_tmp_0_1_1)
#                         StoreName("x", _dp_tmp_0_1_1)
#                         Del { name: "_dp_tmp_0_1_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             body()
#                             jump bb1

# while_else

while cond:
    body()
else:
    done()

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         jump bb4
#         block bb4:
#             if_term cond:
#                 then:
#                     block bb3:
#                         body()
#                         jump bb1
#                 else:
#                     block bb2:
#                         done()
#                         return NONE

# with_as

with cm as x:
    body()

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb4:
#         StoreName("_dp_with_exit_1", contextmanager_get_exit(cm))
#         StoreName("x", contextmanager_enter(cm))
#         StoreName("_dp_with_ok_2", TRUE)
#         jump bb17
#         block bb17:
#             body()
#             jump bb9
#             block bb9:
#                 jump bb6(AbruptKind(Fallthrough), None)
#                 block bb6(_dp_try_exc_0_1_0: Exception, _dp_try_abrupt_kind_0_1_1: AbruptKind, _dp_try_abrupt_payload_0_1_2: AbruptPayload):
#                     exc_param: _dp_try_exc_0_1_0
#                     if_term _dp_with_ok_2:
#                         then:
#                             block bb7(_dp_try_exc_0_1_0: Exception):
#                                 exc_param: _dp_try_exc_0_1_0
#                                 contextmanager_exit(_dp_with_exit_1, NONE)
#                                 jump bb5
#                         else:
#                             block bb8(_dp_try_exc_0_1_0: Exception):
#                                 exc_param: _dp_try_exc_0_1_0
#                                 jump bb5
#                     block bb5(_dp_try_exc_0_1_0: Exception):
#                         exc_param: _dp_try_exc_0_1_0
#                         StoreName("_dp_with_exit_1", NONE)
#                         jump bb1
#                         block bb1:
#                             branch_table _dp_try_abrupt_kind_0_1_1 -> [bb0, bb2, bb3] default bb0
#                             block bb0:
#                                 return NONE
#                             block bb2:
#                                 return _dp_try_abrupt_payload_0_1_2
#                             block bb3:
#                                 raise _dp_try_abrupt_payload_0_1_2
#     block bb10(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         jump bb6(AbruptKind(Exception), Name("_dp_try_exc_0_1_0"))
#     block bb14(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         if_term exception_matches(_dp_try_exc_0_1_0, BaseException):
#             then:
#                 jump bb15
#             else:
#                 jump bb16
#     block bb15(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         StoreName("_dp_with_ok_2", FALSE)
#         contextmanager_exit(_dp_with_exit_1, _dp_try_exc_0_1_0)
#         jump bb9
#     block bb16(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         raise _dp_try_exc_0_1_0

# function_local_ann_assign


def inner():
    value: int = 1
    return value


# ==

# function inner():
#     function_id: 0:1
#     block bb1:
#         StoreName("value", 1)
#         return value

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("inner", MakeFunction(0:1, Function, tuple_values(), NONE))
#         return NONE

# comprehension_global

xs = [x for x in it]
ys = {x for x in it}
zs = {k: v for k, v in items}

# ==

# function _dp_listcomp_3(_dp_iter_2):
#     function_id: 0:1
#     display_name: <listcomp>
#     block bb3:
#         StoreName("_dp_tmp_1", list(tuple_values()))
#         StoreName("_dp_iter_0_1_0", iter(_dp_iter_2))
#         jump bb1
#         block bb1:
#             StoreName("_dp_tmp_0_1_1", next_or_sentinel(_dp_iter_0_1_0))
#             if_term BinOp(Is, _dp_tmp_0_1_1, ITER_COMPLETE):
#                 then:
#                     block bb4:
#                         return _dp_tmp_1
#                 else:
#                     block bb2:
#                         StoreName("_dp_tmp_0_1_1", _dp_tmp_0_1_1)
#                         StoreName("x", _dp_tmp_0_1_1)
#                         Del { name: "_dp_tmp_0_1_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             GetAttr(_dp_tmp_1, "append")(x)
#                             jump bb1

# function _dp_setcomp_6(_dp_iter_5):
#     function_id: 0:2
#     display_name: <setcomp>
#     block bb3:
#         StoreName("_dp_tmp_4", set())
#         StoreName("_dp_iter_0_2_0", iter(_dp_iter_5))
#         jump bb1
#         block bb1:
#             StoreName("_dp_tmp_0_2_1", next_or_sentinel(_dp_iter_0_2_0))
#             if_term BinOp(Is, _dp_tmp_0_2_1, ITER_COMPLETE):
#                 then:
#                     block bb4:
#                         return _dp_tmp_4
#                 else:
#                     block bb2:
#                         StoreName("_dp_tmp_0_2_1", _dp_tmp_0_2_1)
#                         StoreName("x", _dp_tmp_0_2_1)
#                         Del { name: "_dp_tmp_0_2_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             GetAttr(_dp_tmp_4, "add")(x)
#                             jump bb1

# function _dp_dictcomp_11(_dp_iter_10):
#     function_id: 0:3
#     display_name: <dictcomp>
#     block bb3:
#         StoreName("_dp_tmp_7", dict())
#         StoreName("_dp_iter_0_3_0", iter(_dp_iter_10))
#         jump bb1
#         block bb1:
#             StoreName("_dp_tmp_0_3_1", next_or_sentinel(_dp_iter_0_3_0))
#             if_term BinOp(Is, _dp_tmp_0_3_1, ITER_COMPLETE):
#                 then:
#                     block bb4:
#                         return _dp_tmp_7
#                 else:
#                     block bb2:
#                         StoreName("_dp_tmp_0_3_1", _dp_tmp_0_3_1)
#                         StoreName("_dp_assign_value_15", _dp_tmp_0_3_1)
#                         StoreName("_dp_unpack_16", unpack(_dp_assign_value_15, tuple_values(TRUE, TRUE)))
#                         StoreName("k", GetItem(_dp_unpack_16, 0))
#                         StoreName("v", GetItem(_dp_unpack_16, 1))
#                         Del { name: "_dp_unpack_16", quietly: false }
#                         Del { name: "_dp_tmp_0_3_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             StoreName("_dp_dictcomp_key_8", k)
#                             StoreName("_dp_dictcomp_value_9", v)
#                             StoreName("_dp_assign_value_12", _dp_dictcomp_value_9)
#                             StoreName("_dp_assign_obj_13", load_deleted_name("_dp_tmp_7", _dp_tmp_7))
#                             StoreName("_dp_assign_index_14", _dp_dictcomp_key_8)
#                             SetItem(_dp_assign_obj_13, _dp_assign_index_14, _dp_assign_value_12)
#                             jump bb1

# function _dp_module_init():
#     function_id: 0:4
#     block bb1:
#         StoreName("_dp_listcomp_3", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("xs", _dp_listcomp_3(it))
#         StoreName("_dp_setcomp_6", MakeFunction(0:2, Function, tuple_values(), NONE))
#         StoreName("ys", _dp_setcomp_6(it))
#         StoreName("_dp_dictcomp_11", MakeFunction(0:3, Function, tuple_values(), NONE))
#         StoreName("zs", _dp_dictcomp_11(items))
#         return NONE

# comprehension_in_function


def f():
    return [x for x in it if x > 0]


# ==

# function f.<locals>._dp_listcomp_3(_dp_iter_2):
#     function_id: 0:1
#     display_name: <listcomp>
#     block bb3:
#         StoreName("_dp_tmp_1", list(tuple_values()))
#         StoreName("_dp_iter_0_1_0", iter(_dp_iter_2))
#         jump bb1
#         block bb1:
#             StoreName("_dp_tmp_0_1_1", next_or_sentinel(_dp_iter_0_1_0))
#             if_term BinOp(Is, _dp_tmp_0_1_1, ITER_COMPLETE):
#                 then:
#                     block bb4:
#                         return _dp_tmp_1
#                 else:
#                     block bb2:
#                         StoreName("_dp_tmp_0_1_1", _dp_tmp_0_1_1)
#                         StoreName("x", _dp_tmp_0_1_1)
#                         Del { name: "_dp_tmp_0_1_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             if_term BinOp(Gt, x, 0):
#                                 then:
#                                     block bb6:
#                                         GetAttr(_dp_tmp_1, "append")(x)
#                                         jump bb1
#                                 else:
#                                     block bb7:
#                                         jump bb1

# function f():
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_listcomp_3", MakeFunction(0:1, Function, tuple_values(), NONE))
#         return _dp_listcomp_3(it)

# function _dp_module_init():
#     function_id: 0:3
#     block bb1:
#         StoreName("f", MakeFunction(0:2, Function, tuple_values(), NONE))
#         return NONE

# comprehension_in_class_body


class C:
    xs = [x for x in it]


# ==

# function C._dp_listcomp_3(_dp_iter_2):
#     function_id: 0:1
#     display_name: <listcomp>
#     block bb3:
#         StoreName("_dp_tmp_1", list(tuple_values()))
#         StoreName("_dp_iter_0_1_0", iter(_dp_iter_2))
#         jump bb1
#         block bb1:
#             StoreName("_dp_tmp_0_1_1", next_or_sentinel(_dp_iter_0_1_0))
#             if_term BinOp(Is, _dp_tmp_0_1_1, ITER_COMPLETE):
#                 then:
#                     block bb4:
#                         return _dp_tmp_1
#                 else:
#                     block bb2:
#                         StoreName("_dp_tmp_0_1_1", _dp_tmp_0_1_1)
#                         StoreName("x", _dp_tmp_0_1_1)
#                         Del { name: "_dp_tmp_0_1_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             GetAttr(_dp_tmp_1, "append")(x)
#                             jump bb1

# function _dp_class_ns_C(_dp_class_ns, _dp_classcell_arg):
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_classcell", _dp_classcell_arg)
#         StoreName("_dp_assign_value_4", __name__)
#         StoreName("_dp_assign_obj_5", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_6", "__module__")
#         SetItem(_dp_assign_obj_5, _dp_assign_index_6, _dp_assign_value_4)
#         StoreName("_dp_assign_value_7", "C")
#         StoreName("_dp_assign_obj_8", load_deleted_name("_dp_class_ns", _dp_class_ns))
#         StoreName("_dp_assign_index_9", "__qualname__")
#         SetItem(_dp_assign_obj_8, _dp_assign_index_9, _dp_assign_value_7)
#         StoreName("_dp_listcomp_3", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("xs", _dp_listcomp_3(it))
#         return NONE

# function _dp_define_class_C(_dp_class_ns_fn, _dp_class_ns_outer, _dp_prepare_dict):
#     function_id: 0:3
#     block bb1:
#         StoreName("_dp_class_ns", _dp_class_ns_outer)
#         return create_class("C", _dp_class_ns_fn, tuple_values(), _dp_prepare_dict, FALSE, 3, tuple_values())

# function _dp_module_init():
#     function_id: 0:4
#     block bb1:
#         StoreName("_dp_class_ns_C", MakeFunction(0:2, Function, tuple_values(), NONE))
#         StoreName("_dp_define_class_C", MakeFunction(0:3, Function, tuple_values(NONE), NONE))
#         StoreName("C", _dp_define_class_C(_dp_class_ns_C, globals()))
#         return NONE

# with_multi

with a as x, b as y:
    body()

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb4:
#         StoreName("_dp_with_exit_4", contextmanager_get_exit(a))
#         StoreName("x", contextmanager_enter(a))
#         StoreName("_dp_with_ok_5", TRUE)
#         jump bb20
#         block bb20:
#             StoreName("_dp_with_exit_1", contextmanager_get_exit(b))
#             StoreName("y", contextmanager_enter(b))
#             StoreName("_dp_with_ok_2", TRUE)
#             jump bb33
#             block bb33:
#                 body()
#                 jump bb25
#                 block bb25:
#                     jump bb22(AbruptKind(Fallthrough), None)
#                     block bb22(_dp_try_exc_0_1_3: Exception, _dp_try_abrupt_kind_0_1_4: AbruptKind, _dp_try_abrupt_payload_0_1_5: AbruptPayload):
#                         exc_param: _dp_try_exc_0_1_3
#                         if_term _dp_with_ok_2:
#                             then:
#                                 block bb23(_dp_try_exc_0_1_3: Exception):
#                                     exc_param: _dp_try_exc_0_1_3
#                                     contextmanager_exit(_dp_with_exit_1, NONE)
#                                     jump bb21
#                             else:
#                                 block bb24(_dp_try_exc_0_1_3: Exception):
#                                     exc_param: _dp_try_exc_0_1_3
#                                     jump bb21
#                         block bb21(_dp_try_exc_0_1_3: Exception):
#                             exc_param: _dp_try_exc_0_1_3
#                             StoreName("_dp_with_exit_1", NONE)
#                             jump bb17
#                             block bb17:
#                                 branch_table _dp_try_abrupt_kind_0_1_4 -> [bb9, bb18, bb19] default bb9
#                                 block bb6(_dp_try_exc_0_1_0: Exception, _dp_try_abrupt_kind_0_1_1: AbruptKind, _dp_try_abrupt_payload_0_1_2: AbruptPayload):
#                                     exc_param: _dp_try_exc_0_1_0
#                                     if_term _dp_with_ok_5:
#                                         then:
#                                             block bb7(_dp_try_exc_0_1_0: Exception):
#                                                 exc_param: _dp_try_exc_0_1_0
#                                                 contextmanager_exit(_dp_with_exit_4, NONE)
#                                                 jump bb5
#                                         else:
#                                             block bb8(_dp_try_exc_0_1_0: Exception):
#                                                 exc_param: _dp_try_exc_0_1_0
#                                                 jump bb5
#                                     block bb5(_dp_try_exc_0_1_0: Exception):
#                                         exc_param: _dp_try_exc_0_1_0
#                                         StoreName("_dp_with_exit_4", NONE)
#                                         jump bb1
#                                         block bb1:
#                                             branch_table _dp_try_abrupt_kind_0_1_1 -> [bb0, bb2, bb3] default bb0
#                                             block bb0:
#                                                 return NONE
#                                             block bb2:
#                                                 return _dp_try_abrupt_payload_0_1_2
#                                             block bb3:
#                                                 raise _dp_try_abrupt_payload_0_1_2
#                                 block bb9:
#                                     jump bb6(AbruptKind(Fallthrough), None)
#                                 block bb18:
#                                     StoreName("_dp_try_abrupt_payload_0_1_2", _dp_try_abrupt_payload_0_1_5)
#                                     jump bb6(AbruptKind(Return), Name("_dp_try_abrupt_payload_0_1_2"))
#                                 block bb19:
#                                     raise _dp_try_abrupt_payload_0_1_5
#     block bb10(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         jump bb6(AbruptKind(Exception), Name("_dp_try_exc_0_1_0"))
#     block bb14(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         if_term exception_matches(_dp_try_exc_0_1_0, BaseException):
#             then:
#                 jump bb15
#             else:
#                 jump bb16
#     block bb15(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         StoreName("_dp_with_ok_5", FALSE)
#         contextmanager_exit(_dp_with_exit_4, _dp_try_exc_0_1_0)
#         jump bb9
#     block bb16(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         raise _dp_try_exc_0_1_0
#     block bb26(_dp_try_exc_0_1_3: Exception):
#         exc_param: _dp_try_exc_0_1_3
#         jump bb22(AbruptKind(Exception), Name("_dp_try_exc_0_1_3"))
#     block bb30(_dp_try_exc_0_1_3: Exception):
#         exc_param: _dp_try_exc_0_1_3
#         if_term exception_matches(_dp_try_exc_0_1_3, BaseException):
#             then:
#                 jump bb31
#             else:
#                 jump bb32
#     block bb31(_dp_try_exc_0_1_3: Exception):
#         exc_param: _dp_try_exc_0_1_3
#         StoreName("_dp_with_ok_2", FALSE)
#         contextmanager_exit(_dp_with_exit_1, _dp_try_exc_0_1_3)
#         jump bb25
#     block bb32(_dp_try_exc_0_1_3: Exception):
#         exc_param: _dp_try_exc_0_1_3
#         raise _dp_try_exc_0_1_3

# async_for


async def run():
    async for x in ait:
        body()


# ==

# coroutine run():
#     function_id: 0:1
#     block bb3:
#         StoreName("_dp_iter_0_1_0", aiter(ait))
#         jump bb1
#         block bb1:
#             StoreName("_dp_tmp_0_1_1", await anext_or_sentinel(_dp_iter_0_1_0))
#             if_term BinOp(Is, _dp_tmp_0_1_1, ITER_COMPLETE):
#                 then:
#                     block bb0:
#                         return NONE
#                 else:
#                     block bb2:
#                         StoreName("_dp_tmp_0_1_1", _dp_tmp_0_1_1)
#                         StoreName("x", _dp_tmp_0_1_1)
#                         Del { name: "_dp_tmp_0_1_1", quietly: false }
#                         jump bb4
#                         block bb4:
#                             body()
#                             jump bb1

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("run", MakeFunction(0:1, Coroutine, tuple_values(), NONE))
#         return NONE

# async_with


async def run():
    async with cm as x:
        body()


# ==

# coroutine run():
#     function_id: 0:1
#     block bb4:
#         StoreName("_dp_with_exit_1", asynccontextmanager_get_aexit(cm))
#         StoreName("x", await asynccontextmanager_aenter(cm))
#         StoreName("_dp_with_ok_2", TRUE)
#         jump bb21
#         block bb21:
#             body()
#             jump bb9
#             block bb9:
#                 jump bb6(AbruptKind(Fallthrough), None)
#                 block bb6(_dp_try_exc_0_1_0: Exception, _dp_try_abrupt_kind_0_1_1: AbruptKind, _dp_try_abrupt_payload_0_1_2: AbruptPayload):
#                     exc_param: _dp_try_exc_0_1_0
#                     if_term _dp_with_ok_2:
#                         then:
#                             block bb7(_dp_try_exc_0_1_0: Exception):
#                                 exc_param: _dp_try_exc_0_1_0
#                                 await asynccontextmanager_exit(_dp_with_exit_1, NONE)
#                                 jump bb5
#                         else:
#                             block bb8(_dp_try_exc_0_1_0: Exception):
#                                 exc_param: _dp_try_exc_0_1_0
#                                 jump bb5
#                     block bb5(_dp_try_exc_0_1_0: Exception):
#                         exc_param: _dp_try_exc_0_1_0
#                         StoreName("_dp_with_exit_1", NONE)
#                         jump bb1
#                         block bb1:
#                             branch_table _dp_try_abrupt_kind_0_1_1 -> [bb0, bb2, bb3] default bb0
#                             block bb0:
#                                 return NONE
#                             block bb2:
#                                 return _dp_try_abrupt_payload_0_1_2
#                             block bb3:
#                                 raise _dp_try_abrupt_payload_0_1_2
#     block bb10(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         jump bb6(AbruptKind(Exception), Name("_dp_try_exc_0_1_0"))
#     block bb14(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         if_term exception_matches(current_exception(), BaseException):
#             then:
#                 jump bb15
#             else:
#                 jump bb20
#     block bb15(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         StoreName("_dp_with_ok_2", FALSE)
#         StoreName("_dp_with_reraise_3", await asynccontextmanager_exit(_dp_with_exit_1, current_exception()))
#         jump bb16
#     block bb16(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         if_term UnaryOp(Not, BinOp(Is, _dp_with_reraise_3, NONE)):
#             then:
#                 jump bb17
#             else:
#                 jump bb18
#     block bb17(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         raise _dp_with_reraise_3
#     block bb18(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         jump bb19
#     block bb19(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         jump bb9
#     block bb20(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         raise

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("run", MakeFunction(0:1, Coroutine, tuple_values(), NONE))
#         return NONE

# match_simple

match value:
    case 1:
        one()
    case _:
        other()

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb4:
#         StoreName("_dp_match_1", value)
#         jump bb1
#         block bb1:
#             if_term BinOp(Eq, _dp_match_1, 1):
#                 then:
#                     block bb2:
#                         one()
#                         return NONE
#                 else:
#                     block bb3:
#                         other()
#                         return NONE

# generator_yield


def gen():
    yield 1


# ==

# generator gen():
#     function_id: 0:1
#     block bb1:
#         yield 1
#         return NONE

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("gen", MakeFunction(0:1, Generator, tuple_values(), NONE))
#         return NONE

# yield_from


def gen():
    yield from it


# ==

# generator gen():
#     function_id: 0:1
#     block bb1:
#         yield from it
#         return NONE

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("gen", MakeFunction(0:1, Generator, tuple_values(), NONE))
#         return NONE

# with_exit_suppresses_exception

with Suppress():
    raise RuntimeError("boom")

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb4:
#         StoreName("_dp_tmp_4", Suppress())
#         StoreName("_dp_with_exit_1", contextmanager_get_exit(_dp_tmp_4))
#         contextmanager_enter(_dp_tmp_4)
#         StoreName("_dp_with_ok_2", TRUE)
#         jump bb17
#         block bb17:
#             raise RuntimeError("boom")
#     block bb0:
#         return NONE
#     block bb1:
#         branch_table _dp_try_abrupt_kind_0_1_1 -> [bb0, bb2, bb3] default bb0
#     block bb2:
#         return _dp_try_abrupt_payload_0_1_2
#     block bb3:
#         raise _dp_try_abrupt_payload_0_1_2
#     block bb5(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         StoreName("_dp_with_exit_1", NONE)
#         StoreName("_dp_tmp_4", NONE)
#         jump bb1
#     block bb6(_dp_try_exc_0_1_0: Exception, _dp_try_abrupt_kind_0_1_1: AbruptKind, _dp_try_abrupt_payload_0_1_2: AbruptPayload):
#         exc_param: _dp_try_exc_0_1_0
#         if_term _dp_with_ok_2:
#             then:
#                 jump bb7
#             else:
#                 jump bb8
#     block bb7(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         contextmanager_exit(_dp_with_exit_1, NONE)
#         jump bb5
#     block bb8(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         jump bb5
#     block bb9:
#         jump bb6(AbruptKind(Fallthrough), None)
#     block bb10(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         jump bb6(AbruptKind(Exception), Name("_dp_try_exc_0_1_0"))
#     block bb14(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         if_term exception_matches(_dp_try_exc_0_1_0, BaseException):
#             then:
#                 jump bb15
#             else:
#                 jump bb16
#     block bb15(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         StoreName("_dp_with_ok_2", FALSE)
#         contextmanager_exit(_dp_with_exit_1, _dp_try_exc_0_1_0)
#         jump bb9
#     block bb16(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         raise _dp_try_exc_0_1_0

# closure_cell_simple


def outer():
    x = 5

    def inner():
        return x

    return inner()


# ==

# function outer.<locals>.inner():
#     function_id: 0:1
#     block bb1:
#         return x

# function outer():
#     function_id: 0:2
#     block bb1:
#         StoreName("x", 5)
#         StoreName("inner", MakeFunction(0:1, Function, tuple_values(), NONE))
#         return inner()

# function _dp_module_init():
#     function_id: 0:3
#     block bb1:
#         StoreName("outer", MakeFunction(0:2, Function, tuple_values(), NONE))
#         return NONE

# bb_if_else_function


def choose(a, b):
    total = a + b
    if total > 5:
        return a
    else:
        return b


# ==

# function choose(a, b):
#     function_id: 0:1
#     block bb4:
#         StoreName("total", BinOp(Add, a, b))
#         jump bb1
#         block bb1:
#             if_term BinOp(Gt, total, 5):
#                 then:
#                     block bb2:
#                         return a
#                 else:
#                     block bb3:
#                         return b

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("choose", MakeFunction(0:1, Function, tuple_values(), NONE))
#         return NONE

# closure_cell_nonlocal


def outer():
    x = 5

    def inner():
        nonlocal x
        x = 2
        return x

    return inner()


# ==

# function outer.<locals>.inner():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", 2)
#         return x

# function outer():
#     function_id: 0:2
#     block bb1:
#         StoreName("x", 5)
#         StoreName("inner", MakeFunction(0:1, Function, tuple_values(), NONE))
#         return inner()

# function _dp_module_init():
#     function_id: 0:3
#     block bb1:
#         StoreName("outer", MakeFunction(0:2, Function, tuple_values(), NONE))
#         return NONE

# plain try / catch

try:
    print(1)
except Exception:
    print(2)

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         jump bb5
#         block bb5:
#             print(1)
#             return NONE
#     block bb2(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         if_term exception_matches(_dp_try_exc_0_1_0, Exception):
#             then:
#                 jump bb3
#             else:
#                 jump bb4
#     block bb3(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         print(2)
#         return NONE
#     block bb4(_dp_try_exc_0_1_0: Exception):
#         exc_param: _dp_try_exc_0_1_0
#         raise _dp_try_exc_0_1_0

# complicated generator


def complicated(a):
    for i in a:
        try:
            j = i + 1
            yield j
        except Exception:
            print("oops")
    else:
        print("finsihed")


# ==

# generator complicated(a):
#     function_id: 0:1
#     block bb3:
#         StoreName("_dp_iter_0_1_0", iter(a))
#         jump bb1
#         block bb1:
#             StoreName("_dp_tmp_0_1_1", next_or_sentinel(_dp_iter_0_1_0))
#             if_term BinOp(Is, _dp_tmp_0_1_1, ITER_COMPLETE):
#                 then:
#                     block bb4:
#                         print("finsihed")
#                         return NONE
#                 else:
#                     block bb2:
#                         StoreName("_dp_tmp_0_1_1", _dp_tmp_0_1_1)
#                         StoreName("i", _dp_tmp_0_1_1)
#                         Del { name: "_dp_tmp_0_1_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             jump bb9
#                             block bb9:
#                                 StoreName("j", BinOp(Add, i, 1))
#                                 yield j
#                                 jump bb1
#     block bb6(_dp_try_exc_0_1_2: Exception):
#         exc_param: _dp_try_exc_0_1_2
#         if_term exception_matches(current_exception(), Exception):
#             then:
#                 jump bb7
#             else:
#                 jump bb8
#     block bb7(_dp_try_exc_0_1_2: Exception):
#         exc_param: _dp_try_exc_0_1_2
#         print("oops")
#         jump bb1
#     block bb8(_dp_try_exc_0_1_2: Exception):
#         exc_param: _dp_try_exc_0_1_2
#         raise

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("complicated", MakeFunction(0:1, Generator, tuple_values(), NONE))
#         return NONE
