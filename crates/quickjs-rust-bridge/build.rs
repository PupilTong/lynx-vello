use std::path::{Path, PathBuf};
use std::{env, fs};

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo always provides CARGO_MANIFEST_DIR"),
    );
    let quickjs_dir = manifest_dir.join("../../vendor/quickjs");
    let version_path = quickjs_dir.join("VERSION");
    let version = fs::read_to_string(&version_path)
        .expect("the pinned QuickJS submodule must contain VERSION");
    let version_define = format!("\"{}\"", version.trim());
    let platform_header = manifest_dir.join("src/rust-platform.h");
    let target = env::var("TARGET").expect("Cargo always provides TARGET");
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo always provides target OS");
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    assert!(
        target_os != "windows" || target_env == "gnu",
        "the pinned QuickJS C sources support Windows through GNU/MinGW, not the MSVC ABI"
    );

    let sources = [
        "quickjs.c",
        "dtoa.c",
        "libregexp.c",
        "libunicode.c",
        "cutils.c",
    ];

    let configure_build = || {
        let mut build = cc::Build::new();
        build
            .define("CONFIG_VERSION", Some(version_define.as_str()))
            .define("QJS_NO_JS_SHARED_MEMORY", None)
            .define("QJS_RUST_ALLOCATOR", None)
            .define("QJS_RUST_TIME_HOST", None)
            .define("QJS_RUST_TIMEZONE_HOST", None)
            .include(manifest_dir.join("src/include"))
            .flag("-include")
            .flag(&platform_header)
            .flag_if_supported("-std=gnu11")
            .flag_if_supported("-fwrapv");
        if target_os == "windows" {
            build.define("__USE_MINGW_ANSI_STDIO", None);
        } else {
            build.define("_GNU_SOURCE", None);
        }
        if target == "wasm32-unknown-unknown" {
            // Match the Rust target features in `.cargo/config.toml`. These
            // flags describe the C object code; `QJS_NO_JS_SHARED_MEMORY`
            // above still omits JS Atomics and SAB.
            build
                .flag("-matomics")
                .flag("-mbulk-memory")
                .flag("-mbulk-memory-opt")
                .flag("-mextended-const")
                .flag("-mmultivalue")
                .flag("-mmutable-globals")
                .flag("-mnontrapping-fptoint")
                .flag("-mreference-types")
                .flag("-mrelaxed-simd")
                .flag("-msign-ext")
                .flag("-msimd128")
                .flag("-mtail-call");
        }
        build
    };

    let mut shim_build = configure_build();
    shim_build
        .flag("-isystem")
        .flag(&quickjs_dir)
        .file(manifest_dir.join("src/shim.c"))
        .warnings(true)
        .warnings_into_errors(true)
        .compile("quickjs_bridge_shim");

    let mut quickjs_build = configure_build();
    quickjs_build.include(&quickjs_dir).warnings(false);
    for source in sources {
        quickjs_build.file(quickjs_dir.join(source));
    }
    quickjs_build.compile("quickjs_bridge_core");

    if env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        println!("cargo:rustc-link-lib=m");
    }

    println!("cargo:rerun-if-changed={}", version_path.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src/shim.c").display()
    );
    rerun_if_changed(&platform_header);
    rerun_if_changed(&manifest_dir.join("src/rust-allocator.h"));
    rerun_if_changed(&manifest_dir.join("src/include/assert.h"));
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

fn rerun_if_changed(path: &Path) {
    println!("cargo:rerun-if-changed={}", path.display());
}
