# bobcat

One native embedder crate with two independent Cargo features. Both are
enabled by default; use `--no-default-features` to build only one product.

```sh
cargo run -p bobcat --no-default-features --features cli --bin bobcat -- --help
LYNX_USE_PORT=8080 cargo run -p bobcat --no-default-features --features server --bin bobcat-server
cargo build -p bobcat --all-features --bins
```

- `cli`: the `bobcat` binary, interactive window/headless runner and PNG output.
  See [CLI usage](CLI.md).
- `server`: the `bobcat-server` binary, HTTP screenshot endpoint and BMP output.
  See [server usage](SERVER.md).

Each binary requires its corresponding feature. Both products use the full
`bobcat-source` and `bobcat-resources` implementations; neither duplicates the
engine pipeline. Server-only builds do not enable the CLI window backend, and
CLI-only builds do not enable the HTTP server dependencies.
