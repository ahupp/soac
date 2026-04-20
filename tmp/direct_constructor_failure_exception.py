class Marker(Exception):
    pass


class Broken:
    def __init__(self, value):
        raise Marker(f"boom:{value}")


def run():
    try:
        Broken(7)
    except Marker as exc:
        return [type(exc).__name__, str(exc), exc.__context__ is None]
