# modes:soac,entry,cpython
# module:sqlalchemy_model
# soac: module(strict_assign=true, checked_attr=true)
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column

class Base(DeclarativeBase):
    pass

class Record(Base):
    __tablename__ = 'records'
    id: Mapped[int] = mapped_column(primary_key=True)
    value: Mapped[int] = mapped_column(default=1)

    def echo(self, value: int) -> int:
        return value
# ok
# test_real_framework_fallback_matches_ordinary_python [sqlalchemy]
import sys
from soac import _soac_ext
import json
import os
import subprocess
# The ordinary framework registry/database must be isolated from the selected
# model, as in the original two-subprocess comparison. This child uses the
# same interpreter, environment and actual installed dependency paths.
ordinary_source = 'import ctypes, importlib, json, sys, types, warnings\nmodel = types.ModuleType(\'sqlalchemy_model\')\nsys.modules[\'sqlalchemy_model\'] = model\nexec(compile("from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column\\n\\nclass Base(DeclarativeBase):\\n    pass\\n\\nclass Record(Base):\\n    __tablename__ = \'records\'\\n    id: Mapped[int] = mapped_column(primary_key=True)\\n    value: Mapped[int] = mapped_column(default=1)\\n\\n    def echo(self, value: int) -> int:\\n        return value\\n", \'<ordinary framework control>\', \'exec\'), vars(model))\nassert _soac_ext.strict_module_diagnostics(model) is None\n\nowner = ctypes.pythonapi.PyType_GetSoacContractOwner\nowner.argtypes = [ctypes.py_object]\nowner.restype = ctypes.c_void_p\nassert owner(model.Record) is None, \'framework class acquired a strict policy\'\nmetadata = ctypes.pythonapi.PyFunction_GetSoacMetadata\nmetadata.argtypes = [ctypes.py_object]\nmetadata.restype = ctypes.c_void_p\nassert metadata(model.Record.echo) is None\n\nfrom sqlalchemy import create_engine, select\nfrom sqlalchemy.orm import Session\nengine = create_engine(\'sqlite:///:memory:\')\nmodel.Base.metadata.create_all(engine)\nwith Session(engine) as session:\n    record = model.Record(value=3)\n    session.add(record)\n    session.commit()\n    assert session.scalar(select(model.Record.value)) == 3\n    record.value = 7\n    session.commit()\n    assert session.scalar(select(model.Record.value)) == 7\n    record.__dict__ = dict(vars(record), value=8)\n    assert record.value == 8\n    result = {\n        \'value\': record.value,\n        \'columns\': list(model.Record.__table__.columns.keys()),\n    }\nengine.dispose()\n\nfor _ in range(128):\n    assert record.echo(\'ordinary argument\') == \'ordinary argument\'\ncall = ctypes.pythonapi.PyObject_CallOneArg\ncall.argtypes = [ctypes.py_object, ctypes.py_object]\ncall.restype = ctypes.py_object\nassert call(record.echo, \'ordinary C argument\') == \'ordinary C argument\'\ndef replacement(self, value):\n    return (\'replaced\', value)\nmodel.Record.echo.__code__ = replacement.__code__\nassert record.echo(\'after mutation\') == (\'replaced\', \'after mutation\')\nmodel.Record.extra_method = lambda self: 19\nassert record.extra_method() == 19\nresult[\'echo\'] = record.echo(\'after mutation\')\nprint(json.dumps(result, sort_keys=True))\n' if __dp_integration_mode__ == 'cpython' else 'import ctypes, importlib, json, sys, types, warnings\nmodel = types.ModuleType(\'sqlalchemy_model\')\nsys.modules[\'sqlalchemy_model\'] = model\nexec(compile("from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column\\n\\nclass Base(DeclarativeBase):\\n    pass\\n\\nclass Record(Base):\\n    __tablename__ = \'records\'\\n    id: Mapped[int] = mapped_column(primary_key=True)\\n    value: Mapped[int] = mapped_column(default=1)\\n\\n    def echo(self, value: int) -> int:\\n        return value\\n", \'<ordinary framework control>\', \'exec\'), vars(model))\nassert _soac_ext.strict_module_diagnostics(model) is None\n\nowner = ctypes.pythonapi.PyType_GetSoacContractOwner\nowner.argtypes = [ctypes.py_object]\nowner.restype = ctypes.c_void_p\nassert owner(model.Record) is None, \'framework class acquired a strict type policy\'\n\nfrom sqlalchemy import create_engine, select\nfrom sqlalchemy.orm import Session\nengine = create_engine(\'sqlite:///:memory:\')\nmodel.Base.metadata.create_all(engine)\nwith Session(engine) as session:\n    record = model.Record(value=3)\n    session.add(record)\n    session.commit()\n    assert session.scalar(select(model.Record.value)) == 3\n    record.value = 7\n    session.commit()\n    assert session.scalar(select(model.Record.value)) == 7\n    record.__dict__ = dict(vars(record), value=8)\n    assert record.value == 8\n    result = {\n        \'value\': record.value,\n        \'columns\': list(model.Record.__table__.columns.keys()),\n    }\nengine.dispose()\n\nassert record.echo(\'ordinary argument\') == \'ordinary argument\'\ndef replacement(self, value):\n    return (\'replaced\', value)\nmodel.Record.echo.__code__ = replacement.__code__\nassert record.echo(\'after mutation\') == (\'replaced\', \'after mutation\')\nmodel.Record.extra_method = lambda self: 19\nassert record.extra_method() == 19\nresult[\'echo\'] = record.echo(\'after mutation\')\nprint(json.dumps(result, sort_keys=True))\n'
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
    model = importlib.import_module('sqlalchemy_model')
    sys.path.insert(0, str(__import__('tests._strict_integration', fromlist=['ROOT']).ROOT))
    from tests._strict_integration import _assert_cpython_module_witness
    _assert_cpython_module_witness(
        model, module_name='sqlalchemy_model', source_path=str(__import__('pathlib').Path(sys.modules['sqlalchemy_model'].__file__)),
        source_sha256='34d522d71af60481ae09e9dd630da6a4999880b650c9c97dad963cbb6e44eeb5',
        artifact_generation=_soac_ext.strict_module_diagnostics(sys.modules['sqlalchemy_model'])['artifact_generation'],
    )

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(model.Record) is None, 'framework class acquired a strict policy'
    metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
    metadata.argtypes = [ctypes.py_object]
    metadata.restype = ctypes.c_void_p
    assert metadata(model.Record.echo) is None

    from sqlalchemy import create_engine, select
    from sqlalchemy.orm import Session
    engine = create_engine('sqlite:///:memory:')
    model.Base.metadata.create_all(engine)
    with Session(engine) as session:
        record = model.Record(value=3)
        session.add(record)
        session.commit()
        assert session.scalar(select(model.Record.value)) == 3
        record.value = 7
        session.commit()
        assert session.scalar(select(model.Record.value)) == 7
        record.__dict__ = dict(vars(record), value=8)
        assert record.value == 8
        result = {
            'value': record.value,
            'columns': list(model.Record.__table__.columns.keys()),
        }
    engine.dispose()

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
    model = importlib.import_module('sqlalchemy_model')
    diagnostic = _soac_ext.strict_module_diagnostics(model)
    assert diagnostic is not None and diagnostic['sealed'] is True
    assert _soac_ext.strict_function_entry_kind(model.Record.echo) == ('entry_interpreter' if __dp_integration_entry__ else 'checked_native')

    owner = ctypes.pythonapi.PyType_GetSoacContractOwner
    owner.argtypes = [ctypes.py_object]
    owner.restype = ctypes.c_void_p
    assert owner(model.Record) is None, 'framework class acquired a strict type policy'

    from sqlalchemy import create_engine, select
    from sqlalchemy.orm import Session
    engine = create_engine('sqlite:///:memory:')
    model.Base.metadata.create_all(engine)
    with Session(engine) as session:
        record = model.Record(value=3)
        session.add(record)
        session.commit()
        assert session.scalar(select(model.Record.value)) == 3
        record.value = 7
        session.commit()
        assert session.scalar(select(model.Record.value)) == 7
        record.__dict__ = dict(vars(record), value=8)
        assert record.value == 8
        result = {
            'value': record.value,
            'columns': list(model.Record.__table__.columns.keys()),
        }
    engine.dispose()

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
assert actual_result == ordinary_result, ('sqlalchemy', sys.version, actual_result, ordinary_result)
