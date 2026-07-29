#!/usr/bin/env python3
"""Turn a complete CSS-paint audit TSV into a checked difference registry.

The audit uses a temporary all-case Chromium reference directory:

    python3 crates/pulsar/tests/support/generate_css_paint_cases.py \
      --split-atlases output/playwright/css-paint/atlases \
      --reference-output /tmp/css-paint-references \
      --include-differences
    CSS_PAINT_REFERENCE_DIR=/tmp/css-paint-references \
      CSS_PAINT_AUDIT=/tmp/css-paint.tsv \
      FLASHBULB_REQUIRE_GPU=1 \
      cargo test -p pulsar --test css_atlas -- --include-ignored

This classifier is deliberately strict. Every one of the 1,000 cases must be
present exactly once, every mismatch must have one issue, and no matching case
may still be assigned to an issue. Normal regressions compare permitted UA
choices with native snapshots and skip the other differences. Audit mode
executes all 1,000 cases against Chromium, so reclassification is the explicit
path for removing a fixed case from the difference registry.
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable
from collections import Counter
from pathlib import Path

from generate_css_paint_cases import all_cases


ROOT = Path(__file__).resolve().parents[4]
DIFFERENCES = ROOT / "crates/pulsar/tests/css-paint-differences.tsv"
CASE_COUNT = 1_000

W3C_CORRECT_RASTER_OR_SAMPLING = "w3c-correct-raster-or-sampling"
W3C_CORRECT_UA_CHOICE = "w3c-correct-ua-choice"
W3C_GAP = "w3c-gap"
ROOT_ROLE_ORACLE_MISMATCH = "root-role-oracle-mismatch"
NON_W3C_COMPATIBILITY = "non-w3c-compatibility"

EXPECTED_DISPOSITION_COUNTS = {
    W3C_CORRECT_RASTER_OR_SAMPLING: 84,
    W3C_CORRECT_UA_CHOICE: 61,
    W3C_GAP: 170,
    ROOT_ROLE_ORACLE_MISMATCH: 22,
    NON_W3C_COMPATIBILITY: 19,
}


def cases(family: str, indices: Iterable[int]) -> set[str]:
    return {f"{family}-{index:03d}" for index in indices}


ISSUES: dict[str, set[str]] = {
    "css-gradient-multi-position-stops": (
        cases(
            "background-linear",
            (index for index in range(40) if index % 5 in {0, 2}),
        )
        | cases("background-radial", range(30))
        | cases("background-conic", range(30))
    ),
    "css-gradient-hard-stop-boundary-sampling": cases(
        "background-linear", range(1, 40, 5)
    ),
    "css-border-dash-dot-pattern": cases("border-style", range(18, 30))
    | cases("outline", range(2, 6)),
    "css-border-3d-light-face-color": cases("border-style", range(36, 60)),
    "css-double-border-rounded-corners": cases(
        "border-radius",
        (
            10,
            11,
            14,
            15,
            18,
            19,
            22,
            23,
            26,
            27,
            30,
            31,
            34,
            35,
            38,
            39,
        ),
    ),
    "css-outline-nonsolid-styles": cases("outline", range(6, 12)),
    "vello-chromium-edge-coverage": (
        {"shadow-multiple-007"}
        | cases("clip-circle", (0, 4, 8, 9, 10, 11, 12, 16, 17, 18, 19))
        | cases("mask-linear", (0, 1, 6, 7, 22, 23, 30, 31, 38, 39))
        | cases("mask-radial", (0, 5, 6, 7, 8, 10, 15, 16, 17, 18, 19))
        | cases("mask-position", (2, 4, 9))
        | cases("mask-boxes", range(10))
        | cases("text-background-clip", (4, 7))
    ),
    "css-atlas-negative-z-root-role-mismatch": (
        cases("paint-order", range(8))
        | cases("paint-order", range(60, 66))
        | cases("paint-order", range(80, 88))
    ),
    "css-position-static-grammar": cases("contain-paint", range(0, 20, 4)),
    "css-filter-brightness-over-one-approximation": {"filter-017"},
    "css-filter-blur-offscreen-pass": cases("filter", range(41, 50)),
    "stylo-lynx-clip-path-geometry-box-grammar": cases(
        "clip-inset", range(1, 20, 2)
    ),
    "pulsar-clip-inset-radius-percent-reference-box": cases("clip-inset", (14, 16)),
    "stylo-lynx-clip-polygon-grammar": cases("clip-polygon", range(10)),
    "pulsar-mask-multiple-layer-composite": (
        cases("mask-multiple", range(0, 4))
        | cases("mask-multiple", range(8, 12))
        | cases("mask-multiple", range(16, 20))
    ),
    "pulsar-mask-luminance-mode": cases("mask-multiple", range(4, 8))
    | cases("mask-multiple", range(12, 16)),
    "text-overflow-wrap-break-word-policy": cases("text-size-color", (18, 19))
    | cases("text-background-clip", (2, 6, 10, 14))
    | cases("text-wrap", (0, 10)),
    "stylo-lynx-repeating-gradient-grammar-scope": cases(
        "text-background-clip", range(16, 20)
    ),
    "stylo-lynx-text-shadow-list-grammar": cases("text-shadow", range(6, 10)),
    "pulsar-text-shadow-blur": cases("text-shadow", range(10, 20)),
    "css-text-decoration-auto-thickness-ua-choice": cases(
        "text-decoration", (2, 6, 10, 14, 18)
    ),
    "stylo-lynx-text-decoration-thickness-grammar": cases(
        "text-decoration", (3, 7, 11, 15, 19)
    ),
    "pulsar-text-stroke-join-geometry": cases("text-stroke", range(20))
    - {"text-stroke-006"},
    "css-text-subpixel-rasterization": (
        cases("text-size-color", (6, 7, 10, 11, 12, 13, 16, 17))
        | cases("text-metrics", (2, 3))
        | cases("text-wrap", range(1, 10))
        | cases("text-wrap", range(11, 20))
    ),
}

ISSUE_DISPOSITIONS = {
    "css-gradient-multi-position-stops": W3C_GAP,
    "css-gradient-hard-stop-boundary-sampling": W3C_CORRECT_RASTER_OR_SAMPLING,
    "css-border-dash-dot-pattern": W3C_CORRECT_UA_CHOICE,
    "css-border-3d-light-face-color": W3C_CORRECT_UA_CHOICE,
    "css-double-border-rounded-corners": W3C_CORRECT_UA_CHOICE,
    "css-outline-nonsolid-styles": W3C_GAP,
    "vello-chromium-edge-coverage": W3C_CORRECT_RASTER_OR_SAMPLING,
    "css-atlas-negative-z-root-role-mismatch": ROOT_ROLE_ORACLE_MISMATCH,
    "css-position-static-grammar": W3C_GAP,
    "css-filter-brightness-over-one-approximation": W3C_GAP,
    "css-filter-blur-offscreen-pass": W3C_GAP,
    "stylo-lynx-clip-path-geometry-box-grammar": W3C_GAP,
    "pulsar-clip-inset-radius-percent-reference-box": W3C_GAP,
    "stylo-lynx-clip-polygon-grammar": W3C_GAP,
    "pulsar-mask-multiple-layer-composite": W3C_GAP,
    "pulsar-mask-luminance-mode": W3C_GAP,
    "text-overflow-wrap-break-word-policy": W3C_GAP,
    "stylo-lynx-repeating-gradient-grammar-scope": W3C_GAP,
    "stylo-lynx-text-shadow-list-grammar": W3C_GAP,
    "pulsar-text-shadow-blur": W3C_GAP,
    "css-text-decoration-auto-thickness-ua-choice": W3C_CORRECT_UA_CHOICE,
    "stylo-lynx-text-decoration-thickness-grammar": W3C_GAP,
    "pulsar-text-stroke-join-geometry": NON_W3C_COMPATIBILITY,
    "css-text-subpixel-rasterization": W3C_CORRECT_RASTER_OR_SAMPLING,
}


def name_to_issue() -> dict[str, str]:
    issue_names = set(ISSUES)
    disposition_names = set(ISSUE_DISPOSITIONS)
    if issue_names != disposition_names:
        missing = sorted(issue_names - disposition_names)
        extra = sorted(disposition_names - issue_names)
        raise SystemExit(
            "issue disposition mismatch: "
            f"missing dispositions={missing}, unknown dispositions={extra}"
        )

    result: dict[str, str] = {}
    for issue, names in ISSUES.items():
        for name in names:
            previous = result.setdefault(name, issue)
            if previous != issue:
                raise SystemExit(
                    f"case {name!r} classified by both {previous!r} and {issue!r}"
                )

    disposition_counts = counts_by_disposition(
        Counter({issue: len(names) for issue, names in ISSUES.items()})
    )
    if disposition_counts != Counter(EXPECTED_DISPOSITION_COUNTS):
        raise SystemExit(
            "case disposition totals changed: "
            f"actual={dict(disposition_counts)}, "
            f"expected={EXPECTED_DISPOSITION_COUNTS}"
        )
    return result


def counts_by_disposition(issue_counts: Counter[str]) -> Counter[str]:
    return Counter(
        {
            disposition: sum(
                issue_counts[issue]
                for issue, issue_disposition in ISSUE_DISPOSITIONS.items()
                if issue_disposition == disposition
            )
            for disposition in EXPECTED_DISPOSITION_COUNTS
        }
    )


def classify(audit: Path, output: Path) -> None:
    expected_names = [case.name for case in all_cases()]
    if len(expected_names) != CASE_COUNT or len(set(expected_names)) != CASE_COUNT:
        raise SystemExit("generator did not produce 1,000 unique case names")

    rows: dict[int, tuple[str, str]] = {}
    seen_names: set[str] = set()
    for line_number, raw in enumerate(audit.read_text().splitlines(), 1):
        columns = raw.split("\t")
        if len(columns) != 8:
            raise SystemExit(f"{audit}:{line_number}: expected 8 TSV columns")
        index = int(columns[0])
        name = columns[1]
        int(columns[4])
        status = columns[7]
        if status not in {"match", "mismatch"}:
            raise SystemExit(f"{audit}:{line_number}: bad status {status!r}")
        if index in rows:
            raise SystemExit(f"{audit}:{line_number}: duplicate case {index:04}")
        if name in seen_names:
            raise SystemExit(f"{audit}:{line_number}: duplicate case name {name!r}")
        seen_names.add(name)
        rows[index] = (name, status)

    expected_indices = set(range(CASE_COUNT))
    if rows.keys() != expected_indices:
        missing = sorted(expected_indices - rows.keys())
        extra = sorted(rows.keys() - expected_indices)
        raise SystemExit(f"audit index mismatch: missing={missing}, extra={extra}")

    for index, expected_name in enumerate(expected_names):
        actual_name = rows[index][0]
        if actual_name != expected_name:
            raise SystemExit(
                f"audit case {index:04} is {actual_name!r}, expected {expected_name!r}"
            )

    classified = name_to_issue()
    lines = [
        "# Generated by support/classify_css_paint_audit.py.",
        "# Audited CSS-paint differences and their standards disposition.",
        "# Browser references: Chromium 150.0.7871.187, 128x128 CSS px, DPR 1.",
        "# Columns: case-name<TAB>issue.",
        "# UA-choice rows use committed native snapshots; all other rows are",
        "# ignored. Browser-match rows are absent from this registry.",
        "# Dispositions: 84 raster/sample, 61 UA choice, 170 W3C gap,",
        "# 22 root-role/oracle mismatch, 19 non-W3C compatibility.",
    ]
    counts: Counter[str] = Counter()
    for index in range(CASE_COUNT):
        name, status = rows[index]
        issue = classified.get(name)
        if status == "mismatch" and issue is None:
            raise SystemExit(f"{index:04} {name}: mismatch has no issue")
        if status == "match" and issue is not None:
            raise SystemExit(f"{index:04} {name}: now matches but still maps to {issue}")
        if issue is not None:
            lines.append(f"{name}\t{issue}")
            counts[issue] += 1

    disposition_counts = counts_by_disposition(counts)
    if disposition_counts != Counter(EXPECTED_DISPOSITION_COUNTS):
        raise SystemExit(
            "audit disposition totals changed: "
            f"actual={dict(disposition_counts)}, "
            f"expected={EXPECTED_DISPOSITION_COUNTS}"
        )

    output.write_text("\n".join(lines) + "\n")
    print(
        f"wrote {sum(counts.values())} differences across "
        f"{len(counts)} issues to {output}"
    )
    for issue in sorted(counts):
        print(f"{issue}\t{counts[issue]}")
    print("disposition totals:")
    for disposition, count in disposition_counts.items():
        print(f"{disposition}\t{count}")
    w3c_correct = (
        EXPECTED_DISPOSITION_COUNTS[W3C_CORRECT_RASTER_OR_SAMPLING]
        + EXPECTED_DISPOSITION_COUNTS[W3C_CORRECT_UA_CHOICE]
    )
    print(f"w3c-correct-total\t{w3c_correct}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("audit", type=Path)
    parser.add_argument("--output", type=Path, default=DIFFERENCES)
    args = parser.parse_args()
    classify(args.audit, args.output)


if __name__ == "__main__":
    main()
