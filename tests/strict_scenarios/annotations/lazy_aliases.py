# modes:soac,entry
# module:alias_subject
# soac: module(strict_assign=true, checked_attr=true)

type Forward = list[Later]
type Recursive = tuple[int, Recursive]

class Later:
    pass
# module:alias_control
type Forward = list[Later]
type Recursive = tuple[int, Recursive]

class Later:
    pass
# ok
# test_type_alias_values_are_lazy_and_use_native_evaluator_replay [default]
import sys
from soac import _soac_ext
import annotationlib, inspect, marshal, types
import alias_control as control
import alias_subject as subject

assert subject.Forward.__value__ == list[subject.Later]
assert subject.Recursive.__value__ == tuple[int, subject.Recursive]
assert subject.Forward.__value__ is subject.Forward.__value__
for candidate, ordinary in [(subject.Forward, control.Forward),
                            (subject.Recursive, control.Recursive)]:
    evaluator = candidate.evaluate_value
    baseline = ordinary.evaluate_value
    signature_errors = []
    for provider in (evaluator, baseline):
        code = provider.__code__
        assert (code.co_argcount, code.co_posonlyargcount, code.co_kwonlyargcount) == (1, 1, 0)
        assert code.co_varnames[0] == '.format'
        try:
            inspect.signature(provider)
        except ValueError as error:
            signature_errors.append(error.args)
        else:
            raise AssertionError('native type evaluator has no valid inspect parameter name')
    assert signature_errors[0] == signature_errors[1]
    assert evaluator.__defaults__ == baseline.__defaults__ == (1,)
    assert evaluator.__code__.co_freevars == baseline.__code__.co_freevars == ()
    assert evaluator.__closure__ is baseline.__closure__ is None
    assert evaluator.__code__.co_flags & 0x10000000
    assert annotationlib.call_evaluate_function(
        evaluator, annotationlib.Format.STRING, owner=candidate
    ) == annotationlib.call_evaluate_function(
        baseline, annotationlib.Format.STRING, owner=ordinary
    )
    assert annotationlib.call_evaluate_function(
        evaluator, annotationlib.Format.FORWARDREF, owner=None
    ) == candidate.__value__
    for code in [evaluator.__code__, evaluator.__code__.replace(),
                 marshal.loads(marshal.dumps(evaluator.__code__))]:
        forged = types.FunctionType(code, evaluator.__globals__, argdefs=(1,))
        try:
            forged()
        except ImportError as error:
            assert "strict code execution" in str(error)
        else:
            raise AssertionError("copied strict evaluator bytecode executed natively")
print("verified-lazy-type-alias-evaluators")
