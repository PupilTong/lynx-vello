//! The input as a graph of up to eight ES modules, `fuzz:0` to `fuzz:7`: the
//! text split at NUL bytes, one module each, importing one another by those
//! names. The graph loads twice, the way the code cache sees it:
//!
//! 1. into a realm that fetches each module when something imports it — the host loader's requests,
//!    completions and resumed loads — compiling each text and capturing its bytecode
//!    (`JS_WriteObject`);
//! 2. into a realm of a fresh runtime handed that bytecode up front, which it reads
//!    (`JS_ReadObject`) instead of compiling.
//!
//! Every module is loaded twice in each realm, so a graph that failed to
//! link is linked again.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quickjs_rust_bridge::{Compiled, Context, ModuleCode, Runtime};
use quickjs_rust_bridge_fuzz::{runtime, settle};

const MODULES: usize = 8;

/// Requests answered per load before the rest are left pending.
const LOAD_ROUNDS: usize = 16;

fuzz_target!(|data: &[u8]| {
    let texts: Vec<String> = data
        .split(|&byte| byte == 0)
        .take(MODULES)
        .map(|text| String::from_utf8_lossy(text).into_owned())
        .collect();
    let captured = load_from_text(&texts);
    load_from_bytecode(&texts, &captured);
});

fn name(index: usize) -> String {
    format!("fuzz:{index}")
}

fn text_of<'a>(texts: &'a [String], name: &str) -> Option<&'a str> {
    let index: usize = name.strip_prefix("fuzz:")?.parse().ok()?;
    texts.get(index).map(String::as_str)
}

fn normalize(_base: &str, specifier: &str) -> Result<String, String> {
    Ok(specifier.to_owned())
}

fn load_from_text(texts: &[String]) -> Vec<Compiled> {
    let Some(mut runtime) = runtime() else {
        return Vec::new();
    };
    let Ok(mut realm) = runtime.create_context() else {
        return Vec::new();
    };
    realm.enable_module_loading(normalize);
    for _ in 0..2 {
        for index in 0..texts.len() {
            let _ = realm.load_module(&name(index));
            serve(&mut runtime, &realm, texts);
        }
    }
    let captured = std::iter::from_fn(|| realm.take_compiled()).collect();
    drop(realm);
    drop(runtime);
    captured
}

/// Answers the realm's module requests until it asks for nothing more.
fn serve(runtime: &mut Runtime, realm: &Context, texts: &[String]) {
    for _ in 0..LOAD_ROUNDS {
        settle(runtime, realm);
        let mut answered = false;
        while let Some(request) = realm.take_module_request() {
            answered = true;
            let _ = match text_of(texts, &request) {
                Some(text) => {
                    realm.complete_module_with(&request, &request, text, ModuleCode::Capture)
                }
                None => realm.complete_module(&request, Err("no such module")),
            };
        }
        if !answered {
            return;
        }
        let _ = realm.resume_module_loads();
    }
    settle(runtime, realm);
}

fn load_from_bytecode(texts: &[String], captured: &[Compiled]) {
    let Some(mut runtime) = runtime() else {
        return;
    };
    let Ok(realm) = runtime.create_context() else {
        return;
    };
    for (index, text) in texts.iter().enumerate() {
        let name = name(index);
        let code = captured
            .iter()
            .find(|compiled| compiled.name == name)
            .map_or(ModuleCode::Compile, |compiled| {
                ModuleCode::Cached(&compiled.bytecode)
            });
        let _ = realm.complete_module_with(&name, &name, text, code);
    }
    for _ in 0..2 {
        for index in 0..texts.len() {
            let _ = realm.load_module(&name(index));
            settle(&mut runtime, &realm);
        }
    }
    drop(realm);
    drop(runtime);
}
