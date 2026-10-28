//! The page flags a container's `Configurations` section sets, read straight
//! from its JSON text.
//!
//! A page reads five of the section's keys, each switched on only by the
//! string `"true"` ([`super::WebTemplate::config_flag`]). This reads those and
//! skips every other value without building it, so a registration pays for
//! no JSON object it would only drop. The text must still be one JSON object:
//! anything else is the error the full decode reports.

use std::fmt;

use bobcat_core::PageConfig;
use serde::de::{self, Deserialize, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

/// The page flags in `text`, `Ok(None)` when it is JSON but no object, and a
/// JSON error when it is no JSON at all. A key given twice counts as its last
/// value, as in a decoded object.
pub(crate) fn page_config(text: &str) -> Result<Option<PageConfig>, serde_json::Error> {
    serde_json::from_str::<ConfigObject>(text).map(|object| object.0)
}

/// Every flag off, which is what a page whose section omits them all has.
pub(crate) const fn all_off() -> PageConfig {
    PageConfig {
        default_display_linear: false,
        default_overflow_visible: false,
        enable_css_selector: false,
        enable_js_data_processor: false,
        enable_css_inheritance: false,
    }
}

struct ConfigObject(Option<PageConfig>);

impl<'de> Deserialize<'de> for ConfigObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_any(ConfigVisitor)
            .map(ConfigObject)
    }
}

struct ConfigVisitor;

impl<'de> Visitor<'de> for ConfigVisitor {
    type Value = Option<PageConfig>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut config = all_off();
        while let Some(Key(flag)) = map.next_key()? {
            match flag {
                Some(flag) => *flag(&mut config) = map.next_value::<IsTrue>()?.0,
                None => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Some(config))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(None)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }
}

/// The flag field one key names, `None` for a key a page does not read.
struct Key(Option<fn(&mut PageConfig) -> &mut bool>);

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(KeyVisitor)
    }
}

struct KeyVisitor;

impl Visitor<'_> for KeyVisitor {
    type Value = Key;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object key")
    }

    fn visit_str<E: de::Error>(self, key: &str) -> Result<Key, E> {
        Ok(Key(match key {
            "defaultDisplayLinear" => Some(|config| &mut config.default_display_linear),
            "defaultOverflowVisible" => Some(|config| &mut config.default_overflow_visible),
            "enableCSSSelector" => Some(|config| &mut config.enable_css_selector),
            "enableJSDataProcessor" => Some(|config| &mut config.enable_js_data_processor),
            "enableCSSInheritance" => Some(|config| &mut config.enable_css_inheritance),
            _ => None,
        }))
    }
}

/// Whether one value is the string `"true"`; any other value, a `true`
/// boolean included, is not.
struct IsTrue(bool);

impl<'de> Deserialize<'de> for IsTrue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(IsTrueVisitor).map(IsTrue)
    }
}

struct IsTrueVisitor;

impl<'de> Visitor<'de> for IsTrueVisitor {
    type Value = bool;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<bool, E> {
        Ok(value == "true")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<bool, A::Error> {
        IgnoredAny.visit_map(map).map(|_| false)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<bool, A::Error> {
        IgnoredAny.visit_seq(seq).map(|_| false)
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<bool, E> {
        Ok(false)
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<bool, E> {
        Ok(false)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<bool, E> {
        Ok(false)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<bool, E> {
        Ok(false)
    }

    fn visit_unit<E: de::Error>(self) -> Result<bool, E> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the full decode reads for the same text: the decoded object's
    /// `config_flag`s.
    fn decoded(text: &str) -> Option<PageConfig> {
        let serde_json::Value::Object(map) = serde_json::from_str(text).unwrap() else {
            return None;
        };
        let flag = |key: &str| map.get(key).and_then(serde_json::Value::as_str) == Some("true");
        Some(PageConfig {
            default_display_linear: flag("defaultDisplayLinear"),
            default_overflow_visible: flag("defaultOverflowVisible"),
            enable_css_selector: flag("enableCSSSelector"),
            enable_js_data_processor: flag("enableJSDataProcessor"),
            enable_css_inheritance: flag("enableCSSInheritance"),
        })
    }

    #[test]
    fn the_flags_read_as_the_decoded_object_reads_them() {
        for text in [
            "{}",
            r#"{"enableCSSSelector":"true","defaultDisplayLinear":"false"}"#,
            r#"{"enableCSSSelector":true,"enableCSSInheritance":"TRUE"}"#,
            r#"{"enableCSSSelector":"true","enableCSSSelector":"false"}"#,
            r#"{"enableCSSSelector":"false","enableCSSSelector":"true"}"#,
            r#"{"cardType":"react","nested":{"enableCSSSelector":"true"},"list":[1,"x",null]}"#,
            r#"{"enableJSDataProcessor":{"a":"true"},"defaultOverflowVisible":["true"]}"#,
            r#"{"enableCSSInheritance":"true","defaultOverflowVisible":"true"}"#,
            r#"[{"enableCSSSelector":"true"}]"#,
            r#""true""#,
            "null",
            "1.5",
        ] {
            assert_eq!(page_config(text).unwrap(), decoded(text), "{text}");
        }
    }

    #[test]
    fn text_that_is_no_json_is_an_error() {
        for text in ["", "{", r#"{"a":}"#, "{} {}", r#"{"a":1,}"#] {
            assert!(page_config(text).is_err(), "{text}");
        }
    }
}
