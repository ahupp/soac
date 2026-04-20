# yield_from

def gen():
    yield from it

# ==

# -- pre-bb --
def _dp_module_init():

    def gen():
        yield from it

# -- bb --
def _dp_bb_gen_0(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_iter_1 = iter(it)
    __dp__.setitem(_dp_state, "_dp_yieldfrom", _dp_yield_from_iter_1)
    return __dp__.try_jump_term(_dp_bb_gen_1, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), (_dp_bb_gen_1,), _dp_bb_gen_2, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), (_dp_bb_gen_2, _dp_bb_gen_3, _dp_bb_gen_4), None, (), (), __dp__._BB_TRY_PASS_TARGET, None)
def _dp_bb_gen_1(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_y_2 = next(_dp_yield_from_iter_1)
    return __dp__.jump(_dp_bb_gen_5, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_2(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_stop_5 = __dp__.current_exception()
    return __dp__.brif(__dp__.exception_matches(_dp_yield_from_stop_5, StopIteration), _dp_bb_gen_3, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), _dp_bb_gen_4, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_3(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_result_4 = _dp_yield_from_stop_5.value
    __dp__.setitem(_dp_state, "_dp_yieldfrom", None)
    __dp__.setitem(_dp_state, "pc", _dp_state.get("_dp_pc_done", __dp__._GEN_PC_DONE))
    return __dp__.ret(None)
def _dp_bb_gen_4(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    __dp__.setitem(_dp_state, "_dp_yieldfrom", None)
    return __dp__.raise_(_dp_yield_from_stop_5)
def _dp_bb_gen_5(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    __dp__.setitem(_dp_state, "pc", 8)
    __dp__.setitem(_dp_state, "args", (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
    return __dp__.ret(_dp_yield_from_y_2)
def _dp_bb_gen_6(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_sent_3 = __dp__.gen_resume_value(_dp_state)
    _dp_yield_from_exc_6 = __dp__.gen_resume_exception(_dp_state)
    return __dp__.brif(_dp_yield_from_exc_6 is not None, _dp_bb_gen_7, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), _dp_bb_gen_15, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_7(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    return __dp__.brif(__dp__.exception_matches(_dp_yield_from_exc_6, GeneratorExit), _dp_bb_gen_8, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), _dp_bb_gen_11, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_8(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_close_7 = getattr(_dp_yield_from_iter_1, "close", None)
    return __dp__.brif(_dp_yield_from_close_7 is not None, _dp_bb_gen_9, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), _dp_bb_gen_10, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_9(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_close_7()
    return __dp__.jump(_dp_bb_gen_10, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_10(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    __dp__.setitem(_dp_state, "_dp_yieldfrom", None)
    return __dp__.raise_(_dp_yield_from_exc_6)
def _dp_bb_gen_11(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_throw_8 = getattr(_dp_yield_from_iter_1, "throw", None)
    return __dp__.brif(_dp_yield_from_throw_8 is None, _dp_bb_gen_10, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), _dp_bb_gen_12, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_12(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    return __dp__.try_jump_term(_dp_bb_gen_13, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), (_dp_bb_gen_13,), _dp_bb_gen_14, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), (_dp_bb_gen_14, _dp_bb_gen_3, _dp_bb_gen_4), None, (), (), __dp__._BB_TRY_PASS_TARGET, None)
def _dp_bb_gen_13(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_y_2 = _dp_yield_from_throw_8(_dp_yield_from_exc_6)
    return __dp__.jump(_dp_bb_gen_5, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_14(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_stop_5 = __dp__.current_exception()
    return __dp__.brif(__dp__.exception_matches(_dp_yield_from_stop_5, StopIteration), _dp_bb_gen_3, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), _dp_bb_gen_4, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_15(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    return __dp__.brif(_dp_yield_from_sent_3 is None, _dp_bb_gen_16, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), _dp_bb_gen_18, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_16(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    return __dp__.try_jump_term(_dp_bb_gen_17, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), (_dp_bb_gen_17,), _dp_bb_gen_20, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), (_dp_bb_gen_20, _dp_bb_gen_3, _dp_bb_gen_4), None, (), (), __dp__._BB_TRY_PASS_TARGET, None)
def _dp_bb_gen_17(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_y_2 = next(_dp_yield_from_iter_1)
    return __dp__.jump(_dp_bb_gen_5, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_18(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    return __dp__.try_jump_term(_dp_bb_gen_19, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), (_dp_bb_gen_19,), _dp_bb_gen_20, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), (_dp_bb_gen_20, _dp_bb_gen_3, _dp_bb_gen_4), None, (), (), __dp__._BB_TRY_PASS_TARGET, None)
def _dp_bb_gen_19(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_y_2 = _dp_yield_from_iter_1.send(_dp_yield_from_sent_3)
    return __dp__.jump(_dp_bb_gen_5, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_20(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    _dp_yield_from_stop_5 = __dp__.current_exception()
    return __dp__.brif(__dp__.exception_matches(_dp_yield_from_stop_5, StopIteration), _dp_bb_gen_3, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8), _dp_bb_gen_4, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb_gen_start(_dp_args_ptr):
    _dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8 = __dp__.take_args(_dp_args_ptr)
    return __dp__.jump(_dp_bb_gen_0, (_dp_state, _dp_yield_from_iter_1, _dp_yield_from_y_2, _dp_yield_from_stop_5, _dp_yield_from_result_4, _dp_yield_from_exc_6, _dp_yield_from_sent_3, _dp_yield_from_close_7, _dp_yield_from_throw_8))
def _dp_bb__dp_module_init_start(_dp_args_ptr):
    __dp__.store_global(globals(), "gen", __dp__.def_gen(21, (_dp_bb_gen_0, _dp_bb_gen_1, _dp_bb_gen_2, _dp_bb_gen_3, _dp_bb_gen_4, _dp_bb_gen_5, _dp_bb_gen_6, _dp_bb_gen_7, _dp_bb_gen_8, _dp_bb_gen_9, _dp_bb_gen_10, _dp_bb_gen_11, _dp_bb_gen_12, _dp_bb_gen_13, _dp_bb_gen_14, _dp_bb_gen_15, _dp_bb_gen_16, _dp_bb_gen_17, _dp_bb_gen_18, _dp_bb_gen_19, _dp_bb_gen_20, _dp_bb_gen_start), (False, True, True, True, True, True, False, True, True, True, True, True, False, True, True, True, False, True, False, True, True, True), (-1, 0, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 12, -1, -1, -1, 16, -1, 18, -1, -1), "gen", "gen", ("_dp_yield_from_iter_1", "_dp_yield_from_y_2", "_dp_yield_from_stop_5", "_dp_yield_from_result_4", "_dp_yield_from_exc_6", "_dp_yield_from_sent_3", "_dp_yield_from_close_7", "_dp_yield_from_throw_8"), ()))
    return __dp__.ret(None)
_dp_module_init = __dp__.def_fn(_dp_bb__dp_module_init_start, "_dp_module_init", "_dp_module_init", (), (), __name__)
del _dp_bb__dp_module_init_start
