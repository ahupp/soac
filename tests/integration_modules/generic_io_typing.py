from typing import Generic, List, TypeVar

AnyStr = TypeVar("AnyStr", str, bytes)


class Example(Generic[AnyStr]):
    def readlines(self) -> List[AnyStr]:
        ...

# diet-python: validate

def validate_module(module):
    # Inspect the typing objects the selected interpreter actually imported.
    # A second checkout or a sys.modules replacement cannot change the already
    # constructed class, and must not turn this behavior check into a skip.
    example = module.Example
    readlines = example.readlines
    annotations = readlines.__annotations__
    ann = annotations["return"]
    assert getattr(ann, "__origin__", None) is list
    assert getattr(ann, "__args__", None) == (module.AnyStr,)
    orig_base = example.__orig_bases__[0]
    assert orig_base.__origin__ is module.Generic
    assert orig_base.__args__ == (module.AnyStr,)
