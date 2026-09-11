// Emits the realm runtime's JavaScript with TypeScript 7 into `dist/`.
//
// `dist/` is committed: `bobcat-core` embeds those files, so a cargo build
// needs no Node. Each file starts with its source's name and that source's
// FNV-1a hash, which `bobcat-core`'s build script recomputes, so a source
// edited after its last emit fails the Rust build rather than being embedded
// as the older code. CI reruns this and fails on any difference in `dist/`.

import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const packageDirectory = fileURLToPath(new URL("..", import.meta.url));
const dist = path.join(packageDirectory, "dist");
const tsc = path.join(
  packageDirectory,
  "node_modules/.bin",
  process.platform === "win32" ? "tsc.cmd" : "tsc",
);

/** FNV-1a over the bytes, as 16 hex digits; `bobcat-core/build.rs` agrees. */
function fnv1a64(bytes: Uint8Array): string {
  let hash = 0xcbf29ce484222325n;
  for (const byte of bytes) {
    hash = ((hash ^ BigInt(byte)) * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return hash.toString(16).padStart(16, "0");
}

// Cleared first, so a module removed from `src/` leaves no file behind.
rmSync(dist, { force: true, recursive: true });
// The type-checking program, emitting: the realm program's options are the
// emit's, and every output is rewritten each time.
execFileSync(
  tsc,
  ["-p", "src", "--noEmit", "false", "--incremental", "false", "--outDir", "dist"],
  { cwd: packageDirectory, stdio: "inherit" },
);
for (const file of readdirSync(dist)) {
  const name = path.basename(file, ".js");
  const source = readFileSync(path.join(packageDirectory, "src", `${name}.ts`));
  const emitted = readFileSync(path.join(dist, file), "utf8");
  writeFileSync(
    path.join(dist, file),
    `// Generated from src/${name}.ts by TypeScript 7: edit that file and run \`pnpm --filter bobcat-element build\`.\n` +
      `// source fnv1a64 ${fnv1a64(source)}\n` +
      emitted,
  );
}
