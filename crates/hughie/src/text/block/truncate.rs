//! Cut computation for `text-maxline` / `text-maxlength` truncation.
//!
//! The algorithm is the web reference implementation's slow path
//! (`XTextTruncation.ts`), verified against its source: the maxline candidate
//! is backed off — three units for the dots, fewer on a short line, or by the
//! freed width the inline-truncation content needs — while the maxlength
//! candidate is never backed off; the final cut is the minimum of the two.
//! Truncation content shows only on maxline overflow: a pure maxlength cut
//! gets the dots (or nothing under clip). Retreat works at cluster
//! granularity rather than the web's raw UTF-16 stepping, so a cut never
//! splits a surrogate pair or grapheme — a recorded deviation.

use parley::{Layout, PositionedLayoutItem};

use super::content::{SourceMap, StyledRange};
use super::shape::NaturalLine;
use super::style::{BlockStyle, TextOverflow};
use crate::style::TextBrush;

/// How many units the dots-form ellipsis backs the maxline cut off, and how
/// many dots it appends.
const ELLIPSIS_UNITS: u32 = 3;

/// One decided truncation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text::block) struct CutPlan {
    pub(in crate::text::block) cut_byte: u32,
    pub(in crate::text::block) cut_unit: u32,
    /// Index of the line the cut falls in; lines past it are dropped.
    pub(in crate::text::block) cut_line: u32,
    pub(in crate::text::block) tail: Tail,
    pub(in crate::text::block) truncation_visible: bool,
}

/// What follows the cut in the rebuilt display layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::text::block) enum Tail {
    None,
    /// A literal run of `count` U+002E dots styled as run item `item`.
    Dots {
        count: u32,
        item: u32,
    },
    /// The inline-truncation content.
    Truncation,
}

/// Decides whether and where to cut. `truncation_width` is present exactly
/// when maxline overflowed and truncation content exists.
#[allow(clippy::too_many_arguments, reason = "one decision reads both spaces")]
pub(in crate::text::block) fn plan(
    natural: &Layout<TextBrush>,
    natural_lines: &[NaturalLine],
    text: &str,
    map: &SourceMap,
    ranges: &[StyledRange],
    slot_units: &[u32],
    style: &BlockStyle,
    maxline_overflow: bool,
    truncation_width: Option<f32>,
    width: Option<f32>,
) -> Option<CutPlan> {
    let last = *natural_lines.last()?;
    let consumed_end = last.end_unit;

    let mut truncation_visible = false;
    let mut maxline_dots = None;
    let maxline_cut = if maxline_overflow {
        let line_index = natural_lines.len() - 1;
        Some(if let Some(truncation_width) = truncation_width {
            if width.is_some_and(|container| truncation_width >= container) {
                // Too wide for the container: hide the truncation content and
                // cut at line start. No dots either — the web's clipped
                // marker is suppressed whenever inline-truncation exists.
                last.start_unit
            } else {
                truncation_visible = true;
                retreat_for_width(
                    natural,
                    line_index,
                    last,
                    text,
                    map,
                    slot_units,
                    truncation_width,
                )
            }
        } else if style.overflow == TextOverflow::Ellipsis {
            let units = last.end_unit - last.start_unit;
            if units < ELLIPSIS_UNITS {
                maxline_dots = Some(units);
                last.start_unit
            } else {
                maxline_dots = Some(ELLIPSIS_UNITS);
                last.end_unit - ELLIPSIS_UNITS
            }
        } else {
            // Clip: cut at the line end, nothing backed off, nothing appended.
            last.end_unit
        })
    } else {
        None
    };

    let maxchars_cut = style
        .max_chars
        .filter(|&max_chars| max_chars < consumed_end);

    let cut_unit = match (maxline_cut, maxchars_cut) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };

    let cut_line = natural_lines
        .iter()
        .position(|line| line.end_unit > cut_unit)
        .unwrap_or(natural_lines.len() - 1);
    let cut_byte = map
        .unit_to_byte(text, cut_unit)
        .unwrap_or_else(|| u32::try_from(text.len()).expect("text fits u32"));

    let tail = if truncation_visible {
        Tail::Truncation
    } else if truncation_width.is_some() {
        // Hidden truncation content suppresses the dots entirely.
        Tail::None
    } else if style.overflow == TextOverflow::Ellipsis {
        // A maxlength-won cut keeps the full three dots; the shrunk count
        // applies only when the short maxline line itself is the cut.
        let count = if maxline_cut == Some(cut_unit) {
            maxline_dots.unwrap_or(ELLIPSIS_UNITS)
        } else {
            ELLIPSIS_UNITS
        };
        match (count, dots_item(ranges, cut_byte)) {
            (0, _) | (_, None) => Tail::None,
            (count, Some(item)) => Tail::Dots { count, item },
        }
    } else {
        Tail::None
    };

    Some(CutPlan {
        cut_byte,
        cut_unit,
        cut_line: u32::try_from(cut_line).expect("line count fits u32"),
        tail,
        truncation_visible,
    })
}

/// Backs the cut off from the line end until the removed atoms free at least
/// `needed` width — the web's fitting loop, at cluster granularity. Always
/// removes at least one atom; lands on the line start when even the whole
/// line does not free enough.
fn retreat_for_width(
    natural: &Layout<TextBrush>,
    line_index: usize,
    line: NaturalLine,
    text: &str,
    map: &SourceMap,
    slot_units: &[u32],
    needed: f32,
) -> u32 {
    let parley_line = natural.get(line_index).expect("the cut line is committed");
    let mut atoms = Vec::new();
    for run in parley_line.runs() {
        for cluster in run.clusters() {
            let start = u32::try_from(cluster.text_range().start).expect("text fits u32");
            atoms.push((map.byte_to_unit(text, start), cluster.advance()));
        }
    }
    for item in parley_line.items() {
        if let PositionedLayoutItem::InlineBox(inline_box) = item {
            let slot = usize::try_from(inline_box.id).expect("slot ids are table indexes");
            atoms.push((slot_units[slot], inline_box.width));
        }
    }
    atoms.sort_unstable_by_key(|atom| atom.0);

    let mut freed = 0.0;
    let mut cut = line.start_unit;
    for &(unit, advance) in atoms.iter().rev() {
        freed += advance;
        cut = unit;
        if freed >= needed {
            break;
        }
    }
    cut
}

/// The run whose style the dots inherit: the one containing the last visible
/// byte, with the first run as the fallback for a cut at the very start.
/// `None` — a paragraph with no text runs at all — suppresses the dots.
fn dots_item(ranges: &[StyledRange], cut_byte: u32) -> Option<u32> {
    if cut_byte > 0
        && let Some(range) = ranges
            .iter()
            .find(|range| range.bytes.start < cut_byte && cut_byte <= range.bytes.end)
    {
        return Some(range.item);
    }
    ranges.first().map(|range| range.item)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn the_dots_inherit_the_style_of_the_run_containing_the_cut() {
        let ranges = [
            StyledRange {
                bytes: 0..4,
                item: 0,
            },
            StyledRange {
                bytes: 4..9,
                item: 2,
            },
        ];
        assert_eq!(dots_item(&ranges, 3), Some(0));
        assert_eq!(dots_item(&ranges, 4), Some(0));
        assert_eq!(dots_item(&ranges, 5), Some(2));
        assert_eq!(
            dots_item(&ranges, 0),
            Some(0),
            "cut at start falls back to the first run"
        );
        assert_eq!(
            dots_item(&[], 0),
            None,
            "a paragraph without runs has no dots style"
        );
    }
}
