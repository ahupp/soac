#!/usr/bin/env python3

from __future__ import annotations

import json
import sys


def parse_folded_stacks(lines: list[str]) -> tuple[list[dict[str, str]], list[list[int]], list[int]]:
    frames: list[dict[str, str]] = []
    frame_ids: dict[str, int] = {}
    samples: list[list[int]] = []
    weights: list[int] = []

    for raw_line in lines:
        line = raw_line.strip()
        if not line:
            continue
        try:
            stack_text, weight_text = line.rsplit(maxsplit=1)
        except ValueError:
            continue
        try:
            weight = int(weight_text)
        except ValueError:
            continue
        stack_ids: list[int] = []
        for frame_name in stack_text.split(";"):
            frame_id = frame_ids.get(frame_name)
            if frame_id is None:
                frame_id = len(frames)
                frame_ids[frame_name] = frame_id
                frames.append({"name": frame_name})
            stack_ids.append(frame_id)
        samples.append(stack_ids)
        weights.append(weight)
    return frames, samples, weights


def main() -> int:
    profile_name = sys.argv[1] if len(sys.argv) > 1 else "perf"
    frames, samples, weights = parse_folded_stacks(sys.stdin.readlines())
    output = {
        "$schema": "https://www.speedscope.app/file-format-schema.json",
        "shared": {"frames": frames},
        "profiles": [
            {
                "type": "sampled",
                "name": profile_name,
                "unit": "samples",
                "samples": samples,
                "weights": weights,
            }
        ],
        "exporter": "soac folded_to_speedscope",
        "name": profile_name,
        "activeProfileIndex": 0,
    }
    json.dump(output, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
