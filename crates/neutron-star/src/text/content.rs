//! CSS whitespace processing and shaped-run range assembly.

use core::ops::Range;

use stylo::computed_values::white_space_collapse;

use crate::style::{TextRun, TextRunStyle};

/// Whitespace-normalized paragraph content and its non-overlapping run ranges.
pub(super) struct ShapingContent<'a, R: TextRunStyle> {
    pub(super) text: String,
    pub(super) ranges: Vec<StyledRange<'a, R>>,
}

/// One contiguous range in normalized UTF-8 text carrying a host run style.
pub(super) struct StyledRange<'a, R: TextRunStyle> {
    pub(super) bytes: Range<usize>,
    pub(super) style: &'a R,
}

/// Applies the container's `white-space-collapse` mode before Parley shapes
/// the paragraph.
///
/// Under `collapse`, CSS document whitespace (space, tab, CR-as-space, and LF
/// segment breaks) is collapsed to one ASCII space. Runs marked
/// `preserve_newlines` — and every run under `preserve-breaks` — retain LF
/// hard breaks while still collapsing other whitespace; spaces immediately
/// adjacent to those breaks are removed. Under `preserve`/`break-spaces` the
/// text passes through unmodified (newlines stay forced breaks; the
/// `break-spaces` end-of-line wrapping refinement is out of measurement
/// scope). Other controls, non-breaking spaces, and other Unicode spaces are
/// deliberately passed through for Parley to render or otherwise process.
pub(super) fn normalize_runs<'a, R, Runs>(
    runs: Runs,
    collapse: white_space_collapse::T,
) -> ShapingContent<'a, R>
where
    R: TextRunStyle + 'a,
    Runs: Iterator<Item = TextRun<'a, R>>,
{
    let mut content = ShapingContent {
        text: String::new(),
        ranges: Vec::new(),
    };

    if matches!(
        collapse,
        white_space_collapse::T::Preserve | white_space_collapse::T::BreakSpaces
    ) {
        for run in runs {
            for character in run.text.chars() {
                content.push(character, run.style);
            }
        }
        return content;
    }
    let force_preserved_breaks = collapse == white_space_collapse::T::PreserveBreaks;

    let mut pending_whitespace = None;
    let mut after_preserved_break = false;

    for run in runs {
        let preserve_newlines = run.preserve_newlines || force_preserved_breaks;
        for character in run.text.chars() {
            match character {
                '\n' => queue_segment_break(
                    &mut content,
                    &mut pending_whitespace,
                    &mut after_preserved_break,
                    run.style,
                    preserve_newlines,
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

    if !content.text.is_empty()
        && let Some(whitespace) = pending_whitespace
    {
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
    use stylo::values::computed::font::{FontFamily, FontFamilyList, SingleFontFamily};

    use super::*;

    #[derive(Debug)]
    struct RunStyle;

    impl TextRunStyle for RunStyle {
        fn font_family(&self) -> FontFamily {
            FontFamily {
                families: FontFamilyList {
                    list: stylo::ArcSlice::from_iter(std::iter::empty::<SingleFontFamily>()),
                },
                is_system_font: false,
                is_initial: false,
            }
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

        let content = normalize_runs(runs.into_iter(), white_space_collapse::T::Collapse);

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

        let content = normalize_runs(runs.into_iter(), white_space_collapse::T::Collapse);

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
            white_space_collapse::T::Collapse,
        );
        assert_eq!(raw.text, "a\nb");

        let controls = normalize_runs(
            [TextRun {
                text: "a\rb\x0Cc",
                style: &first,
                preserve_newlines: true,
            }]
            .into_iter(),
            white_space_collapse::T::Collapse,
        );
        assert_eq!(controls.text, "a b\x0Cc");

        let chinese = normalize_runs(
            [TextRun {
                text: "你\n好",
                style: &first,
                preserve_newlines: false,
            }]
            .into_iter(),
            white_space_collapse::T::Collapse,
        );
        assert_eq!(chinese.text, "你好");

        let korean = normalize_runs(
            [TextRun {
                text: "안\n녕",
                style: &first,
                preserve_newlines: false,
            }]
            .into_iter(),
            white_space_collapse::T::Collapse,
        );
        assert_eq!(korean.text, "안 녕");

        let zero_width_space = normalize_runs(
            [TextRun {
                text: "a\n\u{200B}b",
                style: &first,
                preserve_newlines: false,
            }]
            .into_iter(),
            white_space_collapse::T::Collapse,
        );
        assert_eq!(zero_width_space.text, "a\u{200B}b");
    }

    #[test]
    fn preserve_modes_pass_text_through_unmodified() {
        let style = RunStyle;
        for collapse in [
            white_space_collapse::T::Preserve,
            white_space_collapse::T::BreakSpaces,
        ] {
            let content = normalize_runs(
                [TextRun {
                    text: "a  \t b\n c",
                    style: &style,
                    preserve_newlines: false,
                }]
                .into_iter(),
                collapse,
            );
            assert_eq!(content.text, "a  \t b\n c");
            assert_eq!(content.ranges.len(), 1);
            assert_eq!(content.ranges[0].bytes, 0..9);
        }
    }

    #[test]
    fn trailing_collapsible_whitespace_keeps_one_space() {
        // Phase-one collapsing keeps a single trailing space; trimming it is
        // the line layout phase's job, not normalization's.
        let style = RunStyle;
        let content = normalize_runs(
            [TextRun {
                text: "a b \t ",
                style: &style,
                preserve_newlines: false,
            }]
            .into_iter(),
            white_space_collapse::T::Collapse,
        );
        assert_eq!(content.text, "a b ");
        assert_eq!(content.ranges[0].bytes, 0..4);
    }

    #[test]
    fn segment_break_removal_covers_supplementary_east_asian_blocks() {
        // Chromium's heuristic removes a collapsible segment break between
        // East Asian wide characters across every relevant block, including
        // compatibility, vertical/halfwidth forms, and the supplementary
        // planes.
        let style = RunStyle;
        for (source, expected) in [
            ("\u{F900}\n\u{F900}", "\u{F900}\u{F900}"), // CJK Compatibility Ideographs
            ("\u{FE10}\n\u{FE10}", "\u{FE10}\u{FE10}"), // Vertical Forms
            ("\u{FE30}\n\u{FE30}", "\u{FE30}\u{FE30}"), // CJK Compatibility Forms
            ("\u{FF01}\n\u{FF01}", "\u{FF01}\u{FF01}"), // Fullwidth Forms
            ("\u{FFE0}\n\u{FFE0}", "\u{FFE0}\u{FFE0}"), // Fullwidth Signs
            ("\u{17000}\n\u{17000}", "\u{17000}\u{17000}"), // Tangut
            ("\u{1AFF0}\n\u{1AFF0}", "\u{1AFF0}\u{1AFF0}"), // Kana Extended-B
            ("\u{1F200}\n\u{1F200}", "\u{1F200}\u{1F200}"), // Enclosed Ideographic Supplement
            ("\u{20000}\n\u{20000}", "\u{20000}\u{20000}"), // CJK Extension B
        ] {
            let content = normalize_runs(
                [TextRun {
                    text: source,
                    style: &style,
                    preserve_newlines: false,
                }]
                .into_iter(),
                white_space_collapse::T::Collapse,
            );
            assert_eq!(content.text, expected, "for source {source:?}");
        }
    }

    #[test]
    fn remove_trailing_space_shrinks_and_drops_emptied_ranges() {
        let style = RunStyle;
        let mut content = ShapingContent::<'_, RunStyle> {
            text: String::new(),
            ranges: Vec::new(),
        };
        content.push('a', &style);
        content.push(' ', &style);
        content.remove_trailing_space();
        assert_eq!(content.text, "a");
        assert_eq!(content.ranges.len(), 1);
        assert_eq!(content.ranges[0].bytes, 0..1);

        // A lone-space range disappears entirely instead of surviving empty.
        let mut only_space = ShapingContent::<'_, RunStyle> {
            text: String::new(),
            ranges: Vec::new(),
        };
        only_space.push(' ', &style);
        only_space.remove_trailing_space();
        assert!(only_space.text.is_empty());
        assert!(only_space.ranges.is_empty());

        // Without a trailing ASCII space the call is a no-op.
        content.remove_trailing_space();
        assert_eq!(content.text, "a");
    }

    #[test]
    fn fully_collapsible_and_empty_inputs_produce_no_shaping_ranges() {
        let first = RunStyle;
        let second = RunStyle;
        let whitespace = normalize_runs(
            [
                TextRun {
                    text: " \t\r",
                    style: &first,
                    preserve_newlines: false,
                },
                TextRun {
                    text: "\n ",
                    style: &second,
                    preserve_newlines: false,
                },
            ]
            .into_iter(),
            white_space_collapse::T::Collapse,
        );
        let no_runs = normalize_runs(
            core::iter::empty::<TextRun<'_, RunStyle>>(),
            white_space_collapse::T::Collapse,
        );

        assert!(whitespace.text.is_empty());
        assert!(whitespace.ranges.is_empty());
        assert!(no_runs.text.is_empty());
        assert!(no_runs.ranges.is_empty());
    }
}
