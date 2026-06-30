//! Encoder version parsing and the feature gates the decoder cares about.
//!
//! The bundle header carries version strings; several body fields only exist at
//! or above a given version (the C++ `base::Version` comparisons). We model a
//! version as up to four numeric components compared lexicographically, and
//! expose the specific gates referenced by the decoder as named constants.

/// A semantic-ish version: up to four numeric components (`a.b.c.d`).
///
/// Parsing is lenient — it reads the leading numeric run of each `.`-separated
/// component and ignores any pre-release/build suffix (e.g. `2.14.0-rc1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version([u16; 4]);

impl Version {
    /// Construct from explicit components.
    #[must_use]
    pub const fn new(a: u16, b: u16, c: u16, d: u16) -> Self {
        Self([a, b, c, d])
    }

    /// Construct from a major/minor pair (the common case for gates).
    #[must_use]
    pub const fn major_minor(a: u16, b: u16) -> Self {
        Self([a, b, 0, 0])
    }

    /// Parse a version string. Unparseable components decode to `0`, so a fully
    /// empty/garbage string yields `0.0.0.0` rather than an error.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        let mut parts = [0u16; 4];
        for (slot, comp) in parts.iter_mut().zip(s.split('.')) {
            let digits: String = comp.chars().take_while(char::is_ascii_digit).collect();
            *slot = digits.parse().unwrap_or(0);
        }
        Self(parts)
    }

    /// The four numeric components.
    #[must_use]
    pub const fn components(self) -> [u16; 4] {
        self.0
    }

    /// Whether `self` is greater than or equal to `gate`.
    #[must_use]
    pub fn is_at_least(self, gate: Version) -> bool {
        self >= gate
    }
}

/// `header_ext_info` / compile options appear at or above this version.
pub const V_1_6: Version = Version::major_minor(1, 6);
/// Baseline for the LepusNG flexible-template era.
pub const V_2_0: Version = Version::major_minor(2, 0);
/// Header-mode `template_info` value appears at or above this version.
pub const V_2_7: Version = Version::major_minor(2, 7);
/// Misc. body additions land at this version.
pub const V_2_8: Version = Version::major_minor(2, 8);
/// CSS variable multi-default-value encoding appears at or above this version.
pub const V_2_14: Version = Version::major_minor(2, 14);
/// CSS parse-token `important` flag appears at or above this version.
pub const V_3_9: Version = Version::major_minor(3, 9);

#[cfg(test)]
mod tests {
    use super::{V_1_6, V_2_7, V_2_14, Version};

    #[test]
    fn parses_components() {
        assert_eq!(Version::parse("2.14.0.3").components(), [2, 14, 0, 3]);
        assert_eq!(Version::parse("3.9").components(), [3, 9, 0, 0]);
        assert_eq!(Version::parse("2.14.0-rc1").components(), [2, 14, 0, 0]);
        assert_eq!(Version::parse("").components(), [0, 0, 0, 0]);
    }

    #[test]
    fn gate_comparisons() {
        assert!(Version::parse("2.14.0").is_at_least(V_2_14));
        assert!(!Version::parse("2.13.9").is_at_least(V_2_14));
        assert!(Version::parse("3.0").is_at_least(V_2_7));
        assert!(Version::parse("1.6.0").is_at_least(V_1_6));
        assert!(!Version::parse("1.5.9").is_at_least(V_1_6));
    }
}
