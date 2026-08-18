import assert from 'node:assert/strict'
import { dirname, resolve } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import {
  cargoClangTargetFeatureFlags,
  clangTargetFeatureFlags,
} from './wasm-target-features.mjs'

test('translates split and joined Rust codegen options', () => {
  assert.deepEqual(
    clangTargetFeatureFlags([
      '-C',
      'target-feature=+atomics,+bulk-memory-opt',
      '-Clink-arg=--shared-memory',
      '-Ctarget-feature=-relaxed-simd,+simd128',
    ]),
    ['-matomics', '-mbulk-memory-opt', '-mno-relaxed-simd', '-msimd128'],
  )
})

test('rejects absent and malformed target features', () => {
  assert.throws(() => clangTargetFeatureFlags(['-C', 'link-arg=--shared-memory']))
  assert.throws(() => clangTargetFeatureFlags(['-Ctarget-feature=simd128']))
  assert.throws(() => clangTargetFeatureFlags(['-Ctarget-feature=+simd 128']))
})

test('loads the canonical Wasm features from Cargo configuration', () => {
  const packageDirectory = resolve(dirname(fileURLToPath(import.meta.url)), '..')
  const flags = cargoClangTargetFeatureFlags({
    cargo: process.env.CARGO ?? 'cargo',
    cwd: packageDirectory,
    target: 'wasm32-unknown-unknown',
  })
  assert.ok(flags.length > 0)
  assert.equal(new Set(flags).size, flags.length)
})
