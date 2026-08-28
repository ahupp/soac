# modes:soac,entry
# module:annotation_members
# soac: module(strict_assign=true, checked_attr=true)
from annotation_member_support import install

class Unrelated:
    install(locals(), False)
    def method(self) -> int:
        return 7

class FakeParameters:
    install(locals(), True)
    def method(self) -> int:
        return 8
# module:annotation_member_support
events = []

class Trap:
    def __getattribute__(self, name):
        events.append(('getattr', name))
        raise AssertionError('annotation lookup inspected an unrelated member')
    def __iter__(self):
        events.append(('iter',))
        raise AssertionError('annotation lookup iterated an unrelated member')

def install(namespace, parameters):
    namespace['__type_params__' if parameters else 'unrelated'] = Trap()
# ok
# test_class_annotation_lookup_never_scans_unrelated_members_for_type_parameters [default]
import sys
from soac import _soac_ext
import annotationlib
import annotation_members as module
from annotation_member_support import events

assert _soac_ext.strict_module_diagnostics(module)['sealed']
for cls in (module.Unrelated, module.FakeParameters):
    for format in (annotationlib.Format.VALUE, annotationlib.Format.FORWARDREF):
        assert annotationlib.get_annotations(cls.method, format=format) == {'return': int}
assert events == [], events
