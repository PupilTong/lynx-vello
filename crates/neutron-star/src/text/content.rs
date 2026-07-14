//! CSS whitespace processing and shaped-run range assembly.

use core::ops::Range;

use crate::style::{TextRun, TextRunStyle};

/// Whitespace-normalized paragraph content and its non-overlapping run ranges.
pub(super) struct ShapingContent<'a, R: TextRunStyle> {
    pub(super) text: String,
    pub(super) ranges: Vec<StyledRange<'a, R>>,
    pub(super) fallback_style: Option<&'a R>,
}

/// One contiguous range in normalized UTF-8 text carrying a host run style.
pub(super) struct StyledRange<'a, R: TextRunStyle> {
    pub(super) bytes: Range<usize>,
    pub(super) style: &'a R,
}

/// Applies the collapsing shared by Lynx's `white-space: normal` and
/// `white-space: nowrap` before Parley shapes the paragraph.
///
/// CSS document whitespace (space, tab, CR-as-space, and LF segment breaks)
/// is collapsed to one ASCII space. Runs marked `preserve_newlines` retain LF
/// hard breaks while still collapsing other whitespace; spaces immediately
/// adjacent to those breaks are removed. Other controls, non-breaking spaces,
/// and other Unicode spaces are deliberately passed through for Parley to
/// render or otherwise process.
pub(super) fn normalize_runs<'a, R, Runs>(runs: Runs) -> ShapingContent<'a, R>
where
    R: TextRunStyle + 'a,
    Runs: Iterator<Item = TextRun<'a, R>>,
{
    let mut content = ShapingContent {
        text: String::new(),
        ranges: Vec::new(),
        fallback_style: None,
    };
    let mut pending_whitespace = None;
    let mut after_preserved_break = false;

    for run in runs {
        content.fallback_style.get_or_insert(run.style);
        for character in run.text.chars() {
            match character {
                '\n' => queue_segment_break(
                    &mut content,
                    &mut pending_whitespace,
                    &mut after_preserved_break,
                    run.style,
                    run.preserve_newlines,
                ),
                ' ' | '\t' | '\r' => {
                    if !after_preserved_break && pending_whitespace.is_none() {
                        pending_whitespace = Some(PendingWhitespace::Space(run.style));
                    }
                }
                _ => {
                    flush_pending_whitespace(&mut content, &mut pending_whitespace, character);
                    content.push(character, run.style);
                    after_preserved_break = false;
                }
            }
        }
    }

    if let Some(whitespace) = pending_whitespace {
        content.push(' ', whitespace.style());
    }

    content
}

#[derive(Clone, Copy)]
enum PendingWhitespace<'a, R: TextRunStyle> {
    Space(&'a R),
    SegmentBreak(&'a R),
}

impl<'a, R: TextRunStyle> PendingWhitespace<'a, R> {
    const fn style(self) -> &'a R {
        match self {
            Self::Space(style) | Self::SegmentBreak(style) => style,
        }
    }
}

fn queue_segment_break<'a, R: TextRunStyle>(
    content: &mut ShapingContent<'a, R>,
    pending_whitespace: &mut Option<PendingWhitespace<'a, R>>,
    after_preserved_break: &mut bool,
    style: &'a R,
    preserve_newlines: bool,
) {
    if preserve_newlines {
        *pending_whitespace = None;
        content.remove_trailing_space();
        content.push('\n', style);
        *after_preserved_break = true;
    } else if !*after_preserved_break
        && !matches!(pending_whitespace, Some(PendingWhitespace::SegmentBreak(_)))
    {
        *pending_whitespace = Some(PendingWhitespace::SegmentBreak(style));
    }
}

fn flush_pending_whitespace<'a, R: TextRunStyle>(
    content: &mut ShapingContent<'a, R>,
    pending_whitespace: &mut Option<PendingWhitespace<'a, R>>,
    next: char,
) {
    let Some(whitespace) = pending_whitespace.take() else {
        return;
    };
    let remove = matches!(whitespace, PendingWhitespace::SegmentBreak(_))
        && should_remove_segment_break(content.text.chars().next_back(), next);
    if !remove {
        content.push(' ', whitespace.style());
    }
}

fn should_remove_segment_break(previous: Option<char>, next: char) -> bool {
    previous.is_some_and(|character| character == '\u{200B}')
        || next == '\u{200B}'
        || previous.is_some_and(|character| {
            is_east_asian_without_word_separators(character)
                && is_east_asian_without_word_separators(next)
        })
}

// Chromium removes collapsible segment breaks between Chinese/Japanese and
// related East Asian wide characters, but retains them for Hangul (whose
// customary source-line joining uses spaces). These ranges cover the
// non-Hangul scripts and symbols relevant to that heuristic, including the
// supplementary CJK and Kana blocks.
const fn is_east_asian_without_word_separators(character: char) -> bool {
    matches!(
        character as u32,
        0x2E80..=0x312F
            | 0x3190..=0xA4CF
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE1F
            | 0xFE30..=0xFE6F
            | 0xFF01..=0xFF9F
            | 0xFFE0..=0xFFE6
            | 0x16FE0..=0x18D8F
            | 0x1AFF0..=0x1B2FF
            | 0x1F200..=0x1F2FF
            | 0x20000..=0x323AF
    )
}

impl<'a, R: TextRunStyle> ShapingContent<'a, R> {
    fn push(&mut self, character: char, style: &'a R) {
        let start = self.text.len();
        self.text.push(character);
        let end = self.text.len();
        if let Some(last) = self.ranges.last_mut()
            && core::ptr::eq(last.style, style)
        {
            last.bytes.end = end;
        } else {
            self.ranges.push(StyledRange {
                bytes: start..end,
                style,
            });
        }
    }

    fn remove_trailing_space(&mut self) {
        if !self.text.ends_with(' ') {
            return;
        }
        self.text.pop();
        let end = self.text.len();
        if let Some(last) = self.ranges.last_mut() {
            last.bytes.end = end;
            if last.bytes.is_empty() {
                self.ranges.pop();
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::style::{FontFamily, FontFeatureSetting, FontVariationSetting};

    #[derive(Debug)]
    struct RunStyle;

    impl TextRunStyle for RunStyle {
        type FontFamilies<'a> = core::iter::Empty<FontFamily<'a>>;
        type FontFeatureSettings<'a> = core::iter::Empty<FontFeatureSetting>;
        type FontVariationSettings<'a> = core::iter::Empty<FontVariationSetting>;

        fn font_families(&self) -> Self::FontFamilies<'_> {
            core::iter::empty()
        }

        fn font_feature_settings(&self) -> Self::FontFeatureSettings<'_> {
            core::iter::empty()
        }

        fn font_variation_settings(&self) -> Self::FontVariationSettings<'_> {
            core::iter::empty()
        }
    }

    #[test]
    fn collapses_css_whitespace_across_run_boundaries() {
        let first = RunStyle;
        let second = RunStyle;
        let runs = [
            TextRun {
                text: "a \t\r\n",
                style: &first,
                preserve_newlines: false,
            },
            TextRun {
                text: "  b\u{a0}c",
                style: &second,
                preserve_newlines: false,
            },
        ];

        let content = normalize_runs(runs.into_iter());

        assert_eq!(content.text, "a b\u{a0}c");
        assert_eq!(content.ranges.len(), 2);
        assert_eq!(content.ranges[0].bytes, 0..2);
        assert_eq!(content.ranges[1].bytes, 2..6);
    }

    #[test]
    fn raw_text_preserves_breaks_and_removes_adjacent_spaces() {
        let style = RunStyle;
        let runs = [TextRun {
            text: "one \r\n \t two\x0Cthree",
            style: &style,
            preserve_newlines: true,
        }];

        let content = normalize_runs(runs.into_iter());

        assert_eq!(content.text, "one\ntwo\x0Cthree");
        assert_eq!(content.ranges[0].bytes, 0..13);
    }

    #[test]
    fn normalizes_cross_run_crlf_and_chromium_segment_breaks() {
        let first = RunStyle;
        let second = RunStyle;
        let raw = normalize_runs(
            [
                TextRun {
                    text: "a\r",
                    style: &first,
                    preserve_newlines: true,
                },
                TextRun {
                    text: "\nb",
                    style: &second,
                    preserve_newlines: true,
                },
            ]
            .into_iter(),
        );
        assert_eq!(raw.text, "a\nb");

        let controls = normalize_runs(
            [TextRun {
                text: "a\rb\x0Cc",
                style: &first,
                preserve_newlines: true,
            }]
            .into_iter(),
        );
        assert_eq!(controls.text, "a b\x0Cc");

        let chinese = normalize_runs(
            [TextRun {
                text: "你\n好",
                style: &first,
                preserve_newlines: false,
            }]
            .into_iter(),
        );
        assert_eq!(chinese.text, "你好");

        let korean = normalize_runs(
            [TextRun {
                text: "안\n녕",
                style: &first,
                preserve_newlines: false,
            }]
            .into_iter(),
        );
        assert_eq!(korean.text, "안 녕");

        let zero_width_space = normalize_runs(
            [TextRun {
                text: "a\n\u{200B}b",
                style: &first,
                preserve_newlines: false,
            }]
            .into_iter(),
        );
        assert_eq!(zero_width_space.text, "a\u{200B}b");
    }

    #[test]
    fn empty_inputs_retain_an_optional_fallback_style() {
        let style = RunStyle;
        let empty_run = normalize_runs(
            [TextRun {
                text: "",
                style: &style,
                preserve_newlines: false,
            }]
            .into_iter(),
        );
        let no_runs = normalize_runs(core::iter::empty::<TextRun<'_, RunStyle>>());

        assert!(empty_run.text.is_empty());
        assert!(empty_run.fallback_style.is_some());
        assert!(no_runs.fallback_style.is_none());
    }
}
