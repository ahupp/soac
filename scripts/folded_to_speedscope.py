#!/usr/bin/env python3

import json
import sys


def parse_folded(lines: list[str]) -> tuple[list[dict[str, str]], list[list[int]], list[int]]:
    frame_ids: dict[str, int] = {}
    frames: list[dict[str, str]] = []
    samples: list[list[int]] = []
    weights: list[int] = []

    for raw_line in lines:
        line = raw_line.strip()
        if not line:
            continue

        try:
            stack, weight_text = line.rsplit(" ", 1)
        except ValueError as exc:
            raise SystemExit(f"invalid folded stack line: {line!r}") from exc

        try:
            weight = int(weight_text)
        except ValueError as exc:
            raise SystemExit(f"invalid folded stack weight: {line!r}") from exc

        sample: list[int] = []
        if stack:
            for frame_name in stack.split(";"):
                frame_id = frame_ids.get(frame_name)
                if frame_id is None:
                    frame_id = len(frames)
                    frame_ids[frame_name] = frame_id
                    frames.append({"name": frame_name})
                sample.append(frame_id)

        samples.append(sample)
        weights.append(weight)

    return frames, samples, weights


def main() -> None:
    profile_name = sys.argv[1] if len(sys.argv) > 1 else "perf"
    frames, samples, weights = parse_folded(sys.stdin.readlines())
    total_weight = sum(weights)
    result = {
        "$schema": "https://www.speedscope.app/file-format-schema.json",
        "shared": {"frames": frames},
        "profiles": [
            {
                "type": "sampled",
                "name": profile_name,
                "unit": "samples",
                "startValue": 0,
                "endValue": total_weight,
                "samples": samples,
                "weights": weights,
            }
        ],
        "activeProfileIndex": 0,
        "exporter": "soac folded_to_speedscope",
        "name": profile_name,
    }
    json.dump(result, sys.stdout)


if __name__ == "__main__":
    main()
