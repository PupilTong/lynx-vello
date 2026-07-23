//! CSS containment: effective-containment derivation.

use stylo::properties::ComputedValues;
pub use stylo::values::computed::{Contain, ContentVisibility};

#[must_use]
pub fn effective_containment(style: &ComputedValues, skipped_contents: bool) -> Contain {
    let mut contain = style.clone_contain();
    match style.clone_content_visibility() {
        ContentVisibility::Visible => {}
        ContentVisibility::Auto => {
            contain.insert(Contain::LAYOUT | Contain::PAINT | Contain::STYLE);
            if skipped_contents {
                contain.insert(Contain::SIZE);
            }
        }
        ContentVisibility::Hidden => {
            contain.insert(Contain::LAYOUT | Contain::PAINT | Contain::SIZE | Contain::STYLE);
        }
    }
    contain
}
