# nanoprintf

This directory vendors the single-header nanoprintf formatter used by
`quickjs-rust-bridge`'s private C platform layer.

- Upstream: <https://github.com/charlesnicholson/nanoprintf>
- Version: `v0.6.0`
- Commit: `e3a1e65c7ae0699d26d25d68f6c3e21babfe31ca`
- Source: <https://github.com/charlesnicholson/nanoprintf/blob/e3a1e65c7ae0699d26d25d68f6c3e21babfe31ca/nanoprintf.h>
- Vendored on: 2026-08-18
- Selected license: `0BSD` (upstream also offers the Unlicense)
- Local modifications: none

Checksums of the unmodified upstream files:

```text
5f97d8ac642d374c31e032dda0168ef45c005e61828ac48a39f8d309c06f6b52  nanoprintf.h
161c07e5ac2244db45e53b7923df8364c2d96fd1dd59804faf7f817e32a6ca98  LICENSE
```

## Updating

1. Resolve the desired upstream release to its full commit SHA.
2. Replace `nanoprintf.h` and `LICENSE` with the unmodified files from that
   commit.
3. Update the version, commit, vendoring date, and SHA-256 checksums above.
4. Re-run the `quickjs-rust-bridge` tests and the QuickJS-enabled
   `wasm32-unknown-unknown` final-link/undefined-symbol audit.

Formatter configuration and Bobcat's namespaced wrapper belong in
`src/platform_printf.c`; do not patch the vendored header.
