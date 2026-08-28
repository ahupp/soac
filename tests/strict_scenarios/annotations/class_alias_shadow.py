# modes:soac,entry
# module:class_alias_subject
# soac: module(strict_assign=true, checked_attr=true)

def build():
    class Alias:
        pass

    class Shadow:
        Alias = bytes
        type Selected = Alias

    class Fallback:
        type Selected = Alias

    return Shadow, Fallback, Alias
# module:class_alias_control
def build():
    class Alias:
        pass

    class Shadow:
        Alias = bytes
        type Selected = Alias

    class Fallback:
        type Selected = Alias

    return Shadow, Fallback, Alias
# ok
# test_class_type_alias_shadow_matches_native [default]
import sys
from soac import _soac_ext
import class_alias_control as control
import class_alias_subject as subject

assert _soac_ext.strict_function_entry_kind(control.build) is None
assert _soac_ext.strict_module_diagnostics(subject)['sealed']
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
assert _soac_ext.strict_function_entry_kind(subject.build) == expected_entry
for build in (control.build, subject.build):
    shadow, fallback, outer = build()
    assert shadow.Selected.__value__ is bytes
    assert fallback.Selected.__value__ is outer
# ok
# test_class_type_alias_uses_native_dictionary_and_cell_fallbacks [default]
import sys
from soac import _soac_ext
_CLASS_ALIAS_SHADOW_SOURCE = '\ndef build():\n    class Alias:\n        pass\n\n    class Shadow:\n        Alias = bytes\n        type Selected = Alias\n\n    class Fallback:\n        type Selected = Alias\n\n    return Shadow, Fallback, Alias\n'
namespace = {"__name__": "class_alias_native"}
exec(  # noqa: S102 - compile the fixed fixture with the native interpreter
    compile(_CLASS_ALIAS_SHADOW_SOURCE, "<class-alias-native>", "exec", dont_inherit=True),
    namespace,
)
shadow, fallback, outer = namespace["build"]()
assert shadow.Selected.__value__ is bytes
assert fallback.Selected.__value__ is outer
