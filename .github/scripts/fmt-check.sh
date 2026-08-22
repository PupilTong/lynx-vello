#!/usr/bin/env bash
# Format-checks every workspace member, and nothing else.
#
# `cargo fmt --all` cannot be used: it follows path dependencies out of the
# workspace and into `vendor/stylo`, where upstream's formatting is not ours to
# change. The alternative CI used was a hand-written `-p` list, which drifts in
# exactly one direction — silently. `lynx-xml` had been missing from it since
# the crate was added.
#
# Asking Cargo for the member list makes that drift impossible instead of
# merely unlikely: a new crate is covered the moment it joins the workspace.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

# No `mapfile`: the macOS runner's /bin/bash is 3.2.
packages=""
while IFS= read -r package; do
  packages="$packages -p $package"
done < <(
  cargo metadata --no-deps --format-version 1 \
    | python3 -c 'import json, sys
for package in sorted(json.load(sys.stdin)["packages"], key=lambda p: p["name"]):
    print(package["name"])'
)

if [ -z "$packages" ]; then
  echo "cargo metadata reported no workspace members" >&2
  exit 1
fi

echo "format-checking workspace members:$packages"
# Word splitting is the point here: $packages is a flag list, not a filename.
# shellcheck disable=SC2086
cargo fmt --check $packages "$@"

# The fuzz package declares its own workspace (see fuzz/Cargo.toml), so the
# member list above cannot reach it. Named explicitly rather than with `--all`,
# which walks back up into the parent workspace and on into vendor/stylo.
echo "format-checking the out-of-workspace fuzz package"
cargo fmt --check --manifest-path fuzz/Cargo.toml -p lynx-vello-fuzz "$@"
