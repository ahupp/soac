#!/usr/bin/env python3

import json
import re
import sys
from dataclasses import dataclass, field


TOP_ENTRY_RE = re.compile(r"^\s*(?P<overhead>\d+\.\d+)%\s+(?P<samples>\d+)\s+(?P<rest>.+?)\s*$")
CALLCHAIN_COUNT_RE = re.compile(r"^\s*(?P<count>\d+)\s*$")


@dataclass
class PerfEntry:
    root_frame: str
    samples: int
    caller_stacks: list[tuple[list[str], int]] = field(default_factory=list)


def normalize_dso_name(dso: str) -> str:
    if dso.startswith("[JIT]"):
        return "[JIT]"
    return dso


def format_root_frame(dso: str, symbol: str) -> str:
    normalized_dso = normalize_dso_name(dso)
    return f"{symbol} {normalized_dso}"


def parse_root_entry(rest: str) -> tuple[str, str] | None:
    if "[.]" not in rest:
        return None

    dso_text, symbol_text = rest.split("[.]", 1)
    dso = dso_text.strip()
    symbol = re.split(r"\s{2,}", symbol_text.strip(), maxsplit=1)[0].strip()
    if not dso or not symbol:
        raise SystemExit(f"invalid perf report entry: {rest!r}")
    return dso, symbol


def finish_pending_callchain(
    entry: PerfEntry | None,
    pending_count: int | None,
    pending_frames: list[str],
) -> tuple[int | None, list[str]]:
    if entry is not None and pending_count is not None:
        entry.caller_stacks.append((pending_frames[:], pending_count))
    return None, []


def parse_perf_report(lines: list[str]) -> list[PerfEntry]:
    entries: list[PerfEntry] = []
    current_entry: PerfEntry | None = None
    pending_count: int | None = None
    pending_frames: list[str] = []

    for raw_line in lines:
        line = raw_line.rstrip("\n")

        top_match = TOP_ENTRY_RE.match(line)
        if top_match is not None:
            pending_count, pending_frames = finish_pending_callchain(
                current_entry, pending_count, pending_frames
            )
            parsed_root = parse_root_entry(top_match.group("rest"))
            if parsed_root is None:
                current_entry = None
                continue
            dso, symbol = parsed_root
            current_entry = PerfEntry(
                root_frame=format_root_frame(dso, symbol),
                samples=int(top_match.group("samples")),
            )
            entries.append(current_entry)
            continue

        if current_entry is None:
            continue

        count_match = CALLCHAIN_COUNT_RE.match(line)
        if count_match is not None:
            pending_count, pending_frames = finish_pending_callchain(
                current_entry, pending_count, pending_frames
            )
            pending_count = int(count_match.group("count"))
            pending_frames = []
            continue

        if pending_count is not None:
            stripped = line.strip()
            if stripped:
                pending_frames.append(stripped)

    finish_pending_callchain(current_entry, pending_count, pending_frames)
    return entries


def build_speedscope_profile(entries: list[PerfEntry]) -> tuple[list[dict[str, str]], list[list[int]], list[int]]:
    frame_ids: dict[str, int] = {}
    frames: list[dict[str, str]] = []
    samples: list[list[int]] = []
    weights: list[int] = []

    def frame_id_for(name: str) -> int:
        frame_id = frame_ids.get(name)
        if frame_id is None:
            frame_id = len(frames)
            frame_ids[name] = frame_id
            frames.append({"name": name})
        return frame_id

    for entry in entries:
        caller_total = sum(weight for _stack, weight in entry.caller_stacks)
        for caller_stack, weight in entry.caller_stacks:
            stack = caller_stack + [entry.root_frame]
            samples.append([frame_id_for(frame) for frame in stack])
            weights.append(weight)

        remainder = entry.samples - caller_total
        if remainder > 0:
            samples.append([frame_id_for(entry.root_frame)])
            weights.append(remainder)

    return frames, samples, weights


def main() -> None:
    profile_name = sys.argv[1] if len(sys.argv) > 1 else "perf"
    entries = parse_perf_report(sys.stdin.readlines())
    frames, samples, weights = build_speedscope_profile(entries)
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
        "exporter": "soac perf_report_to_speedscope",
        "name": profile_name,
    }
    json.dump(result, sys.stdout)


if __name__ == "__main__":
    main()
