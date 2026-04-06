# subscript

x = a[b]

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", GetItem(a, b))
#         return NONE

# subscript_slice

x = a[1:2:3]

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", GetItem(a, slice(1, 2, 3)))
#         return NONE

# binary_add

x = a + b

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", BinOp(Add, a, b))
#         return NONE

# binary_bitwise_or

x = a | b

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", BinOp(Or, a, b))
#         return NONE

# unary_neg

x = -a

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", UnaryOp(Neg, a))
#         return NONE

# boolop_chain

x = a and b or c

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_target_2", a)
#         if_term _dp_target_2:
#             then:
#                 block bb2:
#                     StoreName("_dp_target_2", b)
#                     jump bb4
#             else:
#                 block bb3:
#                     jump bb4
#         block bb4:
#             StoreName("_dp_target_1", _dp_target_2)
#             if_term UnaryOp(Not, _dp_target_1):
#                 then:
#                     block bb5:
#                         StoreName("_dp_target_1", c)
#                         jump bb7
#                 else:
#                     block bb6:
#                         jump bb7
#             block bb7:
#                 StoreName("x", _dp_target_1)
#                 return NONE

# compare_lt

x = a < b

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", BinOp(Lt, a, b))
#         return NONE

# compare_chain

x = a < b < c

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("_dp_compare_1", a)
#         StoreName("_dp_compare_3", b)
#         StoreName("_dp_target_2", BinOp(Lt, _dp_compare_1, _dp_compare_3))
#         if_term _dp_target_2:
#             then:
#                 block bb2:
#                     StoreName("_dp_target_2", BinOp(Lt, _dp_compare_3, c))
#                     jump bb4
#             else:
#                 block bb3:
#                     jump bb4
#         block bb4:
#             StoreName("x", _dp_target_2)
#             return NONE

# compare_not_in

x = a not in b

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", UnaryOp(Not, BinOp(Contains, b, a)))
#         return NONE

# if_expr

x = a if cond else b

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         if_term cond:
#             then:
#                 block bb2:
#                     StoreName("_dp_tmp_1", a)
#                     jump bb4
#             else:
#                 block bb3:
#                     StoreName("_dp_tmp_1", b)
#                     jump bb4
#         block bb4:
#             StoreName("x", _dp_tmp_1)
#             return NONE

# named_expr

x = (y := f())

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("y", f())
#         StoreName("x", y)
#         return NONE

# lambda_simple

x = lambda y: y + 1

# ==

# function <lambda>(y):
#     function_id: 0:1
#     display_name: <lambda>
#     block bb1:
#         return BinOp(Add, y, 1)

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("x", MakeFunction(0:1, Function, tuple_values(), NONE))
#         return NONE

# generator_expr

x = (i for i in it)

# ==

# generator <genexpr>(_dp_iter_2):
#     function_id: 0:1
#     display_name: <genexpr>
#     block bb2:
#         StoreName("_dp_iter_3", _dp_iter_2)
#         jump bb1
#         block bb1:
#             if_term TRUE:
#                 then:
#                     block bb3:
#                         StoreName("_dp_tmp_4", next_or_sentinel(_dp_iter_3))
#                         if_term BinOp(Is, _dp_tmp_4, ITER_COMPLETE):
#                             then:
#                                 block bb4:
#                                     return NONE
#                             else:
#                                 block bb5:
#                                     StoreName("i", _dp_tmp_4)
#                                     yield i
#                                     jump bb1
#                 else:
#                     block bb0:
#                         return NONE

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_genexpr_1", MakeFunction(0:1, Generator, tuple_values(), NONE))
#         StoreName("x", _dp_genexpr_1(iter(it)))
#         return NONE

# list_literal

x = [a, b]

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", list(tuple_values(a, b)))
#         return NONE

# list_literal_splat

x = [a, *b]

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", list(BinOp(Add, tuple_values(a), tuple_from_iter(b))))
#         return NONE

# tuple_splat

x = (a, *b)

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", BinOp(Add, tuple_values(a), tuple_from_iter(b)))
#         return NONE

# set_literal

x = {a, b}

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", set(tuple_values(a, b)))
#         return NONE

# dict_literal

x = {"a": 1, "b": 2}

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", dict(tuple_values(tuple_values("a", 1), tuple_values("b", 2))))
#         return NONE

# dict_literal_unpack

x = {"a": 1, **m, "b": 2}

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", BinOp(Or, BinOp(Or, dict(tuple_values(tuple_values("a", 1))), dict(m)), dict(tuple_values(tuple_values("b", 2)))))
#         return NONE

# list_comp

x = [i for i in it]

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
#                         StoreName("i", _dp_tmp_0_1_1)
#                         Del { name: "_dp_tmp_0_1_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             GetAttr(_dp_tmp_1, "append")(i)
#                             jump bb1

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_listcomp_3", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("x", _dp_listcomp_3(it))
#         return NONE

# set_comp

x = {i for i in it}

# ==

# function _dp_setcomp_3(_dp_iter_2):
#     function_id: 0:1
#     display_name: <setcomp>
#     block bb3:
#         StoreName("_dp_tmp_1", set())
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
#                         StoreName("i", _dp_tmp_0_1_1)
#                         Del { name: "_dp_tmp_0_1_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             GetAttr(_dp_tmp_1, "add")(i)
#                             jump bb1

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_setcomp_3", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("x", _dp_setcomp_3(it))
#         return NONE

# dict_comp

x = {k: v for k, v in it}

# ==

# function _dp_dictcomp_5(_dp_iter_4):
#     function_id: 0:1
#     display_name: <dictcomp>
#     block bb3:
#         StoreName("_dp_tmp_1", dict())
#         StoreName("_dp_iter_0_1_0", iter(_dp_iter_4))
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
#                         StoreName("_dp_tmp_0_1_2", unpack(_dp_tmp_0_1_1, tuple_values(TRUE, TRUE)))
#                         StoreName("k", GetItem(_dp_tmp_0_1_2, 0))
#                         StoreName("v", GetItem(_dp_tmp_0_1_2, 1))
#                         Del { name: "_dp_tmp_0_1_2", quietly: false }
#                         Del { name: "_dp_tmp_0_1_1", quietly: false }
#                         jump bb5
#                         block bb5:
#                             StoreName("_dp_dictcomp_key_2", k)
#                             StoreName("_dp_dictcomp_value_3", v)
#                             StoreName("_dp_assign_value_6", _dp_dictcomp_value_3)
#                             StoreName("_dp_assign_obj_7", load_deleted_name("_dp_tmp_1", _dp_tmp_1))
#                             StoreName("_dp_assign_index_8", _dp_dictcomp_key_2)
#                             SetItem(_dp_assign_obj_7, _dp_assign_index_8, _dp_assign_value_6)
#                             jump bb1

# function _dp_module_init():
#     function_id: 0:2
#     block bb1:
#         StoreName("_dp_dictcomp_5", MakeFunction(0:1, Function, tuple_values(), NONE))
#         StoreName("x", _dp_dictcomp_5(it))
#         return NONE

# attribute_non_chain

x = f().y

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", GetAttr(f(), "y"))
#         return NONE

# fstring_simple

x = f"{a}"

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", format(a))
#         return NONE

# tstring_simple

x = t"{a}"

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", templatelib_Template(*tuple_values(templatelib_Interpolation(a, "a", NONE, ""))))
#         return NONE

# complex_literal

x = 1j

# ==

# snapshot regeneration failed
# panic: complex literal reached late core BlockPy boundary

# float_literal_long

x = 1.234567890123456789

# ==

# function _dp_module_init():
#     function_id: 0:1
#     block bb1:
#         StoreName("x", 1.2345678901234567)
#         return NONE
