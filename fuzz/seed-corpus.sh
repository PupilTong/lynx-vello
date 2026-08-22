#!/usr/bin/env bash
# Populates each target's corpus from fixtures already in the repository.
#
# libFuzzer starting from nothing spends nearly its whole budget failing the
# `SDRA`/`WROF` magic or the `<?xml` prologue, so an unseeded run reports
# millions of executions while never reaching a section decoder. Seeding costs
# nothing and is the difference between fuzzing the format and fuzzing the
# first four bytes of it.
#
# Idempotent: re-running only re-copies the seeds. Corpora are not committed
# (see .gitignore) — CI restores the accumulated one from cache and reseeds on
# top of it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
corpus="$root/fuzz/corpus"

mkdir -p "$corpus/template_container" "$corpus/template_style_info" "$corpus/lynx_xml"

# Whole bundles: the container target consumes these as-is.
for bundle in "$root"/crates/lynx-template-decoder/tests/fixtures/*.web.bundle; do
  [ -e "$bundle" ] || continue
  cp "$bundle" "$corpus/template_container/seed-$(basename "$bundle")"
done

# The style-info target is handed a bare section body, not a bundle, so extract
# the StyleInfo section (label 2) out of each fixture first.
for bundle in "$root"/crates/lynx-template-decoder/tests/fixtures/*.web.bundle; do
  [ -e "$bundle" ] || continue
  python3 "$root/fuzz/extract-style-info.py" "$bundle" \
    "$corpus/template_style_info/seed-$(basename "$bundle" .web.bundle)"
done

for xml in "$root"/packages/github-pages/public/*.lynx.xml; do
  [ -e "$xml" ] || continue
  cp "$xml" "$corpus/lynx_xml/seed-$(basename "$xml")"
done

echo "seeded:"
for target in template_container template_style_info lynx_xml; do
  echo "  $target: $(find "$corpus/$target" -type f | wc -l | tr -d ' ') input(s)"
done
