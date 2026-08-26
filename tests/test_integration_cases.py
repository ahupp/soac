from __future__ import annotations

import sys
from dataclasses import replace
from pathlib import Path

import pytest

from scripts.strict_pyperformance_sources import strict_opt_in
from tests._integration import (
    exec_integration_validation,
    integration_module,
    split_integration_case,
)
from tests._strict_integration import (
    StrictValidationCase,
    assert_strict_source_rejected,
    create_strict_project,
)

MODULES_DIR = Path(__file__).resolve().parent / "integration_modules"
# Preserve the original ordinary programs/tails and report their excluded SOAC
# observations explicitly. These legacy frame-only variants are not admitted
# strict programs: run=False must not turn missing admission into a fake runtime
# failure. Frame-free semantic companions remain in test_strict_call_context.py.
SOAC_FRAME_INSPECTION_XFAILS = {
    "yield_from_stack_names": "source and caller frame names through sys._getframe()/f_back",
    "dir_filters": "function-local names through argumentless dir()",
    "locals_cell_contents": "closure/cell contents through locals()",
    "named_expression_locals_unbound": "walrus binding visibility through locals()",
    "exception_cleanup_name": "deleted exception binding visibility through locals()",
}

# Reviewed individually; adding a new stock source does not opt it into strict
# execution. The original bodies and validation tails remain unchanged. The
# genuine checker has accepted all 23 with only the explicit strict future.
STRICT_BASIC_CASES = {
    "assert_shadowing": ("trigger",),
    "bounded_loop": ("bounded_loop",),
    "chained_comparison": ("value", "probe"),
    "chained_comparison_side_effects_once": ("value", "probe"),
    "compare_in_while": ("loop_compare",),
    "float_literal_precision": (),
    "for_else_break_minimal": ("collect_for_else_break_minimal",),
    "for_else_continue": ("collect_for_else_continue",),
    "for_else_continue_minimal": ("collect_for_else_continue_minimal",),
    "for_loop_carried_local": ("run_plain", "run_getitem"),
    "for_loop_empty": ("run",),
    "for_loop_no_else": ("exercise",),
    "fstring_debug_conversion": ("format_debug",),
    "fstring_format_shadow": ("format", "trigger"),
    "fstring_ifexpr_interpolation": ("pluralize",),
    "listcomp_unbound_target": ("run",),
    "map_unpack": (),
    "map_unpacking_module": ("summarize",),
    "match_guard": ("probe",),
    "maybe_unbound_join_not_loaded": ("run",),
    "named_expr_while_not": ("walk_until_truthy",),
    "slice_binding": ("collect_segments",),
    "tuple_unpacking_module": ("parse_line",),
}


# Exception, closure, and cleanup cases reviewed separately from the basic
# expression cohort. Keep original bodies and validators; checker rejection
# must be resolved explicitly, never hidden by a cast or a runtime fallback.
STRICT_CONTROL_CASES = {
    "except_as_clears_exception": ("capture", "count_exception_referrer_frames"),
    "except_star_bind_group": ("handle",),
    "except_star_group": ("handle",),
    "try_orelse_on_exception": ("exercise",),
    "closure_cell_nonlocal": ("outer",),
    "closure_attr": ("outer",),
    "nonlocal_del_binding": ("outer", "main"),
    "delete_nonlocal_compiles": ("outer",),
    "with_exit_suppresses_exception": ("run",),
    "with_return_context": ("use_context", "run"),
    "with_extended_targets": ("unpack_starred_list",),
    "with_special_lookup": ("run",),
    "with_context_exception_leak": ("leak_check",),
    "exception_refcycle_after_except": ("run",),
    "exception_refcycle_args_tuple": ("run",),
    "support_current_exception_recursion_minimal": ("exercise",),
    "assert_raises_refcount": ("_boom", "run"),
    "for_loop_temp_drop": ("run",),
    "coroutine_return_value": ("main", "manual"),
}

# Reviewed class/target/lifetime cases. Custom attribute hooks keep the
# language's automatic dynamic class path; the surrounding functions still
# require actual strict admission and the requested synchronous entry.
STRICT_CLASS_LIFETIME_CASES = {
    "assign_target_eval_order": (
        "run_named_subscript",
        "run_nested_subscript",
        "run_attr",
    ),
    "augassign_target_eval_order": (
        "run_named_subscript",
        "run_nested_subscript",
        "run_attr",
    ),
    "chained_assignment": (),
    "effect_only_selected_expr_semantics": (
        "ifexpr_effect",
        "boolop_and_effect",
        "boolop_or_effect",
        "not_boolop_effect",
        "compare_chain_effect",
    ),
    "listcomp_iter_once": ("run",),
    "unpack_temp_drop": ("run",),
}

# Reviewed comprehension scope, class-cell, and protocol cases. The five
# entries without global function witnesses prove module admission and their
# original behavior only; their initializer always uses entry_interpreter.
STRICT_COMPREHENSION_PROTOCOL_CASES = {
    "comprehension_filters": ("run",),
    "comprehension_iter_list": ("run",),
    "comprehension_scope_shadowing": (),
    "dictcomp_temp_collision": ("dict_comp_fib",),
    "dictcomp_temp_collision_class": (),
    "class_comprehension": (),
    "class_scope_comprehension": (),
    "listcomp_classcell": ("classcell_values",),
    "richcompare_rhs_fallback": ("run",),
    "property_setter": (),
    "class_private_attribute_set": ("run",),
    "list_setitem_specialization": ("set_item",),
}

# This reviewed cohort also exercises the interpreter-only acceptance path.
# Its original source bodies and validators have no retained-entry assumptions.
# Enroll further cohorts only after reviewing their strict/native outcomes;
# in particular, inherited strict eval/exec is not ordinary execution.
CPYTHON_INTERPRETER_CASES = frozenset(STRICT_COMPREHENSION_PROTOCOL_CASES) | {
    # The original lifetime body mutates a sealed class variable in __new__.
    # Interpreter-only execution must enforce the same explicit rejection.
    "iter_refcount_behavior",
    # Reusing an exposed original code object is not native execution authority.
    "genexpr_iterator_semantics",
    # Original semantic source runs in every enrolled backend. Only SOAC's
    # excluded frame-inspection fragments are split from the validators below.
    "async_await_comprehension",
    "async_genexpr_async_comp",
    "asyncgen_expression_async_for",
    "asyncio_taskgroup_base_error_refcycle",
    "coroutine_closure_state",
    "except_as_clears_exception",
    "listcomp_iter_once",
    "meta_path_pathfinder_preserved",
    "named_expr_comprehension_scope",
    "nested_async_comprehension",
    "sync_generator_closure_state",
    "class_scope_nonlocal_syntaxerror",
    "enum_dynamic_members_vars_update",
    "scope_locals",
}


# Split only the excluded inspection fragments from retained SOAC validation.
# Ordinary CPython and the interpreter-only backend keep the original tails.
def _retained_tail_replacement(case_name, original_fragment, replacement):
    _, original = split_integration_case(MODULES_DIR / f"{case_name}.py")
    assert original.count(original_fragment) == 1, (
        f"{case_name}: reviewed inspection no longer matches the original validator"
    )
    return original.replace(original_fragment, replacement, 1)



# Keep every original send/throw/yield-from/closure/result assertion. The SOAC
# tails neither inspect a suspended frame nor require an inspection refusal.
ORDINARY_SUSPENDED_FRAME_CHECKS = {
    "coroutine_closure_state": (
        """    if type(coro).__name__ == "Coroutine":
        assert coro.cr_frame is None
    else:
        assert coro.cr_frame is not None
""",
        "",
    ),
    "sync_generator_closure_state": (
        """    if type(counter).__name__ == "ClosureGenerator":
        assert not hasattr(counter, "gi_frame")
    else:
        assert hasattr(counter, "gi_frame")
""",
        "",
    ),
}

RETAINED_SEMANTIC_VALIDATORS = {
    case_name: _retained_tail_replacement(case_name, *fragments)
    for case_name, fragments in ORDINARY_SUSPENDED_FRAME_CHECKS.items()
}

# Only the function-local observations are omitted. These original modules
# still exercise their class mappings and non-inspection lexical operations.
RETAINED_SEMANTIC_VALIDATORS.update({
    "scope_locals": _retained_tail_replacement(
        "scope_locals",
        '''    if __dp_integration_soac__:
        try:
            module.function_locals()
        except NotImplementedError:
            pass
        else:
            raise AssertionError("expected locals() to be unsupported")
    else:
        func_locals = module.function_locals()
        assert "h" in func_locals
        assert "_dp_fn_h" not in func_locals
        del func_locals["h"]
        assert func_locals == {"x": 2, "y": 7, "w": 6}
''',
        "",
    ),
})


STRICT_NAMESPACE_SEMANTIC_CASES = {
    "enum_dynamic_members_vars_update": ("Foo.upper",),
    "scope_locals": ("class_locals", "class_namespace_overrides_closure"),
}


def _retained_semantic_validators(reviewed_cases):
    return {
        name: RETAINED_SEMANTIC_VALIDATORS[name]
        for name in reviewed_cases
        if name in RETAINED_SEMANTIC_VALIDATORS
    }


# Individually reviewed lexical class/closure cases. Plain-method witnesses use
# raw own class namespaces; only the two cases with no functions remain
# initializer/behavior-only. No original body or validator is adapted.
STRICT_CLASS_CLOSURE_CASES = {
    "class_attr_default": ("Example.method",),
    "class_body_closure_self": ("make", "CDLL.__init__"),
    "class_body_default_closure": ("make", "run"),
    "class_body_outer_local": ("build",),
    "class_scope_capture": ("outer",),
    "class_scope_inner_capture": ("outer",),
    "class_scope_inner_sees_outer_scope": (),
    "class_scope_inner_sees_outer_scope_closure": ("inner_sees_outer_scope_closure",),
    "class_method_outer_cell": ("run",),
    "class_method_import_shadowing": ("Example.__init__",),
    "class_method_time_shadowing": ("Base.__init__", "Base.time"),
    "class_lookup_lambda_recursion": (),
    "lambda_classcell": ("classcell_lambda",),
    "lambda_qualname": ("global_function",),
    "lambda_qualname_minimal": ("global_function",),
    "nested_class_binding": ("get_member",),
    "nested_class_closure": ("use_container", "Container.build"),
    "nested_class_method_shadowing": ("Outer.Inner.format_help",),
    "nested_class_nonlocal_method": ("Outer.run",),
    "nested_class_qualname": ("Container.make",),
    "nested_classcell_capture": ("exercise",),
    "nested_super": ("Container.build", "Base.probe"),
    "nonlocal_binding": ("Example.trigger",),
    "method_local_shadowing": ("Example.run",),
    "posonly_shadows_class_attr": ("make_value",),
}

# Individually reviewed source/validator pairs for attribute binding, ordinary
# descriptor protocols, method names, and class-cell errors. Unknown descriptor
# and decorator classes take automatic dynamic participation; every listed
# callable witness is still checked through the real source admission path.
STRICT_ATTRIBUTE_PROTOCOL_CASES = {
    "ast_visit_ellipsis": ("visit_ellipsis",),
    "descriptor_special_method_binding": (),
    "list_getitem_specialization": ("get_item",),
    "method_docstring": ("Example.do_thing", "build_annotations"),
    "method_name_clash": ("Example.date",),
    "method_name_local_binding": ("Example.close",),
    "method_named_open": ("write_and_read",),
    "method_named_open_calls_builtin": ("write_and_read",),
    "nested_class_base": ("get_base_name",),
    "nested_classes": ("record",),
    "nested_getattribute": ("Container.probe",),
    "obscure_super_errors": ("exercise",),
    "private_method": ("Example.reveal",),
    "private_name_mangling_empty_class_name": ("run",),
    "property_copydoc": ("copydoc",),
    "property_copydoc_uses_original_attribute_name": ("copydoc",),
    "property_decorator_order": ("Example.__init__",),
    "property_sub_doc": ("get_doc",),
    "raise_from_nonexception_cause": ("run",),
    "super_empty_classcell": ("exercise",),
    "super_new_base": ("build_child",),
    "truthiness_notimplemented": ("run",),
    "with_class_mock_calls": ("run",),
}

# Original coroutine/generator bodies and validators, each reviewed and run
# through authenticated startup in both modes. Three module-only entries have
# no synchronous public witness; their behavior is not a native-entry claim.
STRICT_ASYNC_GENERATOR_CASES = {
    "await_return_passthrough": ("check",),
    "coroutine_await_lowering": (),
    "coroutine_dunder_await_direct": ("direct",),
    "coroutine_closure_state": ("make_runner",),
    "closure_backed_coroutine_persistence": ("make_runner", "manual"),
    "nested_coroutine_capture": ("build",),
    "simple_sync_generator_stop": ("make_counter",),
    "sync_generator_default_param": ("collect_default", "collect_explicit"),
    "sync_generator_closure_state": (
        "make_counter",
        "exercise_throw",
        "exercise_yield_from",
    ),
    "sync_generator_nested_list": (
        "collect",
        "collect_tupled",
        "collect_single",
        "peek_genexpr_progress",
    ),
    "sync_generator_terminal_cleanup": (
        "completed_payload_released",
        "escaped_payload_released",
        "closed_throw_uses_terminal_state",
    ),
    "sync_generator_throw_cleanup": ("make_gen", "exercise"),
    "bb_generator_assign_yield": (),
    "generator_boolop_expr": ("main",),
    "generator_boolop_filter_scope": ("fields_in_init_order", "Field.__init__"),
    "generator_filter_projection": ("fields_in_init_order", "Field.__init__"),
    "generator_return_yield_expr": (),
    "async_generator_closure_state": ("build",),
    "asyncgen_expression_async_for": ("make_arange", "get_values"),
}

# Keep the original reviewed ordering: delegated exception handling followed
# by TaskGroup lifetime checks also detects leaked handled-exception state.
# Admission is not a waiver for a behavioral failure in either execution mode.
STRICT_ASYNC_PROTOCOL_CASES = {
    "async_with_return_after_awaiting_aexit": ("check",),
    "async_with_nonawaitable_aenter": ("get_error",),
    "bad_async_enter": ("main",),
    "bad_async_exit": ("main",),
    "await_inside_except_raise": ("run",),
    "async_contextmanager_stopiter": ("check",),
    "async_contextmanager_stopiter_regression": ("check",),
    "genexpr_async_aiter": ("main",),
    "genexpr_async_await": ("main",),
    "async_genexpr_async_comp": ("main",),
    "nested_async_comprehension": ("get_values", "get_gen_values"),
    "async_await_comprehension": (),
}

STRICT_YIELD_LIFETIME_CASES = {
    "yield_from_module": (),
    "yield_from_gi_code_name": ("get_name",),
    "yield_from_gi_yieldfrom": ("get_yieldfrom_name",),
    "yield_from_throw_clears_delegate": ("throw_check",),
    "generator_object_name": (),
    "generator_exception_context": ("exercise",),
    "eval_source_yield_from": (),
    "asyncio_taskgroup_base_error_refcycle": ("referrer_frames",),
    "asyncio_wait_for_release": ("leak_check",),
    "asyncio_wait_for_release_regression": ("throw_cancelled_check", "leak_check"),
}

# These declarations are allowed by the strict language even when the target
# is initially absent. They have their own fixture while the checker's bounded
# declared-global reconciliation is integrated; a rejection is a test failure.
STRICT_DECLARED_GLOBAL_CASES = {
    "exception_cleanup_global": ("cleanup_global_exception_name",),
    "except_star_global_binding": ("run",),
}

# These source/validator pairs were reviewed separately and exercised with
# genuine offline publications. Keep the assertions unchanged: enrollment is
# not a waiver for the remaining cleanup or generator-identity regressions.
STRICT_CLASS_SCOPE_CASES = {
    "class_attr_delete": (),
    "class_attribute_unpacking": (),
    "class_body_closure_self_flags": ("make", "CDLL.__init__"),
    "class_body_globals": (),
    "class_global_classcell": ("exercise",),
    "class_kwarg_metaclass_expansion": ("Meta.__new__",),
    "class_method_decorators": (),
    "class_namespace_get": ("Example.get",),
    "class_private_attribute": ("use_example", "Example.read"),
    "class_scope_attr_vs_global": ("C1.read",),
    "class_scope_class_method_reads_global": ("outer_with_class_method_reads_global",),
    "class_scope_delete_local": (),
    "class_scope_except_global_binding": (),
    "class_scope_except_local_binding": (),
    "class_scope_global_assignment": (),
    "class_scope_inner_class_global_assignment": (
        "outer_with_inner_class_global_assignment",
    ),
    "class_scope_method_global_assignment": ("C3.set_x", "C3.read_x"),
    "class_scope_method_nonlocal_inner": ("C4.outer",),
    "class_scope_nonlocal_assign": ("outer",),
    "class_scope_nonlocal_for_target": ("outer",),
    "class_scope_nonlocal_inner_class": ("outer_with_nonlocal_and_inner_class",),
    "class_scope_nonlocal_named_expr": ("outer",),
    "class_scope_with_target_local": (),
}

STRICT_ITERATOR_METADATA_CASES = {
    "genexpr_inherited_capture_order": ("genexpr_scope",),
    "genexpr_iter_once": ("run",),
    "genexpr_name": ("get_genexpr_name",),
    "genexpr_name_regression": ("get_name",),
    "iter_refcount_behavior": ("run",),
    "sync_genexpr_progress": ("progress", "stops"),
    "sync_nested_genexpr_progress": (
        "nested_progress_first_step",
        "nested_progress_second_step",
        "collect_progress",
        "collect_stops",
    ),
    "named_expr_comprehension_scope": ("comp_scope", "genexpr_scope"),
    "recursive_local_function": ("exercise",),
}

STRICT_BINDING_PATTERN_CASES = {
    "global_class_qualname": ("make_name",),
    "global_inner_function_qualname": ("build_qualnames",),
    "global_inner_function_qualname_regression": ("build_qualnames",),
    "global_only_body_compiles": ("f",),
    "match_builtin_class_pattern": (),
    "match_builtin_class_pattern_regression": (),
    "match_builtin_class_pattern_subject_temp": (),
    "templatelib_cleanup": ("make",),
    "bb_entry_missing_state_deleted": ("f",),
    "bb_entry_module_init_empty": (),
    "stdlib_import_alias": (),
    "cleanup_dp_globals": ("has_dp_name",),
}

STRICT_TYPING_METADATA_CASES = {
    "generic_module": ("make_specialization",),
    "pep695_type_aliases": (),
    "type_alias_in_function": ("make_alias",),
    "type_checking_annotations": (),
    "type_checking_annotations_regression": (),
    "typing_nested_class_repr": ("Container.make",),
}

STRICT_MODULE_PROTOCOL_CASES = {
    "builtins_deepcopy": ("run",),
    "clif_vectorcall_non_repo_file": ("f",),
    "cpython_strptime_failure": ("parse_invalid_offset",),
    "ctypes_import": (),
    "dunder_getitem_decorator": ("call_original",),
    "mock_class_property": ("mock_class_property_ok",),
    "module_all_helpers": (),
    "module_docstring_direct_loader": (),
    "operator_all_module_attrs": ("exercise",),
    "scope_dict_proxy_contains": ("has_name",),
    "scope_dict_proxy_items": ("get_globals_items",),
    "skip_outside_repo_transform": ("imported_without_transform",),
    "translation_module": ("call_translate", "translate_message"),
    "typing_import": (),
}

STRICT_FRAMEWORK_ANNOTATION_CASES = {
    "annotation_only_body_compiles": ("f",),
    "annotationlib_fakeglobals": ("annotate", "run"),
    "annotationlib_nonlocal_scope": ("run",),
    "class_annotations_deferred": (),
    "class_annotations_forwardref": (),
    "class_dictproxy_no_annotate": ("run",),
    "class_no_annotations_dunder_annotate": (),
    "dataclass_module": (),
    "dataclass_slots_union_default": ("build_example",),
    "dataclasses_make_dataclass_invalid_field": (),
    "dynamicclassattribute_class_scope_getter": ("get_value",),
    "enum_flag_nonmember_auto_or": ("build_values",),
    "enum_new_super": ("build_enum",),
    "function_local_annotation": ("exercise",),
    "functools_lru_cache_pickle_method": ("pickle_cached_method",),
    "functools_singledispatch_qualname": (
        "Wrapper.make_nested_class",
        "Wrapper.bad_register_message",
    ),
    "functools_singledispatch_repr": ("build_message",),
    "generic_namedtuple_fields": (),
    "singledispatch_qualname": ("Wrapper.bad_register_message",),
}

# These operations deliberately differ under the documented strict contract.
# Stock still runs the original validator; strict runs an explicit rejection
# validator after proving the actual public function entry and module owner.
STRICT_CONTRACT_REJECTION_CASES = {
    "functiontype_globals": (
        "StrictRuntimeUnavailableError",
        ("make_inner", "run"),
        "module.run()",
    ),
    "genexpr_iterator_semantics": (
        "StrictRuntimeUnavailableError",
        ("make", "replay", "main"),
        "module.main()",
    ),
    "mutated_function_defaults": (
        "StrictMutationError",
        ("make", "run"),
        "module.run()",
    ),
    "mutated_closure_function_defaults": (
        "StrictMutationError",
        ("make", "run"),
        "module.run()",
    ),
    "builtin_dynamic_global_shadow": (
        "StrictMutationError",
        ("install_len_and_call",),
        "module.install_len_and_call([1, 2, 3])",
    ),
}

# String compilation inherits strict language flags; explicit dictionaries do
# not create authenticated dynamic-code provenance, and raw strict code objects
# do not grant an execution owner. Neither restriction is frame inspection.
STRICT_DYNAMIC_CODE_CASES = {
    "exec_globals": ("run",),
    "exec_globals_kw": ("run",),
    "exec_locals_kw": ("run",),
    "exec_closure_kw": ("run",),
    "eval_closure": ("run",),
    "exec_locals": ("run",),
    "class_scope_nonlocal_syntaxerror": ("nonlocal_in_class_body_error",),
}

STRICT_IMPORT_LIFECYCLE_CASES = {
    "dotted_import_alias": (),
    "future_import_invalid_feature": (),
    "import_star_math": (),
    "jit_main_module_plan_alias": (),
    "meta_path_pathfinder_preserved": ("import_with_filtered_meta_path",),
    "taskgroup_propagate_cancellation_refcycle": ("run",),
    "transform_temp_module": ("import_temp_module",),
}

STRICT_IMPORT_DEPENDENCIES = (
    "dotted_import_alias_pkg/__init__.py",
    "dotted_import_alias_pkg/submodule.py",
    "missing_from_import_target.py",
)

STRICT_IMPORT_INTEROP_CASES = {
    "asyncio_interrupt_identity": ("unresolved-attribute", ("run_interrupt_case",)),
    "dotted_import_alias_rebind": ("unresolved-import", ("alias_rebind_attrs",)),
    "missing_from_import": ("unresolved-import", ()),
}

# The original code is valid ordinary Python but deliberately violates static
# checking. Preserve both real strict rejection and ordinary interoperability;
# do not claim the original function bodies were transformed.
STRICT_CONTROL_INTEROP_CASES = {
    # Dynamic class namespace insertion does not declare this bare source
    # reference to ty. The original Enum behavior remains an ordinary
    # dependency; the separate selected Enum case tests actual vars() writes.
    "enum_ignore_dynamic_names": ("unresolved-reference", ()),
    # A deferred method creates _Foo__x1, which another scope reads without a
    # module-level declaration. The absent-global warning does not relax that
    # unresolved read. Keep the complete original source/validator ordinary;
    # selected frame-free walrus semantics remain in dictcomp_temp_collision
    # and named_expr_comprehension_scope, including the original fib expression.
    "named_expression_cases": (
        "unresolved-reference",
        ("dict_comp_fib", "genexp_scope_state", "mangled_global_value"),
    ),
    "closure_cells": (
        "unresolved-reference",
        (
            "outer_read",
            "outer_assign_local",
            "outer_assign_local_read_before",
            "outer_nonlocal",
        ),
    ),
    "exception_cleanup_local": ("unresolved-reference", ("cleanup_local",)),
    "exception_cleanup_deleted": (
        "unresolved-reference",
        ("cleanup_deleted", "unbound_after_delete"),
    ),
    "with_enter_result_lifetime": ("call-non-callable", ("run",)),
    "sync_generator_preserved_delete": ("unresolved-reference", ("make_gen",)),
    "delattr_missing": ("unresolved-attribute", ()),
    "io_open_class_attr": ("no-matching-overload", ("read_self",)),
    "raise_from_import_shadow": (
        "invalid-assignment",
        ("raise_from_with_import_patch",),
    ),
    "super_rebind_class_name": ("invalid-assignment", ("Alias.__init__",)),
    "with_error_messages": ("invalid-context-manager", ("run_sync", "run_async")),
    "with_protocol_errors": ("invalid-context-manager", ("exercise",)),
    "async_for_missing_aiter": ("not-iterable", ("get_error",)),
    "async_with_missing_aexit": ("invalid-context-manager", ("get_result",)),
    "coroutine_async_with_synconly_error": ("invalid-context-manager", ("make_inner",)),
    "asyncio_call_graph_async_gen": ("unresolved-attribute", ("run",)),
    "class_delayed_classcell": ("invalid-argument-type", ("exercise",)),
    "class_scope_unbound_freevar": ("unresolved-reference", ("outer",)),
    "classcell_delete_multiple_targets": ("unresolved-reference", ("exercise",)),
    "generic_io_typing": ("empty-body", ()),
    "pep695_type_params": ("invalid-type-form", ()),
    "reprlib_type_params": ("invalid-parameter-default", ("run",)),
    "typing_generics_cases": (
        "shadowed-type-variable",
        ("inner_class_hint_is_inner", "pep695_generic_info"),
    ),
    "support_import_internalcapi": ("unresolved-import", ("exercise",)),
    "annotationlib_meta": ("call-non-callable", ("run",)),
    "annotationlib_partial_eval_cell": ("unresolved-attribute", ("run",)),
    "class_dictproxy_annotate_func": ("call-non-callable", ("run",)),
}

# Reloading the real stdlib dataclasses module changes shared identities. Its
# ordinary control therefore gets an isolated process, not a cohort batch.
STRICT_ISOLATED_FRAMEWORK_INTEROP_CASES = {
    "frozen_dataclass": ("invalid-assignment", ()),
}

# The parser's string payload cannot represent these values losslessly. Strict
# source fails before publication; ordinary modules preserve the exact CPython
# values when called through a selected strict boundary.
STRICT_SOURCE_LITERAL_INTEROP_CASES = {
    "concat_surrogates": ("unsupported Unicode surrogate escape U+DCA7", ("run",)),
    "fstring_surrogates": ("unsupported Unicode surrogate escape U+D83D", ("run",)),
    "surrogate_unicode_escape_repr": (
        "unsupported Unicode surrogate escape U+DCBA",
        ("repr_value", "ascii_value"),
    ),
}


def _strict_case_source(path: Path) -> str:
    source, _ = split_integration_case(path)
    prepared, _ = strict_opt_in(source.encode("utf-8"), str(path))
    return prepared.decode("utf-8")


def _selected_integration_case_modes(
    items, *, test_path: Path, test_function, case_directory: Path
) -> frozenset[tuple[str, str]]:
    """Read this worker's final pytest selection, never rendered node IDs.

    This is fixture scheduling data, not strict admission. The exact collected
    function/path and parametrized source path identify a requested case. Each
    cohort still uses its reviewed source set, dependencies and native checks.
    """
    test_path = test_path.resolve()
    case_directory = case_directory.resolve()
    selected = set()
    for item in items:
        if (
            Path(item.path).resolve() != test_path
            or getattr(item, "obj", None) is not test_function
        ):
            continue
        callspec = getattr(item, "callspec", None)
        if callspec is None or not {"case_path", "mode"} <= callspec.params.keys():
            raise ValueError("collected integration case is missing its parameters")
        path = callspec.params["case_path"]
        mode = callspec.params["mode"]
        if (
            not isinstance(path, Path)
            or path.suffix != ".py"
            or path.resolve().parent != case_directory
        ):
            raise ValueError("collected integration case has an unexpected source path")
        if mode not in ("stock", "soac", "entry", "cpython"):
            raise ValueError("collected integration case has an unexpected mode")
        if mode != "stock":
            selected.add((path.stem, mode))
    return frozenset(selected)


@pytest.fixture(scope="module")
def strict_selected_case_modes(request):
    # The parallel runner starts pytest with only this worker's node IDs.
    # session.items has also had normal pytest deselection applied by now.
    # No environment variable, process-global cache or node-ID parser is used.
    return _selected_integration_case_modes(
        request.session.items,
        test_path=Path(__file__),
        test_function=test_integration_case,
        case_directory=MODULES_DIR,
    )


def _selected_cohort_cases(reviewed_cases, selected_case_modes):
    """Intersect explicit requests with one reviewed catalog in catalog order."""
    requested = {}
    for mode in ("soac", "entry", "cpython"):
        names = tuple(
            name for name in reviewed_cases if (name, mode) in selected_case_modes
        )
        if names:
            requested[mode] = names
    return requested


def _strict_cohort_results(
    tmp_path_factory,
    cohort,
    reviewed_cases,
    *,
    selected_case_modes,
    analysis_timeout=180,
    dependencies=(),
    isolated=(),
    validators=None,
    retained_validators=None,
    import_validators=None,
):
    requested = _selected_cohort_cases(reviewed_cases, selected_case_modes)
    if not requested:
        return {}
    # Keep analysis/admission inputs unchanged. Only validation execution
    # is narrowed to this worker's requested (case, mode) pairs.
    modules = {name: f"{name}.py" for name in reviewed_cases}
    sources = {
        path: _strict_case_source(MODULES_DIR / path) for path in modules.values()
    }
    sources.update({path: (MODULES_DIR / path).read_text() for path in dependencies})
    cases = {
        name: StrictValidationCase(
            (validators or {}).get(
                name, split_integration_case(MODULES_DIR / modules[name])[1]
            ),
            MODULES_DIR / modules[name],
            required_functions=witnesses,
        )
        for name, witnesses in reviewed_cases.items()
    }
    # These cases do not observe process lifetime or mutate shared interpreter
    # state. Batch only this reviewed cohort, with per-case native admission,
    # validation, contamination guards, and individually reported failures.
    results = {}
    projects = {}
    for mode, names in requested.items():
        backend = "cpython" if mode == "cpython" else "soac"
        if backend not in projects:
            # Backend selection is part of the authenticated environment. Never
            # replay a SOAC publication as interpreter-only authority.
            projects[backend] = create_strict_project(
                tmp_path_factory.mktemp(f"strict-{cohort}-{backend}-cases"),
                sources,
                modules=modules,
                analysis_timeout=analysis_timeout,
                backend=backend,
            )
        project = projects[backend]
        mode_cases = dict(cases)
        if backend == "soac":
            for name, validate_source in (retained_validators or {}).items():
                mode_cases[name] = replace(cases[name], validate_source=validate_source)
        import_checks = import_validators or {}
        assert set(import_checks).issubset(reviewed_cases)
        refused = set(import_checks)
        batch = {
            name: mode_cases[name]
            for name in names
            if name not in isolated and name not in refused
        }
        results[mode] = (
            project.run_cases(batch, entry_interpreter=mode == "entry")
            if batch
            else {}
        )
        for name in names:
            if name not in isolated and name not in refused:
                continue
            case = mode_cases[name]
            try:
                if name in import_checks:
                    project.run(
                        import_checks[name],
                        entry_interpreter=mode == "entry",
                        backend=backend,
                    )
                else:
                    project.run_case(
                        name,
                        case.validate_source,
                        case.module_path,
                        required_functions=case.required_functions,
                        entry_interpreter=mode == "entry",
                    )
            except AssertionError as error:
                results[mode][name] = str(error)
            else:
                results[mode][name] = None
    return results


@pytest.fixture(scope="module")
def strict_basic_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "basic",
        STRICT_BASIC_CASES,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_control_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "control",
        STRICT_CONTROL_CASES,
        selected_case_modes=strict_selected_case_modes,
        retained_validators=_retained_semantic_validators(STRICT_CONTROL_CASES),
    )


@pytest.fixture(scope="module")
def strict_class_lifetime_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "class-lifetime",
        STRICT_CLASS_LIFETIME_CASES,
        selected_case_modes=strict_selected_case_modes,
        retained_validators=_retained_semantic_validators(STRICT_CLASS_LIFETIME_CASES),
    )


@pytest.fixture(scope="module")
def strict_comprehension_protocol_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "comprehension-protocol",
        STRICT_COMPREHENSION_PROTOCOL_CASES,
        selected_case_modes=strict_selected_case_modes,
        retained_validators=_retained_semantic_validators(STRICT_COMPREHENSION_PROTOCOL_CASES),
    )


@pytest.fixture(scope="module")
def strict_class_closure_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "class-closure",
        STRICT_CLASS_CLOSURE_CASES,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_attribute_protocol_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "attribute-protocol",
        STRICT_ATTRIBUTE_PROTOCOL_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_async_generator_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "async-generator",
        STRICT_ASYNC_GENERATOR_CASES,
        selected_case_modes=strict_selected_case_modes,
        retained_validators=_retained_semantic_validators(STRICT_ASYNC_GENERATOR_CASES),
    )


@pytest.fixture(scope="module")
def strict_async_protocol_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "async-protocol",
        STRICT_ASYNC_PROTOCOL_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
        retained_validators=_retained_semantic_validators(STRICT_ASYNC_PROTOCOL_CASES),
    )


@pytest.fixture(scope="module")
def strict_yield_lifetime_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "yield-lifetime",
        STRICT_YIELD_LIFETIME_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
        retained_validators=_retained_semantic_validators(STRICT_YIELD_LIFETIME_CASES),
    )


@pytest.fixture(scope="module")
def strict_declared_global_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "declared-global",
        STRICT_DECLARED_GLOBAL_CASES,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_class_scope_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "class-scope",
        STRICT_CLASS_SCOPE_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_iterator_metadata_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "iterator-metadata",
        STRICT_ITERATOR_METADATA_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
        retained_validators=_retained_semantic_validators(STRICT_ITERATOR_METADATA_CASES),
        import_validators={
            "iter_refcount_behavior": """
import importlib
import sys
from soac.strict import StrictMutationError

# OPT_GOAL/STRICT_MODULES seal participating class dictionaries. The original
# C.__new__ increments C.count before object allocation, so importing this
# exact source must reject that mutation, even in interpreter-only execution.
# The unchanged stock validator remains the positive refcount/finalizer test.
assert 'iter_refcount_behavior' not in sys.modules
try:
    importlib.import_module('iter_refcount_behavior')
except StrictMutationError as error:
    assert type(error) is StrictMutationError
    assert str(error) == 'cannot mutate sealed strict class C'
else:
    raise AssertionError('an immutable strict class counter was mutated')
assert 'iter_refcount_behavior' not in sys.modules
""",
        },
    )


@pytest.fixture(scope="module")
def strict_binding_pattern_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "binding-pattern",
        STRICT_BINDING_PATTERN_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_typing_metadata_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "typing-metadata",
        STRICT_TYPING_METADATA_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_module_protocol_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "module-protocol",
        STRICT_MODULE_PROTOCOL_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_framework_annotation_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "framework-annotation",
        STRICT_FRAMEWORK_ANNOTATION_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_contract_rejection_results(tmp_path_factory, strict_selected_case_modes):
    reviewed = {
        name: witnesses
        for name, (_, witnesses, _) in STRICT_CONTRACT_REJECTION_CASES.items()
    }
    validators = {
        name: (
            "def validate_module(module):\n"
            "    import pytest\n"
            "    from soac.strict import StrictMutationError, StrictRuntimeUnavailableError\n"
            f"    with pytest.raises({error}):\n        {call}\n"
        )
        for name, (error, _, call) in STRICT_CONTRACT_REJECTION_CASES.items()
    }
    # Keep the original negative-input checks before its strict-only diagnostic
    # adaptation. Dynamic code has a separate reviewed authority policy below.
    for name in ("genexpr_iterator_semantics",):
        _, validators[name] = split_integration_case(MODULES_DIR / f"{name}.py")
    # Both strict consumers refuse a new FunctionType without an authenticated
    # activation, before testing whether its argument is iterable. Preserve the
    # original ordinary tail and both negative calls; update only this reviewed
    # strict diagnostic and its backend predicate.
    replay = validators["genexpr_iterator_semantics"]
    previous = "strict code execution requires an authenticated runtime entry"
    current = "strict code execution requires an authenticated interpreter activation"
    assert replay.count(previous) == 2
    assert replay.count("if __dp_integration_soac__:") == 1
    validators["genexpr_iterator_semantics"] = replay.replace(previous, current).replace(
        "if __dp_integration_soac__:", "if __dp_integration_strict__:", 1
    )
    return _strict_cohort_results(
        tmp_path_factory,
        "contract-rejection",
        reviewed,
        validators=validators,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_namespace_semantic_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "namespace-semantic",
        STRICT_NAMESPACE_SEMANTIC_CASES,
        retained_validators=_retained_semantic_validators(STRICT_NAMESPACE_SEMANTIC_CASES),
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_dynamic_code_results(tmp_path_factory, strict_selected_case_modes):
    validators = {}
    for name in STRICT_DYNAMIC_CODE_CASES:
        original_path = MODULES_DIR / f"{name}.py"
        source, ordinary_validation = split_integration_case(original_path)
        # This is an independent ordinary control, not execution authority for
        # a selected strict string/code object. Keep the original tail intact.
        validator = (
            "def validate_module(module):\n"
            "    import pytest\n"
            "    from pathlib import Path\n"
            "    from soac import StrictMutationError, StrictRuntimeUnavailableError\n"
            "    from tests._integration import stock_module, exec_integration_validation\n"
            "    from soac import _soac_ext\n"
            f"    with stock_module(Path({str(tmp_path_factory.mktemp('dynamic-stock-' + name))!r}), "
            f"{('ordinary_' + name)!r}, {source!r}) as ordinary:\n"
            "        assert _soac_ext.strict_module_diagnostics(ordinary) is None\n"
            f"        exec_integration_validation({ordinary_validation!r}, ordinary, "
            f"Path({str(original_path)!r}), mode='stock')\n"
            "    bindings = {name: id(value) for name, value in vars(module).items()}\n"
        )
        if name == "class_scope_nonlocal_syntaxerror":
            validator += (
                "    assert type(ordinary.result) is str and ordinary.result\n"
                "    if __dp_integration_soac__:\n"
                "        assert type(module.result) is NotImplementedError\n"
                "        assert 'authenticated dynamic-code protocol' in str(module.result)\n"
                "        repeated = module.nonlocal_in_class_body_error()\n"
                "        assert type(repeated) is NotImplementedError\n"
                "        assert str(repeated) == str(module.result)\n"
                "    else:\n"
                "        assert module.result == ordinary.result\n"
                "        assert module.nonlocal_in_class_body_error() == ordinary.result\n"
                "    assert 'Bad' not in vars(module) and 'Bad' not in vars(ordinary)\n"
                "    with pytest.raises(StrictMutationError):\n"
                "        module.result = 'replacement'\n"
            )
        elif name == "exec_closure_kw":
            validator += (
                "    with pytest.raises(StrictRuntimeUnavailableError):\n"
                "        module.run()\n"
            )
        else:
            reason = (
                "requires explicit globals"
                if name in {"eval_closure", "exec_locals_kw"}
                else "authenticated dynamic-code protocol"
            )
            validator += (
                f"    with pytest.raises(NotImplementedError, match={reason!r}):\n"
                "        module.run()\n"
            )
        validator += (
            "    assert {name: id(value) for name, value in vars(module).items()} == bindings\n"
        )
        validators[name] = validator
    return _strict_cohort_results(
        tmp_path_factory,
        "dynamic-code",
        STRICT_DYNAMIC_CODE_CASES,
        validators=validators,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_import_lifecycle_results(tmp_path_factory, strict_selected_case_modes):
    return _strict_cohort_results(
        tmp_path_factory,
        "import-lifecycle",
        STRICT_IMPORT_LIFECYCLE_CASES,
        dependencies=STRICT_IMPORT_DEPENDENCIES,
        isolated=("meta_path_pathfinder_preserved",),
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
        retained_validators=_retained_semantic_validators(STRICT_IMPORT_LIFECYCLE_CASES),
    )


def _ordinary_interop_cohort_results(
    tmp_path_factory,
    cohort,
    reviewed_cases,
    *,
    selected_case_modes,
    analysis_timeout=180,
    dependencies=(),
    isolated=False,
):
    requested = _selected_cohort_cases(reviewed_cases, selected_case_modes)
    if not requested:
        return {}
    # Keep every reviewed bridge/source and dependency in analysis;
    # selection never changes checker policy or supplies authority.
    sources = {}
    modules = {}
    cases = {}
    for name, (_, ordinary_functions) in reviewed_cases.items():
        path = MODULES_DIR / f"{name}.py"
        source, validation = split_integration_case(path)
        bridge = f"interop_{name}"
        sources[f"{name}.py"] = source
        sources[f"{bridge}.py"] = (
            "from __future__ import strict\n"
            f"import {name} as ordinary\n"
            "def invoke_validation(callback, source, path):\n"
            "    return callback(source, ordinary, path, mode='stock')\n"
        )
        modules[bridge] = f"{bridge}.py"
        # Run the unchanged validator through a genuinely selected strict
        # caller. Its callback and the original module stay ordinary, with
        # explicit negative ownership checks instead of a claimed fallback.
        validation_bridge = (
            "def validate_module(module):\n"
            "    import ctypes\n"
            "    from pathlib import Path\n"
            "    from soac import _soac_ext\n"
            "    from tests._integration import exec_integration_validation\n"
            "    from tests._strict_integration import _plain_function_witness\n"
            "    assert _soac_ext.strict_module_diagnostics(module.ordinary) is None\n"
            "    owner = ctypes.pythonapi.PyFunction_GetSoacStrictOwner\n"
            "    owner.argtypes = [ctypes.py_object]\n"
            "    owner.restype = ctypes.c_void_p\n"
            f"    for name in {ordinary_functions!r}:\n"
            "        function = _plain_function_witness(module.ordinary, name)\n"
            "        assert owner(function) is None\n"
            "        assert _soac_ext.strict_function_entry_kind(function) is None\n"
            "    module.invoke_validation(\n"
            f"        exec_integration_validation, {validation!r}, Path({str(path)!r})\n"
            "    )\n"
        )
        cases[bridge] = StrictValidationCase(
            validation_bridge, path, required_functions=("invoke_validation",)
        )
    sources.update({path: (MODULES_DIR / path).read_text() for path in dependencies})
    project = create_strict_project(
        tmp_path_factory.mktemp(f"strict-{cohort}-interop-cases"),
        sources,
        modules=modules,
        analysis_timeout=analysis_timeout,
    )
    results = {}
    for mode, names in requested.items():
        selected_cases = {f"interop_{name}": cases[f"interop_{name}"] for name in names}
        if isolated:
            observed = {}
            for name, case in selected_cases.items():
                try:
                    project.run_case(
                        name,
                        case.validate_source,
                        case.module_path,
                        required_functions=case.required_functions,
                        entry_interpreter=mode == "entry",
                    )
                except AssertionError as error:
                    observed[name] = str(error)
                else:
                    observed[name] = None
        else:
            observed = project.run_cases(
                selected_cases, entry_interpreter=mode == "entry"
            )
        results[mode] = {
            name.removeprefix("interop_"): error for name, error in observed.items()
        }
    return results


@pytest.fixture(scope="module")
def strict_control_interop_results(tmp_path_factory, strict_selected_case_modes):
    return _ordinary_interop_cohort_results(
        tmp_path_factory,
        "control",
        STRICT_CONTROL_INTEROP_CASES,
        analysis_timeout=600,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_source_literal_interop_results(tmp_path_factory, strict_selected_case_modes):
    return _ordinary_interop_cohort_results(
        tmp_path_factory,
        "source-literal",
        STRICT_SOURCE_LITERAL_INTEROP_CASES,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_isolated_framework_interop_results(
    tmp_path_factory, strict_selected_case_modes
):
    return _ordinary_interop_cohort_results(
        tmp_path_factory,
        "isolated-framework",
        STRICT_ISOLATED_FRAMEWORK_INTEROP_CASES,
        analysis_timeout=600,
        isolated=True,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_import_interop_results(tmp_path_factory, strict_selected_case_modes):
    return _ordinary_interop_cohort_results(
        tmp_path_factory,
        "import",
        STRICT_IMPORT_INTEROP_CASES,
        analysis_timeout=600,
        dependencies=STRICT_IMPORT_DEPENDENCIES,
        isolated=True,
        selected_case_modes=strict_selected_case_modes,
    )


@pytest.fixture(scope="module")
def strict_class_annotations_mutation_results(
    tmp_path_factory, strict_selected_case_modes
):
    name = "class_annotations_mutation"
    requested = _selected_cohort_cases({name: ()}, strict_selected_case_modes)
    if not requested:
        return {}
    project = create_strict_project(
        tmp_path_factory.mktemp("strict-class-annotation-mutation"),
        {f"{name}.py": _strict_case_source(MODULES_DIR / f"{name}.py")},
        modules={name: f"{name}.py"},
    )
    results = {}
    for mode in requested:
        try:
            project.run(
                "import pytest\nwith pytest.raises(NameError, match='__annotations__'):\n    import class_annotations_mutation\n",
                entry_interpreter=mode == "entry",
            )
        except AssertionError as error:
            results[mode] = {name: str(error)}
        else:
            results[mode] = {name: None}
    return results


@pytest.fixture(scope="module")
def strict_bad_syntax_diagnostic(tmp_path_factory):
    source, _ = split_integration_case(MODULES_DIR / "bad_syntax.py")
    # Deliberately invalid source cannot go through the AST-preserving opt-in
    # helper. It must fail offline, before any authority can be published.
    return assert_strict_source_rejected(
        tmp_path_factory.mktemp("strict-bad-syntax"),
        "from __future__ import strict\n" + source,
        module_name="bad_syntax",
        diagnostic="source is not valid in the selected checker dialect and Python version",
    )


@pytest.mark.parametrize("case_name", STRICT_SOURCE_LITERAL_INTEROP_CASES)
def test_strict_source_literal_case_is_rejected_before_publication(tmp_path, case_name):
    diagnostic, _ = STRICT_SOURCE_LITERAL_INTEROP_CASES[case_name]
    assert_strict_source_rejected(
        tmp_path,
        _strict_case_source(MODULES_DIR / f"{case_name}.py"),
        module_name=case_name,
        diagnostic=diagnostic,
    )


@pytest.mark.parametrize(
    "case_name",
    STRICT_CONTROL_INTEROP_CASES
    | STRICT_ISOLATED_FRAMEWORK_INTEROP_CASES
    | STRICT_IMPORT_INTEROP_CASES,
)
def test_strict_control_ordinary_source_is_rejected(tmp_path, case_name):
    diagnostic, _ = (
        STRICT_CONTROL_INTEROP_CASES
        | STRICT_ISOLATED_FRAMEWORK_INTEROP_CASES
        | STRICT_IMPORT_INTEROP_CASES
    )[case_name]
    source = _strict_case_source(MODULES_DIR / f"{case_name}.py")
    root = tmp_path / case_name
    sources = {f"{case_name}.py": source}
    if case_name in STRICT_IMPORT_INTEROP_CASES:
        sources.update(
            {
                path: (MODULES_DIR / path).read_text()
                for path in STRICT_IMPORT_DEPENDENCIES
            }
        )
    with pytest.raises(AssertionError, match="actual checker rejected fixture"):
        create_strict_project(
            root,
            sources,
            modules={case_name: f"{case_name}.py"},
        )
    # This is the real CLI diagnostic interface, not an exception-text xfail or
    # a runtime admission error being mistaken for the expected behavior.
    errors = (root / "checker.stderr.log").read_text()
    assert f"{case_name}:" in errors
    assert f"CheckerError: {diagnostic}:" in errors
    assert "blocking strict diagnostic: CheckerError" in errors
    assert not (root / "authority" / "deployment.json").exists()


def _case_paths() -> list[Path]:
    cases: list[Path] = []
    for path in sorted(MODULES_DIR.glob("*.py")):
        try:
            if "# diet-python: validate" in path.read_text(encoding="utf-8"):
                cases.append(path)
        except OSError:
            continue
    return cases


def _case_parameters():
    paths = _case_paths()
    parameters = []
    for mode in ("stock", "soac", "entry", "cpython"):
        for path in paths:
            if mode == "cpython" and path.stem not in CPYTHON_INTERPRETER_CASES:
                continue
            marks = []
            if mode in {"soac", "entry"} and path.stem in SOAC_FRAME_INSPECTION_XFAILS:
                marks.append(pytest.mark.xfail(
                    reason=(
                        "SOAC frame inspection is out of scope (2026-08-25 PDT): "
                        + SOAC_FRAME_INSPECTION_XFAILS[path.stem]
                    ),
                    run=False,
                ))
            parameters.append(pytest.param(mode, path, marks=marks, id=f"{mode}-{path.stem}"))
    return parameters


@pytest.mark.integration
@pytest.mark.parametrize("mode,case_path", _case_parameters())
def test_integration_case(
    tmp_path: Path, case_path: Path, mode: str, request: pytest.FixtureRequest
) -> None:
    if mode != "stock":
        if case_path.stem == "bad_syntax":
            request.getfixturevalue("strict_bad_syntax_diagnostic")
            return
        for cohort, reviewed_cases in (
            ("basic", STRICT_BASIC_CASES),
            ("control", STRICT_CONTROL_CASES),
            ("class_lifetime", STRICT_CLASS_LIFETIME_CASES),
            ("comprehension_protocol", STRICT_COMPREHENSION_PROTOCOL_CASES),
            ("class_closure", STRICT_CLASS_CLOSURE_CASES),
            ("attribute_protocol", STRICT_ATTRIBUTE_PROTOCOL_CASES),
            ("async_generator", STRICT_ASYNC_GENERATOR_CASES),
            ("async_protocol", STRICT_ASYNC_PROTOCOL_CASES),
            ("yield_lifetime", STRICT_YIELD_LIFETIME_CASES),
            ("declared_global", STRICT_DECLARED_GLOBAL_CASES),
            ("class_scope", STRICT_CLASS_SCOPE_CASES),
            ("iterator_metadata", STRICT_ITERATOR_METADATA_CASES),
            ("binding_pattern", STRICT_BINDING_PATTERN_CASES),
            ("typing_metadata", STRICT_TYPING_METADATA_CASES),
            ("module_protocol", STRICT_MODULE_PROTOCOL_CASES),
            ("framework_annotation", STRICT_FRAMEWORK_ANNOTATION_CASES),
            ("contract_rejection", STRICT_CONTRACT_REJECTION_CASES),
            ("namespace_semantic", STRICT_NAMESPACE_SEMANTIC_CASES),
            ("dynamic_code", STRICT_DYNAMIC_CODE_CASES),
            ("import_lifecycle", STRICT_IMPORT_LIFECYCLE_CASES),
            ("control_interop", STRICT_CONTROL_INTEROP_CASES),
            ("source_literal_interop", STRICT_SOURCE_LITERAL_INTEROP_CASES),
            ("isolated_framework_interop", STRICT_ISOLATED_FRAMEWORK_INTEROP_CASES),
            ("import_interop", STRICT_IMPORT_INTEROP_CASES),
            ("class_annotations_mutation", {"class_annotations_mutation": ()}),
        ):
            if case_path.stem in reviewed_cases:
                results = request.getfixturevalue(f"strict_{cohort}_results")
                error = results[mode][case_path.stem]
                assert error is None, (
                    f"{case_path.stem} failed through strict {mode}:\n{error}"
                )
                return
    if case_path.stem == "multiprocessing_barrier_abort_reset":
        # Spawn-mode multiprocessing pickling cannot currently rediscover the
        # helper target function under the generated integration module name.
        pytest.xfail("spawn-mode multiprocessing helper pickling is not yet stable")
    if mode != "stock":
        pytest.fail(
            f"{case_path.stem} needs an explicitly reviewed strict admission or ordinary-interoperability decision"
        )
    source, validate_source = split_integration_case(case_path)
    module_name = case_path.stem

    sys.path.insert(0, str(MODULES_DIR))
    try:
        if case_path.stem == "bad_syntax":
            with (
                pytest.raises(SyntaxError),
                integration_module(tmp_path, module_name, source, mode=mode),
            ):
                pass
            return
        if case_path.stem == "class_annotations_mutation":
            with (
                pytest.raises(NameError),
                integration_module(tmp_path, module_name, source, mode=mode),
            ):
                pass
            return
        with integration_module(tmp_path, module_name, source, mode=mode) as module:
            exec_integration_validation(validate_source, module, case_path, mode=mode)
    finally:
        if str(MODULES_DIR) in sys.path:
            sys.path.remove(str(MODULES_DIR))
