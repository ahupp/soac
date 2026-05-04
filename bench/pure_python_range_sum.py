"""Sum over a pure-Python range-like iterator."""

import sys
import time


class PurePythonRange:
    def __init__(self, start, stop, step):
        self.current = start
        self.stop = stop
        self.step = step

    def __iter__(self):
        return self

    def __next__(self):
        current = self.current
        stop = self.stop
        step = self.step
        if step > 0:
            if current >= stop:
                raise StopIteration
        elif current <= stop:
            raise StopIteration
        self.current = current + step
        return current


def range(stop):
    return PurePythonRange(0, stop, 1)


def sum(i):
    s = 0
    for j in range(i):
        s += j
    return s


def expected_sum(i):
    if i <= 0:
        return 0
    return i * (i - 1) // 2


def main(argv=None):
    if argv is None:
        argv = sys.argv
    if len(argv) != 2:
        print(f"usage: {argv[0]} <n>", file=sys.stderr)
        return 2
    try:
        n = int(argv[1])
    except ValueError:
        print(f"invalid input: {argv[1]!r}", file=sys.stderr)
        return 2

    start = time.perf_counter()
    result = sum(n)
    elapsed = time.perf_counter() - start
    expected = expected_sum(n)
    if result != expected:
        print(f"wrong result: got {result}, expected {expected}", file=sys.stderr)
        return 1
    print(f"sum({n}) = {result}")
    print(f"elapsed_s = {elapsed:.9f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
