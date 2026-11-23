# Dev profile debug info and target size

The `dev` profile (and `test`, which inherits it) builds workspace crates with
`debug = "line-tables-only"` and every dependency, `vendor/stylo` included,
with `debug = false`. Panic messages and backtraces still name file and line
in workspace code; a debugger shows no local variables anywhere and no source
lines inside dependencies. The reason is disk: on macOS debug info stays in the
object files Cargo keeps beside each artifact, and a clean
`cargo build --workspace --all-targets` measured 26.2 GB with full debug info
against 14.0 GB with these settings.

**When a problem is hard to debug, raise the level for that build rather than
working around the missing information** — stepping through code in `lldb`,
reading locals in a crash, or a backtrace that stops at a dependency frame are
all reasons to. Use a command-line override, so the committed profile stays as
it is:

```sh
# Full debug info for workspace crates
CARGO_PROFILE_DEV_DEBUG=true cargo test -p <crate> <filter>

# ...and for dependencies too (stylo, vello_gpu, parley, ...)
cargo test -p <crate> <filter> \
  --config 'profile.dev.debug=true' \
  --config 'profile.dev.package."*".debug=true'

# One dependency only
cargo test -p <crate> <filter> --config 'profile.dev.package.stylo.debug=true'
```

Changing the level recompiles the affected crates and writes a second set of
artifacts next to the first, so go back to the plain command once the problem
is understood. Do not commit a raised level in `Cargo.toml`.

Cargo never deletes superseded artifacts, and each edit, branch switch or
feature combination adds a new hashed set: a long-lived worktree's `target`
grows without bound, mostly in `debug/deps` and `debug/incremental`. With no
build running in that checkout, remove what has not been used recently with
[`cargo-sweep`](https://github.com/holmgr/cargo-sweep)
(`cargo install cargo-sweep`):

```sh
cargo sweep --time 3          # this checkout: artifacts unused for 3 days
cargo sweep --time 3 -r ..    # every Cargo project below a directory
```

It only deletes build output; the next build recompiles whatever it needed
after all. For a worktree, `worktree-cow.py seed --replace` (above) is the
complete reset.
