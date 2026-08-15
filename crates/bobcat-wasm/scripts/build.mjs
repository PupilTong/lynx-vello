import { execFileSync } from 'node:child_process'
import { rmSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const packageDirectory = fileURLToPath(new URL('..', import.meta.url))
rmSync(new URL('../pkg/', import.meta.url), { force: true, recursive: true })
execFileSync(
  'wasm-pack',
  [
    'build',
    '.',
    '--target',
    'web',
    '--release',
    '--out-dir',
    'pkg',
    '--out-name',
    'bobcat_wasm',
    '--no-pack',
    '--',
    '-Z',
    'build-std=std,panic_abort',
  ],
  {
    cwd: packageDirectory,
    stdio: 'inherit',
  },
)

await import('./prepare-pkg.mjs')
