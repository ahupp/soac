# modes:soac,entry,cpython
# module:pydantic_model
# soac: module(strict_assign=true, checked_attr=true)
from functools import cached_property
from pydantic import BaseModel, ConfigDict, Field, PrivateAttr, computed_field, field_validator

EVENTS = []

class Parent:
    def value(self):
        return -1

class Record(BaseModel, Parent):
    model_config = ConfigDict(validate_assignment=True, extra='allow', defer_build=True)
    value: int
    child: 'Child | None' = None
    legacy: int = Field(default=4, deprecated='use value instead')
    _hits: int = PrivateAttr(default=0)

    @field_validator('value', mode='before')
    @classmethod
    def coerce(cls, value: int) -> int:
        EVENTS.append(('validate', type(value).__name__, value))
        return int(value)

    @cached_property
    def doubled(self) -> int:
        self._hits += 1
        return self.value * 2

    @computed_field
    @property
    def tripled(self) -> int:
        return self.value * 3

    def echo(self, value: int) -> int:
        return value

class Child(BaseModel):
    count: int
# ok
# test_real_framework_fallback_matches_ordinary_python [pydantic]
import sys
from soac import _soac_ext
import json
import os
import subprocess
# The ordinary framework registry/database must be isolated from the selected
# model, as in the original two-subprocess comparison. This child uses the
# same interpreter, environment and actual installed dependency paths.
ordinary_source = 'import ctypes, importlib, json, sys, types, warnings\nmodel = types.ModuleType(\'pydantic_model\')\nsys.modules[\'pydantic_model\'] = model\nexec(compile("from functools import cached_property\\nfrom pydantic import BaseModel, ConfigDict, Field, PrivateAttr, computed_field, field_validator\\n\\nEVENTS = []\\n\\nclass Parent:\\n    def value(self):\\n        return -1\\n\\nclass Record(BaseModel, Parent):\\n    model_config = ConfigDict(validate_assignment=True, extra=\'allow\', defer_build=True)\\n    value: int\\n    child: \'Child | None\' = None\\n    legacy: int = Field(default=4, deprecated=\'use value instead\')\\n    _hits: int = PrivateAttr(default=0)\\n\\n    @field_validator(\'value\', mode=\'before\')\\n    @classmethod\\n    def coerce(cls, value: int) -> int:\\n        EVENTS.append((\'validate\', type(value).__name__, value))\\n        return int(value)\\n\\n    @cached_property\\n    def doubled(self) -> int:\\n        self._hits += 1\\n        return self.value * 2\\n\\n    @computed_field\\n    @property\\n    def tripled(self) -> int:\\n        return self.value * 3\\n\\n    def echo(self, value: int) -> int:\\n        return value\\n\\nclass Child(BaseModel):\\n    count: int\\n", \'<ordinary framework control>\', \'exec\'), vars(model))\nassert _soac_ext.strict_module_diagnostics(model) is None\n\nowner = ctypes.pythonapi.PyType_GetSoacContractOwner\nowner.argtypes = [ctypes.py_object]\nowner.restype = ctypes.c_void_p\nassert owner(model.Record) is None, \'framework class acquired a strict policy\'\nmetadata = ctypes.pythonapi.PyFunction_GetSoacMetadata\nmetadata.argtypes = [ctypes.py_object]\nmetadata.restype = ctypes.c_void_p\nassert metadata(model.Record.echo) is None\n\nassert model.Record.__pydantic_complete__ is False\nassert model.Record.model_rebuild() is True\nrecord = model.Record(value=\'2\', child={\'count\': \'3\'}, extra=\'kept\')\nassert record.value == 2 and not callable(record.value)\nassert record.child.count == 3\nassert record.extra == \'kept\'\nassert record.doubled == 4 and record.doubled == 4 and record._hits == 1\nrecord.value = \'5\'\nassert record.value == 5\nafter_assignment = (record.doubled, record._hits)\nrecord.doubled = 99\nassert record.doubled == 99\ndel record.doubled\nassert record.doubled == 10\nwith warnings.catch_warnings(record=True) as deprecated:\n    warnings.simplefilter(\'always\')\n    assert record.legacy == 4\nassert [str(item.message) for item in deprecated] == [\'use value instead\']\nrecord.__dict__ = dict(vars(record), value=7)\nassert record.value == 7\ndump = record.model_dump()\nassert dump[\'tripled\'] == 21 and dump[\'child\'] == {\'count\': 3}\nresult = {\n    \'dump\': dump,\n    \'hits\': record._hits,\n    \'cache_after_assignment\': after_assignment,\n    \'fields_set\': sorted(record.model_fields_set),\n    \'validation\': model.EVENTS,\n    \'deprecated\': [str(item.message) for item in deprecated],\n}\n\nfor _ in range(128):\n    assert record.echo(\'ordinary argument\') == \'ordinary argument\'\ncall = ctypes.pythonapi.PyObject_CallOneArg\ncall.argtypes = [ctypes.py_object, ctypes.py_object]\ncall.restype = ctypes.py_object\nassert call(record.echo, \'ordinary C argument\') == \'ordinary C argument\'\ndef replacement(self, value):\n    return (\'replaced\', value)\nmodel.Record.echo.__code__ = replacement.__code__\nassert record.echo(\'after mutation\') == (\'replaced\', \'after mutation\')\nmodel.Record.extra_method = lambda self: 19\nassert record.extra_method() == 19\nresult[\'echo\'] = record.echo(\'after mutation\')\nprint(json.dumps(result, sort_keys=True))\n' if __dp_integration_mode__ == 'cpython' else 'import ctypes, importlib, json, sys, types, warnings\nmodel = types.ModuleType(\'pydantic_model\')\nsys.modules[\'pydantic_model\'] = model\nexec(compile("from functools import cached_property\\nfrom pydantic import BaseModel, ConfigDict, Field, PrivateAttr, computed_field, field_validator\\n\\nEVENTS = []\\n\\nclass Parent:\\n    def value(self):\\n        return -1\\n\\nclass Record(BaseModel, Parent):\\n    model_config = ConfigDict(validate_assignment=True, extra=\'allow\', defer_build=True)\\n    value: int\\n    child: \'Child | None\' = None\\n    legacy: int = Field(default=4, deprecated=\'use value instead\')\\n    _hits: int = PrivateAttr(default=0)\\n\\n    @field_validator(\'value\', mode=\'before\')\\n    @classmethod\\n    def coerce(cls, value: int) -> int:\\n        EVENTS.append((\'validate\', type(value).__name__, value))\\n        return int(value)\\n\\n    @cached_property\\n    def doubled(self) -> int:\\n        self._hits += 1\\n        return self.value * 2\\n\\n    @computed_field\\n    @property\\n    def tripled(self) -> int:\\n        return self.value * 3\\n\\n    def echo(self, value: int) -> int:\\n        return value\\n\\nclass Child(BaseModel):\\n    count: int\\n", \'<ordinary framework control>\', \'exec\'), vars(model))\nassert _soac_ext.strict_module_diagnostics(model) is None\n\nowner = ctypes.pythonapi.PyType_GetSoacContractOwner\nowner.argtypes = [ctypes.py_object]\nowner.restype = ctypes.c_void_p\nassert owner(model.Record) is None, \'framework class acquired a strict type policy\'\n\nassert model.Record.__pydantic_complete__ is False\nassert model.Record.model_rebuild() is True\nrecord = model.Record(value=\'2\', child={\'count\': \'3\'}, extra=\'kept\')\nassert record.value == 2 and not callable(record.value)\nassert record.child.count == 3\nassert record.extra == \'kept\'\nassert record.doubled == 4 and record.doubled == 4 and record._hits == 1\nrecord.value = \'5\'\nassert record.value == 5\nafter_assignment = (record.doubled, record._hits)\nrecord.doubled = 99\nassert record.doubled == 99\ndel record.doubled\nassert record.doubled == 10\nwith warnings.catch_warnings(record=True) as deprecated:\n    warnings.simplefilter(\'always\')\n    assert record.legacy == 4\nassert [str(item.message) for item in deprecated] == [\'use value instead\']\nrecord.__dict__ = dict(vars(record), value=7)\nassert record.value == 7\ndump = record.model_dump()\nassert dump[\'tripled\'] == 21 and dump[\'child\'] == {\'count\': 3}\nresult = {\n    \'dump\': dump,\n    \'hits\': record._hits,\n    \'cache_after_assignment\': after_assignment,\n    \'fields_set\': sorted(record.model_fields_set),\n    \'validation\': model.EVENTS,\n    \'deprecated\': [str(item.message) for item in deprecated],\n}\n\nassert record.echo(\'ordinary argument\') == \'ordinary argument\'\ndef replacement(self, value):\n    return (\'replaced\', value)\nmodel.Record.echo.__code__ = replacement.__code__\nassert record.echo(\'after mutation\') == (\'replaced\', \'after mutation\')\nmodel.Record.extra_method = lambda self: 19\nassert record.extra_method() == 19\nresult[\'echo\'] = record.echo(\'after mutation\')\nprint(json.dumps(result, sort_keys=True))\n'
bootstrap = ('import sys\nsys.path[:0] = ' + repr(sys.path) + '\n'
             'from soac import _soac_ext\n')
completed = subprocess.run(
    [sys.executable, '-I', '-B', '-c', bootstrap + ordinary_source],
    cwd=os.getcwd(), env=os.environ.copy(), capture_output=True, text=True, timeout=90,
)
assert completed.returncode == 0, completed.stdout + completed.stderr
ordinary_result = json.loads(completed.stdout.splitlines()[-1])

if __dp_integration_mode__ == 'cpython':
    import ctypes, importlib, json, sys, types, warnings
    model = importlib.import_module('pydantic_model')
    sys.path.insert(0, str(__import__('tests._strict_integration', fromlist=['ROOT']).ROOT))
    from tests._strict_integration import _assert_cpython_module_witness
    _assert_cpython_module_witness(
        model, module_name='pydantic_model', source_path=str(__import__('pathlib').Path(sys.modules['pydantic_model'].__file__)),
        source_sha256='6c5e2b8b77eee0952e515814899cad9080c8b0cea0d464c09bda66e68f216500',
        artifact_generation=_soac_ext.strict_module_diagnostics(sys.modules['pydantic_model'])['artifact_generation'],
    )

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(model.Record) is None, 'framework class acquired a strict policy'
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert metadata(model.Record.echo) is None

    assert model.Record.__pydantic_complete__ is False
    assert model.Record.model_rebuild() is True
    record = model.Record(value='2', child={'count': '3'}, extra='kept')
    assert record.value == 2 and not callable(record.value)
    assert record.child.count == 3
    assert record.extra == 'kept'
    assert record.doubled == 4 and record.doubled == 4 and record._hits == 1
    record.value = '5'
    assert record.value == 5
    after_assignment = (record.doubled, record._hits)
    record.doubled = 99
    assert record.doubled == 99
    del record.doubled
    assert record.doubled == 10
    with warnings.catch_warnings(record=True) as deprecated:
        warnings.simplefilter('always')
        assert record.legacy == 4
    assert [str(item.message) for item in deprecated] == ['use value instead']
    record.__dict__ = dict(vars(record), value=7)
    assert record.value == 7
    dump = record.model_dump()
    assert dump['tripled'] == 21 and dump['child'] == {'count': 3}
    result = {
        'dump': dump,
        'hits': record._hits,
        'cache_after_assignment': after_assignment,
        'fields_set': sorted(record.model_fields_set),
        'validation': model.EVENTS,
        'deprecated': [str(item.message) for item in deprecated],
    }

    for _ in range(128):
        assert record.echo('ordinary argument') == 'ordinary argument'
    call = ctypes.pythonapi.PyObject_CallOneArg
    call.argtypes = [ctypes.py_object, ctypes.py_object]
    call.restype = ctypes.py_object
    assert call(record.echo, 'ordinary C argument') == 'ordinary C argument'
    def replacement(self, value):
        return ('replaced', value)
    model.Record.echo.__code__ = replacement.__code__
    assert record.echo('after mutation') == ('replaced', 'after mutation')
    model.Record.extra_method = lambda self: 19
    assert record.extra_method() == 19
    result['echo'] = record.echo('after mutation')
    print(json.dumps(result, sort_keys=True))

    from soac.strict import StrictMutationError

    namespace = vars(model)
    original_record = model.Record
    set_attr = ctypes.pythonapi.PyObject_SetAttrString
    set_attr.argtypes = [ctypes.py_object, ctypes.c_char_p, ctypes.py_object]
    set_attr.restype = ctypes.c_int
    set_item = ctypes.pythonapi.PyDict_SetItemString
    set_item.argtypes = [ctypes.py_object, ctypes.c_char_p, ctypes.py_object]
    set_item.restype = ctypes.c_int
    replacement_record = object()
    for mutation in (
        lambda: setattr(model, "Record", replacement_record),
        lambda: delattr(model, "Record"),
        lambda: namespace.__setitem__("Record", replacement_record),
        lambda: namespace.__delitem__("Record"),
        lambda: set_attr(model, b"Record", replacement_record),
        lambda: set_item(namespace, b"Record", replacement_record),
    ):
        try:
            mutation()
        except StrictMutationError:
            pass
        else:
            raise AssertionError("framework fallback revoked the surrounding module policy")
        assert vars(model) is namespace
        assert model.Record is original_record
        assert namespace["Record"] is original_record
    assert _soac_ext.strict_module_diagnostics(model)["sealed"] is True
    assert owner(model.Record) is None

else:
    import ctypes, importlib, json, sys, types, warnings
    model = importlib.import_module('pydantic_model')
    diagnostic = _soac_ext.strict_module_diagnostics(model)
    assert diagnostic is not None and diagnostic['sealed'] is True
    assert _soac_ext.strict_function_entry_kind(model.Record.echo) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(model.Record) is None, 'framework class acquired a strict type policy'

    assert model.Record.__pydantic_complete__ is False
    assert model.Record.model_rebuild() is True
    record = model.Record(value='2', child={'count': '3'}, extra='kept')
    assert record.value == 2 and not callable(record.value)
    assert record.child.count == 3
    assert record.extra == 'kept'
    assert record.doubled == 4 and record.doubled == 4 and record._hits == 1
    record.value = '5'
    assert record.value == 5
    after_assignment = (record.doubled, record._hits)
    record.doubled = 99
    assert record.doubled == 99
    del record.doubled
    assert record.doubled == 10
    with warnings.catch_warnings(record=True) as deprecated:
        warnings.simplefilter('always')
        assert record.legacy == 4
    assert [str(item.message) for item in deprecated] == ['use value instead']
    record.__dict__ = dict(vars(record), value=7)
    assert record.value == 7
    dump = record.model_dump()
    assert dump['tripled'] == 21 and dump['child'] == {'count': 3}
    result = {
        'dump': dump,
        'hits': record._hits,
        'cache_after_assignment': after_assignment,
        'fields_set': sorted(record.model_fields_set),
        'validation': model.EVENTS,
        'deprecated': [str(item.message) for item in deprecated],
    }

    assert record.echo('ordinary argument') == 'ordinary argument'
    def replacement(self, value):
        return ('replaced', value)
    model.Record.echo.__code__ = replacement.__code__
    assert record.echo('after mutation') == ('replaced', 'after mutation')
    model.Record.extra_method = lambda self: 19
    assert record.extra_method() == 19
    result['echo'] = record.echo('after mutation')
    print(json.dumps(result, sort_keys=True))

actual_result = json.loads(json.dumps(result, sort_keys=True))
assert actual_result == ordinary_result, ('pydantic', sys.version, actual_result, ordinary_result)
