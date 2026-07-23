//! Dumps a decoded `.web.bundle` as JSON for inspection / cross-validation
//! against the reference `@lynx-js/web-core` decoder.

use serde_json::json;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump <bundle>");
    let bytes = std::fs::read(&path).expect("read bundle");
    let template = lynx_template_decoder::decode(&bytes).expect("decode bundle");

    let lepus: serde_json::Map<String, serde_json::Value> = template
        .lepus_code
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                json!({
                    "byteLength": v.len(),
                    "prefix": v.chars().take(60).collect::<String>(),
                }),
            )
        })
        .collect();

    let style: Vec<serde_json::Value> = template
        .style_info
        .iter()
        .flat_map(|info| info.css_id_to_style_sheet.iter())
        .map(|(css_id, sheet)| {
            let rules: Vec<serde_json::Value> = sheet
                .rules
                .iter()
                .map(|rule| {
                    json!({
                        "type": format!("{:?}", rule.kind),
                        "selectors": rule
                            .prelude
                            .selectors
                            .iter()
                            .map(lynx_template_decoder::style_info::Selector::to_css_string)
                            .collect::<Vec<_>>(),
                        "declarations": rule
                            .declaration_block
                            .declarations
                            .iter()
                            .map(|d| format!("{}:{}", d.property.name(), d.value_text()))
                            .collect::<Vec<_>>(),
                        "nested": rule.children.len(),
                    })
                })
                .collect();
            json!({ "cssId": css_id, "imports": sheet.imports, "rules": rules })
        })
        .collect();

    let summary = json!({
        "version": template.version,
        "config": template.config,
        "lepusCode": lepus,
        "manifest": template.manifest.keys().collect::<Vec<_>>(),
        "customSections": template.custom_sections,
        "styleInfo": style,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
}
