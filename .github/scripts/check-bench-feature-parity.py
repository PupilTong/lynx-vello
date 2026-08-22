#!/usr/bin/env python3
"""Fails when a workspace library is benchmarked in a non-production feature set.

Cargo unifies the features of a package's *dev*-dependencies into that
package's own library whenever dev targets are part of the build. `cargo build
--workspace` and `cargo build --workspace --benches` can therefore compile the
same library two different ways — and the second is what `cargo codspeed build`
and `cargo llvm-cov` run. A benchmark measuring a library that production never
ships is not measuring anything anyone will run.

This compares the two resolutions directly, out of Cargo's own unit graph, and
fails on any difference that is not written down in ACCEPTED below. Recording a
deviation is a deliberate act with a reason attached; acquiring one silently is
what this prevents.

Usage:  python3 .github/scripts/check-bench-feature-parity.py
"""

from __future__ import annotations

import json
import subprocess
import sys

# package -> {feature: why this deviation is accepted}
#
# An entry here means: the benchmarks of this library run against a build that
# production does not ship, and that is known and accepted. Adding one is a
# decision about benchmark fidelity, so say what it costs.
ACCEPTED: dict[str, dict[str, str]] = {
    "dom": {
        "layout-test-utils": (
            "hughie dev-depends on dom/layout-test-utils for its bench harness, and "
            "dom depends on hughie, so any build that includes bench targets "
            "resolves the cycle by turning the feature on for dom's own library "
            "too. Cost: one `test_leaf_metrics()` probe per leaf in "
            "crates/dom/src/layout/host.rs that a release build does not contain. "
            "Breaking the cycle means moving hughie's dom-based benches into dom, "
            "which renumbers every CodSpeed benchmark id and discards its history. "
            "See AGENTS.md, 'Benchmarks measure a debug-instrumented dom'."
        )
    },
    "hughie": {
        "layout-test-utils": (
            "Same cycle, seen from the other side. On hughie the feature only adds "
            "the `compute_leaf_layout_with_measurement_for_testing` wrapper; no "
            "hot path gains a branch, so this side is cost-free."
        )
    },
}


def shipped_libraries() -> set[str]:
    """Workspace libraries that something depends on outside of dev targets.

    A crate every dependent lists under `[dev-dependencies]` — flashbulb, the
    screenshot harness — has no shipped configuration to deviate from. It shows
    up in the production graph only because `cargo build --workspace` builds
    every member's library, and comparing against that is meaningless.
    """
    metadata = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    members = {package["name"] for package in metadata["packages"]}
    return {
        dependency["name"]
        for package in metadata["packages"]
        for dependency in package["dependencies"]
        if dependency["kind"] is None and dependency["name"] in members
    }


def unit_graph(*flags: str) -> list[dict]:
    return json.loads(
        subprocess.run(
            ["cargo", "build", "--unit-graph", "-Z", "unstable-options", *flags],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )["units"]


def reachable_from_benches(units: list[dict]) -> set[int]:
    """Indices of the units a benchmark binary actually links.

    Cargo's `--benches` graph also carries libraries nothing benchmarks — a
    dev-dependency of a package that has no bench target at all. Those may
    differ from production freely; no measurement reads them.
    """
    seen: set[int] = set()
    stack = [i for i, unit in enumerate(units) if unit["target"]["kind"] == ["bench"]]
    while stack:
        index = stack.pop()
        if index in seen:
            continue
        seen.add(index)
        stack.extend(dependency["index"] for dependency in units[index]["dependencies"])
    return seen


def library_features(units: list[dict], indices: set[int] | None = None) -> dict[str, set[str]]:
    """Feature set each workspace library is compiled with, per Cargo."""
    members = shipped_libraries()
    resolved: dict[str, set[str]] = {}
    for index, unit in enumerate(units):
        if indices is not None and index not in indices:
            continue
        target = unit["target"]
        if target["kind"] != ["lib"] or target["name"] not in members:
            continue
        resolved.setdefault(target["name"], set()).update(unit["features"])
    return resolved


def main() -> int:
    production = library_features(unit_graph("--workspace"))
    bench_units = unit_graph("--workspace", "--benches")
    benchmarked = library_features(bench_units, reachable_from_benches(bench_units))

    unrecorded: list[str] = []
    recorded: list[str] = []
    for package, features in sorted(benchmarked.items()):
        for feature in sorted(features - production.get(package, set())):
            reason = ACCEPTED.get(package, {}).get(feature)
            if reason is None:
                unrecorded.append(f"{package}/{feature}")
            else:
                recorded.append(f"{package}/{feature}: {reason}")

    for entry in recorded:
        print(f"accepted deviation — {entry}")

    # A stale allowlist is its own kind of wrong: it tells a reader a cost is
    # still being paid after someone removed it.
    stale = [
        f"{package}/{feature}"
        for package, features in ACCEPTED.items()
        for feature in features
        if feature not in benchmarked.get(package, set())
        or feature in production.get(package, set())
    ]
    if stale:
        print()
        print("ACCEPTED lists deviations that no longer exist:")
        for entry in stale:
            print(f"  {entry}")
        print("Delete them from .github/scripts/check-bench-feature-parity.py.")
        return 1

    if unrecorded:
        print()
        print("Benchmarks would measure a library production does not ship:")
        for entry in unrecorded:
            print(f"  {entry}")
        print()
        print(
            "A dev-dependency turned this feature on for the library itself. Either\n"
            "break the dev-dependency cycle, or add the feature to ACCEPTED in\n"
            ".github/scripts/check-bench-feature-parity.py with what it costs."
        )
        return 1

    print("no unrecorded deviation between the production and benchmarked builds")
    return 0


if __name__ == "__main__":
    sys.exit(main())
