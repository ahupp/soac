# modes:soac,entry
# module:annotation_cell_subject
# soac: module(strict_assign=true, checked_attr=true)

def build():
    class Alias:
        pass

    class Shadow:
        locals()['Alias'] = bytes
        value: Alias

        def method(self, value: Alias) -> Alias:
            return value

    class Fallback:
        value: Alias

        def method(self, value: Alias) -> Alias:
            return value

    return Shadow, Fallback, Alias
# module:annotation_cell_control
def build():
    class Alias:
        pass

    class Shadow:
        locals()['Alias'] = bytes
        value: Alias

        def method(self, value: Alias) -> Alias:
            return value

    class Fallback:
        value: Alias

        def method(self, value: Alias) -> Alias:
            return value

    return Shadow, Fallback, Alias
# module:annotation_cell_observer
def observe(build):
    shadow, fallback, outer = build()
    fallback_cells = []
    rows = []
    for cls, expected, has_shadow in (
        (shadow, bytes, True),
        (fallback, outer, False),
    ):
        class_provider = cls.__annotate__
        method_provider = cls.method.__annotate__
        class_cells = dict(zip(
            class_provider.__code__.co_freevars,
            class_provider.__closure__,
        ))
        method_cells = dict(zip(
            method_provider.__code__.co_freevars,
            method_provider.__closure__,
        ))
        fallback_cells.extend((class_cells['Alias'], method_cells['Alias']))
        namespace_cell = class_cells['__classdict__']
        namespace = namespace_cell.cell_contents
        class_values = class_provider(1)
        method_values = method_provider(1)
        rows.append({
            'class_value': class_values['value'] is expected,
            'method_value': method_values['value'] is expected,
            'method_return': method_values['return'] is expected,
            'class_dictionary': (
                ('Alias' in namespace) == has_shadow
                and namespace.get('Alias', outer) is expected
            ),
            'shared_dictionary_cell': (
                method_cells['__classdict__'] is namespace_cell
            ),
        })
    return {
        'rows': rows,
        'shared_outer_cell': all(
            cell is fallback_cells[0] for cell in fallback_cells
        ),
        'outer_cell_contents': all(
            cell.cell_contents is outer for cell in fallback_cells
        ),
    }
# ok
# test_annotation_dictionary_cell_fallback_matches_native [default]
import sys
from soac import _soac_ext
import annotation_cell_control as control
import annotation_cell_subject as subject
from annotation_cell_observer import observe

assert _soac_ext.strict_function_entry_kind(control.build) is None
expected = observe(control.build)
assert expected == {'rows': [{'class_value': True, 'method_value': True, 'method_return': True, 'class_dictionary': True, 'shared_dictionary_cell': True}, {'class_value': True, 'method_value': True, 'method_return': True, 'class_dictionary': True, 'shared_dictionary_cell': True}], 'shared_outer_cell': True, 'outer_cell_contents': True}, expected
assert _soac_ext.strict_module_diagnostics(subject)['sealed']
expected_entry = ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')
assert _soac_ext.strict_function_entry_kind(subject.build) == expected_entry
actual = observe(subject.build)
assert actual == expected, (actual, expected)
# ok
# test_annotation_dictionary_cell_fallback_native [default]
import sys
from soac import _soac_ext
_ANNOTATION_CELL_SHADOW_EXPECTED = {'rows': [{'class_value': True, 'method_value': True, 'method_return': True, 'class_dictionary': True, 'shared_dictionary_cell': True}, {'class_value': True, 'method_value': True, 'method_return': True, 'class_dictionary': True, 'shared_dictionary_cell': True}], 'shared_outer_cell': True, 'outer_cell_contents': True}
_ANNOTATION_CELL_SHADOW_OBSERVER = "\ndef observe(build):\n    shadow, fallback, outer = build()\n    fallback_cells = []\n    rows = []\n    for cls, expected, has_shadow in (\n        (shadow, bytes, True),\n        (fallback, outer, False),\n    ):\n        class_provider = cls.__annotate__\n        method_provider = cls.method.__annotate__\n        class_cells = dict(zip(\n            class_provider.__code__.co_freevars,\n            class_provider.__closure__,\n        ))\n        method_cells = dict(zip(\n            method_provider.__code__.co_freevars,\n            method_provider.__closure__,\n        ))\n        fallback_cells.extend((class_cells['Alias'], method_cells['Alias']))\n        namespace_cell = class_cells['__classdict__']\n        namespace = namespace_cell.cell_contents\n        class_values = class_provider(1)\n        method_values = method_provider(1)\n        rows.append({\n            'class_value': class_values['value'] is expected,\n            'method_value': method_values['value'] is expected,\n            'method_return': method_values['return'] is expected,\n            'class_dictionary': (\n                ('Alias' in namespace) == has_shadow\n                and namespace.get('Alias', outer) is expected\n            ),\n            'shared_dictionary_cell': (\n                method_cells['__classdict__'] is namespace_cell\n            ),\n        })\n    return {\n        'rows': rows,\n        'shared_outer_cell': all(\n            cell is fallback_cells[0] for cell in fallback_cells\n        ),\n        'outer_cell_contents': all(\n            cell.cell_contents is outer for cell in fallback_cells\n        ),\n    }\n"
_ANNOTATION_CELL_SHADOW_SOURCE = "\ndef build():\n    class Alias:\n        pass\n\n    class Shadow:\n        locals()['Alias'] = bytes\n        value: Alias\n\n        def method(self, value: Alias) -> Alias:\n            return value\n\n    class Fallback:\n        value: Alias\n\n        def method(self, value: Alias) -> Alias:\n            return value\n\n    return Shadow, Fallback, Alias\n"
namespace = {"__name__": "annotation_cell_native"}
observer = {}
exec(  # noqa: S102 - compile the fixed fixture with the native interpreter
    compile(_ANNOTATION_CELL_SHADOW_SOURCE, "<annotation-cell-native>", "exec", dont_inherit=True),
    namespace,
)
exec(  # noqa: S102 - the observer is a fixed, non-transformed fixture
    compile(_ANNOTATION_CELL_SHADOW_OBSERVER, "<annotation-cell-observer>", "exec", dont_inherit=True),
    observer,
)
assert observer["observe"](namespace["build"]) == _ANNOTATION_CELL_SHADOW_EXPECTED
