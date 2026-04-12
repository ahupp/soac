import gc
import weakref


class Payload:
    pass


def child(refs):
    obj = Payload()
    refs.append(weakref.ref(obj))
    yield object()


def delegator(refs):
    yield from child(refs)


def throw_check():
    refs = []
    gen = delegator(refs)
    next(gen)
    try:
        gen.throw(Exception("boom"))
    except Exception:
        pass
    del gen
    gc.collect()
    return refs[0]()


# diet-python: validate

def validate_module(module):
    assert module.throw_check() is None
