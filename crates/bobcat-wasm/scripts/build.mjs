import { execFileSync, spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const packageDirectory = fileURLToPath(new URL('..', import.meta.url))
const wasmTarget = 'wasm32-unknown-unknown'
const ccEnvironmentName = 'CC_wasm32_unknown_unknown'
const arEnvironmentName = 'AR_wasm32_unknown_unknown'
const requiredClangFlags = [
  '-matomics',
  '-mbulk-memory',
  '-mbulk-memory-opt',
  '-mextended-const',
  '-mmultivalue',
  '-mmutable-globals',
  '-mnontrapping-fptoint',
  '-mreference-types',
  '-mrelaxed-simd',
  '-msign-ext',
  '-msimd128',
  '-mtail-call',
]

function unique(values) {
  return [...new Set(values.filter(Boolean))]
}

function executableCandidates(explicit, defaults) {
  // An explicit target-specific cc-rs setting is a contract: report that
  // command's failure instead of silently compiling with a different tool.
  return explicit ? [explicit] : unique(defaults)
}

function run(executable, args, options = {}) {
  return spawnSync(executable, args, {
    encoding: 'utf8',
    stdio: ['pipe', 'pipe', 'pipe'],
    ...options,
  })
}

function formatFailure(result) {
  if (result.error) return result.error.message
  return (result.stderr || result.stdout || `exit status ${result.status}`).trim()
}

function clangCandidates() {
  const llvmBin = process.env.BOBCAT_WASM_LLVM_BIN
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

function arCandidates(clang) {
  const explicit = process.env[arEnvironmentName]
  const llvmBin = process.env.BOBCAT_WASM_LLVM_BIN
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

function selectWasmCToolchain() {
  const probeDirectory = mkdtempSync(join(tmpdir(), 'bobcat-wasm-llvm-'))
  const objectPath = join(probeDirectory, 'probe.o')
  const archivePath = join(probeDirectory, 'probe.a')
  const clangFailures = []

  try {
    for (const clang of clangCandidates()) {
      const compile = run(
        clang,
        [`--target=${wasmTarget}`, ...requiredClangFlags, '-x', 'c', '-c', '-', '-o', objectPath],
        { input: 'int bobcat_wasm_llvm_probe(void) { return 0; }\n' },
      )
      if (compile.status !== 0) {
        clangFailures.push(`${clang}: ${formatFailure(compile)}`)
        continue
      }

      const arFailures = []
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
      requiredClangFlags.join(' '),
      'Apple clang has no WebAssembly backend. Install LLVM 22 or newer',
      '(for example, `brew install llvm` on macOS), then set',
      `${ccEnvironmentName} and ${arEnvironmentName}, or set`,
      'BOBCAT_WASM_LLVM_BIN to its bin directory.',
      ...clangFailures,
    ].join('\n'),
  )
}

const { clang, llvmAr } = selectWasmCToolchain()
console.log(`QuickJS Wasm C toolchain: ${clang} + ${llvmAr}`)

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
    env: {
      ...process.env,
      [ccEnvironmentName]: clang,
      [arEnvironmentName]: llvmAr,
    },
    stdio: 'inherit',
  },
)

await import('./prepare-pkg.mjs')
