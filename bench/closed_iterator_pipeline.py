def convert(value):
    return value * 3 + 1


def keep(value):
    return value % 2 == 0


def collect(count):
    return tuple(
        filter(
            keep,
            map(convert, (value for value in range(count))),
        )
    )


def expected(count):
    return tuple(value * 3 + 1 for value in range(count) if (value * 3 + 1) % 2 == 0)
