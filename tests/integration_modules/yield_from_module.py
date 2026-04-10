def child():
    events = []
    try:
        value = yield "start"
        events.append(("send", value))
        while True:
            try:
                value = yield value
                events.append(("send", value))
            except KeyError as exc:
                events.append(("throw", str(exc)))
                value = "handled"
            if value == "stop":
                break
    finally:
        events.append(("finally", None))
    return events


def delegator():
    result = yield from child()
    return ("done", result)

# diet-python: validate

def validate_module(module):

    import pytest
    import builtins

    if __dp_integration_soac__:
        assert "runtime" not in module.__dict__
        assert not hasattr(builtins, "runtime")
        assert not hasattr(builtins, "__soac__")

    gen = module.delegator()

    assert next(gen) == "start"
    assert gen.send("first") == "first"
    assert gen.throw(KeyError("boom")) == "handled"

    with pytest.raises(StopIteration) as exc:
        gen.send("stop")

        result = exc.value.value
        assert result[0] == "done"
        assert result[1] == [
        ("send", "first"),
        ("throw", "'boom'"),
        ("send", "stop"),
        ("finally", None),
        ]
