from __future__ import annotations


def add(value: int, increment: int) -> int:
    return value + increment


def run() -> int:
    total = 0
    for outer in range(200):
        for inner in range(50):
            total = add(total, outer + inner)
    return total
