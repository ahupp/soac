import asyncio


class Once:
    def __await__(self):
        yield "tick"
        return 41


async def run():
    value = await Once()
    return value + 1


async def coro():
    return 1


class C:
    async def method(self):
        return 2

# diet-python: validate

def validate_module(module):
    import asyncio
    import inspect

    coro = module.run()
    assert asyncio.iscoroutine(coro)
    assert coro.send(None) == "tick"
    try:
        coro.send(None)
    except StopIteration as exc:
        assert exc.value == 42
    else:
        raise AssertionError("expected StopIteration")

    assert asyncio.iscoroutinefunction(module.run)
    assert inspect.iscoroutinefunction(module.coro)
    assert inspect.iscoroutinefunction(module.C.method)
    assert inspect.iscoroutinefunction(module.C().method)
