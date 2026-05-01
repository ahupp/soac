"""Tiny recursive Fibonacci benchmark."""

import sys
import time


def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


def main(argv=None):
    if argv is None:
        argv = sys.argv
    if len(argv) != 2:
        print(f"usage: {argv[0]} <n>", file=sys.stderr)
        return 2
    try:
        n = int(argv[1])
    except ValueError:
        print(f"invalid fibonacci input: {argv[1]!r}", file=sys.stderr)
        return 2
    if n < 0:
        print("fibonacci input must be non-negative", file=sys.stderr)
        return 2

    start = time.perf_counter()
    result = fib(n)
    elapsed = time.perf_counter() - start
    print(f"fib({n}) = {result}")
    print(f"elapsed_s = {elapsed:.9f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
