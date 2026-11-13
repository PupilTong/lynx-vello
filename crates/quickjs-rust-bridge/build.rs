use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo always provides CARGO_MANIFEST_DIR"),
    );
    let quickjs_dir = manifest_dir.join("../../vendor/quickjs");
    let version_path = quickjs_dir.join("VERSION");
    let version = fs::read_to_string(&version_path)
        .expect("the pinned QuickJS submodule must contain VERSION");
    let version_define = format!("\"{}\"", version.trim());
    let platform_header = manifest_dir.join("src/rust-platform.h");
    let platform_printf_source = manifest_dir.join("src/platform_printf.c");
    let nanoprintf_dir = manifest_dir.join("vendor/nanoprintf");
    let nanoprintf_header = nanoprintf_dir.join("nanoprintf.h");
    let target = env::var("TARGET").expect("Cargo always provides TARGET");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo always provides target OS");
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let clang_target_feature_flags = if target == "wasm32-unknown-unknown" {
        clang_target_feature_flags_from_cargo()
    } else {
        Vec::new()
    };

    assert_supported_target(&target_os, &target_env);

    let sources = [
        "quickjs.c",
        "dtoa.c",
        "libregexp.c",
        "libunicode.c",
        "cutils.c",
    ];
    let headers = [
        "quickjs.h",
        "quickjs-atom.h",
        "quickjs-opcode.h",
        "cutils.h",
        "dtoa.h",
        "libregexp.h",
        "libregexp-opcode.h",
        "libunicode.h",
        "libunicode-table.h",
        "list.h",
    ];

    let configure_quickjs_build = || {
        let mut build = cc::Build::new();
        build
            .define("CONFIG_VERSION", Some(version_define.as_str()))
            .define("QJS_NO_JS_SHARED_MEMORY", None)
            .define("QJS_NO_STDIO_DIAGNOSTICS", None)
            .define("QJS_RUST_ALLOCATOR", None)
            .define("QJS_RUST_TIME_HOST", None)
            .define("QJS_RUST_TIMEZONE_HOST", None)
            .include(manifest_dir.join("src/include"))
            .flag("-include")
            .flag(&platform_header)
            .flag_if_supported("-std=gnu11");
        configure_target(&mut build, &target, &clang_target_feature_flags);
        build
    };

    let mut shim_build = configure_quickjs_build();
    shim_build
        .flag("-isystem")
        .flag(&quickjs_dir)
        .file(manifest_dir.join("src/shim.c"))
        .warnings(true)
        .warnings_into_errors(true)
        .compile("quickjs_bridge_shim");

    let mut quickjs_build = configure_quickjs_build();
    quickjs_build.include(&quickjs_dir).warnings(false);
    for source in sources {
        quickjs_build.file(quickjs_dir.join(source));
    }
    quickjs_build.compile("quickjs_bridge_core");

    // Keep the formatter in its own translation unit. In particular, do not
    // force-include `rust-platform.h`: that header maps `vsnprintf` to this
    // wrapper and would make the implementation recurse into itself.
    let mut printf_build = cc::Build::new();
    configure_target(&mut printf_build, &target, &clang_target_feature_flags);
    printf_build
        .include(&nanoprintf_dir)
        .file(&platform_printf_source)
        .flag_if_supported("-std=gnu11")
        .warnings(true)
        .warnings_into_errors(true)
        .compile("quickjs_bridge_printf");

    if env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        println!("cargo:rustc-link-lib=m");
    }

    println!("cargo:rerun-if-changed={}", version_path.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src/shim.c").display()
    );
    rerun_if_changed(&platform_printf_source);
    rerun_if_changed(&nanoprintf_header);
    rerun_if_changed(&platform_header);
    rerun_if_changed(&manifest_dir.join("src/rust-allocator.h"));
    rerun_if_platform_headers_changed(&manifest_dir);
    for source in sources {
        rerun_if_changed(&quickjs_dir.join(source));
    }
    for header in headers {
        rerun_if_changed(&quickjs_dir.join(header));
    }

    export_engine_sources_digest(&manifest_dir, &quickjs_dir, &sources, &headers);
}

/// Exports `QJS_ENGINE_SOURCES_DIGEST`, the build half of the bridge's
/// `bytecode_engine_id`.
fn export_engine_sources_digest(
    manifest_dir: &Path,
    quickjs_dir: &Path,
    sources: &[&str],
    headers: &[&str],
) {
    // Everything that decides whether bytecode one build writes can be read
    // by another: the engine's C sources and headers, the version file they
    // are built under, the shim, and this script, whose defines change what
    // the engine compiles (its atom table among them). Each file is hashed
    // under a path relative to the crate, in sorted order, so the digest is
    // the same wherever the checkout lives. Every file named here already
    // has its `rerun-if-changed` in `main`, and Cargo reruns a changed build
    // script on its own.
    let mut engine_files: Vec<(String, PathBuf)> = sources
        .iter()
        .chain(headers.iter())
        .chain(std::iter::once(&"VERSION"))
        .map(|name| (format!("vendor/quickjs/{name}"), quickjs_dir.join(name)))
        .chain([
            ("src/shim.c".to_owned(), manifest_dir.join("src/shim.c")),
            ("build.rs".to_owned(), manifest_dir.join("build.rs")),
        ])
        .collect();
    engine_files.sort();
    let mut digest = Fnv1a::new();
    for (label, path) in &engine_files {
        let contents = fs::read(path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        digest.write(label.as_bytes());
        digest.write(&[0]);
        digest.write(&(contents.len() as u64).to_le_bytes());
        digest.write(&contents);
    }
    println!(
        "cargo:rustc-env=QJS_ENGINE_SOURCES_DIGEST={:016x}",
        digest.finish()
    );
}

/// 64-bit FNV-1a: stable across Rust releases and platforms, which
/// `std::hash` does not promise.
struct Fnv1a(u64);

impl Fnv1a {
    const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    const fn finish(&self) -> u64 {
        self.0
    }
}

/// The instrumentation this build gives Rust code, which the C objects linked
/// into the same binary must carry as well.
///
/// Both come from `RUSTFLAGS`, so a sanitized or fuzzing build passes no C
/// flags of its own. `-Zsanitizer=<name>` sets `cfg(sanitize = "<name>")` and
/// links that sanitizer's runtime, which C objects instrumented for the same
/// sanitizer report through. `cargo fuzz` builds with `--cfg fuzzing` and
/// `SanitizerCoverage` for Rust; libFuzzer can steer by the engine's branches
/// only when its C carries the same counters. The C compiler must be LLVM's
/// Clang (`CC`): Apple's names a runtime of its own in every instrumented
/// object (`__asan_version_mismatch_check_apple_clang_*`), which rustc's does
/// not define, so nothing it instruments links.
struct Instrumentation {
    sanitizers: Vec<String>,
    fuzzing: bool,
}

impl Instrumentation {
    fn from_cargo() -> Self {
        let sanitizers: Vec<String> = env::var("CARGO_CFG_SANITIZE")
            .unwrap_or_default()
            .split(',')
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        for name in &sanitizers {
            assert!(
                matches!(
                    name.as_str(),
                    "address" | "hwaddress" | "leak" | "memory" | "thread"
                ),
                "-Zsanitizer={name} has no Clang counterpart for the QuickJS C sources"
            );
        }
        Self {
            sanitizers,
            fuzzing: env::var_os("CARGO_CFG_FUZZING").is_some(),
        }
    }

    fn apply(&self, build: &mut cc::Build) {
        for name in &self.sanitizers {
            build.flag(format!("-fsanitize={name}"));
        }
        if self.sanitizers.iter().any(|name| name == "address") {
            // Clang defines `__SANITIZE_ADDRESS__` here, and QuickJS then
            // gives every allocation to the host allocator instead of
            // carving small ones out of its 4 KiB arenas, so the sanitizer
            // sees each object's bounds and lifetime.
            //
            // Rust has no UndefinedBehaviorSanitizer, but rustc's
            // AddressSanitizer runtime carries its handlers, so the C is
            // checked for undefined behaviour too, and the first report
            // aborts. `function` is left out: QuickJS calls functions
            // through pointer types whose parameters differ from theirs by
            // design (`js_realloc_rt`, taking a `JSRuntime *`, is called as
            // a `DynBufReallocFunc`, taking a `void *`).
            build
                .flag("-fsanitize=undefined")
                .flag("-fno-sanitize=function")
                .flag("-fno-sanitize-recover=undefined");
        }
        if !self.sanitizers.is_empty() {
            // Reports unwind through C frames by frame pointer.
            build.flag("-fno-omit-frame-pointer");
        }
        if self.fuzzing {
            build.flag("-fsanitize=fuzzer-no-link");
        }
    }
}

fn assert_supported_target(target_os: &str, target_env: &str) {
    assert!(
        target_os != "windows" || target_env == "gnu",
        "the pinned QuickJS C sources support Windows through GNU/MinGW, not the MSVC ABI"
    );
}

/// What every C object of this build takes from the Rust build it links
/// into: the target's features and Rust's instrumentation.
fn configure_target(build: &mut cc::Build, target: &str, clang_target_feature_flags: &[String]) {
    Instrumentation::from_cargo().apply(build);
    build.flag_if_supported("-fwrapv");
    if target == "wasm32-unknown-unknown" {
        // These describe C object code only; the QuickJS build still omits
        // its JavaScript Atomics and SharedArrayBuffer intrinsics.
        for flag in clang_target_feature_flags {
            build.flag(flag);
        }
    }
}

fn clang_target_feature_flags_from_cargo() -> Vec<String> {
    let encoded = env::var("CARGO_ENCODED_RUSTFLAGS")
        .expect("Cargo always provides CARGO_ENCODED_RUSTFLAGS to build scripts");
    let rustflags: Vec<_> = encoded.split('\u{1f}').collect();
    let mut clang_flags = Vec::new();
    let mut index = 0;

    while index < rustflags.len() {
        let rustflag = rustflags[index];
        let codegen_option = if rustflag == "-C" {
            index += 1;
            rustflags.get(index).copied()
        } else {
            rustflag.strip_prefix("-C")
        };

        if let Some(features) =
            codegen_option.and_then(|option| option.strip_prefix("target-feature="))
        {
            for feature in features.split(',').filter(|feature| !feature.is_empty()) {
                let (clang_prefix, name) = match feature.as_bytes()[0] {
                    b'+' => ("-m", &feature[1..]),
                    b'-' => ("-mno-", &feature[1..]),
                    _ => panic!("Rust target feature '{feature}' must start with '+' or '-'"),
                };
                assert!(
                    !name.is_empty()
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
                    "Rust target feature '{feature}' cannot be translated to a Clang flag"
                );
                clang_flags.push(format!("{clang_prefix}{name}"));
            }
        }
        index += 1;
    }

    assert!(
        !clang_flags.is_empty(),
        "wasm32-unknown-unknown must configure an explicit Rust target-feature list; \
         the QuickJS C flags are derived from it"
    );
    clang_flags
}

fn rerun_if_changed(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}

fn rerun_if_platform_headers_changed(manifest_dir: &Path) {
    for header in [
        "assert.h",
        "inttypes.h",
        "math.h",
        "stdio.h",
        "stdlib.h",
        "string.h",
    ] {
        rerun_if_changed(&manifest_dir.join("src/include").join(header));
    }
}
