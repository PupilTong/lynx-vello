import { rm } from 'node:fs/promises'

// wasm-pack creates this nested ignore file even with --no-pack. Leaving it in
// place makes npm omit the generated glue and shared-memory module despite the
// package's explicit `files` allowlist.
await rm(new URL('../pkg/.gitignore', import.meta.url), { force: true })
