//! The writer half of the `<utf16Length>:<text>` record format every host
//! answer with more than one string field uses: an element's attribute
//! names, its computed style, a component method's data and the native
//! module table. `bobcat:record`'s `splitRecord` is the one reader in a
//! realm, and the runtime's `take_record_field` reads the records a realm
//! sends the other way.
//!
//! Its own module because both halves of this thread write records: the
//! runtime's host members, and the components in [`super::tree`], which
//! answer a UI method's data as a record and do not depend on the runtime.

use std::fmt::Write as _;

/// Appends one `<units>:<text>` field, the runtime's `take_record_field`'s
/// inverse; the count is in UTF-16 code units because
/// `String.prototype.slice` consumes it.
pub(crate) fn write_record_field(record: &mut String, text: &str) {
    let units: usize = text.chars().map(char::len_utf16).sum();
    write!(record, "{units}:").expect("writing to a String cannot fail");
    record.push_str(text);
}
