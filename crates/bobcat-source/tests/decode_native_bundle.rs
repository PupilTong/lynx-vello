//! Verify the compiler's source-section output, including production bundles.

#[path = "../../../packages/reactlynx-test-fixtures/fixtures.rs"]
mod fixtures;

use bobcat_source::{native, web};

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn compiled_pages_and_lazy_chunks_contain_source() {
    for name in [
        "react-data-processor",
        "react-global-props",
        "react-global-props-development",
        "react-lazy",
        "react-lazy-sync",
        "react-lazy-nested",
        "react-lazy-nested-development",
        "react-list",
        "react-native",
        "react-reload",
        "react-reload-development",
    ] {
        let fixture = fixtures::fixture(name);
        // decode rejects executable root bytecode and JsBytecode sections.
        let page = native::decode(fixture.page).unwrap_or_else(|error| panic!("{name}: {error}"));
        let entry = name.trim_end_matches("-development");
        let main_thread = &page.lepus_code[&format!("{entry}__main-thread")];
        assert!(!main_thread.is_empty(), "{name}: missing MTS source");
        assert!(!page.lepus_code.contains_key("root"));
        assert!(
            !page.manifest["/app-service.js"].is_empty(),
            "{name}: missing BTS bootstrap"
        );
        assert!(page.manifest.len() > 1, "{name}: missing BTS modules");

        for &(path, bytes) in fixture.chunks {
            let chunk =
                native::decode(bytes).unwrap_or_else(|error| panic!("{name}/{path}: {error}"));
            let sections = chunk.custom_sections.as_ref().unwrap();
            assert!(
                sections["background"]["content"].is_string()
                    || sections["main-thread"]["content"].is_string(),
                "{name}/{path}: missing FetchBundle source"
            );
        }

        let converted = web::decode(&native::convert(fixture.page).unwrap()).unwrap();
        assert_eq!(converted.lepus_code, page.lepus_code);
        assert_eq!(converted.manifest, page.manifest);
        assert_eq!(converted.custom_sections, page.custom_sections);
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn compiled_lazy_source_sections_preserve_css() {
    let fixture = fixtures::fixture("react-lazy");
    let chunk = native::decode(fixture.chunks[0].1).unwrap();
    let sections = chunk.custom_sections.as_ref().unwrap();
    assert_eq!(sections["CSS"]["encoding"], "CSS");
    assert!(sections["CSS"]["content"].to_string().contains(".lazy-box"));
}
