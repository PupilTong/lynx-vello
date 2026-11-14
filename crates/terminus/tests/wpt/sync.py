#!/usr/bin/env python3
"""Vendors the web-platform-tests files `tests/wpt.rs` runs.

Usage: sync.py <path to a web-platform-tests checkout>

Copies, from the checkout into `tests/wpt/suite/` (replacing what is there):

- every HTML, CSS and image file under the suites `tests/wpt.rs` lists, so
  every test file is classified and counted, including the ones the runner
  reports as needing a script engine;
- `css/reference/`, the shared references;
- each local stylesheet, image or reference another directory provides that
  a copied file links to.

Scripts are not copied: the engine runs none. The checkout's commit is
recorded in `tests/wpt/WPT_COMMIT`, and upstream's license next to the files.
Run `WPT_UPDATE=1 cargo test -p terminus --test wpt` afterwards.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
SUITE = HERE / "suite"
SUITES = ["css/css-animations", "css/css-flexbox", "css/css-grid"]
SHARED = ["css/reference"]
KEEP = {".html", ".htm", ".xht", ".xhtml", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg"}
REFERENCE = re.compile(r"""(?:href|src)\s*=\s*["']?([^"' >]+)""", re.IGNORECASE)


def wanted(path: Path) -> bool:
    return path.suffix.lower() in KEEP


def copy(source_root: Path, relative: str) -> bool:
    source = source_root / relative
    if not source.is_file() or not wanted(source):
        return False
    target = SUITE / relative
    if target.exists():
        return False
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, target)
    return True


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    source_root = Path(sys.argv[1]).resolve()
    commit = subprocess.run(
        ["git", "-C", str(source_root), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()

    if SUITE.exists():
        shutil.rmtree(SUITE)
    SUITE.mkdir(parents=True)

    pending: list[str] = []
    for directory in SUITES + SHARED:
        for path in sorted((source_root / directory).rglob("*")):
            if path.is_file() and wanted(path):
                relative = path.relative_to(source_root).as_posix()
                if copy(source_root, relative):
                    pending.append(relative)

    # Files outside the copied directories that a copied file links to.
    while pending:
        relative = pending.pop()
        if Path(relative).suffix.lower() not in {".html", ".htm", ".xht", ".xhtml", ".css"}:
            continue
        text = (SUITE / relative).read_text(errors="ignore")
        for href in REFERENCE.findall(text):
            href = href.split("#")[0].split("?")[0]
            if not href or "://" in href or href.startswith(("data:", "mailto:")):
                continue
            if href.startswith("/"):
                linked = href.lstrip("/")
            else:
                linked = (Path(relative).parent / href).as_posix()
            parts: list[str] = []
            for part in linked.split("/"):
                if part == "..":
                    if parts:
                        parts.pop()
                elif part not in ("", "."):
                    parts.append(part)
            linked = "/".join(parts)
            if copy(source_root, linked):
                pending.append(linked)

    shutil.copyfile(source_root / "LICENSE.md", SUITE / "LICENSE.md")
    (HERE / "WPT_COMMIT").write_text(commit + "\n")
    count = sum(1 for path in SUITE.rglob("*") if path.is_file())
    print(f"vendored {count} files from web-platform-tests {commit}")


if __name__ == "__main__":
    main()
