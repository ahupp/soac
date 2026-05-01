import asyncio


class AwaitingExit:
    async def __aenter__(self):
        await asyncio.sleep(0)
        return self

    async def __aexit__(self, exc_type, exc, tb):
        await asyncio.sleep(0)
        return None


def check():
    async def inner():
        async with AwaitingExit():
            for _ in range(1):
                await asyncio.sleep(0)
            result = 9.5
            print("async_with_return_after_awaiting_aexit", type(result).__name__, result)
            return result

    return asyncio.run(inner())


TOP_LEVEL_RESULT = check()
assert TOP_LEVEL_RESULT == 9.5


# diet-python: validate


def validate_module(module):
    assert module.check() == 9.5
    assert module.TOP_LEVEL_RESULT == 9.5
