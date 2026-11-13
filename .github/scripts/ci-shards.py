#!/usr/bin/env python3
"""Splits CI's tests and memory benchmarks into shards, and checks the split.

CI runs the workspace's tests in two jobs and its memory benchmarks in several,
because one runner doing every instrumented build, every test binary or every
thin-LTO bench link in sequence was the longest part of a run. This script is
the one place the split is written down; the workflow asks it for the flags of
a shard instead of listing crates or bench targets itself.

A shard must compile what the unsharded job compiled. Cargo resolves features
over the packages a command selects, so selecting part of the workspace can
compile a library with fewer features than a whole-workspace build does: the
shard would then test or measure a library CI never built before. Two
mechanisms keep the units identical, and `check` verifies both from Cargo's
unit graph:

- Test shards set FEATURE_UNIFICATION_ENV (Cargo's unstable `feature-unification
  = "workspace"`), which makes Cargo resolve features over every workspace
  member whatever the package selection is.
- Bench shards keep `--workspace` and only name `--bench` targets, so the
  package selection, and with it the feature resolution, does not change.

Usage:
  ci-shards.py tests engine|runtime    cargo-llvm-cov selection flags
  ci-shards.py benches INDEX TOTAL     --bench flags for shard INDEX (0-based)
  ci-shards.py check [--bench-shards N]
                                       compare every shard's unit graph with
                                       the unsharded build's (N defaults to 3)
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

# The `engine` test shard: the layout, paint and resource crates. Every other
# workspace member, including a crate added later, is tested by `runtime`.
#
# `engine` selects these with `--package`, so its coverage report holds only
# these crates; their tests never execute the other crates' code, so the report
# loses nothing. `runtime` excludes them from the test run but keeps them in its
# report, because bobcat-core's tests execute their code.
ENGINE_CRATES = ["terminus", "seldon", "prime-radiant", "vant"]

TEST_SHARDS = ["engine", "runtime"]

# Set in the environment of the `test` jobs. With both, Cargo resolves features
# over the whole workspace even when the command selects only some packages.
FEATURE_UNIFICATION_ENV = {
    "CARGO_UNSTABLE_FEATURE_UNIFICATION": "true",
    "CARGO_RESOLVER_FEATURE_UNIFICATION": "workspace",
}

DEFAULT_BENCH_SHARDS = 3


def fail(message: str) -> None:
    print(f"ci-shards.py: {message}", file=sys.stderr)
    sys.exit(1)


def run_cargo(args: list[str], env: dict[str, str]) -> str:
    result = subprocess.run(["cargo", *args], capture_output=True, text=True, env=env)
    if result.returncode != 0:
        fail(f"`cargo {' '.join(args)}` failed:\n{result.stderr}")
    return result.stdout


# --- test shards ------------------------------------------------------------


def llvm_cov_selection(shard: str) -> list[str]:
    """Flags that select a test shard for `cargo llvm-cov`."""
    if shard == "engine":
        return [flag for crate in ENGINE_CRATES for flag in ("--package", crate)]
    if shard == "runtime":
        return ["--workspace"] + [
            flag for crate in ENGINE_CRATES for flag in ("--exclude-from-test", crate)
        ]
    fail(f"unknown test shard {shard!r}; expected one of {', '.join(TEST_SHARDS)}")
    raise AssertionError


def cargo_test_selection(shard: str) -> list[str]:
    """The same selection in Cargo's own flags, as cargo-llvm-cov passes it on."""
    if shard == "engine":
        return [flag for crate in ENGINE_CRATES for flag in ("--package", crate)]
    if shard == "runtime":
        return ["--workspace"] + [flag for crate in ENGINE_CRATES for flag in ("--exclude", crate)]
    fail(f"unknown test shard {shard!r}; expected one of {', '.join(TEST_SHARDS)}")
    raise AssertionError


# --- bench shards -----------------------------------------------------------


def bench_targets_by_package() -> dict[str, list[str]]:
    metadata = json.loads(
        run_cargo(
            ["metadata", "--no-deps", "--format-version", "1", "--locked"], dict(os.environ)
        )
    )
    members = set(metadata["workspace_members"])
    by_package: dict[str, list[str]] = {}
    owner: dict[str, str] = {}
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        for target in package["targets"]:
            if "bench" not in target["kind"]:
                continue
            name = target["name"]
            if name in owner:
                # `--bench NAME` would then select both targets, and no flag
                # could put them in different shards or name only one.
                fail(
                    f"bench target {name!r} exists in both {owner[name]} and "
                    f"{package['name']}; bench target names must be unique in the workspace"
                )
            owner[name] = package["name"]
            by_package.setdefault(package["name"], []).append(name)
    return by_package


def assign_bench_shards(total: int) -> list[list[str]]:
    """Whole packages to shards, largest first, each to the emptiest shard.

    Keeping a package's benches together keeps the assignment stable when a
    bench is added to a package; the greedy order keeps the shards' target
    counts close.
    """
    if total < 1:
        fail(f"the shard count must be at least 1, got {total}")
    shards: list[list[str]] = [[] for _ in range(total)]
    packages = sorted(bench_targets_by_package().items(), key=lambda item: (-len(item[1]), item[0]))
    for _, targets in packages:
        emptiest = min(range(total), key=lambda index: (len(shards[index]), index))
        shards[emptiest].extend(targets)
    return [sorted(shard) for shard in shards]


def bench_flags(index: int, total: int) -> list[str]:
    shards = assign_bench_shards(total)
    if not 0 <= index < total:
        fail(f"shard index {index} is out of range for {total} shard(s) (indices are 0-based)")
    if not shards[index]:
        fail(
            f"bench shard {index} of {total} has no bench target; there are fewer "
            f"packages with benches than shards"
        )
    return [flag for name in shards[index] for flag in ("--bench", name)]


# --- check ------------------------------------------------------------------

UnitKey = tuple[str, str, tuple[str, ...], str, str | None, str]


def unit_graph(args: list[str], unify: bool) -> dict[UnitKey, set[tuple[str, ...]]]:
    """Unit key -> every feature set Cargo compiles that unit with.

    One key can carry several feature sets (a crate built for the host and for
    the target, or as a build dependency), so comparisons are subset tests.
    """
    env = {key: value for key, value in os.environ.items() if key not in FEATURE_UNIFICATION_ENV}
    if unify:
        env.update(FEATURE_UNIFICATION_ENV)
    graph = json.loads(run_cargo([*args, "--unit-graph", "-Z", "unstable-options", "--locked"], env))
    units: dict[UnitKey, set[tuple[str, ...]]] = {}
    for unit in graph["units"]:
        key = (
            unit["pkg_id"],
            unit["target"]["name"],
            tuple(unit["target"]["kind"]),
            unit["mode"],
            unit["platform"],
            unit["profile"]["name"],
        )
        units.setdefault(key, set()).add(tuple(sorted(unit["features"])))
    return units


def describe(key: UnitKey) -> str:
    package = key[0].rsplit("/", 1)[-1]
    platform = f" {key[4]}" if key[4] else ""
    return f"{package} [{key[1]} {'/'.join(key[2])} {key[3]} {key[5]}{platform}]"


def compare(label: str, reference: dict, shard: dict) -> list[str]:
    """Problems with shard units the reference build does not have."""
    problems: list[str] = []
    for key, feature_sets in sorted(shard.items(), key=lambda item: describe(item[0])):
        if key not in reference:
            problems.append(f"{label}: {describe(key)} is not in the unsharded build")
            continue
        for features in sorted(feature_sets - reference[key]):
            expected = " | ".join(",".join(f) or "(none)" for f in sorted(reference[key]))
            problems.append(
                f"{label}: {describe(key)} has features {','.join(features) or '(none)'}; "
                f"the unsharded build has {expected}"
            )
            shard_all = set(features)
            reference_all = set().union(*map(set, reference[key]))
            if reference_all - shard_all:
                problems.append(f"    missing in shard: {', '.join(sorted(reference_all - shard_all))}")
            if shard_all - reference_all:
                problems.append(f"    only in shard: {', '.join(sorted(shard_all - reference_all))}")
    return problems


def coverage(label: str, reference: set, shards: dict[str, set]) -> list[str]:
    """Problems unless the shards hold every reference unit exactly once."""
    problems: list[str] = []
    for key in sorted(reference, key=describe):
        owners = [name for name, keys in shards.items() if key in keys]
        if len(owners) != 1:
            where = ", ".join(owners) if owners else "no shard"
            problems.append(f"{label}: {describe(key)} is built by {where}; expected exactly one")
    return problems


def check_tests() -> tuple[list[str], list[str]]:
    reference = unit_graph(["test", "--no-run", "--workspace"], unify=False)
    problems: list[str] = []
    summary: list[str] = []
    test_units: dict[str, set] = {}
    for shard in TEST_SHARDS:
        graph = unit_graph(["test", "--no-run", *cargo_test_selection(shard)], unify=True)
        problems += compare(f"test shard {shard}", reference, graph)
        test_units[shard] = {key for key in graph if key[3] == "test"}
        summary.append(
            f"test shard {shard}: {len(graph)} units, {len(test_units[shard])} test units"
        )
    reference_tests = {key for key in reference if key[3] == "test"}
    # A shard test unit outside the reference is already reported by compare().
    problems += coverage("test units", reference_tests, test_units)
    summary.append(f"unsharded test build: {len(reference)} units, {len(reference_tests)} test units")
    return problems, summary


def check_benches(total: int) -> tuple[list[str], list[str]]:
    reference = unit_graph(["build", "--profile", "bench", "--workspace", "--benches"], unify=False)
    problems: list[str] = []
    summary: list[str] = []
    bench_units: dict[str, set] = {}
    for index, names in enumerate(assign_bench_shards(total)):
        label = f"bench shard {index}"
        if not names:
            problems.append(f"{label}: no bench target assigned")
            continue
        flags = [flag for name in names for flag in ("--bench", name)]
        graph = unit_graph(["build", "--profile", "bench", "--workspace", *flags], unify=False)
        problems += compare(label, reference, graph)
        bench_units[label] = {key for key in graph if "bench" in key[2]}
        summary.append(f"{label}: {len(graph)} units, {len(names)} bench targets: {' '.join(names)}")
    reference_benches = {key for key in reference if "bench" in key[2]}
    problems += coverage("bench targets", reference_benches, bench_units)
    summary.append(
        f"unsharded bench build: {len(reference)} units, {len(reference_benches)} bench targets"
    )
    return problems, summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)
    tests = commands.add_parser("tests", help="cargo-llvm-cov flags for a test shard")
    tests.add_argument("shard", choices=TEST_SHARDS)
    benches = commands.add_parser("benches", help="--bench flags for a bench shard")
    benches.add_argument("index", type=int, help="0-based shard index")
    benches.add_argument("total", type=int, help="number of shards")
    check = commands.add_parser("check", help="compare the shards' unit graphs with the unsharded build")
    check.add_argument("--bench-shards", type=int, default=DEFAULT_BENCH_SHARDS)
    args = parser.parse_args()

    if args.command == "tests":
        print(" ".join(llvm_cov_selection(args.shard)))
        return 0
    if args.command == "benches":
        print(" ".join(bench_flags(args.index, args.total)))
        return 0

    test_problems, test_summary = check_tests()
    bench_problems, bench_summary = check_benches(args.bench_shards)
    for line in test_summary + bench_summary:
        print(line)
    problems = test_problems + bench_problems
    if problems:
        print()
        print("The shards do not build what the unsharded jobs build:")
        for problem in problems:
            print(f"  {problem}")
        print()
        print(
            "A shard compiling a unit with other features tests or measures a library the\n"
            "unsharded build never compiled. Fix the split in .github/scripts/ci-shards.py,\n"
            "or the feature setup that makes the selection matter."
        )
        return 1
    print("every shard unit exists in the unsharded build with the same features")
    return 0


if __name__ == "__main__":
    sys.exit(main())
