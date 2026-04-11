from __future__ import annotations

import asyncio
import gc
import weakref


class Payload:
    pass


async def hold_ref(ref_holder):
    obj = Payload()
    ref_holder.append(weakref.ref(obj))
    await asyncio.sleep(10)


class SuspendOnce:
    def __await__(self):
        yield self
        return None


async def hold_ref_until_cancelled(ref_holder):
    obj = Payload()
    ref_holder.append(weakref.ref(obj))
    await SuspendOnce()


def throw_cancelled_check():
    ref_holder = []
    coro = hold_ref_until_cancelled(ref_holder)
    coro.send(None)
    try:
        coro.throw(asyncio.CancelledError)
    except asyncio.CancelledError:
        pass
    del coro
    gc.collect()
    return ref_holder[0]()


def leak_check():
    ref_holder = []

    async def runner():
        await asyncio.wait_for(hold_ref(ref_holder), 0.01)

    try:
        asyncio.run(runner())
    except asyncio.TimeoutError:
        pass

    gc.collect()
    return ref_holder[0]()


# diet-python: validate

def validate_module(module):
    assert module.throw_cancelled_check() is None
    assert module.leak_check() is None
