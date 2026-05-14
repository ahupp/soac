"""Recomposed N-Queens search loop with diagonal consumers."""

from nqueens_slice_support import run_slice


def permutations(iterable, r=None):
    pool = tuple(iterable)
    n = len(pool)
    if r is None:
        r = n
    indices = list(range(n))
    cycles = list(range(n - r + 1, n + 1))[::-1]
    yield tuple(pool[i] for i in indices[:r])
    while n:
        for i in reversed(range(r)):
            cycles[i] -= 1
            if cycles[i] == 0:
                indices[i:] = indices[i + 1 :] + indices[i : i + 1]
                cycles[i] = n - i
            else:
                j = cycles[i]
                indices[i], indices[-j] = indices[-j], indices[i]
                yield tuple(pool[i] for i in indices[:r])
                break
        else:
            return


def nqueens_composed_consumers(queen_count):
    cols = range(queen_count)
    accepted = 0
    for vec in permutations(cols):
        if queen_count == len(set(vec[i] + i for i in cols)) == len(
            set(vec[i] - i for i in cols)
        ):
            accepted += 1
    return accepted


def expected_result(queen_count):
    if queen_count == 4:
        return 2
    if queen_count == 8:
        return 92
    return None


def main(argv=None):
    return run_slice(
        "nqueens_composed_consumers",
        nqueens_composed_consumers,
        expected_result,
        argv,
    )


if __name__ == "__main__":
    raise SystemExit(main())
