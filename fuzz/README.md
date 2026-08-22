# Fuzz targets

Machine verification that the two parsers fed **untrusted external bytes**
reject malformed input instead of panicking.

Both are written in the `Result` style, and both are covered by unit tests over
inputs a *correct* encoder produces. Neither had anything asserting the
property that actually matters at an embedder's trust boundary: that no byte
string — however hostile — can take the process down. That is what these
targets assert, and it is not a property a hand-written test suite establishes.

| Target | Entry point | What it covers |
| --- | --- | --- |
| `template_container` | `lynx_template_decoder::decode` | `.web.bundle` framing: magic, version, section table, UTF-16 JSON payloads, length-prefixed string maps |
| `template_style_info` | the same, around a synthetic one-section container | the rkyv `StyleInfo` archive: `check_archived_root` validation and the deserialize that follows it |
| `lynx_xml` | `lynx_xml::parse` | the Lynx XML source envelope, plus the borrow and offset invariants its callers depend on |

`template_style_info` exists separately because reaching the rkyv path through
`decode` alone would spend nearly every execution on the container header. It
builds the smallest valid 20-byte envelope around the fuzzer's bytes; only that
envelope is synthetic, the decoder under test is the real one. Two unit tests
in `src/lib.rs` assert the envelope really does reach the section decoder — a
silently wrong envelope would report millions of executions while fuzzing
nothing.

## Running

```sh
./fuzz/seed-corpus.sh              # seed from the fixtures already in the repo
cargo fuzz run template_container  # ^C to stop
```

Seeding is not optional in practice. Unseeded, almost every execution stops at
the `SDRA`/`WROF` magic or the `<?xml` prologue and no section decoder is ever
reached.

To reproduce a crash CI reported, download the artifact it uploaded and pass
the input file:

```sh
cargo fuzz run template_container fuzz/artifacts/template_container/crash-<hash>
```

`cargo fuzz fmt <target> <input>` prints the `Arbitrary`-decoded form of an
input, which is what you want for `lynx_xml` — its target takes `&str`, so the
raw artifact bytes are not the string the parser saw.

## Why this is not in the workspace

The package is excluded in the root `Cargo.toml` and declares its own
`[workspace]`. `cargo fuzz` builds with libFuzzer and a sanitizer runtime under
its own profile; letting that into the workspace resolve would change what
`cargo clippy`, `cargo llvm-cov` and `cargo codspeed` build. It is exercised by
`.github/workflows/fuzz.yml` instead — a short smoke run on every pull request
that touches a parser, and a nightly run with a cached, accumulating corpus.

## Scope

Panic-freedom and, for `lynx_xml`, two API invariants. **Not** correctness of
successful decodes: an input that decodes to the wrong thing without panicking
passes. Round-trip correctness belongs in the crates' own test suites, which is
where it already lives.
