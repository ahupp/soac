import gc
import weakref


class Payload:
    pass


def _payload_ref_from(gen):
    refs = []
    gen_obj = gen(refs)
    assert next(gen_obj) == 1
    try:
        next(gen_obj)
    except StopIteration:
        pass
    gc.collect()
    return refs[0](), gen_obj.gi_yieldfrom


def completed_payload_released():
    def gen(refs):
        payload = Payload()
        refs.append(weakref.ref(payload))
        yield 1

    return _payload_ref_from(gen)


def escaped_payload_released():
    refs = []

    def gen():
        payload = Payload()
        refs.append(weakref.ref(payload))
        yield 1
        raise ValueError("boom")

    gen_obj = gen()
    assert next(gen_obj) == 1
    try:
        next(gen_obj)
    except ValueError:
        pass
    gc.collect()
    return refs[0](), gen_obj.gi_yieldfrom


def closed_throw_uses_terminal_state():
    def gen():
        yield 1

    gen_obj = gen()
    assert next(gen_obj) == 1
    try:
        next(gen_obj)
    except StopIteration:
        pass
    try:
        gen_obj.throw(ValueError("boom"))
    except ValueError as exc:
        return str(exc), gen_obj.gi_yieldfrom
    raise AssertionError("closed generator throw should reraise the supplied exception")


# diet-python: validate

def validate_module(module):
    assert module.completed_payload_released() == (None, None)
    assert module.escaped_payload_released() == (None, None)
    assert module.closed_throw_uses_terminal_state() == ("boom", None)
