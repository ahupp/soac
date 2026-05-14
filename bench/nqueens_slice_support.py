"""Shared CLI runner for isolated N-Queens benchmark slices."""

import sys
import time


def parse_args(argv):
    compile_only = False
    args = list(argv[1:])
    if args and args[-1] == "--compile-only":
        compile_only = True
        args.pop()

    if len(args) not in {1, 2}:
        print(
            f"usage: {argv[0]} <queen-count> [loops] [--compile-only]",
            file=sys.stderr,
        )
        return None

    try:
        queen_count = int(args[0])
        loops = int(args[1]) if len(args) == 2 else 1
    except ValueError:
        print("queen-count and loops must be integers", file=sys.stderr)
        return None

    if queen_count <= 0 or loops <= 0:
        print("queen-count and loops must be positive", file=sys.stderr)
        return None

    return queen_count, loops, compile_only


def run_slice(name, workload, expected_result, argv=None):
    if argv is None:
        argv = sys.argv

    parsed = parse_args(argv)
    if parsed is None:
        return 2

    queen_count, loops, compile_only = parsed
    if compile_only:
        compile_queen_count = 1
        compile_result = workload(compile_queen_count)
        print(f"slice = {name}")
        print(f"queen_count = {queen_count}")
        print(f"loops = {loops}")
        print("compile_only = true")
        print(f"compile_queen_count = {compile_queen_count}")
        print(f"compile_result = {compile_result}")
        return 0

    expected = expected_result(queen_count)
    start = time.perf_counter()
    result = None
    for _ in range(loops):
        result = workload(queen_count)
    elapsed = time.perf_counter() - start

    if expected is not None and result != expected:
        print(
            f"wrong result for {name}: got {result}, expected {expected}",
            file=sys.stderr,
        )
        return 1

    print(f"slice = {name}")
    print(f"queen_count = {queen_count}")
    print(f"loops = {loops}")
    print(f"result = {result}")
    print(f"elapsed_s = {elapsed:.9f}")
    print(f"iterations_per_s = {loops / elapsed:.3f}")
    return 0
