#!/usr/bin/env bash
# Runs the fuzz targets of `crates/quickjs-rust-bridge/fuzz` under
# AddressSanitizer (and LeakSanitizer, where the platform has it).
#
#   fuzz-quickjs.sh replay            each corpus and seed input once, no
#                                     mutation: deterministic (pull requests)
#   fuzz-quickjs.sh fuzz <seconds>    mutate for <seconds> per target, adding
#                                     what reaches new code to the corpus
#                                     (pushes to main)
#
# The corpus is `fuzz/corpus/<target>`, which CI caches between runs; a
# crashing input is written to `fuzz/artifacts/<target>/`. Seeds are QuickJS's
# own tests and examples, plus the module graphs in `fuzz/seeds/modules`.
# Needs `cargo-fuzz` and, for the engine's C, Clang (`CC`).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../../crates/quickjs-rust-bridge"

mode="${1:?usage: fuzz-quickjs.sh replay | fuzz <seconds>}"
case "$mode" in
  replay) run_flags="-runs=0" ;;
  fuzz) run_flags="-max_total_time=${2:?fuzz needs a number of seconds per target}" ;;
  *)
    echo "unknown mode: $mode" >&2
    exit 2
    ;;
esac

seeds="fuzz/target/seeds"
rm -rf "$seeds"
mkdir -p "$seeds"
cp ../../vendor/quickjs/tests/*.js ../../vendor/quickjs/examples/*.js "$seeds/"

# The target rustc builds for, named explicitly: `cargo fuzz` otherwise uses
# the triple its own binary was built for, and a prebuilt Linux cargo-fuzz is
# a musl build, which AddressSanitizer cannot instrument.
target_triple="$(rustc -vV | sed -n 's/^host: //p')"

cargo fuzz build --target "$target_triple"

for target in script modules; do
  corpus="fuzz/corpus/$target"
  artifacts="fuzz/artifacts/$target"
  mkdir -p "$corpus" "$artifacts"
  extra_seeds=""
  if [ "$target" = modules ]; then
    extra_seeds="fuzz/seeds/modules"
  fi
  echo "::group::$target ($mode)"
  # New inputs go to the first directory, the corpus. Word splitting of
  # `$run_flags` and `$extra_seeds` is intended: each is one flag or none.
  # shellcheck disable=SC2086
  cargo fuzz run --target "$target_triple" "$target" "$corpus" "$seeds" $extra_seeds -- \
    -dict=fuzz/js.dict \
    -timeout=10 \
    -rss_limit_mb=4096 \
    -artifact_prefix="$artifacts/" \
    $run_flags
  echo "::endgroup::"
done
