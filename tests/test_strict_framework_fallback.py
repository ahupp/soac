"""Real framework classes stay ordinary inside authenticated strict modules."""

import json
import sys
import textwrap

import pytest

from tests._strict_integration import ROOT, create_strict_project

SOURCES = {
    "pydantic": """
from __future__ import strict
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
""",
    "django": """
from __future__ import strict
from django.db import models

class Record(models.Model):
    value = models.IntegerField(default=1)
    label = models.CharField(max_length=20, default='first')

    class Meta:
        app_label = 'soac_fallback'

    def echo(self, value: int) -> int:
        return value
""",
    "sqlalchemy": """
from __future__ import strict
from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column

class Base(DeclarativeBase):
    pass

class Record(Base):
    __tablename__ = 'records'
    id: Mapped[int] = mapped_column(primary_key=True)
    value: Mapped[int] = mapped_column(default=1)

    def echo(self, value: int) -> int:
        return value
""",
}


PRELUDES = {
    "pydantic": "",
    "django": """
import django
from django.conf import settings
settings.configure(
    INSTALLED_APPS=[],
    DATABASES={'default': {'ENGINE': 'django.db.backends.sqlite3', 'NAME': ':memory:'}},
    DEFAULT_AUTO_FIELD='django.db.models.AutoField',
    SECRET_KEY='local-compatibility-test',
    USE_TZ=False,
)
django.setup()
""",
    "sqlalchemy": "",
}


EXERCISES = {
    "pydantic": """
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
""",
    "django": """
from django.db import connection
with connection.schema_editor() as editor:
    editor.create_model(model.Record)
record = model.Record(value=3)
record.save()
assert model.Record.objects.get(pk=record.pk).value == 3
record.__dict__ = dict(vars(record), value=6)
assert record.value == 6
record.save(update_fields=['value'])
result = {
    'value': model.Record.objects.get(pk=record.pk).value,
    'fields': [field.name for field in model.Record._meta.fields],
    'label': record.label,
}
""",
    "sqlalchemy": """
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
""",
}


@pytest.fixture(scope="module", params=tuple(SOURCES))
def framework_project(request, tmp_path_factory):
    framework = request.param
    module = f"{framework}_model"
    project = create_strict_project(
        tmp_path_factory.mktemp(f"strict-framework-{framework}"),
        {f"{module}.py": SOURCES[framework]},
        modules={module: f"{module}.py"},
    )
    return framework, module, project


def _exercise_source(framework, module, *, strict, entry_interpreter):
    source = textwrap.dedent(SOURCES[framework]).lstrip("\n")
    if strict:
        load = f"model = importlib.import_module({module!r})\n"
    else:
        ordinary = source.replace("from __future__ import strict\n", "", 1)
        load = (
            f"model = types.ModuleType({module!r})\n"
            f"sys.modules[{module!r}] = model\n"
            f"exec(compile({ordinary!r}, '<ordinary framework control>', 'exec'), vars(model))\n"
        )
    expected_entry = "entry_interpreter" if entry_interpreter else "checked_native"
    witness = (
        "diagnostic = _soac_ext.strict_module_diagnostics(model)\n"
        "assert diagnostic is not None and diagnostic['sealed'] is True\n"
        f"assert _soac_ext.strict_function_entry_kind(model.Record.echo) == {expected_entry!r}\n"
        if strict
        else "assert _soac_ext.strict_module_diagnostics(model) is None\n"
    )
    return (
        "import ctypes, importlib, json, sys, types, warnings\n"
        + textwrap.dedent(PRELUDES[framework])
        + load
        + witness
        + """
owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert owner(model.Record) is None, 'framework class acquired a strict type policy'
"""
        + textwrap.dedent(EXERCISES[framework])
        + """
assert record.echo('ordinary argument') == 'ordinary argument'
def replacement(self, value):
    return ('replaced', value)
model.Record.echo.__code__ = replacement.__code__
assert record.echo('after mutation') == ('replaced', 'after mutation')
model.Record.extra_method = lambda self: 19
assert record.extra_method() == 19
result['echo'] = record.echo('after mutation')
print(json.dumps(result, sort_keys=True))
"""
    )


@pytest.mark.parametrize("entry_interpreter", [False, True])
def test_real_framework_fallback_matches_ordinary_python(
    framework_project, entry_interpreter
):
    framework, module, project = framework_project
    results = []
    for strict in (False, True):
        completed = project.run(
            _exercise_source(
                framework, module, strict=strict, entry_interpreter=entry_interpreter
            ),
            entry_interpreter=entry_interpreter,
        )
        results.append(json.loads(completed.stdout.splitlines()[-1]))
    assert results[1] == results[0], (framework, sys.version, results)



@pytest.fixture(scope="module", params=tuple(SOURCES))
def cpython_framework_project(request, tmp_path_factory):
    framework = request.param
    module = f"{framework}_model"
    project = create_strict_project(
        tmp_path_factory.mktemp(f"cpython-framework-{framework}"),
        {f"{module}.py": SOURCES[framework]},
        modules={module: f"{module}.py"}, backend="cpython",
    )
    return framework, module, project


def _cpython_framework_exercise(framework, module, project, *, strict):
    import hashlib

    source = textwrap.dedent(SOURCES[framework]).lstrip("\n")
    if strict:
        load = f"model = importlib.import_module({module!r})\n"
        source_path = project.project / f"{module}.py"
        witness = (
            f"sys.path.insert(0, {str(ROOT)!r})\n"
            "from tests._strict_integration import _assert_cpython_module_witness\n"
            "_assert_cpython_module_witness(\n"
            f"    model, module_name={module!r}, source_path={str(source_path)!r},\n"
            f"    source_sha256={hashlib.sha256(source_path.read_bytes()).hexdigest()!r},\n"
            f"    artifact_generation={project.publication['generation']!r},\n"
            ")\n"
        )
    else:
        ordinary = source.replace("from __future__ import strict\n", "", 1)
        load = (
            f"model = types.ModuleType({module!r})\n"
            f"sys.modules[{module!r}] = model\n"
            f"exec(compile({ordinary!r}, '<ordinary framework control>', 'exec'), vars(model))\n"
        )
        witness = "assert _soac_ext.strict_module_diagnostics(model) is None\n"
    module_checks = """
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
""" if strict else ""
    return (
        "import ctypes, importlib, json, sys, types, warnings\n"
        + textwrap.dedent(PRELUDES[framework])
        + load
        + witness
        + """
owner = ctypes.pythonapi.PyType_GetSoacContractOwner
owner.argtypes = [ctypes.py_object]
owner.restype = ctypes.c_void_p
assert owner(model.Record) is None, 'framework class acquired a strict policy'
metadata = ctypes.pythonapi.PyFunction_GetSoacMetadata
metadata.argtypes = [ctypes.py_object]
metadata.restype = ctypes.c_void_p
assert metadata(model.Record.echo) is None
"""
        + textwrap.dedent(EXERCISES[framework])
        + """
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
"""
        + module_checks
    )


def test_cpython_backend_framework_fallback_preserves_original_behavior(cpython_framework_project):
    framework, module, project = cpython_framework_project
    results = []
    for strict in (False, True):
        completed = project.run(
            _cpython_framework_exercise(framework, module, project, strict=strict),
            backend="cpython",
        )
        results.append(json.loads(completed.stdout.splitlines()[-1]))
    assert results[1] == results[0], (framework, sys.version, results)
