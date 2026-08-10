//! The primitive value the Element PAPI carries for attribute and dataset
//! arguments.
//!
//! web-core types these slots as `Cloneable` (dataset) and
//! `string | null | undefined | boolean` (attributes), and the values reaching
//! them from a compiled `ReactLynx` bundle are JavaScript primitives. The
//! `QuickJS` host boundary above this crate is primitives-only for the same
//! reason, so this enum is exactly the set that crosses it.

use std::fmt;

/// A primitive Element-PAPI argument value.
#[derive(Clone, Debug, PartialEq)]
pub enum PapiValue {
    /// JavaScript `null`.
    Null,
    /// JavaScript `undefined`.
    Undefined,
    Boolean(bool),
    Number(f64),
    String(String),
}

impl PapiValue {
    /// Whether the value clears rather than sets.
    ///
    /// web-core's `__SetAttribute` funnels into
    /// `setElementPropertyOrAttribute`, which removes the attribute for
    /// `null`/`undefined` and writes `String(value)` otherwise — so `false`
    /// and `0` are *written*, as `"false"` and `"0"`.
    #[must_use]
    pub const fn is_nullish(&self) -> bool {
        matches!(self, Self::Null | Self::Undefined)
    }

    /// Whether the value is falsy by JavaScript's `if (value)` test.
    ///
    /// `__AddDataset` uses exactly this test to decide whether to write or
    /// remove the mirrored `data-*` attribute, while storing the value in the
    /// engine's dataset either way (`createElementAPI.ts:426-437`).
    #[must_use]
    pub fn is_falsy(&self) -> bool {
        match self {
            Self::Null | Self::Undefined => true,
            Self::Boolean(value) => !value,
            // JavaScript: `0`, `-0` and `NaN` are falsy.
            Self::Number(value) => *value == 0.0 || value.is_nan(),
            Self::String(value) => value.is_empty(),
        }
    }
}

impl fmt::Display for PapiValue {
    /// ECMAScript `String(value)`, which is what every web-core call site
    /// applies before the value reaches the DOM.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => formatter.write_str("null"),
            Self::Undefined => formatter.write_str("undefined"),
            Self::Boolean(value) => write!(formatter, "{value}"),
            Self::Number(value) => formatter.write_str(&number_to_string(*value)),
            Self::String(value) => formatter.write_str(value),
        }
    }
}

/// ECMAScript `Number::toString` for the cases an Element-PAPI argument can
/// actually take.
///
/// Rust's `{}` for `f64` already prints integral values without a fractional
/// part (`1` rather than `1.0`) and uses the shortest round-tripping form, so
/// only the three non-finite spellings and negative zero need handling.
fn number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if value == 0.0 {
        // ECMAScript prints both `0` and `-0` as "0".
        return "0".to_owned();
    }
    format!("{value}")
}

#[cfg(test)]
mod tests {
    use super::PapiValue;

    #[test]
    fn stringification_matches_javascript_string_coercion() {
        assert_eq!(PapiValue::Boolean(true).to_string(), "true");
        assert_eq!(PapiValue::Number(1.0).to_string(), "1");
        assert_eq!(PapiValue::Number(1.5).to_string(), "1.5");
        assert_eq!(PapiValue::Number(-0.0).to_string(), "0");
        assert_eq!(PapiValue::Number(f64::NAN).to_string(), "NaN");
        assert_eq!(PapiValue::Number(f64::INFINITY).to_string(), "Infinity");
        assert_eq!(PapiValue::String("x".to_owned()).to_string(), "x");
        assert_eq!(PapiValue::Null.to_string(), "null");
    }

    #[test]
    fn falsiness_follows_javascript_not_nullishness() {
        assert!(PapiValue::Number(0.0).is_falsy());
        assert!(!PapiValue::Number(0.0).is_nullish());
        assert!(PapiValue::Boolean(false).is_falsy());
        assert!(PapiValue::String(String::new()).is_falsy());
        assert!(!PapiValue::String("0".to_owned()).is_falsy());
        assert!(PapiValue::Undefined.is_nullish());
        assert!(PapiValue::Null.is_nullish());
    }
}
