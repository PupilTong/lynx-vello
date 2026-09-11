import {
  execFileSync,
  spawnSync,
  type SpawnSyncReturns,
} from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, delimiter, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { cargoClangTargetFeatureFlags } from './wasm-target-features.ts'

const packageDirectory = fileURLToPath(new URL('..', import.meta.url))
const wasmTarget = 'wasm32-unknown-unknown'
const ccEnvironmentName = 'CC_wasm32_unknown_unknown'
const arEnvironmentName = 'AR_wasm32_unknown_unknown'
const expectedWasmOptVersion = '132'
const wasmOptBinDirectory = resolve(packageDirectory, 'node_modules/.bin')
const wasmOptExecutable = resolve(
  wasmOptBinDirectory,
  process.platform === 'win32' ? 'wasm-opt.cmd' : 'wasm-opt',
)
// The package's own TypeScript compiler, installed beside `wasm-opt`.
const tscExecutable = resolve(
  wasmOptBinDirectory,
  process.platform === 'win32' ? 'tsc.cmd' : 'tsc',
)
const wasmClangTargetFeatureFlags = cargoClangTargetFeatureFlags({
  cargo: process.env['CARGO'] ?? 'cargo',
  cwd: packageDirectory,
  target: wasmTarget,
})

function unique(values: (string | undefined)[]): string[] {
  // `filter(Boolean)` drops the absent and empty candidates, which its
  // declared type does not narrow away.
  return [...new Set(values.filter(Boolean))] as string[]
}

function executableCandidates(
  explicit: string | undefined,
  defaults: (string | undefined)[],
): string[] {
  // An explicit target-specific cc-rs setting is a contract: report that
  // command's failure instead of silently compiling with a different tool.
  return explicit ? [explicit] : unique(defaults)
}

function run(
  executable: string,
  args: string[],
  options: { input?: string } = {},
): SpawnSyncReturns<string> {
  return spawnSync(executable, args, {
    encoding: 'utf8',
    stdio: ['pipe', 'pipe', 'pipe'],
    ...options,
  })
}

function formatFailure(result: SpawnSyncReturns<string>): string {
  if (result.error) return result.error.message
  return (result.stderr || result.stdout || `exit status ${result.status}`).trim()
}

function verifyWasmOpt(): string {
  const result = run(wasmOptExecutable, ['--version'])
  if (result.status !== 0) {
    throw new Error(
      [
        `Could not run the pinned Binaryen wasm-opt at '${wasmOptExecutable}':`,
        formatFailure(result),
        'Run `pnpm install` from the workspace root.',
      ].join('\n'),
    )
  }

  const version = `${result.stdout}\n${result.stderr}`.trim()
  if (!new RegExp(`\\bversion ${expectedWasmOptVersion}\\b`).test(version)) {
    throw new Error(
      `Expected Binaryen wasm-opt version ${expectedWasmOptVersion}, got '${version}'`,
    )
  }
  return version
}

function clangCandidates(): string[] {
  const llvmBin = process.env['BOBCAT_WASM_LLVM_BIN']
  return executableCandidates(process.env[ccEnvironmentName], [
    llvmBin && resolve(llvmBin, 'clang'),
    // Homebrew deliberately keeps LLVM keg-only so Apple clang stays first
    // on PATH. Check both Apple Silicon and Intel prefixes explicitly.
    '/opt/homebrew/opt/llvm/bin/clang',
    '/usr/local/opt/llvm/bin/clang',
    'clang-22',
    'clang',
  ])
}

function arCandidates(clang: string): string[] {
  const explicit = process.env[arEnvironmentName]
  const llvmBin = process.env['BOBCAT_WASM_LLVM_BIN']
  const clangName = basename(clang)
  const versionSuffix = /^clang(-[0-9]+)$/.exec(clangName)?.[1] ?? ''
  const clangSibling =
    dirname(clang) === '.' ? undefined : join(dirname(clang), `llvm-ar${versionSuffix}`)
  return executableCandidates(explicit, [
    llvmBin && resolve(llvmBin, 'llvm-ar'),
    clangSibling,
    versionSuffix && `llvm-ar${versionSuffix}`,
    '/opt/homebrew/opt/llvm/bin/llvm-ar',
    '/usr/local/opt/llvm/bin/llvm-ar',
    'llvm-ar-22',
    'llvm-ar',
  ])
}

function selectWasmCToolchain(): { clang: string; llvmAr: string } {
  const probeDirectory = mkdtempSync(join(tmpdir(), 'bobcat-wasm-llvm-'))
  const objectPath = join(probeDirectory, 'probe.o')
  const archivePath = join(probeDirectory, 'probe.a')
  const clangFailures: string[] = []

  try {
    for (const clang of clangCandidates()) {
      const compile = run(
        clang,
        [
          `--target=${wasmTarget}`,
          ...wasmClangTargetFeatureFlags,
          '-x',
          'c',
          '-c',
          '-',
          '-o',
          objectPath,
        ],
        { input: 'int bobcat_wasm_llvm_probe(void) { return 0; }\n' },
      )
      if (compile.status !== 0) {
        clangFailures.push(`${clang}: ${formatFailure(compile)}`)
        continue
      }

      const arFailures: string[] = []
      for (const llvmAr of arCandidates(clang)) {
        rmSync(archivePath, { force: true })
        const archive = run(llvmAr, ['crs', archivePath, objectPath])
        if (archive.status === 0) return { clang, llvmAr }
        arFailures.push(`${llvmAr}: ${formatFailure(archive)}`)
      }

      throw new Error(
        [
          `Clang '${clang}' compiled the Wasm probe,`,
          'but no compatible LLVM archiver was found:',
          ...arFailures,
        ].join('\n'),
      )
    }
  } finally {
    rmSync(probeDirectory, { force: true, recursive: true })
  }

  throw new Error(
    [
      'bobcat-wasm needs LLVM Clang with a WebAssembly backend and these target flags:',
      wasmClangTargetFeatureFlags.join(' '),
      'Apple clang has no WebAssembly backend. Install LLVM 22 or newer',
      '(for example, `brew install llvm` on macOS), then set',
      `${ccEnvironmentName} and ${arEnvironmentName}, or set`,
      'BOBCAT_WASM_LLVM_BIN to its bin directory.',
      ...clangFailures,
    ].join('\n'),
  )
}

const wasmOptVersion = verifyWasmOpt()
console.log(`Wasm optimizer: ${wasmOptVersion}`)

const { clang, llvmAr } = selectWasmCToolchain()
console.log(`QuickJS Wasm C toolchain: ${clang} + ${llvmAr}`)

rmSync(new URL('../pkg/', import.meta.url), { force: true, recursive: true })
// `dist/` also holds the compiler's build info, so clearing it makes the
// TypeScript emit below a full one.
rmSync(new URL('../dist/', import.meta.url), { force: true, recursive: true })
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
    env: {
      ...process.env,
      PATH: [wasmOptBinDirectory, process.env['PATH']].filter(Boolean).join(delimiter),
      [ccEnvironmentName]: clang,
      [arEnvironmentName]: llvmAr,
    },
    stdio: 'inherit',
  },
)

await import('./prepare-pkg.ts')

// The browser sources emit last: the Worker program is typed against the
// `pkg/bobcat_wasm.d.ts` wasm-pack has just generated.
execFileSync(
  tscExecutable,
  ['-b', 'js/tsconfig.json', 'js/tsconfig.worker.json'],
  { cwd: packageDirectory, stdio: 'inherit' },
)
