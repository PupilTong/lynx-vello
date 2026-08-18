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
    for header in [
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
    ] {
        rerun_if_changed(&quickjs_dir.join(header));
    }
}

fn assert_supported_target(target_os: &str, target_env: &str) {
    assert!(
        target_os != "windows" || target_env == "gnu",
        "the pinned QuickJS C sources support Windows through GNU/MinGW, not the MSVC ABI"
    );
}

fn configure_target(build: &mut cc::Build, target: &str, clang_target_feature_flags: &[String]) {
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
