"""Ordered N-Queens solutions through generalized closed iterator fusion.

The named ``permutations`` generator and the diagonal ``set`` consumers remain
native. The outer ``list(filter(..., map(..., genexpr)))`` pipeline is the
generalized fusion candidate.
"""

from itertools import permutations as itertools_permutations
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


def ordered_closed_pipeline(queen_count):
    cols = range(queen_count)

    def valid(vec):
        return queen_count == len(set(vec[i] + i for i in cols)) == len(
            set(vec[i] - i for i in cols)
        )

    source = (vec for vec in permutations(cols))
    return list(filter(valid, map(tuple, source)))


def expected_result(queen_count):
    if queen_count == 1:
        return [(0,)]
    if queen_count == 4:
        return [(1, 3, 0, 2), (2, 0, 3, 1)]
    if queen_count == 8:
        # run_slice computes this independent reference before starting the timer.
        result = []
        for vec in itertools_permutations(range(queen_count)):
            positive_diagonals = set()
            negative_diagonals = set()
            for index, value in enumerate(vec):
                positive_diagonals.add(value + index)
                negative_diagonals.add(value - index)
            if (
                queen_count == len(positive_diagonals)
                and queen_count == len(negative_diagonals)
            ):
                result.append(vec)
        return result
    return None


def main(argv=None):
    return run_slice(
        "ordered_closed_pipeline",
        ordered_closed_pipeline,
        expected_result,
        argv,
    )


if __name__ == "__main__":
    raise SystemExit(main())
