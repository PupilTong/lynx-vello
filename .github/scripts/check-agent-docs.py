#!/usr/bin/env python3
"""Fails when the agent guide outgrows its limits or points at nothing.

The agent guide is the root `AGENTS.md` plus one `AGENTS.md` per crate or
package and the topic files under `crates/*/docs/`. Codex loads at most 32 KiB
of project documentation at session start (`project_doc_max_bytes`), so it does
not load all of a root file beyond that, and every byte of the file fills the
context of every Claude Code session. The root therefore holds only what every session
needs, and the rest is read per crate when that crate is touched. This keeps
that split from eroding:

1. the root `AGENTS.md` stays within 32 KiB;
2. every other guide file stays within 64 KiB (a file that reaches it is split
   by topic, not exempted);
3. every `crates/<name>` with a `Cargo.toml` has its own `AGENTS.md`, and the
   root's Workspace map names `crates/<name>`;
4. every relative Markdown link in the guide and in `CLAUDE.md` resolves.

The script walks the filesystem rather than asking Git, so it also checks files
that are not staged yet.

Usage:  python3 .github/scripts/check-agent-docs.py
"""

from __future__ import annotations

import os
import re
import sys
from pathlib import Path

ROOT_LIMIT = 32 * 1024
OTHER_LIMIT = 64 * 1024
SKIP_DIRS = {"vendor", "node_modules", "target"}

LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
FENCE = re.compile(r"^\s*(```|~~~)")


def guide_files(root: Path) -> list[Path]:
    found: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(
            d for d in dirnames if d not in SKIP_DIRS and not d.startswith(".")
        )
        if "AGENTS.md" in filenames:
            found.append(Path(dirpath) / "AGENTS.md")
    for crate in sorted((root / "crates").iterdir()):
        docs = crate / "docs"
        if docs.is_dir():
            found.extend(sorted(docs.glob("*.md")))
    return found


def links(path: Path) -> list[tuple[int, str]]:
    out: list[tuple[int, str]] = []
    in_fence = False
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        # Inline code spans can hold bracket-paren text that is not a link.
        text = re.sub(r"`[^`]*`", "", line)
        for match in LINK.finditer(text):
            out.append((number, match.group(1)))
    return out


def main() -> int:
    root = Path.cwd()
    root_doc = root / "AGENTS.md"
    if not root_doc.is_file():
        print("check-agent-docs: run from the repository root (no AGENTS.md here)")
        return 1

    failures: list[str] = []
    files = guide_files(root)

    for path in files:
        rel = path.relative_to(root)
        size = path.stat().st_size
        limit = ROOT_LIMIT if path == root_doc else OTHER_LIMIT
        print(f"{size:7d} / {limit:5d} bytes  {rel}")
        if size > limit:
            failures.append(
                f"{rel} is {size} bytes, over its {limit}-byte limit; split it by "
                "topic (as crates/bobcat-core/docs/ is) instead of raising the limit"
            )

    workspace_map = root_doc.read_text(encoding="utf-8")
    for crate in sorted((root / "crates").iterdir()):
        if not (crate / "Cargo.toml").is_file():
            continue
        rel = crate.relative_to(root).as_posix()
        if not (crate / "AGENTS.md").is_file():
            failures.append(f"{rel} has a Cargo.toml but no {rel}/AGENTS.md")
        if f"`{rel}`" not in workspace_map:
            failures.append(f"the root AGENTS.md's Workspace map does not name `{rel}`")

    checked = list(files)
    claude = root / "CLAUDE.md"
    if claude.is_file():
        checked.append(claude)
    for path in checked:
        for number, target in links(path):
            if re.match(r"^(https?:|mailto:|#)", target):
                continue
            file_part = target.split("#", 1)[0]
            if not file_part:
                continue
            resolved = (path.parent / file_part).resolve()
            if not resolved.exists():
                failures.append(
                    f"{path.relative_to(root)}:{number}: link target {target!r} "
                    "does not exist"
                )

    if failures:
        print()
        for failure in failures:
            print(f"error: {failure}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
