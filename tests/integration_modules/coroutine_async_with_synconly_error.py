class SyncOnly:
    def __enter__(self):
        return self
    def __exit__(self, exc_type, exc, tb):
        return False


def make_inner():
    async def inner():
        try:
            async with SyncOnly():
                pass
        except TypeError as exc:
            return str(exc)
        return None

    return inner()


EXPECTED = (
    f"{SyncOnly.__module__}.{SyncOnly.__qualname__!s}"
)
EXPECTED_MESSAGE = (
    f"{EXPECTED!r} object does not support the asynchronous context manager protocol "
    "(missed __aexit__ method) but it supports the context manager protocol. "
    "Did you mean to use 'with'?"
)

# diet-python: validate

def validate_module(module):
    if __dp_integration_soac__:
        coro = module.make_inner()
        try:
            coro.send(None)
        except StopIteration as exc:
            assert exc.value == module.EXPECTED_MESSAGE
        else:
            raise AssertionError('expected StopIteration')
    else:
        import asyncio

        assert asyncio.run(module.make_inner()) == module.EXPECTED_MESSAGE
