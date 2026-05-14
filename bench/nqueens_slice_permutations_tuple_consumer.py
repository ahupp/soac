"""Permutation tuple consumer from the N-Queens benchmark."""

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


def permutations_tuple_consumer(queen_count):
    return sum(1 for _ in permutations(range(queen_count)))


def expected_result(queen_count):
    expected = 1
    for value in range(2, queen_count + 1):
        expected *= value
    return expected


def main(argv=None):
    return run_slice(
        "permutations_tuple_consumer",
        permutations_tuple_consumer,
        expected_result,
        argv,
    )


if __name__ == "__main__":
    raise SystemExit(main())
