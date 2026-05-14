"""Diagonal `set(genexpr)` consumers from the N-Queens benchmark."""

from nqueens_slice_support import run_slice


def diagonal_set_consumers(queen_count):
    cols = range(queen_count)
    vec = tuple(range(queen_count))
    total = 0
    total += len(set(vec[i] + i for i in cols))
    total += len(set(vec[i] - i for i in cols))
    return total


def expected_result(queen_count):
    return queen_count + 1


def main(argv=None):
    return run_slice(
        "diagonal_set_consumers",
        diagonal_set_consumers,
        expected_result,
        argv,
    )


if __name__ == "__main__":
    raise SystemExit(main())
