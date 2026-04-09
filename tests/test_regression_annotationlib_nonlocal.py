import annotationlib

def test_forwardref_nonlocal_annotation_scope(run_integration_module):
    with run_integration_module("annotationlib_nonlocal_scope") as module:
        ok_types, x_val, y_val = module.run()
        assert ok_types is True
        assert isinstance(x_val, annotationlib.ForwardRef)
        assert x_val.__forward_arg__ == "sequence_b"
        assert isinstance(y_val, annotationlib.ForwardRef)
        assert y_val.__forward_arg__.startswith("sequence_b[")


def test_forwardref_partial_evaluation_cell(run_integration_module):
    with run_integration_module("annotationlib_partial_eval_cell") as module:
        value = module.run()
        assert isinstance(value, annotationlib.ForwardRef)
        assert value.__forward_arg__ == "obj.missing"
