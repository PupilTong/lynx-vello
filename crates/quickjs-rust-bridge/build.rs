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
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo always provides target OS");
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_vendor = env::var("CARGO_CFG_TARGET_VENDOR").unwrap_or_default();

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

    let mut build = cc::Build::new();
    build
        .include(&quickjs_dir)
        .file(manifest_dir.join("src/shim.c"))
        .define("CONFIG_VERSION", Some(version_define.as_str()))
        .flag_if_supported("-std=gnu11")
        .flag_if_supported("-fwrapv")
        .warnings(false);

    if target_os == "windows" {
        build.define("__USE_MINGW_ANSI_STDIO", None);
    } else {
        build.define("_GNU_SOURCE", None);
    }

    for source in sources {
        build.file(quickjs_dir.join(source));
    }
    build.compile("quickjs_bridge");

    if env::var("CARGO_CFG_TARGET_FAMILY").as_deref() == Ok("unix") {
        println!("cargo:rustc-link-lib=m");
        if target_vendor != "apple" {
            println!("cargo:rustc-link-lib=pthread");
        }
    } else if target_os == "windows" {
        println!("cargo:rustc-link-lib=winpthread");
    }

    println!("cargo:rerun-if-changed={}", version_path.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src/shim.c").display()
    );
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
