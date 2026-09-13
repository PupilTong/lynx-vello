// Emits the realm runtime with the workspace's TypeScript 7 compiler.
// Cargo passes its private output directory; the package command defaults to
// ignored `dist/` for inspecting the emitted JavaScript locally.

import { execFileSync } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const packageDirectory = fileURLToPath(new URL("..", import.meta.url));
const outDir = path.resolve(packageDirectory, process.argv[2] ?? "dist");
const tsc = fileURLToPath(new URL("./bin/tsc", import.meta.resolve("typescript/package.json")));

// Removed modules must leave no stale output behind. Cargo's directory is
// private to one build, so concurrent target/profile builds cannot race here.
rmSync(outDir, { force: true, recursive: true });
execFileSync(
  process.execPath,
  [tsc, "-p", "src", "--noEmit", "false", "--incremental", "false", "--outDir", outDir],
  { cwd: packageDirectory, stdio: "inherit" },
);
