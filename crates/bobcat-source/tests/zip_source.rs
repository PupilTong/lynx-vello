//! Identical archive selection/registration cases run natively and in Wasm.
use std::io::{Cursor, Write};

use bobcat_resources::{Resources, ResourcesConfig};
use bobcat_source::{ZipSource, ZipSourceError};
use url::Url;
use zip::write::SimpleFileOptions;

const XML: &[u8] = b"<lynx engine-version=\"4.2\"><script thread=\"main\">main</script></lynx>";

fn archive(entries: &[(&str, &[u8])], method: zip::CompressionMethod) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn resources() -> Resources {
    Resources::new(
        ResourcesConfig {
            worker_threads: 1,
            log_to_stderr: false,
            ..ResourcesConfig::default()
        },
        || {},
    )
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn loads_xml_and_registers_encoded_resource_paths_on_both_hosts() {
    for method in [
        zip::CompressionMethod::Stored,
        zip::CompressionMethod::Deflated,
    ] {
        let bytes = archive(
            &[("dist/card.xml", XML), ("dist/images/a #%.svg", b"<svg/>")],
            method,
        );
        for url in [
            "bobcat-memory://archive/dist/card.xml",
            "https://cdn.example/dist/card.xml?version=2#entry",
            "file:///dist/card.xml",
        ] {
            let input = Url::parse(url).unwrap();
            let zip = ZipSource::from_bytes(&bytes).unwrap();
            let page = zip.page(&input).unwrap();
            assert_eq!(page.input_url(), &input);
            assert!(page.config().enable_css_selector);
            let resources = resources();
            zip.register_with(&resources, &input).unwrap();
            page.register_with(&resources);
            assert!(resources.unregister(&page.view_sources().entry));
            let mut asset = input.clone();
            asset.set_query(None);
            asset.set_fragment(None);
            asset
                .path_segments_mut()
                .unwrap()
                .clear()
                .extend(["dist", "images", "a #%.svg"]);
            assert!(resources.unregister(asset.as_str()));
        }
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn selects_real_binary_bundle_through_the_shared_page_adapter() {
    let bytes = archive(
        &[(
            "dist/main.web.bundle",
            include_bytes!("fixtures/basic-class-selector.web.bundle"),
        )],
        zip::CompressionMethod::Deflated,
    );
    let zip = ZipSource::from_bytes(&bytes).unwrap();
    let page = zip
        .page(&Url::parse("https://cdn.example/dist/main.web.bundle").unwrap())
        .unwrap();
    assert_eq!(
        page.view_sources().entry,
        "bobcat-memory://bundle/lepus-root.js"
    );
    assert!(!page.view_sources().style_sheets.is_empty());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rejects_unsafe_names_before_registration() {
    for name in ["../a", "./a", "/a", "a//b", "a\\b", "a/../b"] {
        let bytes = archive(&[(name, XML)], zip::CompressionMethod::Stored);
        assert!(
            matches!(
                ZipSource::from_bytes(&bytes),
                Err(ZipSourceError::InvalidPath)
            ),
            "{name}"
        );
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn rejects_duplicate_central_directory_names_before_zip_can_deduplicate_them() {
    let mut bytes = archive(&[("a", XML), ("b", XML)], zip::CompressionMethod::Stored);
    for index in 0..bytes.len() - 46 {
        if bytes[index..].starts_with(b"PK\x01\x02") {
            bytes[index + 46] = b'a';
        }
    }
    assert!(matches!(
        ZipSource::from_bytes(&bytes),
        Err(ZipSourceError::DuplicatePath)
    ));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn malformed_archives_and_missing_or_invalid_entries_fail() {
    let bytes = archive(
        &[("dist/card.xml", XML), ("bad.web.bundle", b"invalid")],
        zip::CompressionMethod::Stored,
    );
    for end in [0, 4, bytes.len() - 1] {
        assert!(ZipSource::from_bytes(&bytes[..end]).is_err());
    }
    let zip = ZipSource::from_bytes(&bytes).unwrap();
    assert!(matches!(
        zip.page(&Url::parse("https://example.test/missing.xml").unwrap()),
        Err(ZipSourceError::MissingEntry)
    ));
    assert!(matches!(
        zip.page(&Url::parse("data:text/plain,x").unwrap()),
        Err(ZipSourceError::InvalidEntryUrl)
    ));
    assert!(matches!(
        zip.page(&Url::parse("https://example.test/bad.web.bundle").unwrap()),
        Err(ZipSourceError::Page(_))
    ));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn corrupt_member_fails_crc_instead_of_publishing_partial_bytes() {
    let mut bytes = archive(&[("card.xml", XML)], zip::CompressionMethod::Stored);
    let start = bytes
        .windows(XML.len())
        .position(|window| window == XML)
        .unwrap();
    bytes[start] ^= 1;
    assert!(ZipSource::from_bytes(&bytes).is_err());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn directory_mutations_and_truncations_do_not_panic() {
    let bytes = archive(&[("card.xml", XML)], zip::CompressionMethod::Deflated);
    for end in 0..bytes.len() {
        assert!(ZipSource::from_bytes(&bytes[..end]).is_err());
    }
    for index in 0..bytes.len() {
        for mask in [1, 0x80, 0xff] {
            let mut mutated = bytes.clone();
            mutated[index] ^= mask;
            let _ = ZipSource::from_bytes(&mutated);
        }
    }
    assert!(ZipSource::from_bytes(&bytes).is_ok());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn symbolic_links_are_not_published_as_resources() {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink("alias", "card.xml", SimpleFileOptions::default())
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    assert!(matches!(
        ZipSource::from_bytes(&bytes),
        Err(ZipSourceError::SymbolicLink)
    ));
}
