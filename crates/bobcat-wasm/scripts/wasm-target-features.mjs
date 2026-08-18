import { execFileSync } from 'node:child_process'

export function clangTargetFeatureFlags(rustflags) {
  if (!Array.isArray(rustflags) || !rustflags.every((flag) => typeof flag === 'string')) {
    throw new TypeError('Cargo target rustflags must be an array of strings')
  }

  const clangFlags = []
  for (let index = 0; index < rustflags.length; index += 1) {
    const rustflag = rustflags[index]
    let codegenOption
    if (rustflag === '-C') {
      index += 1
      codegenOption = rustflags[index]
      if (codegenOption === undefined) throw new Error("Cargo rustflags ends with a bare '-C'")
    } else if (rustflag.startsWith('-C')) {
      codegenOption = rustflag.slice(2)
    } else {
      continue
    }

    if (!codegenOption.startsWith('target-feature=')) continue
    for (const feature of codegenOption.slice('target-feature='.length).split(',')) {
      if (feature.length === 0) continue
      const prefix = feature[0] === '+' ? '-m' : feature[0] === '-' ? '-mno-' : undefined
      const name = feature.slice(1)
      if (prefix === undefined || !/^[A-Za-z0-9_-]+$/.test(name)) {
        throw new Error(`Rust target feature '${feature}' cannot be translated to a Clang flag`)
      }
      clangFlags.push(`${prefix}${name}`)
    }
  }

  if (clangFlags.length === 0) {
    throw new Error(
      'wasm32-unknown-unknown must configure an explicit Rust target-feature list; ' +
        'the QuickJS C flags are derived from it',
    )
  }
  return clangFlags
}

export function cargoClangTargetFeatureFlags({ cargo = 'cargo', cwd, target }) {
  const key = `target.${target}.rustflags`
  let output
  try {
    output = execFileSync(
      cargo,
      ['-Z', 'unstable-options', 'config', 'get', key, '--format', 'json'],
      { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] },
    )
  } catch (error) {
    const detail = error.stderr?.trim() || error.message
    throw new Error(`Could not read ${key} through Cargo: ${detail}`, { cause: error })
  }

  let config
  try {
    config = JSON.parse(output)
  } catch (error) {
    throw new Error(`Cargo returned invalid JSON for ${key}`, { cause: error })
  }
  return clangTargetFeatureFlags(config?.target?.[target]?.rustflags)
}
