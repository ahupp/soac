#!/usr/bin/env python3
"""Inventory SOAC crates, local crate deps, CLI tools, and Codex skills."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parents[4]


@dataclass(frozen=True)
class CrateInfo:
    name: str
    path: Path
    local_deps: tuple[str, ...]


@dataclass(frozen=True)
class BinInfo:
    package: str
    name: str
    path: Path


@dataclass(frozen=True)
class SkillInfo:
    name: str
    path: Path
    description: str


def rel(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(ROOT))
    except ValueError:
        return str(path)


def cargo_metadata() -> dict | None:
    try:
        proc = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            cwd=ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError) as err:
        print(f"; cargo metadata unavailable: {err}", file=sys.stderr)
        return None
    return json.loads(proc.stdout)


def crate_infos_from_metadata(metadata: dict) -> tuple[list[CrateInfo], list[BinInfo]]:
    package_by_id = {package["id"]: package for package in metadata["packages"]}
    workspace_ids = set(metadata.get("workspace_members", []))
    workspace_names = {
        package_by_id[package_id]["name"]
        for package_id in workspace_ids
        if package_id in package_by_id
    }
    crates: list[CrateInfo] = []
    bins: list[BinInfo] = []
    for package_id in sorted(workspace_ids, key=lambda item: package_by_id[item]["name"]):
        package = package_by_id[package_id]
        manifest_path = Path(package["manifest_path"])
        if not rel(manifest_path).startswith("crates/"):
            continue
        local_deps = tuple(
            sorted(
                dep["name"]
                for dep in package.get("dependencies", [])
                if dep["name"] in workspace_names
            )
        )
        crates.append(CrateInfo(package["name"], manifest_path.parent, local_deps))
        for target in package.get("targets", []):
            if "bin" in target.get("kind", []):
                bins.append(BinInfo(package["name"], target["name"], Path(target["src_path"])))
    return crates, sorted(bins, key=lambda item: (item.package, item.name))


def crate_infos_from_toml() -> tuple[list[CrateInfo], list[BinInfo]]:
    crates: list[CrateInfo] = []
    bins: list[BinInfo] = []
    crate_names = set()
    manifests = sorted((ROOT / "crates").glob("*/Cargo.toml"))
    raw = {}
    for manifest in manifests:
        data = tomllib.loads(manifest.read_text())
        name = data["package"]["name"]
        crate_names.add(name)
        raw[manifest] = data
    for manifest, data in raw.items():
        name = data["package"]["name"]
        deps = []
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for dep_name in data.get(section, {}):
                if dep_name in crate_names:
                    deps.append(dep_name)
        crates.append(CrateInfo(name, manifest.parent, tuple(sorted(set(deps)))))
        main = manifest.parent / "src" / "main.rs"
        if main.exists():
            bins.append(BinInfo(name, name, main))
        for bin_src in sorted((manifest.parent / "src" / "bin").glob("*.rs")):
            bins.append(BinInfo(name, bin_src.stem, bin_src))
    return sorted(crates, key=lambda item: item.name), sorted(bins, key=lambda item: (item.package, item.name))


def frontmatter_value(lines: list[str], key: str) -> str:
    prefix = f"{key}:"
    for line in lines:
        if line.startswith(prefix):
            return line[len(prefix) :].strip().strip('"')
    return ""


def skill_infos() -> list[SkillInfo]:
    skills = []
    for skill_file in sorted((ROOT / ".codex" / "skills").glob("*/SKILL.md")):
        text = skill_file.read_text()
        lines = text.splitlines()
        if lines[:1] == ["---"]:
            try:
                end = lines[1:].index("---") + 1
                fm = lines[1:end]
            except ValueError:
                fm = []
        else:
            fm = []
        name = frontmatter_value(fm, "name") or skill_file.parent.name
        description = frontmatter_value(fm, "description")
        skills.append(SkillInfo(name, skill_file, description))
    return skills


def markdown_table(headers: tuple[str, ...], rows: list[tuple[str, ...]]) -> str:
    out = ["| " + " | ".join(headers) + " |"]
    out.append("| " + " | ".join("---" for _ in headers) + " |")
    for row in rows:
        out.append("| " + " | ".join(cell.replace("\n", " ") for cell in row) + " |")
    return "\n".join(out)


def render_markdown(crates: list[CrateInfo], bins: list[BinInfo], skills: list[SkillInfo]) -> str:
    crate_rows = [
        (f"`{crate.name}`", f"`{rel(crate.path)}`", ", ".join(f"`{dep}`" for dep in crate.local_deps) or "-")
        for crate in crates
    ]
    bin_rows = [
        (f"`{item.name}`", f"`{item.package}`", f"`{rel(item.path)}`")
        for item in bins
    ]
    skill_rows = [
        (f"`{skill.name}`", f"`{rel(skill.path)}`", skill.description or "-")
        for skill in skills
    ]
    sections = [
        "# SOAC Selfdoc Inventory",
        "",
        "## Crates",
        "",
        markdown_table(("Crate", "Path", "Local deps"), crate_rows),
        "",
        "## CLI Tools",
        "",
        markdown_table(("Tool", "Package", "Source"), bin_rows),
        "",
        "## Codex Skills",
        "",
        markdown_table(("Skill", "Path", "Description"), skill_rows),
        "",
    ]
    return "\n".join(sections)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Inventory SOAC crates, CLI tools, and Codex skills for self-documentation."
    )
    parser.add_argument("--out", type=Path, help="Write markdown inventory to this path.")
    args = parser.parse_args()

    crates, bins = crate_infos_from_toml()
    metadata = cargo_metadata()
    if metadata is not None:
        _, metadata_bins = crate_infos_from_metadata(metadata)
        merged_bins = {(item.package, item.name, item.path): item for item in bins}
        for item in metadata_bins:
            merged_bins[(item.package, item.name, item.path)] = item
        bins = sorted(merged_bins.values(), key=lambda item: (item.package, item.name))
    output = render_markdown(crates, bins, skill_infos())
    if args.out:
        out = args.out
        if not out.is_absolute():
            out = ROOT / out
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(output)
        print(rel(out))
    else:
        print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
