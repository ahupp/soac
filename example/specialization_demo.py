from __future__ import annotations

from soac import import_hook

import_hook.install()

import specialization_workload


def main() -> None:
    result = specialization_workload.run()
    print(f"result={result}")


if __name__ == "__main__":
    main()
