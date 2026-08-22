#!/usr/bin/env python3
"""Extracts the StyleInfo section body from a `.web.bundle`.

`fuzz_targets/template_style_info.rs` is handed a bare section body and builds
its own container around it, so the corpus has to hold bodies rather than whole
bundles. Reimplementing the framing here (rather than reusing the decoder)
keeps the seeding step from depending on the very code under test.
"""

from __future__ import annotations

import struct
import sys

MAGIC = struct.pack("<II", 0x41524453, 0x464F5257)
STYLE_INFO = 2


def extract(data: bytes) -> bytes | None:
    if not data.startswith(MAGIC):
        return None
    pos = len(MAGIC) + 4  # magic, then the version word
    while pos + 8 <= len(data):
        label, length = struct.unpack_from("<II", data, pos)
        pos += 8
        if pos + length > len(data):
            return None
        if label == STYLE_INFO:
            return data[pos : pos + length]
        pos += length
    return None


def main() -> int:
    source, destination = sys.argv[1], sys.argv[2]
    with open(source, "rb") as handle:
        section = extract(handle.read())
    if section is None:
        return 0
    with open(destination, "wb") as handle:
        handle.write(section)
    return 0


if __name__ == "__main__":
    sys.exit(main())
