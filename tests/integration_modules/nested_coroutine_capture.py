def build():
    cancelled = False

    class Test:
        async def run(self):
            async def mark_cancelled():
                nonlocal cancelled
                cancelled = True

            await mark_cancelled()

        def was_cancelled(self):
            return cancelled

    return Test()


# diet-python: validate

def validate_module(module):
    instance = module.build()
    coroutine = instance.run()
    try:
        coroutine.send(None)
    except StopIteration as exc:
        assert exc.value is None
    else:
        raise AssertionError("coroutine should finish on its first send")
    assert instance.was_cancelled() is True
