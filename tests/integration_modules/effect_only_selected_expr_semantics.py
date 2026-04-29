events = []


class Marker:
    def __init__(self, label, truth):
        self.label = label
        self.truth = truth

    def __bool__(self):
        events.append(f"bool:{self.label}")
        return self.truth


class NoBool:
    def __init__(self, label):
        self.label = label

    def __bool__(self):
        raise AssertionError(f"{self.label} should not be truth-tested")


class ChainValue:
    def __init__(self, label):
        self.label = label

    def __lt__(self, other):
        events.append(f"lt:{self.label}:{other.label}")
        return Marker(f"{self.label}:{other.label}", True)


def truthy(label):
    events.append(f"call:{label}")
    return Marker(label, True)


def falsey(label):
    events.append(f"call:{label}")
    return Marker(label, False)


def final(label):
    events.append(f"final:{label}")
    return NoBool(label)


def ifexpr_effect():
    events.clear()
    final("then") if truthy("cond") else final("else")
    return list(events)


def boolop_and_effect():
    events.clear()
    truthy("left") and final("right")
    return list(events)


def boolop_or_effect():
    events.clear()
    falsey("left") or final("fallback")
    return list(events)


def not_boolop_effect():
    events.clear()
    not (truthy("left") and truthy("right"))
    return list(events)


def compare_chain_effect():
    events.clear()
    a = ChainValue("a")
    b = ChainValue("b")
    c = ChainValue("c")
    a < b < c
    return list(events)


# diet-python: validate


def validate_module(module):
    assert module.ifexpr_effect() == ["call:cond", "bool:cond", "final:then"]
    assert module.boolop_and_effect() == ["call:left", "bool:left", "final:right"]
    assert module.boolop_or_effect() == ["call:left", "bool:left", "final:fallback"]
    assert module.not_boolop_effect() == [
        "call:left",
        "bool:left",
        "call:right",
        "bool:right",
    ]
    assert module.compare_chain_effect() == ["lt:a:b", "bool:a:b", "lt:b:c"]
