"""CLI dispatcher for isolated N-Queens-derived benchmark slices."""

import importlib
import sys


SLICE_MODULES = {
    "permutations_tuple_consumer": "nqueens_slice_permutations_tuple_consumer",
    "diagonal_set_consumers": "nqueens_slice_diagonal_set_consumers",
    "nqueens_composed_consumers": "nqueens_slice_nqueens_composed_consumers",
    "full_nqueens_list_consumer": "nqueens_slice_full_nqueens_list_consumer",
}


def parse_args(argv):
    if len(argv) < 3:
        print(
            f"usage: {argv[0]} <slice> <queen-count> [loops] [--compile-only]",
            file=sys.stderr,
        )
        return None

    name = argv[1]
    module_name = SLICE_MODULES.get(name)
    if module_name is None:
        choices = ", ".join(sorted(SLICE_MODULES))
        print(f"unknown slice: {name!r}; choose one of: {choices}", file=sys.stderr)
        return None

    return module_name, argv[2:]


def main(argv=None):
    if argv is None:
        argv = sys.argv

    parsed = parse_args(argv)
    if parsed is None:
        return 2

    module_name, forwarded_args = parsed
    module = importlib.import_module(module_name)
    return module.main([f"{module_name}.py", *forwarded_args])


if __name__ == "__main__":
    raise SystemExit(main())
