# lynx-vello

A Rust workspace for working with the [LynxJS](https://github.com/lynx-family/lynx)
runtime. The first deliverable is **`reactlynx-decoder`** — a from-scratch,
idiomatic-Rust decoder for the ReactLynx *native binary template* format
(template-bundle magic `0x00241922`).

## Scope

The decoder targets the **latest feature subset** of the format. Where the
encoder selects behavior with a switch, we decode the new variant and skip the
legacy one:

| Area            | Implemented (latest)             | Skipped (legacy)                    |
| --------------- | -------------------------------- | ----------------------------------- |
| Element trees   | `NEW_ELEMENT_TEMPLATE` section   | `ELEMENT_TEMPLATE`, radon/vnode tree |
| Styles          | `STYLE_OBJECT` / parsed styles   | inline TTSS string re-parse          |
| Page descriptor | binary `CONFIG` / page config    | —                                   |

See [`docs/lynx/`](docs/lynx) for the reverse-engineered format reference that
drives the implementation.

## Layout

```
crates/
  reactlynx-decoder/   # the decoder library
docs/lynx/             # binary-format reference (the "repo knowledge")
.claude/skills/        # repo skill summarizing the format for future sessions
```

## Toolchain

Pinned **nightly** (`rust-toolchain.toml`). Edition 2024, resolver 3,
workspace-inherited lints.

```sh
cargo build
cargo test
cargo clippy --all-targets
cargo fmt
```

CI runs on an aarch64 (Apple Silicon) macOS runner — see
[`.github/workflows/ci.yml`](.github/workflows/ci.yml).
