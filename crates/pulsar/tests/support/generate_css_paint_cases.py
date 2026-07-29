#!/usr/bin/env python3
"""Generate the 1,000-case pure-div CSS paint screenshot matrix.

The generated Rust is consumed by ``tests/css_atlas.rs``. Chromium matches own
browser PNGs, W3C-correct raster/sampling differences and permitted UA choices
own native Pulsar/Parley snapshots, and the remaining audited differences are
``#[ignore]``. Every difference retains a standalone HTML reproduction.

The optional HTML output contains forty 5x5 browser atlases; every cell is an
isolated 128x128 iframe.

Reference images are intentionally browser-owned.  Generate the atlas PNGs
with Playwright CLI, inspect them, then split them into per-case goldens:

    python3 crates/pulsar/tests/support/generate_css_paint_cases.py \
      --html-output output/playwright/css-paint
    python3 -m http.server 8765 --bind 127.0.0.1
    node crates/pulsar/tests/support/capture_css_paint_references.mjs \
      http://127.0.0.1:8765 output/playwright/css-paint/atlases
    python3 crates/pulsar/tests/support/generate_css_paint_cases.py \
      --split-atlases output/playwright/css-paint/atlases

For a full audit, split all references into a disposable directory:

    python3 crates/pulsar/tests/support/generate_css_paint_cases.py \
      --split-atlases output/playwright/css-paint/atlases \
      --reference-output /tmp/css-paint-references --include-differences

The split step uses Pillow only as a maintainer tool.  The Rust test suite
decodes committed PNGs through ``flashbulb`` and has no Python dependency.
"""

from __future__ import annotations

import argparse
import html
import re
from collections import Counter
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Iterable


ROOT = Path(__file__).resolve().parents[4]
GENERATED = ROOT / "crates/pulsar/tests/generated/css_paint_cases.rs"
BROWSER_GOLDENS = ROOT / "crates/pulsar/tests/screenshots/css-paint"
NATIVE_GOLDENS = ROOT / "crates/pulsar/tests/screenshots/css-paint-native"
DIFFERENCE_FIXTURES = ROOT / "crates/pulsar/tests/fixtures/css-paint-differences"
DIFFERENCES = ROOT / "crates/pulsar/tests/css-paint-differences.tsv"

CASE_COUNT = 1_000
CELL_SIZE = 128
GRID = 5
CASES_PER_SHARD = GRID * GRID
SHARD_COUNT = CASE_COUNT // CASES_PER_SHARD
ATLAS_SIZE = CELL_SIZE * GRID


@dataclass(frozen=True)
class Case:
    name: str
    category: str
    source: str
    fragment: str


class DifferenceKind(Enum):
    RASTER_OR_SAMPLING = "w3c-correct-raster-or-sampling"
    UA_CHOICE = "w3c-correct-ua-choice"
    W3C_GAP = "w3c-gap"
    NON_W3C_COMPATIBILITY = "non-w3c-compatibility"


@dataclass(frozen=True)
class Difference:
    issue: str
    kind: DifferenceKind


NATIVE_SNAPSHOT_KINDS = {
    DifferenceKind.RASTER_OR_SAMPLING,
    DifferenceKind.UA_CHOICE,
}

RASTER_OR_SAMPLING_ISSUES = {
    "css-gradient-hard-stop-boundary-sampling",
    "vello-chromium-edge-coverage",
    "css-text-subpixel-rasterization",
}

UA_CHOICE_ISSUES = {
    "css-border-dash-dot-pattern",
    "css-border-3d-light-face-color",
    "css-double-border-rounded-corners",
    "css-text-decoration-auto-thickness-ua-choice",
}

W3C_GAP_ISSUES = {
    "css-gradient-multi-position-stops",
    "css-outline-nonsolid-styles",
    "css-position-static-grammar",
    "css-filter-brightness-over-one-approximation",
    "css-filter-blur-offscreen-pass",
    "stylo-lynx-clip-path-geometry-box-grammar",
    "pulsar-clip-inset-radius-percent-reference-box",
    "stylo-lynx-clip-polygon-grammar",
    "pulsar-mask-multiple-layer-composite",
    "pulsar-mask-luminance-mode",
    "text-overflow-wrap-break-word-policy",
    "stylo-lynx-repeating-gradient-grammar-scope",
    "stylo-lynx-text-shadow-list-grammar",
    "pulsar-text-shadow-blur",
    "stylo-lynx-text-decoration-thickness-grammar",
}

def stage(content: str, extra: str = "") -> str:
    return (
        '<div style="position:relative;display:flex;width:128px;height:128px;'
        f'overflow:hidden;background:#ffffff;box-sizing:border-box;{extra}">'
        f"{content}</div>"
    )


def box(style: str, content: str = "") -> str:
    # hughie does not implement flow/block layout yet: `display: flow`
    # intentionally lowers to a leaf and hides descendants.  Flex gives every
    # structural test div a real child-formatting context in both engines.
    return f'<div style="display:flex;{style}">{content}</div>'


def named(category: str, ordinal: int) -> str:
    return f"{category}-{ordinal:03d}"


def backgrounds() -> list[Case]:
    result: list[Case] = []
    colors = ["#ef4444", "#22c55e", "#3b82f6", "#a855f7", "#f59e0b"]
    opacities = ["1", ".8", ".55", ".3"]
    for index, (color, opacity) in enumerate(
        (pair for color in colors for pair in ((color, opacity) for opacity in opacities))
    ):
        target = box(
            "position:absolute;left:16px;top:20px;width:96px;height:88px;"
            f"background-color:{color};opacity:{opacity}"
        )
        result.append(
            Case(
                named("background-solid", index),
                "background",
                "wpt/css/css-backgrounds/",
                stage(target, "background:#e2e8f0;"),
            )
        )

    linear_stops = [
        "#ef4444 0 25%,#22c55e 25% 50%,#3b82f6 50% 75%,#f59e0b 75%",
        "#111827 0%,#111827 45%,#f8fafc 45%,#f8fafc 55%,#111827 55%",
        "transparent 0 20%,#8b5cf6 20% 80%,transparent 80%",
        "#06b6d4 10%,#f43f5e 55%,#facc15 90%",
        "#0f172a 0%,#ffffff 50%,#0f172a 100%",
    ]
    for angle in range(0, 360, 45):
        for stop_index, stops in enumerate(linear_stops):
            index = (angle // 45) * len(linear_stops) + stop_index
            target = box(
                "position:absolute;left:12px;top:16px;width:104px;height:96px;"
                f"background:#dc2626 linear-gradient({angle}deg,{stops})"
            )
            result.append(
                Case(
                    named("background-linear", index),
                    "background",
                    "wpt/css/css-backgrounds/gradients/",
                    stage(target, "background:#cbd5e1;"),
                )
            )

    radial_positions = ["center", "left top", "right top", "left bottom", "75% 40%"]
    radial_shapes = [
        "circle closest-side",
        "circle farthest-corner",
        "ellipse farthest-side",
    ]
    radial_stops = [
        "#22c55e 0 35%,#2563eb 36% 70%,#ef4444 71%",
        "transparent 0 25%,#f59e0b 26% 65%,#7c3aed 66%",
    ]
    for position_index, position in enumerate(radial_positions):
        for shape_index, shape in enumerate(radial_shapes):
            for stop_index, stops in enumerate(radial_stops):
                index = (
                    position_index * len(radial_shapes) * len(radial_stops)
                    + shape_index * len(radial_stops)
                    + stop_index
                )
                target = box(
                    "position:absolute;left:12px;top:12px;width:104px;height:104px;"
                    f"background:#dc2626 radial-gradient({shape} at {position},{stops})"
                )
                result.append(
                    Case(
                        named("background-radial", index),
                        "background",
                        "wpt/css/css-backgrounds/gradients/",
                        stage(target),
                    )
                )

    conic_positions = ["center", "25% 25%", "75% 25%", "25% 75%", "70% 60%"]
    for angle_index, angle in enumerate([0, 30, 75, 120, 210, 300]):
        for position_index, position in enumerate(conic_positions):
            index = angle_index * len(conic_positions) + position_index
            target = box(
                "position:absolute;left:12px;top:12px;width:104px;height:104px;"
                f"background:conic-gradient(from {angle}deg at {position},"
                "#ef4444 0 25%,#22c55e 0 50%,#3b82f6 0 75%,#f59e0b 0)"
            )
            result.append(
                Case(
                    named("background-conic", index),
                    "background",
                    "wpt/css/css-backgrounds/gradients/",
                    stage(target),
                )
            )
    assert len(result) == 120
    return result


def borders() -> list[Case]:
    result: list[Case] = []
    styles = [
        "none",
        "hidden",
        "solid",
        "dashed",
        "dotted",
        "double",
        "groove",
        "ridge",
        "inset",
        "outset",
    ]
    for style_index, border_style in enumerate(styles):
        for width_index, width in enumerate([2, 6, 12]):
            for color_index, color in enumerate(["#2563eb", "#e11d48"]):
                index = style_index * 6 + width_index * 2 + color_index
                target = box(
                    "position:absolute;left:18px;top:22px;width:92px;height:84px;"
                    "box-sizing:border-box;background:#22c55e;"
                    f"border:{width}px {border_style} {color}"
                )
                result.append(
                    Case(
                        named("border-style", index),
                        "border",
                        "wpt/css/css-borders/",
                        stage(target, "background:#fef2f2;"),
                    )
                )

    radii = [
        "0",
        "8px",
        "24px",
        "50%",
        "12px 28px",
        "8px 24px 36px",
        "4px 20px 36px 12px",
        "40px / 12px",
        "12px 36px / 32px 8px",
        "70px 10px 40px 4px / 20px 60px 8px 36px",
    ]
    for radius_index, radius in enumerate(radii):
        for style_index, border_style in enumerate(["solid", "double"]):
            for color_index, color in enumerate(["#7c3aed", "#0891b2"]):
                index = radius_index * 4 + style_index * 2 + color_index
                target = box(
                    "position:absolute;left:14px;top:18px;width:100px;height:92px;"
                    "box-sizing:border-box;background:#f59e0b;border-width:8px;"
                    f"border-style:{border_style};border-color:{color};border-radius:{radius}"
                )
                result.append(
                    Case(
                        named("border-radius", index),
                        "border",
                        "wpt/css/css-borders/border-radius/",
                        stage(target, "background:#e0f2fe;"),
                    )
                )
    assert len(result) == 100
    return result


def shadows_and_outlines() -> list[Case]:
    result: list[Case] = []
    offsets = [(-10, -6), (-6, 8), (0, 8), (8, 0), (10, 10)]
    blur_spread = [(0, 0), (4, 0), (8, 0), (6, 4), (12, -2), (16, 3)]
    for offset_index, (x, y) in enumerate(offsets):
        for shape_index, (blur, spread) in enumerate(blur_spread):
            index = offset_index * len(blur_spread) + shape_index
            target = box(
                "position:absolute;left:30px;top:32px;width:68px;height:64px;"
                "background:#22c55e;border-radius:10px;"
                f"box-shadow:{x}px {y}px {blur}px {spread}px rgba(37,99,235,.8)"
            )
            result.append(
                Case(
                    named("shadow-outset", index),
                    "shadow-outline",
                    "wpt/css/css-backgrounds/box-shadow/",
                    stage(target, "background:#fee2e2;"),
                )
            )

    for index in range(20):
        x = [-8, -3, 0, 4, 8][index % 5]
        y = [-6, 0, 5, 9][(index // 5) % 4]
        blur = [0, 4, 10, 16][index % 4]
        spread = [0, 3, -2][index % 3]
        target = box(
            "position:absolute;left:20px;top:20px;width:88px;height:88px;"
            "background:#f8fafc;border:4px solid #0f172a;border-radius:14px;"
            f"box-shadow:inset {x}px {y}px {blur}px {spread}px #f43f5e"
        )
        result.append(
            Case(
                named("shadow-inset", index),
                "shadow-outline",
                "wpt/css/css-backgrounds/box-shadow/",
                stage(target, "background:#cbd5e1;"),
            )
        )

    for index in range(10):
        first = 2 + index % 4
        second = 5 + (index * 3) % 9
        target = box(
            "position:absolute;left:28px;top:30px;width:72px;height:68px;"
            "background:#f59e0b;"
            f"box-shadow:{first}px {first}px 0 #7c3aed,"
            f"-{second}px {second}px {index % 6}px rgba(6,182,212,.8),"
            f"0 -{first + 2}px {index % 5}px rgba(239,68,68,.7)"
        )
        result.append(
            Case(
                named("shadow-multiple", index),
                "shadow-outline",
                "wpt/css/css-backgrounds/box-shadow/",
                stage(target),
            )
        )

    outline_styles = [
        "solid",
        "dashed",
        "dotted",
        "double",
        "groove",
        "ridge",
        "inset",
        "outset",
        "none",
        "hidden",
    ]
    for style_index, outline_style in enumerate(outline_styles):
        for width_index, width in enumerate([3, 9]):
            index = style_index * 2 + width_index
            target = box(
                "position:absolute;left:28px;top:28px;width:72px;height:72px;"
                "background:#3b82f6;border-radius:12px;"
                f"outline:{width}px {outline_style} #e11d48"
            )
            result.append(
                Case(
                    named("outline", index),
                    "shadow-outline",
                    "wpt/css/CSS2/ui/outline/",
                    stage(target, "background:#dcfce7;"),
                )
            )
    assert len(result) == 80
    return result


def paint_order() -> list[Case]:
    result: list[Case] = []
    z_pairs = [
        (-2, -1),
        (-1, -2),
        (-1, 0),
        (0, -1),
        (0, 1),
        (1, 0),
        (1, 2),
        (2, 1),
        (1, 999),
        (999, 1),
    ]
    for mode in range(5):
        for pair_index, (first_z, second_z) in enumerate(z_pairs):
            for reverse in [False, True]:
                index = mode * 20 + pair_index * 2 + int(reverse)
                first = box(
                    "position:absolute;left:20px;top:20px;width:72px;height:72px;"
                    f"background:#ef4444;z-index:{first_z}"
                )
                second = box(
                    "position:absolute;left:42px;top:42px;width:68px;height:68px;"
                    f"background:#22c55e;z-index:{second_z}"
                )
                ordered = second + first if reverse else first + second
                if mode == 0:
                    content = ordered
                elif mode == 1:
                    content = box(
                        "position:absolute;inset:0;opacity:.72;",
                        ordered,
                    )
                elif mode == 2:
                    content = box(
                        "position:absolute;inset:0;transform:translate(0);",
                        ordered,
                    )
                elif mode == 3:
                    nested = box(
                        "position:absolute;left:12px;top:12px;width:104px;height:104px;"
                        f"background:#3b82f6;z-index:{first_z}",
                        box(
                            "position:absolute;left:28px;top:28px;width:70px;height:70px;"
                            f"background:#ef4444;z-index:{second_z}"
                        ),
                    )
                    sibling = box(
                        "position:absolute;left:44px;top:44px;width:64px;height:64px;"
                        "background:#22c55e;z-index:0"
                    )
                    content = sibling + nested if reverse else nested + sibling
                else:
                    context = box(
                        "position:absolute;left:10px;top:10px;width:106px;height:106px;"
                        f"opacity:.85;z-index:{first_z}",
                        first + second,
                    )
                    cover = box(
                        "position:absolute;left:36px;top:36px;width:76px;height:76px;"
                        f"background:#f59e0b;z-index:{second_z}"
                    )
                    content = cover + context if reverse else context + cover
                result.append(
                    Case(
                        named("paint-order", index),
                        "paint-order",
                        "wpt/css/CSS2/zindex/",
                        stage(content, "background:#e2e8f0;"),
                    )
                )
    assert len(result) == 100
    return result


def overflow_and_containment() -> list[Case]:
    result: list[Case] = []
    overflow_values = ["visible", "hidden"]
    radii = ["0", "12px", "28px", "50%"]
    child_offsets = [(-18, 18), (28, -20), (55, 52), (-12, -12), (34, 34)]
    for overflow_index, overflow in enumerate(overflow_values):
        for radius_index, radius in enumerate(radii):
            for offset_index, (left, top) in enumerate(child_offsets):
                index = overflow_index * 20 + radius_index * 5 + offset_index
                child = box(
                    f"position:absolute;left:{left}px;top:{top}px;width:76px;height:76px;"
                    "background:#ef4444;transform:rotate(18deg)"
                )
                parent = box(
                    "position:absolute;left:28px;top:28px;width:72px;height:72px;"
                    "background:#22c55e;border:5px solid #2563eb;"
                    f"box-sizing:border-box;overflow:{overflow};border-radius:{radius}",
                    child,
                )
                result.append(
                    Case(
                        named("overflow", index),
                        "overflow-containment",
                        "wpt/css/css-overflow/",
                        stage(parent, "background:#fef2f2;"),
                    )
                )

    escape_geometries = [
        (54, 18, 12, 10),
        (30, -22, -10, 12),
        (-34, 24, 14, -8),
        (58, 50, -12, -10),
        (10, 56, 8, 14),
    ]
    for index in range(20):
        contain = "paint" if index % 2 == 0 else "none"
        positioning = [
            "position:static",
            "position:relative",
            "position:absolute",
            "position:relative;z-index:0",
        ][index % 4]
        left, top, shadow_x, shadow_y = escape_geometries[index // 4]
        child = box(
            f"position:absolute;left:{left}px;top:{top}px;width:72px;height:72px;"
            f"background:#f43f5e;box-shadow:{shadow_x}px {shadow_y}px 0 #7c3aed"
        )
        parent = box(
            "position:absolute;left:18px;top:24px;width:76px;height:76px;"
            f"background:#22c55e;contain:{contain};{positioning}",
            child,
        )
        result.append(
            Case(
                named("contain-paint", index),
                "overflow-containment",
                "wpt/css/css-contain/",
                stage(parent, "background:#dbeafe;"),
            )
        )

    for index in range(20):
        outer_overflow = "hidden" if index % 2 else "visible"
        inner_overflow = "hidden" if (index // 2) % 2 else "visible"
        outer_radius = [0, 8, 20, 36, 50][index % 5]
        child = box(
            "position:absolute;left:28px;top:28px;width:70px;height:70px;"
            "background:#f59e0b;transform:translate(18px,12px) rotate(12deg)"
        )
        inner = box(
            "position:absolute;left:18px;top:18px;width:76px;height:76px;"
            f"background:#22c55e;overflow:{inner_overflow};border-radius:18px",
            child,
        )
        outer = box(
            "position:absolute;left:10px;top:10px;width:108px;height:108px;"
            f"background:#3b82f6;overflow:{outer_overflow};"
            f"border-radius:{outer_radius}px",
            inner,
        )
        result.append(
            Case(
                named("overflow-chain", index),
                "overflow-containment",
                "wpt/css/css-overflow/",
                stage(outer),
            )
        )
    assert len(result) == 80
    return result


def transforms() -> list[Case]:
    result: list[Case] = []
    for index in range(20):
        x = -24 + (index % 5) * 12
        y = -20 + (index // 5) * 12
        target = box(
            "position:absolute;left:30px;top:34px;width:68px;height:60px;"
            "background:#22c55e;border:5px solid #1d4ed8;"
            f"transform:translate({x}px,{y}px)"
        )
        result.append(
            Case(
                named("transform-translate", index),
                "transform",
                "wpt/css/css-transforms/",
                stage(target, "background:#fee2e2;"),
            )
        )

    angles = [-180, -135, -90, -45, -20, 0, 20, 45, 90, 135]
    for angle_index, angle in enumerate(angles):
        for radius_index, radius in enumerate([0, 18]):
            index = angle_index * 2 + radius_index
            target = box(
                "position:absolute;left:30px;top:34px;width:68px;height:60px;"
                "background:#f59e0b;border:5px solid #7c3aed;"
                f"border-radius:{radius}px;transform:rotate({angle}deg)"
            )
            result.append(
                Case(
                    named("transform-rotate", index),
                    "transform",
                    "wpt/css/css-transforms/",
                    stage(target, "background:#dbeafe;"),
                )
            )

    scales = [
        (0.25, 0.5),
        (0.5, 1),
        (0.75, 1.25),
        (1, 1),
        (1.25, 0.75),
        (1.5, 1.5),
        (2, 0.5),
        (-0.5, 1),
        (1, -0.75),
        (-1, -1),
    ]
    for scale_index, (sx, sy) in enumerate(scales):
        for origin_index, origin in enumerate(["center", "left top"]):
            index = scale_index * 2 + origin_index
            target = box(
                "position:absolute;left:32px;top:36px;width:64px;height:56px;"
                "background:#06b6d4;border:4px solid #be123c;"
                f"transform-origin:{origin};transform:scale({sx},{sy})"
            )
            result.append(
                Case(
                    named("transform-scale", index),
                    "transform",
                    "wpt/css/css-transforms/",
                    stage(target, "background:#fef3c7;"),
                )
            )

    for index in range(20):
        x = [-35, -20, -10, 10, 25][index % 5]
        y = [-25, -10, 10, 30][(index // 5) % 4]
        target = box(
            "position:absolute;left:30px;top:34px;width:68px;height:60px;"
            "background:#a855f7;border:4px solid #15803d;"
            f"transform:skew({x}deg,{y}deg)"
        )
        result.append(
            Case(
                named("transform-skew", index),
                "transform",
                "wpt/css/css-transforms/",
                stage(target, "background:#e0f2fe;"),
            )
        )

    origins = [
        "0 0",
        "100% 0",
        "0 100%",
        "100% 100%",
        "50% 50%",
        "25% 75%",
        "12px 48px",
        "80px 14px",
        "left center",
        "right bottom",
    ]
    for origin_index, origin in enumerate(origins):
        for nested in [False, True]:
            index = origin_index * 2 + int(nested)
            target = box(
                "position:absolute;left:28px;top:30px;width:72px;height:68px;"
                "background:#ef4444;border:5px solid #22c55e;"
                f"transform-origin:{origin};transform:rotate(28deg) translate(8px,-4px)"
            )
            content = (
                box(
                    "position:absolute;inset:8px;transform:scale(.9) rotate(-12deg);",
                    target,
                )
                if nested
                else target
            )
            result.append(
                Case(
                    named("transform-origin", index),
                    "transform",
                    "wpt/css/css-transforms/",
                    stage(content, "background:#dbeafe;"),
                )
            )
    assert len(result) == 100
    return result


def filters_and_opacity() -> list[Case]:
    result: list[Case] = []
    for index, opacity in enumerate(
        [0, 0.05, 0.1, 0.2, 0.35, 0.5, 0.65, 0.8, 0.95, 1] * 2
    ):
        reverse = index >= 10
        first = box(
            "position:absolute;left:20px;top:24px;width:76px;height:76px;"
            "background:#ef4444"
        )
        second = box(
            "position:absolute;left:44px;top:46px;width:66px;height:66px;"
            "background:#22c55e"
        )
        children = second + first if reverse else first + second
        group = box(f"position:absolute;inset:0;opacity:{opacity}", children)
        result.append(
            Case(
                named("opacity-group", index),
                "filter-opacity",
                "wpt/css/css-color/opacity/",
                stage(group, "background:#3b82f6;"),
            )
        )

    filter_families = [
        ("grayscale", ["0", ".2", ".5", ".8", "1", "25%", "50%", "75%", "100%", "none"]),
        ("brightness", ["0", ".25", ".5", ".75", "1", "1.25", "1.5", "2", "50%", "150%"]),
        ("contrast", ["0", ".25", ".5", ".75", "1", "1.25", "1.5", "2", "50%", "150%"]),
        ("saturate", ["0", ".25", ".5", ".75", "1", "1.25", "1.5", "2", "50%", "150%"]),
        ("blur", ["0px", "1px", "2px", "3px", "4px", "5px", "7px", "9px", "12px", "16px"]),
    ]
    for family_index, (family, values) in enumerate(filter_families):
        for value_index, value in enumerate(values):
            index = family_index * 10 + value_index
            filter_value = "none" if value == "none" else f"{family}({value})"
            target = box(
                "position:absolute;left:20px;top:22px;width:88px;height:84px;"
                "background:linear-gradient(45deg,#ef4444 0%,#22c55e 50%,#2563eb 100%);"
                f"filter:{filter_value}"
            )
            result.append(
                Case(
                    named("filter", index),
                    "filter-opacity",
                    "wpt/css/filter-effects/",
                    stage(target, "background:#f8fafc;"),
                )
            )

    for index in range(10):
        opacity = [0.25, 0.5, 0.75, 1][index % 4]
        filter_value = [
            "grayscale(.6)",
            "brightness(.7)",
            "contrast(.6)",
            "saturate(.4)",
            "blur(4px)",
        ][index % 5]
        child = box(
            "position:absolute;left:24px;top:26px;width:82px;height:78px;"
            "background:linear-gradient(90deg,#ef4444,#22c55e,#3b82f6);"
            f"filter:{filter_value}"
        )
        group = box(
            f"position:absolute;inset:0;opacity:{opacity};transform:rotate({index - 5}deg)",
            child,
        )
        result.append(
            Case(
                named("filter-opacity-nested", index),
                "filter-opacity",
                "wpt/css/filter-effects/",
                stage(group, "background:#fef3c7;"),
            )
        )
    assert len(result) == 80
    return result


def clip_paths() -> list[Case]:
    result: list[Case] = []
    inset_values = [
        "0",
        "8px",
        "12px 20px",
        "8px 18px 28px",
        "5% 15% 25% 35%",
        "10px round 14px",
        "12px 20px round 24px 8px",
        "0 28px round 50%",
        "20% round 20% / 40%",
        "4px 18px 30px 10px round 8px 24px",
    ]
    for inset_index, inset in enumerate(inset_values):
        for geometry_index, geometry in enumerate([None, "content-box"]):
            index = inset_index * 2 + geometry_index
            geometry_suffix = "" if geometry is None else f" {geometry}"
            target = box(
                "position:absolute;left:14px;top:14px;width:100px;height:100px;"
                "padding:12px;border:6px solid #1d4ed8;box-sizing:border-box;"
                "background:linear-gradient(135deg,#ef4444 0%,#22c55e 100%);"
                f"clip-path:inset({inset}){geometry_suffix}"
            )
            result.append(
                Case(
                    named("clip-inset", index),
                    "clip-path",
                    "wpt/css/css-masking/clip-path/",
                    stage(target, "background:#fef2f2;"),
                )
            )

    circle_radii = ["10px", "25%", "40%", "closest-side", "farthest-side"]
    circle_positions = ["center", "25% 25%", "75% 25%", "25% 75%"]
    for radius_index, radius in enumerate(circle_radii):
        for position_index, position in enumerate(circle_positions):
            index = radius_index * 4 + position_index
            target = box(
                "position:absolute;left:12px;top:12px;width:104px;height:104px;"
                "background:conic-gradient(#ef4444,#22c55e,#3b82f6,#ef4444);"
                f"clip-path:circle({radius} at {position})"
            )
            result.append(
                Case(
                    named("clip-circle", index),
                    "clip-path",
                    "wpt/css/css-masking/clip-path/",
                    stage(target, "background:#e2e8f0;"),
                )
            )

    ellipse_radii = ["20px 40px", "25% 45%", "closest-side farthest-side", "40% 20%"]
    ellipse_positions = ["center", "20% 30%", "80% 30%", "40% 80%", "right bottom"]
    for radius_index, radius in enumerate(ellipse_radii):
        for position_index, position in enumerate(ellipse_positions):
            index = radius_index * 5 + position_index
            target = box(
                "position:absolute;left:12px;top:12px;width:104px;height:104px;"
                "background:linear-gradient(45deg,#f59e0b,#7c3aed);"
                f"clip-path:ellipse({radius} at {position})"
            )
            result.append(
                Case(
                    named("clip-ellipse", index),
                    "clip-path",
                    "wpt/css/css-masking/clip-path/",
                    stage(target, "background:#dcfce7;"),
                )
            )

    paths = [
        "M 0 0 L 100 0 L 100 100 Z",
        "M 50 0 L 100 50 L 50 100 L 0 50 Z",
        "M 10 10 H 90 V 90 H 10 Z",
        "M 0 50 C 20 0 80 0 100 50 C 80 100 20 100 0 50 Z",
        "M 50 0 A 50 50 0 1 1 49.9 0 Z",
        "M 0 0 L 100 100 M 100 0 L 0 100 Z",
        "M 10 50 Q 50 0 90 50 Q 50 100 10 50 Z",
        "M 0 20 L 80 20 L 100 50 L 80 80 L 0 80 Z",
        "M 20 0 L 80 0 L 100 100 L 0 100 Z",
        "M 0 0 L 100 0 L 50 50 L 100 100 L 0 100 Z",
    ]
    for index, path in enumerate(paths):
        target = box(
            "position:absolute;left:14px;top:14px;width:100px;height:100px;"
            "background:linear-gradient(90deg,#ef4444,#22c55e,#2563eb);"
            f"clip-path:path('{path}')"
        )
        result.append(
            Case(
                named("clip-path-function", index),
                "clip-path",
                "wpt/css/css-masking/clip-path/",
                stage(target, "background:#fef3c7;"),
            )
        )

    polygons = [
        "0 0,100% 0,100% 100%",
        "50% 0,100% 50%,50% 100%,0 50%",
        "0 0,100% 20%,80% 100%,20% 80%",
        "10% 10%,90% 10%,50% 90%",
        "0 50%,25% 0,75% 0,100% 50%,75% 100%,25% 100%",
        "0 0,100% 0,50% 50%,100% 100%,0 100%",
        "20% 0,80% 0,100% 100%,0 100%",
        "0 20%,80% 20%,100% 50%,80% 80%,0 80%",
        "5% 5%,95% 15%,85% 95%,15% 85%",
        "50% 0,65% 35%,100% 40%,72% 62%,80% 100%,50% 78%,20% 100%,28% 62%,0 40%,35% 35%",
    ]
    for index, polygon in enumerate(polygons):
        target = box(
            "position:absolute;left:14px;top:14px;width:100px;height:100px;"
            "background:linear-gradient(135deg,#f43f5e,#06b6d4);"
            f"clip-path:polygon({polygon})"
        )
        result.append(
            Case(
                named("clip-polygon", index),
                "clip-path",
                "wpt/css/css-masking/clip-path/",
                stage(target, "background:#e2e8f0;"),
            )
        )
    assert len(result) == 80
    return result


def masks() -> list[Case]:
    result: list[Case] = []
    directions = ["0deg", "45deg", "90deg", "135deg", "180deg"]
    stops = [
        "transparent 0%,transparent 25%,black 25%,black 100%",
        "black 0%,black 40%,transparent 60%,transparent 100%",
        "transparent 0%,black 45%,black 55%,transparent 100%",
        "black 0%,black 20%,transparent 20%,transparent 40%,"
        "black 40%,black 60%,transparent 60%,transparent 80%,black 80%,black 100%",
    ]
    repeats = ["no-repeat", "repeat"]
    for direction_index, direction in enumerate(directions):
        for stop_index, stop in enumerate(stops):
            for repeat_index, repeat in enumerate(repeats):
                index = direction_index * 8 + stop_index * 2 + repeat_index
                size = "100% 100%" if repeat == "no-repeat" else "36px 28px"
                target = box(
                    "position:absolute;left:12px;top:12px;width:104px;height:104px;"
                    "background:linear-gradient(135deg,#ef4444,#22c55e,#2563eb);"
                    f"mask-image:linear-gradient({direction},{stop});"
                    f"mask-repeat:{repeat};mask-size:{size}"
                )
                result.append(
                    Case(
                        named("mask-linear", index),
                        "mask",
                        "wpt/css/css-masking/mask-image/",
                        stage(target, "background:#fef2f2;"),
                    )
                )

    radial_shapes = [
        "circle closest-side",
        "circle farthest-side",
        "ellipse closest-side",
        "ellipse farthest-corner",
    ]
    radial_positions = ["center", "25% 25%", "75% 30%", "30% 75%", "right bottom"]
    for shape_index, shape in enumerate(radial_shapes):
        for position_index, position in enumerate(radial_positions):
            index = shape_index * 5 + position_index
            target = box(
                "position:absolute;left:12px;top:12px;width:104px;height:104px;"
                "background:conic-gradient(#ef4444,#22c55e,#2563eb,#ef4444);"
                f"mask-image:radial-gradient({shape} at {position},"
                "black 0%,black 45%,transparent 55%,transparent 100%);"
                "mask-repeat:no-repeat"
            )
            result.append(
                Case(
                    named("mask-radial", index),
                    "mask",
                    "wpt/css/css-masking/mask-image/",
                    stage(target, "background:#e2e8f0;"),
                )
            )

    mask_positions = [
        "left top",
        "center top",
        "right top",
        "left center",
        "center",
        "right center",
        "left bottom",
        "center bottom",
        "right bottom",
        "30% 70%",
    ]
    for index, position in enumerate(mask_positions):
        target = box(
            "position:absolute;left:10px;top:10px;width:108px;height:108px;"
            "padding:12px;border:8px solid #7c3aed;box-sizing:border-box;"
            "background:#22c55e;"
            "mask-image:linear-gradient(90deg,black 0%,black 50%,"
            "transparent 50%,transparent 100%);"
            "mask-size:54px 42px;mask-repeat:no-repeat;"
            f"mask-position:{position};mask-origin:border-box;mask-clip:border-box"
        )
        result.append(
            Case(
                named("mask-position", index),
                "mask",
                "wpt/css/css-masking/mask-position/",
                stage(target, "background:#fee2e2;"),
            )
        )

    mask_box_pairs = [
        (origin, clip)
        for origin in ["border-box", "padding-box", "content-box"]
        for clip in ["border-box", "padding-box", "content-box"]
    ]
    mask_box_pairs.append(("border-box", "border-box"))
    for index, (origin, clip) in enumerate(mask_box_pairs):
        extra_geometry = (
            "mask-position:center;mask-size:72px 72px;" if index == 9 else ""
        )
        target = box(
            "position:absolute;left:12px;top:12px;width:104px;height:104px;"
            "padding:14px;border:8px solid #2563eb;box-sizing:border-box;background:#f59e0b;"
            "mask-image:linear-gradient(135deg,black 0%,black 58%,"
            "transparent 58%,transparent 100%);"
            f"mask-origin:{origin};mask-clip:{clip};mask-repeat:no-repeat;{extra_geometry}"
        )
        result.append(
            Case(
                named("mask-boxes", index),
                "mask",
                "wpt/css/css-masking/mask-origin/",
                stage(target, "background:#dcfce7;"),
            )
        )

    second_mask_positions = ["25% 25%", "50% 25%", "75% 35%", "35% 70%", "70% 70%"]
    for index in range(20):
        composite = ["add", "subtract", "intersect", "exclude"][index % 4]
        mode = ["alpha", "luminance"][index // 4 % 2]
        second_position = second_mask_positions[index // 4]
        target = box(
            "position:absolute;left:12px;top:12px;width:104px;height:104px;"
            "background:linear-gradient(90deg,#ef4444,#22c55e,#2563eb);"
            "mask-image:linear-gradient(90deg,black 0%,black 62%,"
            "transparent 62%,transparent 100%),"
            f"radial-gradient(circle at {second_position},black 0%,black 32%,"
            "transparent 34%,transparent 100%);"
            "mask-repeat:no-repeat,no-repeat;"
            f"mask-composite:{composite};mask-mode:{mode}"
        )
        result.append(
            Case(
                named("mask-multiple", index),
                "mask",
                "wpt/css/css-masking/mask-composite/",
                stage(target, "background:#fef3c7;"),
            )
        )
    assert len(result) == 100
    return result


def text_cases() -> list[Case]:
    result: list[Case] = []
    sizes = [8, 10, 12, 14, 16, 18, 20, 24, 28, 32]
    for size_index, size in enumerate(sizes):
        for color_index, color in enumerate(["#0f172a", "#e11d48"]):
            index = size_index * 2 + color_index
            target = box(
                "position:absolute;left:8px;top:12px;width:112px;height:104px;"
                "flex-direction:column;font-family:Ahem;overflow:hidden;background:#dbeafe;"
                f"font-size:{size}px;line-height:1.25;color:{color}",
                "XXXX XX XX",
            )
            result.append(
                Case(
                    named("text-size-color", index),
                    "text",
                    "wpt/css/css-fonts/",
                    stage(target),
                )
            )

    for index in range(20):
        size = [12, 16, 20, 24][index % 4]
        line_height = ["normal", "1", "1.25", "1.5", "28px"][index % 5]
        spacing = [-2, 0, 1, 3][index // 5]
        target = box(
            "position:absolute;left:8px;top:8px;width:112px;height:112px;"
            "flex-direction:column;font-family:Ahem;color:#1d4ed8;background:#fef2f2;"
            f"font-size:{size}px;line-height:{line_height};letter-spacing:{spacing}px",
            "XXXX XXXX XXXX XXXX",
        )
        result.append(
            Case(
                named("text-metrics", index),
                "text",
                "wpt/css/css-text/",
                stage(target),
            )
        )

    alignments = ["left", "center", "right", "start", "end"]
    directions = ["ltr", "rtl"]
    widths = [56, 96]
    for alignment_index, alignment in enumerate(alignments):
        for direction_index, direction in enumerate(directions):
            for width_index, width in enumerate(widths):
                index = alignment_index * 4 + direction_index * 2 + width_index
                target = box(
                    f"position:absolute;left:12px;top:12px;width:{width}px;height:104px;"
                    "flex-direction:column;font-family:Ahem;font-size:16px;"
                    "line-height:20px;color:#0f172a;"
                    "background:#dcfce7;overflow:hidden;"
                    f"text-align:{alignment};direction:{direction}",
                    "XX XX XX XX",
                )
                result.append(
                    Case(
                        named("text-align-direction", index),
                        "text",
                        "wpt/css/css-text/text-align/",
                        stage(target),
                    )
                )

    decoration_styles = ["solid", "double", "dotted", "dashed", "wavy"]
    decoration_lines = ["underline", "line-through"]
    for style_index, decoration_style in enumerate(decoration_styles):
        for line_index, decoration_line in enumerate(decoration_lines):
            for thickness_index, thickness in enumerate(["auto", "3px"]):
                index = style_index * 4 + line_index * 2 + thickness_index
                target = box(
                    "position:absolute;left:8px;top:30px;width:112px;height:68px;"
                    "flex-direction:column;font-family:Ahem;font-size:22px;"
                    "line-height:30px;color:#1d4ed8;"
                    f"text-decoration-line:{decoration_line};"
                    f"text-decoration-style:{decoration_style};"
                    "text-decoration-color:#e11d48;"
                    f"text-decoration-thickness:{thickness}",
                    "XXXX",
                )
                result.append(
                    Case(
                        named("text-decoration", index),
                        "text",
                        "wpt/css/css-text-decor/",
                        stage(target),
                    )
                )

    shadow_values = [
        "4px 4px 0 #ef4444",
        "-4px -4px 0 #22c55e",
        "0 6px 0 rgba(37,99,235,.6)",
        "3px 3px 0 #f59e0b,-3px -3px 0 #7c3aed",
        "8px 0 0 #ef4444,0 8px 0 #22c55e",
        "2px 2px 2px #ef4444",
        "-3px 4px 4px #22c55e",
        "0 6px 6px #2563eb",
        "4px 0 10px rgba(225,29,72,.8)",
        "0 0 14px #7c3aed",
    ]
    for shadow_index, shadow in enumerate(shadow_values):
        for decoration in [False, True]:
            index = shadow_index * 2 + int(decoration)
            decoration_css = "text-decoration:underline wavy #0f766e;" if decoration else ""
            target = box(
                "position:absolute;left:8px;top:28px;width:112px;height:72px;"
                "flex-direction:column;font-family:Ahem;font-size:24px;"
                "line-height:32px;color:#0f172a;"
                f"text-shadow:{shadow};{decoration_css}",
                "XXXX",
            )
            result.append(
                Case(
                    named("text-shadow", index),
                    "text",
                    "wpt/css/css-text-decor/text-shadow/",
                    stage(target),
                )
            )

    for index in range(20):
        width = [1, 2, 3, 5][index % 4]
        fill = ["#f8fafc", "#f59e0b", "transparent", "#22c55e", "#3b82f6"][index // 4]
        target = box(
            "position:absolute;left:8px;top:24px;width:112px;height:78px;"
            "flex-direction:column;font-family:Ahem;font-size:28px;line-height:36px;"
            f"color:{fill};-webkit-text-stroke:{width}px #be123c;"
            f"text-stroke:{width}px #be123c",
            "XXXX",
        )
        result.append(
            Case(
                named("text-stroke", index),
                "text",
                "wpt/css/css-text-decor/",
                stage(target, "background:#e0f2fe;"),
            )
        )

    gradients = [
        "linear-gradient(90deg,#ef4444,#22c55e,#2563eb)",
        "linear-gradient(45deg,#7c3aed 0%,#7c3aed 50%,#f59e0b 50%,#f59e0b 100%)",
        "radial-gradient(circle at 30% 40%,#22c55e 0%,#22c55e 30%,"
        "#2563eb 70%,#2563eb 100%)",
        "conic-gradient(#ef4444,#22c55e,#2563eb,#ef4444)",
        "repeating-linear-gradient(90deg,#0f172a 0px,#0f172a 6px,"
        "#f8fafc 6px,#f8fafc 12px)",
    ]
    for gradient_index, gradient in enumerate(gradients):
        for variant in range(4):
            index = gradient_index * 4 + variant
            extra = [
                "",
                "text-align:center;",
                "letter-spacing:3px;",
                "transform:rotate(-8deg);",
            ][variant]
            target = box(
                "position:absolute;left:8px;top:22px;width:112px;height:82px;"
                "flex-direction:column;font-family:Ahem;font-size:26px;"
                "line-height:36px;color:transparent;"
                f"background-image:{gradient};background-clip:text;{extra}",
                "XXXX",
            )
            result.append(
                Case(
                    named("text-background-clip", index),
                    "text",
                    "wpt/css/css-backgrounds/background-clip/",
                    stage(target, "background:#e2e8f0;"),
                )
            )

    wrapping = [
        ("normal", "normal"),
        ("normal", "break-all"),
        ("normal", "keep-all"),
        ("nowrap", "normal"),
    ]
    for wrap_index, (white_space, word_break) in enumerate(wrapping):
        for width_index, width in enumerate([42, 56, 72, 88, 108]):
            index = wrap_index * 5 + width_index
            target = box(
                f"position:absolute;left:10px;top:8px;width:{width}px;height:112px;"
                "flex-direction:column;font-family:Ahem;font-size:14px;"
                "line-height:18px;color:#0f172a;"
                "background:#dcfce7;overflow:hidden;"
                f"white-space:{white_space};word-break:{word_break}",
                "XXXX XX XXXX XX",
            )
            result.append(
                Case(
                    named("text-wrap", index),
                    "text",
                    "wpt/css/css-text/white-space/",
                    stage(target),
                )
            )
    assert len(result) == 160
    return result


def all_cases() -> list[Case]:
    cases = [
        *backgrounds(),
        *borders(),
        *shadows_and_outlines(),
        *paint_order(),
        *overflow_and_containment(),
        *transforms(),
        *filters_and_opacity(),
        *clip_paths(),
        *masks(),
        *text_cases(),
    ]
    assert len(cases) == CASE_COUNT
    names = [case.name for case in cases]
    assert len(set(names)) == CASE_COUNT
    fragments = [case.fragment for case in cases]
    assert len(set(fragments)) == CASE_COUNT, "every test must be a unique visual probe"
    for case in cases:
        # Test content may contain only div elements. Text nodes are allowed.
        tags = set(re.findall(r"</?([a-zA-Z][a-zA-Z0-9-]*)", case.fragment))
        assert tags <= {"div"}, (case.name, tags)
    return cases


def classify_difference(name: str, issue: str) -> DifferenceKind:
    if issue in RASTER_OR_SAMPLING_ISSUES:
        return DifferenceKind.RASTER_OR_SAMPLING
    if issue in UA_CHOICE_ISSUES:
        return DifferenceKind.UA_CHOICE
    if issue in W3C_GAP_ISSUES:
        return DifferenceKind.W3C_GAP
    if issue == "pulsar-text-stroke-join-geometry":
        return DifferenceKind.NON_W3C_COMPATIBILITY
    raise SystemExit(f"{name}: unclassified CSS-paint difference issue {issue!r}")


def load_differences(cases: Iterable[Case]) -> dict[str, Difference]:
    valid = {case.name for case in cases}
    if not DIFFERENCES.exists():
        raise SystemExit(f"missing checked difference registry {DIFFERENCES}")
    result: dict[str, Difference] = {}
    for line_number, raw in enumerate(DIFFERENCES.read_text().splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        try:
            name, issue = line.split("\t")
        except ValueError as error:
            raise SystemExit(
                f"{DIFFERENCES}:{line_number}: expected <case> TAB <issue>"
            ) from error
        if name not in valid:
            raise SystemExit(f"{DIFFERENCES}:{line_number}: unknown case {name!r}")
        if name in result:
            raise SystemExit(f"{DIFFERENCES}:{line_number}: duplicate case {name!r}")
        result[name] = Difference(issue, classify_difference(name, issue))

    counts = Counter(difference.kind for difference in result.values())
    expected = {
        DifferenceKind.RASTER_OR_SAMPLING: 84,
        DifferenceKind.UA_CHOICE: 61,
        DifferenceKind.W3C_GAP: 170,
        DifferenceKind.NON_W3C_COMPATIBILITY: 19,
    }
    if counts != expected:
        raise SystemExit(
            f"{DIFFERENCES}: difference classification counts {counts}, expected {expected}"
        )
    return result


def rust_string(value: str) -> str:
    hashes = ""
    while f'"{hashes}' in value:
        hashes += "#"
    return f'r{hashes}"{value}"{hashes}'


def rust_difference_kind(kind: DifferenceKind) -> str:
    return {
        DifferenceKind.RASTER_OR_SAMPLING: "DifferenceKind::RasterOrSampling",
        DifferenceKind.UA_CHOICE: "DifferenceKind::UaChoice",
        DifferenceKind.W3C_GAP: "DifferenceKind::W3cGap",
        DifferenceKind.NON_W3C_COMPATIBILITY: "DifferenceKind::NonW3cCompatibility",
    }[kind]


def write_rust(cases: list[Case], differences: dict[str, Difference]) -> None:
    lines = [
        "// @generated by support/generate_css_paint_cases.py; do not edit.",
        f"pub(super) static CASES: [CssPaintCase; {CASE_COUNT}] = [",
    ]
    for case in cases:
        difference = differences.get(case.name)
        if difference is None:
            expected = "Expectation::BrowserMatch"
        elif difference.kind in NATIVE_SNAPSHOT_KINDS:
            expected = (
                "Expectation::NativeSnapshot { "
                f"kind: {rust_difference_kind(difference.kind)}, "
                f"issue: {rust_string(difference.issue)} "
                "}"
            )
        else:
            expected = (
                "Expectation::Skip { "
                f"kind: {rust_difference_kind(difference.kind)}, "
                f"issue: {rust_string(difference.issue)} "
                "}"
            )
        lines.extend(
            [
                "    CssPaintCase {",
                f"        name: {rust_string(case.name)},",
                f"        category: {rust_string(case.category)},",
                f"        source: {rust_string(case.source)},",
                f"        fragment: {rust_string(case.fragment)},",
                f"        expectation: {expected},",
                "    },",
            ]
        )
    lines.append("];")
    lines.append("")
    lines.append("css_paint_case_tests! {")
    lines.append("    browser_matches {")
    for index, case in enumerate(cases):
        if case.name in differences:
            continue
        identifier = "css_" + case.name.replace("-", "_")
        lines.append(f"        {index} => {identifier};")
    lines.append("    }")
    lines.append("    native_snapshots {")
    for index, case in enumerate(cases):
        difference = differences.get(case.name)
        if difference is None or difference.kind not in NATIVE_SNAPSHOT_KINDS:
            continue
        identifier = "css_native_" + case.name.replace("-", "_")
        lines.append(f"        {index} => {identifier};")
    lines.append("    }")
    lines.append("    skips {")
    for index, case in enumerate(cases):
        difference = differences.get(case.name)
        if difference is None or difference.kind in NATIVE_SNAPSHOT_KINDS:
            continue
        identifier = "css_" + case.name.replace("-", "_")
        reason = f"{difference.kind.value}: {difference.issue}"
        lines.append(f"        {index} => {identifier}, {rust_string(reason)};")
    lines.extend(["    }", "}", ""])
    GENERATED.parent.mkdir(parents=True, exist_ok=True)
    GENERATED.write_text("\n".join(lines))


def iframe_document(case: Case) -> str:
    # Native imports the stage as the document element, so it is the initial
    # stacking-context root. Isolate the browser stage to test the same
    # descendant paint-order semantics without changing the shared fragment.
    return (
        "<!doctype html><meta charset=utf-8><style>"
        "@font-face{font-family:Ahem;src:url('/crates/hughie/tests/fixtures/Ahem.ttf')"
        " format('truetype')}html,body{margin:0;width:128px;height:128px;"
        "overflow:hidden;background:#fff}"
        "body>div:first-of-type{isolation:isolate}</style>"
        f"{case.fragment}<script>"
        "document.fonts.ready.then(()=>document.fonts.load('16px Ahem','X')).then(faces=>{"
        "if(faces.length!==1||!document.fonts.check('16px Ahem','X'))"
        "throw new Error('Ahem failed to load');"
        "requestAnimationFrame(()=>requestAnimationFrame(()=>"
        "parent.postMessage({ready:true,name:"
        f"{case.name!r}"
        "},'*')))}).catch(error=>parent.postMessage({error:String(error),name:"
        f"{case.name!r}"
        "},'*'));"
        "</script>"
    )


def difference_fixture_document(case: Case, difference: Difference) -> str:
    return (
        "<!doctype html>\n"
        "<meta charset=\"utf-8\">\n"
        f'<meta name="css-paint-case" content="{html.escape(case.name, quote=True)}">\n'
        f'<meta name="css-paint-difference-kind" content="{difference.kind.value}">\n'
        f'<meta name="css-paint-issue" content="{html.escape(difference.issue, quote=True)}">\n'
        f"<title>{html.escape(case.name)}</title>\n"
        "<style>\n"
        "@font-face {\n"
        "  font-family: Ahem;\n"
        "  src: url('/crates/hughie/tests/fixtures/Ahem.ttf') format('truetype');\n"
        "}\n"
        "html, body {\n"
        "  margin: 0;\n"
        "  width: 128px;\n"
        "  height: 128px;\n"
        "  overflow: hidden;\n"
        "  background: #fff;\n"
        "}\n"
        "body > div:first-of-type {\n"
        "  isolation: isolate;\n"
        "}\n"
        "</style>\n"
        f"<!-- source: {html.escape(case.source)} -->\n"
        f"{case.fragment}\n"
    )


def write_difference_fixtures(
    cases: list[Case], differences: dict[str, Difference]
) -> None:
    DIFFERENCE_FIXTURES.mkdir(parents=True, exist_ok=True)
    expected = set(differences)
    for stale in DIFFERENCE_FIXTURES.glob("*.html"):
        if stale.stem not in expected:
            stale.unlink()
    for case in cases:
        difference = differences.get(case.name)
        if difference is None:
            continue
        (DIFFERENCE_FIXTURES / f"{case.name}.html").write_text(
            difference_fixture_document(case, difference)
        )


def write_html(cases: list[Case], output: Path) -> None:
    output.mkdir(parents=True, exist_ok=True)
    for shard in range(SHARD_COUNT):
        shard_cases = cases[
            shard * CASES_PER_SHARD : (shard + 1) * CASES_PER_SHARD
        ]
        frames: list[str] = []
        names = [case.name for case in shard_cases]
        for slot, case in enumerate(shard_cases):
            x = (slot % GRID) * CELL_SIZE
            y = (slot // GRID) * CELL_SIZE
            source = html.escape(iframe_document(case), quote=True)
            frames.append(
                f'<iframe title="{case.name}" srcdoc="{source}" '
                f'style="position:absolute;left:{x}px;top:{y}px;width:128px;'
                'height:128px;border:0;margin:0;padding:0"></iframe>'
            )
        page = (
            "<!doctype html><meta charset=utf-8><style>"
            "html,body{margin:0;width:640px;height:640px;overflow:hidden;background:#fff}"
            "</style>"
            + "".join(frames)
            + "<script>"
            f"const expected=new Set({names!r});"
            "addEventListener('message',event=>{"
            "if(event.data&&event.data.error)window.__ATLAS_ERROR__="
            "event.data.name+': '+event.data.error;"
            "if(event.data&&event.data.ready)expected.delete(event.data.name);"
            "if(expected.size===0)window.__ATLAS_READY__=true;"
            "});"
            "</script>"
        )
        (output / f"shard-{shard:02d}.html").write_text(page)


def split_atlases(
    cases: list[Case],
    differences: dict[str, Difference],
    atlas_dir: Path,
    output: Path,
    include_differences: bool,
) -> None:
    try:
        from PIL import Image
    except ImportError as error:
        raise SystemExit("splitting browser atlases requires Pillow") from error
    output.mkdir(parents=True, exist_ok=True)
    for shard in range(SHARD_COUNT):
        path = atlas_dir / f"shard-{shard:02d}.png"
        image = Image.open(path).convert("RGBA")
        if image.size != (ATLAS_SIZE, ATLAS_SIZE):
            raise SystemExit(f"{path}: expected 640x640, got {image.size}")
        for slot in range(CASES_PER_SHARD):
            case = cases[shard * CASES_PER_SHARD + slot]
            if case.name in differences and not include_differences:
                continue
            x = (slot % GRID) * CELL_SIZE
            y = (slot // GRID) * CELL_SIZE
            tile = image.crop((x, y, x + CELL_SIZE, y + CELL_SIZE))
            colors = tile.getcolors(maxcolors=CELL_SIZE * CELL_SIZE)
            if colors is not None and len(colors) == 1:
                color = colors[0][1]
                if color == (255, 255, 255, 255) or color[3] == 0:
                    raise SystemExit(
                        f"{case.name}: browser reference is a blank solid-color tile"
                    )
            tile.save(output / f"{case.name}.png", optimize=True)


def prune_reference_assets(
    cases: list[Case], differences: dict[str, Difference]
) -> None:
    browser_matches = {case.name for case in cases if case.name not in differences}
    native_snapshots = {
        name
        for name, difference in differences.items()
        if difference.kind in NATIVE_SNAPSHOT_KINDS
    }
    for stale in BROWSER_GOLDENS.glob("*.png"):
        if stale.stem not in browser_matches:
            stale.unlink()
    if NATIVE_GOLDENS.is_dir():
        for stale in NATIVE_GOLDENS.glob("*.png"):
            if stale.stem not in native_snapshots:
                stale.unlink()


def asset_basenames(directory: Path, suffix: str) -> set[str]:
    if not directory.is_dir():
        raise SystemExit(f"missing asset directory {directory}")
    return {path.stem for path in directory.iterdir() if path.suffix == suffix}


def validate_assets(cases: list[Case], differences: dict[str, Difference]) -> None:
    all_names = {case.name for case in cases}
    difference_names = set(differences)
    browser_matches = all_names - difference_names
    native_snapshots = {
        name
        for name, difference in differences.items()
        if difference.kind in NATIVE_SNAPSHOT_KINDS
    }
    skipped_names = difference_names - native_snapshots
    if (
        len(all_names) != 1_000
        or len(browser_matches) != 666
        or len(native_snapshots) != 145
        or len(skipped_names) != 189
    ):
        raise SystemExit(
            "CSS-paint inventory must contain 1000 total / 666 browser matches / "
            "145 native snapshots / 189 skipped"
        )
    if browser_matches & difference_names or native_snapshots & skipped_names:
        raise SystemExit("CSS-paint expectation sets overlap")

    browser_golden_names = asset_basenames(BROWSER_GOLDENS, ".png")
    if browser_golden_names != browser_matches:
        raise SystemExit(
            "committed browser PNG basename mismatch: "
            f"missing={sorted(browser_matches - browser_golden_names)}, "
            f"extra={sorted(browser_golden_names - browser_matches)}"
        )
    native_golden_names = asset_basenames(NATIVE_GOLDENS, ".png")
    if native_golden_names != native_snapshots:
        raise SystemExit(
            "committed native PNG basename mismatch: "
            f"missing={sorted(native_snapshots - native_golden_names)}, "
            f"extra={sorted(native_golden_names - native_snapshots)}"
        )
    fixture_names = asset_basenames(DIFFERENCE_FIXTURES, ".html")
    if fixture_names != difference_names:
        raise SystemExit(
            "difference HTML basename mismatch: "
            f"missing={sorted(difference_names - fixture_names)}, "
            f"extra={sorted(fixture_names - difference_names)}"
        )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--html-output", type=Path)
    parser.add_argument("--split-atlases", type=Path)
    parser.add_argument(
        "--reference-output",
        type=Path,
        default=BROWSER_GOLDENS,
        help="per-case PNG destination (defaults to committed Chromium matches)",
    )
    parser.add_argument(
        "--include-differences",
        action="store_true",
        help="split all 1000 references; requires a non-committed reference output",
    )
    parser.add_argument("--prune-reference-assets", action="store_true")
    parser.add_argument("--validate-assets", action="store_true")
    args = parser.parse_args()
    cases = all_cases()
    differences = load_differences(cases)
    write_rust(cases, differences)
    write_difference_fixtures(cases, differences)
    if args.include_differences and args.split_atlases is None:
        parser.error("--include-differences requires --split-atlases")
    reference_output = args.reference_output.resolve()
    browser_goldens = BROWSER_GOLDENS.resolve()
    screenshot_root = browser_goldens.parent
    if reference_output != browser_goldens and (
        reference_output == screenshot_root or screenshot_root in reference_output.parents
    ):
        parser.error(
            "--reference-output inside tests/screenshots is reserved for committed references"
        )
    if args.include_differences and reference_output == browser_goldens:
        parser.error(
            "--include-differences requires --reference-output outside the committed "
            "screenshot tree"
        )
    if args.html_output is not None:
        write_html(cases, args.html_output)
    if args.split_atlases is not None:
        split_atlases(
            cases,
            differences,
            args.split_atlases,
            args.reference_output,
            args.include_differences,
        )
        if reference_output == browser_goldens:
            prune_reference_assets(cases, differences)
    if args.prune_reference_assets:
        prune_reference_assets(cases, differences)
    if args.validate_assets:
        validate_assets(cases, differences)
    native_count = sum(
        difference.kind in NATIVE_SNAPSHOT_KINDS
        for difference in differences.values()
    )
    print(
        f"generated {len(cases)} cases ({len(cases) - len(differences)} browser "
        f"matches, {native_count} native snapshots, "
        f"{len(differences) - native_count} skipped), {SHARD_COUNT} shards"
    )


if __name__ == "__main__":
    main()
