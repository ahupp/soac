# diet-python: disabled

_MISSING = object()


def _mro_getattr(cls, name: str):
    for base in cls.__mro__:
        try:
            return base.__dict__[name]
        except KeyError:
            continue
    return _MISSING


def aiter(obj):
    try:
        aiter_fn = obj.__aiter__
    except AttributeError:
        obj_type = type(obj).__name__
        obj = None
        raise TypeError(
            f"'async for' requires an object with __aiter__ method, got {obj_type}"
        ) from None
    iterator = aiter_fn()
    if not hasattr(iterator, "__anext__"):
        iter_type = type(iterator).__name__
        iterator = None
        raise TypeError(
            "'async for' received an object from __aiter__ that does not implement __anext__"
            f": {iter_type}"
        ) from None
    return iterator
