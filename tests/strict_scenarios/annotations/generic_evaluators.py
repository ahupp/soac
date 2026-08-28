# modes:soac,entry
# module:generic_subject
# soac: module(strict_assign=true, checked_attr=true)

import typing

T = str
type Forward[T: Later = Later] = list[T]
type Constrained[T: (First, Second) = First] = tuple[T]
type CallableAlias[**P = [int, str]] = typing.Callable[P, int]

def repeated():
    type Piece[T] = list[T]
    return Piece

class Later:
    pass

class First:
    pass

class Second:
    pass
# module:generic_control
import typing

T = str
type Forward[T: Later = Later] = list[T]
type Constrained[T: (First, Second) = First] = tuple[T]
type CallableAlias[**P = [int, str]] = typing.Callable[P, int]

def repeated():
    type Piece[T] = list[T]
    return Piece

class Later:
    pass

class First:
    pass

class Second:
    pass
# module:generic_scope_subject
# soac: module(strict_assign=true, checked_attr=true)

T = str

def identity[T: Later = Later](value: T) -> T:
    return value

def default_from_outer[T](value=T):
    return value

class Generic[T: Later = Later]:
    item: T

class Later:
    pass
# module:generic_scope_control
T = str

def identity[T: Later = Later](value: T) -> T:
    return value

def default_from_outer[T](value=T):
    return value

class Generic[T: Later = Later]:
    item: T

class Later:
    pass
# module:generic_pack_subject
# soac: module(strict_assign=true, checked_attr=true)
type TupleAlias[*Ts = *tuple[int, str]] = tuple[*Ts]
# module:generic_pack_control
type TupleAlias[*Ts = *tuple[int, str]] = tuple[*Ts]
# module:generic_context_subject
# soac: module(strict_assign=true, checked_attr=true)

T = bytes

class Holder:
    class Local:
        pass

    def method[T](self, value: Local) -> Local:
        return value

    def identity[T](self, value: T) -> T:
        return value

    def from_global(self, value: T) -> T:
        return value
# module:generic_context_control
T = bytes

class Holder:
    class Local:
        pass

    def method[T](self, value: Local) -> Local:
        return value

    def identity[T](self, value: T) -> T:
        return value

    def from_global(self, value: T) -> T:
        return value
# ok
# test_generic_alias_bounds_constraints_and_defaults_are_lazy_native_evaluators [default]
import sys
from soac import _soac_ext
import annotationlib, inspect, types, typing
import generic_control as control
import generic_subject as subject

assert subject.T is control.T is str
assert 'P' not in vars(subject) and 'Ts' not in vars(subject)
bound, = subject.Forward.__type_params__
assert bound.__bound__ is subject.Later
assert bound.__default__ is subject.Later
assert subject.Forward.__value__ == list[bound]
constrained, = subject.Constrained.__type_params__
assert constrained.__constraints__ == (subject.First, subject.Second)
assert constrained.__default__ is subject.First
assert subject.Constrained.__value__ == tuple[constrained]

for name in ['Forward', 'Constrained', 'CallableAlias']:
    alias, ordinary = getattr(subject, name), getattr(control, name)
    parameter, = alias.__type_params__
    baseline, = ordinary.__type_params__
    for attribute in ['evaluate_bound', 'evaluate_constraints', 'evaluate_default']:
        evaluator = getattr(parameter, attribute, None)
        previous = getattr(baseline, attribute, None)
        if evaluator is None:
            assert previous is None
            continue
        assert isinstance(evaluator, types.FunctionType)
        for provider in (evaluator, previous):
            code = provider.__code__
            assert (code.co_argcount, code.co_posonlyargcount, code.co_kwonlyargcount) == (1, 1, 0)
            assert code.co_varnames[0] == '.format'
        assert evaluator.__defaults__ == previous.__defaults__ == (1,)
        assert evaluator.__code__.co_flags & 0x10000000
        assert annotationlib.call_evaluate_function(
            evaluator, annotationlib.Format.STRING, owner=parameter
        ) == annotationlib.call_evaluate_function(
            previous, annotationlib.Format.STRING, owner=baseline
        )
    assert parameter.__default__ is parameter.__default__
paramspec, = subject.CallableAlias.__type_params__
assert paramspec.__default__ == [int, str]
print('verified-generic-lazy-type-evaluators')
# ok
# test_generic_function_and_class_providers_keep_the_parameter_scope_private [default]
import sys
from soac import _soac_ext
import annotationlib
import generic_scope_control as control
import generic_scope_subject as subject

assert subject.T is control.T is str
assert subject.default_from_outer() is control.default_from_outer() is str
parameter, = subject.identity.__type_params__
assert parameter.__bound__ is subject.Later
assert parameter.__default__ is subject.Later
annotations = annotationlib.get_annotations(subject.identity)
assert annotations == {'value': parameter, 'return': parameter}
value = subject.Later()
assert subject.identity(value) is value
class_parameter, = subject.Generic.__type_params__
assert class_parameter is not parameter
assert class_parameter.__bound__ is subject.Later
assert class_parameter.__default__ is subject.Later
assert 'T' not in vars(subject.Generic)
assert annotationlib.get_annotations(subject.Generic) == {'item': class_parameter}
assert annotationlib.get_annotations(
    subject.Generic, format=annotationlib.Format.STRING
) == annotationlib.get_annotations(control.Generic, format=annotationlib.Format.STRING)
print('verified-private-generic-parameter-scope')
# ok
# test_generic_typevartuple_starred_default_uses_the_native_single_unpack [default]
import sys
from soac import _soac_ext
import annotationlib, types
import generic_pack_control as control
import generic_pack_subject as subject

pack, = subject.TupleAlias.__type_params__
previous, = control.TupleAlias.__type_params__
assert 'Ts' not in vars(subject)
evaluator = pack.evaluate_default
assert isinstance(evaluator, types.FunctionType)
assert evaluator.__code__.co_varnames[0] == '.format'
assert evaluator.__defaults__ == previous.evaluate_default.__defaults__ == (1,)
assert evaluator.__code__.co_flags & 0x10000000
assert pack.__default__ == previous.__default__
assert pack.__default__ is pack.__default__
assert annotationlib.call_evaluate_function(
    evaluator, annotationlib.Format.STRING, owner=pack
) == annotationlib.call_evaluate_function(
    previous.evaluate_default, annotationlib.Format.STRING, owner=previous
)
assert subject.TupleAlias.__value__ == tuple[*pack]
print('verified-starred-type-parameter-default')
# ok
# test_provenance_only_generic_scopes_and_evaluators_do_not_grow_pending_targets [default]
import sys
from soac import _soac_ext
import annotationlib, gc, weakref
import generic_subject as subject

def pending_edges():
    # Observe the dictionary's GC-visible owner graph, without using
    # its metadata or Python attributes as execution authority.
    return sum(isinstance(edge, weakref.ReferenceType)
               for owner in gc.get_referents(vars(subject))
               for edge in gc.get_referents(owner))

initial = pending_edges()
for _ in range(200):
    alias = subject.repeated()
    parameter, = alias.__type_params__
    assert alias.__value__ == list[parameter]
    assert annotationlib.call_evaluate_function(
        alias.evaluate_value, annotationlib.Format.STRING, owner=alias
    ) == 'list[T]'
    del alias, parameter
gc.collect()
assert pending_edges() == initial
print('verified-bounded-provenance-only-type-factories')
# ok
# test_generic_method_providers_inherit_only_the_active_class_execution [default]
import sys
from soac import _soac_ext
import annotationlib
import generic_context_subject as subject

method = subject.Holder.method
parameter, = method.__type_params__
assert parameter.__name__ == 'T'
assert annotationlib.get_annotations(method) == {
    'value': subject.Holder.Local, 'return': subject.Holder.Local,
}
own_parameter, = subject.Holder.identity.__type_params__
assert own_parameter is not parameter
assert annotationlib.get_annotations(subject.Holder.identity) == {
    'value': own_parameter, 'return': own_parameter,
}
# A sibling's same-named type parameter is not a lexical binding for
# this nongeneric method. Lookup must use the actual global alias.
assert subject.T is bytes
assert annotationlib.get_annotations(subject.Holder.from_global) == {
    'value': bytes, 'return': bytes,
}
instance = subject.Holder()
arbitrary = object()
assert instance.identity(arbitrary) is arbitrary
assert instance.from_global(b'value') == b'value'
local = subject.Holder.Local()
assert instance.method(local) is local
assert instance.method(arbitrary) is arbitrary
print('verified-generic-active-class-execution')
